#!/usr/bin/env bash
# NFR-6（docs/spec/04-requirements.md）専有計測 wrapper（TASK-15.4 相当 / #178）。
#
# このスクリプトの役割:
#   `benches/lib/exclusive.sh` の専有実行枠（flock 相互排他・静穏確認・環境
#   スナップショット）を取得したうえで、`benches/{webrtc,graphql,hub}-nfr6-bench.sh`
#   を順次実行し、`scripts/accept/lib/nfr6-ratio.sh` の `evaluate_nfr6_ratio` で
#   PASS/WARN/FAIL を確定する。REQ-5（GraphQL）・REQ-8（WebRTC）・REQ-9（hub-wiring）
#   の NFR-6 受け入れが host contention により振れて確定できなかった問題
#   （`benches/reports/task-9.5-hub-wiring-performance.md` 診断）への対処。
#
# 前提: 各対象の release バイナリを事前にビルドしておくこと（自動ビルドしない。
# 個々の `*-nfr6-bench.sh` が存在検査してエラーメッセージを出す）。
#   cargo build --release -p fandhe-backend-core --example minimal --no-default-features
#   cargo build --release -p fandhe-backend-core --example webrtc_nfr6 --features webrtc
#   cargo build --release -p fandhe-backend-core --example graphql_nfr6 --features graphql
#   cargo build --release -p fandhe-backend-plugin-hub-wiring --example hub_link_only
#
# 呼び出し元: 人間が `bash benches/nfr6-exclusive.sh` として直接実行する
# （CI 常設ジョブへは組み込まない。self-hosted runner 負荷抑制方針、.claude/rules/ci.md）。
# 対象は `TARGETS`（既定 "webrtc graphql hub"）で選択可。
#
# 終了コード: 0 = 全対象を計測・判定確定（PASS/WARN/FAIL いずれでも判定を
# 偽らず記録する。FAIL 残存時も 0 で終える —— 判定確定こそが本スクリプトの
# 責務であり、判定結果の是非は人間レビューが担う）。
# `BF_NFR6_BLOCKED_EXIT_CODE`（既定 2） = 静穏未達またはロック取得不能で
# 計測不能（BLOCKED。PASS へ丸めない。フェイルクローズ）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=lib/exclusive.sh
source "${SCRIPT_DIR}/lib/exclusive.sh"
# shellcheck source=../scripts/accept/lib/nfr6-ratio.sh
source "${WORKSPACE_ROOT}/scripts/accept/lib/nfr6-ratio.sh"

TARGETS="${TARGETS:-webrtc graphql hub}"

# 対象名 → bench スクリプトファイル名の対応。
declare -A BENCH_SCRIPT=(
    [webrtc]="webrtc-nfr6-bench.sh"
    [graphql]="graphql-nfr6-bench.sh"
    [hub]="hub-nfr6-bench.sh"
)

release_exclusive_lock_on_exit() {
    release_exclusive_lock
}
trap release_exclusive_lock_on_exit EXIT

echo "=== NFR-6 専有計測 wrapper（TARGETS=${TARGETS}） ===" >&2

echo "--- 専有ロック取得を試行（${BF_NFR6_LOCK}） ---" >&2
if ! acquire_exclusive_lock; then
    echo "BLOCKED: 専有ロックを取得できませんでした。他の計測プロセスが実行中の可能性があります" >&2
    exit "${BF_NFR6_BLOCKED_EXIT_CODE}"
fi
echo "専有ロック取得済み" >&2

echo "--- 静穏確認（LOAD1_MAX=${LOAD1_MAX} QUIESCE_WAIT_SECS=${QUIESCE_WAIT_SECS}） ---" >&2
if ! wait_for_quiescence; then
    echo "BLOCKED: ${QUIESCE_WAIT_SECS}s 待っても静穏（loadavg <= ${LOAD1_MAX}・cargo/rustc/oha 不在）が得られませんでした" >&2
    snapshot_environment blocked >&2
    exit "${BF_NFR6_BLOCKED_EXIT_CODE}"
fi
echo "静穏確認 OK" >&2
snapshot_environment before >&2

OVERALL_STATUS=0

for target in ${TARGETS}; do
    script_name="${BENCH_SCRIPT[${target}]:-}"
    if [ -z "${script_name}" ]; then
        echo "エラー: 未知の対象 '${target}'（有効値: ${!BENCH_SCRIPT[*]}）" >&2
        exit 1
    fi

    # 対象ごとに計測直前で静穏を再確認する（前対象の計測完了直後は自プロセス
    # ツリーの残留、他ジョブの割り込み開始等が起きうるため）。
    echo "" >&2
    echo "### 対象: ${target}（${script_name}） 静穏再確認 ###" >&2
    if ! wait_for_quiescence; then
        echo "BLOCKED: 対象 ${target} の計測直前に静穏を再取得できませんでした" >&2
        snapshot_environment "blocked-${target}" >&2
        exit "${BF_NFR6_BLOCKED_EXIT_CODE}"
    fi

    echo "### 対象: ${target} 計測開始 ###" >&2
    bench_output="$(bash "${SCRIPT_DIR}/${script_name}")"
    echo "${bench_output//$'\n'/$'\n'    }" >&2

    rps_ratio="$(echo "${bench_output}" | grep '^rps_ratio_pct=' | cut -d= -f2)"
    p95_ratio="$(echo "${bench_output}" | grep '^p95_ratio_pct=' | cut -d= -f2)"

    verdict="$(evaluate_nfr6_ratio "${rps_ratio}" "${p95_ratio}")"
    echo "判定（${target}）: ${verdict}（rps_ratio_pct=${rps_ratio} p95_ratio_pct=${p95_ratio}）" >&2

    if [ "${verdict}" = "FAIL" ]; then
        OVERALL_STATUS=1
    fi

    # machine-readable な結果を stdout へ（レポート転記の自動化用）。
    printf 'target=%s rps_ratio_pct=%s p95_ratio_pct=%s verdict=%s\n' \
        "${target}" "${rps_ratio}" "${p95_ratio}" "${verdict}"
done

snapshot_environment after >&2

echo "" >&2
if [ "${OVERALL_STATUS}" -ne 0 ]; then
    echo "=== 総合: 少なくとも 1 対象が FAIL（判定は丸めない。レポート追補へ正直に記録すること） ===" >&2
else
    echo "=== 総合: 全対象が PASS/WARN ===" >&2
fi

# 判定確定こそが本スクリプトの責務であり、FAIL 残存の是非判断は人間レビューへ
# 委ねるため、FAIL があっても非 0 では終了しない（BLOCKED とは区別する）。
exit 0
