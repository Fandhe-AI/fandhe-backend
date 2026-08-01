//! `webrtc-proxy` feature（TASK-2.1 / #18）配線の統合テスト（feature 有効側）。
//!
//! `crates/core/src/plugin.rs` の非公開 `try_intercept` シームが実際に
//! `fandhe_backend_plugin_webrtc_proxy::try_handle_rtc_offer` へ委譲し、`POST /rtc/offer`
//! が既定 `Handler` より先にインターセプトされることを、モック上流 TCP
//! サーバ + `tokio::io::duplex` で駆動する `handle_connection` を通して
//! 検証する。無関係パスは素通りして既定 `Handler` に到達することも併せて
//! 確認する（`docs/design/plugin-boundary.md` の検証観点）。
//!
//! feature 無効時の陰性対照は `plugin_boundary_disabled.rs` を参照
//! （feature ごとにファイルを分けているのは、`ProxyConfig` 等の型自体が
//! feature 無効ビルドでは存在しないため）。

#![cfg(feature = "webrtc-proxy")]

use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_webrtc_proxy::ProxyConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// `Handler::handle` が呼ばれたら panic するトイハンドラ。
///
/// `plugin::try_intercept` が `Some` を返した場合は既定 `Handler` を呼ばない
/// 契約（`crates/core/src/server.rs` の `handle_connection` を参照）の証跡に
/// 使う: このハンドラが呼ばれずにテストが通ることが「プラグインが処理を
/// 完結させた」ことの直接的な確認になる。
struct NotCalledHandler;
impl Handler for NotCalledHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        panic!("plugin::try_intercept が Some を返したのに既定 Handler が呼ばれた");
    }
}

/// 固定 200 応答を返すだけのトイハンドラ（フォールスルー確認用）。
struct FixedOkHandler;
impl Handler for FixedOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        Box::pin(std::future::ready(Response::new(200, b"ok".to_vec())))
    }
}

/// SDP Answer 固定バイト列を返すモック上流 WebRTC シグナリングサーバ。
///
/// `fandhe_backend_plugin_webrtc_proxy::client::forward_offer` が中継する先として使う。
/// 本テストの関心は「コアが上流応答を最終応答へ変換して返すこと」であり、
/// 上流プロトコルの厳密なパースは `crates/plugin-webrtc-proxy` 側の責務
/// （既存テストで検証済み）のため、ここでは固定応答で十分とする。
async fn spawn_mock_upstream() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let body = b"answer-sdp";
            let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(body).await;
        }
    });
    addr
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
async fn intercepted_rtc_offer_bypasses_default_handler_and_forwards_upstream() {
    let upstream_addr = spawn_mock_upstream().await;
    let config = ProxyConfig::new(upstream_addr.to_string());
    let server = Server::new().webrtc_proxy(config).handler(NotCalledHandler);

    let body = b"offer-sdp";
    let request = format!(
        "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        std::str::from_utf8(body).unwrap()
    );
    let response = roundtrip(&server, request.as_bytes()).await;

    // ステータス・Content-Type・body の全件を検証する（PoC-9 教訓:
    // ステータスのみの検証は reason/Content-Type/body の劣化を見逃す。
    // `crates/plugin-webrtc-proxy/src/handler.rs` のテストコメントと同一原則）。
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("Content-Type: application/sdp\r\n"));
    assert!(response.ends_with("answer-sdp"));
}

#[tokio::test]
async fn unrelated_path_falls_through_to_default_handler() {
    // 上流に接続しないパスなので、上流アドレスが未起動でもテストに影響しない
    // （対象外パスは try_intercept 内で即 None、`crate::plugin` の doc を参照）。
    let config = ProxyConfig::new("127.0.0.1:1");
    let server = Server::new().webrtc_proxy(config).handler(FixedOkHandler);

    let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("ok"));
}

#[tokio::test]
async fn upstream_failure_returns_bad_gateway_with_reason_and_content_type() {
    // 上流未起動（127.0.0.1:1 は通常未リッスン）: 502/504 のいずれかへ丸められる
    // ことに加え、reason phrase が空文字へ劣化していないことを確認する
    // （TASK-2.1 レビュー指摘: fandhe_backend_http::response::Response の固定 reason
    // テーブルに 502/504 が欠けていると `HTTP/1.1 502 \r\n` になってしまう）。
    let config =
        ProxyConfig::new("127.0.0.1:1").with_connect_timeout(std::time::Duration::from_millis(200));
    let server = Server::new().webrtc_proxy(config).handler(NotCalledHandler);

    let body = b"offer-sdp";
    let request = format!(
        "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        std::str::from_utf8(body).unwrap()
    );
    let response = roundtrip(&server, request.as_bytes()).await;

    assert!(
        response.starts_with("HTTP/1.1 502 Bad Gateway\r\n")
            || response.starts_with("HTTP/1.1 504 Gateway Timeout\r\n"),
        "reason phrase が空文字に劣化していないことを含めて検証: {response:?}"
    );
    assert!(response.contains("Content-Type: text/plain; charset=utf-8\r\n"));
}

#[tokio::test]
async fn config_built_via_core_reexport_forwards_upstream() {
    // イシュー #435: `fandhe_backend_core::plugin_webrtc_proxy::ProxyConfig`
    // （プラグインクレートへの直接依存を追加しない再エクスポート経路）
    // 経由で構築した設定でも、直接依存経路（上のテスト）と同一の配線・
    // 応答になることを確認する（`plugin_static_boundary.rs` の
    // `config_built_via_core_reexport_serves_file` と同型パターン、
    // イシュー #421）。
    let upstream_addr = spawn_mock_upstream().await;
    let config =
        fandhe_backend_core::plugin_webrtc_proxy::ProxyConfig::new(upstream_addr.to_string());
    let server = Server::new().webrtc_proxy(config).handler(NotCalledHandler);

    let body = b"offer-sdp";
    let request = format!(
        "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        std::str::from_utf8(body).unwrap()
    );
    let response = roundtrip(&server, request.as_bytes()).await;

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("answer-sdp"));
}
