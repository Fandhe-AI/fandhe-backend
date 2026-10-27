//! `RequestGate` が入れた検証結果が WebSocket の 4 箇所へ届くこと（イシュー
//! #935、`docs/design/gate-request-extensions.md` 14 節・
//! `docs/design/ws-connection-context-and-close.md` 17.8 節）の実 TCP 統合テスト。
//!
//! `Server::bind().run()`（本番の accept ループ）+ `Server::gate` +
//! `Server::websocket` の組み合わせで、ゲートの値を
//! - `WebSocketConfig::with_handshake_check` のフック（`WsHandshakeContext::head()`）
//! - `WsMessageHandler::on_open`（`WsOpenContext::extensions()`）
//! - `on_message_with_ctx`（`WsConnContext::extensions()`）
//! - `on_close`（同上）
//!
//! から読めること、ゲートが拒否した upgrade は 101 にもフック・ハンドラにも
//! 届かないこと、別接続へ値が漏れないことを固定する。
//!
//! 置き場が `crates/core/tests/` なのは、`plugin-websocket` が core に依存しない
//! 設計で、dev-dependency に core を足すと package 単位の循環となり
//! `cargo geiger` 等の `cargo metadata` ベース検証が偽陽性で FAIL するため
//! （`crates/plugin-websocket/tests/conn_extensions_e2e.rs` は duplex 版の
//! 網羅テストで、本ファイルは実 TCP での配線確認を担う分担）。
//! クライアントは `websocket_upgrade.rs` と同型の手書きフレームで駆動し、
//! 全 read を時間制限付きにして退行時に CI がハングしないようにする。

#![cfg(feature = "websocket")]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_core::Server;
use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestExtensions, RequestGate};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_plugin_websocket::handler::{
    BoxFuture, CloseReason, WsConnContext, WsHandlerError, WsMessage, WsMessageHandler,
    WsOpenContext, WsOutcome,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, WsHandshakeContext};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

const LIMIT: Duration = Duration::from_secs(5);

/// ゲートが検証して得た値（テスト用の「検証済み主体」）。
struct Principal(String);

/// `X-Test-User` があれば `Principal` を入れて通し、なければ値なしで通すゲート
/// （`gate_request_extensions.rs::InsertingGate` と同型）。
struct InsertingGate;

impl RequestGate for InsertingGate {
    fn name(&self) -> &'static str {
        "inserting"
    }

    fn check(
        &self,
        head: &RequestHead,
        _ctx: &GateContext,
        extensions: &mut RequestExtensions,
    ) -> GateOutcome {
        if let Some(user) = head.header("x-test-user")
            && extensions.insert(Principal(user.to_string())).is_err()
        {
            return GateOutcome::reject(500, Vec::new());
        }
        GateOutcome::Allow
    }
}

/// `X-Deny` があれば拒否するゲート（先行ゲートが値を入れた後の Reject 検証用）。
struct DenyHeaderGate;

impl RequestGate for DenyHeaderGate {
    fn name(&self) -> &'static str {
        "deny-header"
    }

    fn check(
        &self,
        head: &RequestHead,
        _ctx: &GateContext,
        _extensions: &mut RequestExtensions,
    ) -> GateOutcome {
        if head.header("x-deny").is_some() {
            GateOutcome::reject(403, b"denied".to_vec())
        } else {
            GateOutcome::Allow
        }
    }
}

/// `Handler::handle` が呼ばれたら panic するトイハンドラ
/// （`ws_peer_addr.rs::NotCalledHandler` と同型）。
struct NotCalledHandler;
impl fandhe_backend_core::Handler for NotCalledHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        panic!("UpgradeHandler がマッチしたのに既定 Handler が呼ばれた");
    }
}

fn principal_text(ext: &RequestExtensions) -> String {
    ext.get::<Principal>()
        .map_or_else(|| "none".to_string(), |p| p.0.clone())
}

/// 観測箇所ごとの記録（`hook:` / `open:` / `msg:` / `close:` 接頭辞）。
type Log = Arc<Mutex<Vec<String>>>;

/// 3 箇所（open / message / close）で `Principal` を読んで記録するハンドラ。
struct Recorder {
    log: Log,
    closed: mpsc::UnboundedSender<()>,
}

impl WsMessageHandler for Recorder {
    fn name(&self) -> &'static str {
        "gate-ext-recorder"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        let text = format!("open:{}", principal_text(ctx.extensions()));
        self.log.lock().unwrap().push(text.clone());
        let sender = ctx.sender().clone();
        tokio::spawn(async move {
            let _ = sender.send(WsMessage::Text(text)).await;
        });
    }

    fn on_message_with_ctx<'a>(
        &'a self,
        ctx: &'a WsConnContext,
        _msg: WsMessage,
    ) -> BoxFuture<'a, Result<WsOutcome, WsHandlerError>> {
        let text = format!("msg:{}", principal_text(ctx.extensions()));
        self.log.lock().unwrap().push(text.clone());
        Box::pin(async move { Ok(WsOutcome::Reply(vec![WsMessage::Text(text)])) })
    }

    fn on_close(&self, ctx: &WsConnContext, _reason: CloseReason) {
        let text = format!("close:{}", principal_text(ctx.extensions()));
        self.log.lock().unwrap().push(text);
        let _ = self.closed.send(());
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

/// ハンドシェイクフックも含めた全観測の記録先とサーバ。
struct Fixture {
    addr: SocketAddr,
    log: Log,
    closed: mpsc::UnboundedReceiver<()>,
}

async fn spawn_server(with_gate: bool) -> Fixture {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let (tx, closed) = mpsc::unbounded_channel();
    let hook_log = Arc::clone(&log);
    let config = WebSocketConfig::default()
        .with_handshake_check(move |ctx: &WsHandshakeContext<'_>| {
            let text = format!("hook:{}", principal_text(ctx.head().extensions()));
            hook_log.lock().unwrap().push(text);
            Ok(())
        })
        .with_handler(Recorder {
            log: Arc::clone(&log),
            closed: tx,
        });
    let mut server = Server::new();
    if with_gate {
        server = server.gate(InsertingGate).gate(DenyHeaderGate);
    }
    let server = server.websocket(config).handler(NotCalledHandler);
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = bound.run().await;
    });
    Fixture { addr, log, closed }
}

fn upgrade_request(headers: &[(&str, &str)]) -> Vec<u8> {
    let mut req = String::from(
        "GET /ws HTTP/1.1\r\nHost: example.com\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n",
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.into_bytes()
}

/// `\r\n\r\n` までを読み切る（`ws_peer_addr.rs::read_response_head` と同型、時間制限付き）。
async fn read_response_head(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        let n = timeout(LIMIT, stream.read(&mut byte))
            .await
            .expect("応答ヘッド待ちがタイムアウト")
            .expect("read response byte");
        assert_ne!(n, 0, "stream closed before response terminator");
        buf.push(byte[0]);
    }
    String::from_utf8(buf).expect("response head must be valid utf-8")
}

/// サーバから届く 1 フレームを読む（`websocket_upgrade.rs::read_server_frame` と同型、時間制限付き）。
async fn read_server_frame(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut header = [0u8; 2];
    timeout(LIMIT, stream.read_exact(&mut header))
        .await
        .expect("フレーム待ちがタイムアウト")
        .unwrap();
    let opcode = header[0] & 0x0f;
    let len = (header[1] & 0x7f) as usize;
    assert_eq!(header[1] & 0x80, 0, "server frames must not be masked");
    let mut payload = vec![0u8; len];
    if len > 0 {
        timeout(LIMIT, stream.read_exact(&mut payload))
            .await
            .expect("ペイロード待ちがタイムアウト")
            .unwrap();
    }
    (opcode, payload)
}

fn masked_text_frame(payload: &[u8]) -> Vec<u8> {
    let mask = [0x12, 0x34, 0x56, 0x78];
    let mut frame = vec![0x81, 0x80 | (payload.len() as u8)];
    frame.extend_from_slice(&mask);
    for (i, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[i % 4]);
    }
    frame
}

fn masked_close_frame() -> Vec<u8> {
    vec![0x88, 0x80, 0, 0, 0, 0]
}

/// 101 応答の必須ヘッダを検証する共通ヘルパー（全成功ケースで `run_session` 経由で適用）。
/// `Sec-WebSocket-Accept` の期待値は `upgrade_request` の固定キー `dGhlIHNhbXBsZSBub25jZQ==` に
/// RFC 6455 4.2.2 節の GUID を連結した SHA-1 の base64（RFC 6455 1.3 節の例示値）。
fn assert_handshake_headers(head: &str) {
    let has = |name: &str, value: &str| {
        head.lines().any(|l| {
            l.split_once(':').is_some_and(|(n, v)| {
                n.trim().eq_ignore_ascii_case(name) && v.trim().eq_ignore_ascii_case(value)
            })
        })
    };
    assert!(has("Upgrade", "websocket"), "{head}");
    assert!(has("Connection", "Upgrade"), "{head}");
    // Accept は大文字小文字を区別する base64 のため厳密一致で比較する。
    assert!(
        head.lines().any(|l| l
            .trim()
            .eq_ignore_ascii_case("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
            && l.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")),
        "{head}"
    );
}

/// 1 接続を完走させ、クライアントが TCP 越しに受け取った `open:` と `msg:` を返す。
async fn run_session(addr: SocketAddr, user: Option<&str>) -> (String, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let headers: Vec<(&str, &str)> = user.map(|u| ("X-Test-User", u)).into_iter().collect();
    stream.write_all(&upgrade_request(&headers)).await.unwrap();
    let head = read_response_head(&mut stream).await;
    assert!(
        head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
        "{head}"
    );
    assert_handshake_headers(&head);

    let (op, open) = read_server_frame(&mut stream).await;
    assert_eq!(op, 0x1);
    stream.write_all(&masked_text_frame(b"hi")).await.unwrap();
    let (op, msg) = read_server_frame(&mut stream).await;
    assert_eq!(op, 0x1);

    stream.write_all(&masked_close_frame()).await.unwrap();
    // クライアント起点の Close 後、サーバが接続を閉じる（EOF）まで待つ。
    let mut rest = Vec::new();
    timeout(LIMIT, stream.read_to_end(&mut rest))
        .await
        .expect("接続クローズ待ちがタイムアウト")
        .ok();
    (
        String::from_utf8(open).unwrap(),
        String::from_utf8(msg).unwrap(),
    )
}

async fn wait_closed(f: &mut Fixture) {
    timeout(LIMIT, f.closed.recv())
        .await
        .expect("on_close の通知待ちがタイムアウト")
        .expect("通知チャネルが閉じた");
}

fn take_log(f: &Fixture) -> Vec<String> {
    std::mem::take(&mut *f.log.lock().unwrap())
}

/// 基準 (a)(b): ゲートの値がフック・open・message・close の 4 箇所へ届く。
#[tokio::test]
async fn gate_value_reaches_handshake_open_message_and_close() {
    let mut f = spawn_server(true).await;
    let (open, msg) = run_session(f.addr, Some("alice")).await;
    wait_closed(&mut f).await;

    assert_eq!(open, "open:alice");
    assert_eq!(msg, "msg:alice");
    assert_eq!(
        take_log(&f),
        ["hook:alice", "open:alice", "msg:alice", "close:alice"]
    );
}

/// 基準 (c): ゲートが拒否した upgrade は 101 を返さず、フックも `on_open` も呼ばれない。
#[tokio::test]
async fn rejected_upgrade_gets_no_101_nor_on_open() {
    let f = spawn_server(true).await;
    let mut stream = TcpStream::connect(f.addr).await.unwrap();
    stream
        .write_all(&upgrade_request(&[
            ("X-Test-User", "alice"),
            ("X-Deny", "1"),
        ]))
        .await
        .unwrap();
    let head = read_response_head(&mut stream).await;
    assert!(head.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{head}");
    assert!(!head.contains("101"), "{head}");
    // keep-alive で接続が残りうるため EOF は待たず、Content-Length 分だけ読む。
    let len: usize = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap())
        })
        .expect("Content-Length");
    let mut body = vec![0u8; len];
    timeout(LIMIT, stream.read_exact(&mut body))
        .await
        .expect("本文待ちがタイムアウト")
        .unwrap();
    assert_eq!(body, b"denied");

    assert!(
        take_log(&f).is_empty(),
        "フック・on_open が呼ばれてはならない"
    );
}

/// 基準 (d): 続けて張った別接続に前の接続の値が現れない。
#[tokio::test]
async fn next_connection_does_not_see_previous_value() {
    let mut f = spawn_server(true).await;

    run_session(f.addr, Some("alice")).await;
    wait_closed(&mut f).await;
    assert_eq!(take_log(&f)[0], "hook:alice");

    let (open, msg) = run_session(f.addr, None).await;
    wait_closed(&mut f).await;
    assert_eq!((open.as_str(), msg.as_str()), ("open:none", "msg:none"));
    assert_eq!(
        take_log(&f),
        ["hook:none", "open:none", "msg:none", "close:none"]
    );

    let (open, _) = run_session(f.addr, Some("bob")).await;
    wait_closed(&mut f).await;
    assert_eq!(open, "open:bob");
    assert_eq!(take_log(&f)[3], "close:bob");
}

/// ゲート未登録なら、クライアントが `X-Test-User` を送っても値にならない。
#[tokio::test]
async fn no_gate_means_client_header_is_not_a_value() {
    let mut f = spawn_server(false).await;
    let (open, msg) = run_session(f.addr, Some("mallory")).await;
    wait_closed(&mut f).await;
    assert_eq!((open.as_str(), msg.as_str()), ("open:none", "msg:none"));
    assert_eq!(
        take_log(&f),
        ["hook:none", "open:none", "msg:none", "close:none"]
    );
}

/// 並行する 2 接続で値が交差しない。
#[tokio::test]
async fn concurrent_connections_do_not_cross_values() {
    let f = spawn_server(true).await;
    let (a, b) = tokio::join!(
        run_session(f.addr, Some("alice")),
        run_session(f.addr, Some("bob"))
    );
    assert_eq!(a, ("open:alice".into(), "msg:alice".into()));
    assert_eq!(b, ("open:bob".into(), "msg:bob".into()));
}
