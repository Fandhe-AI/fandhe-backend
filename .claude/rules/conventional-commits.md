# Conventional Commits 規約

コミット・PR タイトルは [Conventional Commits](https://www.conventionalcommits.org/) に従う。

## 形式

```
<type>(<scope>): <description>

[optional body]

[optional footer(s)]
```

## type

| type | 用途 |
|------|------|
| `feat` | 機能追加 |
| `fix` | バグ修正 |
| `perf` | 性能改善（本フレームワークでは重要） |
| `refactor` | 挙動を変えないリファクタ |
| `test` | テスト追加・修正 |
| `docs` | ドキュメント |
| `build` | ビルド・依存・feature 構成 |
| `ci` | CI 設定 |
| `chore` | その他雑務 |

## scope

- クレート／プラグイン単位を推奨: `core`, `http`, `routes`, `plugin-websocket`,
  `plugin-graphql`, `plugin-openapi`, `plugin-webrtc`, `plugin-hub-wiring`, `plugin-tracing`,
  `bench`, `spec`, `ci`, `global`
- workspace 横断は `global`

## description

- 命令形・現在形。日本語でよい（[[japanese-style]]）。末尾ピリオド不要

## Breaking Change

- `feat!:` または footer に `BREAKING CHANGE: <説明>` を明記

## 厳守事項

- **`--no-verify` 禁止**。pre-commit / commit-msg フックを必ず通す
  （フックは lefthook で管理: `lefthook.yml`・`scripts/commit-msg-check.sh`、
  配線は `make hooks`。導入は `lefthook` skill を参照）
- コミット作成は `create-commit` skill、PR は `create-pr` skill を使う
- commitlint の詳細は `commitlint` skill を参照
