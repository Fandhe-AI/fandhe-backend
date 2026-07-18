# backend-framework

AI によるセキュリティ脆弱性発見リスクに備え、Rust で新規構築する軽量・高速・高並行なバックエンドフレームワークです。axum 級の性能を目標に、プラグインによる拡張（WebRTC / WebSocket）、多様な通信規格（GraphQL / tRPC）、OpenAPI 自動生成・容易なロギングを備えます。

> フレームワーク名は仮称（`backend-framework`）です。正式名称の決定候補として
> `wrenframe` を選定しました（命名根拠・候補評価は
> [`docs/design/framework-naming.md`](./docs/design/framework-naming.md) 参照）。
> この決定候補は PR レビューゲート（人間承認）の通過をもって正式に確定し、
> crate 名・リポジトリ名等への反映は同ドキュメントの段階的移行計画に従い
> 順次進めます。

## 仕様

仕様書（ブレスト〜PoC〜要件定義〜タスク分解〜ロードマップ）は [Fandhe-AI/backend-framework-spec](https://github.com/Fandhe-AI/backend-framework-spec) で管理し、`docs/spec/` にサブモジュールとして取り込んでいます。

```bash
git clone --recurse-submodules git@github.com:Fandhe-AI/backend-framework.git
# 既存クローンの場合
git submodule update --init
```

| ドキュメント | 内容 |
|-------------|------|
| [`docs/spec/04-requirements.md`](./docs/spec/04-requirements.md) | MoSCoW 優先度付き要件（REQ-1〜15）・受け入れ基準 |
| [`docs/spec/05-tasks.md`](./docs/spec/05-tasks.md) | タスク分解（全 56 タスク・依存関係・工数） |
| [`docs/spec/06-roadmap.md`](./docs/spec/06-roadmap.md) | マイルストーン MS-1〜MS-6・着手判定 |

## 開発の進め方

`docs/spec/06-roadmap.md` のマイルストーンに従って実装します。

- **MS-1**: 基盤構築（`cargo workspace`・CI・依存監査ベースライン）・最小コア（HTTP/1.1・3 種拡張点）・プラグイン機構
- **MS-2〜MS-4**: Must 要件（性能ベンチマーク・セキュリティ・OpenAPI 自動生成ほか）
- **MS-5〜MS-6**: Should 要件（GraphQL / tRPC・WebRTC / WebSocket プラグイン・micro-service-hub 共通配線）

実装着手の最初のタスクは TASK-1.1（`cargo workspace`・CI 基盤整備）です。

## コントリビュート

開発フロー・コミット規約・設計原則は [`CONTRIBUTING.md`](./CONTRIBUTING.md) を参照してください。

## ライセンス

本プロジェクトは [MIT ライセンス](./LICENSE-MIT) と [Apache License 2.0](./LICENSE-APACHE) の
デュアルライセンスで提供されます。あなたが本プロジェクトへ提出する Contribution は、明示的な
別段の定めがない限り、上記デュアルライセンスの下で提供されるものとみなされます
（詳細は [`CONTRIBUTING.md`](./CONTRIBUTING.md) を参照）。

crates.io への公開手順（名前確保・所有権・リリース CI）は
[`docs/design/crates-io-release.md`](./docs/design/crates-io-release.md) で定めています
（現時点では実際の公開は行っていません）。
