//! `WsSender::closed()` / `is_closed()` の E2E テスト（イシュー #727、親 #705）。
//!
//! `handler.rs` の doc test・単体テストは `handler::channel` を直接使う内部
//! 経路を検証済み。本ファイルは `server_push_e2e.rs` と同様に
//! `handle_upgrade` を実際に駆動し、**セッションの終了経路ごとに**
//! `closed()`/`is_closed()` が正しく完了・遷移することを確認する。
//!
//! 受け入れ基準（イシュー #727 本文）:
//! 1. `closed().await` が、どの終了経路でも終了後に完了する
//! 2. `is_closed()` が終了前は `false`、終了後は `true`
//! 3. clone した `WsSender` でも同じように動く

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome, WsSender,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列
/// （`server_push_e2e.rs` / `handler_e2e.rs` と同一のテスト用固定リクエスト）。
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
/// サーバ側 `handle_upgrade` タスクを返す（`server_push_e2e.rs` と同一実装、
/// キャンセルは `std::future::pending` で無期限 pending にする）。
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

/// テスト共通: ハンドシェイクを成立させ、キャンセルトリガ（`oneshot`）を
/// 呼び出し側で握った状態でセッションを開始する（`server_push_e2e.rs` と
/// 同一パターン）。世代キャンセル経路専用。
async fn spawn_session_with_cancel(
    config: WebSocketConfig,
) -> (
    WebSocketStream<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), fandhe_backend_plugin_websocket::WsError>>,
    tokio::sync::oneshot::Sender<()>,
) {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let (server_side, mut client_side) = tokio::io::duplex(64 * 1024);
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

    let server_task = tokio::spawn(async move {
        handle_upgrade(server_side, &head, Vec::new(), &config, async move {
            let _ = cancel_rx.await;
        })
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    (client, server_task, cancel_tx)
}

/// 終了経路ごとのケースが共通で確認する事項: `server_task` が終わった後
/// （または abort 後）に取り出した `WsSender` とそのクローンの双方で、
/// `closed()` が有界時間内に完了し `is_closed() == true` であること
/// （受け入れ基準 1・3）。
async fn assert_closed_eventually(sender: &WsSender, cloned: &WsSender) {
    tokio::time::timeout(Duration::from_secs(2), sender.closed())
        .await
        .expect("WsSender::closed() should complete within timeout");
    tokio::time::timeout(Duration::from_secs(2), cloned.closed())
        .await
        .expect("cloned WsSender::closed() should complete within timeout");
    assert!(sender.is_closed());
    assert!(cloned.is_closed());
}

/// 各ケースで使うハンドラ: `on_open` で `WsSender` を捕捉し、`on_message` は
/// モード（エコー/Close/Err）を切り替えられる。
enum ReplyMode {
    Echo,
    Close,
    Err,
}

struct CaptureHandler {
    slot: Arc<Mutex<Option<WsSender>>>,
    mode: ReplyMode,
}

impl WsMessageHandler for CaptureHandler {
    fn name(&self) -> &'static str {
        "capture-closed"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        *self.slot.lock().unwrap() = Some(ctx.sender().clone());
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        match self.mode {
            ReplyMode::Echo => Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) }),
            ReplyMode::Close => Box::pin(async move { Ok(WsOutcome::Close) }),
            // "trigger-error" のみでエラーを返し、それ以外（`on_open` 実行
            // 保証用の往復プローブ "hi"）はエコーする。往復プローブ自体で
            // 即座に切断してしまうと `round_trip_and_capture` が
            // `WsSender` を取り出す前に接続が終わってしまうため。
            ReplyMode::Err => Box::pin(async move {
                match &msg {
                    WsMessage::Text(t) if t == "trigger-error" => {
                        Err(WsHandlerError::new("boom-for-test".to_string()))
                    }
                    _ => Ok(WsOutcome::Reply(vec![msg])),
                }
            }),
        }
    }
}

/// `on_open` 実行を保証するための 1 往復エコー（101 応答を読めただけでは
/// `on_open` 実行を保証しない、`WsSender::closed` doc comment 参照）。
async fn round_trip_and_capture(
    client: &mut WebSocketStream<tokio::io::DuplexStream>,
    slot: &Arc<Mutex<Option<WsSender>>>,
) -> WsSender {
    client
        .send(Message::Text("hi".into()))
        .await
        .expect("send handshake probe");
    let _ = client.next().await;
    slot.lock()
        .unwrap()
        .clone()
        .expect("on_open must have run for an established session")
}

/// ケース 1: 終了前は `is_closed() == false`・`closed()` は未完了。その後
/// クライアントが正常 Close すると、`false → true` へ遷移する（受け入れ
/// 基準 1・2・3 を一括で確認する）。
#[tokio::test]
async fn not_closed_before_termination_then_closes_on_normal_close() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(CaptureHandler {
        slot: Arc::clone(&slot),
        mode: ReplyMode::Echo,
    });
    let (mut client, server_task) = spawn_session(config).await;

    let sender = round_trip_and_capture(&mut client, &slot).await;
    let cloned = sender.clone();

    assert!(!sender.is_closed());
    assert!(!cloned.is_closed());
    let not_yet = tokio::time::timeout(Duration::from_millis(50), sender.closed()).await;
    assert!(
        not_yet.is_err(),
        "closed() must not complete before the session ends"
    );

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");

    assert_closed_eventually(&sender, &cloned).await;
}

/// ケース 2: クライアントの Close フレームで終了した場合も `closed()` が
/// 完了する（ケース 1 と同一経路だが、`send`/`is_closed` を挟まない最小形も
/// 独立して確認する）。
#[tokio::test]
async fn closes_on_client_close_frame() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(CaptureHandler {
        slot: Arc::clone(&slot),
        mode: ReplyMode::Echo,
    });
    let (mut client, server_task) = spawn_session(config).await;
    let sender = round_trip_and_capture(&mut client, &slot).await;
    let cloned = sender.clone();

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");

    assert_closed_eventually(&sender, &cloned).await;
}

/// ケース 3: クライアント側ストリームの突然の EOF（drop）でも `closed()` が
/// 完了する。
#[tokio::test]
async fn closes_on_abrupt_client_eof() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(CaptureHandler {
        slot: Arc::clone(&slot),
        mode: ReplyMode::Echo,
    });
    let (mut client, server_task) = spawn_session(config).await;
    let sender = round_trip_and_capture(&mut client, &slot).await;
    let cloned = sender.clone();

    drop(client);
    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within timeout")
        .unwrap();
    // 突然の EOF は正常終了・エラー終了のいずれの表現でもよい（重要なのは
    // `closed()` が有界時間内に完了すること）。
    let _ = result;

    assert_closed_eventually(&sender, &cloned).await;
}

/// ケース 4: idle timeout 発火で終了した場合も `closed()` が完了する。
#[tokio::test]
async fn closes_on_idle_timeout() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default()
        .with_idle_timeout(Duration::from_millis(200))
        .with_handler(CaptureHandler {
            slot: Arc::clone(&slot),
            mode: ReplyMode::Echo,
        });
    let (mut client, server_task) = spawn_session(config).await;
    let sender = round_trip_and_capture(&mut client, &slot).await;
    let cloned = sender.clone();

    // idle timeout 発火によるサーバ起点 Close を読み進める（`idle_timeout.rs`
    // と同一パターン）。
    let _ = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("close frame should arrive within timeout");
    let _ = client.next().await;

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within timeout")
        .unwrap();
    assert!(
        result.is_ok(),
        "idle timeout should end the session normally: {result:?}"
    );

    assert_closed_eventually(&sender, &cloned).await;
}

/// ケース 5: 世代キャンセル（最終 graceful shutdown・rebind 世代 drain 相当）
/// による終了でも `closed()` が完了する（`server_push_e2e.rs` のキャンセル
/// トリガ駆動パターンを流用）。
#[tokio::test]
async fn closes_on_generation_cancel() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(CaptureHandler {
        slot: Arc::clone(&slot),
        mode: ReplyMode::Echo,
    });
    let (mut client, server_task, cancel_tx) = spawn_session_with_cancel(config).await;
    let sender = round_trip_and_capture(&mut client, &slot).await;
    let cloned = sender.clone();

    cancel_tx.send(()).expect("cancel receiver must be alive");

    // サーバは Close フレーム（1001 Going Away）を送出する。応答送出を駆動
    // するため 2 回 `next()` を呼ぶ（`server_push_e2e.rs` と同一パターン）。
    let _ = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("close frame should arrive before test timeout");
    let _ = client.next().await;

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within grace period")
        .unwrap();
    assert!(
        result.is_ok(),
        "cancellation should end the session normally: {result:?}"
    );

    assert_closed_eventually(&sender, &cloned).await;
}

/// ケース 6: プロトコルエラー（`max_message_size` 超過）による終了でも
/// `closed()` が完了する。
#[tokio::test]
async fn closes_on_protocol_error() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default()
        .with_max_message_size(16)
        .with_handler(CaptureHandler {
            slot: Arc::clone(&slot),
            mode: ReplyMode::Echo,
        });
    let (mut client, server_task) = spawn_session(config).await;
    let sender = round_trip_and_capture(&mut client, &slot).await;
    let cloned = sender.clone();

    let oversized = "a".repeat(32);
    client
        .send(Message::Text(oversized.into()))
        .await
        .expect("send oversized text");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within timeout")
        .unwrap();
    assert!(
        result.is_err(),
        "oversized message should be rejected as a protocol error: {result:?}"
    );

    assert_closed_eventually(&sender, &cloned).await;
}

/// ケース 7: ハンドラが `Err` を返した（`WsError::Handler` 経路）ことによる
/// 終了でも `closed()` が完了する。
#[tokio::test]
async fn closes_on_handler_error() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(CaptureHandler {
        slot: Arc::clone(&slot),
        mode: ReplyMode::Err,
    });
    let (mut client, server_task) = spawn_session(config).await;
    let sender = round_trip_and_capture(&mut client, &slot).await;
    let cloned = sender.clone();

    client
        .send(Message::Text("trigger-error".into()))
        .await
        .expect("send message that triggers handler error");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within timeout")
        .unwrap();
    assert!(
        matches!(
            result,
            Err(fandhe_backend_plugin_websocket::WsError::Handler(_))
        ),
        "handler error should propagate as WsError::Handler: {result:?}"
    );

    assert_closed_eventually(&sender, &cloned).await;
}

/// ケース 8: ハンドラが `WsOutcome::Close`（サーバー起点の Close）を返した
/// ことによる終了でも `closed()` が完了する。
#[tokio::test]
async fn closes_on_handler_requested_close() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(CaptureHandler {
        slot: Arc::clone(&slot),
        mode: ReplyMode::Close,
    });
    let (mut client, server_task) = spawn_session(config).await;

    // ケース 8 では最初の送信で on_open を確認しつつ、同じ送信が
    // `WsOutcome::Close` をトリガする（`round_trip_and_capture` はエコー
    // 返信を前提とするため使わず、Close フレームの到着を直接待つ）。
    client
        .send(Message::Text("hi".into()))
        .await
        .expect("send handshake probe");
    let sender = {
        // on_open は 101 応答送出直後・メッセージ処理より前に実行される
        // ため、Close フレームを読む前に必ず取得できる。
        loop {
            if let Some(s) = slot.lock().unwrap().clone() {
                break s;
            }
            tokio::task::yield_now().await;
        }
    };
    let cloned = sender.clone();

    let _ = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("close frame should arrive within timeout");
    let _ = client.next().await;

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within timeout")
        .unwrap();
    assert!(
        result.is_ok(),
        "handler-requested close should end the session normally: {result:?}"
    );

    assert_closed_eventually(&sender, &cloned).await;
}

/// ケース 9: future の drop（`server_task.abort()`）でも、`closed()` は
/// 有界時間内に完了する（コアの grace 超過時強制クローズに相当）。
#[tokio::test]
async fn closes_on_server_task_abort() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(CaptureHandler {
        slot: Arc::clone(&slot),
        mode: ReplyMode::Echo,
    });
    let (mut client, server_task) = spawn_session(config).await;
    let sender = round_trip_and_capture(&mut client, &slot).await;
    let cloned = sender.clone();

    server_task.abort();
    let _ = server_task.await;

    assert_closed_eventually(&sender, &cloned).await;
}

/// ケース 10（待機中に終了）: `on_open` で spawn したタスクが
/// `closed().await` の完了後に `oneshot` で通知する。クライアントが
/// close した後、有界時間内に通知が届くことで、ブロック中の待機が
/// 確実に起こされることを確認する（`closed_wakes_pending_waiter_on_
/// receiver_drop` 単体テストの E2E 版）。
struct WaitOnCloseHandler {
    notify_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl WsMessageHandler for WaitOnCloseHandler {
    fn name(&self) -> &'static str {
        "wait-on-close"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        let sender = ctx.sender().clone();
        let notify_tx = self.notify_tx.lock().unwrap().take();
        tokio::spawn(async move {
            sender.closed().await;
            if let Some(tx) = notify_tx {
                let _ = tx.send(());
            }
        });
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

#[tokio::test]
async fn spawned_waiter_is_woken_after_client_close() {
    let (notify_tx, notify_rx) = tokio::sync::oneshot::channel::<()>();
    let config = WebSocketConfig::default().with_handler(WaitOnCloseHandler {
        notify_tx: Mutex::new(Some(notify_tx)),
    });
    let (mut client, server_task) = spawn_session(config).await;

    client
        .send(Message::Text("hi".into()))
        .await
        .expect("send handshake probe");
    let _ = client.next().await;

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");

    tokio::time::timeout(Duration::from_secs(2), notify_rx)
        .await
        .expect("spawned waiter should be woken within timeout")
        .expect("notify_tx should not be dropped without sending");
}
