#!/usr/bin/env bash
# regression-injection-verify.sh（#238、NFR-8 注入リグレッション検知ハーネス）の
# セルフテスト。
#
# `--gate-cmd` にスタブ（`fixtures/regression-injection/stub-gate.sh`）を注入して
# cargo・実クレートビルドに非依存で判定ロジック（検知率集計・閾値判定・タイムアウト
# 扱い・パッチ適用失敗のフェイルクローズ）を検証する（third-party-verify.sh 系の
# セルフテスト方針・`run-ai-autonomy-accept-tests.sh` の隔離フィクスチャ方式を踏襲）。
#
# 実際の cargo ゲートを使った実測（本番計測）は
# `bash scripts/regression-injection-verify.sh` を直接実行して行う。本スクリプトの
# green はあくまで「集計ロジックが正しい」ことの保証であり、NFR-8 の実測検知率とは
# 別物である（advisor 指摘: 自己テストの green を実測の代わりにしない）。
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
HARNESS="${SCRIPTS_DIR}/regression-injection-verify.sh"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/regression-injection"
STUB_GATE="${FIXTURES_DIR}/stub-gate.sh"

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

# haystack に needle が固定文字列として含まれるかを判定する（#511/#514: パイプ経由の
# grep -q 判定は set -euo pipefail 下で SIGPIPE/EPIPE により誤 FAIL・誤 pass を招くため
# bash 組み込みパターンマッチを使う。needle は必ずダブルクォートで囲み glob メタ文字を
# 文字どおりに扱わせる）。
assert_contains() {
    local desc="$1" haystack="$2" needle="$3"
    if [[ "${haystack}" == *"${needle}"* ]]; then
        pass "${desc}"
    else
        fail "${desc}（\"${needle}\" が出力に含まれません）"
    fi
}

# 共有 CARGO_TARGET_DIR 引数はセルフテストでは未使用（cargo を呼ばないため）だが、
# 未指定時に harness が `mktemp -d` で自動生成したディレクトリを毎回残さないよう、
# セルフテスト側でも明示的に指定して掃除する。
TARGET_DIR_STUB="$(mktemp -d)"
trap 'rm -rf "${TARGET_DIR_STUB}"' EXIT

run_harness() {
    # 呼び出し元がすでに `set -e` の外側にいる前提で、戻り値は呼び出し元が拾う。
    bash "${HARNESS}" --patches-dir "$1" --gate-cmd "${STUB_GATE}" --target-dir "${TARGET_DIR_STUB}"
}

# ケース 1: 全 12 件検知 → 検知率 100% で exit 0、metric 行が pass=12 total=12
out1="$(run_harness "${FIXTURES_DIR}/patches-ok" 2>&1)"
rc1=$?
if [ "${rc1}" -eq 0 ]; then
    pass "全件検知（12/12）は exit 0"
else
    fail "全件検知（12/12）が非 0 終了しました（rc=${rc1}）: ${out1}"
fi
assert_contains "全件検知時の metric 行" "${out1}" "metric=injection_detection_rate pass=12 fail=0 pending=0 total=12"

# ケース 2: 1 件検知漏れ（11/12 ≒ 91%）→ 90% 以上のため exit 0
out2="$(STUB_MISS_IDS="R-05" run_harness "${FIXTURES_DIR}/patches-one-missed" 2>&1)"
rc2=$?
if [ "${rc2}" -eq 0 ]; then
    pass "1 件検知漏れ（11/12=91%）は exit 0（閾値 90% 以上）"
else
    fail "1 件検知漏れが非 0 終了しました（rc=${rc2}）: ${out2}"
fi
assert_contains "1 件検知漏れの metric 行" "${out2}" "metric=injection_detection_rate pass=11 fail=1 pending=0 total=12"

# ケース 3: 2 件検知漏れ（10/12 ≒ 83%）→ 90% 未満のため非 0
out3="$(STUB_MISS_IDS="R-05 R-09" run_harness "${FIXTURES_DIR}/patches-two-missed" 2>&1)"
rc3=$?
if [ "${rc3}" -ne 0 ]; then
    pass "2 件検知漏れ（10/12=83%）は非 0 終了（閾値未達）"
else
    fail "2 件検知漏れが exit 0 になりました（フェイルクローズ違反）"
fi
assert_contains "2 件検知漏れの metric 行" "${out3}" "metric=injection_detection_rate pass=10 fail=2 pending=0 total=12"

# ケース 4: タイムアウトは検知として扱われる（timeout 短縮のため小さな上限を注入）
out4="$(STUB_TIMEOUT_IDS="R-01" REGRESSION_INJECTION_TIMEOUT=2 run_harness "${FIXTURES_DIR}/patches-ok" 2>&1)"
rc4=$?
if [ "${rc4}" -eq 0 ]; then
    pass "タイムアウトケースを含んでも他全件検知なら exit 0"
else
    fail "タイムアウトケースを含む全件検知が非 0 終了しました（rc=${rc4}）: ${out4}"
fi
assert_contains "タイムアウトが timeout チャンネルとして記録される" "${out4}" "R-01: DETECTED（timeout）"

# ケース 5: パッチ適用不能ケースを含む場合はフェイルクローズ（検知率が閾値以上でも非 0）
out5="$(run_harness "${FIXTURES_DIR}/patches-bad-apply" 2>&1)"
rc5=$?
if [ "${rc5}" -ne 0 ]; then
    pass "パッチ適用不能ケースを含む場合は非 0 終了（フェイルクローズ）"
else
    fail "パッチ適用不能ケースを含むのに exit 0 になりました"
fi
assert_contains "パッチ適用不能ケースのエラーメッセージ" "${out5}" "パッチが適用できません"

# ケース 6: パッチディレクトリが存在しない場合は使用方法エラー（exit 2）
bash "${HARNESS}" --patches-dir "${FIXTURES_DIR}/does-not-exist" --gate-cmd "${STUB_GATE}" >/dev/null 2>&1
rc6=$?
if [ "${rc6}" -eq 2 ]; then
    pass "パッチディレクトリ不在は exit 2"
else
    fail "パッチディレクトリ不在の終了コードが想定外です（rc=${rc6}）"
fi

echo "---------------------------------------------------"
echo "PASS=${PASS_COUNT} FAIL=${FAIL_COUNT}"
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
exit 0
