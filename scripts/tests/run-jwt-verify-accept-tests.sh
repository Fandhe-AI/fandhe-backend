#!/usr/bin/env bash
# scripts/accept/lib/jwt-verify-checks.sh の判定ロジックのオフライン・セルフテスト
# （REQ-16、イシュー #823）。cargo・ネットワーク非依存。陽性・陰性の両方を検証する。
#
# 呼び出し元: 人間 / CI が `bash scripts/tests/run-jwt-verify-accept-tests.sh` で実行する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=../accept/lib/jwt-verify-checks.sh
source "${SCRIPTS_DIR}/accept/lib/jwt-verify-checks.sh"

PASS_COUNT=0
FAIL_COUNT=0
pass() { echo "PASS: $1"; PASS_COUNT=$((PASS_COUNT + 1)); }
fail() { echo "FAIL: $1" >&2; FAIL_COUNT=$((FAIL_COUNT + 1)); }
assert_eq() {
    if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待: '$2'、実際: '$3'）"; fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

echo "===== detect_forbidden_io_in_sources ====="
mkdir -p "${TMP}/src-clean" "${TMP}/src-bad"
cat > "${TMP}/src-clean/a.rs" <<'RS'
//! 同期のみ。`.await` なし、tokio:: 不使用。
/// async fn ではない。
pub fn check() -> u32 { 1 }
RS
cat > "${TMP}/src-bad/a.rs" <<'RS'
pub async fn check() -> u32 { 1 }
RS
cat > "${TMP}/src-bad/b.rs" <<'RS'
pub fn f() { let _ = std::fs::read("x"); }
RS
assert_eq "コメント中の .await/tokio:: は検出しない（偽陽性回帰防止）" "" "$(detect_forbidden_io_in_sources "${TMP}/src-clean")"
assert_eq "コード行の async fn と std::fs を検出する（2 件）" "2" "$(detect_forbidden_io_in_sources "${TMP}/src-bad" | wc -l | tr -d ' ')"

echo "===== check_dependency_allowlist ====="
cat > "${TMP}/ok.toml" <<'T'
[dependencies]
base64 = { workspace = true }
ring = { workspace = true }
[dev-dependencies]
tokio = "1"
T
cat > "${TMP}/bad.toml" <<'T'
[dependencies]
ring = { workspace = true }
jsonwebtoken = "9"
tokio = "1"
T
assert_eq "許可集合内 + dev-dependencies の tokio は違反なし" "" "$(check_dependency_allowlist "${TMP}/ok.toml")"
assert_eq "許可外の依存を検出する" "jsonwebtoken tokio" "$(check_dependency_allowlist "${TMP}/bad.toml" | tr '\n' ' ' | sed 's/ $//')"


echo "===== check_test_names_ok ====="
printf 'test t_a ... ok\ntest t_b ... FAILED\n' > "${TMP}/out.txt"
assert_eq "全件 ok なら空" "" "$(check_test_names_ok "${TMP}/out.txt" t_a)"
assert_eq "FAILED は不足として報告" "t_b" "$(check_test_names_ok "${TMP}/out.txt" t_a t_b)"
assert_eq "欠落は不足として報告" "t_c" "$(check_test_names_ok "${TMP}/out.txt" t_a t_c)"

echo "===== count_pkg_in_tree ====="
printf 'core v0.1.0\nring v0.17.0\n' > "${TMP}/tree.txt"
assert_eq "0 件" "0" "$(count_pkg_in_tree "${TMP}/tree.txt" fandhe-backend-plugin-jwt)"
assert_eq "1 件" "1" "$(count_pkg_in_tree "${TMP}/tree.txt" ring)"

echo "===== list_fuzz_targets ====="
mkdir -p "${TMP}/ft/common"
touch "${TMP}/ft/x.rs" "${TMP}/ft/y.rs"
assert_eq "target 名を列挙（サブディレクトリは除外）" "x y" "$(list_fuzz_targets "${TMP}/ft" | tr '\n' ' ' | sed 's/ $//')"

echo ""
echo "合計: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT}"
[ "${FAIL_COUNT}" -eq 0 ]
