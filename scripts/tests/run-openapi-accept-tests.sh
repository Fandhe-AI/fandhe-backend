#!/usr/bin/env bash
# openapi-accept.sh のセルフテスト（TASK-3.3、#32）。
#
# `scripts/accept/openapi-accept.sh` はネットワーク・cargo ビルド・`openapi-spec-validator`
# の有無に依存するため、本スクリプトは判定ロジックの部分（CI ジョブ存在確認の grep
# パターン・`lib/common.sh` の PASS/FAIL/SKIP 集計と終了コードの対応）を fixture・直接呼び出し
# で切り出して検証する。`run-pay-for-what-you-use-tests.sh` 等と同じくネットワーク・cargo
# ビルドに依存せず完結させる。
#
# 検証範囲外（本スクリプトが担わないもの）:
#   - openapi-accept.sh 全体の実行結果そのもの（cargo test 実行・validator 呼び出しを
#     含むため、CI・人間によるローカル実行で確認する）
#   - openapi-spec-validator 自体の判定精度（ツール側の責務）
#
# 呼び出し元: `.github/workflows/ci.yml` の unsafe-triage ジョブから既存セルフテスト群と
# 同列で呼ばれる想定（追加は別途 CI 変更で行う）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/openapi-accept"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

PASS_COUNT=0
FAIL_COUNT=0

pass() {
    echo "PASS: $1"
    PASS_COUNT=$((PASS_COUNT + 1))
}

fail() {
    echo "FAIL: $1" >&2
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

# openapi-accept.sh 節 5 が使う grep パターンと同一条件を fixture に対して適用する
# （ジョブ名・スクリプト呼び出しの両方が存在して初めて PASS 相当と判定する2条件 AND）。
ci_job_check() {
    local file="$1"
    grep -q "openapi-two-stage:" "${file}" && grep -q "scripts/openapi-two-stage.sh" "${file}"
}

echo "===== CI 2 段階ビルドジョブ存在確認（節 5）のロジック検証 ====="

if ci_job_check "${FIXTURES_DIR}/ci-with-job.yml"; then
    pass "openapi-two-stage ジョブ + スクリプト呼び出しを含む fixture は PASS 相当と判定される"
else
    fail "openapi-two-stage ジョブ + スクリプト呼び出しを含む fixture が PASS 相当と判定されなかった"
fi

if ! ci_job_check "${FIXTURES_DIR}/ci-without-job.yml"; then
    pass "openapi-two-stage ジョブを含まない fixture は FAIL 相当と判定される"
else
    fail "openapi-two-stage ジョブを含まない fixture が誤って PASS 相当と判定された"
fi

echo ""
echo "===== 実リポジトリの ci.yml に対する節 5 ロジックの疎通確認 ====="
if ci_job_check "${WORKSPACE_ROOT}/.github/workflows/ci.yml"; then
    pass "実リポジトリの .github/workflows/ci.yml は openapi-two-stage ジョブを含む（TASK-3.2 実装済みの回帰検知）"
else
    fail "実リポジトリの .github/workflows/ci.yml から openapi-two-stage ジョブが検出できない（退行の可能性）"
fi

echo ""
echo "===== lib/common.sh の PASS/FAIL/SKIP 集計と終了コードの対応検証 ====="

# サブシェルで lib/common.sh を source し、record_* の組み合わせごとに
# summary_exit_code() が正しい終了コードを返すことを検証する（openapi-accept.sh の
# 「SKIP は判定不能の安全側記録であり非 0 終了させない」という設計方針そのものを固定化する）。
check_exit_code() {
    local desc="$1"
    local expected="$2"
    shift 2
    local actual
    actual="$(
        # shellcheck source=../accept/lib/common.sh
        source "${WORKSPACE_ROOT}/scripts/accept/lib/common.sh" >/dev/null
        for entry in "$@"; do
            "record_${entry%%:*}" "criterion" "${entry#*:}" >/dev/null
        done
        summary_exit_code
    )"
    if [ "${actual}" -eq "${expected}" ]; then
        pass "${desc}（exit code: ${actual}）"
    else
        fail "${desc}（期待 exit code: ${expected}, 実際: ${actual}）"
    fi
}

check_exit_code "PASS のみ → exit 0" 0 "pass:ok"
check_exit_code "SKIP のみ → exit 0（判定不能を非 0 にしない）" 0 "skip:blocked"
check_exit_code "PASS + SKIP 混在 → exit 0" 0 "pass:ok" "skip:blocked"
check_exit_code "FAIL を含む → exit 1" 1 "pass:ok" "skip:blocked" "fail:ng"

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
