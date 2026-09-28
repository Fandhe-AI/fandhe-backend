//! 接続元アドレス受け渡し（`handle_upgrade_with_peer_addr`、イシュー #728）
//! の統合テスト。
//!
//! `on_close_e2e.rs` と同様に `tokio::io::duplex` + `tokio-tungstenite`
//! クライアントで実ハンドシェイクを駆動する。`peer_addr` は
//! `crates/core/src/plugin.rs` の `try_handle_upgrade` が accept したソケット
//! から得た値をそのまま渡すだけの非公開実装だが、本クレート単体では
//! duplex 上で任意の `Some(SocketAddr)` / `None` を注入して契約を検証する。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome,
};
use fandhe_backend_plugin_websocket::{
    WebSocketConfig, handle_upgrade, handle_upgrade_with_peer_addr,
};
use futures_util::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列
/// （`on_close_e2e.rs` と同一のリクエスト）。
fn handshake_request_bytes() -> &'static [u8] {
    b"GET /ws HTTP/1.1\r\n\
      Host: example.com\r\n\
      Upgrade: websocket\r\n\
      Connection: Upgrade\r\n\
      Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
      Sec-WebSocket-Version: 13\r\n\
      \r\n"
}

fn parse_head() -> fandhe_backend_http::request::RequestHead {
    match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    }
}

/// クライアント側ストリームから `\r\n\r\n` までを読み切る
/// （`on_close_e2e.rs` と同一のヘルパー）。
async fn read_http_response_line<S: AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).await.expect("read response byte");
        assert_ne!(n, 0, "stream closed before response terminator");
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buf).expect("response must be valid utf-8")
}

/// `on_open` で観測した `peer_addr()` を 1 回だけ記録するハンドラ。
struct RecordPeerAddr {
    observed: Arc<Mutex<Option<Option<SocketAddr>>>>,
}

impl WsMessageHandler for RecordPeerAddr {
    fn name(&self) -> &'static str {
        "record-peer-addr"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        *self.observed.lock().unwrap() = Some(ctx.peer_addr());
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

/// T4（AC2）: 既存 `handle_upgrade`（`peer_addr` を渡さない後方互換経路）は
/// `on_open` の `peer_addr()` が常に `None` になること。
#[tokio::test]
async fn handle_upgrade_legacy_yields_none_peer_addr() {
    let head = parse_head();
    let observed = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(RecordPeerAddr {
        observed: Arc::clone(&observed),
    });

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
        )
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client.close(None).await.expect("client close should send");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok(), "session should end normally: {result:?}");

    assert_eq!(
        *observed.lock().unwrap(),
        Some(None),
        "handle_upgrade (legacy) must always yield peer_addr() == None"
    );
}

/// T5: `handle_upgrade_with_peer_addr` に `Some(addr)` を渡すと、`on_open`
/// の `peer_addr()` がその値をそのまま観測できること。
#[tokio::test]
async fn handle_upgrade_with_peer_addr_observed_by_on_open() {
    let head = parse_head();
    let observed = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(RecordPeerAddr {
        observed: Arc::clone(&observed),
    });
    let injected: SocketAddr = "203.0.113.7:54321".parse().unwrap();

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            Some(injected),
        )
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client.close(None).await.expect("client close should send");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok(), "session should end normally: {result:?}");

    assert_eq!(
        *observed.lock().unwrap(),
        Some(Some(injected)),
        "handle_upgrade_with_peer_addr must pass the injected addr through to on_open"
    );
}

/// T6: ハンドシェイク失敗（`Sec-WebSocket-Version` 不一致 → 426）では
/// `on_open` 自体が呼ばれないこと（フェイルクローズ契約は `peer_addr` の
/// 有無に関わらず不変）。
#[tokio::test]
async fn handle_upgrade_with_peer_addr_handshake_failure_skips_on_open() {
    let bad_request = b"GET /ws HTTP/1.1\r\n\
        Host: example.com\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
        Sec-WebSocket-Version: 12\r\n\
        \r\n";
    let head = match parse_request_head(bad_request).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let observed = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(RecordPeerAddr {
        observed: Arc::clone(&observed),
    });
    let injected: SocketAddr = "203.0.113.7:54321".parse().unwrap();

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            Some(injected),
        )
        .await
    });

    // 426 応答を読み切ってからクライアント側を drop する（サーバタスクが
    // 書き込み完了を待たずに終了するのを防ぐ）。
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = client_side
            .read(&mut byte)
            .await
            .expect("read response byte");
        if n == 0 {
            break;
        }
        buf.push(byte[0]);
    }
    let response = String::from_utf8(buf).unwrap();
    assert!(response.starts_with("HTTP/1.1 426 Upgrade Required\r\n"));
    client_side.shutdown().await.ok();

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_err(), "handshake failure must return Err");

    assert_eq!(
        *observed.lock().unwrap(),
        None,
        "on_open must not be called when the handshake fails (fail-closed symmetry)"
    );
}
