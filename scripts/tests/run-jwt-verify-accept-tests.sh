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

echo "===== toml_has_normal_dependency ====="
cat > "${TMP}/hub-ok.toml" <<'T'
[package]
description = "mentions fandhe-backend-plugin-jwt"
# fandhe-backend-plugin-jwt in comment
[dependencies]
fandhe-backend-plugin-jwt = { workspace = true }
[dev-dependencies]
T
cat > "${TMP}/hub-removed.toml" <<'T'
[package]
description = "mentions fandhe-backend-plugin-jwt"
[dependencies]
# fandhe-backend-plugin-jwt は一本化した
serde = { workspace = true }
[dev-dependencies]
fandhe-backend-plugin-jwt = { workspace = true }
T
if toml_has_normal_dependency "${TMP}/hub-ok.toml" fandhe-backend-plugin-jwt; then pass "[dependencies] の宣言を検出"; else fail "[dependencies] の宣言を検出できない"; fi
if toml_has_normal_dependency "${TMP}/hub-removed.toml" fandhe-backend-plugin-jwt; then fail "description・コメント・dev 節のヒットで偽陽性"; else pass "宣言行を除去すると FAIL（description・コメント・dev 節は無視）"; fi

echo "===== fuzz_dirs_registers ====="
cat > "${TMP}/fuzz-ok.sh" <<'T'
#   - `crates/plugin-jwt/fuzz`: コメントでの言及
FUZZ_DIRS=("crates/http/fuzz" "crates/plugin-jwt/fuzz")
T
cat > "${TMP}/fuzz-removed.sh" <<'T'
#   - `crates/plugin-jwt/fuzz`: コメントでの言及
FUZZ_DIRS=("crates/http/fuzz")
# FUZZ_DIRS=("crates/plugin-jwt/fuzz")
T
if fuzz_dirs_registers "${TMP}/fuzz-ok.sh" crates/plugin-jwt/fuzz; then pass "FUZZ_DIRS 登録を検出"; else fail "FUZZ_DIRS 登録を検出できない"; fi
if fuzz_dirs_registers "${TMP}/fuzz-removed.sh" crates/plugin-jwt/fuzz; then fail "コメントのヒットで偽陽性"; else pass "登録を除去すると FAIL（コメント行は無視）"; fi

echo "===== workflow_job_runs_script ====="
cat > "${TMP}/ci-ok.yml" <<'T'
jobs:
  other:
    steps:
      - run: echo other
  fuzz-smoke:
    steps:
      # scripts/fuzz.sh の説明
      - name: scripts/fuzz.sh
        run: bash scripts/fuzz.sh --max-total-time 60
T
cat > "${TMP}/ci-removed.yml" <<'T'
jobs:
  fuzz-smoke:
    steps:
      # run: bash scripts/fuzz.sh は廃止
      - name: scripts/fuzz.sh（名前だけ残存）
        run: echo skipped
  other:
    steps:
      - run: bash scripts/fuzz.sh
T
if workflow_job_runs_script "${TMP}/ci-ok.yml" fuzz-smoke scripts/fuzz.sh; then pass "fuzz-smoke の run を検出"; else fail "fuzz-smoke の run を検出できない"; fi
if workflow_job_runs_script "${TMP}/ci-removed.yml" fuzz-smoke scripts/fuzz.sh; then fail "コメント・別ジョブ・step 名のヒットで偽陽性"; else pass "run 行を除去すると FAIL（コメント・step 名・別ジョブは無視）"; fi

echo "===== 実ファイルに対する陽性・陰性対照（E・F） ====="
REPO_ROOT="$(cd "${SCRIPTS_DIR}/.." && pwd)"
HUB_TOML="${REPO_ROOT}/crates/plugin-hub-wiring/Cargo.toml"
FUZZ_SH="${REPO_ROOT}/scripts/fuzz.sh"
CI_YML="${REPO_ROOT}/.github/workflows/ci.yml"
if toml_has_normal_dependency "${HUB_TOML}" fandhe-backend-plugin-jwt; then pass "実 hub-wiring Cargo.toml: 依存宣言あり"; else fail "実 hub-wiring Cargo.toml に依存宣言が無い"; fi
if fuzz_dirs_registers "${FUZZ_SH}" crates/plugin-jwt/fuzz; then pass "実 fuzz.sh: FUZZ_DIRS 登録あり"; else fail "実 fuzz.sh に FUZZ_DIRS 登録が無い"; fi
if workflow_job_runs_script "${CI_YML}" fuzz-smoke scripts/fuzz.sh; then pass "実 ci.yml: fuzz-smoke が fuzz.sh を実行"; else fail "実 ci.yml の fuzz-smoke が fuzz.sh を実行していない"; fi
# 本物の行だけを除いたコピー（説明・コメントの言及は残る）で FAIL することを確認する
{ grep -v '^fandhe-backend-plugin-jwt *=' "${HUB_TOML}" || true; } > "${TMP}/real-hub.toml"
{ grep -v 'run: bash scripts/fuzz.sh' "${CI_YML}" || true; } > "${TMP}/real-ci.yml"
sed 's|"crates/plugin-jwt/fuzz"||' "${FUZZ_SH}" > "${TMP}/real-fuzz.sh"
if toml_has_normal_dependency "${TMP}/real-hub.toml" fandhe-backend-plugin-jwt; then fail "陰性対照: 実 Cargo.toml から依存行を除いても検出される（偽陽性）"; else pass "陰性対照: 実 Cargo.toml の依存行を除くと FAIL"; fi
if fuzz_dirs_registers "${TMP}/real-fuzz.sh" crates/plugin-jwt/fuzz; then fail "陰性対照: 実 fuzz.sh から登録を除いても検出される（偽陽性）"; else pass "陰性対照: 実 fuzz.sh の登録を除くと FAIL"; fi
if workflow_job_runs_script "${TMP}/real-ci.yml" fuzz-smoke scripts/fuzz.sh; then fail "陰性対照: 実 ci.yml から run 行を除いても検出される（偽陽性）"; else pass "陰性対照: 実 ci.yml の run 行を除くと FAIL"; fi

echo ""
echo "合計: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT}"
[ "${FAIL_COUNT}" -eq 0 ]
