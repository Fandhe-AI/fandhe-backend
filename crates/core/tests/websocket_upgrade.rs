//! `websocket` feature（TASK-4.1 / #22）配線の統合テスト（feature 有効側）。
//!
//! `crates/core/src/plugin.rs` の非公開 `try_handle_upgrade` シームが実際に
//! `fandhe_backend_plugin_websocket::handle_upgrade` へ委譲し、`GET /ws`（既定パス）への
//! アップグレードが `UpgradeHandler` 拡張点経由で成立することを、モック
//! クライアントを生 TCP + 手書きフレームで駆動する `handle_connection` を
//! 通して検証する。
//!
//! コアの dev-dependencies に `tokio-tungstenite` を増やさない方針
//! （Issue #22 実装計画 5 節）のため、クライアント側フレームは RFC 6455 の
//! マスク規則に従い最小限（Text/Close のみ）を手書きする。
//!
//! feature 無効時の陰性対照は `websocket_upgrade_disabled.rs` を参照。

#![cfg(feature = "websocket")]

use fandhe_backend_core::{
    GateContext, GateOutcome, Handler, RequestGate, Server, handle_connection,
};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_websocket::WebSocketConfig;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// `Handler::handle` が呼ばれたら panic するトイハンドラ。
///
/// `UpgradeHandler` がマッチした接続は既定 `Handler` へ到達しない契約
/// （`crates/core/src/server.rs` の `handle_connection` を参照）の証跡に使う。
struct NotCalledHandler;
impl Handler for NotCalledHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        panic!("UpgradeHandler がマッチしたのに既定 Handler が呼ばれた");
    }
}

/// 常に拒否する `RequestGate`（評価順 `RequestGate` → `UpgradeHandler` の
/// 固定確認用、フェイルクローズ、`.claude/rules/security.md`）。
struct DenyAllGate;
impl RequestGate for DenyAllGate {
    fn name(&self) -> &'static str {
        "deny-all"
    }
    fn check(&self, _head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
        GateOutcome::reject(403, Vec::new())
    }
}

/// `127.0.0.1:0` に bind した実サーバへ `handle_connection` を 1 接続ずつ
/// spawn する最小 accept ループ。テスト全体で生 TCP を使う理由は Issue #22
/// 実装計画 5 節（コアの dev-dependencies にクライアント実装を増やさないため）。
async fn spawn_server(server: Server) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::sync::Arc::new(server);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let server = server.clone();
            tokio::spawn(async move { handle_connection(&server, stream).await });
        }
    });
    addr
}

const VALID_HANDSHAKE_REQUEST: &[u8] = b"GET /ws HTTP/1.1\r\n\
    Host: example.com\r\n\
    Upgrade: websocket\r\n\
    Connection: Upgrade\r\n\
    Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
    Sec-WebSocket-Version: 13\r\n\
    \r\n";

/// `\r\n\r\n` までを読み切り、応答ヘッド部分を文字列として返す。
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

/// マスク付き Text フレーム（RFC 6455 5.2・5.3）を組み立てる。
/// クライアント→サーバのフレームはマスク必須。
fn masked_text_frame(payload: &[u8]) -> Vec<u8> {
    let mask = [0x12, 0x34, 0x56, 0x78];
    let mut frame = vec![0x81, 0x80 | (payload.len() as u8)];
    frame.extend_from_slice(&mask);
    for (i, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[i % 4]);
    }
    frame
}

/// マスク付き Close フレーム（payload なし）を組み立てる。
fn masked_close_frame() -> Vec<u8> {
    let mask = [0x00, 0x00, 0x00, 0x00];
    vec![0x88, 0x80, mask[0], mask[1], mask[2], mask[3]]
}

/// サーバから届く 1 フレームを読み取り、opcode とペイロードを返す
/// （サーバ→クライアントはマスクなし、RFC 6455 5.1）。
async fn read_server_frame(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).await.unwrap();
    let opcode = header[0] & 0x0f;
    let len = (header[1] & 0x7f) as usize;
    assert_eq!(header[1] & 0x80, 0, "server frames must not be masked");
    // テストで使う payload は 125 バイト未満のみ（拡張長は扱わない）。
    let mut payload = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut payload).await.unwrap();
    }
    (opcode, payload)
}

#[tokio::test]
async fn upgrade_succeeds_and_echoes_text_frame() {
    let server = Server::new()
        .websocket(WebSocketConfig::default())
        .handler(NotCalledHandler);
    let addr = spawn_server(server).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();

    let response_head = read_response_head(&mut stream).await;
    assert!(response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
    assert!(response_head.contains("Upgrade: websocket\r\n"));
    assert!(response_head.contains("Connection: Upgrade\r\n"));
    // RFC 6455 4.2.2 の既知ベクタ（`crates/plugin-websocket` 単体テストと同一値）。
    assert!(response_head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));

    stream.write_all(&masked_text_frame(b"hi")).await.unwrap();
    let (opcode, payload) = read_server_frame(&mut stream).await;
    assert_eq!(opcode, 0x1, "expected Text opcode echoed back");
    assert_eq!(payload, b"hi");

    stream.write_all(&masked_close_frame()).await.unwrap();
    // Close 応答を待ってから接続が閉じることを確認する（EOF まで読み切る）。
    let mut trailing = Vec::new();
    let _ = stream.read_to_end(&mut trailing).await;
}

#[tokio::test]
async fn missing_sec_websocket_key_is_rejected_with_400() {
    let server = Server::new()
        .websocket(WebSocketConfig::default())
        .handler(NotCalledHandler);
    let addr = spawn_server(server).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = b"GET /ws HTTP/1.1\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Version: 13\r\n\
        \r\n";
    stream.write_all(request).await.unwrap();

    let response_head = read_response_head(&mut stream).await;
    assert!(response_head.starts_with("HTTP/1.1 400 Bad Request\r\n"));
}

#[tokio::test]
async fn unsupported_version_is_rejected_with_426() {
    let server = Server::new()
        .websocket(WebSocketConfig::default())
        .handler(NotCalledHandler);
    let addr = spawn_server(server).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = b"GET /ws HTTP/1.1\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
        Sec-WebSocket-Version: 8\r\n\
        \r\n";
    stream.write_all(request).await.unwrap();

    let response_head = read_response_head(&mut stream).await;
    assert!(response_head.starts_with("HTTP/1.1 426 Upgrade Required\r\n"));
    assert!(response_head.contains("Sec-WebSocket-Version: 13\r\n"));
}

#[tokio::test]
async fn non_websocket_path_falls_through_to_default_handler() {
    struct FixedOkHandler;
    impl Handler for FixedOkHandler {
        fn handle(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> fandhe_backend_routes::HandlerFuture {
            Box::pin(std::future::ready(Response::new(200, b"ok".to_vec())))
        }
    }

    let server = Server::new()
        .websocket(WebSocketConfig::default())
        .handler(FixedOkHandler);
    let addr = spawn_server(server).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /other HTTP/1.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.unwrap();
    let response = String::from_utf8(out).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("ok"));
}

/// `Server::websocket` を異なる `path` で複数回呼んだとき、両方のパスへの
/// アップグレードが成立することを確認する回帰テスト（Bugbot 指摘: Duplicate
/// websocket() breaks first path。単一 `websocket_config: Option<T>` だと
/// 2 回目の呼び出しで 1 回目の設定が上書きされ、最初に登録したパスへの
/// アップグレードが 501 になっていた）。
#[tokio::test]
async fn multiple_websocket_registrations_all_remain_reachable() {
    let server = Server::new()
        .websocket(WebSocketConfig::default().with_path("/ws-first"))
        .websocket(WebSocketConfig::default().with_path("/ws-second"))
        .handler(NotCalledHandler);
    let addr = spawn_server(server).await;

    for path in ["/ws-first", "/ws-second"] {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let request = format!(
            "GET {path} HTTP/1.1\r\n\
             Host: example.com\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             \r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();

        let response_head = read_response_head(&mut stream).await;
        assert!(
            response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
            "path {path} が 101 以外を返した: {response_head}"
        );
    }
}

/// 評価順 `RequestGate` → `UpgradeHandler` を維持することを固定する
/// （将来の TenantGate が WS アップグレードも既定拒否できる構造の維持、
/// `crates/core/src/server.rs` 冒頭 doc・REQ-9）。
#[tokio::test]
async fn request_gate_rejection_takes_precedence_over_websocket_upgrade() {
    let server = Server::new()
        .gate(DenyAllGate)
        .websocket(WebSocketConfig::default())
        .handler(NotCalledHandler);
    let addr = spawn_server(server).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();

    // Gate 拒否応答はヘッドのみ確認する（`Connection: close` を送っていない
    // リクエストのため keep-alive のまま接続が残り得る。`read_to_end` で
    // EOF を待つと `READ_TIMEOUT`（30 秒）まで無駄にブロックするため避ける）。
    let response_head = read_response_head(&mut stream).await;
    assert!(response_head.starts_with("HTTP/1.1 403"));
}

/// ユーザー定義 `WsMessageHandler`（Issue #179）を `Server::websocket` 経由で
/// 登録した場合、コア配線（`try_handle_upgrade` → spawn →
/// `fandhe_backend_plugin_websocket::handle_upgrade`）を通ってもカスタム応答になることを
/// 確認する。permit 契約・再 spawn 経路自体は
/// `upgrade_succeeds_and_echoes_text_frame` 等の既存テストで担保済みのため、
/// 本テストはハンドラ差し替えがコア経由で反映される点のみを検証する。
#[tokio::test]
async fn custom_handler_registered_via_server_websocket_is_reachable() {
    use fandhe_backend_plugin_websocket::handler::{
        WsHandlerError, WsMessage, WsMessageHandler, WsOutcome,
    };
    use std::future::Future;
    use std::pin::Pin;

    /// Text を大文字化して返すトイハンドラ（コア配線経由での反映確認用）。
    ///
    /// `WsMessageHandler::on_message` の戻り値型（`futures_util::future::BoxFuture`
    /// の別名）は `Pin<Box<dyn Future<...> + Send + '_>>` そのものであり
    /// （型エイリアス、`futures_core::future::BoxFuture` 定義参照）、本クレート
    /// （`fandhe-backend-core`）の dev-dependencies に `futures-util` を
    /// 追加せずとも構造的に同一の型を直接書けば満たせる。新規依存を増やさない
    /// （pay-for-what-you-use、`.claude/rules/pay-for-what-you-use.md`）。
    struct UppercaseHandler;
    impl WsMessageHandler for UppercaseHandler {
        fn name(&self) -> &'static str {
            "uppercase"
        }
        fn on_message(
            &self,
            msg: WsMessage,
        ) -> Pin<Box<dyn Future<Output = Result<WsOutcome, WsHandlerError>> + Send + '_>> {
            Box::pin(async move {
                let WsMessage::Text(t) = msg else {
                    return Ok(WsOutcome::Reply(vec![]));
                };
                Ok(WsOutcome::Reply(vec![WsMessage::Text(t.to_uppercase())]))
            })
        }
    }

    let server = Server::new()
        .websocket(WebSocketConfig::default().with_handler(UppercaseHandler))
        .handler(NotCalledHandler);
    let addr = spawn_server(server).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();

    let response_head = read_response_head(&mut stream).await;
    assert!(response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    stream.write_all(&masked_text_frame(b"hi")).await.unwrap();
    let (opcode, payload) = read_server_frame(&mut stream).await;
    assert_eq!(
        opcode, 0x1,
        "expected Text opcode from custom handler reply"
    );
    assert_eq!(
        payload, b"HI",
        "custom handler reply must be reflected through core wiring"
    );

    stream.write_all(&masked_close_frame()).await.unwrap();
    let mut trailing = Vec::new();
    let _ = stream.read_to_end(&mut trailing).await;
}

#[tokio::test]
async fn config_built_via_core_reexport_completes_handshake() {
    // イシュー #435: `fandhe_backend_core::plugin_websocket::WebSocketConfig`
    // （プラグインクレートへの直接依存を追加しない再エクスポート経路）
    // 経由で構築した設定でも、直接依存経路（上のテスト）と同一の配線・
    // 応答になることを確認する（`plugin_static_boundary.rs` の
    // `config_built_via_core_reexport_serves_file` と同型パターン、
    // イシュー #421）。
    let config = fandhe_backend_core::plugin_websocket::WebSocketConfig::default();
    let server = Server::new().websocket(config).handler(NotCalledHandler);
    let addr = spawn_server(server).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();

    let response_head = read_response_head(&mut stream).await;
    assert!(response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    stream.write_all(&masked_close_frame()).await.unwrap();
    let mut trailing = Vec::new();
    let _ = stream.read_to_end(&mut trailing).await;
}
