#!/usr/bin/env bash
# CI 完遂判定基準の強制（TASK-14.1、#39、docs/spec/04-requirements.md REQ-14）と
# レビューゲートの土台（TASK-14.3、#41、REQ-14）を main ブランチの repository ruleset に
# 設定する冪等スクリプト。
#
# 設定するルールは 4 種:
# - required_status_checks: .github/workflows/ci.yml の集約ゲートジョブ `ci-complete`
#   （TASK-14.1）
# - pull_request: main への直 push を禁止し PR 経由を機械強制する（TASK-14.3）。
#   required_approving_review_count は 0（既存の AI レビュー運用を壊さない安全側の値）。
#   1 以上への引き上げは人間管理者が判断するダイヤルとして
#   docs/design/review-gate.md に明記する
# - non_fast_forward: main への force push を禁止する（TASK-14.3）
# - deletion: main ブランチの削除を禁止する（TASK-14.3）
#
# 詳細（レビューゲートの運用定義・人間判断ダイヤル）は docs/design/review-gate.md を参照。
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
# ruleset 定義（required_status_checks + pull_request + non_fast_forward + deletion）。
#
# - strict_required_status_checks_policy: false は「マージ前に対象ブランチへの
#   追従再実行を必須にしない」設定。true 化（ブランチ追従の強制）は
#   implement-issue-tree の並列実装（CI 1 回起動運用）への影響を要検討のため、
#   人間管理者が判断するダイヤルとして docs/design/review-gate.md に明記する
#   （本スクリプトでは変更しない）。
# - pull_request.required_approving_review_count: 0 は「PR 経由を必須化しつつ、
#   単独メンテナ + AI レビュー運用（push 前 review）を壊さない」安全側の値。
#   1 以上への引き上げも同様に人間管理者のダイヤルとする。
# - bypass_actors は明示的に空配列を送る（= 例外なし、fail-closed）。
#   GitHub の ruleset update（PUT）API はフィールド省略時に既存の bypass_actors を
#   クリアする保証がない（省略 = 変更なし、と解釈されうる）。冪等なスクリプトの
#   再実行で既存 bypass 例外が残留するのを防ぐため、必ずフィールドを明示する
#   （PR #117 レビュー指摘）。
# --------------------------------------------------
RULESET_PAYLOAD="$(jq -n \
    --arg name "${RULESET_NAME}" \
    --arg branch "${DEFAULT_BRANCH}" \
    --arg check "${REQUIRED_CHECK_NAME}" \
    '{
        name: $name,
        target: "branch",
        enforcement: "active",
        bypass_actors: [],
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
            },
            {
                type: "pull_request",
                parameters: {
                    required_approving_review_count: 0,
                    dismiss_stale_reviews_on_push: false,
                    require_code_owner_review: false,
                    require_last_push_approval: false,
                    required_review_thread_resolution: false
                }
            },
            {
                type: "non_fast_forward"
            },
            {
                type: "deletion"
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

echo "==> setup-required-checks.sh: '${DEFAULT_BRANCH}' に required status check '${REQUIRED_CHECK_NAME}' + PR 必須化 + force push/削除禁止を設定しました"
