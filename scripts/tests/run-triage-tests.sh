#!/usr/bin/env bash
# トリアージロジックのセルフテスト（TASK-12.1-1、#79）:
# audit-triage.sh / unsafe-triage.sh の判定ロジックを、ネットワーク・cargo ビルドに
# 依存しないフィクスチャ・擬似クレートで検証する。ci.yml の unsafe-triage ジョブから
# 呼ばれる（ビルド不要で軽量に完結させるための設計）。
#
# 各テストは独立した assert 関数で実行し、失敗があれば非 0 で終了する
# （フェイルクローズ、.claude/rules/security.md）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures"

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

# ==================================================
# audit-triage.sh のテスト
# ==================================================

echo "===== audit-triage.sh: audit-clean.json ====="
set +e
out_clean="$(bash "${SCRIPTS_DIR}/audit-triage.sh" --input "${FIXTURES_DIR}/audit-clean.json" 2>&1)"
exit_clean=$?
set -e
assert_exit_code "audit-clean は exit 0" 0 "${exit_clean}"
assert_contains "audit-clean のレポートに『該当なし』が3回分含まれる" "${out_clean}" "該当なし"
# #226: 該当エントリがない区分には検証方法・リスク欄を出力しない設計を固定する
# （非空分岐限定の出力であることの negative assert）。
if printf '%s' "${out_clean}" | grep -qF "検証方法:"; then
    fail "audit-clean（該当なし）のレポートに検証方法欄が誤って出力されている"
else
    pass "audit-clean（該当なし）のレポートに検証方法欄が出力されない"
fi
if printf '%s' "${out_clean}" | grep -qF "リスク:"; then
    fail "audit-clean（該当なし）のレポートにリスク欄が誤って出力されている"
else
    pass "audit-clean（該当なし）のレポートにリスク欄が出力されない"
fi

echo "===== audit-triage.sh: audit-patched.json ====="
set +e
out_patched="$(bash "${SCRIPTS_DIR}/audit-triage.sh" --input "${FIXTURES_DIR}/audit-patched.json" 2>&1)"
exit_patched=$?
set -e
assert_exit_code "audit-patched は exit 1（vulnerability あり）" 1 "${exit_patched}"
assert_contains "audit-patched のレポートに自動更新提案の advisory ID を含む" "${out_patched}" "RUSTSEC-2099-0001"
assert_contains "audit-patched のレポートに cargo update 提案を含む" "${out_patched}" "cargo update -p example-crate"
# #226: 改善提案フローの必須 5 項目（背景・根拠データ／検証方法／リスク）を機械確認する。
assert_contains "audit-patched のレポートに背景・根拠データ欄を含む" "${out_patched}" "背景・根拠データ"
assert_contains "audit-patched のレポートに検証方法欄を含む" "${out_patched}" "検証方法:"
assert_contains "audit-patched のレポートに検証方法として dep-audit.sh 再実行を含む" "${out_patched}" "scripts/dep-audit.sh"
assert_contains "audit-patched のレポートにリスク欄を含む" "${out_patched}" "リスク:"
assert_contains "audit-patched のリスク欄に『対応しない場合』を含む" "${out_patched}" "対応しない場合:"

echo "===== audit-triage.sh: audit-unpatched-warning.json ====="
set +e
out_unpatched="$(bash "${SCRIPTS_DIR}/audit-triage.sh" --input "${FIXTURES_DIR}/audit-unpatched-warning.json" 2>&1)"
exit_unpatched=$?
set -e
assert_exit_code "audit-unpatched-warning は exit 1（vulnerability あり）" 1 "${exit_unpatched}"
assert_contains "要エスカレーションの advisory ID を含む" "${out_unpatched}" "RUSTSEC-2099-0002"
assert_contains "情報（記録・監視）の advisory ID を含む" "${out_unpatched}" "RUSTSEC-2099-0003"
assert_contains "エスカレーション推奨アクション文言を含む" "${out_unpatched}" "ユーザーへエスカレーション"
# #226: 要エスカレーション区分・情報区分の双方に検証方法・リスク欄が出力されることを確認する。
assert_contains "audit-unpatched-warning のレポートに検証方法欄を含む" "${out_unpatched}" "検証方法:"
assert_contains "audit-unpatched-warning のレポートにリスク欄を含む" "${out_unpatched}" "リスク:"
assert_contains "要エスカレーション区分のリスク文言（未対応のまま残置）を含む" "${out_unpatched}" "未対応のまま残置される"
assert_contains "情報区分の検証方法文言（継続監視）を含む" "${out_unpatched}" "継続監視"

echo "===== audit-triage.sh: --output オプション ====="
OUTPUT_TMP="$(mktemp)"
bash "${SCRIPTS_DIR}/audit-triage.sh" --input "${FIXTURES_DIR}/audit-clean.json" --output "${OUTPUT_TMP}" > /dev/null
if [ -s "${OUTPUT_TMP}" ]; then
    pass "--output で指定したファイルにレポートが書き出される"
else
    fail "--output で指定したファイルが空、または生成されていません"
fi
rm -f "${OUTPUT_TMP}"

echo "===== audit-triage.sh: --vuln-ids-output は warnings の advisory ID を含まない ====="
# 回帰テスト: CI の Issue 起票ステップが markdown レポート全体を正規表現で走査すると
# 「情報（記録・監視）」区分（warnings）の advisory ID まで vulnerability として拾って
# しまい、audit-triage.sh が exit 0（vulnerability なし）でも Issue が起票されうる
# 不整合があった。--vuln-ids-output は vulnerabilities.list[] のみに限定する。
VULN_IDS_TMP="$(mktemp)"
set +e
bash "${SCRIPTS_DIR}/audit-triage.sh" --input "${FIXTURES_DIR}/audit-unpatched-warning.json" --vuln-ids-output "${VULN_IDS_TMP}" > /dev/null 2>&1
set -e
vuln_ids_content="$(cat "${VULN_IDS_TMP}")"
assert_contains "--vuln-ids-output に要エスカレーションの advisory ID を含む" "${vuln_ids_content}" "RUSTSEC-2099-0002"
if printf '%s' "${vuln_ids_content}" | grep -qF "RUSTSEC-2099-0003"; then
    fail "--vuln-ids-output に warnings（情報・記録のみ区分）の advisory ID RUSTSEC-2099-0003 が混入している"
else
    pass "--vuln-ids-output に warnings の advisory ID が混入しない"
fi
rm -f "${VULN_IDS_TMP}"

VULN_IDS_CLEAN_TMP="$(mktemp)"
bash "${SCRIPTS_DIR}/audit-triage.sh" --input "${FIXTURES_DIR}/audit-clean.json" --vuln-ids-output "${VULN_IDS_CLEAN_TMP}" > /dev/null
if [ -s "${VULN_IDS_CLEAN_TMP}" ]; then
    fail "vulnerability なしの fixture で --vuln-ids-output が空でない"
else
    pass "vulnerability なしの fixture では --vuln-ids-output が空になる"
fi
rm -f "${VULN_IDS_CLEAN_TMP}"

# ==================================================
# unsafe-triage.sh のテスト
# 実 workspace（crates/）を汚さないよう、一時ディレクトリに擬似 crates/ を作って
# unsafe-triage.sh を REPO_ROOT ごと差し替えて実行する。
# ==================================================

run_unsafe_triage_in() {
    local workdir="$1"
    shift
    # FANDHE_BACKEND_UNSAFE_TRIAGE_REPO_ROOT で擬似 workspace を指す（unsafe-triage.sh 側の
    # テスト注入口）。実 workspace の crates/・scripts/unsafe-baseline.json は
    # 一切変更しない。
    FANDHE_BACKEND_UNSAFE_TRIAGE_REPO_ROOT="${workdir}" bash "${SCRIPTS_DIR}/unsafe-triage.sh" "$@"
}

setup_pseudo_workspace() {
    local base
    base="$(mktemp -d)"
    mkdir -p "${base}/crates/pseudo-crate/src"
    mkdir -p "${base}/scripts"
    echo "${base}"
}

echo "===== unsafe-triage.sh: 初回ベースライン生成（unsafe なし） ====="
WS1="$(setup_pseudo_workspace)"
cat > "${WS1}/crates/pseudo-crate/src/lib.rs" <<'EOF'
//! テスト用の擬似クレート（unsafe なし）。
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
EOF
set +e
out1="$(run_unsafe_triage_in "${WS1}" --update-baseline 2>&1)"
exit1=$?
set -e
assert_exit_code "unsafe なし擬似クレートの --update-baseline は exit 0" 0 "${exit1}"
if [ -f "${WS1}/scripts/unsafe-baseline.json" ]; then
    pass "ベースラインファイルが生成される"
else
    fail "ベースラインファイルが生成されていません"
fi

echo "===== unsafe-triage.sh: baseline 比較（変化なし） ====="
set +e
out2="$(run_unsafe_triage_in "${WS1}" 2>&1)"
exit2=$?
set -e
assert_exit_code "変化なしは exit 0" 0 "${exit2}"

echo "===== unsafe-triage.sh: SAFETY コメントなしの unsafe 追加を検知 ====="
cat >> "${WS1}/crates/pseudo-crate/src/lib.rs" <<'EOF'

#[allow(unsafe_code)]
unsafe fn danger() {}
EOF
set +e
out3="$(run_unsafe_triage_in "${WS1}" 2>&1)"
exit3=$?
set -e
assert_exit_code "SAFETY コメントなしの unsafe 追加は exit 1" 1 "${exit3}"
assert_contains "SAFETY コメント欠落エラーを報告する" "${out3}" "SAFETY:"

echo "===== unsafe-triage.sh: SAFETY コメント追加後も baseline 比較で増加検知 ====="
cat > "${WS1}/crates/pseudo-crate/src/lib.rs" <<'EOF'
//! テスト用の擬似クレート（unsafe あり、SAFETY コメント付き）。
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

// SAFETY: テスト用のダミー関数。実際には何もしないため安全。
#[allow(unsafe_code)]
unsafe fn danger() {}
EOF
set +e
out4="$(run_unsafe_triage_in "${WS1}" 2>&1)"
exit4=$?
set -e
assert_exit_code "SAFETY コメントありでも baseline 比較で増加は exit 1" 1 "${exit4}"
assert_contains "増加を検知して file:line を報告する" "${out4}" "pseudo-crate"

echo "===== unsafe-triage.sh: --update-baseline でベースライン更新後は exit 0 ====="
set +e
out5="$(run_unsafe_triage_in "${WS1}" --update-baseline 2>&1)"
exit5=$?
out6="$(run_unsafe_triage_in "${WS1}" 2>&1)"
exit6=$?
set -e
assert_exit_code "--update-baseline 自体は exit 0" 0 "${exit5}"
assert_exit_code "更新後の baseline 比較は exit 0" 0 "${exit6}"

rm -rf "${WS1}"

echo
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -ne 0 ]; then
    exit 1
fi
exit 0
