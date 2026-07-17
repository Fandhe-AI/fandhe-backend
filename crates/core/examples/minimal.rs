//! `crates/core` コアループの手動動作確認用最小サーバ（TASK-1.4-2 / #70、
//! TASK-1.5 / #14 で `bf_routes::Router` を使う構成に更新）。
//!
//! [`backend_framework_core::Server`] に既定ハンドラとして
//! [`bf_routes::Router`] を 1 件登録した最小構成で
//! `cargo run --example minimal -p backend-framework-core` から起動できる
//! ことを確認する。`Router` は `backend_framework_core::server` の
//! `impl Handler for bf_routes::Router` 経由でそのまま `Server::handler` に
//! 渡せる（`server → routes` 依存の実利用例）。
//!
//! 動作確認手順:
//! ```text
//! $ cargo run --example minimal -p backend-framework-core
//! $ curl -v http://127.0.0.1:3000/           # 200 応答
//! $ curl -v -X POST http://127.0.0.1:3000/    # 405 応答（/ は GET のみ登録）
//! $ curl -v http://127.0.0.1:3000/missing     # 404 応答（未登録パス）
//! $ curl -v -H 'Connection: close' http://127.0.0.1:3000/  # 応答後に接続クローズ
//! ```

use backend_framework_core::Server;
use bf_http::response::Response;
use bf_routes::Router;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let router = Router::new().route("GET", "/", |_head, _body| {
        Response::new(200, b"backend-framework: minimal example\n".to_vec())
    });

    let server = Server::new().handler(router);
    let bound = server.bind("127.0.0.1:3000").await?;
    println!("listening on http://{}", bound.local_addr()?);
    bound.run().await
}
