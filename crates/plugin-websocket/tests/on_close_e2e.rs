//! 切断通知フック `WsMessageHandler::on_close` の統合テスト（イシュー #729、
//! 親 #705）。
//!
//! `cancellation.rs` / `idle_timeout.rs` と同様、`tokio::io::duplex` +
//! `tokio-tungstenite` クライアントで `handle_upgrade` を実際に駆動する。
//! `on_close` はセッションタスク内で同期的に呼ばれるため、判定は必ず
//! サーバタスクの `JoinHandle` を `tokio::time::timeout` 付きで await し
//! 終えたあとに行う（`sleep` に依存するフレーク対策。join できた時点で
//! `on_close` は実行済みであることが保証される）。
//!
//! 検証する経路（全終了経路を網羅、`docs/design/
//! ws-connection-context-and-close.md` 4 節の脱出点対応表と対応）:
//!
//! 1. クライアントの Close → `on_close_client_close_called_once`
//! 2. EOF（Close なしの切断） → `on_close_eof_called_once`
//! 3. idle timeout → `on_close_idle_timeout_called_once`
//! 4. shutdown/rebind キャンセル → `on_close_cancelled_called_once`
//! 5. プロトコルエラー → `on_close_protocol_error_called_once`
//! 6. ハンドラが返す Close → `on_close_handler_close_called_once`
//! 7. ハンドラのエラー → `on_close_handler_error_called_once`
//! 8. 受信サイズ上限超過 → `on_close_message_too_large_called_once`
//!
//! さらに、`on_open` が呼ばれていない接続（ハンドシェイク失敗・101 送出前
//! キャンセル）では `on_close` も呼ばれないこと（フェイルクローズの対称性）、
//! 既定 `on_close`（no-op）を実装しないハンドラ（`EchoHandler`）が無変更で
//! 動作すること（受け入れ基準 4 の回帰ガード）も確認する。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    CloseReason, WsConnContext, WsConnId, WsHandlerError, WsMessage, WsMessageHandler,
    WsOpenContext, WsOutcome,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, WsError, handle_upgrade};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列
/// （`cancellation.rs`/`idle_timeout.rs` と同一のリクエスト）。
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
/// （`cancellation.rs` と同一のヘルパー）。
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

/// 101 応答（`handshake::serialize_101`）を厳密に検証する共通ヘルパー
/// （AGENTS.md「アサーション網羅性」節: ステータス行・ヘッダ・ボディを
/// すべて検証する。PR #732 レビュー指摘対応）。
///
/// 本ファイルの全テストが送る `handshake_request_bytes` の
/// `Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==` は RFC 6455 4.2.2 の
/// 既知ベクタで、対応する `Sec-WebSocket-Accept` は
/// `s3pPLMBiTxaQ9kYGzzhZRbK+xOo=` に固定される（`handshake.rs` の
/// `serialize_101_produces_expected_headers` と同一の期待値）。
/// `read_http_response_line` は応答全体を `\r\n\r\n` まで読み切る契約
/// のため、この完全一致は「ステータス行」「Upgrade / Connection /
/// Sec-WebSocket-Accept ヘッダ」「ボディなし（ヘッダ終端直後で応答が
/// 終わる）」の 3 点を同時に保証する。
fn assert_101_response(response: &str) {
    assert_eq!(
        response,
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\
         \r\n",
        "101 response must exactly match handshake::serialize_101's output \
         (status line + Upgrade/Connection/Sec-WebSocket-Accept headers, no body)"
    );
}

/// 426 応答（`handshake::serialize_426`）を厳密に検証する共通ヘルパー
/// （`Sec-WebSocket-Version` 不一致時のフェイルクローズ応答。
/// PR #732 レビュー指摘対応）。
///
/// `Content-Length: 0` を含む固定テンプレートと完全一致させることで、
/// ステータス行・`Sec-WebSocket-Version`/`Connection`/`Content-Length`
/// ヘッダに加え、ボディが空であることも保証する。
fn assert_426_response(response: &str) {
    assert_eq!(
        response,
        "HTTP/1.1 426 Upgrade Required\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Connection: close\r\n\
         Content-Length: 0\r\n\
         \r\n",
        "426 response must exactly match handshake::serialize_426's output \
         (status line + Sec-WebSocket-Version/Connection/Content-Length headers, empty body)"
    );
}

/// `RequestHead` を都度パースするヘルパー（各テストが所有権を持てるよう
/// `head` を返す。`fandhe_backend_http::request::RequestHead` は `Clone` を
/// 実装しないため、テストごとに再パースする）。
fn parse_head() -> fandhe_backend_http::request::RequestHead {
    match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    }
}

/// `on_open`/`on_close` の呼び出し記録を保持する共有状態。
#[derive(Default)]
struct Shared {
    opens: Mutex<Vec<WsConnId>>,
    closes: Mutex<Vec<(WsConnId, CloseReason)>>,
}

/// ハンドラの挙動（受信メッセージに対して何を返すか）。テストごとに
/// 切り替える。
enum Behavior {
    /// 受信メッセージをそのまま返送する（`EchoHandler` 相当）。
    Echo,
    /// 受信メッセージを無視して `WsOutcome::Close` を返す。
    Close,
    /// 受信メッセージを無視して `WsHandlerError` を返す。
    Err,
}

/// `on_open`/`on_close` の呼び出しを記録するトイハンドラ。
struct Recording {
    shared: Arc<Shared>,
    behavior: Behavior,
}

impl WsMessageHandler for Recording {
    fn name(&self) -> &'static str {
        "recording"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move {
            match self.behavior {
                Behavior::Echo => Ok(WsOutcome::Reply(vec![msg])),
                Behavior::Close => Ok(WsOutcome::Close),
                Behavior::Err => Err(WsHandlerError::new("boom")),
            }
        })
    }

    fn on_open(&self, ctx: WsOpenContext) {
        self.shared.opens.lock().unwrap().push(ctx.conn_id());
    }

    fn on_close(&self, ctx: &WsConnContext, reason: CloseReason) {
        self.shared
            .closes
            .lock()
            .unwrap()
            .push((ctx.conn_id(), reason));
    }
}

/// テスト 1: クライアントが Close フレームを送出すると、`on_close` が
/// ちょうど 1 回・`CloseReason::ClientClose` で呼ばれ、`handle_upgrade` は
/// `Ok(())` を返すこと。
#[tokio::test]
async fn on_close_client_close_called_once() {
    let head = parse_head();
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default().with_handler(Recording {
        shared: Arc::clone(&shared),
        behavior: Behavior::Echo,
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

    let response = read_http_response_line(&mut client_side).await;
    assert_101_response(&response);

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client.close(None).await.expect("client close should send");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok(), "session should end normally: {result:?}");

    let opens = shared.opens.lock().unwrap();
    let closes = shared.closes.lock().unwrap();
    assert_eq!(opens.len(), 1, "on_open should be called exactly once");
    assert_eq!(closes.len(), 1, "on_close should be called exactly once");
    assert_eq!(closes[0], (opens[0], CloseReason::ClientClose));
}

/// テスト 2: Close ハンドシェイクなしにクライアント側を drop すると
/// （EOF）、`on_close` が `CloseReason::Eof` でちょうど 1 回呼ばれること。
/// tokio-tungstenite 0.30 では `Protocol(ResetWithoutClosingHandshake)` が
/// 観測される主経路（`Result` は `Err`）であり、`Eof` へ分類されることを
/// 合わせて確認する（`docs/design/ws-connection-context-and-close.md` 4 節）。
#[tokio::test]
async fn on_close_eof_called_once() {
    let head = parse_head();
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default().with_handler(Recording {
        shared: Arc::clone(&shared),
        behavior: Behavior::Echo,
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

    let response = read_http_response_line(&mut client_side).await;
    assert_101_response(&response);

    // Close ハンドシェイクを行わず、クライアント側の生ストリームを drop
    // する（`WebSocketStream` へ包まないため Close フレームは送出されない）。
    drop(client_side);

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");

    let opens = shared.opens.lock().unwrap();
    let closes = shared.closes.lock().unwrap();
    assert_eq!(opens.len(), 1, "on_open should be called exactly once");
    assert_eq!(closes.len(), 1, "on_close should be called exactly once");
    assert_eq!(
        closes[0].1,
        CloseReason::Eof,
        "expected Eof, got {:?}",
        closes[0].1
    );
    // `result` は経路によって `Ok`/`Err` いずれもありうる
    // （`CloseReason::Eof` の doc・4 節を参照）。ここでは reason の一致のみを
    // 主張し、`Result` の値は固定しない。
    let _ = result;
}

/// テスト 3: `WebSocketConfig::idle_timeout` が発火すると、`on_close` が
/// `CloseReason::IdleTimeout` でちょうど 1 回呼ばれること。
#[tokio::test]
async fn on_close_idle_timeout_called_once() {
    let head = parse_head();
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default()
        .with_idle_timeout(Duration::from_millis(100))
        .with_close_grace(Duration::from_millis(300))
        .with_handler(Recording {
            shared: Arc::clone(&shared),
            behavior: Behavior::Echo,
        });

    let (server_side, client_side) = tokio::io::duplex(4096);
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

    let mut client_side = client_side;
    let response = read_http_response_line(&mut client_side).await;
    assert_101_response(&response);

    // クライアントは何も送らず接続を保持したまま放置する（idle timeout を
    // 発火させるため drop しない）。
    std::mem::forget(client_side);

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok(), "session should end normally: {result:?}");

    let closes = shared.closes.lock().unwrap();
    assert_eq!(closes.len(), 1, "on_close should be called exactly once");
    assert_eq!(closes[0].1, CloseReason::IdleTimeout);
}

/// テスト 4: コアの世代キャンセルシグナルが発火すると、`on_close` が
/// `CloseReason::Cancelled` でちょうど 1 回呼ばれること。
#[tokio::test]
async fn on_close_cancelled_called_once() {
    let head = parse_head();
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default().with_handler(Recording {
        shared: Arc::clone(&shared),
        behavior: Behavior::Echo,
    });

    let (server_side, mut client_side) = tokio::io::duplex(64 * 1024);
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

    let server_task = tokio::spawn(async move {
        handle_upgrade(server_side, &head, Vec::new(), &config, async move {
            let _ = cancel_rx.await;
        })
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert_101_response(&response);

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    cancel_tx.send(()).unwrap();

    // サーバが送る Close フレーム（1001 Going Away）を受信し、応答を返す。
    let _ = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("close frame should arrive before test timeout");
    let _ = client.next().await;

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within grace period")
        .expect("task should not panic");
    assert!(result.is_ok(), "session should end normally: {result:?}");

    let closes = shared.closes.lock().unwrap();
    assert_eq!(closes.len(), 1, "on_close should be called exactly once");
    assert_eq!(closes[0].1, CloseReason::Cancelled);
}

/// テスト 5: プロトコル違反（マスクなしフレーム）を受信すると、`on_close`
/// が `CloseReason::Failed(FailureKind::Protocol)` でちょうど 1 回呼ばれる
/// こと。tokio-tungstenite 0.30 はサーバーロールで既定
/// `accept_unmasked_frames = false` のため、マスクなし Text フレームを
/// `ProtocolError::UnmaskedFrameFromClient` として拒否する。
#[tokio::test]
async fn on_close_protocol_error_called_once() {
    let head = parse_head();
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default().with_handler(Recording {
        shared: Arc::clone(&shared),
        behavior: Behavior::Echo,
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

    let response = read_http_response_line(&mut client_side).await;
    assert_101_response(&response);

    // マスクなし Text フレーム（FIN=1, opcode=1, payload len=2, "hi"）を
    // 生バイトで直接書き込む（`WebSocketStream` を経由するとクライアント
    // ロールで自動的にマスクされてしまうため）。
    use tokio::io::AsyncWriteExt;
    client_side
        .write_all(&[0x81, 0x02, b'h', b'i'])
        .await
        .expect("write unmasked frame");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(
        result.is_err(),
        "protocol violation should surface as an error: {result:?}"
    );

    let closes = shared.closes.lock().unwrap();
    assert_eq!(closes.len(), 1, "on_close should be called exactly once");
    assert_eq!(
        closes[0].1,
        CloseReason::Failed(fandhe_backend_plugin_websocket::handler::FailureKind::Protocol)
    );
}

/// テスト 6: ハンドラが `WsOutcome::Close` を返すと、`on_close` が
/// `CloseReason::HandlerClose` でちょうど 1 回呼ばれること。
#[tokio::test]
async fn on_close_handler_close_called_once() {
    let head = parse_head();
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default().with_handler(Recording {
        shared: Arc::clone(&shared),
        behavior: Behavior::Close,
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

    let response = read_http_response_line(&mut client_side).await;
    assert_101_response(&response);

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client
        .send(Message::Text("trigger".into()))
        .await
        .expect("send text");

    // サーバ起点の Close を受け取り、応答する。
    let _ = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("close frame should arrive before test timeout");
    let _ = client.next().await;

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok(), "session should end normally: {result:?}");

    let closes = shared.closes.lock().unwrap();
    assert_eq!(closes.len(), 1, "on_close should be called exactly once");
    assert_eq!(closes[0].1, CloseReason::HandlerClose);
}

/// テスト 7: ハンドラが `Err` を返すと、`on_close` が
/// `CloseReason::Failed(FailureKind::Handler)` でちょうど 1 回呼ばれること。
#[tokio::test]
async fn on_close_handler_error_called_once() {
    let head = parse_head();
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default().with_handler(Recording {
        shared: Arc::clone(&shared),
        behavior: Behavior::Err,
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

    let response = read_http_response_line(&mut client_side).await;
    assert_101_response(&response);

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client
        .send(Message::Text("trigger".into()))
        .await
        .expect("send text");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(
        matches!(result, Err(WsError::Handler(_))),
        "handler error should surface as WsError::Handler: {result:?}"
    );

    let closes = shared.closes.lock().unwrap();
    assert_eq!(closes.len(), 1, "on_close should be called exactly once");
    assert_eq!(
        closes[0].1,
        CloseReason::Failed(fandhe_backend_plugin_websocket::handler::FailureKind::Handler)
    );
}

/// テスト 8: 受信メッセージが `max_message_size`/`max_frame_size` を超過
/// すると、`on_close` が `CloseReason::MessageTooLarge` でちょうど 1 回
/// 呼ばれること。
#[tokio::test]
async fn on_close_message_too_large_called_once() {
    let head = parse_head();
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default()
        .with_max_message_size(64)
        .with_max_frame_size(64)
        .with_handler(Recording {
            shared: Arc::clone(&shared),
            behavior: Behavior::Echo,
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
    assert_101_response(&response);

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    let oversized = "x".repeat(256);
    client
        .send(Message::Text(oversized.into()))
        .await
        .expect("client-side send does not enforce the server's max_frame_size");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(
        result.is_err(),
        "oversized message should surface as an error: {result:?}"
    );

    let closes = shared.closes.lock().unwrap();
    assert_eq!(closes.len(), 1, "on_close should be called exactly once");
    assert_eq!(closes[0].1, CloseReason::MessageTooLarge);
}

/// テスト 9（フェイルクローズの対称性）: ハンドシェイク検証失敗
/// （`Sec-WebSocket-Version` 不一致で 426 応答）の接続では、`on_open` も
/// `on_close` も呼ばれないこと。
#[tokio::test]
async fn on_close_not_called_on_handshake_failure() {
    let bad_request = b"GET /ws HTTP/1.1\r\n\
      Host: example.com\r\n\
      Upgrade: websocket\r\n\
      Connection: Upgrade\r\n\
      Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
      Sec-WebSocket-Version: 8\r\n\
      \r\n";
    let head = match parse_request_head(bad_request).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default().with_handler(Recording {
        shared: Arc::clone(&shared),
        behavior: Behavior::Echo,
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

    // 426 応答（Upgrade Required）を読み切る（`read_http_response_line` は
    // ステータス行を含むヘッダ全体を `\r\n\r\n` まで読む共通ヘルパー）。
    let response = read_http_response_line(&mut client_side).await;
    assert_426_response(&response);

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(
        result.is_err(),
        "handshake failure should surface as an error"
    );

    assert_eq!(
        shared.opens.lock().unwrap().len(),
        0,
        "on_open must not be called"
    );
    assert_eq!(
        shared.closes.lock().unwrap().len(),
        0,
        "on_close must not be called"
    );
}

/// テスト 10（フェイルクローズの対称性）: 101 応答送出前にキャンセルが
/// 既に発火済みの接続では、`on_open` も `on_close` も呼ばれないこと。
#[tokio::test]
async fn on_close_not_called_when_cancelled_before_101() {
    let head = parse_head();
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default().with_handler(Recording {
        shared: Arc::clone(&shared),
        behavior: Behavior::Echo,
    });

    let (server_side, _client_side) = tokio::io::duplex(4096);
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
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(
        result.is_ok(),
        "cancelled-before-handshake should end normally: {result:?}"
    );

    assert_eq!(
        shared.opens.lock().unwrap().len(),
        0,
        "on_open must not be called"
    );
    assert_eq!(
        shared.closes.lock().unwrap().len(),
        0,
        "on_close must not be called"
    );
}

/// 受け入れ基準 4（回帰ガード）: `on_close` を実装しない既定ハンドラ
/// （`EchoHandler`）が無変更のままコンパイル・動作すること。
#[tokio::test]
async fn on_close_default_noop_keeps_echo_working() {
    let head = parse_head();
    let config = WebSocketConfig::default();

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

    let response = read_http_response_line(&mut client_side).await;
    assert_101_response(&response);

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client
        .send(Message::Text("hello".into()))
        .await
        .expect("send text");
    let echoed = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("echo should arrive before test timeout")
        .expect("stream should not end")
        .expect("no error");
    assert_eq!(echoed, Message::Text("hello".into()));

    client.close(None).await.expect("close");
    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok(), "session should end normally: {result:?}");
}
