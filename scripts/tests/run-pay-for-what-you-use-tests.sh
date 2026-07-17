#!/usr/bin/env bash
# pay-for-what-you-use-check.sh のセルフテスト（TASK-2.2、#19）。
#
# `scripts/tests/fixtures/pay-for-what-you-use/*` を注入し、(a) プラグイン feature 列挙
# （命名規約違反・0 件のフェイルクローズを含む）・(b) cargo tree 判定（無効構成の依存
# 漏れ・有効構成の配線切れ・他プラグインとの混入）・(c) cargo geiger 判定・(d) バイナリ
# サイズ比較とシンボル表検査のロジックを、workspace の実状態・cargo ビルド・ネットワーク
# に依存せず固定化する。
#
# (e) 全構成ビルド検証は cargo ビルドそのものが検証対象のため fixture 化しない
# （`--skip-build-steps` で (d)/(e) の実ビルドを回避しつつ、注入した値で (d) の判定
# ロジックのみを検証する）。実ビルドを伴う (e) の動作確認は本スクリプトの通常実行
# （CI・人間によるローカル実行）に委ねる。
#
# run-dep-direction-tests.sh 等の既存セルフテストと同じく、ネットワーク・cargo ビルドに
# 依存せず完結させる（ci.yml の unsafe-triage ジョブから呼ばれる想定）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/pay-for-what-you-use"

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

run_check() {
    # $1 以降をそのまま pay-for-what-you-use-check.sh に渡す
    set +e
    output="$(bash "${SCRIPTS_DIR}/pay-for-what-you-use-check.sh" "$@" 2>&1)"
    status=$?
    set -e
}

echo "=== pay-for-what-you-use-check.sh セルフテスト ==="

# --- ケース 1: 正常構成（列挙・tree・geiger・size・symbols すべて green） ---
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-valid.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-clean.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-valid" \
    --geiger-packages-file "${FIXTURES_DIR}/geiger-packages-clean.txt" \
    --size-negative 1000 --size-positive 1200 \
    --symbols-file "${FIXTURES_DIR}/symbols-clean.txt"
assert_exit_code "正常構成は exit 0" 0 "${status}"
assert_contains "正常構成は (a) が PASS" "${output}" "[PASS] a:"
assert_contains "正常構成は (b) 無効構成が PASS" "${output}" "[PASS] b: cargo tree 検証（無効構成）"
assert_contains "正常構成は (b) 有効構成が PASS" "${output}" "[PASS] b: cargo tree 検証（有効構成 webrtc-proxy）"
assert_contains "正常構成は (c) が PASS" "${output}" "[PASS] c:"
assert_contains "正常構成は (d) サイズ比較が PASS" "${output}" "[PASS] d: バイナリサイズ計測"
assert_contains "正常構成は (d) シンボル検証が PASS" "${output}" "[PASS] d: シンボル表検証"

# --- ケース 2: プラグイン feature が 0 件（列挙ロジックの腐敗をフェイルクローズで検知） ---
run_check --skip-build-steps --metadata-file "${FIXTURES_DIR}/metadata-no-plugin-features.json"
assert_exit_code "feature 0 件は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "feature 0 件は判定不能メッセージを含む" "${output}" "1 件も見つかりませんでした（判定不能）"

# --- ケース 3: feature 命名規約違反（クレート名から導出した期待名と不一致） ---
run_check --skip-build-steps --metadata-file "${FIXTURES_DIR}/metadata-naming-violation.json"
assert_exit_code "命名規約違反は exit 1" 1 "${status}"
assert_contains "命名規約違反は違反内容を報告する" "${output}" "feature 命名規約"

# --- ケース 4: --metadata-file が存在しないパス ---
run_check --skip-build-steps --metadata-file "${FIXTURES_DIR}/does-not-exist.json"
assert_exit_code "存在しない metadata-file は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "存在しない metadata-file は判定不能メッセージを含む" "${output}" "存在しません"

# --- ケース 5: cargo tree（無効構成）にプラグインクレートが漏れている ---
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-valid.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-leaked.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-valid"
assert_exit_code "無効構成への依存漏れは exit 1" 1 "${status}"
assert_contains "無効構成への依存漏れは漏れクレートを報告する" "${output}" "無効構成にもかかわらず出現したクレート: bf-plugin-webrtc-proxy"

# --- ケース 6: cargo tree（有効構成）で対象クレートが出現しない（配線切れ） ---
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-valid.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-clean.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-missing"
assert_exit_code "配線切れ（有効構成に出現しない）は exit 1" 1 "${status}"
assert_contains "配線切れは配線切れの疑いメッセージを含む" "${output}" "配線切れの疑い"

# --- ケース 7: cargo tree（有効構成）で他プラグインクレートが混入 ---
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-two-plugins.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-clean.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-crosscontam"
assert_exit_code "他プラグイン混入は exit 1" 1 "${status}"
assert_contains "他プラグイン混入は混入クレートを報告する" "${output}" "他プラグインクレートが混入: bf-plugin-example-other"

# --- ケース 8: cargo geiger の依存グラフにプラグインクレートが出現（unsafe 計上対象） ---
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-valid.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-clean.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-valid" \
    --geiger-packages-file "${FIXTURES_DIR}/geiger-packages-leaked.txt"
assert_exit_code "geiger 漏れは exit 1" 1 "${status}"
assert_contains "geiger 漏れは漏れクレートを報告する" "${output}" "unsafe 計上対象になり得る"

# --- ケース 9: バイナリサイズが無効構成 > 有効構成（サイズ増加の異常） ---
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-valid.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-clean.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-valid" \
    --geiger-packages-file "${FIXTURES_DIR}/geiger-packages-clean.txt" \
    --size-negative 2000 --size-positive 1000
assert_exit_code "サイズ逆転は exit 1" 1 "${status}"
assert_contains "サイズ逆転はサイズ超過メッセージを含む" "${output}" "上回りました"

# --- ケース 10: シンボル表にプラグイン由来シンボルが混入 ---
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-valid.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-clean.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-valid" \
    --geiger-packages-file "${FIXTURES_DIR}/geiger-packages-clean.txt" \
    --size-negative 1000 --size-positive 1200 \
    --symbols-file "${FIXTURES_DIR}/symbols-leaked.txt"
assert_exit_code "シンボル混入は exit 1" 1 "${status}"
assert_contains "シンボル混入は混入クレートを報告する" "${output}" "プラグイン由来シンボルが検出されました: bf-plugin-webrtc-proxy"

echo ""
echo "=== 結果: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
