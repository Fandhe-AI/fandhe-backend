#!/usr/bin/env bash
# audit 指摘トリアージ（TASK-12.1-1、#79、docs/spec/05-tasks.md）:
# `cargo audit --json` の出力を解析し、指摘を「自動更新提案」「要エスカレーション」
# 「情報（記録・監視）」の 3 区分に分類して markdown レポートを生成する。
#
# 位置づけ: TASK-15.2（#17）の scripts/dep-audit.sh は `cargo audit` を素通しで
# 実行するだけで、指摘が出ても「CI が赤くなる」以上の情報（次に何をすべきか）を
# 出力しない。本スクリプトはその不足を補い、dep-audit.sh から呼ばれる形で
# 「検知したら機械可読なトリアージが自動生成される」状態にする（TASK-12.1「AI が
# 能動的に改善案を提示する機構」の一部）。改善提案フロー・運用規約のドキュメント
# 化は #80（TASK-12.1-2）のスコープであり、本スクリプトは機構（ロジック）のみを
# 担う。
#
# 区分ロジック:
#   - vulnerabilities.list[] のうち versions.patched が非空          → 自動更新提案
#   - vulnerabilities.list[] のうち versions.patched が空（未修正）  → 要エスカレーション
#   - warnings.*（unmaintained / unsound / yanked / notice）         → 情報（記録・監視）
#     （cargo audit の既定どおり、warnings のみでは CI を失敗させない安全側の判断）
#
# 終了コード:
#   0 = vulnerability なし（warnings のみ、または完全にクリーン）
#   1 = vulnerability あり（フェイルクローズ、.claude/rules/security.md）
#   2 = 前提ツール・引数エラー
#
# セキュリティ（OWASP A03 インジェクション対策）: advisory の id/title/description 等は
# 外部の advisory DB に由来する信頼できない文字列として扱う。すべて jq でエスケープ済みの
# 値として取り出し、`eval` やシェルの再解釈（コマンド置換・`sh -c` 埋め込み）に一切渡さない。
# printf '%s\n' で出力するに留め、レポート本文への埋め込みも markdown の素の文字列として
# 扱う（コマンドとして実行されない）。
#
# 改善提案フローとの整合（#226、docs/design/improvement-proposal-flow.md 4 節）:
# 改善提案の必須 5 項目（背景・根拠データ／影響範囲／対応方針／検証方法／リスク）のうち、
# 従来は影響範囲（crate 表）と対応方針（推奨アクション）しか出力しておらず、CI が
# `--body-file` でそのまま Issue 起票する際に「検証方法」「リスク」欄が欠落していた
# （#218 D-2 の人手評価で不当判定の一因）。本スクリプトは 3 区分それぞれの非空分岐に
# 検証方法・リスクの定型文を追加し、5 項目を機械的に揃える。区分ロジック・終了コード・
# `--vuln-ids-output` の出力内容は変更しない（互換性維持）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

INPUT_JSON=""
OUTPUT_FILE=""
VULN_IDS_OUTPUT_FILE=""

usage() {
    cat <<'EOF'
使い方: audit-triage.sh [--input <cargo-audit-json>] [--output <report.md>]
                        [--vuln-ids-output <ids.txt>]

  --input <path>            cargo audit --json の出力ファイルを指定する（テスト用
                             フィクスチャ注入口）。未指定時はネットワーク接続のうえ
                             `cargo audit --json` を実行する。
  --output <path>           トリアージレポート（markdown）の出力先。指定時もレポートは
                             常に標準出力へ要約される。
  --vuln-ids-output <path>  vulnerability（自動更新提案・要エスカレーション区分）の
                             advisory ID のみを改行区切りで書き出す（vulnerability
                             なしなら空ファイル）。warnings（情報・記録のみの区分）の
                             advisory ID は含めない。CI の Issue 起票ステップ等が
                             markdown レポート全体を正規表現で走査すると warnings の
                             advisory ID まで拾ってしまうため、機械可読な区別が必要な
                             呼び出し元はこちらを使うこと。
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --input)
            INPUT_JSON="${2:-}"
            if [ -z "${INPUT_JSON}" ]; then
                echo "エラー: --input には値が必要です" >&2
                exit 2
            fi
            shift 2
            ;;
        --output)
            OUTPUT_FILE="${2:-}"
            if [ -z "${OUTPUT_FILE}" ]; then
                echo "エラー: --output には値が必要です" >&2
                exit 2
            fi
            shift 2
            ;;
        --vuln-ids-output)
            VULN_IDS_OUTPUT_FILE="${2:-}"
            if [ -z "${VULN_IDS_OUTPUT_FILE}" ]; then
                echo "エラー: --vuln-ids-output には値が必要です" >&2
                exit 2
            fi
            shift 2
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

# --------------------------------------------------
# 前提ツールの存在検査（自動インストールしない。dep-audit.sh と同じ方針）
# --------------------------------------------------
check_command() {
    local cmd="$1"
    local install_hint="$2"
    if ! command -v "${cmd}" >/dev/null 2>&1; then
        echo "エラー: ${cmd} が見つかりません。次のコマンドで導入してください:" >&2
        echo "  ${install_hint}" >&2
        exit 2
    fi
}

check_command "jq" "OS のパッケージマネージャで jq を導入してください（例: apt install jq）"

WORKDIR="$(mktemp -d)"
trap 'rm -rf "${WORKDIR}"' EXIT

if [ -n "${INPUT_JSON}" ]; then
    if [ ! -f "${INPUT_JSON}" ]; then
        echo "エラー: --input で指定されたファイルが見つかりません: ${INPUT_JSON}" >&2
        exit 2
    fi
    AUDIT_JSON="${INPUT_JSON}"
else
    check_command "cargo-audit" "cargo install --locked cargo-audit@0.22.2"

    # Cargo.lock は .gitignore 対象（コミットしない運用、dep-audit.sh と同じ理由）。
    # 監査直前に無ければ生成する。
    if [ ! -f Cargo.lock ]; then
        echo "==> Cargo.lock を生成" >&2
        cargo generate-lockfile
    fi

    AUDIT_JSON="${WORKDIR}/cargo-audit.json"
    echo "==> cargo audit --json" >&2
    # cargo audit は脆弱性検知時に非 0 で終了するため、ここでは終了コードを握りつぶし
    # JSON 出力のみを後段のトリアージ判定に使う（フェイルクローズの最終判定は
    # 本スクリプト自身の終了コードで行う）。
    cargo audit --json > "${AUDIT_JSON}" || true
fi

if ! jq empty "${AUDIT_JSON}" >/dev/null 2>&1; then
    echo "エラー: cargo audit の出力が JSON として解析できません（前段の実行に失敗した可能性があります）: ${AUDIT_JSON}" >&2
    exit 2
fi

VULN_COUNT="$(jq -r '.vulnerabilities.count // 0' "${AUDIT_JSON}")"

# --------------------------------------------------
# レポート本体を組み立てる（markdown）
# --------------------------------------------------
REPORT_FILE="${WORKDIR}/report.md"

{
    echo "# audit 指摘トリアージレポート"
    echo
    echo "生成日時: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    echo
    # 改善提案フロー（docs/design/improvement-proposal-flow.md 4 節）の必須項目
    # 「背景・根拠データ」を明示する。一次データは cargo audit --json の生出力そのもの。
    if [ -n "${INPUT_JSON}" ]; then
        printf '背景・根拠データ: cargo audit --json の出力（%s）を一次データとして自動生成\n' "${INPUT_JSON}"
    else
        echo "背景・根拠データ: 本リポジトリで実行した cargo audit --json の出力を一次データとして自動生成"
    fi
    echo

    echo "## 概要"
    echo
    echo "| 区分 | 件数 |"
    echo "|---|---|"
    printf '| 自動更新提案 | %s |\n' "$(jq '[.vulnerabilities.list[]? | select((.versions.patched // []) | length > 0)] | length' "${AUDIT_JSON}")"
    printf '| 要エスカレーション | %s |\n' "$(jq '[.vulnerabilities.list[]? | select((.versions.patched // []) | length == 0)] | length' "${AUDIT_JSON}")"
    printf '| 情報（記録・監視） | %s |\n' "$(jq '[.warnings // {} | to_entries[] | .value[]?] | length' "${AUDIT_JSON}")"
    echo

    echo "## 自動更新提案（patched バージョンあり）"
    echo
    AUTO_UPDATE_LIST="$(jq -r '
        .vulnerabilities.list[]?
        | select((.versions.patched // []) | length > 0)
        | [.advisory.id, .package.name, .package.version, (.versions.patched | join(", ")), .advisory.title]
        | @tsv
    ' "${AUDIT_JSON}")"
    if [ -z "${AUTO_UPDATE_LIST}" ]; then
        echo "該当なし"
    else
        echo "| advisory ID | crate | 現バージョン | 修正版 | 概要 |"
        echo "|---|---|---|---|---|"
        while IFS=$'\t' read -r id name version patched title; do
            [ -z "${id}" ] && continue
            printf '| %s | %s | %s | %s | %s |\n' "${id}" "${name}" "${version}" "${patched}" "${title}"
        done <<< "${AUTO_UPDATE_LIST}"
        echo
        echo "推奨アクション: 各 crate を修正版へ更新する。例:"
        echo
        echo '```bash'
        while IFS=$'\t' read -r id name version patched title; do
            [ -z "${name}" ] && continue
            printf 'cargo update -p %s\n' "${name}"
        done <<< "${AUTO_UPDATE_LIST}"
        echo '```'
        echo
        # advisory ID 一覧は jq で取り出した信頼できない文字列を printf '%s' の引数として
        # のみ埋め込む（コマンド置換・eval への再解釈は行わない、OWASP A03 対策）。
        AUTO_UPDATE_IDS="$(jq -r '
            [.vulnerabilities.list[]? | select((.versions.patched // []) | length > 0) | .advisory.id]
            | join(", ")
        ' "${AUDIT_JSON}")"
        echo "検証方法:"
        echo '- 上記の `cargo update -p <crate>` を適用後、`bash scripts/dep-audit.sh` を再実行し全 feature 構成で当該指摘が解消されることを確認する'
        echo "- CI \`dep-audit\` ジョブ（.github/workflows/ci.yml）の通過を確認する"
        echo
        echo "リスク:"
        printf -- '- 対応しない場合: 既知の脆弱性（advisory ID: %s）が依存ツリーに残置される\n' "${AUTO_UPDATE_IDS}"
        echo "- 対応する場合: crate 更新に伴う API・挙動変化の可能性がある（CI 全ジョブ通過を条件に検証する）"
    fi
    echo

    echo "## 要エスカレーション（未修正 = patched バージョンなし）"
    echo
    ESCALATE_LIST="$(jq -r '
        .vulnerabilities.list[]?
        | select((.versions.patched // []) | length == 0)
        | [.advisory.id, .package.name, .package.version, .advisory.title]
        | @tsv
    ' "${AUDIT_JSON}")"
    if [ -z "${ESCALATE_LIST}" ]; then
        echo "該当なし"
    else
        echo "| advisory ID | crate | 現バージョン | 概要 |"
        echo "|---|---|---|---|"
        while IFS=$'\t' read -r id name version title; do
            [ -z "${id}" ] && continue
            printf '| %s | %s | %s | %s |\n' "${id}" "${name}" "${version}" "${title}"
        done <<< "${ESCALATE_LIST}"
        echo
        echo "推奨アクション: 修正版が存在しないため自動更新できない。次のいずれかをユーザーへエスカレーションする:"
        echo "- 代替 crate への切り替えを検討する"
        echo "- 影響がない・許容範囲と判断できる場合は \`deny.toml\` の advisories ignore に理由を明記して追加する（ユーザー承認必須）"
        echo
        ESCALATE_IDS="$(jq -r '
            [.vulnerabilities.list[]? | select((.versions.patched // []) | length == 0) | .advisory.id]
            | join(", ")
        ' "${AUDIT_JSON}")"
        echo "検証方法:"
        echo "- 代替 crate へ切り替えた場合: \`bash scripts/dep-audit.sh\` を再実行し当該指摘が解消されることを確認する"
        echo "- \`deny.toml\` ignore を追加した場合: 理由の明記とユーザー承認を確認のうえ \`bash scripts/dep-audit.sh\` の通過を確認する"
        echo
        echo "リスク:"
        printf -- '- 対応しない場合: 修正版が存在しない脆弱性（advisory ID: %s）が未対応のまま残置される\n' "${ESCALATE_IDS}"
        echo "- 対応する場合: 代替 crate への切り替えに伴う互換性リスク、または ignore 追加が恒久化し検知が形骸化するリスク"
    fi
    echo

    echo "## 情報（記録・監視。unmaintained / unsound / yanked / notice）"
    echo
    WARNING_LIST="$(jq -r '
        (.warnings // {})
        | to_entries[]
        | .key as $kind
        | .value[]?
        | [$kind, .package.name, .package.version, (.advisory.id // "-"), (.advisory.title // "-")]
        | @tsv
    ' "${AUDIT_JSON}")"
    if [ -z "${WARNING_LIST}" ]; then
        echo "該当なし"
    else
        echo "| 種別 | crate | バージョン | advisory ID | 概要 |"
        echo "|---|---|---|---|---|"
        while IFS=$'\t' read -r kind name version id title; do
            [ -z "${kind}" ] && continue
            printf '| %s | %s | %s | %s | %s |\n' "${kind}" "${name}" "${version}" "${id}" "${title}"
        done <<< "${WARNING_LIST}"
        echo
        echo "推奨アクション: CI は失敗させない（cargo audit 既定の安全側動作を踏襲）。定期的な監視・棚卸しの対象として記録する。"
        echo
        echo "検証方法:"
        echo "- 日次 schedule の CI \`dep-audit\` ジョブによる継続監視で状態変化（yanked 化・脆弱性化）を検知する"
        echo
        echo "リスク:"
        echo "- 対応しない場合: unmaintained / unsound 等の crate が将来の脆弱性の温床になりうる（現時点で CI は失敗させない）"
        echo "- 対応する場合: 追加コストは監視・棚卸しのみで小さい"
    fi
    echo
} > "${REPORT_FILE}"

cat "${REPORT_FILE}"

if [ -n "${OUTPUT_FILE}" ]; then
    cp "${REPORT_FILE}" "${OUTPUT_FILE}"
    echo "==> レポートを出力しました: ${OUTPUT_FILE}" >&2
fi

if [ -n "${VULN_IDS_OUTPUT_FILE}" ]; then
    # vulnerabilities.list[] のみを対象にする（warnings の advisory ID は含めない）。
    # 呼び出し元（CI の Issue 起票ステップ等）が markdown レポート全体を正規表現で
    # 走査すると、情報（記録・監視）区分の advisory ID まで vulnerability として
    # 誤検知しうるため、この専用出力で機械的に区別する。
    jq -r '.vulnerabilities.list[]?.advisory.id' "${AUDIT_JSON}" | sort -u > "${VULN_IDS_OUTPUT_FILE}"
fi

if [ "${VULN_COUNT}" -gt 0 ]; then
    echo "==> audit-triage.sh: ${VULN_COUNT} 件の vulnerability を検知しました（フェイルクローズ）" >&2
    exit 1
fi

echo "==> audit-triage.sh: vulnerability は検知されませんでした" >&2
exit 0
