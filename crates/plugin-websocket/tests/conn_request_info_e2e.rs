//! 接続コンテキストへの接続元アドレス・主要リクエストヘッダ・query の展開
//! （イシュー #717、親 #715）の統合テスト。
//!
//! `peer_addr_e2e.rs` / `on_close_e2e.rs` と同様、`tokio::io::duplex` +
//! `tokio-tungstenite` クライアントで `handle_upgrade` /
//! `handle_upgrade_with_peer_addr` を実際に駆動する。
//!
//! 検証観点（受け入れ基準対応）:
//!
//! 1. `on_open`・`on_message_with_ctx`・`on_close` の 3 箇所すべてで、
//!    `WsOpenContext` / `WsConnContext` から同一の値
//!    （`peer_addr`/`host`/`origin`/`user_agent`/`query`）を観測できること
//!    → `all_fields_observable_at_open_message_and_close`
//! 2. 旧 API `handle_upgrade`（`peer_addr` を渡さない）・ヘッダ/query 欠落の
//!    リクエストでは、対応する値がすべて `None` になること
//!    → `legacy_api_and_missing_headers_yield_none`
//! 3. 上限（`MAX_CONTEXT_HEADER_VALUE_BYTES`）超過の `Origin` は切り詰め
//!    られず `None` になり、他の項目は正常に取得できること
//!    → `over_limit_header_becomes_none_others_unaffected`
//! 4. ハンドシェイク失敗（400 応答）では `on_open`/`on_close` 自体が
//!    呼ばれないこと（フェイルクローズの対称性、`on_close_e2e.rs` と同型）
//!    → `handshake_failure_skips_open_and_close`

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    CloseReason, WsConnContext, WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext,
    WsOutcome,
};
use fandhe_backend_plugin_websocket::{
    WebSocketConfig, handle_upgrade, handle_upgrade_with_peer_addr,
};
use futures_util::SinkExt;
use futures_util::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

/// クライアント側ストリームから `\r\n\r\n` までを読み切る
/// （`peer_addr_e2e.rs` と同一のヘルパー）。
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

/// `101 Switching Protocols` 応答の期待バイト列（固定テンプレート、
/// `crates/plugin-websocket/src/handshake.rs::serialize_101`）。本ファイルの
/// 全テストは `Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==`（RFC 6455 4.2.2
/// の既知ベクタ）で統一しているため、`Sec-WebSocket-Accept` の導出値
/// （`s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`）も固定できる。
const EXPECTED_101_RESPONSE: &str = "HTTP/1.1 101 Switching Protocols\r\n\
    Upgrade: websocket\r\n\
    Connection: Upgrade\r\n\
    Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\
    \r\n";

/// `400 Bad Request` 応答の期待バイト列（固定テンプレート、
/// `crates/plugin-websocket/src/handshake.rs::serialize_400`）。
const EXPECTED_400_RESPONSE: &str =
    "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";

/// 101 応答直後に想定外のバイト列（ボディ相当）が続いていないことを確認する。
/// `WsMessageHandler`（本テストの `RecordAllPhases`）はメッセージ受信まで
/// サーバー起点で能動送信しないため、クライアントが何も送っていない時点で
/// サーバー側ストリームから読み取りを試みれば必ずタイムアウトする契約
/// （AGENTS.md「アサーション網羅性」節: ステータス行・ヘッダだけでなく
/// ボディ〔ここでは「空である」こと〕まで確認する）。
async fn assert_no_immediate_body<S: AsyncRead + Unpin>(stream: &mut S) {
    let mut probe = [0u8; 1];
    let result = tokio::time::timeout(Duration::from_millis(200), stream.read(&mut probe)).await;
    assert!(
        result.is_err(),
        "101 response must have an empty body: no bytes should arrive before any WS frame \
         is sent by the client"
    );
}

fn parse_head(buf: &[u8]) -> RequestHead {
    match parse_request_head(buf).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!("test fixture must be a complete head"),
    }
}

/// 観測結果（`peer_addr`/`host`/`origin`/`user_agent`/`query`）のスナップショット。
/// `on_open` / `on_message_with_ctx` / `on_close` の 3 箇所から同一の形で
/// 記録するために揃えた。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    peer_addr: Option<SocketAddr>,
    host: Option<String>,
    origin: Option<String>,
    user_agent: Option<String>,
    query: Option<String>,
}

impl Observed {
    fn from_open(ctx: &WsOpenContext) -> Self {
        Self {
            peer_addr: ctx.peer_addr(),
            host: ctx.host().map(str::to_string),
            origin: ctx.origin().map(str::to_string),
            user_agent: ctx.user_agent().map(str::to_string),
            query: ctx.query().map(str::to_string),
        }
    }

    fn from_conn(ctx: &WsConnContext) -> Self {
        Self {
            peer_addr: ctx.peer_addr(),
            host: ctx.host().map(str::to_string),
            origin: ctx.origin().map(str::to_string),
            user_agent: ctx.user_agent().map(str::to_string),
            query: ctx.query().map(str::to_string),
        }
    }
}

/// `on_open`・`on_message_with_ctx`・`on_close` それぞれで観測した
/// [`Observed`] を記録するハンドラ。
struct RecordAllPhases {
    open: Arc<Mutex<Option<Observed>>>,
    message: Arc<Mutex<Option<Observed>>>,
    close: Arc<Mutex<Option<Observed>>>,
}

impl WsMessageHandler for RecordAllPhases {
    fn name(&self) -> &'static str {
        "record-all-phases"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        *self.open.lock().unwrap() = Some(Observed::from_open(&ctx));
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }

    fn on_message_with_ctx<'a>(
        &'a self,
        ctx: &'a WsConnContext,
        msg: WsMessage,
    ) -> BoxFuture<'a, Result<WsOutcome, WsHandlerError>> {
        *self.message.lock().unwrap() = Some(Observed::from_conn(ctx));
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }

    fn on_close(&self, ctx: &WsConnContext, _reason: CloseReason) {
        *self.close.lock().unwrap() = Some(Observed::from_conn(ctx));
    }
}

/// (a) 全項目が観測できる経路: Host / Origin / User-Agent と
/// `?token=abc` を含むハンドシェイクを `handle_upgrade_with_peer_addr` で
/// 駆動し、`on_open`・`on_message_with_ctx`・`on_close` の 3 箇所で全項目が
/// 同じ値になることを確認する（受け入れ基準 1）。
#[tokio::test]
async fn all_fields_observable_at_open_message_and_close() {
    let request = b"GET /ws?token=abc HTTP/1.1\r\n\
        Host: example.com\r\n\
        Origin: https://example.com\r\n\
        User-Agent: test-client/1.0\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
        Sec-WebSocket-Version: 13\r\n\
        \r\n";
    let head = parse_head(request);
    let peer_addr: SocketAddr = "203.0.113.42:54321".parse().unwrap();

    let open = Arc::new(Mutex::new(None));
    let message = Arc::new(Mutex::new(None));
    let close = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(RecordAllPhases {
        open: Arc::clone(&open),
        message: Arc::clone(&message),
        close: Arc::clone(&close),
    });

    let (server_side, mut client_side) = tokio::io::duplex(8192);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            Some(peer_addr),
        )
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert_eq!(
        response, EXPECTED_101_RESPONSE,
        "101 response must match the exact status line, headers, and empty body"
    );
    assert_no_immediate_body(&mut client_side).await;

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    // `on_message_with_ctx` を経由させるため、1 通送って echo 応答を待つ。
    client
        .send(Message::text("hello"))
        .await
        .expect("client send should succeed");
    use futures_util::StreamExt;
    let echoed = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("echo should arrive within timeout")
        .expect("stream should not end")
        .expect("frame should not error");
    assert_eq!(echoed.into_text().unwrap(), "hello");

    client.close(None).await.expect("client close should send");
    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok(), "session should end normally: {result:?}");

    let expected = Observed {
        peer_addr: Some(peer_addr),
        host: Some("example.com".to_string()),
        origin: Some("https://example.com".to_string()),
        user_agent: Some("test-client/1.0".to_string()),
        query: Some("token=abc".to_string()),
    };
    assert_eq!(
        open.lock().unwrap().clone(),
        Some(expected.clone()),
        "on_open must observe all 5 fields"
    );
    assert_eq!(
        message.lock().unwrap().clone(),
        Some(expected.clone()),
        "on_message_with_ctx must observe the same 5 fields as on_open"
    );
    assert_eq!(
        close.lock().unwrap().clone(),
        Some(expected),
        "on_close must observe the same 5 fields as on_open"
    );
}

/// (b) 旧 API とヘッダ欠落の経路: `handle_upgrade`（`peer_addr` を渡さない
/// 後方互換ラッパー）で、Origin・User-Agent・query を送らずに接続すると、
/// 両コンテキストの該当フィールドがすべて `None` になること。
#[tokio::test]
async fn legacy_api_and_missing_headers_yield_none() {
    let request = b"GET /ws HTTP/1.1\r\n\
        Host: example.com\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
        Sec-WebSocket-Version: 13\r\n\
        \r\n";
    let head = parse_head(request);

    let open = Arc::new(Mutex::new(None));
    let message = Arc::new(Mutex::new(None));
    let close = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(RecordAllPhases {
        open: Arc::clone(&open),
        message: Arc::clone(&message),
        close: Arc::clone(&close),
    });

    let (server_side, mut client_side) = tokio::io::duplex(8192);
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
    assert_eq!(
        response, EXPECTED_101_RESPONSE,
        "101 response must match the exact status line, headers, and empty body"
    );
    assert_no_immediate_body(&mut client_side).await;

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client.close(None).await.expect("client close should send");
    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok(), "session should end normally: {result:?}");

    let observed_open = open
        .lock()
        .unwrap()
        .clone()
        .expect("on_open must be called");
    assert_eq!(observed_open.peer_addr, None);
    assert_eq!(observed_open.origin, None);
    assert_eq!(observed_open.user_agent, None);
    assert_eq!(observed_open.query, None);
    // `Host` は送信しているため観測できる（許可リストの他項目とは無関係に
    // 独立して欠落しうることの確認）。
    assert_eq!(observed_open.host.as_deref(), Some("example.com"));

    let observed_close = close
        .lock()
        .unwrap()
        .clone()
        .expect("on_close must be called");
    assert_eq!(observed_close, observed_open);
}

/// (c) 上限超過の経路: `MAX_CONTEXT_HEADER_VALUE_BYTES` を超える `Origin`
/// を送ると、`origin()` は両コンテキストで `None` になり、他の項目
/// （`Host`）は正常に取得できること（フェイルクローズ・非切り詰め契約）。
#[tokio::test]
async fn over_limit_header_becomes_none_others_unaffected() {
    use fandhe_backend_plugin_websocket::handler::MAX_CONTEXT_HEADER_VALUE_BYTES;

    let over_limit_origin = "a".repeat(MAX_CONTEXT_HEADER_VALUE_BYTES + 1);
    let request = format!(
        "GET /ws HTTP/1.1\r\n\
         Host: example.com\r\n\
         Origin: {over_limit_origin}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n"
    );
    let head = parse_head(request.as_bytes());

    let open = Arc::new(Mutex::new(None));
    let message = Arc::new(Mutex::new(None));
    let close = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(RecordAllPhases {
        open: Arc::clone(&open),
        message: Arc::clone(&message),
        close: Arc::clone(&close),
    });

    let (server_side, mut client_side) = tokio::io::duplex(8192 + over_limit_origin.len());
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
    assert_eq!(
        response, EXPECTED_101_RESPONSE,
        "101 response must match the exact status line, headers, and empty body"
    );
    assert_no_immediate_body(&mut client_side).await;

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client.close(None).await.expect("client close should send");
    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok(), "session should end normally: {result:?}");

    let observed = open
        .lock()
        .unwrap()
        .clone()
        .expect("on_open must be called");
    assert_eq!(
        observed.origin, None,
        "over-limit Origin must become None, not be truncated"
    );
    assert_eq!(observed.host.as_deref(), Some("example.com"));
}

/// (d) ハンドシェイク失敗（400）の経路: `on_open`/`on_close` 自体が呼ばれず、
/// 情報も観測されないこと（フェイルクローズの対称性、`on_close_e2e.rs` と
/// 同型の検証）。
#[tokio::test]
async fn handshake_failure_skips_open_and_close() {
    // `Sec-WebSocket-Key` を欠落させ 400 を誘発する。
    let bad_request = b"GET /ws HTTP/1.1\r\n\
        Host: example.com\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Version: 13\r\n\
        \r\n";
    let head = parse_head(bad_request);

    let open = Arc::new(Mutex::new(None));
    let message = Arc::new(Mutex::new(None));
    let close = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(RecordAllPhases {
        open: Arc::clone(&open),
        message: Arc::clone(&message),
        close: Arc::clone(&close),
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
    assert_eq!(
        response, EXPECTED_400_RESPONSE,
        "400 response must match the exact status line, headers, and empty body"
    );

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_err(), "handshake failure must return Err");

    assert_eq!(*open.lock().unwrap(), None, "on_open must not be called");
    assert_eq!(
        *message.lock().unwrap(),
        None,
        "on_message_with_ctx must not be called"
    );
    assert_eq!(*close.lock().unwrap(), None, "on_close must not be called");
}
