#!/usr/bin/env bash
# extension-closure-gate.sh のセルフテスト（TASK-13.2/#50 で新設）。
#
# `scripts/tests/fixtures/extension-closure-gate/*.txt`（1 行 1 パスの変更ファイルリスト）を
# `--files-from` で注入し、workspace の実際の git 履歴・merge-base 解決に依存せず
# SKIP/PASS/理由明記済み PASS/FAIL・フェイルクローズ挙動を固定化する。`--base` 経由の
# merge-base 解決・ref 検証パスは実 git リポジトリでのみ検証可能なため、run-*-tests.sh の
# 慣例どおり最小限のフェイルクローズケース（不正 ref・引数欠落等）のみ本テストで直接確認する。
#
# 既存セルフテスト（run-extension-closure-tests.sh 等）と同じく、ネットワーク・cargo
# ビルドに依存せず完結させる。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/extension-closure-gate"

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

run_gate_files() {
    local files_file="$1"
    set +e
    output="$(bash "${SCRIPTS_DIR}/extension-closure-gate.sh" --files-from "${files_file}" 2>&1)"
    status=$?
    set -e
}

echo "=== extension-closure-gate.sh セルフテスト ==="

# --- ケース 1: 拡張点に無関係な変更は SKIP（exit 0） ---
run_gate_files "${FIXTURES_DIR}/skip-unrelated.txt"
assert_exit_code "拡張点無関係の変更は exit 0（SKIP）" 0 "${status}"
assert_contains "拡張点無関係の変更は [RESULT] SKIP を含む" "${output}" "[RESULT] SKIP"

# --- ケース 2: 拡張点関連かつ A〜D に閉じる変更は PASS ---
run_gate_files "${FIXTURES_DIR}/closed-pass.txt"
assert_exit_code "閉包する拡張点変更は exit 0（PASS）" 0 "${status}"
assert_contains "閉包する拡張点変更は [RESULT] PASS を含む" "${output}" "[RESULT] PASS"

# --- ケース 3: E ファイルがあるが docs/design/*.md に理由記載済みなら WARN 付き PASS ---
# violation-documented.txt の crates/http/src/response.rs は
# docs/design/extension-closure-verification.md 3.2 節に実際に記載済み（TASK-13.1 実例）。
run_gate_files "${FIXTURES_DIR}/violation-documented.txt"
assert_exit_code "理由明記済み逸脱は exit 0（WARN 付き PASS）" 0 "${status}"
assert_contains "理由明記済み逸脱は [WARN] を含む" "${output}" "[WARN]"
assert_contains "理由明記済み逸脱は [RESULT] PASS を含む" "${output}" "[RESULT] PASS"

# --- ケース 4: E ファイルが docs/design/*.md のどこにも記載がなければ FAIL ---
run_gate_files "${FIXTURES_DIR}/violation-undocumented.txt"
assert_exit_code "理由未記載の逸脱は exit 1（FAIL）" 1 "${status}"
assert_contains "理由未記載の逸脱は [RESULT] FAIL を含む" "${output}" "[RESULT] FAIL"
assert_contains "理由未記載の逸脱は未記載ファイルを列挙する" "${output}" "[未記載] crates/core/src/nonexistent_totally_undocumented_file_for_gate_test.rs"

# --- ケース 5: 変更ファイル 0 件は判定不能として FAIL（フェイルクローズ） ---
run_gate_files "${FIXTURES_DIR}/empty.txt"
assert_exit_code "空ファイルリストは exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "空ファイルリストは測定不能メッセージを含む" "${output}" "変更ファイルが 0 件"

# --- ケース 6: 空白行のみのファイルリストは判定不能として FAIL（フェイルクローズ） ---
run_gate_files "${FIXTURES_DIR}/blank-lines-only.txt"
assert_exit_code "空白行のみのファイルリストは exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "空白行のみのファイルリストは測定不能メッセージを含む" "${output}" "変更ファイルが 0 件"

# --- ケース 7: --files-from に存在しないパスを渡すと判定不能として FAIL ---
set +e
output="$(bash "${SCRIPTS_DIR}/extension-closure-gate.sh" --files-from "${FIXTURES_DIR}/does-not-exist.txt" 2>&1)"
status=$?
set -e
assert_exit_code "存在しない files-from は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "存在しない files-from は判定不能メッセージを含む" "${output}" "存在しません"

# --- ケース 8: 引数なしは判定不能として FAIL（フェイルクローズ） ---
set +e
output="$(bash "${SCRIPTS_DIR}/extension-closure-gate.sh" 2>&1)"
status=$?
set -e
assert_exit_code "引数なしは exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "引数なしは必須引数メッセージを含む" "${output}" "いずれかが必須です"

# --- ケース 9: --base と --files-from の同時指定は判定不能として FAIL ---
set +e
output="$(bash "${SCRIPTS_DIR}/extension-closure-gate.sh" --base origin/main --files-from "${FIXTURES_DIR}/closed-pass.txt" 2>&1)"
status=$?
set -e
assert_exit_code "同時指定は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "同時指定は排他エラーメッセージを含む" "${output}" "同時指定できません"

# --- ケース 10: --base に解決不能な ref を渡すと判定不能として FAIL（フェイルクローズ） ---
set +e
output="$(bash "${SCRIPTS_DIR}/extension-closure-gate.sh" --base does-not-exist-ref-for-gate-test-xyz 2>&1)"
status=$?
set -e
assert_exit_code "解決不能な ref は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "解決不能な ref は判定不能メッセージを含む" "${output}" "ref として解決できません"

echo ""
echo "=== 結果: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
