//! `cors` feature（イシュー #305）配線の陰性対照テスト（feature 無効側）。
//!
//! feature を有効化していない既定構成では `fandhe-backend-plugin-cors` 自体が
//! 依存グラフに存在せず、`Server::cors` API も生えない。非公開シーム
//! `crate::plugin::finalize_response` は即座にレスポンスを無改変で返す
//! だけの薄い関数となり、`Origin` ヘッダ付きリクエストでも一切ヘッダを
//! 付与しないことを確認する。feature 有効側のテストは
//! `plugin_cors_boundary.rs` を参照。

#![cfg(not(feature = "cors"))]

use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct FixedOkHandler;
impl Handler for FixedOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        Response::new(200, b"ok".to_vec())
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
