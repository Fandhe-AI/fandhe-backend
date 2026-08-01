//! `cors` feature（イシュー #305）配線の陰性対照テスト（feature 無効側）。
//!
//! feature を有効化していない既定構成では `fandhe-backend-plugin-cors` 自体が
//! 依存グラフに存在せず、`Server::cors` API も生えない。非公開シーム
//! `crate::plugin::finalize_response` は即座にレスポンスを無改変で返す
//! だけの薄い関数となり、`Origin` ヘッダ付きリクエストでも一切ヘッダを
//! 付与しないことを確認する。feature 有効側のテストは
//! `plugin_cors_boundary.rs` を参照。

#![cfg(not(feature = "cors"))]

use fandhe_backend_core::streaming::StreamingResponse;
use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct FixedOkHandler;
impl Handler for FixedOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        Box::pin(std::future::ready(Response::new(200, b"ok".to_vec())))
    }
}

/// `handle_streaming` で単一チャンクを返すトイハンドラ
/// （`plugin_cors_boundary.rs` の `StreamingOkHandler` と同型）。
struct StreamingOkHandler;
impl Handler for StreamingOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        Box::pin(std::future::ready(Response::empty(599)))
    }

    fn handle_streaming(&self, _head: &RequestHead, _body: &[u8]) -> Option<StreamingResponse> {
        let (response, writer) = StreamingResponse::channel(200, Some("text/plain"), 4);
        tokio::spawn(async move {
            let _ = writer.send(b"chunk".to_vec()).await;
            let _ = writer.finish().await;
        });
        Some(response)
    }
}

async fn roundtrip(server: &Server, request: &[u8]) -> String {
    let (mut client, server_stream) = tokio::io::duplex(8192);
    client.write_all(request).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

#[tokio::test]
async fn origin_header_is_ignored_when_feature_disabled() {
    let server = Server::new().handler(FixedOkHandler);
    let request = b"GET / HTTP/1.1\r\nOrigin: https://app.example.com\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(!response.contains("Access-Control-Allow-Origin"));
}

/// イシュー #451 の陰性対照: `cors` feature 無効時は `crate::plugin::
/// finalize_streaming_head` が薄い no-op となり、ストリーミング応答へも
/// `Origin` ヘッダ付きリクエストで CORS ヘッダが一切付かない
/// （pay-for-what-you-use）。
#[tokio::test]
async fn streaming_response_origin_header_is_ignored_when_feature_disabled() {
    let server = Server::new().handler(StreamingOkHandler);
    let request = b"GET / HTTP/1.1\r\nOrigin: https://app.example.com\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "response: {response}"
    );
    assert!(
        !response.contains("Access-Control-Allow-Origin"),
        "response: {response}"
    );
    assert!(response.contains("5\r\nchunk\r\n"), "response: {response}");
    assert!(response.ends_with("0\r\n\r\n"), "response: {response}");
}
