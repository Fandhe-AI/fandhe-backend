#!/usr/bin/env bash
# dep-direction-check.sh のセルフテスト（TASK-1.5/#14 で新設、TASK-11.1/#33 で
# --crates-dir 系ケースを追加）。
#
# `scripts/tests/fixtures/dep-direction/*.json`（cargo metadata --no-deps
# --format-version 1 相当の最小 JSON）を `--metadata-file` で注入し、
# workspace の実際の Cargo.toml 構成に依存せず判定ロジック（ホワイトリスト
# 照合・循環検出・dev-dependency 除外）を固定化する。チェック 2（エントリポイント
# 依存方向宣言）は `scripts/tests/fixtures/dep-direction/crates-valid/` ・
# `crates-missing-declaration/` の最小クレートツリーを `--crates-dir` で注入し、
# 宣言あり（`src/lib.rs`・`src/main.rs` フォールバック双方）の PASS と宣言欠落の
# FAIL を実 workspace の状態に依存せず固定化する。
#
# run-feature-flow-tests.sh 等の既存セルフテストと同じく、ネットワーク・cargo
# ビルドに依存せず完結させる（ci.yml の unsafe-triage ジョブから呼ばれる想定）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/dep-direction"

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

run_check() {
    local metadata_file="$1"
    set +e
    output="$(bash "${SCRIPTS_DIR}/dep-direction-check.sh" --metadata-file "${metadata_file}" 2>&1)"
    status=$?
    set -e
}

# チェック 2（エントリポイント依存方向宣言）を fixture クレートツリーで隔離検証する。
# --metadata-file にはチェック 1 が確実に PASS する valid-graph.json を併用し、
# チェック 2 の PASS/FAIL のみを fixture の差し替えで制御する。
run_check_crates_dir() {
    local crates_dir="$1"
    set +e
    output="$(bash "${SCRIPTS_DIR}/dep-direction-check.sh" \
        --metadata-file "${FIXTURES_DIR}/valid-graph.json" \
        --crates-dir "${crates_dir}" 2>&1)"
    status=$?
    set -e
}

echo "=== dep-direction-check.sh セルフテスト ==="

# --- ケース 1: 正常グラフ（server → routes → http::* の一方向・循環なし） ---
run_check "${FIXTURES_DIR}/valid-graph.json"
assert_exit_code "正常グラフは exit 0" 0 "${status}"
assert_contains "正常グラフはチェック1が PASS" "${output}" "[PASS] 1:"

# --- ケース 2: 逆方向エッジ（http → routes → http の循環） ---
run_check "${FIXTURES_DIR}/reverse-edge.json"
assert_exit_code "逆方向エッジグラフは exit 1" 1 "${status}"
assert_contains "逆方向エッジグラフは循環検出で FAIL" "${output}" "循環が検出されました"

# --- ケース 3: コアから未許可プラグインへの依存（ホワイトリスト違反、循環なし） ---
# ホワイトリスト方式の意図（本体の該当コメント参照）「未知のエッジはすべて拒否」を
# 固定するケース。`fandhe-backend-plugin-webrtc-proxy` は TASK-2.1（#18）の例外として許可済みに
# なったため、まだ許可されていない架空プラグイン名でこの fail-closed 挙動を検証する
# （ケース 3-2 で許可済みエッジ自体は PASS することを別途固定する）。
run_check "${FIXTURES_DIR}/core-depends-on-unlisted-plugin.json"
assert_exit_code "コア→未許可プラグイン依存グラフは exit 1" 1 "${status}"
assert_contains "コア→未許可プラグイン依存グラフは許可リスト外エッジで FAIL" "${output}" "許可リスト外のエッジ"
assert_contains "違反エッジの内容を報告する" "${output}" "fandhe-backend-core -> fandhe-backend-plugin-example-unlisted"

# --- ケース 3-2: コアから許可済みプラグイン（fandhe-backend-plugin-webrtc-proxy）への依存は PASS ---
# TASK-2.1（#18）で確立した唯一の例外エッジ（本体の該当コメント参照）。
run_check "${FIXTURES_DIR}/core-depends-on-whitelisted-plugin.json"
assert_exit_code "コア→許可済みプラグイン依存グラフは exit 0" 0 "${status}"
assert_contains "コア→許可済みプラグイン依存グラフはチェック1が PASS" "${output}" "[PASS] 1:"

# --- ケース 4: dev-dependency は判定対象外（http --dev--> routes があっても正常扱い） ---
run_check "${FIXTURES_DIR}/dev-dependency-excluded.json"
assert_exit_code "dev-dependency のみの逆方向は exit 0" 0 "${status}"
assert_contains "dev-dependency 除外グラフはチェック1が PASS" "${output}" "[PASS] 1:"

# --- ケース 5: --metadata-file に存在しないパスを渡すと判定不能として FAIL ---
run_check "${FIXTURES_DIR}/does-not-exist.json"
assert_exit_code "存在しない metadata-file は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "存在しない metadata-file は判定不能メッセージを含む" "${output}" "存在しません"

# --- ケース 6: 実 workspace（既定 --crates-dir crates）で全チェック PASS ---
run_check "${FIXTURES_DIR}/valid-graph.json"
assert_contains "実 workspace エントリポイント宣言検査（チェック2）が PASS" "${output}" "[PASS] 2:"
assert_contains "実 workspace プラグイン非依存検査（チェック3）が PASS" "${output}" "[PASS] 3:"

# --- ケース 7: --crates-dir crates-valid — 宣言あり（lib.rs・main.rs フォールバック双方）は PASS ---
run_check_crates_dir "${FIXTURES_DIR}/crates-valid"
assert_exit_code "crates-valid fixture は exit 0" 0 "${status}"
assert_contains "crates-valid fixture はチェック2が PASS（2 クレート列挙）" "${output}" "[PASS] 2: エントリポイント依存方向宣言 — ${FIXTURES_DIR}/crates-valid 直下 2 クレート"

# --- ケース 8: --crates-dir crates-missing-declaration — 宣言欠落クレートを含む場合は FAIL（フェイルクローズ） ---
run_check_crates_dir "${FIXTURES_DIR}/crates-missing-declaration"
assert_exit_code "crates-missing-declaration fixture は exit 1（フェイルクローズ）" 1 "${status}"
assert_contains "crates-missing-declaration fixture はチェック2が欠落ファイル名を報告する" "${output}" "crate-b-bin/src/main.rs"

echo ""
echo "=== 結果: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
