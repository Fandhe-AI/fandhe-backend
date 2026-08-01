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

use fandhe_backend_core::streaming::StreamingResponse;
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

/// `handle_streaming`（#319）を持つトイハンドラ（イシュー #461 の
/// 陰性対照: `compression` feature 無効時はストリーミング応答も無改変で
/// あることを確認する）。
struct StreamingTextHandler;
impl Handler for StreamingTextHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        Box::pin(std::future::ready(Response::empty(599)))
    }

    fn handle_streaming(&self, _head: &RequestHead, _body: &[u8]) -> Option<StreamingResponse> {
        let (response, writer) = StreamingResponse::channel(200, Some("text/plain"), 4);
        tokio::spawn(async move {
            let _ = writer.send("x".repeat(2048).into_bytes()).await;
            let _ = writer.finish().await;
        });
        Some(response)
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

#[tokio::test]
async fn streaming_response_is_unmodified_when_feature_disabled() {
    // イシュー #461: `compression` feature 無効時は `crate::plugin::
    // prepare_streaming_compression` が identity のまま（`StreamingBodyEncoder`
    // の cfg 分岐、`crates/core/src/plugin.rs` の doc を参照）で、chunked
    // ストリーミング応答が無改変で届くことを確認する。
    let server = Server::new().handler(StreamingTextHandler);
    let request = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("Transfer-Encoding: chunked\r\n"));
    assert!(!response.contains("Content-Encoding"));
    let expected_chunk_size = format!("{:x}", "x".repeat(2048).len());
    assert!(response.contains(&format!("{expected_chunk_size}\r\n")));
    assert!(response.contains(&"x".repeat(2048)));
    assert!(response.ends_with("0\r\n\r\n"));
}
