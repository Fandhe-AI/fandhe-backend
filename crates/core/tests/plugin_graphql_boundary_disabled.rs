//! `graphql` feature（TASK-2.4 / #21）配線の陰性対照テスト（feature 無効側）。
//!
//! feature を有効化していない既定構成では `bf-plugin-graphql` 自体が依存グラフに
//! 存在せず、`POST /graphql` へのリクエストは非公開 `plugin::try_intercept` を
//! 素通りして既定 `Handler`（未登録時は 404）へフォールスルーすることを確認する。
//! feature 有効側のテストは `plugin_graphql_boundary.rs` を参照。

#![cfg(not(feature = "graphql"))]

use backend_framework_core::{Server, handle_connection};
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
async fn graphql_path_is_404_when_feature_disabled() {
    let server = Server::new();
    let request = b"POST /graphql HTTP/1.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}
