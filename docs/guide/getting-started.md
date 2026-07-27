# Getting Started

crates.io に公開済みの fandhe-backend を自分のプロジェクトへ組み込み、最小サーバを
起動して動作確認するまでの最短手順です。リポジトリのクローンは不要です。

## 前提

- Rust の stable ツールチェーン（`rustup` でインストールされていれば追加設定は
  不要です）

## 1. プロジェクトを作成して依存を追加する

crates.io v0.1.0 として公開済み（2026-07-21）です。
[crates.io](https://crates.io/crates/fandhe-backend-core) から直接依存に追加します。

```bash
cargo new my-app && cd my-app

cargo add fandhe-backend-core fandhe-backend-http fandhe-backend-routes
cargo add tokio --features rt-multi-thread,macros

# プラグインは feature で有効化します（例: WebSocket。一覧は本ページ 5 節参照）
cargo add fandhe-backend-core --features websocket
```

公開対象クレートは `fandhe-backend-core` / `fandhe-backend-http` / `fandhe-backend-routes` と
`fandhe-backend-plugin-*` の計 13 クレート（すべて v0.1.0 の lockstep）ですが、
通常は `fandhe-backend-core` の feature 経由で利用すれば十分です（feature 一覧は
本ページ 5 節参照）。feature を何も指定しない場合、`fandhe-backend-plugin-*` の
依存・コードは一切バイナリに含まれません（pay-for-what-you-use、
[`.claude/rules/pay-for-what-you-use.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/pay-for-what-you-use.md)）。

`cargo new` の代わりに雛形から始めることもできます。複数 feature を組み合わせた
実運用形の雛形は
[`templates/app/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/templates/app/)、
1 機能ずつの独立サンプルは
[`examples/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/examples/)
にあり、いずれも standalone プロジェクトとしてコピーしてそのまま `cargo run`
できます（コピー後は `Cargo.toml` の依存から `path = ...` を外し、
`version = "0.1.0"` のみの crates.io 版参照に切り替えてください）。

## 2. 最小サーバを書く

`src/main.rs` を次の内容に置き換えます。`fandhe_backend_core::Server` に
`fandhe_backend_routes::Router` を 1 件登録しただけの最小構成です。

```rust,no_run
use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::Router;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let router = Router::new()
        .route("GET", "/", |_head, _body| {
            Response::new(200, b"hello fandhe-backend\n".to_vec())
        })
        .route("GET", "/health", |_head, _body| {
            Response::new(200, b"ok\n".to_vec())
        });

    let server = Server::new().handler(router);
    let bound = server.bind("127.0.0.1:3000").await?;
    println!("listening on http://{}", bound.local_addr()?);
    bound.run().await
}
```

## 3. 起動して動作確認する

```bash
cargo run
```

別ターミナルから動作確認します。

```bash
curl -v http://127.0.0.1:3000/            # 200 応答
curl -v http://127.0.0.1:3000/health      # 200 応答
curl -v -X POST http://127.0.0.1:3000/    # 405 応答（/ は GET のみ登録）
curl -v http://127.0.0.1:3000/missing     # 404 応答（未登録パス）
```

`127.0.0.1` 固定でループバックにのみ待ち受けます。外部公開する場合は呼び出し側の
責任でバインドアドレスを明示的に変更してください
（[`.claude/rules/security.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/security.md) の攻撃表面最小化方針）。

## 4. コア構成の概観

- **`Server`**（`fandhe_backend_core::Server`）: builder パターンで構成する
  エントリポイント。`handler` でデフォルトハンドラ（通常は `fandhe_backend_routes::Router`）を、
  `middleware` / `gate` / `upgrade_handler` で拡張点を登録し、`bind` → `run` で
  サーバを起動します
- **`fandhe_backend_routes::Router`**: パス・メソッドごとにハンドラを登録するルーティング層。
  `impl Handler for Router` により `Server::handler` にそのまま渡せます
- **3 拡張点**（`fandhe_backend_core::{Middleware, UpgradeHandler, RequestGate}`）:
  新機能はまずこの 3 種のいずれかに載るか検討します（[`.claude/rules/coding-rust.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/coding-rust.md)）
  - `Middleware`: リクエスト/レスポンスの前後処理（例: `plugin-tracing`）
  - `UpgradeHandler`: プロトコルアップグレード（例: `plugin-websocket` の
    WebSocket ハンドシェイク）
  - `RequestGate`: リクエストの許可/拒否判定

## 5. feature 一覧

| feature | 提供プラグイン | 概要 |
|---------|---------------|------|
| （なし・既定） | — | HTTP/1.1 コア + ルーティングのみ |
| `websocket` | `fandhe-backend-plugin-websocket` | RFC 6455 ハンドシェイク + フレーミング（`UpgradeHandler` 経由） |
| `graphql` | `fandhe-backend-plugin-graphql` | `POST /graphql` パスインターセプト + `async-graphql` 実行 |
| `webrtc-proxy` | `fandhe-backend-plugin-webrtc-proxy` | WebRTC シグナリングを別プロセスに切り出すプロキシ型（MVP 推奨） |
| `webrtc` | `fandhe-backend-plugin-webrtc` | in-process WebRTC（`webrtc-rs` 直接依存、攻撃表面が大きいため通常は `webrtc-proxy` を推奨） |
| `tracing` | `fandhe-backend-plugin-tracing` | サンプリング付き可観測性（`Middleware` 経由） |
| `openapi` | `fandhe-backend-plugin-openapi` | `Server::openapi()` / `openapi_with(doc)` 登録時のみ `GET /openapi.json` / `GET /openapi.yaml` を配信 |
| `cors` | `fandhe-backend-plugin-cors` | `Server::cors(config)` 登録時のみ実リクエスト応答へ CORS ヘッダを付与（プリフライトは `Router::options_fallback` で配線） |
| `compression` | `fandhe-backend-plugin-compression` | `Server::compression(config)` 登録時のみ条件を満たすレスポンスを gzip 圧縮 |
| `static` | `fandhe-backend-plugin-static` | `Server::static_files(config)` 登録時のみ静的ファイルを `GET` 配信（二層防御のパストラバーサル対策付き） |

なお `fandhe-backend-plugin-hub-wiring`（JWT 検証・テナント境界強制）は
`crates/core` の feature ではなく、`RequestGate` 拡張点（`TenantGate`）を直接
登録して使う独立クレートです（`crates/core/Cargo.toml` の `[features]` が
feature 一覧の正です）。

feature 構成別の実行可能サンプルは [`feature-samples.md`](./feature-samples.md) を、
拡張点の実装を含む段階的な学習は [`tutorial.md`](./tutorial.md) を参照してください。
独立プロジェクトとしてそのまま `cargo run` できる standalone サンプル集は
[`examples/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/examples/)、
複数 feature を組み合わせた実運用形の雛形は
[`templates/app/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/templates/app/)
にあります。
