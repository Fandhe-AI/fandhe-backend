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
# 前提: axum-ref・core-bench の release ビルドは `bench-accept.sh` 内部で行われるため
# 本 wrapper で事前ビルドは必須ではないが、専有ロック取得後にビルドが走ると静穏確認の
# 意味が薄れるため、事前に以下を実行しておくことを推奨する:
#   cargo build --release
#   cargo build --release --example core-bench -p fandhe-backend-core
#
# 呼び出し元: 人間が `bash benches/bench-accept-exclusive.sh` として直接実行する
# （CI 常設ジョブへは組み込まない。self-hosted runner 負荷抑制方針、.claude/rules/ci.md）。
#
# 終了コード: `bench-accept.sh` の終了コードをそのまま透過する
# （0 = 全項目 PASS、1 = 1 件以上 FAIL、2 = CORE_BIN 未整備で BLOCKED）。
# `FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE`（既定 2） = 専有ロック取得不能・静穏未達で
# 計測そのものに着手できず BLOCKED（PASS へ丸めない。フェイルクローズ）。
# 変数名は `lib/exclusive.sh` の既存 export をそのまま再利用する（NFR-6 専用の意味は
# 持たず、本 wrapper でも「計測不能時の BLOCKED 終了コード」として共用する）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/exclusive.sh
source "${SCRIPT_DIR}/lib/exclusive.sh"

release_exclusive_lock_on_exit() {
    release_exclusive_lock
}
trap release_exclusive_lock_on_exit EXIT

echo "=== REQ-2 基準 5 専有計測 wrapper（bench-accept.sh） ===" >&2

echo "--- 専有ロック取得を試行（${FANDHE_BACKEND_NFR6_LOCK}） ---" >&2
if ! acquire_exclusive_lock; then
    echo "BLOCKED: 専有ロックを取得できませんでした。他の計測プロセスが実行中の可能性があります" >&2
    exit "${FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE}"
fi
echo "専有ロック取得済み" >&2

echo "--- 静穏確認（LOAD1_MAX=${LOAD1_MAX} QUIESCE_WAIT_SECS=${QUIESCE_WAIT_SECS}） ---" >&2
if ! wait_for_quiescence; then
    echo "BLOCKED: ${QUIESCE_WAIT_SECS}s 待っても静穏（loadavg <= ${LOAD1_MAX}・cargo/rustc/oha 不在）が得られませんでした" >&2
    snapshot_environment blocked >&2
    exit "${FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE}"
fi
echo "静穏確認 OK" >&2
snapshot_environment before >&2

echo "" >&2
echo "### bench-accept.sh 実行開始 ###" >&2
set +e
bash "${SCRIPT_DIR}/bench-accept.sh"
accept_status=$?
set -e

snapshot_environment after >&2

echo "" >&2
if [ "${accept_status}" -eq 0 ]; then
    echo "=== 総合: PASS（bench-accept.sh 終了コード 0） ===" >&2
elif [ "${accept_status}" -eq 2 ]; then
    echo "=== 総合: BLOCKED（bench-accept.sh 終了コード 2。CORE_BIN 未整備） ===" >&2
else
    echo "=== 総合: FAIL（bench-accept.sh 終了コード ${accept_status}。判定は丸めない） ===" >&2
fi

# bench-accept.sh の終了コードをそのまま透過する（0/1/2 の意味は同スクリプトの
# doc comment を参照。本 wrapper 独自の丸め込みは行わない）。
exit "${accept_status}"
