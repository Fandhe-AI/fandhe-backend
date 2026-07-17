//! 委譲後の専用タスク再 spawn・permit 引き継ぎの統合テスト
//! （TASK-4.2 / #23【条件(1)】）。
//!
//! `crates/core/src/plugin.rs` の `try_handle_upgrade` が、WebSocket マッチ
//! 確定時に `handle_connection` の巨大な future をそのまま握り続けず、
//! セッション専用の小さなタスクへ切り離すことを次の 2 点で検証する:
//! - 元の `handle_connection` タスクがハンドシェイク直後に完了すること
//!   （その後もセッションが専用タスクで継続し、フレームの往復が成立する）
//! - 同時接続数上限の `OwnedSemaphorePermit` がセッション専用タスクへ
//!   move され、WS セッション生存中は `max_connections` のカウントに
//!   残り続けること（素朴な再 spawn だと漏れる DoS リグレッション）
//!
//! `websocket_upgrade.rs` の陽性テスト（ハンドシェイク成立・エコー）は
//! 既存のまま維持し、本ファイルは再 spawn 特有の観測可能な挙動に絞る。

#![cfg(feature = "websocket")]

use backend_framework_core::{Handler, Server, handle_connection};
use bf_http::request::RequestHead;
use bf_http::response::Response;
use bf_plugin_websocket::WebSocketConfig;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// `Handler::handle` が呼ばれたら panic するトイハンドラ（`UpgradeHandler` が
/// マッチした接続は既定 `Handler` へ到達しない契約の証跡、
/// `websocket_upgrade.rs` と同一パターン）。
struct NotCalledHandler;
impl Handler for NotCalledHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        panic!("UpgradeHandler がマッチしたのに既定 Handler が呼ばれた");
    }
}

const VALID_HANDSHAKE_REQUEST: &[u8] = b"GET /ws HTTP/1.1\r\n\
    Host: example.com\r\n\
    Upgrade: websocket\r\n\
    Connection: Upgrade\r\n\
    Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
    Sec-WebSocket-Version: 13\r\n\
    \r\n";

async fn read_response_head(stream: &mut TcpStream) -> String {
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
    String::from_utf8(buf).expect("response head must be valid utf-8")
}

fn masked_text_frame(payload: &[u8]) -> Vec<u8> {
    let mask = [0x12, 0x34, 0x56, 0x78];
    let mut frame = vec![0x81, 0x80 | (payload.len() as u8)];
    frame.extend_from_slice(&mask);
    for (i, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[i % 4]);
    }
    frame
}

async fn read_server_frame(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).await.unwrap();
    let opcode = header[0] & 0x0f;
    let len = (header[1] & 0x7f) as usize;
    assert_eq!(header[1] & 0x80, 0, "server frames must not be masked");
    let mut payload = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut payload).await.unwrap();
    }
    (opcode, payload)
}

/// 元の `handle_connection` タスクがハンドシェイク直後に完了し（＝大きな
/// ステートマシンが解放され）、その後もセッションが専用タスクで継続する
/// ことを確認する。
///
/// `handle_connection` の `JoinHandle` をハンドシェイク応答受信直後に
/// タイムアウト付きで await し、素早く完了することを確認する。素朴な
/// インライン await 実装であれば、この `JoinHandle` は WS セッションが
/// 終了する（Close フレーム往復まで）まで完了しないため、本テストは
/// 再 spawn の有無を区別できる。
#[tokio::test]
async fn handle_connection_task_completes_before_session_ends() {
    let server = Server::new()
        .websocket(WebSocketConfig::default())
        .handler(NotCalledHandler);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::sync::Arc::new(server);

    // 1 接続だけを受理し、`handle_connection` の `JoinHandle` をテスト側へ
    // 渡すための最小 accept（`spawn_server` ヘルパーは JoinHandle を外へ
    // 返さないため、本テスト専用に手書きする）。
    // 内側の `tokio::spawn` は意図的に await せず `JoinHandle` をそのまま
    // 外側タスクの戻り値として返す（テストが `handle_connection` タスクの
    // 完了だけを個別に観測するための構成）。`async_yields_async` は誤検出
    // として明示的に許容する。
    #[allow(clippy::async_yields_async)]
    let connection_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let server = server.clone();
        tokio::spawn(async move { handle_connection(&server, stream).await })
    });

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();

    let response_head = read_response_head(&mut stream).await;
    assert!(response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let handle_connection_join = connection_task
        .await
        .expect("accept タスクが handle_connection の JoinHandle を返すはず");

    // ハンドシェイク応答受信直後、`handle_connection` タスクは既に完了して
    // いるはず（再 spawn により大きな future が即座に解放される、
    // `crate::plugin::try_handle_upgrade` の doc「委譲後の専用タスク再
    // spawn」を参照）。まだクライアントは Close フレームを送っておらず、
    // 素朴なインライン await 実装ならここではまだ完了しない。
    timeout(Duration::from_secs(1), handle_connection_join)
        .await
        .expect(
            "handle_connection タスクはハンドシェイク直後に完了するはず\
             （専用タスク再 spawn により大きな future を保持し続けない）",
        )
        .expect("handle_connection タスクが panic せず完了すること");

    // `handle_connection` タスク完了後もセッションは専用タスクで継続して
    // おり、Text フレームのエコーが成立することを確認する。
    stream
        .write_all(&masked_text_frame(b"still alive"))
        .await
        .unwrap();
    let (opcode, payload) = read_server_frame(&mut stream).await;
    assert_eq!(opcode, 0x1, "expected Text opcode echoed back");
    assert_eq!(payload, b"still alive");
}

/// permit の引き継ぎ（TASK-4.2 / #23「permit の契約」）を実接続の
/// `max_connections` 経路で検証する。
///
/// `max_connections(1)` の下で WS セッションを張ったまま 2 本目の接続を
/// 試みると、素朴な再 spawn（permit を move しない実装）であれば
/// `handle_connection` の戻りと同時に permit が解放され、2 本目が即座に
/// 受理されてしまう。permit がセッション専用タスクへ正しく move されて
/// いれば、WS セッションを閉じるまで 2 本目は受理されない。
#[tokio::test]
async fn websocket_session_holds_connection_permit_until_closed() {
    // client2 は非 WS パス（`/`）へ到達させるが、ハンドラは意図的に未登録
    // のままにする。未登録時は既定 `Handler::handle` を呼ばず直接 404 を
    // 返す契約（`crates/core/src/server.rs` の `handle_connection` を参照）
    // のため、`NotCalledHandler` を使わずとも安全に 404 を観測できる。
    let server = Server::new()
        .max_connections(1)
        .websocket(WebSocketConfig::default());
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = bound.run().await;
    });

    // client1: WS へアップグレードし、唯一の permit をセッションタスクへ
    // 引き継がせる。
    let mut client1 = TcpStream::connect(addr).await.unwrap();
    client1.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();
    let response_head = read_response_head(&mut client1).await;
    assert!(response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    // client2: permit が枯渇しているため、run() の accept ループはまだ
    // この接続を受理していないはず（`max_connections_limits_concurrent_accept`
    // と同一の観測パターン、`crates/core/src/server.rs` を参照）。
    let mut client2 = TcpStream::connect(addr).await.unwrap();
    client2
        .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut probe = [0u8; 1];
    let no_response_yet = timeout(Duration::from_millis(300), client2.read(&mut probe)).await;
    assert!(
        no_response_yet.is_err(),
        "WS セッションが permit を保持している間、client2 はまだ応答を受け取らないはず\
         （permit がセッション専用タスクへ move されていない場合はここで漏れる）"
    );

    // client1（WS セッション）を閉じ、セッション専用タスクの終了と同時に
    // permit が解放されることを確認する。
    drop(client1);

    let mut out = Vec::new();
    timeout(Duration::from_secs(2), client2.read_to_end(&mut out))
        .await
        .expect("WS セッション終了後は permit が解放され、client2 が受理されるはず")
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
}
