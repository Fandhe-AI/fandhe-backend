//! `static` feature（イシュー #318）配線の陰性対照テスト（feature 無効側）。
//!
//! feature を有効化していない既定構成では `fandhe-backend-plugin-static` 自体が
//! 依存グラフに存在せず、`Server::static_files` API も生えない。
//! `crate::plugin::try_intercept` の static 分岐は cfg で消え、`/static/*`
//! への `GET` も既定 `Handler`（未登録時 404）へそのままフォールスルーする
//! ことを確認する。feature 有効側のテストは `plugin_static_boundary.rs` を参照。

#![cfg(not(feature = "static"))]

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
async fn static_prefixed_path_falls_through_to_default_handler_when_feature_disabled() {
    let server = Server::new().handler(FixedOkHandler);
    let request = b"GET /static/app.js HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("ok"));
}

#[tokio::test]
async fn static_prefixed_path_returns_404_without_handler_when_feature_disabled() {
    let server = Server::new();
    let request = b"GET /static/app.js HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}
