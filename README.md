# fandhe-backend

AI によるセキュリティ脆弱性発見リスクに備え、Rust で新規構築する軽量・高速・高並行なバックエンドフレームワークです。axum 級の性能を目標に、プラグインによる拡張（WebRTC / WebSocket）、多様な通信規格（GraphQL / tRPC）、OpenAPI 自動生成・容易なロギングを備えます。

> 正式名称は **`fandhe-backend`** に確定しています（決定経緯・可用性証跡・
> 新旧マッピングは
> [`docs/design/framework-naming.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/framework-naming.md) 参照）。

## インストール

crates.io（v0.4.0、2026-08-13 公開済み。変更履歴は `CHANGELOG.md` 参照）から
最小コアのみを使う場合は:

```bash
cargo add fandhe-backend-core
```

プラグインは Cargo feature で着脱します（pay-for-what-you-use。無効な feature の依存・コードはバイナリに一切含まれません）:

```bash
# 例: WebSocket プラグインを有効化
cargo add fandhe-backend-core --features websocket

# 例: 複数 feature の同時有効化
cargo add fandhe-backend-core --features graphql,openapi,cors
```

公開対象クレートは `fandhe-backend-http` / `fandhe-backend-routes` / `fandhe-backend-core` と
`fandhe-backend-plugin-*`（websocket / graphql / openapi / webrtc / webrtc-proxy / tracing /
hub-wiring / cors / compression / static）の 13 クレートで、すべて同一バージョン
（lockstep）で公開します。現時点の crates.io 公開版は 0.4.0（2026-08-13 公開、
変更履歴は `CHANGELOG.md`、公開手順は `docs/design/crates-io-release.md` 参照）です。
通常は `fandhe-backend-core` の feature 経由で利用し、
個別クレートを直接依存に追加する必要はありません
（`fandhe-backend-plugin-hub-wiring` のみ独立クレートとして直接利用します）。

## Getting Started

crates.io からの依存追加〜最小サーバ起動までの最短手順は [`docs/guide/getting-started.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/guide/getting-started.md) を、
feature 構成別のサンプル（websocket / graphql / webrtc 系 / tracing / openapi / cors / compression / static / hub-wiring）は
[`docs/guide/feature-samples.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/guide/feature-samples.md) を、
拡張点の実装まで含むチュートリアルは [`docs/guide/tutorial.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/guide/tutorial.md) を参照してください。
利用者向けドキュメントサイトは <https://fandhe-ai.github.io/fandhe-backend/> にあります。

## 仕様

仕様書（ブレスト〜PoC〜要件定義〜タスク分解〜ロードマップ）は [Fandhe-AI/fandhe-backend-spec](https://github.com/Fandhe-AI/fandhe-backend-spec) で管理し、`docs/spec/` にサブモジュールとして取り込んでいます。

```bash
git clone --recurse-submodules git@github.com:Fandhe-AI/fandhe-backend.git
# 既存クローンの場合
git submodule update --init
```

| ドキュメント | 内容 |
|-------------|------|
| [`docs/spec/04-requirements.md`](https://github.com/Fandhe-AI/fandhe-backend-spec/blob/main/04-requirements.md) | MoSCoW 優先度付き要件（REQ-1〜15）・受け入れ基準 |
| [`docs/spec/05-tasks.md`](https://github.com/Fandhe-AI/fandhe-backend-spec/blob/main/05-tasks.md) | タスク分解（全 56 タスク・依存関係・工数） |
| [`docs/spec/06-roadmap.md`](https://github.com/Fandhe-AI/fandhe-backend-spec/blob/main/06-roadmap.md) | マイルストーン MS-1〜MS-6・着手判定 |

## 開発の進め方

`docs/spec/06-roadmap.md` のマイルストーンに従って実装します。

- **MS-1**: 基盤構築（`cargo workspace`・CI・依存監査ベースライン）・最小コア（HTTP/1.1・3 種拡張点）・プラグイン機構
- **MS-2〜MS-4**: Must 要件（性能ベンチマーク・セキュリティ・OpenAPI 自動生成ほか）
- **MS-5〜MS-6**: Should 要件（GraphQL / tRPC・WebRTC / WebSocket プラグイン・micro-service-hub 共通配線）

実装着手の最初のタスクは TASK-1.1（`cargo workspace`・CI 基盤整備）です。

## 開発環境の構築

開発タスクは `Makefile` に集約しています。クローン後に `make setup` を実行すると
仕様書 submodule の取得と git hooks（lefthook）の配線が完了します。

```bash
git clone git@github.com:Fandhe-AI/fandhe-backend.git
cd fandhe-backend
make setup    # submodule 取得 + lefthook install（pre-commit / commit-msg フック配線）

make help     # ターゲット一覧
make build    # デフォルト構成のビルド
make test-all # 全 feature 有効のテスト（doc test 含む）
make lint     # cargo fmt --check + clippy -D warnings（CI と同一コマンド）
```

git hooks は [lefthook](https://lefthook.dev/)（`lefthook.yml`）で管理し、pre-commit で
`cargo fmt --all --check`、commit-msg で Conventional Commits 形式検証
（`scripts/commit-msg-check.sh`、外部依存なし）を行います。`--no-verify` での
スキップは禁止です（[`CONTRIBUTING.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/CONTRIBUTING.md) 参照）。

ホスト環境に Rust ツールチェーンを入れずに開発する場合は Docker を使えます
（`Dockerfile` / `compose.yaml`）:

```bash
make docker-build  # 開発用イメージのビルド
make docker-shell  # コンテナのシェルに入る（リポジトリを /work にマウント）
make docker-test   # コンテナ内で make test-all を実行
```

エディタ設定は `.editorconfig` で統一しています（Rust は rustfmt と同じ 4 スペース）。

## コントリビュート

開発フロー・コミット規約・設計原則は [`CONTRIBUTING.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/CONTRIBUTING.md) を参照してください。

## ライセンス

本プロジェクトは [MIT ライセンス](https://github.com/Fandhe-AI/fandhe-backend/blob/main/LICENSE-MIT) と
[Apache License 2.0](https://github.com/Fandhe-AI/fandhe-backend/blob/main/LICENSE-APACHE) の
デュアルライセンスで提供されます。あなたが本プロジェクトへ提出する Contribution は、明示的な
別段の定めがない限り、上記デュアルライセンスの下で提供されるものとみなされます
（詳細は [`CONTRIBUTING.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/CONTRIBUTING.md) を参照）。

crates.io への公開手順（名前確保・所有権・リリース CI）は
[`docs/design/crates-io-release.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/crates-io-release.md)
で定めています。
