//! `openapi` feature（TASK-2.1 / #256）配線の陰性対照テスト（feature 無効側）。
//!
//! feature を有効化していない既定構成では `fandhe-backend-plugin-openapi` 自体が
//! 依存グラフに存在せず、`GET /openapi.json` / `GET /openapi.yaml`（#279）への
//! リクエストは非公開 `plugin::try_intercept` を素通りして既定 `Handler`
//! （未登録時は 404）へフォールスルーすることを確認する。feature 有効側の
//! テストは `plugin_openapi_boundary.rs` を参照。

#![cfg(not(feature = "openapi"))]

use fandhe_backend_core::{Server, handle_connection};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
async fn openapi_json_path_is_404_when_feature_disabled() {
    let server = Server::new();
    let request = b"GET /openapi.json HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}

#[tokio::test]
async fn openapi_yaml_path_is_404_when_feature_disabled() {
    let server = Server::new();
    let request = b"GET /openapi.yaml HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}
