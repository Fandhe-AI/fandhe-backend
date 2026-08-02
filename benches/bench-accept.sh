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

echo "# bench-accept.sh: TASK-1.6-1 性能受け入れ判定"
echo "実行日時: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
echo "パラメータ: RUNS=${RUNS} DURATION=${DURATION} CONNECTIONS=${CONNECTIONS}"
echo "baseline: ${BASELINE_BIN}（${BASELINE_HOST}:${BASELINE_PORT}）"
echo "core    : ${CORE_BIN}（${CORE_HOST}:${CORE_PORT}）"
echo

# baseline（axum-ref）は TASK-1.2 の成果物として常に存在する前提。存在しない場合は
# ビルド漏れであり、判定不能として明確にエラー終了する（ブロック扱いとは区別する）。
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
    echo "エラー: baseline バイナリ ${BASELINE_BIN} が見つかりません。'cargo build --release' を確認してください" >&2
    echo "        実効 target dir（BENCH_TARGET_DIR=${BENCH_TARGET_DIR}）が cargo の実際の" >&2
    echo "        ビルド出力先と一致しているか確認してください（CARGO_TARGET_DIR env・" >&2
    echo "        .cargo/config.toml の build.target-dir が原因で食い違うことがあります。" >&2
    echo "        イシュー #480、benches/reports/issue480-target-dir-investigation.md 参照）" >&2
    write_report_conclusion "BLOCKED（baseline バイナリ未整備のため判定不能。既存の古い判定は無効）"
    exit 1
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

echo "== baseline（axum-ref）計測 =="
TARGET_BIN="${BASELINE_BIN}" TARGET_HOST="${BASELINE_HOST}" TARGET_PORT="${BASELINE_PORT}" TARGET_URL="${BASELINE_URL}" \
    RUNS="${RUNS}" DURATION="${DURATION}" CONNECTIONS="${CONNECTIONS}" \
    RESULT_JSON="${BASELINE_HTTP_JSON}" "${SCRIPT_DIR}/bench-http.sh"
TARGET_BIN="${BASELINE_BIN}" TARGET_HOST="${BASELINE_HOST}" TARGET_PORT="${BASELINE_PORT}" TARGET_URL="${BASELINE_URL}" \
    RUNS="${RUNS}" DURATION="${DURATION}" CONNECTIONS="${CONNECTIONS}" \
    RESULT_JSON="${BASELINE_RSS_JSON}" "${SCRIPT_DIR}/bench-rss.sh"
TARGET_BIN="${BASELINE_BIN}" TARGET_HOST="${BASELINE_HOST}" TARGET_PORT="${BASELINE_PORT}" TARGET_URL="${BASELINE_URL}" \
    RUNS="${RUNS}" \
    RESULT_JSON="${BASELINE_FOOT_JSON}" "${SCRIPT_DIR}/bench-footprint.sh"

echo
echo "== core 計測 =="
TARGET_BIN="${CORE_BIN}" TARGET_HOST="${CORE_HOST}" TARGET_PORT="${CORE_PORT}" TARGET_URL="${CORE_URL}" \
    RUNS="${RUNS}" DURATION="${DURATION}" CONNECTIONS="${CONNECTIONS}" \
    RESULT_JSON="${CORE_HTTP_JSON}" "${SCRIPT_DIR}/bench-http.sh"
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

OVERALL_PASS=1
declare -a JUDGE_ROWS=()

# 判定 1 行を記録する。$1=指標名 $2=baseline値 $3=core値 $4=比較値(比率or絶対差)
# $5=基準 $6=PASS/FAIL
record_row() {
    local metric="$1" baseline_val="$2" core_val="$3" compare_val="$4" criteria="$5" verdict="$6"
    if [ "${verdict}" != "PASS" ]; then
        OVERALL_PASS=0
    fi
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
    p95_verdict="FAIL"
    judge_le "${p95_ratio}" "${P95_RATIO_MAX}" && p95_verdict="PASS"
    p99_verdict="FAIL"
    judge_le "${p99_ratio}" "${P99_RATIO_MAX}" && p99_verdict="PASS"

    record_row "RPS ${label}" "${baseline_rps}" "${core_rps}" "${rps_ratio}" ">= ${RPS_RATIO_MIN}" "${rps_verdict}"
    record_row "p95 ${label}" "${baseline_p95}" "${core_p95}" "${p95_ratio}" "<= ${P95_RATIO_MAX}" "${p95_verdict}"
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
    } >>"${REPORT_MD}"
fi

# `scripts/accept/plugin-mechanism-accept.sh` 基準 5 は REPORT_MD の「## 結論」
# セクション内の `**総合判定: PASS/FAIL**` 行のみを機械判定に使う（他セクションに
# 引用として埋め込まれた同一文言の誤検知を避けるため、セクション限定・複数存在時は
# 末尾のセクションを採用する設計、イシュー #260 Bugbot 指摘対応）。再計測のたびに
# 新しい「## 結論」セクションを追記することで、レポートを手編集しなくても
# 受け入れゲートへ再計測結果を機械的に反映できるようにする。
if [ "${OVERALL_PASS}" -eq 1 ]; then
    write_report_conclusion "PASS"
else
    write_report_conclusion "FAIL"
fi

if [ "${OVERALL_PASS}" -eq 1 ]; then
    echo "## 判定結果: PASS"
    exit 0
else
    echo "## 判定結果: FAIL（1 件以上の基準未達）"
    exit 1
fi
