//! WebSocket ハンドシェイクの受理判定フック（`WebSocketConfig::
//! with_handshake_check`、イシュー #716）のコア側統合テスト。
//!
//! `crates/core/src/plugin.rs` の `try_handle_upgrade` が accept したソケット
//! の実 peer address を `fandhe_backend_plugin_websocket::
//! handle_upgrade_with_peer_addr` へ渡す経路（イシュー #728、
//! `ws_peer_addr.rs` で検証済み）の先で、フックが実 peer address を観測
//! できること・拒否時に指定ステータスが返ることを `Server::bind().run()`
//! （本番の accept ループ）経由で検証する。プラグイン単体での網羅的な
//! 検証は `crates/plugin-websocket/tests/handshake_check_e2e.rs` が担う
//! （本テストはコア配線の end-to-end 証跡に限定する）。

#![cfg(feature = "websocket")]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use fandhe_backend_core::{Handler, Server};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_websocket::{WebSocketConfig, WsHandshakeContext};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// `Handler::handle` が呼ばれたら panic するトイハンドラ（`ws_peer_addr.rs`
/// と同型）。
struct NotCalledHandler;
impl Handler for NotCalledHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
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

/// `\r\n\r\n` までを読み切り、応答ヘッド部分を文字列として返す
/// （`ws_peer_addr.rs::read_response_head` と同型）。
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

/// EOF まで読み切る（拒否応答は `Connection: close` で送出される）。
async fn read_to_eof(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        let n = stream.read(&mut chunk).await.expect("read should succeed");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    buf
}

/// 実 TCP 経由で `Server::bind().run()` を駆動し、フックが観測した
/// `peer_addr()` がクライアントの `local_addr()` と一致すること
/// （受理判定フックにも `GateContext::peer_addr` と同一由来のアドレスが
/// 渡ること）を確認する。
#[tokio::test]
async fn real_bound_server_run_lets_hook_observe_actual_peer_addr() {
    let observed: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));
    let observed_for_hook = Arc::clone(&observed);
    let config =
        WebSocketConfig::default().with_handshake_check(move |ctx: &WsHandshakeContext<'_>| {
            *observed_for_hook.lock().unwrap() = ctx.peer_addr();
            Ok(())
        });
    let server = Server::new().websocket(config).handler(NotCalledHandler);
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = bound.run().await;
    });

    let mut stream = TcpStream::connect(addr).await.unwrap();
    let client_local_addr = stream.local_addr().unwrap();
    stream.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();

    let response_head = read_response_head(&mut stream).await;
    assert!(response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    assert_eq!(
        *observed.lock().unwrap(),
        Some(client_local_addr),
        "受理判定フックが観測した peer_addr はクライアントの local_addr と一致するはず"
    );
}

/// 実 TCP 経由でフックが拒否した場合、`Server::bind().run()` の接続でも
/// 指定ステータスが返り upgrade しないこと。
#[tokio::test]
async fn real_bound_server_run_returns_rejection_response_from_hook() {
    let config = WebSocketConfig::default()
        .with_handshake_check(|_ctx: &WsHandshakeContext<'_>| Err(Response::empty(404)));
    let server = Server::new().websocket(config).handler(NotCalledHandler);
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = bound.run().await;
    });

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();

    let text = String::from_utf8(read_to_eof(&mut stream).await).unwrap();
    assert!(text.starts_with("HTTP/1.1 404"));
    assert!(!text.contains("101 Switching Protocols"));
}
