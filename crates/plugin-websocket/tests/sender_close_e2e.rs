//! `WsSender::close` の E2E テスト（イシュー #710、親 #708「サーバー起点で
//! 任意タイミングに Close を送れる WebSocket API」）。
//!
//! `handler.rs` の doc test・単体テストは `handler::channel` を直接使う内部
//! 経路（順序保証・検証・2 回目以降の呼び出し等）を検証済み。本ファイルは
//! `sender_closed_e2e.rs` / `server_push_e2e.rs` と同様に `handle_upgrade`
//! を実際に駆動し、`on_open` から spawn したタスクが `WsSender::send` →
//! `WsSender::close` を呼ぶ経路を通しで確認する。
//!
//! 受け入れ基準（イシュー #710 本文）:
//! 1. close 呼び出し前にキューへ入った push は、すべてクライアントへ届いてから
//!    Close フレームが届く（順序保証）
//! 2. 指定した close code と reason がクライアントに届く
//! 3. close 後の `send` は Closed エラーになる
//! 4. これらを検証するテストがある（本ファイル）

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    CloseReason, WsConnContext, WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext,
    WsOutcome, WsSendError, WsSender,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列
/// （`sender_closed_e2e.rs` / `server_push_e2e.rs` と同一のテスト用固定
/// リクエスト）。
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
/// （`sender_closed_e2e.rs` と同一のヘルパー）。
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
/// サーバ側 `handle_upgrade` タスクを返す（`sender_closed_e2e.rs` と同一
/// 実装、キャンセルは `std::future::pending` で無期限 pending にする）。
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

/// `on_open` で捕捉した `WsSender` を使って push を複数回送ってから
/// `close(code, reason)` を呼ぶハンドラ。呼び出し後の `send` 結果と
/// `on_close` の記録を外部から検証できるように、両方を共有スロットへ書く。
struct PushThenCloseHandler {
    push_count: usize,
    close_code: u16,
    close_reason: &'static str,
    /// close 後に送った `send` の結果（検証用）。
    post_close_send: Arc<Mutex<Option<Result<(), WsSendError>>>>,
    /// `on_close` が記録した終了理由（ちょうど 1 回のはず）。
    close_reasons: Arc<Mutex<Vec<CloseReason>>>,
    /// spawn したタスクが完走したことを知らせる。
    done_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl WsMessageHandler for PushThenCloseHandler {
    fn name(&self) -> &'static str {
        "push-then-close-e2e"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        let sender: WsSender = ctx.sender().clone();
        let push_count = self.push_count;
        let close_code = self.close_code;
        let close_reason = self.close_reason;
        let post_close_send = Arc::clone(&self.post_close_send);
        let done_tx = self.done_tx.lock().unwrap().take();
        tokio::spawn(async move {
            // 容量（既定 8）を超える件数を push し、バックプレッシャの経路を
            // 実際に通す（受け入れ基準 1 の「順序保証」を有意に検証するため）。
            for i in 0..push_count {
                sender
                    .send(WsMessage::Text(format!("push-{i}")))
                    .await
                    .expect("push should succeed before close");
            }
            sender
                .close(close_code, close_reason)
                .await
                .expect("close should succeed");

            // close 後の send は Closed エラーになる（受け入れ基準 3）。
            let result = sender.send(WsMessage::Text("late".to_string())).await;
            *post_close_send.lock().unwrap() = Some(result);

            if let Some(tx) = done_tx {
                let _ = tx.send(());
            }
        });
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }

    fn on_close(&self, _ctx: &WsConnContext, reason: CloseReason) {
        self.close_reasons.lock().unwrap().push(reason);
    }
}

/// 受け入れ基準 1・2: push 20 件（容量 8 超）→ `close(4001, "target
/// destroyed")` の順で呼ぶと、クライアントは 20 件を順に受け取ったあと
/// 一致する code・reason の Close フレームを受け取る。
#[tokio::test]
async fn pushes_arrive_in_order_before_close_frame() {
    const PUSH_COUNT: usize = 20;

    let post_close_send = Arc::new(Mutex::new(None));
    let close_reasons = Arc::new(Mutex::new(Vec::new()));
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();

    let config = WebSocketConfig::default().with_handler(PushThenCloseHandler {
        push_count: PUSH_COUNT,
        close_code: 4001,
        close_reason: "target destroyed",
        post_close_send: Arc::clone(&post_close_send),
        close_reasons: Arc::clone(&close_reasons),
        done_tx: Mutex::new(Some(done_tx)),
    });

    let (mut client, server_task) = spawn_session(config).await;

    for i in 0..PUSH_COUNT {
        let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("push should arrive within timeout")
            .expect("stream should not end before all pushes arrive")
            .expect("frame should not error");
        assert_eq!(
            msg.into_text().unwrap().as_str(),
            format!("push-{i}"),
            "push {i} arrived out of order or was lost"
        );
    }

    let close_frame = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("close frame should arrive within timeout")
        .expect("stream should not end before close frame")
        .expect("frame should not error");
    match close_frame {
        Message::Close(Some(frame)) => {
            assert_eq!(u16::from(frame.code), 4001);
            assert_eq!(frame.reason.as_str(), "target destroyed");
        }
        other => panic!("expected a close frame with code/reason, got {other:?}"),
    }

    // クライアント側も Close 応答を返し、サーバのドレインを完了させる
    // （`sender_closed_e2e.rs` と同じパターン: 受信済み Close フレームに
    // 対する tungstenite の内部自動応答を、もう一度 `next()` を呼んで
    // フラッシュさせる。明示的な `client.close(None)` は不要）。
    let _ = client.next().await;

    tokio::time::timeout(Duration::from_secs(2), done_rx)
        .await
        .expect("handler task should finish within timeout")
        .expect("handler task should not be dropped without sending");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within timeout")
        .unwrap();
    assert!(
        result.is_ok(),
        "sender-initiated close should end the session normally: {result:?}"
    );

    // 受け入れ基準 3: close 後の send は Closed エラー（`WsSendError`）。
    assert_eq!(
        post_close_send.lock().unwrap().take(),
        Some(Err(WsSendError))
    );

    // `on_close` はちょうど 1 回、`CloseReason::SenderClose` で呼ばれる。
    assert_eq!(
        close_reasons.lock().unwrap().as_slice(),
        &[CloseReason::SenderClose]
    );
}
