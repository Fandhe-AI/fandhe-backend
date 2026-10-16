#!/usr/bin/env bash
# REQ-16（汎用 JWT 検証プラグイン `fandhe-backend-plugin-jwt`）受け入れ検証オーケストレータ
# （イシュー #823）。`docs/spec/04-requirements.md` REQ-16 の受け入れ基準 1〜6 を機械検証する:
#   A (基準1): 各種拒否（iss/aud/typ/nbf/exp/kid/alg/鍵種別/none/HS）のテストが名前単位で ok
#   B (基準2): RS256・ES256 の受理と鍵の回転のテストが名前単位で ok
#   C (基準3): 同期・I/O なし（RequestGate 契約、NFR-7）。静的確認 + gate テスト
#   D (基準4): 新規外部依存なし（暗号は ring のみ）・未使用構成で依存ツリーに 0 件
#   E (基準5): REQ-9 hub 配線が本プラグインの検証部を使う形へ移行済み
#   F (基準6): JWT 解析部が fuzz 対象（NFR-11）。実行は JWT_ACCEPT_RUN_FUZZ=1 の opt-in
#
# 判定不能（前提ツール・履歴の欠如）は SKIP とし PASS と偽らない（.claude/rules/security.md）。
# ツールの自動ダウンロード・自動インストールはしない。ネットワークを使うのは
# cargo audit / cargo deny の DB 更新のみ。
#
# 呼び出し元: 人間が `bash scripts/accept/jwt-verify-accept.sh` で直接実行する。
# 判定ロジックのセルフテストは `scripts/tests/run-jwt-verify-accept-tests.sh`。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
# shellcheck source=lib/jwt-verify-checks.sh
source "${SCRIPT_DIR}/lib/jwt-verify-checks.sh"
cd "${WORKSPACE_ROOT}"

PKG="fandhe-backend-plugin-jwt"
CRATE_DIR="crates/plugin-jwt"
# REQ-16 着手前（#838 の直前）。既存依存の比較基準。
LOCK_BASE_REV="e9ceffd"

C1_TESTS=(baseline_is_accepted c1_rejects_iss_mismatch c1_rejects_aud_mismatch
    c1_rejects_typ_mismatch_and_missing c1_rejects_before_nbf c1_rejects_after_exp
    c1_rejects_unknown_kid c1_rejects_alg_outside_allowlist c1_rejects_key_kind_alg_mismatch
    c1_rejects_alg_none c1_rejects_hs_family c1_rejects_der_encoded_ecdsa_signature)
C2_TESTS=(c2_accepts_rs256_with_fixture_key c2_accepts_es256_with_fixture_key
    c2_accepts_mixed_rs_es_keyset c2_rotation_same_kid_rs256 c2_rotation_same_kid_es256
    c2_rotation_distinct_kids c2_rotation_lifecycle_via_shared_key_set)

echo "=== REQ-16 受け入れ検証（汎用 JWT 検証プラグイン、#823） ==="
echo "workspace root: ${WORKSPACE_ROOT}"
echo ""

TMPDIR_ACCEPT="$(mktemp -d)"
trap 'rm -rf "${TMPDIR_ACCEPT}"' EXIT

# 名前単位の照合つきで cargo test を実行する共通処理。
# 引数: $1 基準名、$2 出力ファイル名、$3.. cargo test 引数の後ろに続けず、
#       テスト名は配列 REQUIRED_NAMES で渡す。
run_named_tests() {
    local label="${1}" outfile="${TMPDIR_ACCEPT}/${2}" status missing
    shift 2
    if ! check_tool cargo "Rust ツールチェーン一式（rustup 等）"; then
        record_skip "${label}" "cargo 未導入"
        return
    fi
    set +e
    cargo test -p "${PKG}" --test req16_acceptance >"${outfile}" 2>&1
    status=$?
    set -e
    if [ "${status}" -ne 0 ]; then
        record_fail "${label}" "cargo test が非 0 終了: $(tail -10 "${outfile}" | tr '\n' ' ')"
        return
    fi
    missing="$(check_test_names_ok "${outfile}" "$@" | tr '\n' ' ')"
    if [ -n "${missing}" ]; then
        record_fail "${label}" "期待テストが ok で現れない: ${missing}"
    else
        record_pass "${label}" "指定 $# 件のテストが名前単位で ok（cargo test -p ${PKG} --test req16_acceptance）"
    fi
}

check_a() { run_named_tests "A: 基準1 各種拒否" a.out "${C1_TESTS[@]}"; }
check_b() { run_named_tests "B: 基準2 RS256/ES256 受理・鍵の回転" b.out "${C2_TESTS[@]}"; }

check_c() {
    local label="C: 基準3 同期・I/O なし"
    if [ ! -d "${CRATE_DIR}/src" ]; then
        record_skip "${label}" "${CRATE_DIR} が存在しない"
        return
    fi
    local fail_msgs=""
    # (a) RequestGate::check が同期 fn
    if ! grep -qE '^[[:space:]]*fn check\(&self, head: &RequestHead, ctx: &GateContext\)' crates/core/src/extension.rs; then
        fail_msgs+="RequestGate::check が同期 fn のシグネチャで見つからない; "
    fi
    # (b) コメントを除いたソースに I/O・非同期の痕跡なし
    local hits
    hits="$(detect_forbidden_io_in_sources "${CRATE_DIR}/src")"
    if [ -n "${hits}" ]; then
        fail_msgs+="禁止パターン検出: $(printf '%s' "${hits}" | head -3 | tr '\n' ' '); "
    fi
    # (c) tokio は [dependencies] にない（dev-dependencies のみ）
    if check_dependency_allowlist "${CRATE_DIR}/Cargo.toml" | grep -qx tokio; then
        fail_msgs+="tokio が [dependencies] にある; "
    fi
    # (d) gate 統合テスト
    local gate_out="${TMPDIR_ACCEPT}/gate.out" gate_status gate_count=""
    if check_tool cargo "Rust ツールチェーン一式（rustup 等）"; then
        set +e
        cargo test -p "${PKG}" --test gate >"${gate_out}" 2>&1
        gate_status=$?
        set -e
        if [ "${gate_status}" -ne 0 ]; then
            fail_msgs+="gate テストが非 0 終了; "
        else
            gate_count="$(grep -oE '^test result: ok\. [0-9]+ passed' "${gate_out}" | grep -oE '[0-9]+' | head -1 || true)"
        fi
    else
        fail_msgs+="cargo 未導入のため gate テスト判定不能; "
    fi
    if [ -n "${fail_msgs}" ]; then
        record_fail "${label}" "${fail_msgs}"
    else
        record_pass "${label}" "check は同期 fn・src に I/O/async 痕跡 0 件（コメント除外）・tokio は dev のみ・gate テスト ${gate_count:-?} 件 PASS"
    fi
}

tree_count() {
    # 引数: cargo tree の追加フラグ。core の normal 依存ツリーでの plugin-jwt 行数を返す。
    local out="${TMPDIR_ACCEPT}/tree.$$.out"
    # fail-closed: cargo tree の失敗・空出力・core 自身の行の欠落は「0 件」ではなく判定不能（ERR）とする
    if ! cargo tree -p fandhe-backend-core -e normal --prefix none "$@" >"${out}" 2>/dev/null \
        || ! grep -q '^fandhe-backend-core ' "${out}"; then
        echo ERR
        return
    fi
    count_pkg_in_tree "${out}" "${PKG}"
}

check_d() {
    local label="D: 基準4 新規外部依存なし・未使用時 0 件"
    if [ ! -f "${CRATE_DIR}/Cargo.toml" ]; then
        record_skip "${label}" "${CRATE_DIR}/Cargo.toml が存在しない"
        return
    fi
    local fail_msgs="" notes=""
    # (a) 許可集合
    local extra
    extra="$(check_dependency_allowlist "${CRATE_DIR}/Cargo.toml" | tr '\n' ' ')"
    [ -z "${extra}" ] || fail_msgs+="許可外の依存: ${extra}; "
    # (b) 外部依存 4 種が REQ-16 着手前から hub-wiring に既存の依存であること。
    #     Cargo.lock は本リポジトリで追跡されない（git ls-files に無い）ため lock 差分では
    #     比較できず、着手前コミットの hub-wiring/Cargo.toml に宣言済みかで「新規外部依存なし」を確認する。
    local ext prior_missing="" b_skipped=0
    if ! git rev-parse --verify --quiet "${LOCK_BASE_REV}^{commit}" >/dev/null 2>&1; then
        # 履歴欠如（shallow clone 等）は「依存が無い」ことを意味しないため判定せず SKIP を記録する
        record_skip "D(b): 既存依存の確認" "${LOCK_BASE_REV} を解決できない（履歴欠如、shallow clone の可能性）"
        b_skipped=1
    else
        for ext in ring base64 serde serde_json; do
            if ! git grep -qE "^${ext} *=" "${LOCK_BASE_REV}" -- crates/plugin-hub-wiring/Cargo.toml 2>/dev/null; then
                prior_missing+="${ext} "
            fi
        done
        if [ -z "${prior_missing}" ]; then
            notes+="外部依存 ring/base64/serde/serde_json は着手前 ${LOCK_BASE_REV} の hub-wiring に既存; "
        else
            fail_msgs+="着手前 ${LOCK_BASE_REV} の hub-wiring に無い外部依存: ${prior_missing}; "
        fi
    fi
    # (c) 未使用構成の依存ツリー
    if check_tool cargo "Rust ツールチェーン一式（rustup 等）"; then
        local n_none n_def n_all
        n_none="$(tree_count --no-default-features)"
        n_def="$(tree_count)"
        n_all="$(tree_count --all-features)"
        if [ "${n_none}" = "0" ] && [ "${n_def}" = "0" ] && [ "${n_all}" = "0" ]; then
            notes+="core の cargo tree に ${PKG} が 0 件（no-default/既定/all-features）; "
        else
            fail_msgs+="core の依存ツリーに ${PKG} が現れる（${n_none}/${n_def}/${n_all}）; "
        fi
    else
        fail_msgs+="cargo 未導入のため依存ツリー判定不能; "
    fi
    # (d) unsafe
    if grep -q '^#!\[forbid(unsafe_code)\]' "${CRATE_DIR}/src/lib.rs"; then
        notes+="lib.rs に forbid(unsafe_code); "
    else
        fail_msgs+="lib.rs に #![forbid(unsafe_code)] がない; "
    fi
    local baseline
    baseline="$(jq -r '."plugin-jwt".unsafe_count // "null"' scripts/unsafe-baseline.json 2>/dev/null || echo null)"
    if [ "${baseline}" = "0" ]; then
        notes+="unsafe-baseline plugin-jwt unsafe_count=0; "
    else
        fail_msgs+="unsafe-baseline の plugin-jwt.unsafe_count が 0 でない/欠落（${baseline}）; "
    fi
    if [ -n "${fail_msgs}" ]; then
        record_fail "${label}" "${fail_msgs}"
    elif [ "${b_skipped}" = "1" ]; then
        # 核となる (b) が未実施のまま PASS にしない
        record_skip "${label}" "(b) 未確認のため判定不能（他項目は充足: ${notes}）"
    else
        record_pass "${label}" "${notes}"
    fi
    # (e) audit / deny（記録のみ。FAIL 判定は結果に従う）
    if check_tool cargo-audit "cargo install cargo-audit --locked"; then
        if cargo audit >"${TMPDIR_ACCEPT}/audit.out" 2>&1; then
            record_pass "D(e): cargo audit" "既知脆弱性 0 件"
        else
            record_fail "D(e): cargo audit" "$(tail -5 "${TMPDIR_ACCEPT}/audit.out" | tr '\n' ' ')"
        fi
    else
        record_skip "D(e): cargo audit" "cargo-audit 未導入"
    fi
    if check_tool cargo-deny "cargo install cargo-deny --locked"; then
        if cargo deny check >"${TMPDIR_ACCEPT}/deny.out" 2>&1; then
            record_pass "D(e): cargo deny check" "違反 0 件"
        else
            record_fail "D(e): cargo deny check" "$(tail -5 "${TMPDIR_ACCEPT}/deny.out" | tr '\n' ' ')"
        fi
    else
        record_skip "D(e): cargo deny check" "cargo-deny 未導入"
    fi
}

check_e() {
    local label="E: 基準5 REQ-9 hub 配線の移行"
    local hub="crates/plugin-hub-wiring"
    if [ ! -d "${hub}" ]; then
        record_skip "${label}" "${hub} が存在しない"
        return
    fi
    local fail_msgs=""
    toml_has_normal_dependency "${hub}/Cargo.toml" "${PKG}" || fail_msgs+="hub-wiring の [dependencies] に ${PKG} が無い; "
    [ ! -e "${hub}/src/jwt.rs" ] && [ ! -e "${hub}/src/jwks.rs" ] || fail_msgs+="旧 jwt.rs/jwks.rs が残存; "
    if [ -x scripts/dep-direction-check.sh ] || [ -f scripts/dep-direction-check.sh ]; then
        bash scripts/dep-direction-check.sh >"${TMPDIR_ACCEPT}/dir.out" 2>&1 || fail_msgs+="dep-direction-check が非 0 終了; "
    else
        fail_msgs+="scripts/dep-direction-check.sh が無い; "
    fi
    if [ -n "${fail_msgs}" ]; then
        record_fail "${label}" "${fail_msgs}"
        return
    fi
    record_pass "${label}" "hub-wiring は ${PKG} に依存・旧 jwt.rs/jwks.rs なし・依存方向検査 PASS"
    # 既存受け入れの再実行（hub-wiring-accept.sh。D は oha 未導入なら SKIP）
    if bash scripts/accept/hub-wiring-accept.sh >"${TMPDIR_ACCEPT}/hub.out" 2>&1; then
        record_pass "E: hub-wiring-accept.sh 再実行" "終了コード 0（FAIL なし。SKIP は REQ-9 レポート参照）"
    else
        record_fail "E: hub-wiring-accept.sh 再実行" "$(tail -8 "${TMPDIR_ACCEPT}/hub.out" | tr '\n' ' ')"
    fi
}

check_f() {
    local label="F: 基準6 fuzz 対象（NFR-11）"
    local ft="${CRATE_DIR}/fuzz/fuzz_targets" fail_msgs="" targets
    if [ ! -d "${ft}" ]; then
        record_skip "${label}" "${ft} が存在しない"
        return
    fi
    targets="$(list_fuzz_targets "${ft}" | tr '\n' ' ')"
    local t
    for t in jwks_from_json verify_claims verify_token; do
        case " ${targets}" in *" ${t} "*) ;; *) fail_msgs+="target ${t} が無い; " ;; esac
    done
    # コメント・description・step 名の文言にヒットしないよう、代入行／ジョブ内の run 行に限定して判定する
    fuzz_dirs_registers scripts/fuzz.sh "crates/plugin-jwt/fuzz" || fail_msgs+="scripts/fuzz.sh の FUZZ_DIRS 代入行に plugin-jwt/fuzz が無い; "
    workflow_job_runs_script .github/workflows/ci.yml fuzz-smoke scripts/fuzz.sh \
        || fail_msgs+="ci.yml の fuzz-smoke ジョブが run で scripts/fuzz.sh を実行していない; "
    if [ -n "${fail_msgs}" ]; then
        record_fail "${label}" "${fail_msgs}"
        return
    fi
    record_pass "${label}" "target: ${targets}/ FUZZ_DIRS 登録・ci.yml fuzz-smoke 経由（静的確認）"
    if [ "${JWT_ACCEPT_RUN_FUZZ:-0}" = "1" ]; then
        if bash scripts/fuzz.sh --max-total-time 60 >"${TMPDIR_ACCEPT}/fuzz.out" 2>&1; then
            record_pass "F: fuzz 実行（60 秒/target）" "scripts/fuzz.sh 終了コード 0"
        else
            record_fail "F: fuzz 実行（60 秒/target）" "$(tail -8 "${TMPDIR_ACCEPT}/fuzz.out" | tr '\n' ' ')"
        fi
    else
        record_skip "F: fuzz 実行" "opt-in（JWT_ACCEPT_RUN_FUZZ=1）。CI の fuzz-smoke の結果を引用する"
    fi
}

check_a
check_b
check_c
check_d
check_e
check_f

record_warn "NFR-6（HTTP 計測）" "benches/jwt-nfr6-bench.sh は oha が必要。本スクリプトでは未計測（判定を丸めない）"

print_summary "REQ-16、#823"
exit "$(summary_exit_code)"
