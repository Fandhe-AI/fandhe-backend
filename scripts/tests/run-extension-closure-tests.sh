#!/usr/bin/env bash
# extension-closure-check.sh のセルフテスト（TASK-13.1/#49 で新設）。
#
# `scripts/tests/fixtures/extension-closure/*.txt`（1 行 1 パスの変更ファイルリスト）を
# `--files-from` で注入し、workspace の実際の git 履歴に依存せず判定ロジック（A〜D
# カテゴリ分類・E 検出・フェイルクローズ挙動）を固定化する。実コミット（3ae6d11 等）への
# 適用結果は `docs/design/extension-closure-verification.md` に記録し、本セルフテストとは
# 独立に確認する（shallow clone 環境では実コミットが解決できず誤 FAIL しうるため、
# 実コミット依存のケースは本セルフテストに含めない）。
#
# run-dep-direction-tests.sh 等の既存セルフテストと同じく、ネットワーク・cargo ビルドに
# 依存せず完結させる（ci.yml のセルフテスト群と同じ並びから呼ばれる想定）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/extension-closure"

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

run_check_files() {
    local files_file="$1"
    set +e
    output="$(bash "${SCRIPTS_DIR}/extension-closure-check.sh" --files-from "${files_file}" 2>&1)"
    status=$?
    set -e
}

echo "=== extension-closure-check.sh セルフテスト ==="

# --- ケース 1: A〜D に閉じる変更リストは PASS ---
run_check_files "${FIXTURES_DIR}/closed-example.txt"
assert_exit_code "閉包ファイルリストは exit 0" 0 "${status}"
assert_contains "閉包ファイルリストは [RESULT] PASS を含む" "${output}" "[RESULT] PASS"
assert_contains "閉包ファイルリストは E 件数 0 を報告する" "${output}" "E. 閉包違反候補:         0 件"

# --- ケース 2: crates/http・crates/routes を含む変更リストは FAIL（E 検出） ---
run_check_files "${FIXTURES_DIR}/violation-example.txt"
assert_exit_code "違反ファイルリストは exit 1（閉包違反）" 1 "${status}"
assert_contains "違反ファイルリストは [RESULT] FAIL を含む" "${output}" "[RESULT] FAIL"
assert_contains "違反ファイル crates/http/src/response.rs を報告する" "${output}" "[E] crates/http/src/response.rs"
assert_contains "違反ファイル crates/routes/src/router.rs を報告する" "${output}" "[E] crates/routes/src/router.rs"

# --- ケース 3: 変更ファイル 0 件は判定不能として FAIL（フェイルクローズ） ---
run_check_files "${FIXTURES_DIR}/empty.txt"
assert_exit_code "空ファイルリストは exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "空ファイルリストは測定不能メッセージを含む" "${output}" "変更ファイルが 0 件"

# --- ケース 3b: 空白行のみのファイルリストは判定不能として FAIL（フェイルクローズ）
# PR #147 Bugbot 指摘対応: 対象ファイル 0 件判定を「対象ファイル総数」と同じ
# `grep -c .` ベースに統一し、空白行のみの入力を確実にフェイルクローズさせることを
# 固定化する（`--files-from` は `$(cat ...)` コマンド置換経由のため、本 fixture
# 自体は既存の `-z` 判定のみでも FAIL していたが、判定根拠の一致という修正意図を
# 回帰テストとして残す） ---
run_check_files "${FIXTURES_DIR}/blank-lines-only.txt"
assert_exit_code "空白行のみのファイルリストは exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "空白行のみのファイルリストは測定不能メッセージを含む" "${output}" "変更ファイルが 0 件"

# --- ケース 4: --files-from に存在しないパスを渡すと判定不能として FAIL ---
set +e
output="$(bash "${SCRIPTS_DIR}/extension-closure-check.sh" --files-from "${FIXTURES_DIR}/does-not-exist.txt" 2>&1)"
status=$?
set -e
assert_exit_code "存在しない files-from は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "存在しない files-from は判定不能メッセージを含む" "${output}" "存在しません"

# --- ケース 5: 引数なしは判定不能として FAIL（フェイルクローズ） ---
set +e
output="$(bash "${SCRIPTS_DIR}/extension-closure-check.sh" 2>&1)"
status=$?
set -e
assert_exit_code "引数なしは exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "引数なしは必須引数メッセージを含む" "${output}" "いずれかが必須です"

# --- ケース 6: --commit に不正な sha 形式を渡すと判定不能として FAIL ---
set +e
output="$(bash "${SCRIPTS_DIR}/extension-closure-check.sh" --commit zzzz 2>&1)"
status=$?
set -e
assert_exit_code "不正な sha 形式は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "不正な sha 形式は形式エラーメッセージを含む" "${output}" "commit sha 形式"

# --- ケース 7: --commit と --files-from の同時指定は判定不能として FAIL ---
set +e
output="$(bash "${SCRIPTS_DIR}/extension-closure-check.sh" --commit 0000000 --files-from "${FIXTURES_DIR}/closed-example.txt" 2>&1)"
status=$?
set -e
assert_exit_code "同時指定は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "同時指定は排他エラーメッセージを含む" "${output}" "同時指定できません"

echo ""
echo "=== 結果: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
