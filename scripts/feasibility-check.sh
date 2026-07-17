#!/usr/bin/env bash
# 対応可否自律判断ガードレールの判定記録バリデータ（TASK-12.3-2、#84）:
# TASK-12.3-1（#83、docs/design/feasibility-guardrail.md）が定める判定規約の
# 「判定記録の形式・必須項目・fail-closed 原則」を機械検証する。規約の判定内容そのもの
# （どの要求をどう判定すべきか）の妥当性はレビューゲート（人間承認）が担い、本スクリプト
# は「判定記録が規約どおりの構造・必須項目を満たしているか」の**形式検証のみ**を行う
# （docs/design/feasibility-guardrail.md「機構組み込み（TASK-12.3-2）」節・
# docs/design/ci-completion-criteria.md と同じ責務分界）。
#
# `--template` はバリデータが検証する項目名と同一の見出しを持つテンプレートを標準出力
# する。テンプレートと検証ロジックを単一ソース化し、規約ドキュメントとのドリフトを防ぐ
# （項目名の定数は本ファイル冒頭にのみ存在する）。
#
# 検証内容（docs/design/feasibility-guardrail.md 3・5・6・7 節の機械化）:
#   - 「判定区分」欄が 4 値のいずれかに厳密一致する（欠落・未知の値・空欄・未記入
#     プレースホルダは fail-closed で exit 1）
#   - 「不可・要エスカレーション」「不可（明確な拒否）」は「該当カテゴリと判断根拠」
#     「要人間判断事項」「代替案」の全欄が記入済み（5 節）
#   - 「条件付き可」は「着手条件」欄が記入済み、かつ「ユーザー承認」欄が厳密に
#     「承認済み」であること（6 節。未承認のまま着手可と読める記録を fail-closed で拒否）
#   - 「可」は「3 軸判定結果」の 3 小見出し（実施可能か・安全か・影響範囲が許容内か）が
#     いずれも記入済み（7 節）
#
# 終了コード: 0 = 規約適合 / 1 = 規約違反（フェイルクローズ） / 2 = 引数・前提エラー
#
# セキュリティ（OWASP A03 インジェクション対策、.claude/rules/security.md）: 判定記録
# ファイルの内容は信頼できない入力として扱う。grep -F（固定文字列）・awk の完全一致行
# 比較のみで照合し、eval・コマンド置換・シェル再解釈には一切渡さない。
set -euo pipefail

# --------------------------------------------------
# 項目名の定数（テンプレート出力・検証ロジック双方がこの定数のみを参照する）
# --------------------------------------------------
readonly H_JUDGMENT="判定区分"
readonly H_CATEGORY="該当カテゴリと判断根拠"
readonly H_HUMAN_JUDGMENT="要人間判断事項"
readonly H_ALTERNATIVE="代替案"
readonly H_CONDITION="着手条件"
readonly H_APPROVAL="ユーザー承認"
readonly H_THREE_AXES="3 軸判定結果"
readonly SH_FEASIBLE="実施可能か"
readonly SH_SAFE="安全か"
readonly SH_IMPACT="影響範囲が許容内か"

readonly V_OK="可"
readonly V_CONDITIONAL="条件付き可"
readonly V_ESCALATE="不可・要エスカレーション"
readonly V_REJECT="不可（明確な拒否）"
readonly V_APPROVED="承認済み"

usage() {
    cat <<'EOF'
使い方: feasibility-check.sh --input <record.md>
       feasibility-check.sh --template

  --input <path>  判定記録（markdown）を検証する。docs/design/feasibility-guardrail.md
                   の判定規約（3・5・6・7 節）に照らした形式検証のみを行う（判定内容の
                   妥当性はレビューゲートが担う）。
  --template       規約準拠の判定記録テンプレートを標準出力する（そのままでは検証を
                   通らない。プレースホルダを埋めてから --input に渡すこと）。
EOF
}

emit_template() {
    cat <<EOF
# 対応可否判定記録

docs/design/feasibility-guardrail.md の判定規約に従う判定記録。
\`scripts/feasibility-check.sh --input <このファイル>\` で形式検証できる。

## ${H_JUDGMENT}

<${V_OK} / ${V_CONDITIONAL} / ${V_ESCALATE} / ${V_REJECT} のいずれか一語のみを記入>

## ${H_CATEGORY}

<不可 2 区分のみ必須。4 節のどのカテゴリに該当し、どの軸が不充足かを記述する。該当しない場合は「なし」>

## ${H_HUMAN_JUDGMENT}

<不可 2 区分のみ必須。人間に何を判断・提供してほしいかを記述する。なければ「なし」>

## ${H_ALTERNATIVE}

<不可 2 区分のみ必須。安全・実施可能な代替手段を記述する。なければ「なし」>

## ${H_CONDITION}

<${V_CONDITIONAL} のみ必須。着手前に補完すべき条件（受け入れ基準の明確化・影響範囲の限定等）を記述する>

## ${H_APPROVAL}

<${V_CONDITIONAL} のみ必須。ユーザー承認を得たら「${V_APPROVED}」と記入する。未承認の間は空欄のままにする>

## ${H_THREE_AXES}

### ${SH_FEASIBLE}

<${V_OK} のみ必須。検証可能な受け入れ基準に落ちるかの判定結果・根拠>

### ${SH_SAFE}

<${V_OK} のみ必須。既存の安全性方針・OWASP Top 10 との整合の判定結果・根拠>

### ${SH_IMPACT}

<${V_OK} のみ必須。クレート・feature・利用者への影響が特定・限定できるかの判定結果・根拠>
EOF
}

# 判定記録ファイルから "## <heading>" セクションの本文を抽出する。
# 次の "## " 見出し（または EOF）までを本文として扱う。完全一致比較のみを用い、
# 判定記録内の文字列を正規表現・シェルへ再解釈させない（OWASP A03 対策）。
extract_section() {
    local file="$1" heading="$2"
    awk -v h="## ${heading}" '
        $0 == h { found=1; next }
        found && index($0, "## ") == 1 { found=0 }
        found { print }
    ' "${file}"
}

# "## 3 軸判定結果" セクション内の "### <heading>" 小見出しの本文を抽出する。
# 親セクション（"## 3 軸判定結果"）の本文に限定してから小見出しを走査するため、
# ファイル中の他の場所にある同名 "###" 見出しや、親セクション自体が欠落している
# 場合を誤って一致させない（fail-closed。親セクション欠落時は空文字列を返し、
# is_unfilled が真になり違反として検知される）。
extract_subsection() {
    local file="$1" heading="$2"
    extract_section "${file}" "${H_THREE_AXES}" | awk -v h="### ${heading}" '
        $0 == h { found=1; next }
        found && index($0, "### ") == 1 { found=0 }
        found { print }
    '
}

# セクション本文の最初の非空行を返す（トリム済み）。プレースホルダ判定・空欄判定に使う。
first_nonblank_line() {
    printf '%s\n' "$1" | awk 'NF { sub(/^[ \t]+/, ""); sub(/[ \t]+$/, ""); print; exit }'
}

# 欄が「空欄」または「未記入プレースホルダ（<...> 形式）」なら真を返す（fail-closed 対象）。
# 正当な判定値（判定区分の 4 値・「承認済み」等）はいずれも "<" で始まらないため、
# 行が "<" で始まるかどうかのみで判定する（閉じ "\>" までの厳密一致は求めない）。
# 厳密一致（\<*\>）にすると、CRLF 由来の末尾 \r や余分な後続文字で行末が ">" と
# 一致しなくなった場合にプレースホルダを「記入済み」と誤判定してしまう
# （fail-closed の抜け穴になるため、より広く弾く形に倒す）。
is_unfilled() {
    local line="$1"
    if [ -z "${line}" ]; then
        return 0
    fi
    case "${line}" in
        \<*) return 0 ;;
    esac
    return 1
}

INPUT_FILE=""
MODE=""

while [ $# -gt 0 ]; do
    case "$1" in
        --input)
            INPUT_FILE="${2:-}"
            if [ -z "${INPUT_FILE}" ]; then
                echo "エラー: --input には値が必要です" >&2
                exit 2
            fi
            MODE="input"
            shift 2
            ;;
        --template)
            MODE="template"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "エラー: 未知の引数です: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [ -z "${MODE}" ]; then
    echo "エラー: --input または --template のいずれかを指定してください" >&2
    usage >&2
    exit 2
fi

if [ "${MODE}" = "template" ]; then
    emit_template
    exit 0
fi

if [ ! -f "${INPUT_FILE}" ]; then
    echo "エラー: --input で指定されたファイルが見つかりません: ${INPUT_FILE}" >&2
    exit 2
fi

VIOLATIONS=()

judgment_raw="$(extract_section "${INPUT_FILE}" "${H_JUDGMENT}")"
judgment="$(first_nonblank_line "${judgment_raw}")"

if is_unfilled "${judgment}"; then
    VIOLATIONS+=("『${H_JUDGMENT}』欄が空欄または未記入プレースホルダのままです")
elif [ "${judgment}" != "${V_OK}" ] && [ "${judgment}" != "${V_CONDITIONAL}" ] \
    && [ "${judgment}" != "${V_ESCALATE}" ] && [ "${judgment}" != "${V_REJECT}" ]; then
    VIOLATIONS+=("『${H_JUDGMENT}』欄が未知の値です（『${judgment}』。4 値のいずれかに厳密一致させてください）")
fi

check_required() {
    local heading="$1"
    local content
    content="$(first_nonblank_line "$(extract_section "${INPUT_FILE}" "${heading}")")"
    if is_unfilled "${content}"; then
        VIOLATIONS+=("『${heading}』欄が空欄または未記入プレースホルダのままです")
    fi
}

check_required_subsection() {
    local heading="$1"
    local content
    content="$(first_nonblank_line "$(extract_subsection "${INPUT_FILE}" "${heading}")")"
    if is_unfilled "${content}"; then
        VIOLATIONS+=("『${H_THREE_AXES}』の『${heading}』欄が空欄または未記入プレースホルダのままです")
    fi
}

case "${judgment}" in
    "${V_ESCALATE}"|"${V_REJECT}")
        # 5 節: 不可 2 区分は該当カテゴリと判断根拠・要人間判断事項・代替案が全件必須
        # （内容が「なし」でも欄自体は必須。プレースホルダのままは不可）。
        check_required "${H_CATEGORY}"
        check_required "${H_HUMAN_JUDGMENT}"
        check_required "${H_ALTERNATIVE}"
        ;;
    "${V_CONDITIONAL}")
        # 6 節: 条件付き可は着手条件が必須、かつユーザー承認が厳密に「承認済み」でない
        # 限り fail-closed（未承認のまま着手可と読める記録を拒否）。
        check_required "${H_CONDITION}"
        approval="$(first_nonblank_line "$(extract_section "${INPUT_FILE}" "${H_APPROVAL}")")"
        if [ "${approval}" != "${V_APPROVED}" ]; then
            VIOLATIONS+=("『${H_APPROVAL}』欄が『${V_APPROVED}』ではありません（未承認のまま着手可と読める記録は規約違反です）")
        fi
        ;;
    "${V_OK}")
        # 7 節: 可は 3 軸すべての判定結果欄が必須（判定を行ったこと自体の追跡可能性）。
        check_required_subsection "${SH_FEASIBLE}"
        check_required_subsection "${SH_SAFE}"
        check_required_subsection "${SH_IMPACT}"
        ;;
    *)
        # 判定区分自体が不正な場合は区分別の追加検証を行わない
        # （上の判定区分チェックで既に VIOLATIONS に積んでいる）。
        ;;
esac

if [ "${#VIOLATIONS[@]}" -eq 0 ]; then
    echo "==> feasibility-check.sh: 判定記録は規約（docs/design/feasibility-guardrail.md）に適合しています（判定区分: ${judgment}）"
    exit 0
fi

echo "==> feasibility-check.sh: 判定記録が規約に違反しています（フェイルクローズ）" >&2
for v in "${VIOLATIONS[@]}"; do
    echo "  - ${v}" >&2
done
exit 1
