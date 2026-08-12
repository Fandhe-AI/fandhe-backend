#!/usr/bin/env bash
# TASK-1.6-1（#71）性能受け入れ判定オーケストレータ。
#
# このスクリプトの役割:
#   `crates/axum-ref`（baseline）と `crates/core` 側フルスクラッチ実装（対象、CORE_BIN）を
#   同一パラメータで順に計測し、`docs/spec/04-requirements.md` REQ-1・NFR-1・NFR-2 の
#   axum 比基準（RPS 90% 以上・p95/p99 110% 以内・アイドル RSS 110% 以内・
#   バイナリサイズ同等以下・起動時間絶対差 20ms 未満）を判定する受け入れテストとして機能する。
#   1 件でも FAIL があれば非 0 で終了し、CI・手動実行の両方で受け入れ失敗を検知できる。
#
# 呼び出し元: 開発者が手動実行する想定（同一ホスト計測ノイズのため CI には組み込まない、
#   benches/README.md）。`bench-http.sh` / `bench-rss.sh` / `bench-footprint.sh` を
#   `RESULT_JSON=<tmp>` 付きでサブプロセスとして呼び出し、機械可読 JSON を読み取る
#   （stdout テキストのパースは行わない。lib/common.sh の write_result_json 契約）。
#
# 前提: `crates/core` 側にエンドポイント等価な計測用バイナリ（`CORE_BIN`）が存在すること。
#   TASK-1.6-3（#168）で `crates/core/examples/core-bench.rs`（axum-ref と機能等価な
#   4 エンドポイントを提供する example）を追加し、この前提を満たした。`CORE_BIN` が
#   見つからない場合（example のビルド漏れ等）は「コア側計測はブロック中」として明示し、
#   判定を実施せずに終了する（安全側に倒す）。
#
# `SKIP_BUILD=1` を指定すると、本スクリプト内部の `cargo build` 2 件（workspace 一括・
# core-bench example）を実行しない（既定は毎回ビルドする）。`benches/bench-accept-exclusive.sh`
# は専有ロック取得後にビルドが走ると静穏（quiescence）確認の意味が失われるため、ロック取得前に
# 事前ビルドを済ませたうえで `SKIP_BUILD=1` を付けて本スクリプトを呼び出す
# （イシュー #260 Bugbot 指摘対応）。`SKIP_BUILD=1` 指定時にバイナリが存在しない場合は
# 通常どおり後続のバイナリ存在チェックで検出され、判定不能として扱われる。
#
# 使い方・パラメータは benches/README.md を参照。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

# --- 計測パラメータ（env で上書き可能。既定は lib/common.sh の RUNS/DURATION/CONNECTIONS を継承） ---
# 既定値は BENCH_TARGET_DIR（lib/common.sh が導出する実効 target ディレクトリ、
# イシュー #480）を基準にする。self-hosted runner がホスト共有
# `CARGO_TARGET_DIR=/cargo-target` を注入する構成では `${WORKSPACE_ROOT}/target`
# 決め打ちのパスにビルド成果物が生成されないため（詳細は
# benches/reports/issue480-target-dir-investigation.md）。
BASELINE_BIN="${BASELINE_BIN:-${BENCH_TARGET_DIR}/release/axum-ref}"
# CORE_BIN の既定値は core-bench example の出力パス（TASK-1.6-3 / #168）。
# `cargo build --release --example core-bench -p fandhe-backend-core` の出力先
# （下の「ビルド」節を参照）。CORE_BIN で任意のパスに上書き可能。
CORE_BIN="${CORE_BIN:-${BENCH_TARGET_DIR}/release/examples/core-bench}"
BASELINE_HOST="${BASELINE_HOST:-127.0.0.1}"
CORE_HOST="${CORE_HOST:-127.0.0.1}"
# baseline/core を同時起動しないが、直前の計測の TIME_WAIT 残留と衝突しないよう
# ポート自体も分ける（README の「同一ホスト計測時のノイズ注意」を踏襲）。
BASELINE_PORT="${BASELINE_PORT:-3101}"
CORE_PORT="${CORE_PORT:-3102}"

# 判定閾値（REQ-1・NFR-1・NFR-2、docs/spec/04-requirements.md）。
RPS_RATIO_MIN="${RPS_RATIO_MIN:-0.90}"
P95_RATIO_MAX="${P95_RATIO_MAX:-1.10}"
P99_RATIO_MAX="${P99_RATIO_MAX:-1.10}"
IDLE_RSS_RATIO_MAX="${IDLE_RSS_RATIO_MAX:-1.10}"
BIN_SIZE_RATIO_MAX="${BIN_SIZE_RATIO_MAX:-1.00}"
STARTUP_DIFF_MAX_MS="${STARTUP_DIFF_MAX_MS:-20}"

# 指定時、判定表を markdown 形式でも追記出力する（benches/reports/*.md 生成用）。
REPORT_MD="${REPORT_MD:-}"

# `P95_BAND=1`（既定 0、opt-in、イシュー #614）: p95 のみ 2 値判定（PASS/FAIL）から
# 3 帯域判定（PASS/INCONCLUSIVE/FAIL、`lib/common.sh` の `p95_band_verdict`）へ
# 切り替える。既定 0 では従来どおり `judge_le` による 2 値判定のまま
# （`bench-accept.sh` の exit 0/1/2 契約を後方互換に維持する。
# `docs/design/bench-p95-criteria.md` 3〜4 節）。RPS・p99・RSS・バイナリサイズ・
# 起動時間の判定は対象外のまま単一しきい値を維持する（同文書 4 節、9 節の実測で
# これらの指標は不安定性が確認されていないため）。
P95_BAND="${P95_BAND:-0}"
if [ "${P95_BAND}" != "0" ] && [ "${P95_BAND}" != "1" ]; then
    echo "エラー: P95_BAND は 0 または 1 である必要があります（現在: ${P95_BAND}）" >&2
    exit 1
fi
# 判定不能帯の相対マージン M（#616 で fail-closed 方針により現状の暫定値のまま
# 維持。新方式・同一コミット系列の実測較正は未収集のため、較正未完了・値その
# ものは変更していない。再較正条件は `benches/reports/issue616-hosted-runner-calibration.md`、
# 設計は `docs/design/bench-p95-criteria.md` 4 節参照）。P95_BAND=0 のときは
# 未使用だが、常に検証だけは行う（下記 validate_numeric）。
P95_MARGIN="${P95_MARGIN:-0.10}"

# `INTERLEAVE=1`（既定 0、opt-in、イシュー #613）: HTTP 系計測（RPS/p95/p99）を
# 「baseline 一括 → core 一括」の 2 ブロックから、baseline/core を `PAIRS`
# 回（`benches/lib/interleave.sh` の既定 8）交互にセッション計測する方式へ
# 切り替える。順序効果・時間帯ドリフトを排除する（背景・実証は
# `benches/reports/issue593-p1-zero-copy-bench.md` 9.7 節、
# `docs/design/bench-hosted-runner.md`）。判定ロジック（axum 比のしきい値・
# 判定表・exit 0/1/2 契約）は無変更 — 各セッションは既存 `RUNS`（最低 3）で
# 内部中央値を出し、セッション間の中央値をエンドポイント値として採用する
# （`endpoints[].{rps,p95,p99}.median` は「PAIRS 個のセッション中央値の中央値」
# へ意味が変わるが、判定コードが読む JSON の形は不変）。RSS・footprint 計測は
# 従来どおり逐次のまま（対象外）。
INTERLEAVE="${INTERLEAVE:-0}"
if [ "${INTERLEAVE}" != "0" ] && [ "${INTERLEAVE}" != "1" ]; then
    echo "エラー: INTERLEAVE は 0 または 1 である必要があります（現在: ${INTERLEAVE}）" >&2
    exit 1
fi

# `SECTION_QUIESCENCE=1`（既定 0、opt-in、イシュー #613）: baseline 区間・core
# 区間それぞれの計測開始前に `benches/lib/exclusive.sh` の静穏確認
# （`wait_for_quiescence`）+ 環境スナップショット（`snapshot_environment`）を
# 実行する。区間ごとの静穏未達は BLOCKED（exit 2）として扱いフェイルクローズし、
# PASS へ丸めない（issue593 レポート 7 節申し送りの「baseline 計測 / core 計測の
# 2 区間分割と区間ごとの静穏再確認」への対応）。区間用しきい値は
# `SECTION_LOAD1_MAX`（未指定時は `LOAD1_MAX` を継承）、待機上限は
# `SECTION_QUIESCE_WAIT_SECS`（既定 300、有界）。
SECTION_QUIESCENCE="${SECTION_QUIESCENCE:-0}"
if [ "${SECTION_QUIESCENCE}" != "0" ] && [ "${SECTION_QUIESCENCE}" != "1" ]; then
    echo "エラー: SECTION_QUIESCENCE は 0 または 1 である必要があります（現在: ${SECTION_QUIESCENCE}）" >&2
    exit 1
fi
SECTION_QUIESCE_WAIT_SECS="${SECTION_QUIESCE_WAIT_SECS:-300}"

if [ "${INTERLEAVE}" = "1" ]; then
    # shellcheck source=lib/interleave.sh
    source "${SCRIPT_DIR}/lib/interleave.sh"
fi
if [ "${SECTION_QUIESCENCE}" = "1" ]; then
    # shellcheck source=lib/exclusive.sh
    source "${SCRIPT_DIR}/lib/exclusive.sh"
    if [ -n "${SECTION_LOAD1_MAX:-}" ]; then
        # 未検証のまま LOAD1_MAX へ代入すると、check_quiescence_once の
        # awk 比較（v <= max）で max が非数値文字列扱いとなり文字列比較に
        # 落ちる（awk の strnum 規則上、数値の v は文字列化されると多くの
        # 場合 "abc" 等の英字始まり文字列より辞書順で小さくなり、実際の
        # loadavg が上限超過でも QUIESCENT と誤判定されうる fail-open。
        # レビュー指摘対応）。common.sh の validate_numeric で事前に有限の
        # 非負数であることを検証し、不正値は exit 1 でフェイルクローズする。
        validate_numeric "${SECTION_LOAD1_MAX}" "SECTION_LOAD1_MAX"
        # shellcheck disable=SC2034 # exclusive.sh（source 先）の
        # check_quiescence_once が参照するグローバル変数（動的 source 先の
        # 参照は shellcheck が追えない）。
        LOAD1_MAX="${SECTION_LOAD1_MAX}"
    fi
    # wait_for_quiescence（exclusive.sh）は QUIESCE_WAIT_SECS を SECONDS との
    # 整数算術（$((...))・-ge）に使うため、小数を許容する validate_numeric では
    # 不十分（例 "1.5" は算術コンテキストで異常終了し、定義済みの BLOCKED
    # （exit 2）経路を通らずに落ちる）。非負整数専用の validate_integer で検証する。
    validate_integer "${SECTION_QUIESCE_WAIT_SECS}" "SECTION_QUIESCE_WAIT_SECS"
    QUIESCE_WAIT_SECS="${SECTION_QUIESCE_WAIT_SECS}"
fi

# 専有ロック取得後にビルドが走ると静穏確認の意味が失われる問題（イシュー #260 Bugbot
# 指摘）への対処。既定 0（毎回ビルドする、従来挙動）。呼び出し元が事前ビルド済みの
# 場合のみ 1 を指定する。
SKIP_BUILD="${SKIP_BUILD:-0}"

# REPORT_MD 指定時、「## 結論」セクションを 1 つ追記する。総合判定が確定できない
# 経路（BLOCKED 等）でも必ず新しい「## 結論」セクションを追記し、レポート末尾の
# 「## 結論」セクションを常に「今回の再計測の結果」で上書きすることで、古い
# PASS/FAIL がそのまま権威として残る事態（stale PASS）を防ぐ（フェイルクローズ、
# イシュー #260 Bugbot 指摘対応。`scripts/accept/lib/plugin-mechanism-conclusion-verdict.awk`
# は「## 結論」見出しごとに区切って判定するため、総合判定行を含まない
# 「## 結論」セクションでも SKIP として正しく扱われ、古いセクションの PASS/FAIL には
# フォールバックしない）。
# 引数: $1 総合判定ラベル（"PASS" / "FAIL" 以外は SKIP 扱いとなる自由記述可、
#         例 "BLOCKED（CORE_BIN 未整備のため判定不能）"）
write_report_conclusion() {
    local label="$1"
    if [ -n "${REPORT_MD}" ]; then
        {
            echo
            echo "## 結論（自動記録: bench-accept.sh 再計測、$(date -u '+%Y-%m-%dT%H:%M:%SZ')）"
            echo
            echo "**総合判定: ${label}**"
        } >>"${REPORT_MD}"
    fi
}

check_dependencies
check_runs_minimum

# env 経由の閾値パラメータは awk の算術式・比較に渡す前に数値形式を検証する
# （コマンドインジェクション・想定外文字列混入の防止、.claude/rules/security.md）。
validate_numeric "${RPS_RATIO_MIN}" "RPS_RATIO_MIN"
validate_numeric "${P95_RATIO_MAX}" "P95_RATIO_MAX"
validate_numeric "${P99_RATIO_MAX}" "P99_RATIO_MAX"
validate_numeric "${IDLE_RSS_RATIO_MAX}" "IDLE_RSS_RATIO_MAX"
validate_numeric "${BIN_SIZE_RATIO_MAX}" "BIN_SIZE_RATIO_MAX"
validate_numeric "${STARTUP_DIFF_MAX_MS}" "STARTUP_DIFF_MAX_MS"
validate_numeric "${P95_MARGIN}" "P95_MARGIN"
# SECTION_QUIESCE_WAIT_SECS は SECTION_QUIESCENCE=1 時の分岐（上記）で
# validate_integer により非負整数として検証済み（wait_for_quiescence の
# 整数算術契約）。ここでの重複検証は行わない。

echo "# bench-accept.sh: TASK-1.6-1 性能受け入れ判定"
echo "実行日時: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
echo "パラメータ: RUNS=${RUNS} DURATION=${DURATION} CONNECTIONS=${CONNECTIONS}"
echo "baseline: ${BASELINE_BIN}（${BASELINE_HOST}:${BASELINE_PORT}）"
echo "core    : ${CORE_BIN}（${CORE_HOST}:${CORE_PORT}）"
echo

# baseline（axum-ref）は TASK-1.2 の成果物として常に存在する前提。存在しない場合は
# ビルド漏れであり、CORE_BIN 欠如（後続分岐）と同じ BLOCKED（終了コード 2）として扱う
# （イシュー #478。旧実装は exit 1 で性能 FAIL と同一コードだったため、
# bench-schedule.yml の起票分岐が環境問題を「性能退行 FAIL」と誤起票していた）。
if [ "${SKIP_BUILD}" = "1" ]; then
    echo "== ビルド: SKIP_BUILD=1 のためスキップ（呼び出し元が事前ビルド済みの前提） =="
else
    echo "== ビルド =="
    cargo build --release --manifest-path "${WORKSPACE_ROOT}/Cargo.toml"
    # `cargo build --release`（workspace ビルド）は example をビルド対象に含めないため、
    # core-bench（TASK-1.6-3 / #168）は個別に明示ビルドする。
    cargo build --release --example core-bench -p fandhe-backend-core --manifest-path "${WORKSPACE_ROOT}/Cargo.toml"
fi
echo

if [ ! -x "${BASELINE_BIN}" ]; then
    echo "## 判定結果: BLOCKED"
    echo
    echo "baseline バイナリ（BASELINE_BIN=${BASELINE_BIN}）が見つかりません。"
    echo "'cargo build --release' が成功しているか確認するか、BASELINE_BIN で既存バイナリのパスを指定して再実行してください。"
    echo "実効 target dir（BENCH_TARGET_DIR=${BENCH_TARGET_DIR}）が cargo の実際のビルド出力先と"
    echo "一致しているか確認してください（CARGO_TARGET_DIR env・.cargo/config.toml の"
    echo "build.target-dir が原因で食い違うことがあります。イシュー #480、"
    echo "benches/reports/issue480-target-dir-investigation.md 参照）"
    if [ -n "${REPORT_MD}" ]; then
        {
            echo
            echo "## 判定結果: BLOCKED"
            echo
            echo "baseline バイナリ（\`BASELINE_BIN=${BASELINE_BIN}\`）が見つからないため、"
            echo "axum-ref との比較判定を実施できませんでした。"
            echo "'cargo build --release' の成功を確認してから再実行してください。"
        } >>"${REPORT_MD}"
    fi
    # 「## 結論」セクションを必ず追記して古い PASS/FAIL を上書きする（stale PASS 防止、
    # イシュー #260 Bugbot 指摘対応）。
    write_report_conclusion "BLOCKED（baseline バイナリ未整備のため判定不能。既存の古い判定は無効）"
    # CORE_BIN 欠如（後続分岐）と同じ BLOCKED 専用終了コード。exit 1（性能 FAIL）と
    # 区別することで bench-schedule.yml の起票分岐・nfr6_run_with_fail_retry の
    # 非再試行契約（exit 1 のみ再試行）に正しく乗せる（イシュー #478）。
    exit 2
fi

# コア側計測用バイナリが未整備の場合（TASK-1.4-2 #70・TASK-1.5 #14 未マージ）は、
# 判定ロジックを実行せずに明示的なブロック終了とする（安全側に倒す）。
# CORE_BIN を明示的に指定した axum-ref 同士のセルフ比較（ハーネス正しさの検証）は
# この分岐を通らず、そのまま計測フローに進む。
if [ ! -x "${CORE_BIN}" ]; then
    echo "## 判定結果: BLOCKED"
    echo
    echo "コア側計測用バイナリ（CORE_BIN=${CORE_BIN}）が見つかりません。"
    echo "'cargo build --release --example core-bench -p fandhe-backend-core' が"
    echo "成功しているか確認するか、CORE_BIN で既存バイナリのパスを指定して再実行してください。"
    echo "実効 target dir（BENCH_TARGET_DIR=${BENCH_TARGET_DIR}）が cargo の実際のビルド出力先と"
    echo "一致しているか確認してください（CARGO_TARGET_DIR env・.cargo/config.toml の"
    echo "build.target-dir が原因で食い違うことがあります。イシュー #480 参照）"
    if [ -n "${REPORT_MD}" ]; then
        {
            echo
            echo "## 判定結果: BLOCKED"
            echo
            echo "コア側計測用バイナリ（\`CORE_BIN=${CORE_BIN}\`）が見つからないため、"
            echo "axum-ref との比較判定を実施できませんでした。"
            echo "'cargo build --release --example core-bench -p fandhe-backend-core' の"
            echo "成功を確認してから再実行してください。"
        } >>"${REPORT_MD}"
    fi
    # 「## 結論」セクションを必ず追記して古い PASS/FAIL を上書きする（stale PASS 防止、
    # イシュー #260 Bugbot 指摘対応）。
    write_report_conclusion "BLOCKED（CORE_BIN 未整備のため判定不能。既存の古い判定は無効）"
    exit 2
fi

# --- 計測用一時ファイル（機械可読 JSON の受け渡し。CWE-377/59 対策で mktemp を使う） ---
BASELINE_HTTP_JSON="$(mktemp)"
BASELINE_RSS_JSON="$(mktemp)"
BASELINE_FOOT_JSON="$(mktemp)"
CORE_HTTP_JSON="$(mktemp)"
CORE_RSS_JSON="$(mktemp)"
CORE_FOOT_JSON="$(mktemp)"
cleanup_tmp() {
    rm -f "${BASELINE_HTTP_JSON}" "${BASELINE_RSS_JSON}" "${BASELINE_FOOT_JSON}" \
        "${CORE_HTTP_JSON}" "${CORE_RSS_JSON}" "${CORE_FOOT_JSON}"
}
trap cleanup_tmp EXIT

# baseline・対象を同時起動せず逐次計測する（ポートは分離済みだが、同時実行すると
# CPU 競合でノイズが乗るため意図的に直列化する）。
# common.sh を本スクリプトが source した時点で TARGET_URL が既定値（TARGET_HOST:TARGET_PORT
# 由来）で export 済みのため、サブスクリプト側の `TARGET_URL="${TARGET_URL:-...}"` が
# 親の値をそのまま継承してしまい HOST/PORT 上書きと食い違う（common.sh の整合性検証で
# エラー終了する）。サブプロセスには BASELINE/CORE それぞれの TARGET_URL を明示的に
# 渡し、親の既定値を隠す。
BASELINE_URL="http://${BASELINE_HOST}:${BASELINE_PORT}"
CORE_URL="http://${CORE_HOST}:${CORE_PORT}"

# 区間静穏確認（SECTION_QUIESCENCE=1 のみ）。未達は BLOCKED（exit 2）で
# フェイルクローズし、PASS へ丸めない（issue593 レポート 7 節申し送り）。
run_section_quiescence_gate() {
    local section_label="$1"
    echo "== 区間静穏確認（${section_label}） =="
    if ! wait_for_quiescence; then
        echo "## 判定結果: BLOCKED" >&2
        echo "${section_label}区間の静穏確認が ${QUIESCE_WAIT_SECS}s 待っても得られませんでした。" >&2
        if [ -n "${REPORT_MD}" ]; then
            {
                echo
                echo "## 判定結果: BLOCKED"
                echo
                echo "${section_label}区間の静穏確認が \`QUIESCE_WAIT_SECS=${QUIESCE_WAIT_SECS}\`s 待っても得られず、計測を実施できませんでした。"
            } >>"${REPORT_MD}"
        fi
        write_report_conclusion "BLOCKED（${section_label}区間の静穏確認未達のため判定不能。既存の古い判定は無効）"
        exit 2
    fi
    # 他の呼び出し元（bench-accept-exclusive.sh・nfr6_run_with_fail_retry 等）と
    # 同様に stderr へ出す（human 可読な stdout は判定表・結論に専念させる。
    # RESULT_JSON/REPORT_MD を機械可読出力として消費する既存契約と整合させる）。
    snapshot_environment "${section_label}" >&2
}

if [ "${INTERLEAVE}" = "1" ]; then
    echo "== HTTP 計測（INTERLEAVE=1、PAIRS=${PAIRS} 交互セッション） =="
    INTERLEAVE_DIR="$(mktemp -d)"
    trap 'rm -rf "${INTERLEAVE_DIR}"; cleanup_tmp' EXIT
    # `SECTION_QUIESCENCE=1` 併用時は baseline/core 各区間開始前に 1 回だけでは
    # PAIRS 回（既定 8）にわたるドリフト・汚染を検出できないため、各ペア開始
    # 直前に静穏ゲートを実行するフックを `interleave_run_pairs` へ渡す
    # （`run_section_quiescence_gate` 自体は未達時に `exit 2` で終了する既存の
    # fail-closed 契約のまま。A/B 各セッションではなくペア単位にする理由
    # （自己負荷の loadavg 残差誤検知の回避）は `benches/lib/interleave.sh` の
    # `interleave_run_pairs` doc comment 参照。イシュー #613 P1 レビュー指摘対応）。
    interleave_quiesce_hook=""
    if [ "${SECTION_QUIESCENCE}" = "1" ]; then
        interleave_quiesce_hook="run_section_quiescence_gate"
    fi
    # `interleave_run_pairs`（`bench-http.sh` 委譲）のセッション実行失敗
    # （ポート衝突・サーバ起動失敗・依存ツール欠如等）は決定論的な環境エラーで
    # あり、性能退行 FAIL（exit 1）とは区別して BLOCKED（exit 2）として扱う。
    # `set -e` 下で素通しすると `bench-http.sh` の exit 1 がそのまま本スクリプトの
    # exit 1（性能 FAIL）として誤分類される（`bench-pair.sh` が同種の問題に採った
    # 対処と同一パターン、イシュー #613 P1 レビュー指摘対応）。
    if ! interleave_run_pairs "${BASELINE_BIN}" "${BASELINE_PORT}" "${CORE_BIN}" "${CORE_PORT}" "${INTERLEAVE_DIR}" "${interleave_quiesce_hook}"; then
        echo "## 判定結果: BLOCKED" >&2
        echo "交互ペア測定のセッション実行に失敗しました（ポート衝突・サーバ起動失敗等の決定論的失敗として BLOCKED 扱い。exit 1 の性能 FAIL とは区別する）" >&2
        if [ -n "${REPORT_MD}" ]; then
            {
                echo
                echo "## 判定結果: BLOCKED"
                echo
                echo "交互ペア測定のセッション実行に失敗しました（ポート衝突・サーバ起動失敗等の決定論的失敗）。"
            } >>"${REPORT_MD}"
        fi
        write_report_conclusion "BLOCKED（交互ペア測定のセッション実行失敗のため判定不能。既存の古い判定は無効）"
        exit 2
    fi
    # `interleave_run_pairs` が成功終了コードを返していても、`bench-http.sh` が
    # RESULT_JSON を書き出す前に予期せず終了する等で JSON が欠落する可能性は
    # 理論上残る（`bench-pair.sh` の同種チェックと同一パターン）。欠落したまま
    # 後続の `jq -s` へ渡すと `set -e` 下で jq の異常終了コードがそのまま本
    # スクリプトの終了コードになり、BLOCKED（exit 2）ではなく不定のコードで
    # 落ちて判定不能の理由が伝わらない。ここで明示的に検査し BLOCKED として
    # フェイルクローズする。
    if [ ! -f "${INTERLEAVE_DIR}/a-1.json" ] || [ ! -f "${INTERLEAVE_DIR}/b-1.json" ]; then
        echo "## 判定結果: BLOCKED" >&2
        echo "交互ペア測定の結果 JSON（${INTERLEAVE_DIR}/a-1.json または b-1.json）が見つかりません" >&2
        if [ -n "${REPORT_MD}" ]; then
            {
                echo
                echo "## 判定結果: BLOCKED"
                echo
                echo "交互ペア測定の結果 JSON が見つかりませんでした。"
            } >>"${REPORT_MD}"
        fi
        write_report_conclusion "BLOCKED（交互ペア測定の結果 JSON 欠落のため判定不能。既存の古い判定は無効）"
        exit 2
    fi
    # PAIRS 個のセッション JSON（各 `bench-http.sh` の既存スキーマそのまま）を
    # エンドポイントごとに集約する。各セッションの `.median` を「1 サンプル」と
    # みなし、セッション間の中央値を最終値として採用する（判定コード
    # （後続の calc_ratio 等）が読む JSON の形・意味（endpoints[].*.median）は
    # 不変のまま、baseline 一括/core 一括の代わりに交互セッションの結果を渡す）。
    merge_interleaved_sessions() {
        local side="$1" out_json="$2"
        local files=()
        local i
        for ((i = 1; i <= PAIRS; i++)); do
            files+=("${INTERLEAVE_DIR}/${side}-${i}.json")
        done
        # `median_of`: `benches/lib/common.sh` の `median()`（奇数個=中央値、
        # 偶数個=中央 2 値の平均）と同一の算出規則を jq 側で再実装する
        # （bash 版と算出方式を揃え、判定結果が経路依存でぶれないようにする）。
        # NOTE: `map` をネストすると内側の `map` は「現在の `.`」（外側 map の
        # 各要素）を入力に取るため、最上位でスラープした全セッション文書の配列を
        # `$docs` として変数に退避しておかないと内側の `map(.endpoints[...])` が
        # 誤った入力（ラベルの entries 要素）を map してしまう（実装時に実データで
        # 検出・修正）。
        jq -s --argjson runs "${RUNS}" --arg duration "${DURATION}" --argjson connections "${CONNECTIONS}" '
            def median_of: sort as $s | ($s | length) as $n
                | if $n == 0 then null
                  elif ($n % 2) == 1 then $s[($n - 1) / 2 | floor]
                  else ($s[$n / 2 - 1] + $s[$n / 2]) / 2
                  end;
            . as $docs
            | ($docs[0].endpoints | map(.label)) as $labels
            | {
                runs: $runs, duration: $duration, connections: $connections,
                endpoints: ($labels | to_entries | map(
                    . as $e | {
                        label: $e.value,
                        rps: {raw: ($docs | map(.endpoints[$e.key].rps.median)), median: ($docs | map(.endpoints[$e.key].rps.median) | median_of)},
                        p50: {raw: ($docs | map(.endpoints[$e.key].p50.median)), median: ($docs | map(.endpoints[$e.key].p50.median) | median_of)},
                        p95: {raw: ($docs | map(.endpoints[$e.key].p95.median)), median: ($docs | map(.endpoints[$e.key].p95.median) | median_of)},
                        p99: {raw: ($docs | map(.endpoints[$e.key].p99.median)), median: ($docs | map(.endpoints[$e.key].p99.median) | median_of)}
                    }
                ))
              }
        ' "${files[@]}" >"${out_json}"
    }
    merge_interleaved_sessions "a" "${BASELINE_HTTP_JSON}"
    merge_interleaved_sessions "b" "${CORE_HTTP_JSON}"
else
    if [ "${SECTION_QUIESCENCE}" = "1" ]; then
        run_section_quiescence_gate "baseline"
    fi
    echo "== baseline（axum-ref）計測 =="
    TARGET_BIN="${BASELINE_BIN}" TARGET_HOST="${BASELINE_HOST}" TARGET_PORT="${BASELINE_PORT}" TARGET_URL="${BASELINE_URL}" \
        RUNS="${RUNS}" DURATION="${DURATION}" CONNECTIONS="${CONNECTIONS}" \
        RESULT_JSON="${BASELINE_HTTP_JSON}" "${SCRIPT_DIR}/bench-http.sh"
fi
TARGET_BIN="${BASELINE_BIN}" TARGET_HOST="${BASELINE_HOST}" TARGET_PORT="${BASELINE_PORT}" TARGET_URL="${BASELINE_URL}" \
    RUNS="${RUNS}" DURATION="${DURATION}" CONNECTIONS="${CONNECTIONS}" \
    RESULT_JSON="${BASELINE_RSS_JSON}" "${SCRIPT_DIR}/bench-rss.sh"
TARGET_BIN="${BASELINE_BIN}" TARGET_HOST="${BASELINE_HOST}" TARGET_PORT="${BASELINE_PORT}" TARGET_URL="${BASELINE_URL}" \
    RUNS="${RUNS}" \
    RESULT_JSON="${BASELINE_FOOT_JSON}" "${SCRIPT_DIR}/bench-footprint.sh"

echo
if [ "${INTERLEAVE}" != "1" ]; then
    if [ "${SECTION_QUIESCENCE}" = "1" ]; then
        run_section_quiescence_gate "core"
    fi
    echo "== core 計測 =="
    TARGET_BIN="${CORE_BIN}" TARGET_HOST="${CORE_HOST}" TARGET_PORT="${CORE_PORT}" TARGET_URL="${CORE_URL}" \
        RUNS="${RUNS}" DURATION="${DURATION}" CONNECTIONS="${CONNECTIONS}" \
        RESULT_JSON="${CORE_HTTP_JSON}" "${SCRIPT_DIR}/bench-http.sh"
fi
TARGET_BIN="${CORE_BIN}" TARGET_HOST="${CORE_HOST}" TARGET_PORT="${CORE_PORT}" TARGET_URL="${CORE_URL}" \
    RUNS="${RUNS}" DURATION="${DURATION}" CONNECTIONS="${CONNECTIONS}" \
    RESULT_JSON="${CORE_RSS_JSON}" "${SCRIPT_DIR}/bench-rss.sh"
TARGET_BIN="${CORE_BIN}" TARGET_HOST="${CORE_HOST}" TARGET_PORT="${CORE_PORT}" TARGET_URL="${CORE_URL}" \
    RUNS="${RUNS}" \
    RESULT_JSON="${CORE_FOOT_JSON}" "${SCRIPT_DIR}/bench-footprint.sh"
echo

# --- 比率・絶対差の算出（LC_NUMERIC=C 固定。カンマ小数点ロケール対策、lib/common.sh と同根） ---
calc_ratio() {
    local numerator="$1" denominator="$2"
    LC_NUMERIC=C awk -v n="${numerator}" -v d="${denominator}" \
        'BEGIN { if (d == 0) { print "nan" } else { printf "%.4f", n / d } }'
}
calc_abs_diff() {
    local a="$1" b="$2"
    LC_NUMERIC=C awk -v a="${a}" -v b="${b}" 'BEGIN { d = a - b; if (d < 0) { d = -d }; printf "%.4f", d }'
}
# 比較 1: value >= threshold なら真（exit 0）。
# calc_ratio が分母 0 で返す "nan" は awk 実装依存の数値変換に頼らず、
# ここで明示的に FAIL（非 0 exit）として弾く（judge_le も同様）。
judge_ge() {
    [ "$1" = "nan" ] && return 1
    LC_NUMERIC=C awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 >= b + 0) }'
}
# 比較 2: value <= threshold なら真（exit 0）
judge_le() {
    [ "$1" = "nan" ] && return 1
    LC_NUMERIC=C awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 <= b + 0) }'
}

# 総合判定は `OVERALL_FAIL`/`OVERALL_INCONCLUSIVE` の 2 フラグで決める
# （優先順位 FAIL > INCONCLUSIVE > PASS、下記「総合判定」節）。P95_BAND=1 導入
# （イシュー #614）以前は PASS/FAIL の 2 値しか存在しなかったため
# `OVERALL_INCONCLUSIVE` は常に 0 のままで、既存の exit 0/1 契約が完全に維持
# される（P95_BAND=0 が既定であるため後方互換）。
OVERALL_FAIL=0
OVERALL_INCONCLUSIVE=0
declare -a JUDGE_ROWS=()

# 判定 1 行を記録する。$1=指標名 $2=baseline値 $3=core値 $4=比較値(比率or絶対差)
# $5=基準 $6=PASS/FAIL/INCONCLUSIVE
record_row() {
    local metric="$1" baseline_val="$2" core_val="$3" compare_val="$4" criteria="$5" verdict="$6"
    case "${verdict}" in
        PASS) ;;
        FAIL) OVERALL_FAIL=1 ;;
        INCONCLUSIVE)
            # P95_BAND=1 の p95 行専用（他指標は PASS/FAIL の 2 値のみを渡す契約）。
            OVERALL_INCONCLUSIVE=1
            ;;
        *)
            echo "エラー: 想定外の判定値 '${verdict}'（指標: ${metric}）" >&2
            exit 1
            ;;
    esac
    JUDGE_ROWS+=("${metric}|${baseline_val}|${core_val}|${compare_val}|${criteria}|${verdict}")
}

# --- RPS・p95・p99（4 エンドポイントすべてで基準を満たすこと） ---
endpoint_count="$(jq '.endpoints | length' "${BASELINE_HTTP_JSON}")"
for ((i = 0; i < endpoint_count; i++)); do
    label="$(jq -r ".endpoints[${i}].label" "${BASELINE_HTTP_JSON}")"
    baseline_rps="$(jq -r ".endpoints[${i}].rps.median" "${BASELINE_HTTP_JSON}")"
    core_rps="$(jq -r ".endpoints[${i}].rps.median" "${CORE_HTTP_JSON}")"
    baseline_p95="$(jq -r ".endpoints[${i}].p95.median" "${BASELINE_HTTP_JSON}")"
    core_p95="$(jq -r ".endpoints[${i}].p95.median" "${CORE_HTTP_JSON}")"
    baseline_p99="$(jq -r ".endpoints[${i}].p99.median" "${BASELINE_HTTP_JSON}")"
    core_p99="$(jq -r ".endpoints[${i}].p99.median" "${CORE_HTTP_JSON}")"

    rps_ratio="$(calc_ratio "${core_rps}" "${baseline_rps}")"
    p95_ratio="$(calc_ratio "${core_p95}" "${baseline_p95}")"
    p99_ratio="$(calc_ratio "${core_p99}" "${baseline_p99}")"

    rps_verdict="FAIL"
    judge_ge "${rps_ratio}" "${RPS_RATIO_MIN}" && rps_verdict="PASS"
    # p95 のみ P95_BAND=1 で 3 帯域判定（PASS/INCONCLUSIVE/FAIL）へ切り替える
    # （イシュー #614）。既定 0 は従来どおり judge_le の 2 値判定のまま。
    p95_criteria="<= ${P95_RATIO_MAX}"
    if [ "${P95_BAND}" = "1" ]; then
        p95_verdict="$(p95_band_verdict "${p95_ratio}" "${P95_RATIO_MAX}" "${P95_MARGIN}")"
        p95_criteria="<= ${P95_RATIO_MAX}（帯域 M=${P95_MARGIN}、判定不能上限 <= $(LC_NUMERIC=C awk -v l="${P95_RATIO_MAX}" -v m="${P95_MARGIN}" 'BEGIN { printf "%.4f", (l + 0) * (1 + (m + 0)) }')）"
    else
        p95_verdict="FAIL"
        judge_le "${p95_ratio}" "${P95_RATIO_MAX}" && p95_verdict="PASS"
    fi
    p99_verdict="FAIL"
    judge_le "${p99_ratio}" "${P99_RATIO_MAX}" && p99_verdict="PASS"

    record_row "RPS ${label}" "${baseline_rps}" "${core_rps}" "${rps_ratio}" ">= ${RPS_RATIO_MIN}" "${rps_verdict}"
    record_row "p95 ${label}" "${baseline_p95}" "${core_p95}" "${p95_ratio}" "${p95_criteria}" "${p95_verdict}"
    record_row "p99 ${label}" "${baseline_p99}" "${core_p99}" "${p99_ratio}" "<= ${P99_RATIO_MAX}" "${p99_verdict}"
done

# --- アイドル RSS（bench-footprint.sh の中央値を採用） ---
baseline_idle_rss="$(jq -r '.idle_rss_kb.median' "${BASELINE_FOOT_JSON}")"
core_idle_rss="$(jq -r '.idle_rss_kb.median' "${CORE_FOOT_JSON}")"
idle_rss_ratio="$(calc_ratio "${core_idle_rss}" "${baseline_idle_rss}")"
idle_rss_verdict="FAIL"
judge_le "${idle_rss_ratio}" "${IDLE_RSS_RATIO_MAX}" && idle_rss_verdict="PASS"
record_row "アイドル RSS" "${baseline_idle_rss}KB" "${core_idle_rss}KB" "${idle_rss_ratio}" "<= ${IDLE_RSS_RATIO_MAX}" "${idle_rss_verdict}"

# --- リリースバイナリサイズ ---
baseline_bin_size="$(jq -r '.binary_size_bytes' "${BASELINE_FOOT_JSON}")"
core_bin_size="$(jq -r '.binary_size_bytes' "${CORE_FOOT_JSON}")"
bin_size_ratio="$(calc_ratio "${core_bin_size}" "${baseline_bin_size}")"
bin_size_verdict="FAIL"
judge_le "${bin_size_ratio}" "${BIN_SIZE_RATIO_MAX}" && bin_size_verdict="PASS"
record_row "バイナリサイズ" "${baseline_bin_size}B" "${core_bin_size}B" "${bin_size_ratio}" "<= ${BIN_SIZE_RATIO_MAX}" "${bin_size_verdict}"

# --- 起動時間（axum との絶対差、NFR-1） ---
baseline_startup="$(jq -r '.startup_ms.median' "${BASELINE_FOOT_JSON}")"
core_startup="$(jq -r '.startup_ms.median' "${CORE_FOOT_JSON}")"
startup_diff="$(calc_abs_diff "${core_startup}" "${baseline_startup}")"
startup_verdict="FAIL"
judge_le "${startup_diff}" "${STARTUP_DIFF_MAX_MS}" && startup_verdict="PASS"
record_row "起動時間(ms・絶対差)" "${baseline_startup}" "${core_startup}" "${startup_diff}" "<= ${STARTUP_DIFF_MAX_MS}" "${startup_verdict}"

# --- 参考値: 負荷時 RSS（README・計画に明記の通り、判定には使わない） ---
baseline_load_rss="$(jq -r '.load_rss_kb_median' "${BASELINE_RSS_JSON}")"
core_load_rss="$(jq -r '.load_rss_kb_median' "${CORE_RSS_JSON}")"

echo "## 判定表"
echo
printf '%-24s | %-16s | %-16s | %-10s | %-10s | %s\n' "指標" "baseline(axum)" "core" "比率/差" "基準" "判定"
printf '%s\n' "----------------------------------------------------------------------------------------------"
for row in "${JUDGE_ROWS[@]}"; do
    IFS='|' read -r metric baseline_val core_val compare_val criteria verdict <<<"${row}"
    printf '%-24s | %-16s | %-16s | %-10s | %-10s | %s\n' "${metric}" "${baseline_val}" "${core_val}" "${compare_val}" "${criteria}" "${verdict}"
done
echo
echo "参考値（判定には使わない）: 負荷時 RSS baseline=${baseline_load_rss}KB core=${core_load_rss}KB"
echo

# CPU_PROBE=1（直接指定、または INTERLEAVE 経路の各セッション経由で継承）のときのみ、
# 収集済みの外部占有率分布を判定表の後に追記する（判定そのものには使わない、
# 退行帰属・診断用の参考情報）。`CPU_PROBE` env は本スクリプトが明示的に
# エクスポートしなくても、呼び出し元の環境から子プロセス（bench-http.sh・
# interleave.sh 経由の各セッション）へ自然に継承される。
CPU_PROBE_SUMMARY=""
if [ "${CPU_PROBE:-0}" = "1" ]; then
    if [ "${INTERLEAVE}" = "1" ]; then
        cpu_probe_files=("${INTERLEAVE_DIR}"/*.json)
    else
        cpu_probe_files=("${BASELINE_HTTP_JSON}" "${CORE_HTTP_JSON}")
    fi
    CPU_PROBE_SUMMARY="$(jq -s '
        [.[].endpoints[]?.cpu_probe? | select(. != null)] as $probes
        | ($probes | map(.ext_cpu_pct[]) | map(select(. != null))) as $shares
        | ($probes | map(.contaminated[]) | add // 0) as $contaminated_total
        | ($probes | map(.remeasure_count[]) | add // 0) as $remeasure_total
        | if ($shares | length) == 0 then
            "CPU_PROBE: 収集済みデータなし"
          else
            "CPU_PROBE 外部占有率分布: 最小=" + (($shares | min) | tostring)
            + "% 中央値=" + (($shares | sort | .[length/2|floor]) | tostring)
            + "% 最大=" + (($shares | max) | tostring)
            + "%（汚染窓 " + ($contaminated_total | tostring) + " 件、窓単位再計測発動 "
            + ($remeasure_total | tostring) + " 回、集計対象窓数 " + ($shares | length | tostring) + "）"
          end
    ' "${cpu_probe_files[@]}" 2>/dev/null | tr -d '"')"
    if [ -n "${CPU_PROBE_SUMMARY}" ]; then
        echo "${CPU_PROBE_SUMMARY}"
        echo
    fi
fi

if [ -n "${REPORT_MD}" ]; then
    {
        echo
        echo "## 判定表"
        echo
        echo "| 指標 | baseline(axum) | core | 比率/差 | 基準 | 判定 |"
        echo "|------|-----------------|------|---------|------|------|"
        for row in "${JUDGE_ROWS[@]}"; do
            IFS='|' read -r metric baseline_val core_val compare_val criteria verdict <<<"${row}"
            echo "| ${metric} | ${baseline_val} | ${core_val} | ${compare_val} | ${criteria} | ${verdict} |"
        done
        echo
        echo "参考値（判定には使わない）: 負荷時 RSS baseline=${baseline_load_rss}KB core=${core_load_rss}KB"
        if [ -n "${CPU_PROBE_SUMMARY}" ]; then
            echo
            echo "${CPU_PROBE_SUMMARY}"
        fi
    } >>"${REPORT_MD}"
fi

# `scripts/accept/plugin-mechanism-accept.sh` 基準 5 は REPORT_MD の「## 結論」
# セクション内の `**総合判定: PASS/FAIL**` 行のみを機械判定に使う（他セクションに
# 引用として埋め込まれた同一文言の誤検知を避けるため、セクション限定・複数存在時は
# 末尾のセクションを採用する設計、イシュー #260 Bugbot 指摘対応）。再計測のたびに
# 新しい「## 結論」セクションを追記することで、レポートを手編集しなくても
# 受け入れゲートへ再計測結果を機械的に反映できるようにする。
# `plugin-mechanism-conclusion-verdict.awk` は "PASS"/"FAIL" の完全一致行のみを
# 判定材料にするため、「INCONCLUSIVE」は自動的にどちらにもマッチしない
# （fail-closed。判定不能を PASS へ丸めない、イシュー #614）。
#
# 総合判定の優先順位: FAIL > INCONCLUSIVE > PASS
# （`OVERALL_FAIL`/`OVERALL_INCONCLUSIVE` は P95_BAND=1 の p95 帯域判定でのみ
# 非 0 になりうる。P95_BAND=0（既定）では両方常に 0 のままで、以下は
# `OVERALL_PASS` のみに依存する従来の 2 分岐と完全に等価になる）。
if [ "${OVERALL_FAIL}" -eq 1 ]; then
    write_report_conclusion "FAIL"
    echo "## 判定結果: FAIL（1 件以上の基準未達）"
    exit 1
elif [ "${OVERALL_INCONCLUSIVE}" -eq 1 ]; then
    write_report_conclusion "INCONCLUSIVE"
    echo "## 判定結果: INCONCLUSIVE（p95 が判定不能帯に収まり退行の有無を確定できない。二次判定（bench-pair.sh）で確定させること）"
    exit 3
else
    write_report_conclusion "PASS"
    echo "## 判定結果: PASS"
    exit 0
fi
