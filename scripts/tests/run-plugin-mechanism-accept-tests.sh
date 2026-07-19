#!/usr/bin/env bash
# scripts/accept/lib/plugin-mechanism-conclusion-verdict.awk のオフライン・セルフテスト
# （TASK-260 / #260 Bugbot 指摘対応）。
#
# `scripts/accept/plugin-mechanism-accept.sh` 基準 5 は本 awk ロジックで
# `benches/reports/task-2.4-plugin-accept.md` の「## 結論」セクション内の総合判定行を
# 判定材料に使う。本体スクリプトは cargo build/test（基準 1〜4）の副作用を持つため
# 直接実行できず、awk ロジックのみを cargo・ネットワーク非依存のフィクスチャで
# 回帰検証する（`scripts/tests/run-graphql-accept-tests.sh` と同じ方針）。
#
# 呼び出し元: 人間 / CI が `bash scripts/tests/run-plugin-mechanism-accept-tests.sh` として
# 直接実行する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AWK_FILE="${SCRIPT_DIR}/../accept/lib/plugin-mechanism-conclusion-verdict.awk"

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

run_verdict() {
    awk -f "${AWK_FILE}" "$1"
}

echo "===== ケース a: トップレベル結論を FAIL に変更しても他セクションへの引用 PASS を無視する ====="
fixture_a="$(mktemp)"
cat >"${fixture_a}" <<'EOF'
# レポート

## 結論

**総合判定: FAIL**

## 判定根拠 1: 過去実測（主根拠）

過去の実測は次の通り引用する:

**総合判定: PASS**（終了コード 0）。全指標が閾値を満たした。
EOF
assert_eq "トップレベル FAIL・引用セクションの PASS は無視" "FAIL" "$(run_verdict "${fixture_a}")"
rm -f "${fixture_a}"

echo "===== ケース b: 単一の「## 結論」セクションが PASS のみなら PASS ====="
fixture_b="$(mktemp)"
cat >"${fixture_b}" <<'EOF'
# レポート

## 結論

**総合判定: PASS**

## 判定根拠 1

詳細本文。
EOF
assert_eq "単一結論 PASS" "PASS" "$(run_verdict "${fixture_b}")"
rm -f "${fixture_b}"

echo "===== ケース c: 「## 結論」セクションが複数存在する場合、末尾のセクションを採用する（再計測反映） ====="
fixture_c="$(mktemp)"
cat >"${fixture_c}" <<'EOF'
# レポート

## 結論

**総合判定: FAIL**

## 判定根拠 1

引用: **総合判定: PASS**（無視されるべき）

## 結論（自動記録: bench-accept.sh 再計測、2026-07-19T00:00:00Z）

**総合判定: PASS**
EOF
assert_eq "末尾の「## 結論（自動記録）」セクションが直近の再計測結果として採用される" "PASS" "$(run_verdict "${fixture_c}")"
rm -f "${fixture_c}"

echo "===== ケース d: 「## 結論」セクションに総合判定の記録がなければ空文字（呼び出し元は SKIP 扱い） ====="
fixture_d="$(mktemp)"
cat >"${fixture_d}" <<'EOF'
# レポート

## 結論

BLOCKED のため判定なし。

## 判定根拠 1

詳細本文。
EOF
assert_eq "総合判定の記録なしは空文字" "" "$(run_verdict "${fixture_d}")"
rm -f "${fixture_d}"

echo "===== ケース e: 同一「## 結論」セクション内に PASS・FAIL が両方現れる異常系は FAIL を優先する ====="
fixture_e="$(mktemp)"
cat >"${fixture_e}" <<'EOF'
# レポート

## 結論

**総合判定: PASS**

**総合判定: FAIL**
EOF
assert_eq "同一セクション内 PASS→FAIL の順でも FAIL を優先" "FAIL" "$(run_verdict "${fixture_e}")"
rm -f "${fixture_e}"

fixture_e2="$(mktemp)"
cat >"${fixture_e2}" <<'EOF'
# レポート

## 結論

**総合判定: FAIL**

**総合判定: PASS**
EOF
assert_eq "同一セクション内 FAIL→PASS の順でも FAIL を優先" "FAIL" "$(run_verdict "${fixture_e2}")"
rm -f "${fixture_e2}"

echo ""
echo "===== サマリー ====="
echo "PASS: ${PASS_COUNT} / FAIL: ${FAIL_COUNT}"
if [ "${FAIL_COUNT}" -ne 0 ]; then
    exit 1
fi
exit 0
