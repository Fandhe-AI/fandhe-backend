#!/usr/bin/env bash
# Conventional Commits 検証スクリプト（lefthook の commit-msg フックから呼ばれる）:
# .claude/rules/conventional-commits.md が定める形式
#   <type>(<scope>): <description>
# にコミットメッセージ 1 行目が適合するかを Node.js 等の外部依存なしで検証する
# （依存最小の方針。厳密な commitlint ルール互換ではなく、type 集合・scope 形式・
# 区切り記法のみを機械検証する。description の内容妥当性はレビューが担う）。
#
# 使い方: bash scripts/commit-msg-check.sh <コミットメッセージファイル>
#   （lefthook.yml の commit-msg フックが {1} でファイルパスを渡す）
#
# 終了コード: 0 = 適合 / 1 = 規約違反（フェイルクローズ） / 2 = 引数・前提エラー
#
# セキュリティ（.claude/rules/security.md）: コミットメッセージは信頼できない入力として
# 扱い、grep -E のパターン照合のみで検証する。eval・コマンド置換への展開は行わない。
set -euo pipefail

if [ $# -ne 1 ] || [ ! -f "$1" ]; then
  echo "使い方: $0 <コミットメッセージファイル>" >&2
  exit 2
fi

# コメント行（# 始まり）を除いた最初の非空行をヘッダとして取り出す
header="$(grep -v '^#' "$1" | grep -m1 -v '^[[:space:]]*$' || true)"

if [ -z "$header" ]; then
  echo "commit-msg: コミットメッセージが空です" >&2
  exit 1
fi

# git が自動生成するメッセージ（merge / revert / fixup / squash）は検証対象外とする
# （fail-closed 原則に対する意図的な例外。git 標準ワークフローを阻害しないための限定緩和）
case "$header" in
  Merge\ * | Revert\ * | fixup!\ * | squash!\ *)
    exit 0
    ;;
esac

# .claude/rules/conventional-commits.md の type 表と同期させること
types='feat|fix|perf|refactor|test|docs|build|ci|chore'

if ! printf '%s\n' "$header" | grep -qE "^(${types})(\([a-z0-9*][a-z0-9*.,/_-]*\))?!?: .+"; then
  cat >&2 <<EOF
commit-msg: Conventional Commits 形式に適合しません
  対象: ${header}
  形式: <type>(<scope>): <description>
  type: ${types}
  例:   feat(core): graceful shutdown の grace 期間を設定可能にする
  詳細: .claude/rules/conventional-commits.md
EOF
  exit 1
fi

exit 0
