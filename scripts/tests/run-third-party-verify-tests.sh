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

# 固定名の一時ファイルは共有 /tmp 環境でシンボリックリンク攻撃・競合のリスクがある
# （本体の third-party-verify.sh の GATE_LOG と同様に mktemp を使う。レビュー指摘、Issue #85）。
TEST_OUTPUT_LOG="$(mktemp)"
trap 'rm -f "${TEST_OUTPUT_LOG}"' EXIT

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
    "$@" >"${TEST_OUTPUT_LOG}" 2>&1
    actual=$?
    if [ "${actual}" -eq "${expected}" ]; then
        pass "${desc}（終了コード ${actual}）"
    else
        fail "${desc}（期待: ${expected}、実際: ${actual}）"
        cat "${TEST_OUTPUT_LOG}" >&2
    fi
}

assert_output_contains() {
    local desc="$1"
    local needle="$2"
    if grep -qF -- "${needle}" "${TEST_OUTPUT_LOG}"; then
        pass "${desc}"
    else
        fail "${desc}（'${needle}' が出力に含まれません）"
        cat "${TEST_OUTPUT_LOG}" >&2
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
    FIXTURE_SETUP_LOG="$(mktemp)"
    BASELINE_LOG="$(mktemp)"
    cleanup_fixture() {
        (cd "${REPO_ROOT}" && git worktree remove "${FIXTURE_DIR}" --force) >/dev/null 2>&1 || true
        rm -rf "${FIXTURE_DIR}"
        rm -f "${TEST_OUTPUT_LOG}" "${FIXTURE_SETUP_LOG}" "${BASELINE_LOG}"
    }
    trap cleanup_fixture EXIT

    if ! (cd "${REPO_ROOT}" && git worktree add "${FIXTURE_DIR}" HEAD) >"${FIXTURE_SETUP_LOG}" 2>&1; then
        fail "fixture worktree の作成に失敗しました（詳細: ${FIXTURE_SETUP_LOG}）"
    else
        assert_exit_code "健全な worktree（HEAD 相当）は exit 0（PASS）" 0 bash "${HARNESS}" --worktree "${FIXTURE_DIR}" --task-id T-TEST-GOOD
        assert_output_contains "PASS ケースのメッセージ" "機械ゲートは PASS した"

        # リグレッション突合: 起点コミットで既に失敗していたテストは新規リグレッションとして
        # 検出されないこと、起点コミットにないテストが新規に失敗した場合は検出されることを
        # 確認する（旧実装は grep パターンが nextest の実出力と一致せず、かつ突合ブロック自体が
        # 到達不能な構造だったため機能していなかった。レビュー指摘、Issue #85）。
        cat >"${BASELINE_LOG}" <<'BASELOG'
        FAIL [   0.003s] (1/1) backend-framework-core third_party_verify_fixture_baseline::third_party_verify_fixture_pre_existing_failure
BASELOG
        printf '\n#[cfg(test)]\nmod third_party_verify_fixture_baseline {\n    #[test]\n    fn third_party_verify_fixture_pre_existing_failure() {\n        assert!(false);\n    }\n}\n' >>"${FIXTURE_DIR}/crates/core/src/lib.rs"
        assert_exit_code "起点コミット由来の既知の失敗のみは baseline 突合で exit 0（PASS、リグレッションなし）" 0 bash "${HARNESS}" --worktree "${FIXTURE_DIR}" --task-id T-TEST-BASELINE-KNOWN --baseline-tests "${BASELINE_LOG}"
        assert_output_contains "既知失敗のみのメッセージ" "リグレッションなし"

        printf '\n#[cfg(test)]\nmod third_party_verify_fixture_new_regression {\n    #[test]\n    fn third_party_verify_fixture_new_failure() {\n        assert!(false);\n    }\n}\n' >>"${FIXTURE_DIR}/crates/core/src/lib.rs"
        assert_exit_code "起点コミットにない新規失敗テストは baseline 突合で exit 1（FAIL、リグレッション検出）" 1 bash "${HARNESS}" --worktree "${FIXTURE_DIR}" --task-id T-TEST-BASELINE-NEW --baseline-tests "${BASELINE_LOG}"
        assert_output_contains "新規リグレッションのメッセージ" "リグレッション検出"

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
