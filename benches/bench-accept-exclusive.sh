#!/usr/bin/env bash
# REQ-2（docs/spec/04-requirements.md）基準 5「両 feature（webrtc-proxy・graphql）
# 無効時のコア性能が REQ-1 の性能基準を維持する」の専有計測 wrapper（TASK-260 相当 / #260）。
#
# このスクリプトの役割:
#   `benches/lib/exclusive.sh` の専有実行枠（flock によるホストグローバル相互排他・
#   静穏（quiescence）確認・環境スナップショット）を取得したうえで
#   `benches/bench-accept.sh`（axum-ref 比 REQ-1・NFR-1・NFR-2 判定オーケストレータ）を
#   1 回実行する。並列 issue 実装ワークフロー下では host contention により RPS 比が
#   大きく振れうるため（`benches/nfr6-exclusive.sh` と同根の問題、
#   `benches/reports/task-9.5-hub-wiring-performance.md` 診断）、`bench-accept.sh` 単体
#   実行では判定が確定しないおそれがある。本 wrapper は `nfr6-exclusive.sh` と同じ
#   専有実行枠の構造を踏襲しつつ、判定ロジックは `bench-accept.sh`（NFR-6 比率帯判定とは
#   異なる REQ-1 受け入れ判定）にそのまま委譲する。
#
# `bench-accept.sh` の `CORE_BIN`（既定 `target/release/examples/core-bench`）は
# `fandhe-backend-core` の `default = []` 構成でビルドされるため、webrtc-proxy・graphql
# 両 feature が無効な状態そのものが計測対象になる（`crates/**` の追加変更は不要）。
#
# ビルド（axum-ref・core-bench の release ビルド）は本 wrapper が**専有ロック取得後・
# 静穏確認前**に行う。`bench-accept.sh` は `SKIP_BUILD=1` を付けて呼び出し、内部の
# `cargo build` を実行させない。ロック取得前にビルドすると、他の専有計測プロセスが
# 既に共有ロックを保持し計測中の間にホスト負荷を急増させてしまい、`nfr6-exclusive.sh`
# が依存する flock 相互排他保証そのものを崩す（イシュー #260 PR #268 Bugbot 指摘：
# "Pre-lock build breaks exclusivity"）。そのため quiescence-safe な順序
# 「lock → build → wait_for_quiescence → SKIP_BUILD=1 で measure」を採る。ビルド自体は
# ロック保持中に行うため、ビルド完了直後に静穏確認（`wait_for_quiescence`）で
# cargo/rustc 由来の負荷が収まるのを待ってから計測に入る。
#
# 呼び出し元: 週次 schedule（`.github/workflows/bench-schedule.yml`、イシュー #285）
# から `FAIL_RETRIES=1` 付きで呼ばれるほか、人間が `bash
# benches/bench-accept-exclusive.sh` として直接実行することもできる（手動実行時は
# `FAIL_RETRIES` 既定 0 のまま。REQ-1/NFR-1 の PR/push 常設ゲート化は行わない。
# self-hosted runner 負荷抑制方針、.claude/rules/ci.md）。
#
# FAIL_RETRIES（既定 0）: `bench-accept.sh` が終了コード 1（FAIL）を返した場合に
# 限り、同一専有ロック保持中に静穏確認をやり直して指定回数だけ再計測する
# （`benches/lib/exclusive.sh` の `nfr6_run_with_fail_retry`。FAIL が続く限り
# 指定回数まで繰り返すループ、PR #291 Bugbot 指摘対応）。
# `benches/reports/task-1.6-1-performance.md` の申し送りどおり、初回計測が
# keep-alive 再接続ノイズ等で単発 FAIL になった実績があるための頑健化。
# 週次 schedule では `FAIL_RETRIES=1` を使うため「単発 FAIL は 1 回のみ再試行可、
# 2 連続 FAIL で退行確定」という規約は `benches/README.md`「定期実行
# （bench-schedule.yml）」節を参照。0（既定）は再試行なしの従来挙動のまま。
# BLOCKED（終了コード 2）は再試行しない（計測環境自体が壊れているため意味がなく、
# フェイルクローズで即座に BLOCKED を返す。再試行前の静穏確認自体が得られなかった
# 場合も同様に BLOCKED を返す）。呼び出し対象コマンド（`bench-accept.sh`）が決定論的な
# 環境失敗を exit 1 で返さないこと（exit 1 は非決定的な計測 FAIL 専用）という契約は
# `nfr6_run_with_fail_retry`（`benches/lib/exclusive.sh`）の doc comment・
# `benches/README.md` の再試行規約節を参照（イシュー #479）。
#
# 終了コード: `bench-accept.sh` の終了コードをそのまま透過する
# （0 = 全項目 PASS、1 = 1 件以上 FAIL、2 = baseline（axum-ref）/ CORE_BIN いずれかの
# バイナリ未整備で BLOCKED。イシュー #478 で baseline 欠如も CORE_BIN 欠如と同じ
# BLOCKED 専用終了コードへ統一し、両者を非対称に扱っていた旧実装（baseline 欠如は
# exit 1 で性能 FAIL と混同）を解消した）。
# `FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE`（既定 2） = 専有ロック取得不能・ビルド失敗・
# 静穏未達で計測そのものに着手できず BLOCKED（PASS へ丸めない。フェイルクローズ）。
# 変数名は `lib/exclusive.sh` の既存 export をそのまま再利用する（NFR-6 専用の意味は
# 持たず、本 wrapper でも「計測不能時の BLOCKED 終了コード」として共用する）。
#
# REPORT_MD 指定時、専有ロック取得不能・ビルド失敗・静穏未達のいずれで BLOCKED
# 終了する場合も REPORT_MD に「## 結論」セクションを追記し、既存の古い PASS/FAIL が
# 権威として残り続ける事態（stale PASS）を防ぐ（フェイルクローズ、イシュー #260
# Bugbot 指摘対応。`bench-accept.sh` 側の同種処理は `write_report_conclusion` を参照）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=lib/exclusive.sh
source "${SCRIPT_DIR}/lib/exclusive.sh"

REPORT_MD="${REPORT_MD:-}"
# 単発 FAIL の限定再試行回数（既定 0 = 再試行なし、従来挙動と同一）。
FAIL_RETRIES="${FAIL_RETRIES:-0}"
if ! [[ "${FAIL_RETRIES}" =~ ^[0-9]+$ ]]; then
    echo "エラー: FAIL_RETRIES は 0 以上の整数である必要があります（現在: ${FAIL_RETRIES}）" >&2
    exit 1
fi

# `bench-accept.sh` の `write_report_conclusion` と同形式の「## 結論」セクションを
# REPORT_MD に追記する（総合判定行を含まない BLOCKED 用。stale PASS 防止）。
write_blocked_conclusion() {
    local reason="$1"
    if [ -n "${REPORT_MD}" ]; then
        {
            echo
            echo "## 結論（自動記録: bench-accept-exclusive.sh 再計測、$(date -u '+%Y-%m-%dT%H:%M:%SZ')）"
            echo
            echo "**総合判定: BLOCKED（${reason}。既存の古い判定は無効）**"
        } >>"${REPORT_MD}"
    fi
}

release_exclusive_lock_on_exit() {
    release_exclusive_lock
}
trap release_exclusive_lock_on_exit EXIT

echo "=== REQ-2 基準 5 専有計測 wrapper（bench-accept.sh） ===" >&2

echo "--- 専有ロック取得を試行（${FANDHE_BACKEND_NFR6_LOCK}） ---" >&2
if ! acquire_exclusive_lock; then
    echo "BLOCKED: 専有ロックを取得できませんでした。他の計測プロセスが実行中の可能性があります" >&2
    write_blocked_conclusion "専有ロック取得不能のため判定不能"
    exit "${FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE}"
fi
echo "専有ロック取得済み" >&2

echo "--- ビルド（専有ロック取得後。ロック保持中に行い、他の専有計測との同時ビルドを防ぐ） ---" >&2
if ! cargo build --release --manifest-path "${WORKSPACE_ROOT}/Cargo.toml" >&2 \
    || ! cargo build --release --example core-bench -p fandhe-backend-core --manifest-path "${WORKSPACE_ROOT}/Cargo.toml" >&2; then
    echo "BLOCKED: ビルドに失敗しました" >&2
    write_blocked_conclusion "ビルド失敗のため判定不能"
    exit "${FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE}"
fi
echo "ビルド完了" >&2

echo "--- 静穏確認（LOAD1_MAX=${LOAD1_MAX} QUIESCE_WAIT_SECS=${QUIESCE_WAIT_SECS}） ---" >&2
if ! wait_for_quiescence; then
    echo "BLOCKED: ${QUIESCE_WAIT_SECS}s 待っても静穏（loadavg <= ${LOAD1_MAX}・cargo/rustc/oha 不在）が得られませんでした" >&2
    snapshot_environment blocked >&2
    write_blocked_conclusion "静穏未達のため判定不能"
    exit "${FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE}"
fi
echo "静穏確認 OK" >&2
snapshot_environment before >&2

echo "" >&2
echo "### bench-accept.sh 実行開始（SKIP_BUILD=1、事前ビルド済みのため。FAIL_RETRIES=${FAIL_RETRIES}） ###" >&2
set +e
nfr6_run_with_fail_retry "${FAIL_RETRIES}" env SKIP_BUILD=1 REPORT_MD="${REPORT_MD}" bash "${SCRIPT_DIR}/bench-accept.sh"
accept_status=$?
set -e

snapshot_environment after >&2

echo "" >&2
if [ "${accept_status}" -eq 0 ]; then
    echo "=== 総合: PASS（bench-accept.sh 終了コード 0） ===" >&2
elif [ "${accept_status}" -eq 2 ]; then
    echo "=== 総合: BLOCKED（終了コード 2。bench-accept.sh 側の baseline / CORE_BIN 未整備、または再試行前の静穏確認未達のいずれか） ===" >&2
    # PR #291 Bugbot 指摘対応: `nfr6_run_with_fail_retry` が再試行前の静穏未達で
    # BLOCKED（終了コード 2）を返した場合、直前の FAIL 実行が REPORT_MD に書き込んだ
    # 「## 結論: FAIL」がそのまま残ってしまう（stale FAIL）。BLOCKED を返す他の経路
    # （専有ロック取得不能・ビルド失敗・初回静穏未達）と同様に、ここでも
    # `write_blocked_conclusion` を呼び最新の「## 結論」を BLOCKED で追記し直し、
    # 古い FAIL が権威として残らないようにする。
    #
    # reason は上の echo（137 行目）と同じ二択のまま特定しない汎用文言にする:
    # ここで観測できるのは `accept_status == 2` のみで、`bench-accept.sh` 自身の
    # BLOCKED（baseline / CORE_BIN 未整備、既に `bench-accept.sh` 側の
    # `write_report_conclusion` で正しい理由が書き込み済み）か、再試行前の静穏未達
    # （`nfr6_run_with_fail_retry` 由来）かをこの時点の終了コードだけからは判別
    # できない。前者ケースで「再試行前の静穏未達」と誤指定すると、`bench-accept.sh`
    # が書いた正しい理由を誤った理由で上書きしてしまうため、両方の可能性を含む
    # 文言にする（イシュー #478）。
    write_blocked_conclusion "再計測不能（bench-accept.sh 側の BLOCKED、または再試行前の静穏未達のいずれか）のため判定不能"
else
    echo "=== 総合: FAIL（bench-accept.sh 終了コード ${accept_status}。FAIL_RETRIES=${FAIL_RETRIES} を使い切っても FAIL。判定は丸めない） ===" >&2
fi

# bench-accept.sh の終了コードをそのまま透過する（0/1/2 の意味は同スクリプトの
# doc comment を参照。本 wrapper 独自の丸め込みは行わない）。
exit "${accept_status}"
