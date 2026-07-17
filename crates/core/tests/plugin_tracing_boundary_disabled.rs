//! `tracing` feature（TASK-10.1 / #56）配線の陰性対照テスト（feature 無効側）。
//!
//! `tracing` は他プラグイン（`webrtc-proxy`・`graphql` 等）と異なりパス
//! インターセプト型ではなく、`Server::tracing` 自体が feature 限定 API のため
//! feature 無効時はメソッドがコンパイル対象から消える（コンパイル時に検証済み。
//! 本ファイルは feature を呼べないことの代わりに、`tracing` feature 無効構成でも
//! 通常のリクエスト処理（`Middleware` 拡張点を一切使わない経路）が従来どおり
//! 動作し続けることを確認する）。feature 有効側のテストは
//! `plugin_tracing_boundary.rs` を参照。

#![cfg(not(feature = "tracing"))]

use backend_framework_core::{Handler, Server, handle_connection};
use bf_http::request::RequestHead;
use bf_http::response::Response;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct FixedOkHandler;
impl Handler for FixedOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        Response::new(200, b"ok".to_vec())
    }
}

#[tokio::test]
async fn requests_succeed_without_tracing_middleware_registered() {
    let server = Server::new().handler(FixedOkHandler);

    let (mut client, server_stream) = tokio::io::duplex(8192);
    let request = b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n";
    client.write_all(request).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(&server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    let response = String::from_utf8(out).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("ok"));
}
