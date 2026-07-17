#!/usr/bin/env bash
# req13-change-impact-accept.sh のセルフテスト（TASK-13.2/#50 で新設）。
#
# 基準 B（プラグイン拡張点対応宣言）・基準 C（契約ドキュメント必須セクション）は
# `--crates-dir` / `--contract-doc` の注入口（dep-direction-check.sh の
# `--crates-dir` 慣例を踏襲）でミニクレート群・ダミードキュメントを与え、
# workspace の実データに依存せず判定ロジックを固定化する。
#
# 基準 A・D・E・F は workspace の実データ（dep-direction-check.sh・実コミット sha・
# extension-closure-verification.md・既存セルフテスト群）に対して実行するのが
# 本質的に妥当なため（plugin-mechanism-accept.sh 等の既存受け入れスクリプトと同方針）、
# 本テストでは「フル実行が非 0 終了しないこと」のみを別途スモーク確認する。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPTS_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/req13-accept"
ACCEPT_SCRIPT="${SCRIPTS_DIR}/accept/req13-change-impact-accept.sh"

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

echo "=== req13-change-impact-accept.sh セルフテスト ==="

# 実在の契約ドキュメント（基準 C を PASS させ、基準 B のみを差し替えて検証する）。
REAL_CONTRACT_DOC="${WORKSPACE_ROOT}/docs/design/dependency-graph-contract.md"

# --- ケース 1: 宣言完備のミニクレート群は基準 B が PASS ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --crates-dir "${FIXTURES_DIR}/ok" --contract-doc "${REAL_CONTRACT_DOC}" 2>&1)"
set -e
assert_contains "宣言完備は基準 B が PASS" "${output}" "[PASS] B:"

# --- ケース 2: 宣言欠落のミニクレートは基準 B が FAIL ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --crates-dir "${FIXTURES_DIR}/missing" --contract-doc "${REAL_CONTRACT_DOC}" 2>&1)"
set -e
assert_contains "宣言欠落は基準 B が FAIL" "${output}" "[FAIL] B:"
assert_contains "宣言欠落は「宣言欠落」を報告する" "${output}" "宣言欠落"

# --- ケース 3: 語彙外の宣言は基準 B が FAIL ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --crates-dir "${FIXTURES_DIR}/bad-vocab" --contract-doc "${REAL_CONTRACT_DOC}" 2>&1)"
set -e
assert_contains "語彙外の宣言は基準 B が FAIL" "${output}" "[FAIL] B:"
assert_contains "語彙外の宣言は「語彙外」を報告する" "${output}" "語彙外"

# --- ケース 4: 非該当宣言の参照先が存在しない場合は基準 B が FAIL ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --crates-dir "${FIXTURES_DIR}/bad-ref" --contract-doc "${REAL_CONTRACT_DOC}" 2>&1)"
set -e
assert_contains "参照先不在の宣言は基準 B が FAIL" "${output}" "[FAIL] B:"
assert_contains "参照先不在の宣言は「参照先不備」を報告する" "${output}" "参照先不備"

# --- ケース 5: 契約ドキュメント必須見出し完備は基準 C が PASS ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --crates-dir "${FIXTURES_DIR}/ok" --contract-doc "${FIXTURES_DIR}/contract-complete.md" 2>&1)"
set -e
assert_contains "必須見出し完備は基準 C が PASS" "${output}" "[PASS] C:"

# --- ケース 6: 契約ドキュメント必須見出し欠落は基準 C が FAIL ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --crates-dir "${FIXTURES_DIR}/ok" --contract-doc "${FIXTURES_DIR}/contract-missing-headings.md" 2>&1)"
set -e
assert_contains "必須見出し欠落は基準 C が FAIL" "${output}" "[FAIL] C:"
assert_contains "必須見出し欠落は欠落見出しを列挙する" "${output}" "必須見出し欠落"

# --- ケース 7: 契約ドキュメント不在は基準 C が FAIL ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --crates-dir "${FIXTURES_DIR}/ok" --contract-doc "${FIXTURES_DIR}/does-not-exist.md" 2>&1)"
set -e
assert_contains "契約ドキュメント不在は基準 C が FAIL" "${output}" "[FAIL] C:"

# --- ケース 8: crates-dir に plugin-* が 1 件もない場合は基準 B が FAIL（フェイルクローズ） ---
mkdir -p "${FIXTURES_DIR}/no-plugins-empty-dir-marker"
set +e
output="$(bash "${ACCEPT_SCRIPT}" --crates-dir "${FIXTURES_DIR}/no-plugins-empty-dir-marker" --contract-doc "${REAL_CONTRACT_DOC}" 2>&1)"
set -e
assert_contains "plugin-* 0 件は基準 B が FAIL（フェイルクローズ）" "${output}" "[FAIL] B:"

# --- ケース 9: フル実行（workspace の実データ、基準 A・D・E・F 含む）は非 0 終了しない ---
set +e
bash "${ACCEPT_SCRIPT}" >/tmp/req13-accept-selftest-full-run.log 2>&1
full_status=$?
set -e
if [ "${full_status}" -eq 0 ]; then
    pass "フル実行（workspace 実データ）は exit 0（詳細: /tmp/req13-accept-selftest-full-run.log）"
else
    fail "フル実行（workspace 実データ）が exit ${full_status}（詳細: /tmp/req13-accept-selftest-full-run.log）"
fi

echo ""
echo "=== 結果: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
