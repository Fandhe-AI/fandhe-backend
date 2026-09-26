#!/usr/bin/env bash
# main ブランチの実際の保護は repository ruleset `main-protection`（イシュー #693 時点で
# id 20587666）で、このスクリプトが定義する内容と一致していなければならない。
#
# 背景（#693）: 本スクリプトはかつて存在しない ruleset `main-required-checks` を宛先にし、
# required status check を `ci-complete` の 1 件だけに絞っていた。しかし実際の保護は
# リポジトリの外（GitHub UI）で手作業により `main-protection` として運用されており、
# required status check は `ci-complete` を含む 25 件（matrix 化した fmt/clippy/test・
# codex 系ジョブ・Cursor Bugbot 等）、`required_review_thread_resolution: true`、
# squash マージのみ、`bypass_actors` 空という構成になっていた。#685（イシュー #679、
# fmt/clippy/test の matrix 化）でこの実態と乖離した状態のまま required check が更新され、
# ruleset は手作業で修正されたが、その変更はリポジトリに記録されていなかった。
#
# 本スクリプトは `main-protection` を正とし、その現行構成をそのまま定義として持つ。
# `--check` で live との差分を読み取り専用で検出し、apply（既定モード）は差分がある
# 場合のみ PUT する（冪等・不要な書き込みをしない）。
#
# ジョブ名を変えるときの運用（docs/design/review-gate.md §2.1 参照）:
#   1. ci.yml（または ai-review.yml）のジョブ名変更と、本スクリプトの
#      REQUIRED_CONTEXTS_TSV の変更を同じ PR で行う
#   2. 管理者が `--check` を実行し、旧名の削除・新名の追加だけが差分に出ることを確認する
#   3. マージ直前に管理者が apply する（旧名が required のままだと matrix 化等の PR が
#      マージできなくなる。#685 で実際に発生した事例）
#   4. マージ後に `--check` が exit 0 になることを確認する
#
# 前提: `gh` の既存認証（`gh auth login` 済み）を利用する。トークンをファイル・ログへ
# 出力しない（.claude/rules/security.md）。apply（既定モード）はリポジトリ管理者権限
# （admin:repo_hook 相当）が無い場合 403 で失敗する。その場合は握りつぶさず、非 0 で
# 終了して呼び出し側に伝える。`--check` / `--print-desired` は読み取りのみで完結する
# （`--print-desired` は gh すら呼ばない）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

MODE="apply"
case "${1:-}" in
    --check)
        MODE="check"
        ;;
    --print-desired)
        MODE="print-desired"
        ;;
    --help|-h)
        cat <<'EOF'
使い方:
  scripts/setup-required-checks.sh              main-protection ruleset を live へ適用する
                                                 （差分がある場合のみ PUT/POST、admin 権限要）
  scripts/setup-required-checks.sh --check      live との差分を表示するだけ（書き込みなし）
  scripts/setup-required-checks.sh --print-desired
                                                 定義（正規化 JSON）を標準出力する（gh 不要）
  scripts/setup-required-checks.sh --help       このヘルプを表示する

終了コード:
  0  一致している（apply なら適用済みか変更不要）
  1  差分あり、または ruleset が存在しない（--check）
  2  前提エラー（gh/jq 不在、認証失敗、API エラー、同名 ruleset の重複、不明な引数 等）
EOF
        exit 0
        ;;
    "")
        MODE="apply"
        ;;
    *)
        echo "エラー: 不明な引数 '${1}'（--help を参照）" >&2
        exit 2
        ;;
esac

# --------------------------------------------------
# 前提ツールの存在検査（自動インストールしない。security.md・dep-audit.sh の既存規約に準拠）
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
if [ "${MODE}" != "print-desired" ]; then
    check_command "gh" "https://cli.github.com/ の手順に従い GitHub CLI を導入してください"
fi

# 対象 ruleset。実態は default branch を `~DEFAULT_BRANCH`（GitHub のプレースホルダ構文、
# `refs/heads/main` のような具体値ではない）で参照する branch ruleset。
readonly RULESET_NAME="main-protection"
readonly TARGET_REF='~DEFAULT_BRANCH'

# --------------------------------------------------
# required status checks の定義（1 か所に集約）。
#
# `context<TAB>integration_id` の TSV。integration_id の由来:
#   15368    = GitHub Actions（このリポジトリの ci.yml / ai-review.yml の各ジョブ）
#   1210556  = Cursor Bugbot（外部 App。App の差し替えで integration_id が変わりうる。
#              その場合は --check が差分として検出するのが正しい挙動）
#
# 各 context の対応（ジョブ名を変更する際はここも同時に更新すること、上記運用手順参照）:
#   ci-complete                                                → ci.yml `ci-complete` ジョブ
#   cargo doc                                                  → ci.yml `doc` ジョブ
#   cargo audit / cargo deny (all feature configs)             → ci.yml `dep-audit` ジョブ
#   unsafe 追加の検知トリアージ                                 → ci.yml `unsafe-triage` ジョブ
#   pay-for-what-you-use 検証（cargo tree/geiger・バイナリサイズ）→ ci.yml 同名ジョブ
#   openapi 2 段階ビルド（gen-openapi --check → cargo build）   → ci.yml 同名ジョブ
#   openapi-typescript 連携パイプライン（…）                    → ci.yml 同名ジョブ
#   actionlint（ワークフロー静的検証）                          → ci.yml 同名ジョブ
#   fuzz smoke（cargo-fuzz、nightly pinned）                    → ci.yml 同名ジョブ
#   WebRTC rebind e2e（standalone crate、#507）                 → ci.yml 同名ジョブ
#   決定的マイクロベンチ（alloc カウンタ、#615）                → ci.yml `microbench` ジョブ
#   cargo llvm-cov（コア行カバレッジ 80% ゲート）                → ci.yml `coverage` ジョブ
#   codex / preflight, codex / review, codex / post_feedback    → ai-review.yml の
#                                                                  reusable workflow 呼び出し
#                                                                  `codex` の子ジョブ
#   cargo fmt --check (ubuntu-latest|macos-latest|windows-latest)
#   cargo clippy (ubuntu-latest|macos-latest|windows-latest)
#   cargo test (ubuntu-latest|macos-latest|windows-latest)      → ci.yml `fmt`/`clippy`/`test`
#                                                                  の matrix（#679・PR #685）
# --------------------------------------------------
read -r -d '' REQUIRED_CONTEXTS_TSV <<'EOF' || true
Cursor Bugbot	1210556
WebRTC rebind e2e（standalone crate、#507）	15368
actionlint（ワークフロー静的検証）	15368
cargo audit / cargo deny (all feature configs)	15368
cargo doc	15368
cargo llvm-cov（コア行カバレッジ 80% ゲート）	15368
ci-complete	15368
fuzz smoke（cargo-fuzz、nightly pinned）	15368
openapi 2 段階ビルド（gen-openapi --check → cargo build）	15368
openapi-typescript 連携パイプライン（schema.d.ts 鮮度検証 → tsc --noEmit）	15368
pay-for-what-you-use 検証（cargo tree/geiger・バイナリサイズ）	15368
unsafe 追加の検知トリアージ	15368
決定的マイクロベンチ（alloc カウンタ、#615）	15368
codex / preflight	15368
codex / review	15368
codex / post_feedback	15368
cargo fmt --check (ubuntu-latest)	15368
cargo fmt --check (macos-latest)	15368
cargo fmt --check (windows-latest)	15368
cargo clippy (ubuntu-latest)	15368
cargo clippy (macos-latest)	15368
cargo clippy (windows-latest)	15368
cargo test (ubuntu-latest)	15368
cargo test (macos-latest)	15368
cargo test (windows-latest)	15368
EOF

REQUIRED_CONTEXTS_JSON="$(printf '%s\n' "${REQUIRED_CONTEXTS_TSV}" | jq -R -s '
    split("\n") | map(select(length > 0)) | map(split("\t")) |
    map({context: .[0], integration_id: (.[1] | tonumber)})
')"

# --------------------------------------------------
# ruleset 定義（deletion + non_fast_forward + pull_request + required_status_checks）。
# live（`main-protection`）の全フィールドを持つ。1 つでも落として PUT すると、その
# フィールドは黙って解除され保護が弱まる（例: `allowed_merge_methods` を落とすと
# squash-only が外れる）。
# --------------------------------------------------
build_desired_json() {
    jq -n \
        --arg name "${RULESET_NAME}" \
        --arg ref "${TARGET_REF}" \
        --argjson contexts "${REQUIRED_CONTEXTS_JSON}" \
        '{
            name: $name,
            target: "branch",
            enforcement: "active",
            bypass_actors: [],
            conditions: {
                ref_name: {
                    include: [$ref],
                    exclude: []
                }
            },
            rules: [
                { type: "deletion" },
                { type: "non_fast_forward" },
                {
                    type: "pull_request",
                    parameters: {
                        required_approving_review_count: 0,
                        dismiss_stale_reviews_on_push: false,
                        required_reviewers: [],
                        require_code_owner_review: false,
                        dismissal_restriction: { enabled: false, allowed_actors: [] },
                        require_last_push_approval: false,
                        required_review_thread_resolution: true,
                        require_extra_approval_for_unattributed_changes: true,
                        allowed_merge_methods: ["squash"]
                    }
                },
                {
                    type: "required_status_checks",
                    parameters: {
                        strict_required_status_checks_policy: false,
                        do_not_enforce_on_create: false,
                        required_status_checks: $contexts
                    }
                }
            ]
        }'
}

# --------------------------------------------------
# 正規化フィルタ（desired・live 双方に同じものを適用し比較可能にする）。
# 比較対象: name / target / enforcement / bypass_actors / conditions / rules。
# 除外するのは id・node_id・created_at・updated_at・_links・source・source_type・
# current_user_can_bypass（GitHub が付与するメタデータ、ruleset の内容ではない）のみ。
# 未知のフィールドは意図的に比較対象へ残す（GitHub が新フィールドを返し始めたら
# 差分として検知できるようにする、fail-closed）。
# rules は type でソートし、required_status_checks は context（次いで integration_id）で
# ソートして、GitHub 側の順序変化を差分として誤検知しない。
# --------------------------------------------------
readonly NORMALIZE_FILTER='
    del(.id, .node_id, .created_at, .updated_at, ._links, .source, .source_type,
        .current_user_can_bypass)
    | .bypass_actors |= (. // [] | sort)
    | .conditions.ref_name.include |= (. // [] | sort)
    | .conditions.ref_name.exclude |= (. // [] | sort)
    | .rules |= (
        map(
            if .type == "required_status_checks" then
                .parameters.required_status_checks |=
                    sort_by(.context, .integration_id)
            else . end
          )
        | sort_by(.type)
      )
'

DESIRED_JSON="$(build_desired_json)"
DESIRED_NORMALIZED="$(printf '%s' "${DESIRED_JSON}" | jq -S "${NORMALIZE_FILTER}")"

if [ "${MODE}" = "print-desired" ]; then
    printf '%s\n' "${DESIRED_NORMALIZED}"
    exit 0
fi

# --------------------------------------------------
# 対象リポジトリの owner/repo を gh の現在のコンテキストから取得する
# （リポジトリ名をハードコードしない。フォーク・テンプレート再利用時にも動作させるため）
# --------------------------------------------------
REPO_NWO="$(gh repo view --json nameWithOwner --jq '.nameWithOwner')"
if [ -z "${REPO_NWO}" ]; then
    echo "エラー: gh repo view でリポジトリを特定できません（gh auth login 未実施の可能性）" >&2
    exit 2
fi
echo "==> 対象リポジトリ: ${REPO_NWO}" >&2

# --------------------------------------------------
# live の main-protection ruleset を取得する（存在確認・重複検出込み）。
# --------------------------------------------------
RULESETS_JSON="$(gh api "repos/${REPO_NWO}/rulesets" 2>/dev/null)" || {
    echo "エラー: repos/${REPO_NWO}/rulesets の取得に失敗しました（gh api エラー）" >&2
    exit 2
}

MATCHING_IDS="$(printf '%s' "${RULESETS_JSON}" | jq -r \
    --arg name "${RULESET_NAME}" --arg target "branch" \
    '[.[] | select(.name == $name and .target == $target) | .id] | join(" ")')"

# shellcheck disable=SC2206 # 数値 id のみを含む前提の単純な単語分割
MATCHING_IDS_ARR=(${MATCHING_IDS})

if [ "${#MATCHING_IDS_ARR[@]}" -gt 1 ]; then
    echo "エラー: ruleset '${RULESET_NAME}'（target=branch）が複数存在します（id: ${MATCHING_IDS}）。手動で解消してください" >&2
    exit 2
fi

EXISTING_ID="${MATCHING_IDS_ARR[0]:-}"

if [ -z "${EXISTING_ID}" ]; then
    if [ "${MODE}" = "check" ]; then
        echo "差分: ruleset '${RULESET_NAME}'（target=branch）が live に存在しません" >&2
        exit 1
    fi

    # apply かつ未存在: 2 つ目の ruleset を誤って作らないよう、default branch へ
    # 既に何らかの branch ruleset が適用されていないか確認してから POST する
    # （fail-closed。新規 fork 等、本当に何も無い場合のみ POST が成立する）。
    DEFAULT_BRANCH="$(gh repo view --json defaultBranchRef --jq '.defaultBranchRef.name')"
    if [ -z "${DEFAULT_BRANCH}" ]; then
        echo "エラー: gh repo view で default branch 名を取得できません（空文字）。'ルールなし' と誤判定して 2 つ目の ruleset を作らないため中止します" >&2
        exit 2
    fi
    # fail-closed: API 呼び出し失敗（一時障害・403 等）と「既存ルールが実際にゼロ件」
    # という正常応答を区別する。失敗を空リストへ読み替えて POST へ進むと、既存の
    # 保護を確認できないまま別の main-protection ruleset を重複作成しうる
    # （PR #695 レビュー指摘。codex/review・Cursor Bugbot が独立に同一箇所を指摘）。
    if ! BRANCH_RULES_JSON="$(gh api "repos/${REPO_NWO}/rules/branches/${DEFAULT_BRANCH}" 2>&1)"; then
        echo "エラー: repos/${REPO_NWO}/rules/branches/${DEFAULT_BRANCH} の取得に失敗しました（gh api エラー: ${BRANCH_RULES_JSON}）。既存ルールの有無を確認できないため中止します" >&2
        exit 2
    fi
    if [ "$(printf '%s' "${BRANCH_RULES_JSON}" | jq 'length')" != "0" ]; then
        echo "エラー: ruleset '${RULESET_NAME}' は存在しませんが、default branch '${DEFAULT_BRANCH}' には既に別の ruleset が適用されています。2 つ目の ruleset を作らないため中止します。手動で確認してください" >&2
        exit 2
    fi

    echo "==> ruleset '${RULESET_NAME}' を新規作成します" >&2
    printf '%s' "${DESIRED_JSON}" | gh api \
        --method POST \
        "repos/${REPO_NWO}/rulesets" \
        --input - >/dev/null
    echo "==> 作成しました" >&2
    exit 0
fi

LIVE_DETAIL="$(gh api "repos/${REPO_NWO}/rulesets/${EXISTING_ID}" 2>/dev/null)" || {
    echo "エラー: repos/${REPO_NWO}/rulesets/${EXISTING_ID} の取得に失敗しました（gh api エラー）" >&2
    exit 2
}
LIVE_NORMALIZED="$(printf '%s' "${LIVE_DETAIL}" | jq -S "${NORMALIZE_FILTER}")"

# 旧名の ruleset が残っていないか警告する（削除はしない。運用者の判断に委ねる）。
if printf '%s' "${RULESETS_JSON}" | jq -e '.[] | select(.name == "main-required-checks")' >/dev/null 2>&1; then
    echo "警告: 旧名の ruleset 'main-required-checks' がまだ存在します（#693 で 'main-protection' に一本化。手動で確認・削除を検討してください）" >&2
fi

if [ "${DESIRED_NORMALIZED}" = "${LIVE_NORMALIZED}" ]; then
    if [ "${MODE}" = "check" ]; then
        echo "==> live と定義は一致しています（ruleset '${RULESET_NAME}'、id=${EXISTING_ID}）" >&2
        exit 0
    fi
    echo "==> 差分なし。書き込みは行いません（ruleset '${RULESET_NAME}'、id=${EXISTING_ID}）" >&2
    exit 0
fi

DESIRED_TMP="$(mktemp)"
LIVE_TMP="$(mktemp)"
cleanup_tmp() {
    rm -f "${DESIRED_TMP}" "${LIVE_TMP}"
}
trap cleanup_tmp EXIT
printf '%s\n' "${DESIRED_NORMALIZED}" >"${DESIRED_TMP}"
printf '%s\n' "${LIVE_NORMALIZED}" >"${LIVE_TMP}"

echo "==> live と定義に差分があります（ruleset '${RULESET_NAME}'、id=${EXISTING_ID}）" >&2
diff -u "${LIVE_TMP}" "${DESIRED_TMP}" || true

if [ "${MODE}" = "check" ]; then
    exit 1
fi

echo "==> 既存 ruleset '${RULESET_NAME}'（id=${EXISTING_ID}）を更新します" >&2
printf '%s' "${DESIRED_JSON}" | gh api \
    --method PUT \
    "repos/${REPO_NWO}/rulesets/${EXISTING_ID}" \
    --input - >/dev/null
echo "==> 更新しました" >&2
