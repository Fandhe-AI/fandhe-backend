#!/usr/bin/env bash
# CI キャッシュ補助スクリプト（ci-cache-slim-target.sh / ci-cache-prune-generations.sh）の
# セルフテスト。
#
# `scripts/tests/fixtures/ci-cache/` の最小 JSON（cargo metadata 相当・Actions キャッシュ一覧
# 相当）を `--metadata-file` / `--input` で注入し、実 workspace・GitHub API・cargo ビルドに
# 依存せず判定ロジックを固定化する（ci.yml の unsafe-triage ジョブから呼ばれる想定）。
#   - slim: ワークスペースの成果物だけを消し、第三者依存（名前が衝突するものを含む）を残す
#   - prune: main スコープの同一 <family>-<OS> で最大版・最新の 1 件だけを残し、
#            ツールキャッシュ・PR スコープには触れない
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/ci-cache"

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

assert_exists() {
    if [ -e "$2" ]; then pass "$1"; else fail "$1（$2 が存在しません）"; fi
}

assert_absent() {
    if [ ! -e "$2" ]; then pass "$1"; else fail "$1（$2 が残っています）"; fi
}

# パイプ経由の grep -q は set -o pipefail 下で SIGPIPE による誤判定を招くため、bash 組み込みの
# パターンマッチで判定する（既存セルフテストと同じ方針）。
assert_contains() {
    if [[ "$2" == *"$3"* ]]; then pass "$1"; else fail "$1（'$3' が出力に含まれません）"; fi
}

assert_not_contains() {
    if [[ "$2" != *"$3"* ]]; then pass "$1"; else fail "$1（'$3' が出力に含まれています）"; fi
}

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

# ---------------------------------------------------------------------------
# ci-cache-slim-target.sh
# ---------------------------------------------------------------------------
H="0123456789abcdef"
make_profile() {
    local p="$1"
    mkdir -p "${p}/deps" "${p}/.fingerprint" "${p}/build" "${p}/examples" "${p}/incremental/demo_core-x"
    touch \
        "${p}/deps/libdemo_core-${H}.rlib" "${p}/deps/libdemo_core-${H}.rmeta" "${p}/deps/demo_core-${H}.d" \
        "${p}/deps/http_flow-${H}" "${p}/deps/http_flow-${H}.pdb" \
        "${p}/deps/demo_cli-${H}.exe" \
        "${p}/deps/libserde-${H}.rlib" "${p}/deps/serde-${H}.d" \
        "${p}/deps/libinterceptor-${H}.rlib" \
        "${p}/examples/minimal-${H}" \
        "${p}/demo-cli" "${p}/demo-cli.d" "${p}/demo-cli.exe" "${p}/keep-me.txt" \
        "${p}/libdemo_core.rlib" "${p}/libdemo_core.d" "${p}/libdemo_core.dylib" "${p}/demo_core.dll" \
        "${p}/libserde.rlib"
    mkdir -p "${p}/.fingerprint/demo-core-${H}" "${p}/.fingerprint/serde-${H}" \
        "${p}/build/demo-cli-${H}" "${p}/build/serde-${H}"
}

T="${WORK}/target"
make_profile "${T}/debug"
mkdir -p "${T}/doc/demo_core"
make_profile "${T}/x86_64-unknown-linux-gnu/release"

set +e
output="$(bash "${SCRIPTS_DIR}/ci-cache-slim-target.sh" --metadata-file "${FIXTURES_DIR}/metadata.json" --dry-run "${T}" 2>&1)"
status=$?
set -e
if [ "${status}" -eq 0 ]; then pass "slim --dry-run は exit 0"; else fail "slim --dry-run が exit ${status}: ${output}"; fi
assert_exists "slim --dry-run は何も消さない" "${T}/debug/deps/libdemo_core-${H}.rlib"
assert_contains "slim --dry-run は削除予定を表示する" "${output}" "(dry-run) rm -rf ${T}/debug/deps/libdemo_core-${H}.rlib"

set +e
output="$(bash "${SCRIPTS_DIR}/ci-cache-slim-target.sh" --metadata-file "${FIXTURES_DIR}/metadata.json" "${T}" "${WORK}/missing" 2>&1)"
status=$?
set -e
if [ "${status}" -eq 0 ]; then pass "slim は exit 0"; else fail "slim が exit ${status}: ${output}"; fi
assert_contains "存在しない TARGET_DIR はスキップする" "${output}" "存在しないためスキップ"

assert_absent "TARGET_DIR/doc（cargo doc --no-deps の出力）を消す" "${T}/doc"

for prof in "${T}/debug" "${T}/x86_64-unknown-linux-gnu/release"; do
    label="${prof#"${T}/"}"
    assert_absent "${label}: ワークスペース lib の rlib を消す" "${prof}/deps/libdemo_core-${H}.rlib"
    assert_absent "${label}: ワークスペース lib の rmeta を消す" "${prof}/deps/libdemo_core-${H}.rmeta"
    assert_absent "${label}: ワークスペース lib の .d を消す" "${prof}/deps/demo_core-${H}.d"
    assert_absent "${label}: テストバイナリを消す" "${prof}/deps/http_flow-${H}"
    assert_absent "${label}: テストバイナリの .pdb を消す" "${prof}/deps/http_flow-${H}.pdb"
    assert_absent "${label}: bin（ハイフン名→アンダースコア）を消す" "${prof}/deps/demo_cli-${H}.exe"
    assert_absent "${label}: examples/ を消す" "${prof}/examples"
    assert_absent "${label}: incremental/ を消す" "${prof}/incremental"
    assert_absent "${label}: プロファイル直下の bin を消す" "${prof}/demo-cli"
    assert_absent "${label}: プロファイル直下の bin の .d を消す" "${prof}/demo-cli.d"
    assert_absent "${label}: プロファイル直下の bin の .exe を消す" "${prof}/demo-cli.exe"
    assert_absent "${label}: プロファイル直下の lib の .rlib（deps へのハードリンク）を消す" "${prof}/libdemo_core.rlib"
    assert_absent "${label}: プロファイル直下の lib の .d を消す" "${prof}/libdemo_core.d"
    assert_absent "${label}: プロファイル直下の lib の .dylib を消す" "${prof}/libdemo_core.dylib"
    assert_absent "${label}: プロファイル直下の lib の .dll（Windows）を消す" "${prof}/demo_core.dll"
    assert_exists "${label}: プロファイル直下の第三者依存の .rlib を残す" "${prof}/libserde.rlib"
    assert_absent "${label}: ワークスペースの .fingerprint を消す" "${prof}/.fingerprint/demo-core-${H}"
    assert_absent "${label}: ワークスペースの build を消す" "${prof}/build/demo-cli-${H}"
    assert_exists "${label}: 第三者依存の rlib を残す" "${prof}/deps/libserde-${H}.rlib"
    assert_exists "${label}: 第三者依存の .d を残す" "${prof}/deps/serde-${H}.d"
    assert_exists "${label}: テスト名と衝突する第三者依存（interceptor）を残す" "${prof}/deps/libinterceptor-${H}.rlib"
    assert_exists "${label}: 第三者依存の .fingerprint を残す" "${prof}/.fingerprint/serde-${H}"
    assert_exists "${label}: 第三者依存の build を残す" "${prof}/build/serde-${H}"
    assert_exists "${label}: 無関係なファイルを残す" "${prof}/keep-me.txt"
done

set +e
output="$(bash "${SCRIPTS_DIR}/ci-cache-slim-target.sh" --metadata-file "${FIXTURES_DIR}/metadata.json" 2>&1)"
status=$?
set -e
if [ "${status}" -eq 2 ]; then pass "slim は TARGET_DIR 未指定で exit 2"; else fail "slim の TARGET_DIR 未指定が exit ${status}"; fi

# ---------------------------------------------------------------------------
# ci-cache-prune-generations.sh
# ---------------------------------------------------------------------------
set +e
output="$(bash "${SCRIPTS_DIR}/ci-cache-prune-generations.sh" --input "${FIXTURES_DIR}/caches.json" 2>&1)"
status=$?
set -e
if [ "${status}" -eq 0 ]; then pass "prune --input は exit 0"; else fail "prune --input が exit ${status}: ${output}"; fi
assert_contains "旧版（版なし = v1）を削除対象にする" "${output}" "delete test-Linux-20260928b940-"
assert_contains "同じ版の古い世代を削除対象にする" "${output}" "delete test-Linux-v2-20260928b940-aaaa"
assert_not_contains "同じ版の最新世代は残す" "${output}" "test-Linux-v2-20260928b940-bbbb"
assert_not_contains "OS が異なる系統（Windows）の唯一の世代は残す" "${output}" "test-Windows-v2"
assert_contains "toolchain が変わった旧世代を削除対象にする" "${output}" "delete fuzz-smoke-Linux-v2-20260928b940-"
assert_not_contains "toolchain 更新後の最新世代は残す" "${output}" "fuzz-smoke-Linux-v2-20261108aaaa"
assert_not_contains "ツールキャッシュ（64hex で終わらないキー）には触れない" "${output}" "cargo-tool-"
assert_not_contains "PR スコープのキャッシュには触れない" "${output}" "test-Linux-v2-20260928b940-cccc"
assert_contains "削除件数を集計する" "${output}" "旧世代 3 件"

set +e
output="$(bash "${SCRIPTS_DIR}/ci-cache-prune-generations.sh" --bogus 2>&1)"
status=$?
set -e
if [ "${status}" -eq 2 ]; then pass "prune は不明な引数で exit 2"; else fail "prune の不明な引数が exit ${status}"; fi

echo ""
echo "=== 結果: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
