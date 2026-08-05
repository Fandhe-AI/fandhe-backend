//! E2E テスト（Issue #179、親 #91）: ユーザー定義 `WsMessageHandler` の委譲・
//! `WsOutcome::Reply`（複数/空返信）・`WsOutcome::Close`・ハンドラエラー・
//! 既定エコー回帰・サイズ上限維持を、`handshake_e2e.rs` と同型
//! （`tokio::io::duplex` + tokio-tungstenite クライアント）で検証する。

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    WsHandlerError, WsMessage, WsMessageHandler, WsOutcome,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列を返す
/// （`handshake_e2e.rs` と同一のテスト用固定リクエスト）。
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

/// Text メッセージを反転して返す（Binary はそのまま返す）カスタムハンドラ。
struct ReverseHandler;

impl WsMessageHandler for ReverseHandler {
    fn name(&self) -> &'static str {
        "reverse"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move {
            let reply = match msg {
                WsMessage::Text(t) => WsMessage::Text(t.chars().rev().collect()),
                other => other,
            };
            Ok(WsOutcome::Reply(vec![reply]))
        })
    }
}

/// 受信 1 件につき複数返信を返すハンドラ（順序保証の検証用）。
struct MultiReplyHandler;

impl WsMessageHandler for MultiReplyHandler {
    fn name(&self) -> &'static str {
        "multi-reply"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move {
            let WsMessage::Text(t) = msg else {
                return Ok(WsOutcome::Reply(vec![]));
            };
            Ok(WsOutcome::Reply(vec![
                WsMessage::Text(format!("{t}-1")),
                WsMessage::Text(format!("{t}-2")),
            ]))
        })
    }
}

/// 受信メッセージが `"silent"` のときは無返信で継続し、それ以外はエコーする
/// ハンドラ（`WsOutcome::Reply(vec![])` 後も次メッセージを処理できることの
/// 検証用）。
struct SilentThenEchoHandler;

impl WsMessageHandler for SilentThenEchoHandler {
    fn name(&self) -> &'static str {
        "silent-then-echo"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move {
            if matches!(&msg, WsMessage::Text(t) if t == "silent") {
                Ok(WsOutcome::Reply(vec![]))
            } else {
                Ok(WsOutcome::Reply(vec![msg]))
            }
        })
    }
}

/// 受信するとサーバ起点で Close するハンドラ。
struct CloseOnMessageHandler;

impl WsMessageHandler for CloseOnMessageHandler {
    fn name(&self) -> &'static str {
        "close-on-message"
    }

    fn on_message(&self, _msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Close) })
    }
}

/// 常にエラーを返すハンドラ（`WsError::Handler` への変換・接続クローズを
/// 検証する）。
struct FailingHandler;

impl WsMessageHandler for FailingHandler {
    fn name(&self) -> &'static str {
        "failing"
    }

    fn on_message(&self, _msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Err(WsHandlerError::new("boom")) })
    }
}

/// テスト共通: ハンドシェイクを成立させ、クライアント側 `WebSocketStream` と
/// サーバ側の `handle_upgrade` タスクを返す。
async fn spawn_session(
    config: WebSocketConfig,
) -> (
    WebSocketStream<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), fandhe_backend_plugin_websocket::WsError>>,
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

    let client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    (client, server_task)
}

/// ケース 1: カスタムハンドラ（Text 反転）が Text/Binary 双方で反映される。
#[tokio::test]
async fn custom_handler_transforms_text_and_passes_through_binary() {
    let config = WebSocketConfig::default().with_handler(ReverseHandler);
    let (mut client, server_task) = spawn_session(config).await;

    client
        .send(Message::Text("hello".into()))
        .await
        .expect("send text");
    let echoed = client.next().await.expect("reply").expect("no error");
    assert_eq!(echoed, Message::Text("olleh".into()));

    client
        .send(Message::Binary(vec![9, 8, 7].into()))
        .await
        .expect("send binary");
    let echoed = client.next().await.expect("reply").expect("no error");
    assert_eq!(echoed, Message::Binary(vec![9, 8, 7].into()));

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// ケース 2: `WsOutcome::Reply(vec![...])` の複数返信が順序どおり届く。
#[tokio::test]
async fn multiple_replies_are_delivered_in_order() {
    let config = WebSocketConfig::default().with_handler(MultiReplyHandler);
    let (mut client, server_task) = spawn_session(config).await;

    client
        .send(Message::Text("x".into()))
        .await
        .expect("send text");

    let first = client.next().await.expect("first reply").expect("no error");
    let second = client
        .next()
        .await
        .expect("second reply")
        .expect("no error");
    assert_eq!(first, Message::Text("x-1".into()));
    assert_eq!(second, Message::Text("x-2".into()));

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// ケース 3: `WsOutcome::Reply(vec![])`（無返信継続）後も次メッセージを
/// 処理できる。
#[tokio::test]
async fn empty_reply_keeps_session_alive_for_next_message() {
    let config = WebSocketConfig::default().with_handler(SilentThenEchoHandler);
    let (mut client, server_task) = spawn_session(config).await;

    client
        .send(Message::Text("silent".into()))
        .await
        .expect("send silent");
    client
        .send(Message::Text("audible".into()))
        .await
        .expect("send audible");

    // "silent" には返信がないため、次に届くのは "audible" のエコーのみ。
    let echoed = client.next().await.expect("reply").expect("no error");
    assert_eq!(echoed, Message::Text("audible".into()));

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// ケース 4: `WsOutcome::Close` でサーバ起点の Close ハンドシェイクが完了する。
#[tokio::test]
async fn server_initiated_close_completes_handshake() {
    let config = WebSocketConfig::default().with_handler(CloseOnMessageHandler);
    let (mut client, server_task) = spawn_session(config).await;

    client
        .send(Message::Text("anything".into()))
        .await
        .expect("send text");

    // サーバが Close フレームを送出する。クライアント側の tokio-tungstenite
    // は Close 受信時に自動で Close 応答を返す（tungstenite の既定動作）。
    let closed = client.next().await;
    match closed {
        Some(Ok(Message::Close(_))) | None => {}
        other => panic!("expected close frame or stream end, got {other:?}"),
    }

    let result = server_task.await.unwrap();
    assert!(
        result.is_ok(),
        "server-initiated close should end cleanly: {result:?}"
    );
}

/// ケース 5: ハンドラ `Err` で接続がクローズされ `handle_upgrade` が
/// `WsError::Handler` を返す。
#[tokio::test]
async fn handler_error_closes_connection_as_handler_error() {
    let config = WebSocketConfig::default().with_handler(FailingHandler);
    let (mut client, server_task) = spawn_session(config).await;

    client
        .send(Message::Text("trigger".into()))
        .await
        .expect("send text");

    // サーバはハンドラエラーで接続を閉じるため、クライアント側の以降の
    // 受信はエラーまたはストリーム終端になる。
    let _ = client.next().await;

    let result = server_task.await.unwrap();
    match result {
        Err(fandhe_backend_plugin_websocket::WsError::Handler(_)) => {}
        other => panic!("expected WsError::Handler, got {other:?}"),
    }
}

/// ケース 6（回帰）: `WebSocketConfig::default()`（ハンドラ未指定）で従来
/// どおりエコーする。
#[tokio::test]
async fn default_config_still_echoes() {
    let config = WebSocketConfig::default();
    assert_eq!(config.handler_name(), "echo");
    let (mut client, server_task) = spawn_session(config).await;

    client
        .send(Message::Text("regression".into()))
        .await
        .expect("send text");
    let echoed = client.next().await.expect("reply").expect("no error");
    assert_eq!(echoed, Message::Text("regression".into()));

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// ケース 7（回帰）: サイズ上限超過メッセージがハンドラへ届く前に拒否される
/// （既存 DoS 上限の維持）。ハンドラが呼ばれていれば `WsOutcome::Reply` で
/// 巨大メッセージがそのまま返るはずだが、上限超過はプロトコルエラーとして
/// tungstenite 側で拒否されるため、クライアントはエコーではなくエラー/
/// 切断を観測する。
#[tokio::test]
async fn oversized_message_is_rejected_before_reaching_handler() {
    let config = WebSocketConfig::default()
        .with_max_message_size(16)
        .with_handler(ReverseHandler);
    let (mut client, server_task) = spawn_session(config).await;

    // max_message_size(16) を超える 32 バイトの Text を送る。
    let oversized = "a".repeat(32);
    client
        .send(Message::Text(oversized.clone().into()))
        .await
        .expect("send oversized text");

    // ReverseHandler が呼ばれていれば逆順文字列がエコーされるはずだが、
    // 上限超過はハンドラ到達前に拒否されるため、そのメッセージ内容の
    // エコーは届かない（エラーまたは接続終了を観測する）。
    let next = client.next().await;
    let reversed: String = oversized.chars().rev().collect();
    if let Some(Ok(Message::Text(text))) = next {
        assert_ne!(
            text.as_str(),
            reversed,
            "oversized message must not reach the handler"
        );
    }

    let result = server_task.await.unwrap();
    assert!(
        result.is_err(),
        "oversized message should be rejected as a protocol error: {result:?}"
    );
}
