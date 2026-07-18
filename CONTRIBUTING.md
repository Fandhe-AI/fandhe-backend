# CONTRIBUTING

backend-framework への貢献ありがとうございます。本ドキュメントは Issue 起票からマージ
までの開発フロー・規約をまとめたものです。詳細な運用ルールは `.claude/rules/` に
機械可読な形で定義されているため、AI エージェントで開発する場合はそちらも参照してください。

> フレームワーク名は仮称です。正式名称の決定は今後の課題です
> （[`docs/spec/01-brainstorm.md`](./docs/spec/01-brainstorm.md) 参照）。

## 前提ツール

- Rust の stable ツールチェーン（[`rust-toolchain.toml`](./rust-toolchain.toml) がバージョンを固定します）
- `rustfmt` / `clippy`（`rustup component add rustfmt clippy` で導入）
- 仕様書は `docs/spec/` に submodule として取り込んでいます。クローン時は次のいずれかで取得してください。

```bash
git clone --recurse-submodules git@github.com:Fandhe-AI/backend-framework.git
# 既存クローンの場合
git submodule update --init
```

## 開発フロー

1. **Issue**: 変更内容を Issue にまとめます。機能要求は
   `.github/ISSUE_TEMPLATE/feature-request.yml`（概要・受け入れ基準・影響範囲の想定が必須）
   を使ってください。受け入れ基準を欠く要求は実装に着手できません。
2. **計画**: 実装方針・対象ファイル・検証方法を計画としてまとめます。
3. **実装**: `docs/spec/06-roadmap.md` のマイルストーン・`crates/` のクレート構成に従って
   実装します。設計原則（後述）を守ってください。
4. **検証**: 変更に応じて以下を実行し、すべて通過することを確認します。

   ```bash
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace --all-features
   ```

5. **PR**: [Conventional Commits](https://www.conventionalcommits.org/) 形式でコミットし、
   PR を作成します。CI の集約ゲート（`ci-complete`）が緑であることと、人間レビューによる
   承認を経てからマージします。自動マージは行いません。

## コミット規約（Conventional Commits）

```
<type>(<scope>): <description>
```

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

- scope はクレート／プラグイン単位を推奨します（例: `core`, `http`, `routes`,
  `plugin-websocket`, `plugin-graphql`, `plugin-openapi`, `bench`, `ci`）。workspace 横断は `global`
- Breaking Change は `feat!:` または footer に `BREAKING CHANGE: <説明>` を明記します
- pre-commit / commit-msg フックは必ず通してください（`--no-verify` は使用不可）

## 設計原則

- **pay-for-what-you-use**: 機能はコアではなく Cargo feature 駆動プラグインに置きます。
  feature を無効化したら、その依存・コード・`unsafe`・バイナリサイズ増をゼロにしてください。
  `cargo tree` で feature 無効時に対象依存が現れないことを確認します。
- **拡張点は 3 種 trait に集約**: `Middleware` / `UpgradeHandler` / `RequestGate`。
  新機能を追加する際はまずこの拡張点に載せられないか検討してください。
- コアクレート（`crates/core` / `crates/http` / `crates/routes`）に重い依存や不要な
  `unsafe` を持ち込まないでください。

## 安全性・並行性

- `unsafe` は最小限にし、使う場合は `// SAFETY:` コメントで不変条件と安全性の根拠を書きます
- ライブラリコードでは `.unwrap()` / `.expect()` を避け、`Result` / `?` でエラーを伝播します
- panic をライブラリ境界の外へ漏らさないでください
- Tokio 上でブロッキング処理を await スレッドで実行しないでください（`spawn_blocking` を使用）

## テスト

- 実装変更（`crates/<name>/src/**/*.rs`）には同一クレートのテスト追加を伴わせてください
  （`crates/<name>/tests/**` の追加、または `#[test]` / `#[tokio::test]` / `#[cfg(test)]` /
  doc test の追加）。機械チェックは `scripts/feature-flow-check.sh --base <base-rev>` です
- 公開 API には doc comment と doc test を付けてください（`cargo test` で検証可能にします）

## セキュリティ・脆弱性の報告

脆弱性を発見した場合は、**公開 Issue には書かないでください**。GitHub の
[Private Vulnerability Reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing/privately-reporting-a-security-vulnerability)
機能を通じて非公開で報告してください（専用の `SECURITY.md` の整備は別途対応予定です）。

その他一般的なセキュリティ観点（入力検証・DoS 耐性・シークレット管理等）は変更のたびに
確認してください。既知脆弱性・ライセンス・`unsafe` は次のコマンドで監査できます。

```bash
cargo audit
cargo deny check
```

## ライセンス

本プロジェクトは MIT ライセンス（[`LICENSE-MIT`](./LICENSE-MIT)）と Apache License 2.0
（[`LICENSE-APACHE`](./LICENSE-APACHE)）のデュアルライセンスで提供されます。

あなたが明示的に別段の定めをしない限り、あなたが本プロジェクトへ提出する Contribution は、
Apache License 2.0 のセクション 5 に定義される条件に従い、追加の条項なしに上記デュアル
ライセンスの下で提供されるものとみなされます。

## 英語版について

現状の CONTRIBUTING / README は日本語を正としています。外部 OSS ユーザー向けの英語版整備は
今後の課題です。
