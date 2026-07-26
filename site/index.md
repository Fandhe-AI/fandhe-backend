# fandhe-backend

**fandhe-backend** は、AI によるセキュリティ脆弱性発見リスクに備えて Rust で
新規構築された、軽量・高速・高並行なバックエンドフレームワークです。axum 級の
性能を目標に、**最小コア + Cargo feature 駆動プラグイン**設計で、WebSocket /
GraphQL / WebRTC / OpenAPI 自動生成 / 可観測性などを段階的に拡張できます。

公開対象の 13 クレートは [crates.io](https://crates.io/crates/fandhe-backend-core) に
v0.1.0 として公開済みです。`cargo add fandhe-backend-core` で導入し、プラグインは
`cargo add fandhe-backend-core --features websocket` のように feature で有効化します。

## 2 つの核となる原則

- **pay-for-what-you-use**: feature を無効化したら、その依存・コード・`unsafe`・
  バイナリサイズ増をすべてゼロにします。使わない機能のコストを一切払わせません。
- **AI ファースト保守性**: doc test・網羅テスト・CI ガードレールを整備し、
  AI エージェントが安全に保守できる状態を保ちます。

## feature プラグイン一覧

最小コア（`fandhe-backend-core`）は HTTP/1.1 サーバと 3 種の拡張点
（`Middleware` / `UpgradeHandler` / `RequestGate`）のみを持ち、以下はすべて
Cargo feature で個別に着脱できるプラグインです。多くは `Server` への明示登録
（opt-in）時のみ動作し、無効化・未登録時は依存・コード・バイナリ増がゼロになります。

| feature | 提供する機能 |
|---------|-------------|
| `websocket` | RFC 6455 準拠の WebSocket ハンドシェイク・ユーザー定義メッセージハンドラ |
| `graphql` | async-graphql によるスキーマ登録・クエリ実行（`POST /graphql`） |
| `openapi` | OpenAPI スキーマ自動生成・配信（`GET /openapi.json` / `GET /openapi.yaml`） |
| `webrtc-proxy` | WebRTC シグナリングプロキシ（別プロセス切り出し型） |
| `webrtc` | in-process WebRTC（`webrtc-rs` 直接依存） |
| `tracing` | サンプリング付き可観測性（非同期・バッファ済み I/O） |
| `cors` | CORS ヘッダ付与（レスポンス後処理型シーム経由） |
| `compression` | レスポンス gzip 圧縮（CORS の後に逐次適用） |
| `static` | 静的ファイル配信（パストラバーサル対策済み） |

## はじめる

### Getting Started

クローンから最小サーバの起動・動作確認までを最短手順で説明します。

→ [Getting Started](/fandhe-backend/getting-started/)

### Guides

feature 構成別サンプル・チュートリアル・拡張点自作・ストリーミング・graceful
shutdown など、目的別ガイドの入口です。

→ [Guides](/fandhe-backend/guides/)

### Examples

`examples/with-*` 3 種と `templates/app` へ、独立して `cargo run` できるサンプルの
入口です。

→ [Examples](/fandhe-backend/examples/)

### API Reference

`Server` / `BoundServer` / `Handler` から各クレート・プラグイン設定 API まで、
公開 API の全体像と契約を俯瞰できます。

→ [API Reference](/fandhe-backend/api/server-api/)

---

ソースコードは [GitHub リポジトリ](https://github.com/Fandhe-AI/fandhe-backend)
で公開されています（MIT OR Apache-2.0 デュアルライセンス）。
