#!/usr/bin/env bash
# イシュー #473: `compression-e2e-bench.sh`（compression 有効構成の並行負荷下
# E2E p99 比較）の専有計測 wrapper（`benches/bench-accept-exclusive.sh` の
# 構造を踏襲）。
#
# `benches/lib/exclusive.sh` の専有実行枠（flock によるホストグローバル
# 相互排他・静穏（quiescence）確認・環境スナップショット）を取得したうえで
# `compression-e2e-bench.sh` を 1 回実行する。並列 issue 実装ワークフロー下では
# host contention により p99 が大きく振れうるため（`benches/nfr6-exclusive.sh`
# と同根の問題）、専有計測なしでは判定が確定しないおそれがある。
#
# 本スクリプトは **PASS/FAIL の閾値判定を持たない**（`bench-accept.sh` とは
# 異なり比較計測であり受け入れテストではない）。実測値の判定
# （しきい値 64 KiB 維持 or 見直し要）は `benches/reports/
# issue473-compression-e2e.md` へ人間可読な形で記録する。
#
# ビルド（`compression_e2e_bench` example の release ビルド）は本 wrapper が
# **専有ロック取得後・静穏確認前**に行う。ロック取得前にビルドすると、他の
# 専有計測プロセスが既に共有ロックを保持し計測中の間にホスト負荷を急増させて
# しまい、`nfr6-exclusive.sh` が依存する flock 相互排他保証そのものを崩す
# （イシュー #260 PR #268 Bugbot 指摘 "Pre-lock build breaks exclusivity" の
# 再発防止。`bench-accept-exclusive.sh` と同一の順序規約
# 「lock → build → wait_for_quiescence → measure」を踏襲する）。
#
# 終了コード:
#   0 = 計測成功（`compression-e2e-bench.sh` が正常終了）
#   1 = `compression-e2e-bench.sh` が異常終了（前提ツール欠如・oha 実行失敗等）
#   `FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE`（既定 2） = 専有ロック取得不能・
#     ビルド失敗・静穏未達で計測そのものに着手できず BLOCKED（PASS へ丸めない。
#     フェイルクローズ）
#
# 呼び出し元: 人間が `bash benches/compression-e2e-exclusive.sh` として直接
# 実行する（週次 schedule への組み込みは不採用。理由は
# `docs/design/plugin-boundary.md` 5.10.7 節「E2E 検証（イシュー #473）」
# 小節・`benches/reports/issue473-compression-e2e.md` を参照）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=lib/exclusive.sh
source "${SCRIPT_DIR}/lib/exclusive.sh"

release_exclusive_lock_on_exit() {
    release_exclusive_lock
}
trap release_exclusive_lock_on_exit EXIT

echo "=== イシュー #473 compression E2E 専有計測 wrapper ===" >&2

echo "--- 専有ロック取得を試行（${FANDHE_BACKEND_NFR6_LOCK}） ---" >&2
if ! acquire_exclusive_lock; then
    echo "BLOCKED: 専有ロックを取得できませんでした。他の計測プロセスが実行中の可能性があります" >&2
    exit "${FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE}"
fi
echo "専有ロック取得済み" >&2

echo "--- ビルド（専有ロック取得後。ロック保持中に行い、他の専有計測との同時ビルドを防ぐ） ---" >&2
if ! cargo build --release --example compression_e2e_bench -p fandhe-backend-core \
    --features compression --manifest-path "${WORKSPACE_ROOT}/Cargo.toml" >&2; then
    echo "BLOCKED: ビルドに失敗しました" >&2
    exit "${FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE}"
fi
echo "ビルド完了" >&2

echo "--- 静穏確認（LOAD1_MAX=${LOAD1_MAX} QUIESCE_WAIT_SECS=${QUIESCE_WAIT_SECS}） ---" >&2
if ! wait_for_quiescence; then
    echo "BLOCKED: ${QUIESCE_WAIT_SECS}s 待っても静穏（loadavg <= ${LOAD1_MAX}・cargo/rustc/oha 不在）が得られませんでした" >&2
    snapshot_environment blocked >&2
    exit "${FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE}"
fi
echo "静穏確認 OK" >&2
snapshot_environment before >&2

echo "" >&2
echo "### compression-e2e-bench.sh 実行開始 ###" >&2
set +e
bash "${SCRIPT_DIR}/compression-e2e-bench.sh"
bench_status=$?
set -e

snapshot_environment after >&2

echo "" >&2
if [ "${bench_status}" -eq 0 ]; then
    echo "=== 総合: 計測成功（compression-e2e-bench.sh 終了コード 0） ===" >&2
else
    echo "=== 総合: 計測失敗（compression-e2e-bench.sh 終了コード ${bench_status}） ===" >&2
fi

exit "${bench_status}"
