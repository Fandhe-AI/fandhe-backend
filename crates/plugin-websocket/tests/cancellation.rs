//! `handle_upgrade` へ渡すキャンセル `Future` 経路の統合テスト（イシュー
//! #492）。
//!
//! `idle_timeout.rs` と同様、`tokio::io::duplex` + `tokio-tungstenite`
//! クライアントで `handle_upgrade` を駆動する。キャンセルトリガには
//! `tokio::sync::oneshot`（`[dev-dependencies]` のみに `sync` feature を
//! 追加、本体依存グラフには影響しない）を使い、以下を検証する:
//!
//! 1. ハンドシェイク前に cancel 済み → 101 を送出せず即座に `Ok(())` 終了
//! 2. セッション確立後に発火 → Close frame（1001 Going Away）を受信し、
//!    クライアントが Close 応答を返せばサーバタスクが有界時間内に終了
//! 3. Close 応答を無視するクライアント → `WebSocketConfig::close_grace`
//!    （既定 10 秒）以内にサーバタスクが終了する（フェイルクローズ）
//! 4. cancel が pending のまま通常の echo セッションが動作する（回帰ガード）
//! 5. （イシュー #500）`with_close_grace` で設定した猶予が実際に適用される
//!    こと（短縮した猶予・`Duration::ZERO` の両方を検証）

use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
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
/// も、サーバタスクが `WebSocketConfig::close_grace`（既定 10 秒）以内に
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
    // 放置する（drop すると duplex が EOF を返し close_grace の効果を検証
    // できなくなるため、明示的に forget する）。
    std::mem::forget(client_side);

    let result = tokio::time::timeout(Duration::from_secs(15), server_task)
        .await
        .expect("server task must not hang beyond close_grace")
        .unwrap();
    assert!(
        result.is_ok(),
        "server must terminate within close_grace even if client ignores close: {result:?}"
    );
}

/// 受け入れ条件(5)（イシュー #500）: `WebSocketConfig::with_close_grace` で
/// 既定（10 秒）より大幅に短い猶予を設定した場合、その値が実際に
/// `close_and_drain` へ反映されること。Close 応答を無視するクライアントに
/// 対しても、既定 10 秒よりずっと短い外側タイムアウト（5 秒）以内にサーバ
/// タスクが終了することで、設定値が有効化されていることを証明する
/// （既定のまま反映されていなければ 5 秒では終了しない）。
#[tokio::test]
async fn configured_close_grace_is_applied_on_cancellation() {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let config = WebSocketConfig::default().with_close_grace(Duration::from_millis(200));

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

    // Close 応答を返さず接続を保持したまま放置する（上記(3)と同じ理由）。
    std::mem::forget(client_side);

    let result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("configured close_grace (200ms) must terminate well within the default 10s window")
        .unwrap();
    assert!(
        result.is_ok(),
        "server must terminate within the configured close_grace: {result:?}"
    );
}

/// 受け入れ条件(5)（イシュー #500・0 の扱い）: `close_grace` に
/// `Duration::ZERO` を設定すると、Close 送出後のドレインを即座に打ち切って
/// 終端すること（doc に明記した「0 は安全側の即終端」という契約の実挙動
/// 保証）。
#[tokio::test]
async fn zero_close_grace_terminates_immediately() {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let config = WebSocketConfig::default().with_close_grace(Duration::ZERO);

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
    std::mem::forget(client_side);

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("close_grace = Duration::ZERO must terminate immediately")
        .unwrap();
    assert!(
        result.is_ok(),
        "server must terminate immediately with close_grace = Duration::ZERO: {result:?}"
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
