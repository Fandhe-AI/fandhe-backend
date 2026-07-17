#!/usr/bin/env bash
# 対応可否自律判断ガードレールの判定記録バリデータ（scripts/feasibility-check.sh）の
# セルフテスト（TASK-12.3-2、#84）。docs/design/feasibility-guardrail.md 4 節の PoC-9
# T-11〜T-15 判定例に対応する fixture と、判定規約（3・5・6・7 節）の fail-closed 分岐を
# 網羅する異常系ケースを検証する。run-triage-tests.sh と同じくネットワーク・cargo ビルド
# に依存せず、ci.yml の unsafe-triage ジョブから軽量に呼ばれる想定（.claude/rules/
# pay-for-what-you-use.md・security.md と整合、追加ツール依存なし）。
#
# 各テストは独立した assert 関数で実行し、失敗があれば非 0 で終了する
# （フェイルクローズ、.claude/rules/security.md）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures"
CHECK="${SCRIPTS_DIR}/feasibility-check.sh"

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

WORKDIR="$(mktemp -d)"
trap 'rm -rf "${WORKDIR}"' EXIT

# ==================================================
# 正常系: PoC-9 T-11〜T-15 判定例（不可・要エスカレーション / 不可（明確な拒否））
# ==================================================

echo "===== T-11（曖昧要求）: 不可・要エスカレーション、exit 0 ====="
set +e
out_t11="$(bash "${CHECK}" --input "${FIXTURES_DIR}/feasibility-t11-ambiguous.md" 2>&1)"
exit_t11=$?
set -e
assert_exit_code "T-11 fixture は規約適合（exit 0）" 0 "${exit_t11}"
assert_contains "T-11 の判定区分を報告する" "${out_t11}" "不可・要エスカレーション"

echo "===== T-12/T-13（未定義依存）: 不可・要エスカレーション、exit 0 ====="
set +e
out_t12="$(bash "${CHECK}" --input "${FIXTURES_DIR}/feasibility-t12-t13-undefined-dependency.md" 2>&1)"
exit_t12=$?
set -e
assert_exit_code "T-12/T-13 fixture は規約適合（exit 0）" 0 "${exit_t12}"

echo "===== T-14（安全性方針との衝突）: 不可・要エスカレーション、exit 0 ====="
set +e
out_t14="$(bash "${CHECK}" --input "${FIXTURES_DIR}/feasibility-t14-safety-conflict.md" 2>&1)"
exit_t14=$?
set -e
assert_exit_code "T-14 fixture は規約適合（exit 0）" 0 "${exit_t14}"

echo "===== T-15（明確な脆弱性を招く要求）: 不可（明確な拒否）、exit 0 ====="
set +e
out_t15="$(bash "${CHECK}" --input "${FIXTURES_DIR}/feasibility-t15-clear-vulnerability.md" 2>&1)"
exit_t15=$?
set -e
assert_exit_code "T-15 fixture は規約適合（exit 0）" 0 "${exit_t15}"
assert_contains "T-15 の判定区分を報告する" "${out_t15}" "不可（明確な拒否）"
# 8・10 節: fixture 自体に実行可能な攻撃コード・エクスプロイト手順を含めない回帰検知。
t15_content="$(cat "${FIXTURES_DIR}/feasibility-t15-clear-vulnerability.md")"
if printf '%s' "${t15_content}" | grep -qE '\$\(|`[^`]*`|sh -c|/bin/sh|system\('; then
    fail "T-15 fixture に実行可能なコード片らしき記述が含まれています（規約 8・10 節違反の疑い）"
else
    pass "T-15 fixture は説明のみで実行可能な攻撃コードを含まない"
fi

# ==================================================
# 正常系: 可 / 条件付き可（承認あり）
# ==================================================

echo "===== 正常系（可）: exit 0 ====="
set +e
out_ok="$(bash "${CHECK}" --input "${FIXTURES_DIR}/feasibility-ok.md" 2>&1)"
exit_ok=$?
set -e
assert_exit_code "可 fixture は規約適合（exit 0）" 0 "${exit_ok}"

echo "===== 正常系（条件付き可・承認済み）: exit 0 ====="
set +e
out_cond="$(bash "${CHECK}" --input "${FIXTURES_DIR}/feasibility-conditional-approved.md" 2>&1)"
exit_cond=$?
set -e
assert_exit_code "条件付き可（承認済み）fixture は規約適合（exit 0）" 0 "${exit_cond}"

# ==================================================
# 異常系: --template の出力そのものは検証を通らない
# ==================================================

echo "===== --template の出力は未記入のため exit 1 ====="
TEMPLATE_FILE="${WORKDIR}/template.md"
bash "${CHECK}" --template > "${TEMPLATE_FILE}"
set +e
out_template="$(bash "${CHECK}" --input "${TEMPLATE_FILE}" 2>&1)"
exit_template=$?
set -e
assert_exit_code "未記入テンプレートは exit 1" 1 "${exit_template}"
assert_contains "未記入プレースホルダの違反を報告する" "${out_template}" "判定区分"

# --template の見出しを埋めれば通ることを確認する（テンプレート = 検証ロジックの単一
# ソース性の回帰検知）。「可」判定に必要な項目（判定区分・3 軸判定結果の 3 小見出し）
# のプレースホルダ行のみを具体的な記述に置換する。
FILLED_FILE="${WORKDIR}/filled.md"
sed -e 's/^<可 \/ 条件付き可 \/ 不可・要エスカレーション \/ 不可（明確な拒否） のいずれか一語のみを記入>$/可/' \
    -e 's/^<可 のみ必須。検証可能な受け入れ基準に落ちるかの判定結果・根拠>$/受け入れ基準はテストPASSに落ちる。/' \
    -e 's/^<可 のみ必須。既存の安全性方針・OWASP Top 10 との整合の判定結果・根拠>$/security.md と整合する。/' \
    -e 's/^<可 のみ必須。クレート・feature・利用者への影響が特定・限定できるかの判定結果・根拠>$/scripts\/ のみに限定される。/' \
    "${TEMPLATE_FILE}" > "${FILLED_FILE}"
set +e
out_filled="$(bash "${CHECK}" --input "${FILLED_FILE}" 2>&1)"
exit_filled=$?
set -e
assert_exit_code "テンプレートの必須項目を埋めると exit 0" 0 "${exit_filled}"

# ==================================================
# 異常系: 判定区分の欠落・未知の値
# ==================================================

echo "===== 判定区分が欠落: exit 1 ====="
MISSING_JUDGMENT="${WORKDIR}/missing-judgment.md"
cat > "${MISSING_JUDGMENT}" <<'EOF'
# 判定記録

## 該当カテゴリと判断根拠

なんらかの理由。
EOF
set +e
out_missing="$(bash "${CHECK}" --input "${MISSING_JUDGMENT}" 2>&1)"
exit_missing=$?
set -e
assert_exit_code "判定区分欠落は exit 1" 1 "${exit_missing}"

echo "===== 判定区分が未知の値: exit 1 ====="
UNKNOWN_JUDGMENT="${WORKDIR}/unknown-judgment.md"
cat > "${UNKNOWN_JUDGMENT}" <<'EOF'
## 判定区分

たぶん可
EOF
set +e
out_unknown="$(bash "${CHECK}" --input "${UNKNOWN_JUDGMENT}" 2>&1)"
exit_unknown=$?
set -e
assert_exit_code "未知の判定区分は exit 1" 1 "${exit_unknown}"
assert_contains "未知の値であることを報告する" "${out_unknown}" "未知の値"

# ==================================================
# 異常系: 不可区分での必須項目欠落
# ==================================================

echo "===== 不可・要エスカレーションで判断根拠が欠落: exit 1 ====="
ESCALATE_MISSING="${WORKDIR}/escalate-missing.md"
cat > "${ESCALATE_MISSING}" <<'EOF'
## 判定区分

不可・要エスカレーション

## 要人間判断事項

数値目標の提示を求める。

## 代替案

なし
EOF
set +e
out_esc_missing="$(bash "${CHECK}" --input "${ESCALATE_MISSING}" 2>&1)"
exit_esc_missing=$?
set -e
assert_exit_code "該当カテゴリと判断根拠の欠落は exit 1" 1 "${exit_esc_missing}"
assert_contains "該当カテゴリと判断根拠の欠落を報告する" "${out_esc_missing}" "該当カテゴリと判断根拠"

# ==================================================
# 異常系: 条件付き可での着手条件欠落・未承認
# ==================================================

echo "===== 条件付き可で着手条件が欠落: exit 1 ====="
COND_MISSING_CONDITION="${WORKDIR}/cond-missing-condition.md"
cat > "${COND_MISSING_CONDITION}" <<'EOF'
## 判定区分

条件付き可

## ユーザー承認

承認済み
EOF
set +e
out_cond_missing="$(bash "${CHECK}" --input "${COND_MISSING_CONDITION}" 2>&1)"
exit_cond_missing=$?
set -e
assert_exit_code "着手条件の欠落は exit 1" 1 "${exit_cond_missing}"

echo "===== 条件付き可で未承認: exit 1 ====="
COND_UNAPPROVED="${WORKDIR}/cond-unapproved.md"
cat > "${COND_UNAPPROVED}" <<'EOF'
## 判定区分

条件付き可

## 着手条件

受け入れ基準を明確化する。

## ユーザー承認

未承認
EOF
set +e
out_cond_unapproved="$(bash "${CHECK}" --input "${COND_UNAPPROVED}" 2>&1)"
exit_cond_unapproved=$?
set -e
assert_exit_code "ユーザー承認が『未承認』のままは exit 1" 1 "${exit_cond_unapproved}"
assert_contains "未承認のまま着手可と読める記録を拒否する" "${out_cond_unapproved}" "承認済み"

# ==================================================
# 回帰: is_unfilled の fail-closed 検知（PR #121 Bugbot 指摘 #1）
# ==================================================

echo "===== 判定区分が『<...>』プレースホルダの末尾に余分な文字を含む場合も未記入扱い: exit 1 ====="
TRAILING_PLACEHOLDER="${WORKDIR}/trailing-placeholder.md"
cat > "${TRAILING_PLACEHOLDER}" <<'EOF'
## 判定区分

<可 / 条件付き可 / 不可・要エスカレーション / 不可（明確な拒否） のいずれか一語のみを記入> 追記
EOF
set +e
out_trailing="$(bash "${CHECK}" --input "${TRAILING_PLACEHOLDER}" 2>&1)"
exit_trailing=$?
set -e
assert_exit_code "『<...>』の後に余分な文字が続くプレースホルダも未記入として拒否する" 1 "${exit_trailing}"

# ==================================================
# 回帰: extract_subsection は親セクション『## 3 軸判定結果』配下に限定する（PR #121 Bugbot 指摘 #2）
# ==================================================

echo "===== 可判定で『## 3 軸判定結果』自体が欠落しているが、無関係な場所に同名『###』見出しがある場合: exit 1 ====="
MISSING_PARENT_SECTION="${WORKDIR}/missing-parent-section.md"
cat > "${MISSING_PARENT_SECTION}" <<'EOF'
## 判定区分

可

## 無関係な節

### 実施可能か

本来のセクション外にある見出し。これで通過してはならない。

### 安全か

同上。

### 影響範囲が許容内か

同上。
EOF
set +e
out_missing_parent="$(bash "${CHECK}" --input "${MISSING_PARENT_SECTION}" 2>&1)"
exit_missing_parent=$?
set -e
assert_exit_code "『## 3 軸判定結果』欠落時は無関係な場所の同名『###』見出しを誤検知しない" 1 "${exit_missing_parent}"
assert_contains "3 軸判定結果の欠落を報告する" "${out_missing_parent}" "3 軸判定結果"

# ==================================================
# 引数エラー
# ==================================================

echo "===== 引数なし: exit 2 ====="
set +e
out_noargs="$(bash "${CHECK}" 2>&1)"
exit_noargs=$?
set -e
assert_exit_code "引数なしは exit 2（usage error）" 2 "${exit_noargs}"

echo "===== --input に存在しないファイル: exit 2 ====="
set +e
out_notfound="$(bash "${CHECK}" --input "${WORKDIR}/does-not-exist.md" 2>&1)"
exit_notfound=$?
set -e
assert_exit_code "存在しないファイル指定は exit 2" 2 "${exit_notfound}"

echo
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -ne 0 ]; then
    exit 1
fi
exit 0
