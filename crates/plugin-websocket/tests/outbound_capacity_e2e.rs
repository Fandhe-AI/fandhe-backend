//! E2E テスト（イシュー #709、親 #708）: 送信キュー容量の設定
//! （`WebSocketConfig::with_outbound_capacity`）と `WsSender::try_send` を
//! 実ハンドシェイク経由で検証する。
//!
//! 単体テスト（`src/config.rs`・`src/handler.rs` の `#[cfg(test)]`）は
//! `handler::channel` を直接呼ぶ低レイヤの検証を担い、本ファイルは
//! `handle_upgrade`（`WebSocketConfig` → `handler::channel(config.
//! outbound_capacity)` の実配線）を経由した場合にも AC1〜AC3 が成り立つこと
//! を確認する（`tests/server_push_e2e.rs` と同じヘルパー構成）。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome, WsSender,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列
/// （`server_push_e2e.rs` と同一のテスト用固定リクエスト）。
fn handshake_request_bytes() -> &'static [u8] {
    b"GET /ws HTTP/1.1\r\n\
      Host: example.com\r\n\
      Upgrade: websocket\r\n\
      Connection: Upgrade\r\n\
      Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
      Sec-WebSocket-Version: 13\r\n\
      \r\n"
}

/// クライアント側ストリームから `\r\n\r\n` までを読み切る
/// （`server_push_e2e.rs` と同一のヘルパー）。
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

/// テスト共通: ハンドシェイクを成立させ、クライアント側 `WebSocketStream` と
/// サーバ側 `handle_upgrade` タスクを返す（`server_push_e2e.rs` と同一実装）。
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

/// ケース 1（AC1・AC2）: `on_open` の中で `ctx.sender().try_send(...)` を
/// `capacity + 1` 回呼ぶハンドラ。101 応答送出成功後・`run_session` 開始前に
/// 同期的に呼ばれるため（`crate::lib::handle_upgrade` の契約）、結果は
/// 決定的になる（クライアントの受信有無に依存しない）。
struct TrySendOnOpenHandler {
    capacity: usize,
    results: Arc<Mutex<Vec<Result<(), &'static str>>>>,
}

impl WsMessageHandler for TrySendOnOpenHandler {
    fn name(&self) -> &'static str {
        "try-send-on-open"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        let sender = ctx.sender();
        let mut results = self.results.lock().unwrap();
        for i in 0..=self.capacity {
            let outcome = sender
                .try_send(WsMessage::Text(format!("push-{i}")))
                .map_err(|err| if err.is_full() { "full" } else { "closed" });
            results.push(outcome);
        }
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

/// AC1・AC2: 設定した容量（4）ちょうどまでの `try_send` は `Ok`、
/// 容量を超えた 1 回だけ `Full` を返す。クライアントは容量分のメッセージを
/// 順番どおり受信できる。
#[tokio::test]
async fn configured_capacity_accepts_exactly_n_try_sends_in_on_open() {
    const CAPACITY: usize = 4;
    let results: Arc<Mutex<Vec<Result<(), &'static str>>>> = Arc::new(Mutex::new(Vec::new()));
    let config = WebSocketConfig::default()
        .with_outbound_capacity(CAPACITY)
        .unwrap()
        .with_handler(TrySendOnOpenHandler {
            capacity: CAPACITY,
            results: Arc::clone(&results),
        });
    let (mut client, server_task) = spawn_session(config).await;

    for i in 0..CAPACITY {
        let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("push should arrive within timeout")
            .expect("stream should not end")
            .expect("frame should not error");
        assert_eq!(msg, Message::Text(format!("push-{i}").into()));
    }

    let recorded = results.lock().unwrap().clone();
    assert_eq!(recorded.len(), CAPACITY + 1);
    for outcome in &recorded[..CAPACITY] {
        assert_eq!(
            *outcome,
            Ok(()),
            "first {CAPACITY} try_send calls should succeed"
        );
    }
    assert_eq!(
        recorded[CAPACITY],
        Err("full"),
        "the {}th try_send (0-indexed) should observe a full queue",
        CAPACITY
    );

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// AC3: `WebSocketConfig` を明示設定しない場合、実配線経由でも既定容量は
/// 8 のまま（後方互換）。
#[tokio::test]
async fn default_capacity_remains_8_via_handle_upgrade() {
    const DEFAULT_CAPACITY: usize = 8;
    let results: Arc<Mutex<Vec<Result<(), &'static str>>>> = Arc::new(Mutex::new(Vec::new()));
    let config = WebSocketConfig::default().with_handler(TrySendOnOpenHandler {
        capacity: DEFAULT_CAPACITY,
        results: Arc::clone(&results),
    });
    let (mut client, server_task) = spawn_session(config).await;

    for i in 0..DEFAULT_CAPACITY {
        let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("push should arrive within timeout")
            .expect("stream should not end")
            .expect("frame should not error");
        assert_eq!(msg, Message::Text(format!("push-{i}").into()));
    }

    let recorded = results.lock().unwrap().clone();
    assert_eq!(recorded.len(), DEFAULT_CAPACITY + 1);
    for outcome in &recorded[..DEFAULT_CAPACITY] {
        assert_eq!(*outcome, Ok(()));
    }
    assert_eq!(recorded[DEFAULT_CAPACITY], Err("full"));

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// ケース 3 用: `on_open` で受け取った `WsSender` を外部 `slot` へ保存する
/// だけのハンドラ（`server_push_e2e.rs::CaptureSenderHandler` と同型）。
struct CaptureSenderHandler {
    slot: Arc<Mutex<Option<WsSender>>>,
}

impl WsMessageHandler for CaptureSenderHandler {
    fn name(&self) -> &'static str {
        "capture-sender"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        *self.slot.lock().unwrap() = Some(ctx.sender().clone());
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

/// AC2: セッション終了後の `try_send` は有界時間内に `Closed` を返す
/// （panic しない）。クライアント切断からセッションタスクが実際に終了する
/// までの間は一時的に `Full`/`Closed` いずれも観測しうるため、`Closed` が
/// 安定して観測されるまで有界回数ポーリングする。
#[tokio::test]
async fn try_send_after_session_end_returns_closed() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(CaptureSenderHandler {
        slot: Arc::clone(&slot),
    });
    let (mut client, server_task) = spawn_session(config).await;

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");

    let sender = slot
        .lock()
        .unwrap()
        .take()
        .expect("on_open must have been called for an established session");

    let poll = async {
        loop {
            match sender.try_send(WsMessage::Text("late".to_string())) {
                Err(err) if err.is_closed() => return,
                Err(err) if err.is_full() => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                other => panic!("expected Closed eventually, got {other:?}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(2), poll)
        .await
        .expect("try_send should observe Closed within timeout after session end");
}
