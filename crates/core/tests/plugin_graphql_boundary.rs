//! `graphql` feature（TASK-2.4 / #21）配線の統合テスト（feature 有効側）。
//!
//! `crates/core/src/plugin.rs` の非公開 `try_intercept` シームが実際に
//! `bf_plugin_graphql::try_handle_graphql` へ委譲し、`POST /graphql` が既定
//! `Handler` より先にインターセプトされることを、`tokio::io::duplex` で駆動する
//! `handle_connection` を通して検証する。無関係パスは素通りして既定 `Handler`
//! に到達することも併せて確認する（`docs/design/plugin-boundary.md` の検証観点、
//! `crates/core/tests/plugin_boundary.rs` と同型のパターン）。
//!
//! feature 無効時の陰性対照は `plugin_graphql_boundary_disabled.rs` を参照。

#![cfg(feature = "graphql")]

use backend_framework_core::{Handler, Server, handle_connection};
use bf_http::request::RequestHead;
use bf_http::response::Response;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `Handler::handle` が呼ばれたら panic するトイハンドラ。
///
/// `plugin::try_intercept` が `Some` を返した場合は既定 `Handler` を呼ばない
/// 契約（`crates/core/src/server.rs` の `handle_connection` を参照）の証跡に使う。
struct NotCalledHandler;
impl Handler for NotCalledHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        panic!("plugin::try_intercept が Some を返したのに既定 Handler が呼ばれた");
    }
}

/// 固定 200 応答を返すだけのトイハンドラ（フォールスルー確認用）。
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
async fn intercepted_graphql_request_bypasses_default_handler() {
    let server = Server::new().handler(NotCalledHandler);

    let request = b"POST /graphql HTTP/1.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;

    // ステータス・Content-Type・body の全件を検証する（PoC-9 教訓:
    // ステータスのみの検証は reason/Content-Type/body の劣化を見逃す。
    // `crates/core/tests/plugin_boundary.rs` と同一原則）。
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("Content-Type: application/json\r\n"));
    assert!(response.ends_with("{\"data\":null}"));
}

#[tokio::test]
async fn unrelated_path_falls_through_to_default_handler() {
    let server = Server::new().handler(FixedOkHandler);

    let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("ok"));
}
