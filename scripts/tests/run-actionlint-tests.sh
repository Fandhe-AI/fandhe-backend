#!/usr/bin/env bash
# actionlint.sh のセルフテスト（TASK-15（#180））。
#
# 検証範囲:
#   1. actionlint 不在時の fail-closed（PATH を空にして exit 2・導入案内メッセージ）
#   2. 陰性対照（discrimination）: 壊れた workflow fixture
#      （scripts/tests/fixtures/actionlint/broken-workflow.yml）を明示ファイル引数で
#      検査し、actionlint 本体が非 0 で検出すること（actionlint 導入済み前提、PATH 制御なし）
#   3. .github/workflows/ci.yml に actionlint ジョブ・ci-complete needs 登録が存在すること
#
# ケース 2 は actionlint 本体の実行を要するため、ci.yml の actionlint ジョブ内
# （Ensure actionlint ステップの後）から呼ぶ想定。ネットワーク・cargo ビルドは
# 使わない（既存 run-*-tests.sh と同じ完結方針）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${SCRIPTS_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/actionlint"

PASS_COUNT=0
FAIL_COUNT=0

fail() {
    echo "FAIL: $1" >&2
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

pass() {
    echo "PASS: $1"
    PASS_COUNT=$((PASS_COUNT + 1))
}

assert_exit_code() {
    local desc="$1"
    local expected="$2"
    local actual="$3"
    if [ "${expected}" -eq "${actual}" ]; then
        pass "${desc}（exit code: ${actual}）"
    else
        fail "${desc}（期待 exit code: ${expected}, 実際: ${actual}）"
    fi
}

assert_contains() {
    local desc="$1"
    local haystack="$2"
    local needle="$3"
    # #511/#514: パイプ経由の grep -q 判定は set -euo pipefail 下で SIGPIPE/EPIPE により
    # 誤 FAIL・誤 pass を招くため bash 組み込みパターンマッチを使う。needle は必ず
    # ダブルクォートで囲み glob メタ文字を文字どおりに扱わせる。
    if [[ "${haystack}" == *"${needle}"* ]]; then
        pass "${desc}"
    else
        fail "${desc}（'${needle}' が出力に含まれません）"
    fi
}

echo "=== actionlint.sh セルフテスト ==="

# --- ケース 1: actionlint 不在時は fail-closed（exit 2・導入案内） ---
# PATH から actionlint を除外した最小 PATH（bash 自体・coreutils は残す）で実行し、
# 「見つからない場合は自動導入せず案内して終了する」既存方針（fuzz.sh 等）を固定する。
minimal_path="$(printf '%s\n' "${PATH}" | tr ':' '\n' | grep -v -x -F "$(dirname "$(command -v actionlint 2>/dev/null || echo /nonexistent)")" | paste -sd: -)"
set +e
output="$(PATH="${minimal_path}" bash "${SCRIPTS_DIR}/actionlint.sh" 2>&1)"
status=$?
set -e
assert_exit_code "actionlint 不在時は exit 2（フェイルクローズ）" 2 "${status}"
assert_contains "actionlint 不在時は導入コマンドを案内する" "${output}" "actionlint が見つかりません"

# --- ケース 2: 陰性対照（discrimination）— 壊れた workflow fixture は非 0 検出 ---
# actionlint 本体を要するため、導入済みの場合のみ実行する（未導入環境ではケース 1 の
# fail-closed 確認のみで完結させ、SKIP を明示する）。
if command -v actionlint >/dev/null 2>&1; then
    set +e
    output="$(bash "${SCRIPTS_DIR}/actionlint.sh" "${FIXTURES_DIR}/broken-workflow.yml" 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        fail "壊れた workflow fixture は非 0 で検出されるべき（discrimination 失敗、実際: exit ${status}）"
    else
        pass "壊れた workflow fixture は非 0 で検出される（discrimination、exit ${status}）"
    fi
    assert_contains "壊れた workflow fixture は runs-on 欠落を報告する" "${output}" "runs-on"
    assert_contains "壊れた workflow fixture は needs 参照切れを報告する" "${output}" "does-not-exist"
else
    echo "SKIP: actionlint 未導入のため陰性対照（discrimination）テストを省略します"
fi

# --- ケース 3: ci.yml に actionlint ジョブ・ci-complete needs 登録が存在すること ---
ci_yml="${REPO_ROOT}/.github/workflows/ci.yml"
if grep -qE '^  actionlint:' "${ci_yml}"; then
    pass "ci.yml に actionlint ジョブが定義されている"
else
    fail "ci.yml に actionlint ジョブが見つからない"
fi

if grep -qE '^\s*needs:.*\bactionlint\b' "${ci_yml}"; then
    pass "ci-complete の needs に actionlint が登録されている"
else
    fail "ci-complete の needs に actionlint が見つからない"
fi

if grep -qE 'RESULT_ACTIONLINT' "${ci_yml}"; then
    pass "ci-complete の判定ループに RESULT_ACTIONLINT が登録されている"
else
    fail "ci-complete の判定ループに RESULT_ACTIONLINT が見つからない"
fi

echo ""
echo "=== 結果: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
