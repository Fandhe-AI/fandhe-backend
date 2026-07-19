#!/usr/bin/env bash
# pay-for-what-you-use-check.sh のセルフテスト（TASK-2.2、#19）。
#
# `scripts/tests/fixtures/pay-for-what-you-use/*` を注入し、(a) プラグイン feature 列挙
# （命名規約違反・0 件のフェイルクローズを含む）・(b) cargo tree 判定（無効構成の依存
# 漏れ・有効構成の配線切れ・他プラグインとの混入）・(c) cargo geiger 判定・(d) バイナリ
# サイズ比較とシンボル表検査のロジックを、workspace の実状態・cargo ビルド・ネットワーク
# に依存せず固定化する。(c) のリトライループ（一過性失敗からの回復・全失敗 fail-closed、
# Issue #212）は PFWU_GEIGER_CMD フックにモック geiger（fixtures/geiger-mock-*.sh）を
# 注入して検証する。
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
# --geiger-packages-file を明示しないと (c) が --skip-build-steps の対象外のまま
# 実 cargo-geiger 実行へフォールスルーし、「ネットワーク・cargo ビルド不要」の
# 前提（本ファイル冒頭のセルフテスト方針）が崩れる（cargo-geiger 導入済み環境で
# ビルドを伴う実行が発生し、unsafe-triage ジョブのタイムアウトリスクを招く）。
# (b) の判定を確認するケースのため (c) は clean fixture で無関係に固定する。
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-valid.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-leaked.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-valid" \
    --geiger-packages-file "${FIXTURES_DIR}/geiger-packages-clean.txt"
assert_exit_code "無効構成への依存漏れは exit 1" 1 "${status}"
assert_contains "無効構成への依存漏れは漏れクレートを報告する" "${output}" "無効構成にもかかわらず出現したクレート: fandhe-backend-plugin-webrtc-proxy"

# --- ケース 6: cargo tree（有効構成）で対象クレートが出現しない（配線切れ） ---
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-valid.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-clean.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-missing" \
    --geiger-packages-file "${FIXTURES_DIR}/geiger-packages-clean.txt"
assert_exit_code "配線切れ（有効構成に出現しない）は exit 1" 1 "${status}"
assert_contains "配線切れは配線切れの疑いメッセージを含む" "${output}" "配線切れの疑い"

# --- ケース 7: cargo tree（有効構成）で他プラグインクレートが混入 ---
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-two-plugins.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-clean.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-crosscontam" \
    --geiger-packages-file "${FIXTURES_DIR}/geiger-packages-clean.txt"
assert_exit_code "他プラグイン混入は exit 1" 1 "${status}"
assert_contains "他プラグイン混入は混入クレートを報告する" "${output}" "他プラグインクレートが混入: fandhe-backend-plugin-example-other"

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
assert_contains "シンボル混入は混入クレートを報告する" "${output}" "プラグイン由来シンボルが検出されました: fandhe-backend-plugin-webrtc-proxy"

# --- ケース 11: cargo geiger のパッケージリストが空（fail-closed。PASS/SKIP にしない） ---
# Bugbot 指摘（PR #134/#19）: geiger_packages が空のまま (c) をフォールスルーさせると
# leaked=0 件の判定に落ちて黙って未検証のまま終了してしまう。空リストは明示的に FAIL
# として扱われることを検証する。
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-valid.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-clean.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-valid" \
    --geiger-packages-file "${FIXTURES_DIR}/geiger-packages-empty.txt" \
    --size-negative 1000 --size-positive 1200 \
    --symbols-file "${FIXTURES_DIR}/symbols-clean.txt"
assert_exit_code "geiger パッケージリスト空は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "geiger パッケージリスト空は判定不能メッセージを含む" "${output}" "geiger_packages が空のため判定不能です"

# --- ケース 12: geiger 一過性失敗からのリトライ回復（Issue #212） ---
# --geiger-packages-file を渡さず、PFWU_GEIGER_CMD フックでモック geiger を注入して
# リトライループ本体を通す（--geiger-packages-file は jq 解析後の結果差し替えのみで
# ループを通らないため）。モックは 2 回失敗 → 3 回目成功。バックオフは
# PFWU_GEIGER_RETRY_WAIT=0 で無効化しテストを高速に保つ。
geiger_mock_state="$(mktemp)"
rm -f "${geiger_mock_state}" # モックが 1 回目から数え始めるよう未作成状態にする
export PFWU_GEIGER_CMD="${FIXTURES_DIR}/geiger-mock-flaky.sh"
export PFWU_GEIGER_RETRY_WAIT=0
export GEIGER_MOCK_STATE="${geiger_mock_state}"
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-valid.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-clean.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-valid" \
    --size-negative 1000 --size-positive 1200 \
    --symbols-file "${FIXTURES_DIR}/symbols-clean.txt"
assert_exit_code "geiger flaky（2 回失敗 → 3 回目成功）は exit 0（リトライ回復）" 0 "${status}"
assert_contains "geiger flaky 回復は (c) が PASS" "${output}" "[PASS] c:"
assert_contains "geiger flaky 回復は試行 1/3 の失敗理由ログを含む" "${output}" "試行 1/3 失敗:"
assert_contains "geiger flaky 回復は試行 2/3 の失敗理由ログを含む" "${output}" "試行 2/3 失敗:"
assert_contains "geiger flaky 回復は失敗理由（stderr 末尾）を転記する" "${output}" "pending_ids.insert(id)"
rm -f "${geiger_mock_state}"
unset GEIGER_MOCK_STATE

# --- ケース 13: geiger 全試行失敗は fail-closed で FAIL（Issue #212） ---
export PFWU_GEIGER_CMD="${FIXTURES_DIR}/geiger-mock-always-fail.sh"
run_check --skip-build-steps \
    --metadata-file "${FIXTURES_DIR}/metadata-valid.json" \
    --tree-negative-file "${FIXTURES_DIR}/tree-negative-clean.txt" \
    --tree-positive-dir "${FIXTURES_DIR}/tree-positive-valid" \
    --size-negative 1000 --size-positive 1200 \
    --symbols-file "${FIXTURES_DIR}/symbols-clean.txt"
assert_exit_code "geiger 全試行失敗は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "geiger 全試行失敗は (c) が FAIL" "${output}" "[FAIL] c: cargo geiger 検証 — cargo geiger の実行に失敗しました"
assert_contains "geiger 全試行失敗は試行 3/3 の失敗理由ログを含む" "${output}" "試行 3/3 失敗:"
assert_contains "geiger 全試行失敗はまとめ出力に stderr を転記する" "${output}" "----- cargo geiger stderr"
assert_contains "geiger 全試行失敗は失敗理由（panic メッセージ）を転記する" "${output}" "pending_ids.insert(id)"
unset PFWU_GEIGER_CMD
unset PFWU_GEIGER_RETRY_WAIT

echo ""
echo "=== 結果: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
