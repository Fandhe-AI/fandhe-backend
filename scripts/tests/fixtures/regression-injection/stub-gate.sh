#!/usr/bin/env bash
# regression-injection-verify.sh のセルフテスト用スタブゲート（cargo 非依存）。
#
# `regression-injection-verify.sh --gate-cmd` から `<worktree-dir> <case-id>` として
# 呼ばれる。環境変数 `STUB_MISS_IDS`（スペース区切りの case-id 一覧）に含まれる
# ケースは「全ゲート通過（検知漏れ）」として終了コード 0 を返し、含まれないケースは
# 「検知」として終了コード 1 を返す。`STUB_TIMEOUT_IDS` に含まれるケースは
# 呼び出し元の `timeout` にタイムアウトさせるため長時間 sleep する。
set -uo pipefail

: "${1:?worktree dir required}"
CASE_ID="${2:?case id required}"

for id in ${STUB_TIMEOUT_IDS:-}; do
    if [ "${id}" = "${CASE_ID}" ]; then
        sleep 999
        exit 0
    fi
done

for id in ${STUB_MISS_IDS:-}; do
    if [ "${id}" = "${CASE_ID}" ]; then
        echo "case ${CASE_ID}: stub gate PASS (simulated missed detection)"
        exit 0
    fi
done

echo "case ${CASE_ID}: stub gate FAIL (simulated detection)"
exit 1
