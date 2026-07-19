#!/usr/bin/env bash
# .claude/worktrees/ 残存ワークツリーの棚卸し・退避・削除（イシュー #221）。
#
# 背景: implement-issue-tree ワークフローは .claude/worktrees/<id>/ に per-issue の git
# worktree を作成する。ワークフロー・セッション終了後に worktree の掃除が漏れると
# `.git/worktrees/` メタデータだけが残る「登録済みだが古い」ケースや、リポジトリ改名
# （#209〜#211、backend-framework → fandhe-backend）で `.git` ファイルの参照先が失われ
# git worktree list に現れない「孤児（orphan）」ディレクトリが蓄積し、ディスクを圧迫する
#（2026-07-19 時点で .claude/worktrees/ 配下 約 160 ディレクトリ・約 97GB）。
#
# 安全基準（fail-closed、.claude/rules/security.md）:
#   - git worktree list --porcelain に登録済みのワークツリーは既定で削除しない
#     （並列実行中の他セッションを壊さないため。locked / dirty は常に skip）。
#   - 削除対象は「.claude/worktrees/ 直下に実在するが登録簿に存在しない孤児」のみ。
#   - 孤児は git で未コミット変更を判定できないため、削除前に source のみ（target/・
#     node_modules/・.git を除く）を _/worktree-salvage/<dirname>.tar.gz へ退避する。
#   - 既定は dry-run（棚卸し一覧の出力のみ）。--apply 指定時のみ削除・prune を実行する。
#   - 削除パスは realpath で解決し、メイン working copy 配下の .claude/worktrees/ 直下
#     であることを検証してから rm する（symlink・パストラバーサル対策）。
#
# 使い方:
#   scripts/clean-worktrees.sh [--apply] [--no-salvage]
#
#   --apply       実際に孤児ディレクトリを退避・削除する（省略時は dry-run のみ）
#   --no-salvage  --apply と併用時、退避 tar の作成をスキップする（既定は退避あり）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# REPO_ROOT はスクリプト自身の所在（scripts/ の親）から解決するのが既定だが、
# scripts/tests/run-clean-worktrees-tests.sh のセルフテストが実 workspace を汚さずに
# 一時 git リポジトリで検証できるよう、環境変数で上書き可能にする
# （unsafe-triage.sh・feature-flow-check.sh と同一パターン）。
REPO_ROOT="${FANDHE_BACKEND_CLEAN_WORKTREES_REPO_ROOT:-$(cd "${SCRIPT_DIR}/.." && pwd)}"
cd "${REPO_ROOT}"

APPLY=0
SALVAGE=1

usage() {
    cat <<'EOF'
使い方: clean-worktrees.sh [--apply] [--no-salvage]

  --apply       孤児ワークツリーを退避・削除する（省略時は棚卸しのみの dry-run）
  --no-salvage  --apply と併用時、退避 tar の作成をスキップする（既定は退避あり）
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --apply)
            APPLY=1
            shift
            ;;
        --no-salvage)
            SALVAGE=0
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

# メイン working copy の .git ディレクトリ（worktree 内から実行された場合でも共有の
# 実体を指す）から worktrees ルートを解決する。
GIT_COMMON_DIR="$(git rev-parse --git-common-dir)"
MAIN_GIT_DIR="$(cd "${GIT_COMMON_DIR}" && pwd)"
MAIN_WORKTREE_ROOT="$(cd "${MAIN_GIT_DIR}/.." && pwd)"
WORKTREES_ROOT="${MAIN_WORKTREE_ROOT}/.claude/worktrees"

if [ ! -d "${WORKTREES_ROOT}" ]; then
    echo "==> clean-worktrees: ${WORKTREES_ROOT} が存在しません（対象なし、exit 0）"
    exit 0
fi

SALVAGE_DIR="${MAIN_WORKTREE_ROOT}/_/worktree-salvage"

# 現在の worktree 自身（このスクリプトを実行している worktree）は登録有無に関わらず
# 削除対象から常に除外する（自己破壊防止）。
SELF_WORKTREE="$(pwd)"

# --------------------------------------------------
# 登録済みワークツリーの分類（git worktree list --porcelain をパースする）。
# porcelain 出力は空行区切りのレコード列、各レコードは複数行の "key value" 形式。
# --------------------------------------------------
declare -A REGISTERED_PATH_STATUS=()  # 絶対パス -> clean / dirty / locked

current_path=""
current_locked=0
flush_record() {
    if [ -z "${current_path}" ]; then
        return
    fi
    local status="clean"
    if [ "${current_locked}" -eq 1 ]; then
        status="locked"
    elif ! git -C "${current_path}" status --porcelain >/dev/null 2>&1; then
        # worktree のメタデータはあるが実体が壊れている等、判定不能なものは安全側で
        # dirty 扱いにする（削除対象は孤児のみなので登録済みなら本来 skip 判定になる）。
        status="dirty"
    elif [ -n "$(git -C "${current_path}" status --porcelain 2>/dev/null)" ]; then
        status="dirty"
    fi
    REGISTERED_PATH_STATUS["${current_path}"]="${status}"
    current_path=""
    current_locked=0
}

while IFS= read -r line; do
    case "${line}" in
        "worktree "*)
            flush_record
            current_path="$(printf '%s' "${line}" | cut -d' ' -f2-)"
            current_path="$(cd "${current_path}" 2>/dev/null && pwd || printf '%s' "${current_path}")"
            ;;
        "locked"*)
            current_locked=1
            ;;
        "")
            flush_record
            ;;
        *)
            : # bare / detached / HEAD / branch 行は分類に不要
            ;;
    esac
done < <(git worktree list --porcelain)
flush_record

# --------------------------------------------------
# .claude/worktrees/ 直下を走査し、登録済み / 孤児に分類する。
# --------------------------------------------------
declare -a ORPHAN_DIRS=()
declare -a REGISTERED_REPORT=()

for entry in "${WORKTREES_ROOT}"/*/; do
    [ -d "${entry}" ] || continue
    abs_path="$(cd "${entry}" && pwd)"
    dirname_only="$(basename "${abs_path}")"
    size="$(du -sh "${abs_path}" 2>/dev/null | cut -f1)"
    mtime="$(date -r "${abs_path}" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo "unknown")"

    if [ "${abs_path}" = "${SELF_WORKTREE}" ]; then
        REGISTERED_REPORT+=("SELF  ${dirname_only}  size=${size}  mtime=${mtime}（実行中の worktree、対象外）")
        continue
    fi

    if [ -n "${REGISTERED_PATH_STATUS[${abs_path}]+x}" ]; then
        status="${REGISTERED_PATH_STATUS[${abs_path}]}"
        REGISTERED_REPORT+=("REGISTERED[${status}]  ${dirname_only}  size=${size}  mtime=${mtime}（削除対象外）")
        continue
    fi

    ORPHAN_DIRS+=("${abs_path}")
    REGISTERED_REPORT+=("ORPHAN  ${dirname_only}  size=${size}  mtime=${mtime}（削除候補）")
done

echo "==> clean-worktrees: 棚卸し結果（${WORKTREES_ROOT}）"
if [ "${#REGISTERED_REPORT[@]}" -eq 0 ]; then
    echo "  （対象ディレクトリなし）"
else
    for line in "${REGISTERED_REPORT[@]}"; do
        echo "  ${line}"
    done
fi
echo "==> 登録済み（削除対象外）: $(( ${#REGISTERED_REPORT[@]} - ${#ORPHAN_DIRS[@]} )) 件 / 孤児（削除候補）: ${#ORPHAN_DIRS[@]} 件"

if [ "${APPLY}" -ne 1 ]; then
    echo "==> dry-run のため削除は行いません（実削除するには --apply を指定してください）"
    exit 0
fi

if [ "${#ORPHAN_DIRS[@]}" -eq 0 ]; then
    echo "==> 孤児ワークツリーがないため削除処理は行いません"
    git worktree prune
    exit 0
fi

if [ "${SALVAGE}" -eq 1 ]; then
    mkdir -p "${SALVAGE_DIR}"
fi

removed_count=0
skipped_count=0

for abs_path in "${ORPHAN_DIRS[@]}"; do
    dirname_only="$(basename "${abs_path}")"

    # 安全弁: realpath 解決後、メイン working copy 配下の .claude/worktrees/ 直下で
    # あることを再検証してから rm する（symlink・パストラバーサル対策、
    # .claude/rules/security.md）。
    resolved_parent="$(cd "${abs_path}/.." && pwd)"
    if [ "${resolved_parent}" != "${WORKTREES_ROOT}" ]; then
        echo "==> 警告: '${dirname_only}' は想定外の場所（${resolved_parent}）のため削除をスキップします" >&2
        skipped_count=$((skipped_count + 1))
        continue
    fi

    if [ "${SALVAGE}" -eq 1 ]; then
        tar_path="${SALVAGE_DIR}/${dirname_only}.tar.gz"
        # --exclude のパターンは "*/target" のようにワイルドカードを先頭に置き、
        # cargo workspace のネストしたビルド成果物（例: crates/*/fuzz/target）も
        # 除外する（トップレベルの "${dirname_only}/target" だけでなく任意の深さに
        # マッチさせるため。イシュー #221 レビュー指摘）。
        if ! tar -C "${WORKTREES_ROOT}" \
            --exclude="*/target" \
            --exclude="*/node_modules" \
            --exclude="${dirname_only}/.git" \
            -czf "${tar_path}" -- "${dirname_only}"; then
            echo "==> 警告: '${dirname_only}' の退避（tar）に失敗したため削除をスキップします" >&2
            skipped_count=$((skipped_count + 1))
            continue
        fi
    fi

    rm -rf -- "${abs_path}"
    echo "==> 削除: ${dirname_only}"
    removed_count=$((removed_count + 1))
done

git worktree prune

echo "==> clean-worktrees: 削除 ${removed_count} 件 / スキップ ${skipped_count} 件"
if [ "${SALVAGE}" -eq 1 ]; then
    echo "==> 退避先: ${SALVAGE_DIR}"
fi
