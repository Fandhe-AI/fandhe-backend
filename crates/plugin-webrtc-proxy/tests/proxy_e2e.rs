//! `bf-plugin-webrtc-proxy` の統合テスト（TASK-8.2-2 / #74）。
//!
//! モック上流（`tokio::net::TcpListener` で SDP Answer を固定応答する簡易サーバ）
//! を各テスト内で起動し、[`try_handle_rtc_offer`] 経由の Offer → Answer 往復・
//! 上流ダウン時の 502 系応答・上流スロー応答時の 504 系応答を検証する。
//! 「別プロセスに切り出した WebRTC サービスとの連携が動作する」という
//! TASK-8.2 の受け入れ基準（本サブタスク範囲）を実証する目的で置く。

use std::time::Duration;

use bf_http::request::{ParseOutcome, RequestHead, parse_request_head};
use bf_plugin_webrtc_proxy::{ProxyConfig, try_handle_rtc_offer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// リクエストバイト列から [`RequestHead`] を作る（統合テスト用ヘルパ）。
fn parse_head(buf: &[u8]) -> RequestHead {
    match parse_request_head(buf).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => panic!("expected Complete"),
    }
}

/// SDP Answer を固定応答するモック上流サーバを起動し、`host:port` を返す。
async fn spawn_mock_upstream(answer: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = socket.read(&mut buf).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n",
            answer.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.write_all(answer).await;
    });
    addr
}

#[tokio::test]
async fn offer_answer_round_trip_via_mock_upstream() {
    let upstream_addr = spawn_mock_upstream(b"v=0\r\no=- 456 IN IP4 127.0.0.1\r\n").await;
    let config = ProxyConfig::new(upstream_addr);

    let offer_body = b"v=0\r\no=- 123 IN IP4 127.0.0.1\r\n";
    let head = parse_head(
        format!(
            "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            offer_body.len()
        )
        .as_bytes(),
    );

    let response = try_handle_rtc_offer(&head, offer_body, &config)
        .await
        .expect("path/method match should produce a response");

    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "application/sdp");
    assert_eq!(response.body, b"v=0\r\no=- 456 IN IP4 127.0.0.1\r\n");
}

#[tokio::test]
async fn upstream_down_yields_bad_gateway() {
    // 上流ポートを一度も bind しない（接続拒否を誘発）。
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener); // bind 解放直後は当該ポートが未リッスンになる。

    let config = ProxyConfig::new(addr).with_connect_timeout(Duration::from_millis(300));
    let offer_body = b"offer-body";
    let head = parse_head(
        format!(
            "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            offer_body.len()
        )
        .as_bytes(),
    );

    let response = try_handle_rtc_offer(&head, offer_body, &config)
        .await
        .expect("path/method match should produce a response");

    // 接続拒否は 502、環境によっては connect がタイムアウトして 504 になりうるため両方許容する。
    assert!(response.status == 502 || response.status == 504);
    assert_eq!(response.content_type, "text/plain; charset=utf-8");
}

#[tokio::test]
async fn slow_upstream_yields_gateway_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        // 応答を送らずソケットを保持し続け、スロー上流を模す。
        tokio::time::sleep(Duration::from_secs(10)).await;
        drop(socket);
    });

    let config = ProxyConfig::new(addr).with_request_timeout(Duration::from_millis(100));
    let offer_body = b"offer-body";
    let head = parse_head(
        format!(
            "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            offer_body.len()
        )
        .as_bytes(),
    );

    let response = try_handle_rtc_offer(&head, offer_body, &config)
        .await
        .expect("path/method match should produce a response");

    assert_eq!(response.status, 504);
}

#[tokio::test]
async fn unrelated_path_falls_through_with_none() {
    let config = ProxyConfig::new("127.0.0.1:9000");
    let head = parse_head(b"GET /healthz HTTP/1.1\r\n\r\n");
    assert!(try_handle_rtc_offer(&head, b"", &config).await.is_none());
}
