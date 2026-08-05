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

# haystack に needle が固定文字列として含まれるかを判定する（#511/#514: パイプ経由の
# grep -q 判定は set -euo pipefail 下で SIGPIPE/EPIPE により誤 FAIL・誤 pass を招くため
# bash 組み込みパターンマッチを使う。needle は必ずダブルクォートで囲み glob メタ文字を
# 文字どおりに扱わせる）。
assert_contains() {
    local desc="$1"
    local haystack="$2"
    local needle="$3"
    if [[ "${haystack}" == *"${needle}"* ]]; then
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
        cat > crates/pseudo-crate/Cargo.toml <<'EOF'
[package]
name = "pseudo-crate-pkg"
version = "0.1.0"
edition = "2021"
EOF
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
    FANDHE_BACKEND_FEATURE_FLOW_REPO_ROOT="${repo}" bash "${SCRIPTS_DIR}/feature-flow-check.sh" "$@"
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
out7="$(FANDHE_BACKEND_FEATURE_FLOW_REPO_ROOT="${REPO7}" bash "${SCRIPTS_DIR}/feature-flow-check.sh" 2>&1)"
exit7=$?
set -e
assert_exit_code "--base 未指定は exit 2" 2 "${exit7}"
rm -rf "${REPO7}"

echo "===== ケース8: src 内の既存 #[test] 本体のみ編集（新規マーカー行なし）は pass ====="
# Bugbot 指摘（scripts/feature-flow-check.sh#L126-L141）の回帰テスト:
# 追加行自体にテストマーカーが現れない編集（既存 #[test] 関数のアサーション変更）
# でも、その差分の文脈に既存のテストマーカーが含まれていれば検知できることを確認する。
REPO8="$(setup_repo)"
(
    cd "${REPO8}"
    cat >> crates/pseudo-crate/src/lib.rs <<'EOF'

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_works_inline() {
        assert_eq!(add(1, 1), 2);
    }
}
EOF
)
commit_all "${REPO8}" "既存テストなし状態から inline テストモジュールを追加"
BASE8="$(cd "${REPO8}" && git rev-parse HEAD)"
sed -i.bak 's/assert_eq!(add(1, 1), 2);/assert_eq!(add(1, 1), 2); \/\/ 変更/' \
    "${REPO8}/crates/pseudo-crate/src/lib.rs"
rm -f "${REPO8}/crates/pseudo-crate/src/lib.rs.bak"
commit_all "${REPO8}" "既存 #[test] 本体のみ編集（新規マーカー行なし）"
set +e
out8="$(run_check_in "${REPO8}" --base "${BASE8}" 2>&1)"
exit8=$?
set -e
assert_exit_code "既存テスト本体のみの編集は exit 0" 0 "${exit8}"
rm -rf "${REPO8}"

echo "===== ケース9: --allow-no-tests に Cargo パッケージ名（ディレクトリ名と異なる）を指定しても許容される ====="
# Bugbot 指摘（scripts/feature-flow-check.sh#L66-L75, #L95-L105）の回帰テスト:
# --allow-no-tests はディレクトリ名（pseudo-crate）だけでなく Cargo パッケージ名
# （pseudo-crate-pkg、Cargo.toml の [package] name）でも免除できることを確認する。
REPO9="$(setup_repo)"
BASE9="$(cd "${REPO9}" && git rev-parse HEAD)"
cat >> "${REPO9}/crates/pseudo-crate/src/lib.rs" <<'EOF'

pub fn noop2() {}
EOF
commit_all "${REPO9}" "テストなし src 変更（パッケージ名で明示除外予定）"
set +e
out9="$(run_check_in "${REPO9}" --base "${BASE9}" --allow-no-tests pseudo-crate-pkg "自明な no-op 追加のため" 2>&1)"
exit9=$?
set -e
assert_exit_code "Cargo パッケージ名指定の --allow-no-tests は exit 0" 0 "${exit9}"
assert_contains "除外理由を警告として出力する（パッケージ名指定）" "${out9}" "自明な no-op 追加のため"
rm -rf "${REPO9}"

echo "===== ケース10: テストマーカーから遠く離れた無関係な src 編集は fail（コンテキスト窓の悪用縮小） ====="
# Bugbot 指摘（scripts/feature-flow-check.sh#L159-164）の回帰テスト:
# ファイル内にテストマーカーが存在していても、実際の変更箇所がそこから
# 十分離れていれば（-U16 の窓外）、テスト追加なしとして正しく検知できることを
# 確認する（旧実装は -U1000000 でファイル全体を文脈に含めるため誤って pass した。
# -U16 は穴を縮小する近似ヒューリスティックであり、窓の境界付近は
# --allow-no-tests + レビューでの運用が前提）。
REPO10="$(setup_repo)"
(
    cd "${REPO10}"
    {
        echo
        echo "#[cfg(test)]"
        echo "mod tests {"
        echo "    use super::*;"
        echo
        echo "    #[test]"
        echo "    fn add_works_inline() {"
        echo "        assert_eq!(add(1, 1), 2);"
        echo "    }"
        echo "}"
        for i in $(seq 1 40); do
            echo
            echo "pub fn unrelated_padding_${i}() {}"
        done
        echo
        echo "pub fn sub(a: i32, b: i32) -> i32 {"
        echo "    a - b"
        echo "}"
    } >> crates/pseudo-crate/src/lib.rs
)
commit_all "${REPO10}" "テストブロックから離れた場所に無関係な関数を追加"
BASE10="$(cd "${REPO10}" && git rev-parse HEAD)"
sed -i.bak 's/    a - b/    a - b \/\/ 無関係な変更（テスト追加なし）/' \
    "${REPO10}/crates/pseudo-crate/src/lib.rs"
rm -f "${REPO10}/crates/pseudo-crate/src/lib.rs.bak"
commit_all "${REPO10}" "テストマーカーから遠い無関係な変更（テスト追加なし）"
set +e
out10="$(run_check_in "${REPO10}" --base "${BASE10}" 2>&1)"
exit10=$?
set -e
assert_exit_code "テストマーカーから遠い無関係な変更は exit 1" 1 "${exit10}"
assert_contains "エラーにクレート名 pseudo-crate を含む（ケース10）" "${out10}" "pseudo-crate"
rm -rf "${REPO10}"

echo "===== ケース11: ネストした src パス（crates/<name>/src/**/*.rs）の変更もテスト追加を要求する ====="
# Bugbot 指摘（scripts/feature-flow-check.sh#L148-L176）の回帰テスト:
# bash の case パターンマッチは `*` が `/` にもマッチする（パス名展開のグロブとは
# 別物）ため、`crates/*/src/*.rs` は `crates/<name>/src/<sub>/<file>.rs` のような
# ネストした src パスにも一致する。テストマーカーを伴わないネストした src 変更は
# 通常の src 変更と同様に exit 1 になることを確認する（誤検知の否定）。
REPO11="$(setup_repo)"
BASE11="$(cd "${REPO11}" && git rev-parse HEAD)"
(
    cd "${REPO11}"
    mkdir -p crates/pseudo-crate/src/nested
    cat > crates/pseudo-crate/src/nested/mod.rs <<'EOF'
pub fn nested_noop() {}
EOF
)
commit_all "${REPO11}" "ネストした src パスへの変更（テスト追加なし）"
set +e
out11="$(run_check_in "${REPO11}" --base "${BASE11}" 2>&1)"
exit11=$?
set -e
assert_exit_code "ネストした src 変更のみは exit 1" 1 "${exit11}"
assert_contains "エラーにクレート名 pseudo-crate を含む（ケース11）" "${out11}" "pseudo-crate"
rm -rf "${REPO11}"

echo "===== ケース12: ネストした tests パス（crates/<name>/tests/**/*）の変更も検知する ====="
REPO12="$(setup_repo)"
(
    cd "${REPO12}"
    mkdir -p crates/pseudo-crate/src/nested crates/pseudo-crate/tests/common
    cat > crates/pseudo-crate/src/nested/mod.rs <<'EOF'
pub fn nested_noop2() {}
EOF
    cat > crates/pseudo-crate/tests/common/helper.rs <<'EOF'
pub fn helper() -> i32 { 1 }
EOF
)
commit_all "${REPO12}" "ネストした src + ネストした tests 変更"
BASE12="$(cd "${REPO12}" && git rev-parse HEAD~1)"
set +e
out12="$(run_check_in "${REPO12}" --base "${BASE12}" 2>&1)"
exit12=$?
set -e
assert_exit_code "ネストした src + ネストした tests 変更は exit 0" 0 "${exit12}"
rm -rf "${REPO12}"

echo
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -ne 0 ]; then
    exit 1
fi
exit 0
