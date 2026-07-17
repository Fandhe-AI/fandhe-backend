#!/usr/bin/env bash
# `third-party-verify.sh`（TASK-12.4-1、#85）のセルフテスト。
#
# 引数検証・worktree 判定ロジック（PENDING 判定、独立性チェック）を
# 高速に確認する層と、実 cargo ビルドを伴う機械ゲート（fmt/clippy/test の
# PASS・FAIL 検出）を確認する層に分ける。後者は `git worktree add` で使い捨て
# fixture を作るため実行に時間がかかる（フル層、既定は無指定で実行）。
#
#   --offline: 引数検証・PENDING 判定のみ（cargo ビルド不要、高速）
#   （無指定）: 上記に加えてフル層（cargo fmt/clippy/test を実際に実行する PASS/FAIL 検出）
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${SCRIPTS_DIR}/.." && pwd)"
HARNESS="${SCRIPTS_DIR}/third-party-verify.sh"

MODE="full"
if [ "${1:-}" = "--offline" ]; then
    MODE="offline"
fi

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

assert_exit_code() {
    local desc="$1"
    local expected="$2"
    shift 2
    local actual
    "$@" >/tmp/third-party-verify-test-output.log 2>&1
    actual=$?
    if [ "${actual}" -eq "${expected}" ]; then
        pass "${desc}（終了コード ${actual}）"
    else
        fail "${desc}（期待: ${expected}、実際: ${actual}）"
        cat /tmp/third-party-verify-test-output.log >&2
    fi
}

assert_output_contains() {
    local desc="$1"
    local needle="$2"
    if grep -qF -- "${needle}" /tmp/third-party-verify-test-output.log; then
        pass "${desc}"
    else
        fail "${desc}（'${needle}' が出力に含まれません）"
        cat /tmp/third-party-verify-test-output.log >&2
    fi
}

echo "===== オフライン層: 引数検証・PENDING 判定 ====="

assert_exit_code "引数なしは exit 2" 2 bash "${HARNESS}"
assert_output_contains "引数なしのメッセージ" "PENDING"

assert_exit_code "存在しない worktree は exit 0（PENDING）" 0 bash "${HARNESS}" --worktree /nonexistent/path-xyz --task-id T-TEST
assert_output_contains "存在しない worktree のメッセージ" "worktree が見つかりません"

assert_exit_code "メイン working copy 自体は exit 0（PENDING、誤爆防止）" 0 bash "${HARNESS}" --worktree "${REPO_ROOT}" --task-id T-TEST
assert_output_contains "working copy 誤爆防止のメッセージ" "worktree にメイン working copy 自体は指定できません"

TMP_NON_GIT_DIR="$(mktemp -d)"
assert_exit_code "git worktree でないディレクトリは exit 0（PENDING）" 0 bash "${HARNESS}" --worktree "${TMP_NON_GIT_DIR}" --task-id T-TEST
assert_output_contains "非 git ディレクトリのメッセージ" "git worktree ではありません"
rmdir "${TMP_NON_GIT_DIR}"

if [ "${MODE}" = "offline" ]; then
    echo "===== --offline モードのためフル層はスキップ ====="
else
    echo "===== フル層: 機械ゲートの PASS/FAIL 検出（cargo ビルドを伴うため時間を要する） ====="

    FIXTURE_DIR="$(mktemp -d)/third-party-verify-fixture"
    cleanup_fixture() {
        (cd "${REPO_ROOT}" && git worktree remove "${FIXTURE_DIR}" --force) >/dev/null 2>&1 || true
        rm -rf "${FIXTURE_DIR}"
    }
    trap cleanup_fixture EXIT

    if ! (cd "${REPO_ROOT}" && git worktree add "${FIXTURE_DIR}" HEAD) >/tmp/third-party-verify-test-fixture-setup.log 2>&1; then
        fail "fixture worktree の作成に失敗しました（詳細: /tmp/third-party-verify-test-fixture-setup.log）"
    else
        assert_exit_code "健全な worktree（HEAD 相当）は exit 0（PASS）" 0 bash "${HARNESS}" --worktree "${FIXTURE_DIR}" --task-id T-TEST-GOOD
        assert_output_contains "PASS ケースのメッセージ" "機械ゲートは PASS した"

        # 意図的にフォーマット崩れを注入し FAIL 検出を確認する（PoC-9 型の退行注入パターン
        # を踏襲。scripts/tests/run-review-gate-tests.sh の deny lint 検出テストと同型）。
        printf '\nfn   badly_formatted_fixture_fn(  )   {}\n' >>"${FIXTURE_DIR}/crates/core/src/lib.rs"
        assert_exit_code "fmt 崩れを注入した worktree は exit 1（FAIL）" 1 bash "${HARNESS}" --worktree "${FIXTURE_DIR}" --task-id T-TEST-BAD
        assert_output_contains "FAIL ケースのメッセージ" "機械ゲート不通過"
    fi
fi

echo "==================================================="
echo "third-party-verify.sh セルフテスト結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT}"
echo "==================================================="

if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
exit 0
