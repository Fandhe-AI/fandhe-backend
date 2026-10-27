//! ゲートの検証結果（`RequestExtensions`）が `on_open` /
//! `on_message_with_ctx` / `on_close` の 3 箇所で読めることの統合テスト
//! （イシュー #934）。
//!
//! ゲートの代わりに `RequestHead::extensions_mut` で値を入れ、
//! `tokio::io::duplex` で `handle_upgrade_with_peer_addr` を駆動する。
//! 実 TCP（`Server::gate` + `Server::websocket`）での確認は別イシュー
//! （#935）の担当。判定はサーバタスクの join 後に行う。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    CloseReason, WsConnContext, WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext,
    WsOutcome,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade_with_peer_addr};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

#[derive(Debug)]
struct Marker(&'static str);

fn parse_head() -> RequestHead {
    let buf = b"GET /ws HTTP/1.1\r\n\
      Host: example.com\r\n\
      Upgrade: websocket\r\n\
      Connection: Upgrade\r\n\
      Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
      Sec-WebSocket-Version: 13\r\n\
      \r\n";
    match parse_request_head(buf).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    }
}

async fn read_http_response<S: AsyncRead + Unpin>(stream: &mut S) {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        let n = stream.read(&mut byte).await.expect("read");
        assert_ne!(n, 0, "closed before 101 terminator");
        buf.push(byte[0]);
    }
    assert!(buf.starts_with(b"HTTP/1.1 101 "));
}

/// 各フックで観測した値（呼ばれたフックのみ `Some`）。
#[derive(Default)]
struct Seen {
    open: Option<Option<&'static str>>,
    message: Option<Option<&'static str>>,
    close: Option<Option<&'static str>>,
}

struct Recorder(Arc<Mutex<Seen>>);

fn read(ctx_ext: &fandhe_backend_http::extensions::RequestExtensions) -> Option<&'static str> {
    ctx_ext.get::<Marker>().map(|m| m.0)
}

impl WsMessageHandler for Recorder {
    fn name(&self) -> &'static str {
        "recorder"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        self.0.lock().unwrap().open = Some(read(ctx.extensions()));
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }

    fn on_message_with_ctx<'a>(
        &'a self,
        ctx: &'a WsConnContext,
        msg: WsMessage,
    ) -> BoxFuture<'a, Result<WsOutcome, WsHandlerError>> {
        self.0.lock().unwrap().message = Some(read(ctx.extensions()));
        self.on_message(msg)
    }

    fn on_close(&self, ctx: &WsConnContext, _reason: CloseReason) {
        self.0.lock().unwrap().close = Some(read(ctx.extensions()));
    }
}

/// 1 接続を完走させる（1 メッセージ往復後にクライアント Close）。
async fn run_connection(head: RequestHead, config: Arc<WebSocketConfig>) {
    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            None,
        )
        .await
    });
    read_http_response(&mut client_side).await;
    let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client.send(Message::Text("hi".into())).await.unwrap();
    let echoed = client.next().await.expect("echo").expect("frame");
    assert_eq!(echoed, Message::Text("hi".into()));
    client.close(None).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish")
        .expect("no panic");
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn gate_value_observable_at_open_message_and_close() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let config = Arc::new(WebSocketConfig::default().with_handler(Recorder(seen.clone())));
    let mut head = parse_head();
    head.extensions_mut().insert(Marker("tenant-a")).unwrap();
    run_connection(head, config).await;

    let s = seen.lock().unwrap();
    assert_eq!(s.open, Some(Some("tenant-a")), "on_open");
    assert_eq!(s.message, Some(Some("tenant-a")), "on_message_with_ctx");
    assert_eq!(s.close, Some(Some("tenant-a")), "on_close");
}

#[tokio::test]
async fn no_gate_value_means_empty_at_all_phases() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let config = Arc::new(WebSocketConfig::default().with_handler(Recorder(seen.clone())));
    run_connection(parse_head(), config).await;

    let s = seen.lock().unwrap();
    assert_eq!(s.open, Some(None));
    assert_eq!(s.message, Some(None));
    assert_eq!(s.close, Some(None));
}

#[tokio::test]
async fn value_does_not_leak_to_next_connection() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    // 同じ設定（同じハンドラインスタンス）を 2 接続で使い回す。
    let config = Arc::new(WebSocketConfig::default().with_handler(Recorder(seen.clone())));

    let mut first = parse_head();
    first.extensions_mut().insert(Marker("first")).unwrap();
    run_connection(first, Arc::clone(&config)).await;
    assert_eq!(seen.lock().unwrap().open, Some(Some("first")));

    *seen.lock().unwrap() = Seen::default();
    run_connection(parse_head(), config).await;
    let s = seen.lock().unwrap();
    assert_eq!(s.open, Some(None));
    assert_eq!(s.message, Some(None));
    assert_eq!(s.close, Some(None));
}
