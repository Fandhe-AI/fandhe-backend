#!/usr/bin/env bash
# lefthook で git hooks（pre-commit / commit-msg）を配線する。`make hooks` / `make setup` から呼ばれる。
# lefthook が PATH に無ければ npx 経由（lefthook@2）で実行する。lefthook.yml の編集方針は
# .claude/rules/git-hooks.md を参照。
set -euo pipefail

if [ "$#" -ne 0 ]; then
  echo "usage: $(basename "$0") (no arguments)" >&2
  exit 2
fi

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if command -v lefthook >/dev/null 2>&1; then
  lefthook install
else
  npx --yes lefthook@2 install
fi
