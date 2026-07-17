//! `crates/core` コアループの手動動作確認用最小サーバ（TASK-1.4-2 / #70）。
//!
//! `crates/routes` が存在しない現時点では、[`backend_framework_core::Server`]
//! に既定ハンドラ（[`backend_framework_core::Handler`]）を 1 件だけ登録した
//! 最小構成で `cargo run --example minimal -p backend-framework-core` から
//! 起動できることを確認する。
//!
//! 動作確認手順:
//! ```text
//! $ cargo run --example minimal -p backend-framework-core
//! $ curl -v http://127.0.0.1:3000/           # 200 応答
//! $ curl -v http://127.0.0.1:3000/missing     # 404 応答（既定ハンドラが / 以外を拒否）
//! $ curl -v -H 'Connection: close' http://127.0.0.1:3000/  # 応答後に接続クローズ
//! ```

use backend_framework_core::{Handler, Server};
use bf_http::request::RequestHead;
use bf_http::response::Response;

/// `/` のみを許可する最小限のトイハンドラ。
struct EchoRootHandler;

impl Handler for EchoRootHandler {
    fn handle(&self, head: &RequestHead, _body: &[u8]) -> Response {
        if head.target == "/" {
            Response::new(200, b"backend-framework: minimal example\n".to_vec())
        } else {
            Response::empty(404)
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let server = Server::new().handler(EchoRootHandler);
    let bound = server.bind("127.0.0.1:3000").await?;
    println!("listening on http://{}", bound.local_addr()?);
    bound.run().await
}
