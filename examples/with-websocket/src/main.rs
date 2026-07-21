//! WebSocket プラグイン（`websocket` feature）の `WebSocketConfig::with_handler`
//! によるユーザー定義メッセージハンドラ配線だけを見せる最小サンプル。
//!
//! `crates/core/examples/ws_echo.rs` は負荷計測専用で既定の `EchoHandler` の
//! ままだが、本サンプルは Issue #179 で追加された
//! `fandhe_backend_plugin_websocket::handler::WsMessageHandler` の利用者向け
//! 配線例を示す（Next.js の `examples/` 方式、`examples/README.md` 参照）。
//!
//! [`PingPongEchoHandler`] は 3 通りの応答を返す:
//! - `"ping"` → `"pong"`（固定の独自応答）
//! - `"bye"` → サーバ起点の Close（[`WsOutcome::Close`]）
//! - それ以外の Text/Binary → そのままエコー
//!
//! HTTP 側は `GET /` に接続方法を案内する最小レスポンスのみを持たせ、
//! `Router` 配線と WS 配線が同一 `Server` に共存できることを示す。
//! `WebSocketConfig` のサイズ・アイドルタイムアウトは既定値（DoS 安全側、
//! 1 MiB / 256 KiB / 60 秒）から変更しない。
//!
//! # 起動方法
//!
//! ```text
//! $ cd examples/with-websocket
//! $ cargo run
//! ```
//!
//! 既定で `127.0.0.1:3000` に bind する（`PORT` 環境変数で上書き可能）。
//!
//! # 動作確認手順
//!
//! ```text
//! # 案内メッセージの確認
//! $ curl -s http://127.0.0.1:3000/
//!
//! # WebSocket 接続（websocat 使用時。既定パス /ws）
//! $ websocat ws://127.0.0.1:3000/ws
//! ping     # -> pong
//! hello    # -> hello（エコー）
//! bye      # -> サーバから Close
//! ```
//!
//! websocat が使えない環境では `cargo test` の E2E テストが
//! ハンドシェイク・メッセージ往復を自動検証する。

use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_websocket::WebSocketConfig;
use fandhe_backend_plugin_websocket::handler::{
    WsHandlerError, WsMessage, WsMessageHandler, WsOutcome,
};
use fandhe_backend_routes::Router;
use futures_util::future::BoxFuture;

/// Text `"ping"` には `"pong"` を返し、Text `"bye"` にはサーバ起点 Close を
/// 返す。それ以外の Text/Binary はエコーで返す（`WsMessageHandler` 実装の
/// 最小例、`WebSocketConfig::with_handler` から登録される）。
struct PingPongEchoHandler;

impl WsMessageHandler for PingPongEchoHandler {
    fn name(&self) -> &'static str {
        "ping-pong-echo"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move {
            let outcome = match msg {
                WsMessage::Text(t) if t == "ping" => {
                    WsOutcome::Reply(vec![WsMessage::Text("pong".to_string())])
                }
                WsMessage::Text(t) if t == "bye" => WsOutcome::Close,
                other => WsOutcome::Reply(vec![other]),
            };
            Ok(outcome)
        })
    }
}

/// `main` とテストの両方から共有する [`Router`] を組み立てる
/// （`examples/with-cors/src/main.rs` の `build_router` と同一パターン）。
/// WebSocket 配線自体は `Router` の責務範囲外のため `main` 側で行う。
fn build_router() -> Router {
    Router::new().route("GET", "/", |_head, _body| {
        Response::new(
            200,
            b"fandhe-backend-example-with-websocket: connect to /ws\n".to_vec(),
        )
        .with_content_type("text/plain")
    })
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> std::io::Result<()> {
    let router = build_router();
    let ws_config = WebSocketConfig::default().with_handler(PingPongEchoHandler);
    let server = Server::new().handler(router).websocket(ws_config);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("127.0.0.1:{port}");
    let bound = server.bind(&addr).await?;
    println!("fandhe-backend-example-with-websocket listening on {addr}");
    bound
        .run_until(async {
            // 登録失敗を握りつぶすと future が即完了し bind 直後にサーバが
            // 終了してしまうため、シグナルハンドラを登録できない環境では
            // 起動継続せず明示的に panic させる（graceful-shutdown ガイド・
            // examples/with-cors と同方針）
            tokio::signal::ctrl_c()
                .await
                .expect("Ctrl-C シグナルハンドラの登録に失敗した");
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::protocol::Role;

    /// ケース 1（ハンドラ単体）: `"ping"` → `"pong"` の独自応答を返す。
    #[tokio::test]
    async fn ping_replies_with_pong() {
        let outcome = PingPongEchoHandler
            .on_message(WsMessage::Text("ping".to_string()))
            .await
            .unwrap();
        assert_eq!(
            outcome,
            WsOutcome::Reply(vec![WsMessage::Text("pong".to_string())])
        );
    }

    /// ケース 2（ハンドラ単体）: `"bye"` はサーバ起点 Close を返す。
    #[tokio::test]
    async fn bye_closes_session() {
        let outcome = PingPongEchoHandler
            .on_message(WsMessage::Text("bye".to_string()))
            .await
            .unwrap();
        assert_eq!(outcome, WsOutcome::Close);
    }

    /// ケース 3（ハンドラ単体）: `"ping"`・`"bye"` 以外の Text はエコーする。
    #[tokio::test]
    async fn other_text_is_echoed() {
        let outcome = PingPongEchoHandler
            .on_message(WsMessage::Text("hello".to_string()))
            .await
            .unwrap();
        assert_eq!(
            outcome,
            WsOutcome::Reply(vec![WsMessage::Text("hello".to_string())])
        );
    }

    /// ケース 4（ハンドラ単体）: Binary メッセージはそのままエコーする。
    #[tokio::test]
    async fn binary_is_echoed() {
        let outcome = PingPongEchoHandler
            .on_message(WsMessage::Binary(vec![1, 2, 3]))
            .await
            .unwrap();
        assert_eq!(
            outcome,
            WsOutcome::Reply(vec![WsMessage::Binary(vec![1, 2, 3])])
        );
    }

    /// RFC 6455 固定ハンドシェイクリクエストの生バイト列
    /// （`crates/plugin-websocket/tests/handler_e2e.rs` と同一の
    /// `Sec-WebSocket-Key` を使い、実装間で検証手順を揃える）。
    fn handshake_request_bytes(path: &str) -> Vec<u8> {
        format!(
            "GET {path} HTTP/1.1\r\n\
             Host: example.com\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             \r\n"
        )
        .into_bytes()
    }

    async fn read_http_response_head(stream: &mut TcpStream) -> String {
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

    /// ケース 5（E2E）: 実 TCP 上でハンドシェイク（101 応答）とメッセージ
    /// 往復（ping→pong / hello→エコー / bye→Close）を検証する
    /// （`crates/plugin-websocket/tests/handler_e2e.rs` の実証済みパターンを
    /// 実ソケットへ適用）。
    #[tokio::test]
    async fn handshake_and_message_roundtrip_over_real_tcp() {
        let router = build_router();
        let ws_config = WebSocketConfig::default().with_handler(PingPongEchoHandler);
        let server = Server::new().handler(router).websocket(ws_config);
        let bound = server.bind("127.0.0.1:0").await.expect("bind");
        let addr = bound.local_addr().expect("local_addr");
        let server_task = tokio::spawn(bound.run());

        // ハンドシェイク検証: 生の GET /ws リクエストを送り、101 応答を確認する。
        let mut raw = TcpStream::connect(addr).await.expect("connect");
        raw.write_all(&handshake_request_bytes("/ws"))
            .await
            .expect("write handshake request");
        let response = read_http_response_head(&mut raw).await;
        assert!(
            response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
            "unexpected handshake response: {response}"
        );

        // メッセージ往復検証: クライアント側フレーミングを確立し、
        // ping→pong / hello→エコー / bye→サーバ起点 Close を確認する。
        let mut client = WebSocketStream::from_raw_socket(raw, Role::Client, None).await;

        client
            .send(Message::Text("ping".into()))
            .await
            .expect("send ping");
        let reply = client.next().await.expect("pong reply").expect("no error");
        assert_eq!(reply, Message::Text("pong".into()));

        client
            .send(Message::Text("hello".into()))
            .await
            .expect("send hello");
        let reply = client.next().await.expect("echo reply").expect("no error");
        assert_eq!(reply, Message::Text("hello".into()));

        client
            .send(Message::Text("bye".into()))
            .await
            .expect("send bye");
        let closed = client.next().await;
        match closed {
            Some(Ok(Message::Close(_))) | None => {}
            other => panic!("expected close frame or stream end, got {other:?}"),
        }

        server_task.abort();
    }
}
