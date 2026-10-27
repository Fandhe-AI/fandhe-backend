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

mkdir -p "${TMP}/src-mixed/refresh"
cat > "${TMP}/src-mixed/gate.rs" <<'RS'
pub fn check() -> u32 { 1 }
RS
cat > "${TMP}/src-mixed/refresh/task.rs" <<'RS'
pub async fn run() { sleep().await; }
RS
assert_eq "除外なしなら refresh/ の async fn・.await を検出する（1 行）" "1" "$(detect_forbidden_io_in_sources "${TMP}/src-mixed" | wc -l | tr -d ' ')"
assert_eq "refresh/ を除外すれば違反なし" "" "$(detect_forbidden_io_in_sources "${TMP}/src-mixed" "${TMP}/src-mixed/refresh/")"
printf 'pub async fn leak() {}\n' > "${TMP}/src-mixed/auth.rs"
assert_eq "除外しても refresh/ 以外の違反は検出する（陰性対照）" "1" "$(detect_forbidden_io_in_sources "${TMP}/src-mixed" "${TMP}/src-mixed/refresh/" | wc -l | tr -d ' ')"

echo "===== optional_dep_gated_by_feature ====="
cat > "${TMP}/gated-ok.toml" <<'T'
[dependencies]
# tokio = { version = "1" }
base64 = { workspace = true }
tokio = { version = "1", default-features = false, features = ["time"], optional = true }
[features]
default = []
jwks-refresh = ["dep:tokio"]
T
cat > "${TMP}/gated-no-optional.toml" <<'T'
[dependencies]
tokio = { version = "1", default-features = false }
[features]
default = []
jwks-refresh = ["dep:tokio"]
T
cat > "${TMP}/gated-no-dep-ref.toml" <<'T'
[dependencies]
tokio = { version = "1", optional = true }
[features]
default = []
jwks-refresh = []
T
cat > "${TMP}/gated-in-default.toml" <<'T'
[dependencies]
tokio = { version = "1", optional = true }
[features]
default = ["jwks-refresh"]
jwks-refresh = ["dep:tokio"]
T
cat > "${TMP}/gated-dev-only.toml" <<'T'
[dependencies]
base64 = { workspace = true }
[dev-dependencies]
tokio = { version = "1", optional = true }
[features]
jwks-refresh = ["dep:tokio"]
T
if optional_dep_gated_by_feature "${TMP}/gated-ok.toml" tokio jwks-refresh; then pass "optional + dep: + default 外は成功"; else fail "正しい配線を検出できない"; fi
if optional_dep_gated_by_feature "${TMP}/gated-no-optional.toml" tokio jwks-refresh; then fail "optional でないのに成功"; else pass "optional でなければ失敗"; fi
if optional_dep_gated_by_feature "${TMP}/gated-no-dep-ref.toml" tokio jwks-refresh; then fail "dep: 参照なしで成功"; else pass "feature が dep: を含まなければ失敗"; fi
if optional_dep_gated_by_feature "${TMP}/gated-in-default.toml" tokio jwks-refresh; then fail "default に入っていて成功"; else pass "default に入っていれば失敗"; fi
if optional_dep_gated_by_feature "${TMP}/gated-dev-only.toml" tokio jwks-refresh; then fail "dev-dependencies のみで成功"; else pass "[dependencies] に無ければ失敗（dev 節は無視）"; fi

echo "===== module_is_cfg_gated ====="
cat > "${TMP}/lib-ok.rs" <<'RS'
//! doc
#[cfg(feature = "jwks-refresh")]
pub mod refresh;
RS
cat > "${TMP}/lib-nocfg.rs" <<'RS'
pub mod refresh;
RS
cat > "${TMP}/lib-otherfeat.rs" <<'RS'
#[cfg(feature = "other")]
pub mod refresh;
RS
cat > "${TMP}/lib-comment.rs" <<'RS'
#[cfg(feature = "jwks-refresh")]
// pub mod refresh;
pub mod other;
RS
if module_is_cfg_gated "${TMP}/lib-ok.rs" refresh jwks-refresh; then pass "cfg 直後の mod を検出"; else fail "cfg 付き mod を検出できない"; fi
if module_is_cfg_gated "${TMP}/lib-nocfg.rs" refresh jwks-refresh; then fail "cfg なしで成功"; else pass "cfg なしなら失敗"; fi
if module_is_cfg_gated "${TMP}/lib-otherfeat.rs" refresh jwks-refresh; then fail "別 feature の cfg で成功"; else pass "別 feature の cfg なら失敗"; fi
if module_is_cfg_gated "${TMP}/lib-comment.rs" refresh jwks-refresh; then fail "コメント中の mod で成功"; else pass "コメント中の mod 宣言は無視"; fi
REAL_LIB="${SCRIPTS_DIR}/../crates/plugin-jwt/src/lib.rs"
if module_is_cfg_gated "${REAL_LIB}" refresh jwks-refresh; then pass "実 lib.rs: refresh は jwks-refresh の cfg 内"; else fail "実 lib.rs の refresh が cfg で閉じていない"; fi
sed 's|^#\[cfg(feature = "jwks-refresh")\]$||' "${REAL_LIB}" > "${TMP}/real-lib.rs"
if module_is_cfg_gated "${TMP}/real-lib.rs" refresh jwks-refresh; then fail "陰性対照: 実 lib.rs から cfg を除いても成功"; else pass "陰性対照: 実 lib.rs の cfg を除くと失敗"; fi
REAL_JWT_TOML="${SCRIPTS_DIR}/../crates/plugin-jwt/Cargo.toml"
if optional_dep_gated_by_feature "${REAL_JWT_TOML}" tokio jwks-refresh; then pass "実 plugin-jwt Cargo.toml: tokio は jwks-refresh 限定の optional"; else fail "実 Cargo.toml の tokio 配線が設計 8.2 と異なる"; fi
sed 's/, optional = true//' "${REAL_JWT_TOML}" > "${TMP}/real-jwt.toml"
if optional_dep_gated_by_feature "${TMP}/real-jwt.toml" tokio jwks-refresh; then fail "陰性対照: optional を除いても成功"; else pass "陰性対照: 実 Cargo.toml の optional を除くと失敗"; fi

echo "===== tokio_feature_lines ====="
printf 'tokio feature "time"\ntokio feature "time" (*)\ntokio feature "sync"\nfandhe-backend-plugin-jwt feature "jwks-refresh"\n' > "${TMP}/feat.txt"
assert_eq "tokio の feature 行だけを重複なく並べる" 'tokio feature "sync" tokio feature "time"' "$(tokio_feature_lines "${TMP}/feat.txt" | tr '\n' ' ' | sed 's/ $//')"

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


# dpop（#907）: 実 lib.rs で dpop が cfg で閉じていること、cfg を外すと失敗すること。
if module_is_cfg_gated "${REAL_LIB}" dpop dpop; then pass "実 lib.rs: dpop は dpop の cfg 内"; else fail "実 lib.rs の dpop が cfg で閉じていない"; fi
sed 's|#\[cfg(feature = "dpop")\]||' "${REAL_LIB}" > "${TMP}/real-lib-nodpop.rs"
if module_is_cfg_gated "${TMP}/real-lib-nodpop.rs" dpop dpop; then fail "陰性対照: 実 lib.rs から dpop の cfg を除いても成功"; else pass "陰性対照: 実 lib.rs の dpop の cfg を除くと失敗"; fi

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

# dpop の fuzz target・期待テスト名が実ファイルに存在し、欠けると検出されること（#907）。
for t in dpop_jwk_htu dpop_proof_claims dpop_verify_proof; do
    if list_fuzz_targets "${REPO_ROOT}/crates/plugin-jwt/fuzz/fuzz_targets" | grep -qx "${t}"; then pass "実 fuzz target: ${t} あり"; else fail "実 fuzz target ${t} が無い"; fi
done
list_fuzz_targets "${REPO_ROOT}/crates/plugin-jwt/fuzz/fuzz_targets" | grep -vx dpop_verify_proof > "${TMP}/targets-missing.txt"
if grep -qx dpop_verify_proof "${TMP}/targets-missing.txt"; then fail "陰性対照: target を除いても検出される"; else pass "陰性対照: dpop の target が欠けると検出できる"; fi
for t in rejects_bad_jwk missing_duplicate_or_mismatched_proof_is_rejected; do
    if grep -rqE "fn ${t}\(" "${REPO_ROOT}/crates/plugin-jwt/tests" "${REPO_ROOT}/crates/plugin-hub-wiring/tests"; then pass "期待テスト ${t} がソースに存在"; else fail "期待テスト ${t} がソースに無い"; fi
done
printf 'test rejects_bad_jwk ... ok\n' > "${TMP}/dpop-out.txt"
assert_eq "dpop: 期待テストが欠けると不足として報告" "rejects_bad_signature" "$(check_test_names_ok "${TMP}/dpop-out.txt" rejects_bad_jwk rejects_bad_signature)"

echo ""
echo "===== request_gate_check_is_sync ====="
SIG='fn check(\n        &self,\n        head: &RequestHead,\n        ctx: &GateContext,\n    ) -> GateOutcome;'
printf 'pub trait RequestGate {\n    %b\n}\n' "${SIG}" > "${TMP}/ext-sync.rs"
printf 'pub trait RequestGate {\n    async %b\n}\n' "${SIG}" > "${TMP}/ext-async.rs"
if request_gate_check_is_sync "${TMP}/ext-sync.rs"; then pass "同期 check は成立"; else fail "同期 check が不成立"; fi
if request_gate_check_is_sync "${TMP}/ext-async.rs"; then fail "陰性対照: async fn check が通る"; else pass "陰性対照: async fn check は不成立"; fi

echo ""
echo "合計: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT}"
[ "${FAIL_COUNT}" -eq 0 ]
