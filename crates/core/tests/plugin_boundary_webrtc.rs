//! `webrtc` feature（TASK-8.1 / #26）配線の統合テスト（feature 有効側）。
//!
//! `crates/core/src/plugin.rs` の非公開 `try_intercept` シームが実際に
//! `bf_plugin_webrtc::try_handle_rtc_offer` へ委譲し、`POST /rtc/offer` が
//! 既定 `Handler` より先にインターセプトされることを、`tokio::io::duplex` で
//! 駆動する `handle_connection` を通して検証する。無関係パスは素通りして既定
//! `Handler` に到達することも併せて確認する
//! （`crates/core/tests/plugin_boundary.rs`（`webrtc-proxy` 側）と同型のテスト、
//! `docs/design/plugin-boundary.md` の検証観点）。
//!
//! 実データチャネル疎通の検証は `crates/plugin-webrtc/tests/webrtc_datachannel.rs`
//! に委ね（core に `webrtc-rs` 由来の dev-dep を持ち込まない）、本テストは
//! 「コアの配線がプラグインを正しくインターセプトすること」の証跡に責務を限定する。
//!
//! feature 無効時の陰性対照は `plugin_boundary_disabled.rs` を参照。

#![cfg(feature = "webrtc")]

use backend_framework_core::{Handler, Server, handle_connection};
use bf_http::request::RequestHead;
use bf_http::response::Response;
use bf_plugin_webrtc::WebRtcConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `Handler::handle` が呼ばれたら panic するトイハンドラ。
///
/// `plugin::try_intercept` が `Some` を返した場合は既定 `Handler` を呼ばない契約
/// （`crates/core/src/server.rs` の `handle_connection` を参照）の証跡に使う:
/// このハンドラが呼ばれずにテストが通ることが「プラグインが処理を完結させた」
/// ことの直接的な確認になる。
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

/// `tokio::io::duplex` でソケット不要に `handle_connection` を駆動し、
/// クライアント側が受け取った生バイト列を文字列化して返す。
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
async fn intercepted_rtc_offer_bypasses_default_handler() {
    // 不正 JSON の Offer を送る: webrtc-rs の重い初期化（MediaEngine 等）を
    // 走らせずに「プラグインが処理を完結させた」証跡（400・NotCalledHandler 未呼び出し）
    // だけを軽量に確認する（実データチャネル疎通は plugin 側テストが担う）。
    let server = Server::new()
        .webrtc(WebRtcConfig::new())
        .handler(NotCalledHandler);

    let body = b"not json";
    let request = format!(
        "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        std::str::from_utf8(body).unwrap()
    );
    let response = roundtrip(&server, request.as_bytes()).await;

    // ステータス・Content-Type・body の全件を検証する（PoC-9 教訓:
    // ステータスのみの検証は reason/Content-Type/body の劣化を見逃す。
    // `crates/plugin-webrtc/src/handler.rs` のテストコメントと同一原則）。
    assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    assert!(response.contains("Content-Type: application/json\r\n"));
    assert!(response.ends_with(r#"{"error":"invalid_offer_json"}"#));
}

#[tokio::test]
async fn unrelated_path_falls_through_to_default_handler() {
    let server = Server::new()
        .webrtc(WebRtcConfig::new())
        .handler(FixedOkHandler);

    let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("ok"));
}
