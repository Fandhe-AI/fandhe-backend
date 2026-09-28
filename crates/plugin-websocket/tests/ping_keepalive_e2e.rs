//! サーバー起点 Ping keepalive の統合テスト（イシュー #713、親 #712）。
//!
//! `idle_timeout.rs` / `on_close_e2e.rs` と同様、`tokio::io::duplex` +
//! `tokio-tungstenite` クライアントで `handle_upgrade` を実際に駆動する。
//! `WebSocketConfig::with_ping_interval` は `idle_timeout` とは独立した
//! 死活監視のため、`idle_timeout` を無効化した構成で Ping keepalive 単体の
//! 挙動を検証する（両者を組み合わせた場合の推奨設定・doc・組み合わせ
//! テストは #714 のスコープ）。
//!
//! 実時間ではなく仮想時間（`#[tokio::test(start_paused = true)]`）で駆動し、
//! テスト自体が実時間で待たずに決定的に終わることを保証する。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    CloseReason, WsConnContext, WsHandlerError, WsMessage, WsMessageHandler, WsOutcome,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::{Bytes, Message};

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

/// ハンドシェイクを成立させ、101 応答を読み切ったクライアント
/// `WebSocketStream` とサーバタスクの `JoinHandle` を返す
/// （`idle_timeout.rs::handshake` と同一のヘルパー）。
async fn handshake(
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

    let client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    (client, server_task)
}

/// `on_close` で通知された `CloseReason` を記録するだけのトイハンドラ
/// （`EchoHandler` と同じくメッセージはそのまま返送する。`on_close_e2e.rs`
/// の `Recording`（`shared: Arc<Shared>` を保持し呼び出し元へも同じ
/// `Arc` を渡すパターン）を踏襲する。`WebSocketConfig::with_handler` は
/// ハンドラを値で受け取り内部で `Arc<dyn WsMessageHandler>` へ包むため、
/// 呼び出し元が結果を読み取るには、ハンドラ自身ではなく内部の共有状態を
/// `Arc` で持たせて clone を手元に残す必要がある）。
struct RecordClose {
    reason: Arc<Mutex<Option<CloseReason>>>,
}

impl WsMessageHandler for RecordClose {
    fn name(&self) -> &'static str {
        "record-close"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }

    fn on_close(&self, _ctx: &WsConnContext, reason: CloseReason) {
        *self
            .reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reason);
    }
}

/// Text 受信ごとに `delay` だけ `await` してから同じ内容を返送するハンドラ。
/// ハンドラ実行中に届いたフレームは、ハンドラが完了して次の受信待ちに戻る
/// までサーバーが読まないことを検証するために使う（`crate::session` モジュール
/// doc「サーバー起点 Ping keepalive」節）。
struct SlowEcho {
    delay: Duration,
}

impl WsMessageHandler for SlowEcho {
    fn name(&self) -> &'static str {
        "slow-echo"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        let delay = self.delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Ok(WsOutcome::Reply(vec![msg]))
        })
    }
}

/// 受け入れ基準 4（既定は無効）: `with_ping_interval` を呼ばない既定構成
/// では、長い仮想時間を進めても Ping が 1 つも送出されないこと。
#[tokio::test(start_paused = true)]
async fn disabled_by_default_sends_no_ping() {
    let config = WebSocketConfig::default().without_idle_timeout();
    let (mut client, server_task) = handshake(config).await;

    // 通常の keepalive 間隔として想定される値を大きく超える期間、何も
    // 届かないことを確認する（有界な `timeout` で、テスト自体が無期限に
    // ハングしないようにする）。
    let outcome = tokio::time::timeout(Duration::from_secs(120), client.next()).await;
    assert!(
        outcome.is_err(),
        "disabled ping keepalive must not send anything even after a long idle period"
    );

    client.close(None).await.expect("close");
    let result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server task should finish after client-initiated close")
        .unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// 受け入れ基準 1・3: 有効化すると `interval` ごとに Ping が届き、
/// クライアントが読み続けて（tungstenite が自動で Pong を返す）いる限り、
/// `pong_timeout` の何倍もの時間が経過しても接続が維持されること。
#[tokio::test(start_paused = true)]
async fn client_reading_keeps_connection_alive_across_many_intervals() {
    let interval = Duration::from_secs(10);
    let pong_timeout = Duration::from_secs(5);
    let config = WebSocketConfig::default()
        .without_idle_timeout()
        .with_ping_interval(interval, pong_timeout)
        .unwrap();
    let (mut client, server_task) = handshake(config).await;

    // interval の 5 倍を超える期間、Ping を受け取り続けられること
    // （tokio-tungstenite クライアントは `next()` 呼び出し時に Ping へ
    // 自動で Pong を返す）。
    for i in 0..5 {
        let received = tokio::time::timeout(interval * 2, client.next())
            .await
            .unwrap_or_else(|_| panic!("ping #{i} should arrive within 2x interval"))
            .expect("stream should yield a message")
            .expect("no protocol error");
        assert!(
            matches!(received, Message::Ping(_)),
            "expected Ping frame, got {received:?}"
        );
    }

    client.close(None).await.expect("close");
    let result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server task should finish")
        .unwrap();
    assert!(
        result.is_ok(),
        "session kept alive by ping/pong should end cleanly on client close: {result:?}"
    );
}

/// 受け入れ基準 2: Ping 送出後 `pong_timeout` 以内に Pong が届かない
/// クライアント（読み取りを止めている）は切断され、`on_close` が
/// `CloseReason::PongTimeout` でちょうど 1 回呼ばれること。
///
/// クライアントは `client.next()` を一切呼ばないため、tungstenite の
/// 自動 Pong 応答（読み取り駆動）が発生せず、サーバー側の Pong 期限が
/// 確実に切れる。接続自体は `forget` して保持したままにし（drop による
/// EOF を発生させない、`idle_timeout.rs::server_terminates_even_if_client_
/// ignores_close` と同じ理由）、Pong 期限の発火のみで切断されることを
/// 確認する。
#[tokio::test(start_paused = true)]
async fn unresponsive_client_is_closed_with_pong_timeout() {
    let interval = Duration::from_millis(200);
    let pong_timeout = Duration::from_millis(100);
    // Close ハンドシェイクのドレインは既定 10 秒（`DEFAULT_CLOSE_GRACE`）を
    // 上限に応答を待つ。読み取りを止めたクライアント（本テスト）からは
    // 応答が来ないため、有界なテストにするには明示的に短い値へ変更する
    // 必要がある（`idle_timeout.rs::server_terminates_even_if_client_
    // ignores_close` と同じ理由）。
    let close_grace = Duration::from_millis(100);
    let reason = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default()
        .without_idle_timeout()
        .with_ping_interval(interval, pong_timeout)
        .unwrap()
        .with_close_grace(close_grace)
        .with_handler(RecordClose {
            reason: reason.clone(),
        });
    let (client, server_task) = handshake(config).await;

    std::mem::forget(client);

    let result = tokio::time::timeout(interval + pong_timeout + close_grace * 4, server_task)
        .await
        .expect("server must not hang: pong timeout + close_grace bound the wait")
        .unwrap();
    assert!(
        result.is_ok(),
        "pong timeout is policy-driven, not a protocol error: {result:?}"
    );
    assert_eq!(
        *reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        Some(CloseReason::PongTimeout),
        "on_close must report PongTimeout exactly once"
    );
}

/// 受け入れ基準 2・3 の境界事例: `pong_timeout` より長く実行されるハンドラの
/// 実行中に Text → Pong の順で届いていた場合でも、誤って `PongTimeout` に
/// しないこと。
///
/// `crate::session` モジュール doc「サーバー起点 Ping keepalive」節が述べる
/// とおり、受信は本モジュール内の 1 か所で逐次処理するため、ハンドラ実行中に
/// 届いた Pong はハンドラが完了して次の受信待ちに戻るまで読まれない。この間
/// Pong 期限（送出済み Ping への応答期限）を過ぎていても、`ws.next()` を優先
/// する契約により、ハンドラ完了直後に読まれるバッファ済みの Pong で期限は
/// 解除され、誤切断しない。
#[tokio::test(start_paused = true)]
async fn slow_handler_does_not_lose_pong_buffered_during_handler_execution() {
    let interval = Duration::from_millis(200);
    let pong_timeout = Duration::from_millis(100);
    let handler_delay = Duration::from_millis(500);
    let config = WebSocketConfig::default()
        .without_idle_timeout()
        .with_ping_interval(interval, pong_timeout)
        .unwrap()
        .with_handler(SlowEcho {
            delay: handler_delay,
        });
    let (mut client, server_task) = handshake(config).await;

    // 最初の Ping を受け取り、未応答の Ping（Pong 期限）が立った状態にする。
    let first = tokio::time::timeout(interval * 2, client.next())
        .await
        .expect("first ping should arrive")
        .expect("stream should yield a message")
        .expect("no protocol error");
    assert!(
        matches!(first, Message::Ping(_)),
        "expected Ping frame, got {first:?}"
    );

    // Text を送ってハンドラ（500ms スリープ）を起動したあと、サーバーが
    // それを読み切ってから戻ってくるまでの間に Pong を送る。Pong 期限
    // （100ms）はハンドラのスリープ中に過ぎるが、サーバーはハンドラ完了後の
    // 次の受信待ちでこの Pong をまず読むため、誤切断してはならない。
    client
        .send(Message::Text("hi".into()))
        .await
        .expect("client send should succeed");
    client
        .send(Message::Pong(Bytes::new()))
        .await
        .expect("client send should succeed");

    let reply = tokio::time::timeout(handler_delay * 4, client.next())
        .await
        .expect("handler should finish and reply within a bounded time")
        .expect("stream should yield a message (a premature close would end the stream instead)")
        .expect("no protocol error");
    assert!(
        matches!(reply, Message::Text(_)),
        "expected the echoed reply, got {reply:?} (a premature PongTimeout close would not reply)"
    );

    client.close(None).await.expect("close");
    let result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server task should finish")
        .unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// 受け入れ基準 2 の境界事例（契約 4 の固定）: Pong を送らず Text だけを
/// 送り続けるクライアントは切断されないこと。
///
/// `ws.next()` が常に Ready になる（フレームが途切れず届く）限り Pong 期限
/// の判定自体が受信待ちの race に至らないため、`idle_timeout` と同じ
/// 「受信し続ける限り生存扱い」という契約になる（意図した挙動、
/// `crate::session` モジュール doc を参照）。
#[tokio::test(start_paused = true)]
async fn client_sending_text_without_pong_is_not_disconnected() {
    let interval = Duration::from_millis(50);
    let pong_timeout = Duration::from_millis(30);
    let config = WebSocketConfig::default()
        .without_idle_timeout()
        .with_ping_interval(interval, pong_timeout)
        .unwrap();
    let (mut client, server_task) = handshake(config).await;

    // pong_timeout の何倍もの期間、Pong を送らず Text だけを送り続ける。
    for i in 0..10 {
        client
            .send(Message::Text(format!("msg-{i}").into()))
            .await
            .expect("client send should succeed");
        let echoed = tokio::time::timeout(pong_timeout * 10, client.next())
            .await
            .unwrap_or_else(|_| panic!("echo #{i} should arrive (must not be disconnected)"))
            .expect("stream should yield a message")
            .expect("no protocol error");
        assert_eq!(
            echoed,
            Message::Text(format!("msg-{i}").into()),
            "expected echoed text, got a different frame (possibly a premature close)"
        );
    }

    client.close(None).await.expect("close");
    let result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server task should finish")
        .unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}
