//! `websocket` feature（TASK-4.1 / #22）配線の陰性対照テスト（feature 無効側）。
//!
//! feature を有効化していない既定構成では `Server::websocket` 自体が存在
//! せず、`crate::plugin::try_handle_upgrade` は常に `Some(stream)` を返す
//! スタブのままである。`UpgradeHandler` がマッチした接続については、
//! feature 有効化前（TASK-1.4-2 / #70）と同じ 501 フォールバック挙動が
//! 不変であることを、`extension.rs` の doc test と同型のトイ
//! `UpgradeHandler`（`Upgrade: websocket` ヘッダの有無のみを見る）を使って
//! 確認する。feature 有効側のテストは `websocket_upgrade.rs` を参照。

#![cfg(not(feature = "websocket"))]

use fandhe_backend_core::{Server, UpgradeHandler, handle_connection};
use fandhe_backend_http::request::RequestHead;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `Upgrade: websocket` ヘッダの有無だけを見るトイ `UpgradeHandler`
/// （`crates/core/src/extension.rs` の doc test と同一の最小実装）。
struct ToyWebSocketUpgrade;
impl UpgradeHandler for ToyWebSocketUpgrade {
    fn name(&self) -> &'static str {
        "toy-websocket-upgrade"
    }
    fn matches(&self, head: &RequestHead) -> bool {
        head.header("upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
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
async fn matched_upgrade_falls_back_to_501_when_feature_disabled() {
    let server = Server::new().upgrade_handler(ToyWebSocketUpgrade);
    let request = b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 501"));
}

#[tokio::test]
async fn websocket_path_is_404_without_any_upgrade_handler_when_feature_disabled() {
    let server = Server::new();
    let request = b"GET /ws HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}
