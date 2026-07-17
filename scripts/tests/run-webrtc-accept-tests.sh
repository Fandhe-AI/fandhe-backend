#!/usr/bin/env bash
# scripts/accept/webrtc-accept.sh の判定ロジックのオフライン・セルフテスト
# （TASK-8.4 / #29）。
#
# `webrtc-accept.sh` 本体は cargo tree・cargo audit・oha 実行等の副作用を持つため
# 直接 source せず、副作用のない `scripts/accept/lib/nfr6-ratio.sh`
# （`evaluate_nfr6_ratio`）と unsafe grep ロジック（`scripts/tests/fixtures/webrtc-accept/`
# の擬似ソースを対象にした grep パイプライン）を対象に、cargo・ネットワーク非依存で
# 判定境界値を回帰検証する（`scripts/tests/run-triage-tests.sh` 等、既存の受け入れ系
# オフラインテストと同じ方針）。
#
# 呼び出し元: 人間 / CI が `bash scripts/tests/run-webrtc-accept-tests.sh` として
# 直接実行する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${SCRIPTS_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/webrtc-accept"

# shellcheck source=../accept/lib/nfr6-ratio.sh
source "${SCRIPTS_DIR}/accept/lib/nfr6-ratio.sh"

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

echo "===== evaluate_nfr6_ratio: 狭義 NFR-6 帯（100.3〜100.8%）は PASS ====="
assert_eq "帯の下端 100.3 は PASS" "PASS" "$(evaluate_nfr6_ratio "100.3")"
assert_eq "帯の上端 100.8 は PASS" "PASS" "$(evaluate_nfr6_ratio "100.8")"
assert_eq "帯中央 100.5 は PASS" "PASS" "$(evaluate_nfr6_ratio "100.5")"

echo "===== evaluate_nfr6_ratio: 実務許容帯内・狭義帯外は WARN ====="
# 本タスクの実測値（TASK-8.4、baseline 比 95.23%）が該当する境界ケース。
assert_eq "実測相当 95.23 は WARN" "WARN" "$(evaluate_nfr6_ratio "95.23")"
assert_eq "実務下端 95 は WARN" "WARN" "$(evaluate_nfr6_ratio "95")"
assert_eq "実務上端 105 は WARN" "WARN" "$(evaluate_nfr6_ratio "105")"
assert_eq "99.9（狭義帯直下）は WARN" "WARN" "$(evaluate_nfr6_ratio "99.9")"

echo "===== evaluate_nfr6_ratio: 実務許容帯外は FAIL（フェイルクローズ） ====="
assert_eq "94.99 は FAIL" "FAIL" "$(evaluate_nfr6_ratio "94.99")"
assert_eq "105.01 は FAIL" "FAIL" "$(evaluate_nfr6_ratio "105.01")"
assert_eq "大幅劣化 60 は FAIL" "FAIL" "$(evaluate_nfr6_ratio "60")"
assert_eq "異常値 200 は FAIL" "FAIL" "$(evaluate_nfr6_ratio "200")"

echo "===== 基準B: unsafe grep ロジック（webrtc-accept.sh と同一パターン） ====="
# webrtc-accept.sh の check_unsafe と同一の grep パイプラインをフィクスチャへ適用し、
# SAFETY コメント誤検出・複数行検出のリグレッションを防ぐ。
check_unsafe_fixture() {
    local dir="$1"
    grep -rn --include='*.rs' -E '\bunsafe\b' "${dir}" | grep -v -E '^[^:]*:[0-9]+:[[:space:]]*//' || true
}

clean_hits="$(check_unsafe_fixture "${FIXTURES_DIR}/clean")"
if [ -z "${clean_hits}" ]; then
    pass "clean フィクスチャは unsafe 0 件と判定"
else
    fail "clean フィクスチャで誤検出: ${clean_hits}"
fi

unsafe_hits="$(check_unsafe_fixture "${FIXTURES_DIR}/with-unsafe")"
if [ -n "${unsafe_hits}" ]; then
    pass "with-unsafe フィクスチャは unsafe 検出あり"
else
    fail "with-unsafe フィクスチャで見逃し（unsafe 使用を検知できず）"
fi

# SAFETY コメント中の "unsafe" という字句自体（行コメント）は誤検出しないこと。
comment_only_hits="$(check_unsafe_fixture "${FIXTURES_DIR}/comment-only")"
if [ -z "${comment_only_hits}" ]; then
    pass "comment-only フィクスチャ（// SAFETY コメントのみ）は誤検出なし"
else
    fail "comment-only フィクスチャで誤検出（コメント中の unsafe 字句を実コードと誤認）: ${comment_only_hits}"
fi

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
exit 0
