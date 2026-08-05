//! `handle_upgrade` へ渡すキャンセル `Future` 経路の統合テスト（イシュー
//! #492・#499）。
//!
//! `idle_timeout.rs` と同様、`tokio::io::duplex` + `tokio-tungstenite`
//! クライアントで `handle_upgrade` を駆動する。キャンセルトリガには
//! `tokio::sync::oneshot`（`[dev-dependencies]` のみに `sync` feature を
//! 追加、本体依存グラフには影響しない）を使い、以下を検証する:
//!
//! 1. ハンドシェイク前に cancel 済み → 101 を送出せず即座に `Ok(())` 終了
//! 2. セッション確立後に発火 → Close frame（1001 Going Away）を受信し、
//!    クライアントが Close 応答を返せばサーバタスクが有界時間内に終了
//! 3. Close 応答を無視するクライアント → `CLOSE_GRACE` 以内にサーバタスクが
//!    終了する（フェイルクローズ）
//! 4. cancel が pending のまま通常の echo セッションが動作する（回帰ガード）
//! 5. （イシュー #499）ユーザーハンドラ実行中に発火 → ハンドラの `Future` が
//!    即座に drop され、Close frame（1001 Going Away）が有界時間内に届く
//! 6. （イシュー #499）5 の際、ハンドラ `Future` が実際に drop されたことを
//!    drop ガードで直接検証する
//! 7. （イシュー #499）`WsOutcome::Reply` 送出中（バックプレッシャで停滞）に
//!    発火 → 送出を打ち切り、後続の Close フレームが有効なバイト列として
//!    届く（ワイヤ安全性）
//! 8. （イシュー #499）7 と同様に送出停滞させたままクライアントが応答しない
//!    場合も `CLOSE_GRACE` 以内にサーバタスクが終了する（フェイルクローズ）
//! 9. （PR #504 レビュー指摘）`WsOutcome::Close` 送出中（`ws.close(None)`）に
//!    発火した場合、`handle_cancellation` が呼ぶ 2 回目の `ws.close` が
//!    `SendAfterClosing` で拒否されてもセッションが `Err` で終わらず正常
//!    終了すること（二重 close の回帰ガード）

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{WsMessage, WsMessageHandler, WsOutcome};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列
/// （`idle_timeout.rs` と同一のリクエスト）。
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
/// （`idle_timeout.rs` と同一のヘルパー）。
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

/// 受け入れ条件(1): ハンドシェイク開始前に既に発火済みのキャンセルを渡すと、
/// 101 応答を送出せずに即座に `Ok(())` で終了すること（クライアント側は
/// Switching Protocols を一切観測しない）。
#[tokio::test]
async fn cancelled_before_handshake_skips_101_response() {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let config = WebSocketConfig::default();

    let (server_side, mut client_side) = tokio::io::duplex(4096);

    // 既に解決済みの Future を渡す（`ready(())` は最初のポーリングで即
    // Ready を返す）。
    let server_task = tokio::spawn(async move {
        handle_upgrade(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::ready(()),
        )
        .await
    });

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish promptly")
        .unwrap();
    assert!(
        result.is_ok(),
        "cancelled-before-handshake should end normally: {result:?}"
    );

    // 101 応答が一切送出されていない（クライアント側が即座に EOF を観測する）
    // ことを確認する。
    let mut probe = [0u8; 1];
    let n = tokio::time::timeout(Duration::from_secs(2), client_side.read(&mut probe))
        .await
        .expect("client read should not hang")
        .expect("read should not error");
    assert_eq!(n, 0, "no bytes (including a 101 response) should be sent");
}

/// 受け入れ条件(2): セッション確立後にキャンセルが発火すると、サーバが
/// Close フレーム（1001 Going Away）を送出し、クライアントが Close 応答を
/// 返せばサーバタスクが有界時間内に `Ok(())` で終了すること。
#[tokio::test]
async fn cancellation_after_handshake_sends_close_frame_1001() {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let config = WebSocketConfig::default();

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

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    // セッション確立後にキャンセルを発火する。
    cancel_tx.send(()).unwrap();

    let received = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("close frame should arrive before test timeout")
        .expect("stream should yield a message")
        .expect("no protocol error");
    match received {
        Message::Close(Some(frame)) => assert_eq!(frame.code, CloseCode::Away),
        other => panic!("expected Close(Some(1001 Away)), got {other:?}"),
    }

    // Close 応答を返す（`idle_timeout.rs` と同じ駆動パターン: もう一度
    // `next()` を呼び、内部の応答フレーム送出を駆動させる）。
    let _ = client.next().await;

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within grace period")
        .unwrap();
    assert!(
        result.is_ok(),
        "cancellation should end the session normally: {result:?}"
    );
}

/// 受け入れ条件(3): キャンセル発火後、クライアントが Close 応答を返さなくて
/// も、サーバタスクが `CLOSE_GRACE`（実装内部定数・非公開、10 秒）以内に
/// 終了すること（フェイルクローズ。`idle_timeout.rs` の
/// `server_terminates_even_if_client_ignores_close` と同一パターン）。
#[tokio::test]
async fn cancellation_terminates_even_if_client_ignores_close() {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let config = WebSocketConfig::default();

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

    cancel_tx.send(()).unwrap();

    // クライアントは Close フレームを受信しても応答せず、接続を保持したまま
    // 放置する（drop すると duplex が EOF を返し `CLOSE_GRACE` を検証
    // できなくなるため、明示的に forget する）。
    std::mem::forget(client_side);

    let result = tokio::time::timeout(Duration::from_secs(15), server_task)
        .await
        .expect("server task must not hang beyond CLOSE_GRACE")
        .unwrap();
    assert!(
        result.is_ok(),
        "server must terminate within CLOSE_GRACE even if client ignores close: {result:?}"
    );
}

/// 受け入れ条件(4)（回帰ガード）: キャンセルが pending のまま（発火しない）
/// 場合、通常の echo セッションが従来どおり動作すること。
#[tokio::test]
async fn pending_cancellation_does_not_affect_normal_session() {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let config = WebSocketConfig::default();

    let (server_side, mut client_side) = tokio::io::duplex(64 * 1024);
    // 送信側を drop せず保持し、cancel 用 Future を無期限 pending にする。
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

    let server_task = tokio::spawn(async move {
        handle_upgrade(server_side, &head, Vec::new(), &config, async move {
            let _ = cancel_rx.await;
        })
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    client
        .send(Message::Text("hello".into()))
        .await
        .expect("send text");
    let echoed = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("echo should arrive before test timeout")
        .expect("echo response")
        .expect("no error");
    assert_eq!(echoed, Message::Text("hello".into()));

    client.close(None).await.expect("close");
    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish")
        .unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// ハンドラ実行中キャンセル（イシュー #499）を検証するためのトイハンドラ。
///
/// `on_message` は呼び出されたことを `started` の oneshot 送信で通知した
/// あと `std::future::pending()` を await し続ける（=完走しない長時間
/// ハンドラを模す）。`drop_flag` は `on_message` が返す `Future` の中で
/// `await` を跨いで保持する drop ガードで、キャンセルによって当該
/// `Future` が実際に drop されたことを直接検証できるようにする。
struct PendingHandlerWithDropGuard {
    started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    drop_flag: Arc<AtomicBool>,
}

/// `PendingHandlerWithDropGuard` の drop 時にフラグを立てる RAII ガード
/// （`on_message` の Future 内で保持し、Future の drop に連動させる）。
struct DropGuard(Arc<AtomicBool>);

impl Drop for DropGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl WsMessageHandler for PendingHandlerWithDropGuard {
    fn name(&self) -> &'static str {
        "pending-with-drop-guard"
    }

    fn on_message(
        &self,
        _msg: WsMessage,
    ) -> BoxFuture<'_, Result<WsOutcome, fandhe_backend_plugin_websocket::handler::WsHandlerError>>
    {
        let started = self.started.lock().unwrap().take();
        let guard = DropGuard(Arc::clone(&self.drop_flag));
        Box::pin(async move {
            let _guard = guard;
            if let Some(tx) = started {
                let _ = tx.send(());
            }
            std::future::pending::<()>().await;
            unreachable!("pending future must never resolve");
        })
    }
}

/// 受け入れ条件(5)(6)（イシュー #499）: ユーザーハンドラ `await` 中に
/// キャンセルが発火すると、ハンドラの `Future` が即座に drop され、Close
/// フレーム（1001 Going Away）が有界時間内に届くこと。drop ガードで
/// `Future` が実際に破棄されたことも直接検証する。
#[tokio::test]
async fn cancellation_during_handler_drops_future_and_sends_close_frame() {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };

    let drop_flag = Arc::new(AtomicBool::new(false));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let handler = PendingHandlerWithDropGuard {
        started: std::sync::Mutex::new(Some(started_tx)),
        drop_flag: Arc::clone(&drop_flag),
    };
    let config = WebSocketConfig::default().with_handler(handler);

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

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    // ハンドラを起動させる（呼ばれるまでキャンセルを発火しても意味がないため）。
    client
        .send(Message::Text("trigger".into()))
        .await
        .expect("send text");

    // ハンドラが実際に呼ばれ、await 中であることを oneshot で確認してから
    // キャンセルを発火する（タイミング依存の sleep を避ける）。
    tokio::time::timeout(Duration::from_secs(2), started_rx)
        .await
        .expect("handler should start before test timeout")
        .expect("handler should signal start");
    cancel_tx.send(()).unwrap();

    let received = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("close frame should arrive before test timeout")
        .expect("stream should yield a message")
        .expect("no protocol error");
    match received {
        Message::Close(Some(frame)) => assert_eq!(frame.code, CloseCode::Away),
        other => panic!("expected Close(Some(1001 Away)), got {other:?}"),
    }

    let _ = client.next().await;

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within grace period")
        .unwrap();
    assert!(
        result.is_ok(),
        "cancellation during handler execution should end the session normally: {result:?}"
    );

    // ハンドラの Future が実際に drop されたこと（打ち切り意味論の直接
    // 検証）。
    assert!(
        drop_flag.load(Ordering::SeqCst),
        "handler future must be dropped on cancellation"
    );
}

/// バックプレッシャで `WsOutcome::Reply` の送出を停滞させるハンドラ
/// （イシュー #499、受け入れ条件(7)(8)）。duplex の容量を小さく保った
/// テストと組み合わせ、`ws.send` が完了せず `await` し続ける状況を作る。
struct LargeReplyHandler {
    payload_len: usize,
}

impl WsMessageHandler for LargeReplyHandler {
    fn name(&self) -> &'static str {
        "large-reply"
    }

    fn on_message(
        &self,
        _msg: WsMessage,
    ) -> BoxFuture<'_, Result<WsOutcome, fandhe_backend_plugin_websocket::handler::WsHandlerError>>
    {
        let payload = vec![b'x'; self.payload_len];
        Box::pin(async move { Ok(WsOutcome::Reply(vec![WsMessage::Binary(payload)])) })
    }
}

/// 受け入れ条件(7)（イシュー #499）: `WsOutcome::Reply` 送出中（送信
/// バッファ満杯によるバックプレッシャで停滞）にキャンセルが発火すると、
/// 送出を打ち切り、その後クライアントが読み取りを再開すると有効な Close
/// フレーム（1001 Going Away）を受信できること（ワイヤ安全性: 打ち切り後
/// も破損したバイト列が流出しない）。
#[tokio::test]
async fn cancellation_during_reply_send_truncates_and_sends_valid_close_frame() {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };

    // 送出中に確実に停滞させるため、返信ペイロードを duplex 容量より
    // 大幅に大きくする。
    let handler = LargeReplyHandler {
        payload_len: 64 * 1024,
    };
    let config = WebSocketConfig::default()
        .with_max_message_size(8 * 1024 * 1024)
        .with_handler(handler);

    // 小さい duplex 容量でバックプレッシャを起こす。
    let (server_side, mut client_side) = tokio::io::duplex(4 * 1024);
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

    let server_task = tokio::spawn(async move {
        handle_upgrade(server_side, &head, Vec::new(), &config, async move {
            let _ = cancel_rx.await;
        })
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    client
        .send(Message::Text("trigger".into()))
        .await
        .expect("send text");

    // 送出が停滞し始めるだけの猶予を与えてからキャンセルを発火する。送出
    // 開始そのものの明示的な通知手段はないため、duplex 容量を明らかに
    // 上回るペイロードで自然に停滞する構成に依拠する。
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel_tx.send(()).unwrap();

    // クライアントが読み取りを再開すると、停滞していた返信の残りに続いて
    // 有効な Close フレームを受信できることを確認する（フレーミング境界が
    // 破損していれば tungstenite がプロトコルエラーを返す）。
    let mut saw_valid_close = false;
    for _ in 0..64 {
        match tokio::time::timeout(Duration::from_secs(5), client.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                assert_eq!(frame.code, CloseCode::Away);
                saw_valid_close = true;
                break;
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(err))) => panic!("wire must not be corrupted after truncation: {err}"),
            Ok(None) => break,
            Err(_) => panic!("client should not hang waiting for close frame"),
        }
    }
    assert!(saw_valid_close, "expected a valid 1001 Away close frame");

    // Close 応答を返す（`cancellation_after_handshake_sends_close_frame_1001`
    // と同じ駆動パターン: もう一度 `next()` を呼び、内部の応答フレーム
    // 送出を駆動させる）。
    let _ = client.next().await;

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within grace period")
        .unwrap();
    assert!(
        result.is_ok(),
        "cancellation during reply send should end the session normally: {result:?}"
    );
}

/// 受け入れ条件(8)（イシュー #499）: 返信送出停滞中にキャンセルが発火し、
/// クライアントがその後も応答しない場合でも `CLOSE_GRACE`
/// （実装内部定数・非公開、10 秒）以内にサーバタスクが終了すること
/// （フェイルクローズ）。
#[tokio::test]
async fn cancellation_during_reply_send_terminates_even_if_client_ignores_close() {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };

    let handler = LargeReplyHandler {
        payload_len: 64 * 1024,
    };
    let config = WebSocketConfig::default()
        .with_max_message_size(8 * 1024 * 1024)
        .with_handler(handler);

    let (server_side, mut client_side) = tokio::io::duplex(4 * 1024);
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

    let server_task = tokio::spawn(async move {
        handle_upgrade(server_side, &head, Vec::new(), &config, async move {
            let _ = cancel_rx.await;
        })
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    client
        .send(Message::Text("trigger".into()))
        .await
        .expect("send text");

    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel_tx.send(()).unwrap();

    // クライアントは以降一切読み取らず接続を保持したまま放置する（drop
    // すると duplex が EOF を返し CLOSE_GRACE を検証できなくなるため、
    // 明示的に forget する）。
    std::mem::forget(client);

    let result = tokio::time::timeout(Duration::from_secs(15), server_task)
        .await
        .expect("server task must not hang beyond CLOSE_GRACE")
        .unwrap();
    assert!(
        result.is_ok(),
        "server must terminate within CLOSE_GRACE even if reply send was truncated: {result:?}"
    );
}

/// 受信したメッセージに対して常に `WsOutcome::Close`（サーバ起点の Close
/// ハンドシェイク開始）を返すハンドラ（PR #504 レビュー指摘の回帰テスト用）。
struct CloseHandler;

impl WsMessageHandler for CloseHandler {
    fn name(&self) -> &'static str {
        "close-on-message"
    }

    fn on_message(
        &self,
        _msg: WsMessage,
    ) -> BoxFuture<'_, Result<WsOutcome, fandhe_backend_plugin_websocket::handler::WsHandlerError>>
    {
        Box::pin(async move { Ok(WsOutcome::Close) })
    }
}

/// PR #504 レビュー指摘（Bugbot、`crates/plugin-websocket/src/session.rs`
/// L246-252・L166-170・L302-308）の回帰テスト。
///
/// ハンドラが `WsOutcome::Close` を返し、`apply_outcome` の `ws.close(None)`
/// が（極小容量 duplex によるバックプレッシャで）実際の書き込み完了前に
/// キャンセルで打ち切られると、`SessionFlow::Cancelled` 経由で
/// `handle_cancellation` が呼ばれ `ws.close(Some(close_frame))` を 2 回目
/// 呼び出す。1 回目の呼び出しで tungstenite 内部状態が既に「クローズ送出
/// 済み」へ遷移していた場合、2 回目は `SendAfterClosing` エラーで拒否
/// される。`close_and_drain` がこれを致命的エラーとして扱うと
/// `run_session` 全体が `Err` で終了し、ドレイン・フラッシュがスキップ
/// される（修正前の不具合）。修正後は `SendAfterClosing` を
/// `ConnectionClosed` / `AlreadyClosed` と同様に成功として扱い、
/// セッションは正常終了 (`Ok(())`) する。
#[tokio::test]
async fn cancellation_during_close_send_does_not_fail_on_double_close() {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };

    let config = WebSocketConfig::default().with_handler(CloseHandler);

    // `ws.close(None)` のフレームはヘッダのみの 2 バイトと極小のため、
    // 確実にバックプレッシャで停滞させるには duplex 容量を 2 バイト未満
    // （1 バイト）にする必要がある（クライアントが読み取りを再開するまで
    // 書き込みが `Pending` のまま止まる）。
    let (server_side, mut client_side) = tokio::io::duplex(1);
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

    let server_task = tokio::spawn(async move {
        handle_upgrade(server_side, &head, Vec::new(), &config, async move {
            let _ = cancel_rx.await;
        })
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    client
        .send(Message::Text("trigger".into()))
        .await
        .expect("send text");

    // クライアントは意図的に読み取らず放置し、極小 duplex を埋めて
    // `ws.close(None)` の送出を停滞させる猶予を与えてからキャンセルを
    // 発火する。
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel_tx.send(()).unwrap();

    // クライアントが読み取りを再開すると、破損しない Close フレームを
    // 受信できること（2 回目の close 呼び出しによる不正な二重フレーム・
    // パニックが発生していないこと）を確認する。
    let mut saw_valid_close = false;
    for _ in 0..64 {
        match tokio::time::timeout(Duration::from_secs(5), client.next()).await {
            Ok(Some(Ok(Message::Close(_)))) => {
                saw_valid_close = true;
                break;
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(err))) => panic!("wire must not be corrupted after double close: {err}"),
            Ok(None) => break,
            Err(_) => panic!("client should not hang waiting for close frame"),
        }
    }
    assert!(saw_valid_close, "expected a close frame to arrive");

    let _ = client.next().await;

    let result = tokio::time::timeout(Duration::from_secs(15), server_task)
        .await
        .expect("server task must not hang beyond CLOSE_GRACE")
        .unwrap();
    assert!(
        result.is_ok(),
        "cancellation racing WsOutcome::Close send must end the session normally, \
         not fail on the resulting SendAfterClosing from the second close call: {result:?}"
    );
}
