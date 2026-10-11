---
paths:
  - "Makefile"
  - "scripts/*.sh"
description: >
  Makefile・scripts/*.sh を編集する際の規約。Makefile は scripts/ または cargo 等の 1 行呼び出しに
  限る薄い入口とし、ターゲット変更時は help・scripts/help.sh・呼び出し元を同期する。
  scripts/hooks/** 相当の git hooks は git-hooks.md、CI は ci.md の担当。
---

# Makefile / scripts 編集ルール

詳細な設計指針は make skill（`.claude/skills/make`）を参照する。

## 原則: Makefile は薄い入口

- レシピは `@scripts/<name>.sh [args]` か、言語ネイティブの 1 行コマンド（`cargo build --workspace`・
  `docker compose build dev` 等）に限る。複数行シェル・`if`/`for`・`curl` 等は `scripts/` へ移す
- `.DEFAULT_GOAL := help`、全ターゲットを `.PHONY` に宣言、`## ` コメントから `make help` を生成する
- 中身の無いターゲット（`@true` 等）を置かない（呼び出し元が終了コードを信用するため）
- GNU Make 3.81（macOS 標準）互換。`.ONESHELL`・`.RECIPEPREFIX` 等 3.82+ 専用機能は使わない

## ターゲットの追加・改名・削除

1. `.PHONY` と `## ` コメントを更新する（`## ` が無いターゲットは `make help` に出ない）
2. `scripts/help.sh`（Make 非依存の一覧。手動保守で自動同期されない）を同じ変更で更新する
3. 呼び出し元を洗い出して更新する:
    `grep -rn "make " .github .claude docs README.md CLAUDE.md AGENTS.md CONTRIBUTING.md lefthook.yml scripts Dockerfile compose.yaml`
4. `make help` と `make -n <target>` で確認する。`CLAUDE.md` のリポジトリ構造（Makefile 説明）も整合させる

## ターゲットの契約

- `fmt-check` / `clippy` / `lint` / `audit` は**ソースを変更しない**（検査のみ）。整形は `fmt`
- CI と同一コマンドをローカルで再現するターゲット（`fmt-check`・`clippy`・`audit`・`webrtc-e2e` 等）は
  CI 側と引数を揃える。差が出る場合は `## ` コメントに明記する
- 既存のターゲット名・終了コードは CI・lefthook・README・docs が参照するため改名しない
  （必要なら呼び出し元を同じ PR で更新する）
- `docker-*` はコンテナ内の `/work` 以外を破壊しない。共有ディレクトリを消す `clean` 系は新設しない

## scripts/*.sh の規約

- shebang `#!/usr/bin/env bash` と `set -euo pipefail`、実行権限（`chmod +x`）を付ける
- cwd に依存しない（`cd "$(dirname "${BASH_SOURCE[0]}")/.."` 等でルートへ移動）
- 引数を取らないスクリプトは未知の引数を `usage` を出して終了コード 2 で拒否する
- `shellcheck` を通す（CI の `actionlint` ジョブは shellcheck 連携を含む）。メッセージの言語は
  既存スクリプトに合わせる
- `scripts/hooks/**` 相当のフック用スクリプト（`commit-msg-check.sh` 等）は [[git-hooks]] に従う。
  CI から呼ぶ場合は [[ci]] に従い、ワークフローに処理を直書きしない
