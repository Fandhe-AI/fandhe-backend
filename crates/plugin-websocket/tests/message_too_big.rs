//! 受信上限（`max_message_size`/`max_frame_size`）超過時の Close 1009 送出を
//! 検証する統合テスト（イシュー #719）。
//!
//! `idle_timeout.rs`・`handler_e2e.rs` と同型（`tokio::io::duplex` +
//! tokio-tungstenite クライアントで `handle_upgrade` を駆動）。
//!
//! 修正前の挙動（受け入れ基準 1。修正前のコード
//! （`SessionFailure::recv(err)` を即 `return` するだけの旧実装）に対し、
//! `with_max_message_size(16)` + 32 バイト Text 送信の同一シナリオで
//! 一時的な検証テストを実行して実測した）: 上限超過時、tungstenite の
//! `Capacity` エラーがハンドラ呼び出し前に検出されると、サーバーは
//! Close フレームを送らずに接続を drop していた。クライアント側は
//! Close を受け取れず、`client.next()` は
//! `Err(Protocol(ResetWithoutClosingHandshake))`
//! （実測: `"WebSocket protocol error: Connection reset without closing
//! handshake"`）を返した（`tokio::io::duplex` は TCP と異なり明示的な
//! RST を模倣しないが、tokio-tungstenite 側は Close ハンドシェイクなしの
//! 切断として同エラーを返す）。サーバー側の戻り値は修正前から
//! `Err(WsError::Protocol(Capacity(_)))`
//! （実測: `Display` は `"websocket protocol error"`）であり、これは
//! 修正後も変わらない（受け入れ基準 3）。修正後は本ファイルの他のテストが
//! 示すとおり、close code 1009・reason `"message too big"` を含む
//! Close フレームが必ず先に届く。
//!
//! サーバー側の戻り値は修正前後で変わらず
//! `Err(WsError::Protocol(tungstenite::Error::Capacity(_)))`
//! のまま（受け入れ基準 3。CHANGELOG 上は非破壊のバグ修正）。

use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    WsHandlerError, WsMessage, WsMessageHandler, WsOutcome,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, WsError, handle_upgrade};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列
/// （`handshake_e2e.rs`・`idle_timeout.rs` と同一）。
fn handshake_request_bytes() -> &'static [u8] {
    b"GET /ws HTTP/1.1\r\n\
      Host: example.com\r\n\
      Upgrade: websocket\r\n\
      Connection: Upgrade\r\n\
      Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
      Sec-WebSocket-Version: 13\r\n\
      \r\n"
}

async fn read_http_response_line<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> String {
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

/// ハンドラが呼ばれたら記録する（受け入れ基準・回帰: 上限超過メッセージが
/// ハンドラへ到達しないこと、既存の DoS 方針を維持していることの確認用）。
struct RecordingHandler {
    called: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl WsMessageHandler for RecordingHandler {
    fn name(&self) -> &'static str {
        "recording"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        self.called.store(true, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

/// ハンドシェイクを成立させ、101 応答を読み切ったクライアント
/// `WebSocketStream` とサーバタスクの `JoinHandle` を返す
/// （`idle_timeout.rs` と同一パターン）。
async fn handshake(
    config: WebSocketConfig,
) -> (
    WebSocketStream<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), WsError>>,
) {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };

    let (server_side, mut client_side) = tokio::io::duplex(64 * 1024);

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

    let client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    (client, server_task)
}

/// サーバーの戻り値が `Err(WsError::Protocol(Capacity(_)))` であることを
/// 検証する共通ヘルパー（`Error::source()` 経由の判別、受け入れ基準 3）。
fn assert_capacity_protocol_error(result: &Result<(), WsError>) {
    let err = result
        .as_ref()
        .expect_err("capacity overflow must be an error");
    match err {
        WsError::Protocol(tokio_tungstenite::tungstenite::Error::Capacity(_)) => {}
        other => panic!("expected WsError::Protocol(Capacity(_)), got {other:?}"),
    }
    // `std::error::Error::source()` 経由でも同じ根本原因を判別できること
    // （利用者が `WsError` を `dyn Error` としてしか保持していない経路の
    // 回帰防止）。
    use std::error::Error as _;
    let source = err.source().expect("Protocol variant must expose a source");
    assert!(
        source
            .downcast_ref::<tokio_tungstenite::tungstenite::Error>()
            .is_some_and(|e| matches!(e, tokio_tungstenite::tungstenite::Error::Capacity(_))),
        "source() must downcast to tungstenite::Error::Capacity"
    );
}

/// ケース (a): 単一フレームで `max_frame_size` を超える送信は、close code
/// 1009・固定 reason `"message too big"` の Close フレームを送出させる。
#[tokio::test]
async fn frame_size_overflow_sends_close_1009() {
    let config = WebSocketConfig::default().with_max_frame_size(16);
    let (mut client, server_task) = handshake(config).await;

    let oversized = "a".repeat(32);
    client
        .send(Message::Text(oversized.into()))
        .await
        .expect("send oversized text");

    let received = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("server should send close before test timeout")
        .expect("stream should yield a message")
        .expect("no protocol error reading the close frame");

    match received {
        Message::Close(Some(frame)) => {
            assert_eq!(frame.code, CloseCode::Size);
            assert_eq!(frame.reason.as_str(), "message too big");
        }
        other => panic!("expected Close(Some(1009)) frame, got {other:?}"),
    }

    // クライアント側を drop し、duplex のもう一方（サーバー側の生読み取り）が
    // EOF を観測できるようにする（`handle_message_too_big` の有界読み捨てが
    // `close_grace` 満了を待たず即終わることを確認するため）。
    drop(client);

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within close_grace")
        .unwrap();
    assert_capacity_protocol_error(&result);
}

/// ケース (b): 単一フレームで `max_message_size` を超える送信（frame 上限は
/// 既定のため先に message 上限側の判定に到達する）でも同様に 1009 が届く。
#[tokio::test]
async fn message_size_overflow_single_frame_sends_close_1009() {
    let config = WebSocketConfig::default().with_max_message_size(16);
    let (mut client, server_task) = handshake(config).await;

    let oversized = "a".repeat(32);
    client
        .send(Message::Text(oversized.into()))
        .await
        .expect("send oversized text");

    let received = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("server should send close before test timeout")
        .expect("stream should yield a message")
        .expect("no protocol error reading the close frame");

    match received {
        Message::Close(Some(frame)) => {
            assert_eq!(frame.code, CloseCode::Size);
            assert_eq!(frame.reason.as_str(), "message too big");
        }
        other => panic!("expected Close(Some(1009)) frame, got {other:?}"),
    }

    drop(client);

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within close_grace")
        .unwrap();
    assert_capacity_protocol_error(&result);
}

/// ケース (d): 上限超過メッセージはハンドラへ到達しない（ハンドラ呼び出し
/// 前に拒否されるという既存の DoS 方針の回帰）。
#[tokio::test]
async fn oversized_message_does_not_reach_handler() {
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let config = WebSocketConfig::default()
        .with_max_message_size(16)
        .with_handler(RecordingHandler {
            called: std::sync::Arc::clone(&called),
        });
    let (mut client, server_task) = handshake(config).await;

    let oversized = "a".repeat(32);
    client
        .send(Message::Text(oversized.into()))
        .await
        .expect("send oversized text");

    let _received = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("server should send close before test timeout")
        .expect("stream should yield a message")
        .expect("no protocol error reading the close frame");

    drop(client);

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within close_grace")
        .unwrap();
    assert_capacity_protocol_error(&result);
    assert!(
        !called.load(std::sync::atomic::Ordering::SeqCst),
        "oversized message must not reach the handler (Issue #175 DoS policy)"
    );
}

/// ケース (e): Close 応答も EOF も返さない放置クライアントでも、サーバー
/// タスクが `close_grace` + ε 以内に終了すること（二次 DoS の回帰防止。
/// `idle_timeout.rs::server_terminates_even_if_client_ignores_close` と
/// 同型のパターン）。
#[tokio::test]
async fn server_terminates_within_close_grace_even_if_client_ignores_close() {
    let config = WebSocketConfig::default()
        .with_max_message_size(16)
        .with_close_grace(Duration::from_millis(200));
    let (mut client, server_task) = handshake(config).await;

    let oversized = "a".repeat(32);
    client
        .send(Message::Text(oversized.into()))
        .await
        .expect("send oversized text");

    // クライアントは Close フレームを受信しても応答せず、ストリームを
    // 保持したまま放置する（drop すると duplex が EOF を返し close_grace を
    // 検証できなくなるため、明示的に forget して接続を握ったままにする）。
    std::mem::forget(client);

    let result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server task must not hang beyond close_grace")
        .unwrap();
    assert_capacity_protocol_error(&result);
}
