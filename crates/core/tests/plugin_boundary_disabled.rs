//! `webrtc-proxy` feature（TASK-2.1 / #18）配線の陰性対照テスト（feature 無効側）。
//!
//! feature を有効化していない既定構成では `Server::webrtc_proxy` 自体が
//! 存在せず、`POST /rtc/offer` へのリクエストは非公開 `plugin::try_intercept`
//! を素通りして既定 `Handler`（未登録時は 404）へフォールスルーすることを
//! 確認する。feature 有効側のテストは `plugin_boundary.rs` を参照。

#![cfg(not(feature = "webrtc-proxy"))]

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
async fn rtc_offer_path_is_404_when_feature_disabled() {
    let server = Server::new();
    let body = b"offer-sdp";
    let request = format!(
        "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        std::str::from_utf8(body).unwrap()
    );
    let response = roundtrip(&server, request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}
