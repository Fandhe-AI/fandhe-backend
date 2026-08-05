//! アイドルタイムアウトの統合テスト（Issue #175）。
//!
//! `handshake_e2e.rs` と同様、`tokio::io::duplex` + `tokio-tungstenite`
//! クライアントで `handle_upgrade` を駆動し、`WebSocketConfig::idle_timeout`
//! の発火・非発火・無効化・Ping による維持・Close 無視クライアントへの
//! 猶予（`WebSocketConfig::close_grace`）を検証する。イシュー #500 で
//! `close_grace` を利用者設定可能にした際、アイドルタイムアウト経路への
//! 配線退行がないことも本ファイルで検証する。

use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列を返す
/// （`handshake_e2e.rs` と同一のリクエストを使う）。
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
/// （`handshake_e2e.rs` と同一のヘルパー）。
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
/// `WebSocketStream` とサーバタスクの `JoinHandle` を返す。
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

/// 受け入れ基準(1)(2): 無通信が `idle_timeout` を超えたら、サーバが
/// Close フレーム（1000 Normal Closure）を送出し、クライアントの Close
/// 応答後にサーバタスクが `Ok(())` で終了すること。
#[tokio::test]
async fn idle_timeout_closes_connection_with_normal_close_frame() {
    let config = WebSocketConfig::default().with_idle_timeout(Duration::from_millis(200));
    let (mut client, server_task) = handshake(config).await;

    // クライアントは無通信のまま待つ。タイムアウト超過分の余裕を見て 2 秒待つ。
    let received = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("server should send close before test timeout")
        .expect("stream should yield a message")
        .expect("no protocol error");

    match received {
        Message::Close(Some(frame)) => {
            assert_eq!(
                frame.code,
                tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal
            );
        }
        Message::Close(None) => {}
        other => panic!("expected Close frame on idle timeout, got {other:?}"),
    }

    // tokio-tungstenite は Close 受信直後は自動応答フレームを内部に溜めるのみで、
    // 実際の書き込みは次回の `read`/`flush` 駆動時に行われる（tungstenite の
    // 仕様。`WebSocketState::write` は既に `ClosedByPeer` の状態では
    // `Message::Close` の明示送信自体を `SendAfterClosing` として拒むため、
    // `close()` の再呼び出しでは応答が飛ばない）。もう一度 `next()` を呼び、
    // その内部の `read` 呼び出しで Close 応答フレームの送出を駆動させる。
    let _ = client.next().await;

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within grace period")
        .unwrap();
    assert!(
        result.is_ok(),
        "idle timeout should end the session normally: {result:?}"
    );
}

/// 受け入れ基準(4) 非発火: タイムアウトを跨いで通信を継続していれば、
/// 切断されずにエコーが継続すること。
#[tokio::test]
async fn ongoing_communication_prevents_idle_timeout() {
    let config = WebSocketConfig::default().with_idle_timeout(Duration::from_millis(500));
    let (mut client, server_task) = handshake(config).await;

    // タイムアウト（500ms）を大きく超える期間（計 1.5 秒超）、100ms 間隔で
    // 送信を継続する。各回のエコーが正常に届けば、アイドル判定されずに
    // タイマーが実質リセットされ続けていることになる。
    for i in 0..16 {
        client
            .send(Message::Text(format!("msg-{i}").into()))
            .await
            .expect("send text");
        let echoed = tokio::time::timeout(Duration::from_secs(1), client.next())
            .await
            .expect("echo should arrive before test timeout")
            .expect("echo response")
            .expect("no error");
        assert_eq!(echoed, Message::Text(format!("msg-{i}").into()));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // クライアント主導で正常終了させる。
    client.close(None).await.expect("close");
    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish")
        .unwrap();
    assert!(
        result.is_ok(),
        "session with ongoing communication should end cleanly: {result:?}"
    );
}

/// 受け入れ基準(1): Ping のみを送り続けるクライアントはデータを送らなくても
/// 接続が維持されること（Ping/Pong を含む全フレーム受信を活動とみなす）。
#[tokio::test]
async fn ping_only_traffic_keeps_connection_alive() {
    let config = WebSocketConfig::default().with_idle_timeout(Duration::from_millis(300));
    let (mut client, server_task) = handshake(config).await;

    // タイムアウト（300ms）を超える期間、100ms 間隔で Ping のみを送信する。
    for _ in 0..8 {
        client
            .send(Message::Ping(Vec::new().into()))
            .await
            .expect("send ping");
        // tungstenite サーバ側は Pong を自動応答するため受信して読み捨てる。
        let reply = tokio::time::timeout(Duration::from_secs(1), client.next())
            .await
            .expect("pong should arrive before test timeout")
            .expect("pong response")
            .expect("no error");
        assert!(matches!(reply, Message::Pong(_)));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // 接続がまだ生きていることをテキスト送受信で確認してからクライアント
    // 主導で終了する。
    client
        .send(Message::Text("still-alive".into()))
        .await
        .expect("send text");
    let echoed = tokio::time::timeout(Duration::from_secs(1), client.next())
        .await
        .expect("echo should arrive")
        .expect("echo response")
        .expect("no error");
    assert_eq!(echoed, Message::Text("still-alive".into()));

    client.close(None).await.expect("close");
    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish")
        .unwrap();
    assert!(
        result.is_ok(),
        "ping-sustained session should end cleanly: {result:?}"
    );
}

/// 受け入れ基準(1): `without_idle_timeout()` で無効化した場合、
/// タイムアウト値相当を無通信で超えても Close が送出されないこと
/// （有界な `tokio::time::timeout` で確認し、テスト自体が無期限に
/// ハングしないようにする）。
#[tokio::test]
async fn disabled_idle_timeout_does_not_close_connection() {
    let config = WebSocketConfig::default()
        .with_idle_timeout(Duration::from_millis(100))
        .without_idle_timeout();
    assert_eq!(config.idle_timeout, None);
    let (mut client, server_task) = handshake(config).await;

    // 無効化前のタイムアウト値（100ms）を大きく超える 500ms 待っても、
    // サーバから何も届かない（Close されない）ことを確認する。
    let outcome = tokio::time::timeout(Duration::from_millis(500), client.next()).await;
    assert!(
        outcome.is_err(),
        "disabled idle timeout must not close the connection"
    );

    // クライアント主導で終了させ、タスクがクリーンアップされることを確認する。
    client.close(None).await.expect("close");
    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish after client-initiated close")
        .unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// 受け入れ基準(2): タイムアウト発火後にクライアントが Close 応答を返さ
/// なくても、サーバタスクが猶予（`WebSocketConfig::close_grace`、既定
/// 10 秒）内に終了すること（二次 DoS 対策）。ここでは十分な余裕（15 秒）を
/// 見た有界待ちで検証する。
#[tokio::test]
async fn server_terminates_even_if_client_ignores_close() {
    let config = WebSocketConfig::default().with_idle_timeout(Duration::from_millis(200));
    let (client, server_task) = handshake(config).await;

    // クライアントは Close フレームを受信しても応答せず、ストリームを
    // 保持したまま放置する（drop すると duplex が EOF を返し close_grace
    // を検証できなくなるため、明示的に forget して接続を握ったままにする）。
    std::mem::forget(client);

    let result = tokio::time::timeout(Duration::from_secs(15), server_task)
        .await
        .expect("server task must not hang beyond close_grace")
        .unwrap();
    assert!(
        result.is_ok(),
        "server must terminate within close_grace even if client ignores close: {result:?}"
    );
}

/// 受け入れ基準(3)（イシュー #500）: `with_idle_timeout` +
/// `with_close_grace` を併用した場合、アイドルタイムアウト経路（
/// `handle_idle_timeout`）でも設定した `close_grace` が反映されること。
/// `close_and_drain` は `handle_idle_timeout` / `handle_cancellation` の
/// 共有ヘルパーだが、アイドル経路側の配線退行も個別に防ぐ。
#[tokio::test]
async fn configured_close_grace_is_applied_on_idle_timeout() {
    let config = WebSocketConfig::default()
        .with_idle_timeout(Duration::from_millis(200))
        .with_close_grace(Duration::from_millis(200));
    let (client, server_task) = handshake(config).await;

    // Close 応答を返さず接続を保持したまま放置する（上記と同じ理由）。
    std::mem::forget(client);

    let result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("configured close_grace (200ms) must terminate well within the default 10s window")
        .unwrap();
    assert!(
        result.is_ok(),
        "server must terminate within the configured close_grace: {result:?}"
    );
}
