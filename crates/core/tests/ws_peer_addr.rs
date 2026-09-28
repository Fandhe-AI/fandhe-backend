//! WebSocket Upgrade 経路への接続元アドレス受け渡し（イシュー #728）の
//! コア側統合テスト。
//!
//! `crates/core/src/plugin.rs` の `try_handle_upgrade` が accept したソケット
//! の実 peer address を `fandhe_backend_plugin_websocket::
//! handle_upgrade_with_peer_addr` へ渡し、`WsOpenContext::peer_addr()` から
//! 観測できることを、`crates/core/tests/websocket_upgrade.rs` と同型の
//! 生 TCP + 手書きフレームで検証する。
//!
//! - `real_bound_server_run_passes_actual_peer_addr_to_on_open`（AC1）:
//!   `Server::bind().run()`（本番経路）でクライアントの `local_addr()` と
//!   `on_open` が観測した `peer_addr()` が一致することを確認する
//! - `handle_connection_without_peer_addr_yields_none`（AC2）:
//!   実 TCP 接続でも `handle_connection`（`peer_addr` を注入しない公開 API）
//!   経由では常に `None` になることを固定する

#![cfg(feature = "websocket")]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_plugin_websocket::WebSocketConfig;
use fandhe_backend_plugin_websocket::handler::{
    WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// `Handler::handle` が呼ばれたら panic するトイハンドラ（`UpgradeHandler`
/// がマッチした接続は既定 `Handler` へ到達しない契約の証跡、
/// `websocket_upgrade.rs::NotCalledHandler` と同型）。
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
/// （`websocket_upgrade.rs::read_response_head` と同型）。
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

/// サーバから届く 1 フレームを読み取り、opcode とペイロードを返す
/// （`websocket_upgrade.rs::read_server_frame` と同型）。
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

/// `on_open` で観測した `peer_addr()` を記録し、同期用の push を送るハンドラ
/// （クライアント側は push を受け取った時点で `on_open` 実行済みと分かる）。
struct RecordPeerAddrHandler {
    observed: Arc<Mutex<Option<Option<SocketAddr>>>>,
}

impl WsMessageHandler for RecordPeerAddrHandler {
    fn name(&self) -> &'static str {
        "record-peer-addr"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        *self.observed.lock().unwrap() = Some(ctx.peer_addr());
        let sender = ctx.sender().clone();
        tokio::spawn(async move {
            let _ = sender.send(WsMessage::Text("ready".to_string())).await;
        });
    }

    fn on_message(
        &self,
        msg: WsMessage,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WsOutcome, WsHandlerError>> + Send + '_>,
    > {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

/// AC1: 実ソケット経路（`Server::bind().run()`、本番の accept ループ）で
/// 接続すると、`on_open` の `WsOpenContext::peer_addr()` がクライアントの
/// `local_addr()` と一致する `Some` を返すこと。
#[tokio::test]
async fn real_bound_server_run_passes_actual_peer_addr_to_on_open() {
    let observed = Arc::new(Mutex::new(None));
    let server = Server::new()
        .websocket(
            WebSocketConfig::default().with_handler(RecordPeerAddrHandler {
                observed: Arc::clone(&observed),
            }),
        )
        .handler(NotCalledHandler);
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

    // `on_open` が push する同期フレームを待つ（`on_open` 実行完了の証跡）。
    let (opcode, payload) = read_server_frame(&mut stream).await;
    assert_eq!(opcode, 0x1, "expected Text opcode for sync push");
    assert_eq!(payload, b"ready");

    assert_eq!(
        *observed.lock().unwrap(),
        Some(Some(client_local_addr)),
        "on_open が観測した peer_addr はクライアントの local_addr と一致するはず"
    );
}

/// AC2: `handle_connection`（`peer_addr` を注入しない公開 API）は実 TCP
/// ソケット経由の接続でも `WsOpenContext::peer_addr()` が常に `None` に
/// なること（フェイルクローズ契約、`GateContext::peer_addr` と同型）。
#[tokio::test]
async fn handle_connection_without_peer_addr_yields_none() {
    let observed = Arc::new(Mutex::new(None));
    let server = Arc::new(
        Server::new()
            .websocket(
                WebSocketConfig::default().with_handler(RecordPeerAddrHandler {
                    observed: Arc::clone(&observed),
                }),
            )
            .handler(NotCalledHandler),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let server = Arc::clone(&server);
        tokio::spawn(async move { handle_connection(&server, stream).await });
    });

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();

    let response_head = read_response_head(&mut stream).await;
    assert!(response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let (opcode, payload) = read_server_frame(&mut stream).await;
    assert_eq!(opcode, 0x1);
    assert_eq!(payload, b"ready");

    assert_eq!(
        *observed.lock().unwrap(),
        Some(None),
        "handle_connection 経由（peer_addr 未注入）は常に None であるはず"
    );
}
