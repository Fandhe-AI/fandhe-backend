#!/usr/bin/env bash
# CI 完遂判定基準の強制（TASK-14.1、#39、docs/spec/04-requirements.md REQ-14）:
# main ブランチの repository ruleset に、.github/workflows/ci.yml の集約ゲートジョブ
# `ci-complete` を required status check として設定する冪等スクリプト。
#
# このスクリプトが設定するのは required_status_checks ルールのみである。
# PR 必須化・人間承認必須・force push 禁止などの追加ルールは TASK-14.3（#41、担当: 人間）の
# スコープであり、本スクリプトでは意図的に扱わない
# （docs/design/ci-completion-criteria.md 参照）。
#
# 前提: `gh` の既存認証（`gh auth login` 済み）を利用する。トークンをファイル・ログへ
# 出力しない（.claude/rules/security.md）。リポジトリ管理者権限（admin:repo_hook 相当）が
# 無い場合 403 で失敗する。その場合は握りつぶさず、非 0 で終了して呼び出し側に伝える。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

# --------------------------------------------------
# 前提ツールの存在検査（自動インストールしない。security.md・dep-audit.sh の既存規約に準拠）
# --------------------------------------------------
check_command() {
    local cmd="$1"
    local install_hint="$2"
    if ! command -v "${cmd}" >/dev/null 2>&1; then
        echo "エラー: ${cmd} が見つかりません。次のコマンドで導入してください:" >&2
        echo "  ${install_hint}" >&2
        exit 1
    fi
}

check_command "gh" "https://cli.github.com/ の手順に従い GitHub CLI を導入してください"
check_command "jq" "OS のパッケージマネージャで jq を導入してください（例: apt install jq）"

# required check のコンテキスト名。.github/workflows/ci.yml の `ci-complete` ジョブの
# `name:` と完全一致させる必要がある（改名時は両方を同時に更新する）。
readonly REQUIRED_CHECK_NAME="ci-complete"
readonly RULESET_NAME="main-required-checks"

# --------------------------------------------------
# 対象リポジトリの owner/repo を gh の現在のコンテキストから取得する
# （リポジトリ名をハードコードしない。フォーク・テンプレート再利用時にも動作させるため）
# --------------------------------------------------
REPO_NWO="$(gh repo view --json nameWithOwner --jq '.nameWithOwner')"
echo "==> 対象リポジトリ: ${REPO_NWO}"

# default branch を動的に取得する（main 固定を避ける）
DEFAULT_BRANCH="$(gh repo view --json defaultBranchRef --jq '.defaultBranchRef.name')"
echo "==> default branch: ${DEFAULT_BRANCH}"

# --------------------------------------------------
# ruleset 定義（required_status_checks のみ）。
# strict_required_status_checks_policy: false は「マージ前に対象ブランチへの
# 追従再実行を必須にしない」設定。ブランチ追従の強制は TASK-14.3 のスコープとする。
# --------------------------------------------------
RULESET_PAYLOAD="$(jq -n \
    --arg name "${RULESET_NAME}" \
    --arg branch "${DEFAULT_BRANCH}" \
    --arg check "${REQUIRED_CHECK_NAME}" \
    '{
        name: $name,
        target: "branch",
        enforcement: "active",
        conditions: {
            ref_name: {
                include: ["refs/heads/" + $branch],
                exclude: []
            }
        },
        rules: [
            {
                type: "required_status_checks",
                parameters: {
                    strict_required_status_checks_policy: false,
                    required_status_checks: [
                        { context: $check }
                    ]
                }
            }
        ]
    }')"

# --------------------------------------------------
# 既存 ruleset の照合（冪等化: 同名があれば PUT で更新、無ければ POST で作成）
# --------------------------------------------------
# `gh api --jq` は jq 式 1 個のみを受け付け `--arg` 等の追加引数を渡せないため、
# `gh api` の JSON 出力を素の `jq` にパイプして `--arg` で変数注入する。
EXISTING_ID="$(gh api "repos/${REPO_NWO}/rulesets" | jq -r \
    --arg name "${RULESET_NAME}" '.[] | select(.name == $name) | .id' 2>/dev/null || true)"

if [ -n "${EXISTING_ID}" ]; then
    echo "==> 既存 ruleset '${RULESET_NAME}'（id=${EXISTING_ID}）を更新"
    echo "${RULESET_PAYLOAD}" | gh api \
        --method PUT \
        "repos/${REPO_NWO}/rulesets/${EXISTING_ID}" \
        --input - >/dev/null
else
    echo "==> ruleset '${RULESET_NAME}' を新規作成"
    echo "${RULESET_PAYLOAD}" | gh api \
        --method POST \
        "repos/${REPO_NWO}/rulesets" \
        --input - >/dev/null
fi

echo "==> setup-required-checks.sh: '${DEFAULT_BRANCH}' の required status check として '${REQUIRED_CHECK_NAME}' を設定しました"
