#!/usr/bin/env bash
# feature-flow-check.sh のセルフテスト（TASK-12.2-1、#81、REQ-12(b)）:
# 一時ディレクトリに fixture 用の git リポジトリを作り、実装変更とテスト追加の
# 組み合わせパターンごとに feature-flow-check.sh の判定を検証する。
# run-triage-tests.sh（scripts/tests/run-triage-tests.sh）と同じく、ネットワーク・
# cargo ビルドに依存せず完結させる（ci.yml の unsafe-triage ジョブから呼ばれる想定）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

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
    if printf '%s' "${haystack}" | grep -qF -- "${needle}"; then
        pass "${desc}"
    else
        fail "${desc}（'${needle}' が出力に含まれません）"
    fi
}

# 一時 git リポジトリを作り、crates/pseudo-crate/{src,tests} の初期状態をコミットする。
# ネットワーク非依存にするため -c user.name/user.email をコマンドラインで指定する
# （グローバル git config を汚さない）。
setup_repo() {
    local dir
    dir="$(mktemp -d)"
    (
        cd "${dir}"
        git init -q
        mkdir -p crates/pseudo-crate/src crates/pseudo-crate/tests
        cat > crates/pseudo-crate/src/lib.rs <<'EOF'
//! テスト用の擬似クレート。
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
EOF
        cat > crates/pseudo-crate/tests/basic.rs <<'EOF'
#[test]
fn add_works() {
    assert_eq!(1 + 1, 2);
}
EOF
        git -c user.name=test -c user.email=test@example.invalid add -A
        git -c user.name=test -c user.email=test@example.invalid commit -q -m "init"
    )
    printf '%s' "${dir}"
}

run_check_in() {
    local repo="$1"
    shift
    BF_FEATURE_FLOW_REPO_ROOT="${repo}" bash "${SCRIPTS_DIR}/feature-flow-check.sh" "$@"
}

commit_all() {
    local repo="$1"
    local msg="$2"
    (
        cd "${repo}"
        git -c user.name=test -c user.email=test@example.invalid add -A
        git -c user.name=test -c user.email=test@example.invalid commit -q -m "${msg}"
    )
}

echo "===== ケース1: src のみ変更（tests・doc test マーカーなし）は fail ====="
REPO1="$(setup_repo)"
BASE1="$(cd "${REPO1}" && git rev-parse HEAD)"
cat >> "${REPO1}/crates/pseudo-crate/src/lib.rs" <<'EOF'

pub fn sub(a: i32, b: i32) -> i32 {
    a - b
}
EOF
commit_all "${REPO1}" "src のみ変更"
set +e
out1="$(run_check_in "${REPO1}" --base "${BASE1}" 2>&1)"
exit1=$?
set -e
assert_exit_code "src のみ変更は exit 1" 1 "${exit1}"
assert_contains "エラーにクレート名 pseudo-crate を含む" "${out1}" "pseudo-crate"
rm -rf "${REPO1}"

echo "===== ケース2: src + tests/ 変更は pass ====="
REPO2="$(setup_repo)"
BASE2="$(cd "${REPO2}" && git rev-parse HEAD)"
cat >> "${REPO2}/crates/pseudo-crate/src/lib.rs" <<'EOF'

pub fn sub(a: i32, b: i32) -> i32 {
    a - b
}
EOF
cat >> "${REPO2}/crates/pseudo-crate/tests/basic.rs" <<'EOF'

#[test]
fn sub_works() {
    assert_eq!(3 - 1, 2);
}
EOF
commit_all "${REPO2}" "src + tests 変更"
set +e
out2="$(run_check_in "${REPO2}" --base "${BASE2}" 2>&1)"
exit2=$?
set -e
assert_exit_code "src + tests 変更は exit 0" 0 "${exit2}"
rm -rf "${REPO2}"

echo "===== ケース3: tests/ のみ変更は pass ====="
REPO3="$(setup_repo)"
BASE3="$(cd "${REPO3}" && git rev-parse HEAD)"
cat >> "${REPO3}/crates/pseudo-crate/tests/basic.rs" <<'EOF'

#[test]
fn another_test() {
    assert!(true);
}
EOF
commit_all "${REPO3}" "tests のみ変更"
set +e
out3="$(run_check_in "${REPO3}" --base "${BASE3}" 2>&1)"
exit3=$?
set -e
assert_exit_code "tests のみ変更は exit 0" 0 "${exit3}"
rm -rf "${REPO3}"

echo "===== ケース4: crates/ 外の変更のみは pass（対象外） ====="
REPO4="$(setup_repo)"
BASE4="$(cd "${REPO4}" && git rev-parse HEAD)"
(
    cd "${REPO4}"
    echo "# note" > README.md
)
commit_all "${REPO4}" "crates 外の変更"
set +e
out4="$(run_check_in "${REPO4}" --base "${BASE4}" 2>&1)"
exit4=$?
set -e
assert_exit_code "crates 外の変更のみは exit 0" 0 "${exit4}"
assert_contains "対象外メッセージを出力する" "${out4}" "対象外"
rm -rf "${REPO4}"

echo "===== ケース5: src のみ変更でも doc test フェンス追加があれば pass ====="
REPO5="$(setup_repo)"
BASE5="$(cd "${REPO5}" && git rev-parse HEAD)"
cat >> "${REPO5}/crates/pseudo-crate/src/lib.rs" <<'EOF'

/// 2 数を掛け算する。
///
/// ```
/// assert_eq!(pseudo_crate::mul(2, 3), 6);
/// ```
pub fn mul(a: i32, b: i32) -> i32 {
    a * b
}
EOF
commit_all "${REPO5}" "doc test 付き src 変更"
set +e
out5="$(run_check_in "${REPO5}" --base "${BASE5}" 2>&1)"
exit5=$?
set -e
assert_exit_code "doc test フェンス追加は exit 0" 0 "${exit5}"
rm -rf "${REPO5}"

echo "===== ケース6: --allow-no-tests で明示除外すると警告付きで pass ====="
REPO6="$(setup_repo)"
BASE6="$(cd "${REPO6}" && git rev-parse HEAD)"
cat >> "${REPO6}/crates/pseudo-crate/src/lib.rs" <<'EOF'

pub fn noop() {}
EOF
commit_all "${REPO6}" "テストなし src 変更（明示除外予定）"
set +e
out6="$(run_check_in "${REPO6}" --base "${BASE6}" --allow-no-tests pseudo-crate "自明な no-op 追加のため" 2>&1)"
exit6=$?
set -e
assert_exit_code "--allow-no-tests 指定時は exit 0" 0 "${exit6}"
assert_contains "除外理由を警告として出力する" "${out6}" "自明な no-op 追加のため"
rm -rf "${REPO6}"

echo "===== ケース7: --base 未指定は使用法エラーで exit 2 ====="
REPO7="$(setup_repo)"
set +e
out7="$(BF_FEATURE_FLOW_REPO_ROOT="${REPO7}" bash "${SCRIPTS_DIR}/feature-flow-check.sh" 2>&1)"
exit7=$?
set -e
assert_exit_code "--base 未指定は exit 2" 2 "${exit7}"
rm -rf "${REPO7}"

echo
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -ne 0 ]; then
    exit 1
fi
exit 0
