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

echo "===== evaluate_nfr6_ratio: p95 比が判定に寄与すること（レビュー指摘 #29 是正） ====="
# RPS 比が帯内でも p95 比が実務許容帯（105%）を超えれば総合判定は FAIL とする。
assert_eq "RPS 100.5（狭義帯内）・p95 106.57（実務帯外）は FAIL" "FAIL" "$(evaluate_nfr6_ratio "100.5" "106.57")"
# RPS・p95 とも狭義帯内なら PASS。
assert_eq "RPS 100.5・p95 100.5（双方狭義帯内）は PASS" "PASS" "$(evaluate_nfr6_ratio "100.5" "100.5")"
# RPS は狭義帯内でも p95 が実務帯内・狭義帯外なら WARN に格上げされる。
assert_eq "RPS 100.5・p95 101.0（実務帯内・狭義帯外）は WARN" "WARN" "$(evaluate_nfr6_ratio "100.5" "101.0")"
# 本タスクの実測値（TASK-8.4、1 回目: RPS 95.23% / p95 106.57%）は RPS 単独では WARN
# 相当だが、p95 が実務帯を超えるため総合判定は FAIL となる。
assert_eq "実測相当（RPS 95.23・p95 106.57）は FAIL" "FAIL" "$(evaluate_nfr6_ratio "95.23" "106.57")"
# p95 が低い（レイテンシが改善している）方向への乖離は問題にしない（下限なし）。
assert_eq "RPS 100.5・p95 50（低レイテンシ方向）は PASS" "PASS" "$(evaluate_nfr6_ratio "100.5" "50")"
# 第 2 引数省略時は従来どおり RPS 比のみで判定する（後方互換）。
assert_eq "p95 省略時は RPS 比のみで判定" "PASS" "$(evaluate_nfr6_ratio "100.5")"

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

echo "===== 基準A: cargo tree 失敗時のフェイルクローズ（Bugbot 指摘是正） ====="
# webrtc-accept.sh check_dep_exclusion と同一の判定パターン（cargo tree の終了コードを
# 明示確認し、失敗時は測定不能として FAIL/WARN を返す）を検証する。実 cargo は使わず、
# 成功/失敗をシミュレートするダミー関数の出力をパイプラインへ渡す。
evaluate_dep_exclusion() {
    # $1: cargo tree 相当の標準出力
    # $2: cargo tree 相当の終了コード（0=成功、非 0=失敗）
    local tree_output="$1" exit_code="$2" disabled_count
    if [ "${exit_code}" -ne 0 ]; then
        echo "FAIL"
        return
    fi
    disabled_count="$(printf '%s\n' "${tree_output}" | grep -c webrtc || true)"
    if [ "${disabled_count}" -eq 0 ]; then
        echo "PASS"
    else
        echo "FAIL"
    fi
}

# cargo tree 自体が失敗（stdout 空・非 0 終了）した場合は、webrtc 系依存 0 件との
# 区別がつかず誤って PASS してしまう旧実装のリグレッションを防ぐ（Bugbot review
# id 4723597731 指摘 1: PR #146）。
assert_eq "cargo tree 失敗時（stdout 空・非 0 終了）は FAIL" "FAIL" "$(evaluate_dep_exclusion "" 1)"
# cargo tree が成功し webrtc 系依存が真に 0 件の場合は PASS。
assert_eq "cargo tree 成功・webrtc 依存 0 件は PASS" "PASS" "$(evaluate_dep_exclusion "fandhe-backend-core v0.1.0
tokio v1.40.0" 0)"
# cargo tree が成功し webrtc 系依存が残留している場合は FAIL。
assert_eq "cargo tree 成功・webrtc 依存残留は FAIL" "FAIL" "$(evaluate_dep_exclusion "fandhe-backend-core v0.1.0
webrtc v0.11.0" 0)"

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
exit 0
