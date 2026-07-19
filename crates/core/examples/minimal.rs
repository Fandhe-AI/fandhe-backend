//! `crates/core` コアループの手動動作確認用最小サーバ（TASK-1.4-2 / #70、
//! TASK-1.5 / #14 で `fandhe_backend_routes::Router` を使う構成に更新）。
//!
//! [`fandhe_backend_core::Server`] に既定ハンドラとして
//! [`fandhe_backend_routes::Router`] を 1 件登録した最小構成で
//! `cargo run --example minimal -p fandhe-backend-core` から起動できる
//! ことを確認する。`Router` は `fandhe_backend_core::server` の
//! `impl Handler for fandhe_backend_routes::Router` 経由でそのまま `Server::handler` に
//! 渡せる（`server → routes` 依存の実利用例）。
//!
//! 動作確認手順:
//! ```text
//! $ cargo run --example minimal -p fandhe-backend-core
//! $ curl -v http://127.0.0.1:3000/           # 200 応答
//! $ curl -v -X POST http://127.0.0.1:3000/    # 405 応答（/ は GET のみ登録）
//! $ curl -v http://127.0.0.1:3000/missing     # 404 応答（未登録パス）
//! $ curl -v -H 'Connection: close' http://127.0.0.1:3000/  # 応答後に接続クローズ
//! $ curl -v http://127.0.0.1:3000/health      # 200 応答（TASK-10.4 / #59 計測対象パス）
//! ```
//!
//! `GET /health` は TASK-10.4（#59）で追加した計測対象ルート。
//! `examples/tracing_nfr.rs`（`tracing` feature 有効の比較対象サーバ）と同一パスを
//! 本ベースラインにも持たせることで、`benches/tracing-nfr-bench.sh` が両サーバの
//! `/health` へ同一負荷をかけて RPS・p95 の比を計測できるようにする（既存の
//! `GET /`＝graphql/webrtc NFR bench のベースラインには影響しない、追加のみ）。

use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::Router;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let router = Router::new()
        .route("GET", "/", |_head, _body| {
            Response::new(200, b"fandhe-backend: minimal example\n".to_vec())
        })
        .route("GET", "/health", |_head, _body| {
            Response::new(200, b"ok\n".to_vec())
        });

    let server = Server::new().handler(router);
    let bound = server.bind("127.0.0.1:3000").await?;
    println!("listening on http://{}", bound.local_addr()?);
    bound.run().await
}
