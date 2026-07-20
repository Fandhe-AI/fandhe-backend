//! `compression` feature（イシュー #321）配線の陰性対照テスト（feature 無効側）。
//!
//! feature を有効化していない既定構成では `fandhe-backend-plugin-compression`
//! 自体が依存グラフに存在せず、`Server::compression` API も生えない。非公開
//! シーム `crate::plugin::finalize_response` は即座にレスポンスを無改変で
//! 返すだけの薄い関数となり、`Accept-Encoding: gzip` 付きリクエストでも
//! 一切圧縮しないことを確認する。feature 有効側のテストは
//! `plugin_compression_boundary.rs` を参照（`plugin_cors_boundary_disabled.rs`
//! と同一パターン）。

#![cfg(not(feature = "compression"))]

use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct LargeTextHandler;
impl Handler for LargeTextHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        let body = "x".repeat(2048);
        Box::pin(std::future::ready(
            Response::new(200, body.into_bytes()).with_content_type("text/plain"),
        ))
    }
}

async fn roundtrip(server: &Server, request: &[u8]) -> String {
    let (mut client, server_stream) = tokio::io::duplex(65536);
    client.write_all(request).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

#[tokio::test]
async fn accept_encoding_header_is_ignored_when_feature_disabled() {
    let server = Server::new().handler(LargeTextHandler);
    let request = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(!response.contains("Content-Encoding"));
    assert!(!response.contains("Vary"));
    assert!(response.ends_with(&"x".repeat(2048)));
}
