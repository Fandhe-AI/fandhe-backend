//! イシュー #706 の受け入れ基準を検証する e2e テスト。
//!
//! `on_message`（[`WsMessageHandler::on_message`]）実行中に
//! [`WsSender::send`] を送信キュー容量（[`DEFAULT_OUTBOUND_CAPACITY`] = 8）を
//! 超える回数呼んでもデッドロックしないこと（受け入れ基準 1）、push と
//! 返信（[`WsOutcome::Reply`]）のフレームが混ざらないこと（受け入れ基準 2）、
//! 送出順序の保証・不定契約（`crates/plugin-websocket/src/session.rs`
//! モジュール doc「ハンドラ実行中の送信キュー消化」節）がテストで固定
//! されること（受け入れ基準 3）を確認する。
//!
//! `tests/server_push_e2e.rs`（イシュー #669〜#672）は「受信がない状態での
//! push」「クライアント受信ループ中の交錯」を検証済みで、本ファイルは
//! 「ハンドラ実行中（`on_message` の `await` 中）に容量超の push を行う」
//! ケースに限定する（重複しない）。ヘルパーは同ファイルの
//! `spawn_session` / `spawn_session_with_cancel` / `read_http_response_line`
//! と同一実装を用いる（`pub(crate)` にできない統合テストの制約上、重複
//! 保持する。既存の重複許容パターン、`handler_e2e.rs` 系列と同型）。

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
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// 送信キュー既定容量（`crate::handler::DEFAULT_OUTBOUND_CAPACITY` は
/// `pub(crate)` のため本テストからは参照できず、同じ値をここに複製する。
/// 値が変わった場合は本テストの push 件数（容量超を作る目的）も見直す）。
const DEFAULT_OUTBOUND_CAPACITY: usize = 8;

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

/// `on_open` で受け取った `WsSender` を外部から参照できるよう共有する
/// ハンドラの共通土台（本ファイルの各ハンドラが埋め込む）。
type SenderSlot = Arc<Mutex<Option<WsSender>>>;

/// T1・T2 共通: `on_message` 実行中に容量超（`push_count` 件、既定 20）の
/// push を行い、最後に固定文字列 `"reply"` を返信するハンドラ。
/// `spawn_pusher` が `true` の場合、push は `tokio::spawn` した別タスクへ
/// 委譲し、その `JoinHandle` を `on_message` 側が await する（受け入れ基準
/// 「別タスク経由でも保証が成り立つ」ことの検証、T2 用）。
struct DrainingPushHandler {
    push_count: usize,
    spawn_pusher: bool,
    slot: SenderSlot,
}

impl WsMessageHandler for DrainingPushHandler {
    fn name(&self) -> &'static str {
        "draining-push"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        *self.slot.lock().unwrap() = Some(ctx.sender().clone());
    }

    fn on_message(&self, _msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        let sender = self
            .slot
            .lock()
            .unwrap()
            .clone()
            .expect("on_open must run before on_message for an established session");
        let push_count = self.push_count;
        let spawn_pusher = self.spawn_pusher;
        Box::pin(async move {
            let push_all = async move {
                for i in 0..push_count {
                    sender
                        .send(WsMessage::Text(format!("push-{i}")))
                        .await
                        .map_err(WsHandlerError::new)?;
                }
                Ok::<(), WsHandlerError>(())
            };
            if spawn_pusher {
                tokio::spawn(push_all)
                    .await
                    .map_err(WsHandlerError::new)??;
            } else {
                push_all.await?;
            }
            Ok(WsOutcome::Reply(vec![WsMessage::Text("reply".to_string())]))
        })
    }
}

/// T1（受け入れ基準 1・2・3）: `on_message` 内で容量超（20 件、既定容量 8
/// の 2.5 倍）の push を `send().await` しても、旧実装ではタイムアウトする
/// はずの処理が完了し、20 件の push が送信順どおり・欠落なく届いたのちに
/// `"reply"` が最後に届くこと（送出順序の保証、`session.rs` モジュール doc
/// を参照）。
#[tokio::test]
async fn handler_push_beyond_capacity_does_not_deadlock_and_preserves_order() {
    const PUSH_COUNT: usize = DEFAULT_OUTBOUND_CAPACITY * 2 + 4; // 20
    let slot: SenderSlot = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(DrainingPushHandler {
        push_count: PUSH_COUNT,
        spawn_pusher: false,
        slot,
    });
    let (mut client, server_task) = spawn_session(config).await;

    client
        .send(Message::Text("go".into()))
        .await
        .expect("client send should succeed");

    let mut received = Vec::with_capacity(PUSH_COUNT + 1);
    for _ in 0..=PUSH_COUNT {
        let msg = tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect(
                "handler push beyond channel capacity must not deadlock \
                 (this timeout reproduces the pre-fix bug)",
            )
            .expect("stream should not end early")
            .expect("frame should not error");
        received.push(msg);
    }

    let expected: Vec<Message> = (0..PUSH_COUNT)
        .map(|i| Message::Text(format!("push-{i}").into()))
        .chain(std::iter::once(Message::Text("reply".into())))
        .collect();
    assert_eq!(
        received, expected,
        "all queued pushes must arrive in send order without loss or duplication, \
         and the handler's reply must arrive only after every push it sent"
    );

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// T2（受け入れ基準 1・3、別タスク経由）: `on_message` が `tokio::spawn` した
/// タスクに容量超の push を委譲し、その `JoinHandle` を await してから返信
/// する場合も T1 と同じ保証（全件到着・順序維持・reply が最後）が成り立つ
/// こと。
#[tokio::test]
async fn handler_push_via_spawned_task_does_not_deadlock_and_preserves_order() {
    const PUSH_COUNT: usize = DEFAULT_OUTBOUND_CAPACITY * 2 + 4;
    let slot: SenderSlot = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(DrainingPushHandler {
        push_count: PUSH_COUNT,
        spawn_pusher: true,
        slot,
    });
    let (mut client, server_task) = spawn_session(config).await;

    client
        .send(Message::Text("go".into()))
        .await
        .expect("client send should succeed");

    let mut received = Vec::with_capacity(PUSH_COUNT + 1);
    for _ in 0..=PUSH_COUNT {
        let msg = tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("handler push via spawned task must not deadlock")
            .expect("stream should not end early")
            .expect("frame should not error");
        received.push(msg);
    }

    let expected: Vec<Message> = (0..PUSH_COUNT)
        .map(|i| Message::Text(format!("push-{i}").into()))
        .chain(std::iter::once(Message::Text("reply".into())))
        .collect();
    assert_eq!(
        received, expected,
        "pushes delegated to a spawned task must still arrive before the reply, in order"
    );

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// T3（受け入れ基準 3、Close 経路）: 容量超 push の後に `WsOutcome::Close`
/// を返すハンドラでも、全 push が Close フレームより先に届くこと。
struct DrainingPushThenCloseHandler {
    push_count: usize,
    slot: SenderSlot,
}

impl WsMessageHandler for DrainingPushThenCloseHandler {
    fn name(&self) -> &'static str {
        "draining-push-then-close"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        *self.slot.lock().unwrap() = Some(ctx.sender().clone());
    }

    fn on_message(&self, _msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        let sender = self
            .slot
            .lock()
            .unwrap()
            .clone()
            .expect("on_open must run before on_message");
        let push_count = self.push_count;
        Box::pin(async move {
            for i in 0..push_count {
                sender
                    .send(WsMessage::Text(format!("push-{i}")))
                    .await
                    .map_err(WsHandlerError::new)?;
            }
            Ok(WsOutcome::Close)
        })
    }
}

#[tokio::test]
async fn handler_push_beyond_capacity_arrives_before_close() {
    const PUSH_COUNT: usize = DEFAULT_OUTBOUND_CAPACITY * 2 + 4;
    let slot: SenderSlot = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(DrainingPushThenCloseHandler {
        push_count: PUSH_COUNT,
        slot,
    });
    let (mut client, server_task) = spawn_session(config).await;

    client
        .send(Message::Text("go".into()))
        .await
        .expect("client send should succeed");

    for i in 0..PUSH_COUNT {
        let msg = tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("push must not deadlock before the close handshake")
            .expect("stream should not end early")
            .expect("frame should not error");
        assert_eq!(
            msg,
            Message::Text(format!("push-{i}").into()),
            "pushes must arrive in order before the close frame"
        );
    }

    let closing = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("close frame should arrive after all pushes")
        .expect("stream should not end early")
        .expect("frame should not error");
    assert!(
        matches!(closing, Message::Close(_)),
        "expected a close frame after all pushes, got {closing:?}"
    );

    client.close(None).await.ok();
    let result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("session should end within a bounded time")
        .unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// T4（決定性の記録）: ハンドラが容量以下（ブロックしない件数）の push を
/// 行った直後、同じポーリングで `Err` を返す場合、`run_handler_with_outbound_drain` は
/// ハンドラを `rx.recv()` より先にポーリングする固定順（`session.rs` の
/// `run_handler_with_outbound_drain` doc を参照）であるため、ハンドラ Future が最初の
/// `poll` で最後まで進み切る（送信が一度もブロックしない）限り
/// `rx.recv()` は一度もポーリングされない。結果として push は 1 件も
/// クライアントへ届かず、セッションは `Err(WsError::Handler(_))` で終わる。
///
/// この決定性はポーリング順（ハンドラ優先）に依存する。将来ポーリング順を
/// 変える場合はこの契約・本テストの前提が崩れることに留意する
/// （`session.rs` モジュール doc の「送出順序の保証と不定」節、ハンドラが
/// `Err` を返した場合は排出しない契約と対応する）。
struct PushThenErrHandler {
    push_count: usize,
    slot: SenderSlot,
}

impl WsMessageHandler for PushThenErrHandler {
    fn name(&self) -> &'static str {
        "push-then-err"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        *self.slot.lock().unwrap() = Some(ctx.sender().clone());
    }

    fn on_message(&self, _msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        let sender = self
            .slot
            .lock()
            .unwrap()
            .clone()
            .expect("on_open must run before on_message");
        let push_count = self.push_count;
        Box::pin(async move {
            for i in 0..push_count {
                sender
                    .send(WsMessage::Text(format!("push-{i}")))
                    .await
                    .map_err(WsHandlerError::new)?;
            }
            Err(WsHandlerError::new("handler intentionally failed"))
        })
    }
}

#[tokio::test]
async fn handler_err_after_non_blocking_pushes_discards_queued_pushes() {
    // 既定容量 (8) 以下に留め、送信がブロックしないことを保証する
    // （ブロックすれば `rx.recv()` がポーリングされ得るため決定性が崩れる）。
    const PUSH_COUNT: usize = DEFAULT_OUTBOUND_CAPACITY - 1;
    let slot: SenderSlot = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(PushThenErrHandler {
        push_count: PUSH_COUNT,
        slot,
    });
    let (mut client, server_task) = spawn_session(config).await;

    client
        .send(Message::Text("go".into()))
        .await
        .expect("client send should succeed");

    // ハンドラが Err で終わるため、セッションは他のフレームを送らずに
    // 接続を終える（既存契約: `outcome?` がハンドラ Err を即座に伝播し、
    // Close ハンドシェイクを経ずに `ws` を drop する。本イシューが変更した
    // 範囲ではない、`apply_outcome` 呼び出し前の既存の早期 return と同型）。
    // クライアント側はハンドシェイクなしの切断として EOF・接続断エラー
    // （`ResetWithoutClosingHandshake` 等）のいずれかを観測しうる。
    let next = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("session should end within a bounded time after handler error");
    match next {
        None => {}
        Some(Err(_)) => {}
        Some(Ok(frame)) => {
            panic!("no push frame should have been sent before the handler error: {frame:?}")
        }
    }

    let result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("session should end within a bounded time")
        .unwrap();
    assert!(
        matches!(
            result,
            Err(fandhe_backend_plugin_websocket::WsError::Handler(_))
        ),
        "session should end with WsError::Handler: {result:?}"
    );
}

/// T5（受け入れ基準、cancel 優先）: ハンドラが恒久的に push を続ける
/// （`WsSender::send` が失敗するまで無限ループ）間に世代キャンセルが発火した
/// 場合、`close_grace`（本テストでは意図的に長め、2 秒）の満了を待たず
/// セッションが終了すること。`run_handler_with_outbound_drain` の cancel 最優先ポーリング
/// （`race_cancel` が内側ループの `race2` より先にキャンセルを確認する）に
/// より、ハンドラ実行中でも `close_grace` 満了を待たされない契約を検証する
/// （`crate::race_cancel` の doc・`session.rs` モジュール doc を参照）。
struct InfinitePushHandler {
    slot: SenderSlot,
}

impl WsMessageHandler for InfinitePushHandler {
    fn name(&self) -> &'static str {
        "infinite-push"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        *self.slot.lock().unwrap() = Some(ctx.sender().clone());
    }

    fn on_message(&self, _msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        let sender = self
            .slot
            .lock()
            .unwrap()
            .clone()
            .expect("on_open must run before on_message");
        Box::pin(async move {
            let mut i: u64 = 0;
            loop {
                if sender
                    .send(WsMessage::Text(format!("push-{i}")))
                    .await
                    .is_err()
                {
                    // セッション終了（本テストではキャンセル）後は送信が
                    // 失敗する。返り値は使われない（ハンドラ Future ごと
                    // drop されるか、ここに到達しても呼び出し元は
                    // Cancelled 経路で終える。中断安全性契約、`handler.rs`
                    // doc を参照）。
                    return Ok(WsOutcome::Reply(vec![]));
                }
                i += 1;
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_infinite_push_is_cancelled_without_waiting_full_close_grace() {
    let slot: SenderSlot = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default()
        .with_close_grace(Duration::from_secs(2))
        .with_handler(InfinitePushHandler { slot });
    let (mut client, server_task, cancel_tx) = spawn_session_with_cancel(config).await;

    client
        .send(Message::Text("go".into()))
        .await
        .expect("client send should succeed");

    // クライアントはハンドラが送出する push を継続的に読み進める
    // バックグラウンドタスクを立てる。読み進めない構成にすると、duplex
    // バッファそのものが congestion し `ws.send()` の I/O 書き込みが
    // ブロックする（クライアントが応答しないスロー接続に対する既存の
    // legitimate backpressure で、`close_grace` の対象。本テストが検証
    // したい「cancel が mpsc チャネルレベルの消化を最優先する」契約とは
    // 別種の遅延であり、混同しないよう分離する）。
    let drain_client = tokio::spawn(async move {
        let mut found_close = false;
        loop {
            match client.next().await {
                Some(Ok(Message::Close(Some(frame)))) => {
                    assert_eq!(frame.code, CloseCode::Away);
                    found_close = true;
                    break;
                }
                Some(Ok(_)) => continue,
                _ => break,
            }
        }
        // tokio-tungstenite はサーバ発の Close を読み取った時点で内部的に
        // Close 応答をキューイングするが、実際の flush は次のポーリングで
        // 行われる（`tests/server_push_e2e.rs` の
        // `send_after_cancellation_returns_error_without_panicking` と同じ
        // 「二段構えの `next()` 呼び出し」パターン）。応答を送出しないまま
        // `client` を drop すると、サーバ側の `close_and_drain` が
        // `ResetWithoutClosingHandshake` を観測する（本イシューの変更とは
        // 無関係な既存の drain 挙動であり、本テストの検証対象ではないため
        // 明示的に応答を駆動する）。
        if found_close {
            let _ = client.next().await;
        }
        found_close
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel_tx.send(()).expect("cancel receiver must be alive");

    // `close_grace`（2 秒）よりも十分短い時間内にセッションが終わることを
    // 確認する（cancel 最優先ポーリングにより close_grace 満了を待たない）。
    let result = tokio::time::timeout(Duration::from_millis(800), server_task)
        .await
        .expect(
            "session must end well before close_grace (2s) elapses even while \
             the handler keeps pushing",
        )
        .unwrap();
    assert!(
        result.is_ok(),
        "cancellation should end the session normally: {result:?}"
    );

    let found_close = tokio::time::timeout(Duration::from_secs(2), drain_client)
        .await
        .expect("client drain task should finish within a bounded time")
        .expect("drain task should not panic");
    assert!(
        found_close,
        "client should observe the server-initiated close (1001 Going Away)"
    );
}
