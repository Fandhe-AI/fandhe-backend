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
