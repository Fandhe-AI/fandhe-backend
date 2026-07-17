//! E2E テスト（TASK-4.1 / #22）: `tokio::io::duplex` + `tokio-tungstenite`
//! クライアントでハンドシェイク成立 → メッセージエコー → Close を検証する。
//!
//! `crates/core` の統合テスト（`crates/core/tests/websocket_upgrade.rs`）は
//! 生 TCP + 手書きフレームでコア配線を検証する一方、本テストは本クレート
//! 単体の `handle_upgrade` が実際の WebSocket クライアント実装
//! （tokio-tungstenite）と相互運用できることを検証する。
//!
//! `handle_upgrade` は「リクエストヘッドは呼び出し元が既に解析済み」という
//! 契約（コアの `RequestGate` → `UpgradeHandler` 評価後に委譲される、
//! `crates/core/src/server.rs` の doc を参照）のため、本テストのクライアント
//! 側は HTTP リクエスト行を実際に書き込まない。代わりに、サーバ側が返す
//! 101 応答をクライアント側で読み切ってから `WebSocketStream::from_raw_socket`
//! （ハンドシェイクなし）に切り替えてフレームをやり取りする。

use bf_http::request::{ParseOutcome, parse_request_head};
use bf_plugin_websocket::{WebSocketConfig, handle_upgrade};
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列を返す。
fn handshake_request_bytes() -> &'static [u8] {
    b"GET /ws HTTP/1.1\r\n\
      Host: example.com\r\n\
      Upgrade: websocket\r\n\
      Connection: Upgrade\r\n\
      Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
      Sec-WebSocket-Version: 13\r\n\
      \r\n"
}

/// クライアント側ストリームから `\r\n\r\n` までを読み切り、101 応答本体を
/// 文字列として返す（残余バイトはストリームに残さない。テストの範囲では
/// サーバは 101 応答の直後にフレームを送らないため単純化できる）。
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

#[tokio::test]
async fn handshake_succeeds_and_echoes_text_and_binary_messages() {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let config = WebSocketConfig::default();

    let (server_side, mut client_side) = tokio::io::duplex(64 * 1024);

    let server_task =
        tokio::spawn(async move { handle_upgrade(server_side, &head, Vec::new(), &config).await });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
    assert!(response.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    client
        .send(Message::Text("hello".into()))
        .await
        .expect("send text");
    let echoed = client
        .next()
        .await
        .expect("echo response")
        .expect("no error");
    assert_eq!(echoed, Message::Text("hello".into()));

    client
        .send(Message::Binary(vec![1, 2, 3].into()))
        .await
        .expect("send binary");
    let echoed = client
        .next()
        .await
        .expect("echo response")
        .expect("no error");
    assert_eq!(echoed, Message::Binary(vec![1, 2, 3].into()));

    client.close(None).await.expect("close");

    let result = server_task.await.unwrap();
    assert!(
        result.is_ok(),
        "server session should end cleanly: {result:?}"
    );
}

#[tokio::test]
async fn leftover_bytes_from_pipelined_frame_are_not_lost() {
    // クライアントが 101 応答を待たずに最初のフレームを先行送信した
    // ケース（HTTP/1.1 パイプライン相当）を模す。コア側はこの先行バイト列を
    // `RecvBuffer::unread()` で取得し `leftover` として `handle_upgrade` へ
    // 渡す契約（Issue #22 実装計画 3.1 節）。
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let config = WebSocketConfig::default();

    // マスク付き Text フレーム "hi" を先行バイト列として用意する
    // （クライアント→サーバのフレームは RFC 6455 5.1 によりマスク必須）。
    let leftover = {
        let (a, mut b) = tokio::io::duplex(4096);
        let mut probe: WebSocketStream<_> =
            WebSocketStream::from_raw_socket(a, Role::Client, None).await;
        probe
            .send(Message::Text("hi".into()))
            .await
            .expect("send probe frame");
        drop(probe);
        let mut buf = Vec::new();
        use tokio::io::AsyncReadExt as _;
        // duplex の書き込み側は writer 破棄後も既に書き込んだバイト列を
        // 読み出せる（バッファ済みのため）。
        let _ = tokio::time::timeout(std::time::Duration::from_millis(50), async {
            let mut tmp = [0u8; 4096];
            if let Ok(n) = b.read(&mut tmp).await {
                buf.extend_from_slice(&tmp[..n]);
            }
        })
        .await;
        buf
    };
    assert!(!leftover.is_empty(), "probe frame must produce bytes");

    let (server_side, mut client_side) = tokio::io::duplex(64 * 1024);

    let server_task =
        tokio::spawn(async move { handle_upgrade(server_side, &head, leftover, &config).await });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    // 先行送信済みの "hi" がエコーされること（取りこぼされていないこと）を
    // 確認する。
    let echoed = client
        .next()
        .await
        .expect("echo response for pipelined frame")
        .expect("no error");
    assert_eq!(echoed, Message::Text("hi".into()));

    client.close(None).await.expect("close");

    let result = server_task.await.unwrap();
    assert!(
        result.is_ok(),
        "server session should end cleanly: {result:?}"
    );
}
