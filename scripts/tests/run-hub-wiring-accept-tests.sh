#!/usr/bin/env bash
# scripts/accept/hub-wiring-accept.sh の判定ロジックのオフライン・セルフテスト
# （TASK-9.5 / #65）。
#
# `hub-wiring-accept.sh` 本体は cargo test・cargo tree・oha 実行等の副作用を持つため
# 直接 source せず、副作用のない `scripts/accept/lib/hub-wiring-loc.sh`
# （マーカー区間 LOC 集計・削減率判定・手書き配線シンボル検出）と
# `scripts/accept/lib/nfr6-ratio.sh`（既存、TASK-8.4 で検証済みのためここでは
# 再検証しない）を対象に、cargo・ネットワーク非依存で判定ロジックを回帰検証する
# （`scripts/tests/run-webrtc-accept-tests.sh` と同型の方針）。
#
# 呼び出し元: 人間 / CI が `bash scripts/tests/run-hub-wiring-accept-tests.sh` として
# 直接実行する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/hub-wiring-accept"

# shellcheck source=../accept/lib/hub-wiring-loc.sh
source "${SCRIPTS_DIR}/accept/lib/hub-wiring-loc.sh"

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

assert_eq() {
    local desc="$1" expected="$2" actual="$3"
    if [ "${expected}" = "${actual}" ]; then
        pass "${desc}"
    else
        fail "${desc}（期待: '${expected}'、実際: '${actual}'）"
    fi
}

echo "===== count_wiring_loc: マーカー区間の LOC 集計 ====="
assert_eq "clean フィクスチャは 3 行" "3" "$(count_wiring_loc "${FIXTURES_DIR}/clean.rs")"
assert_eq "no-marker フィクスチャはマーカー不在で 0 行" "0" "$(count_wiring_loc "${FIXTURES_DIR}/no-marker.rs")"
assert_eq "empty-region フィクスチャは区間内コメントのみで 0 行" "0" "$(count_wiring_loc "${FIXTURES_DIR}/empty-region.rs")"
assert_eq "with-handwritten-auth フィクスチャのマーカー区間自体は 2 行" "2" "$(count_wiring_loc "${FIXTURES_DIR}/with-handwritten-auth.rs")"

# 実ファイル（examples/hub_service_demo.rs）が意図どおり小さい配線区間になっている
# ことも回帰対象にする（crate doc 中の説明文がマーカーと誤認されないことの実証）。
DEMO_EXAMPLE="${SCRIPTS_DIR}/../crates/plugin-hub-wiring/examples/hub_service_demo.rs"
if [ -f "${DEMO_EXAMPLE}" ]; then
    demo_loc="$(count_wiring_loc "${DEMO_EXAMPLE}")"
    if [ "${demo_loc}" -ge 1 ] && [ "${demo_loc}" -le 20 ]; then
        pass "実ファイル hub_service_demo.rs のマーカー区間 LOC は妥当範囲（${demo_loc} 行、1〜20 行）"
    else
        fail "実ファイル hub_service_demo.rs のマーカー区間 LOC が想定外（${demo_loc} 行）。crate doc 中の言及をマーカーと誤認していないか確認すること"
    fi
else
    echo "情報: ${DEMO_EXAMPLE} が見つからないためこのケースは省略" >&2
fi

echo "===== evaluate_wiring_reduction: 削減率の PASS/FAIL 境界 ====="
assert_eq "0 行（削減率 100%）は PASS" "PASS 100.0" "$(evaluate_wiring_reduction 0)"
assert_eq "6 行（削減率 97.1%、実測相当）は PASS" "PASS 97.1" "$(evaluate_wiring_reduction 6)"
assert_eq "20 行（削減率 90.3%）は PASS" "PASS 90.3" "$(evaluate_wiring_reduction 20)"
assert_eq "21 行（削減率 89.9%、90% 未満）は FAIL" "FAIL 89.9" "$(evaluate_wiring_reduction 21)"
assert_eq "207 行（削減率 0%、そのまま）は FAIL" "FAIL 0.0" "$(evaluate_wiring_reduction 207)"

echo "===== detect_handwritten_auth_in_handlers: ハンドラ領域の手書き配線シンボル検出 ====="
clean_hits="$(detect_handwritten_auth_in_handlers "${FIXTURES_DIR}/clean.rs")"
if [ -z "${clean_hits}" ]; then
    pass "clean フィクスチャは手書き配線シンボル 0 件と判定"
else
    fail "clean フィクスチャで誤検出: ${clean_hits}"
fi

handwritten_hits="$(detect_handwritten_auth_in_handlers "${FIXTURES_DIR}/with-handwritten-auth.rs")"
if [ -n "${handwritten_hits}" ]; then
    pass "with-handwritten-auth フィクスチャは手書き配線シンボル検出あり"
else
    fail "with-handwritten-auth フィクスチャで見逃し（verify_token 混入を検知できず）"
fi

if [ -f "${DEMO_EXAMPLE}" ]; then
    demo_hits="$(detect_handwritten_auth_in_handlers "${DEMO_EXAMPLE}")"
    if [ -z "${demo_hits}" ]; then
        pass "実ファイル hub_service_demo.rs のハンドラ領域は手書き配線シンボル 0 件"
    else
        fail "実ファイル hub_service_demo.rs のハンドラ領域で手書き配線シンボルを検出: ${demo_hits}"
    fi
fi

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
exit 0
