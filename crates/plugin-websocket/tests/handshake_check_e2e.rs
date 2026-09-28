//! ハンドシェイクの受理判定フック（`WebSocketConfig::with_handshake_check`、
//! イシュー #716）の統合テスト。
//!
//! `peer_addr_e2e.rs` / `on_close_e2e.rs` と同様に `tokio::io::duplex` +
//! `tokio-tungstenite` クライアントで実ハンドシェイクを駆動する。フックが
//! 拒否したときに upgrade せず指定レスポンスを返すこと（受け入れ基準 1）、
//! パスパラメータ・ヘッダ・接続元アドレスを参照できること（受け入れ基準
//! 2）を実証する。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_websocket::handler::{
    WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome,
};
use fandhe_backend_plugin_websocket::{
    WebSocketConfig, WsHandshakeContext, handle_upgrade_with_peer_addr,
};
use futures_util::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;

fn parse_head(bytes: &[u8]) -> RequestHead {
    match parse_request_head(bytes).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    }
}

fn handshake_request_bytes() -> &'static [u8] {
    b"GET /ws HTTP/1.1\r\n\
      Host: example.com\r\n\
      Origin: https://allowed.example\r\n\
      Upgrade: websocket\r\n\
      Connection: Upgrade\r\n\
      Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
      Sec-WebSocket-Version: 13\r\n\
      \r\n"
}

/// クライアント側ストリームが EOF に達するまで全バイトを読み切る
/// （拒否応答は `Connection: close` で送出されるため EOF まで読める）。
async fn read_to_eof<S: AsyncRead + Unpin>(stream: &mut S) -> Vec<u8> {
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

async fn read_http_response_head<S: AsyncRead + Unpin>(stream: &mut S) -> String {
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

/// `on_open`/`on_close` の呼び出し回数を記録するハンドラ（フックが拒否した
/// 接続ではいずれも呼ばれないことを検証するため）。
#[derive(Default)]
struct CallCounter {
    open_calls: AtomicUsize,
}

impl WsMessageHandler for CallCounter {
    fn name(&self) -> &'static str {
        "call-counter"
    }

    fn on_open(&self, _ctx: WsOpenContext) {
        self.open_calls.fetch_add(1, Ordering::SeqCst);
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

/// 受け入れ基準 1: フックが拒否したときは指定レスポンスを返し、upgrade しない。
#[tokio::test]
async fn rejecting_hook_returns_specified_response_without_upgrading() {
    let head = parse_head(handshake_request_bytes());
    let counter = Arc::new(CallCounter::default());
    let counter_for_handler = Arc::clone(&counter);
    let config = WebSocketConfig::default()
        .with_handshake_check(|_ctx: &WsHandshakeContext<'_>| {
            Err(Response::new(404, b"no such page".to_vec())
                .with_header("X-Reject-Reason", "not-found")
                .unwrap())
        })
        .with_handler(CallCounterHandler(counter_for_handler));

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            None,
        )
        .await
    });

    let response = read_to_eof(&mut client_side).await;
    let text = String::from_utf8(response).unwrap();
    // ステータス行・ヘッダ（`Connection: close`・カスタムヘッダ・
    // `Content-Length`）・ボディをすべて検証する（AGENTS.md
    // 「アサーション網羅性」規約、イシュー #716 P2 レビュー指摘）。
    assert!(text.starts_with("HTTP/1.1 404"));
    assert!(text.contains("Connection: close"));
    assert!(text.contains("X-Reject-Reason: not-found"));
    assert!(text.contains("Content-Length: 12\r\n"));
    assert!(text.ends_with("no such page"));
    assert!(!text.contains("101 Switching Protocols"));

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok(), "rejection must be a clean Ok(()), not Err");
    assert_eq!(
        counter.open_calls.load(Ordering::SeqCst),
        0,
        "on_open must not be called when the handshake check rejects"
    );
}

/// `WsMessageHandler` は `Send + Sync + 'static` の `Arc` 経由で共有する
/// ラッパー（`CallCounter` 自体を複数テストで clone せず使い回すため）。
struct CallCounterHandler(Arc<CallCounter>);

impl WsMessageHandler for CallCounterHandler {
    fn name(&self) -> &'static str {
        self.0.name()
    }

    fn on_open(&self, ctx: WsOpenContext) {
        self.0.on_open(ctx);
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        self.0.on_message(msg)
    }
}

/// 受け入れ基準 1（1xx 正規化）: フックが 101 を返しても、クライアントには
/// upgrade 成功と誤認させる応答を送出せず 400 になる。
#[tokio::test]
async fn rejecting_hook_returning_101_is_normalized_to_400() {
    let head = parse_head(handshake_request_bytes());
    let config = WebSocketConfig::default()
        .with_handshake_check(|_ctx: &WsHandshakeContext<'_>| Err(Response::empty(101)));

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            None,
        )
        .await
    });

    let text = String::from_utf8(read_to_eof(&mut client_side).await).unwrap();
    // ステータス行だけでなく、拒否後の接続を再利用しない契約
    // （`Connection: close`）と `normalize_rejection` が 400 正規化時に
    // `Response::empty(400)` を使う（空ボディ）ことも検証する
    // （AGENTS.md「アサーション網羅性」規約、イシュー #716 P2 レビュー指摘）。
    assert!(text.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    assert!(!text.contains("101 Switching Protocols"));
    assert!(text.contains("Connection: close\r\n"));
    assert!(text.contains("Content-Length: 0\r\n"));
    let head_end = text
        .find("\r\n\r\n")
        .expect("response must have header terminator")
        + 4;
    assert!(
        text[head_end..].is_empty(),
        "400 正規化応答はボディを送出しないはず: {:?}",
        &text[head_end..]
    );

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok());
}

/// 受け入れ基準 2: フックからパスパラメータを参照できる。
#[tokio::test]
async fn hook_observes_path_parameter_and_rejects_unknown_id() {
    let known = b"GET /devtools/page/KNOWN HTTP/1.1\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
        Sec-WebSocket-Version: 13\r\n\
        \r\n";
    let head = parse_head(known);
    let config = WebSocketConfig::default()
        .with_path_pattern("/devtools/page/{id}")
        .unwrap()
        .with_handshake_check(|ctx: &WsHandshakeContext<'_>| match ctx.param("id") {
            Some("KNOWN") => Ok(()),
            _ => Err(Response::empty(404)),
        });

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            None,
        )
        .await
    });

    let response_head = read_http_response_head(&mut client_side).await;
    // ステータス行だけでなく、RFC 6455 4.2.2 が要求する 101 応答の必須ヘッダ
    // （`Upgrade`/`Connection`/`Sec-WebSocket-Accept`。値は固定 nonce
    // `dGhlIHNhbXBsZSBub25jZQ==` に対する既知ベクタ、`handshake_e2e.rs` /
    // `conn_request_info_e2e.rs` の `EXPECTED_101_RESPONSE` と同一値）も
    // 検証する（AGENTS.md「アサーション網羅性」規約、イシュー #716 P2 レビュー
    // 指摘。フックが `Ok(())` を返した場合に通常どおりの 101 応答が組み立て
    // られることの証跡）。
    assert!(response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
    assert!(response_head.contains("Upgrade: websocket\r\n"));
    assert!(response_head.contains("Connection: Upgrade\r\n"));
    assert!(response_head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client.close(None).await.expect("client close should send");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok());
}

#[tokio::test]
async fn hook_rejects_unknown_path_parameter() {
    let unknown = b"GET /devtools/page/UNKNOWN HTTP/1.1\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
        Sec-WebSocket-Version: 13\r\n\
        \r\n";
    let head = parse_head(unknown);
    let config = WebSocketConfig::default()
        .with_path_pattern("/devtools/page/{id}")
        .unwrap()
        .with_handshake_check(|ctx: &WsHandshakeContext<'_>| match ctx.param("id") {
            Some("KNOWN") => Ok(()),
            _ => Err(Response::empty(404)),
        });

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            None,
        )
        .await
    });

    let text = String::from_utf8(read_to_eof(&mut client_side).await).unwrap();
    // ステータス行だけでなく `Connection: close`・空ボディ
    // （`Response::empty(404)` 由来）も検証する（AGENTS.md
    // 「アサーション網羅性」規約、イシュー #716 P2 レビュー指摘）。
    assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(text.contains("Connection: close\r\n"));
    assert!(text.contains("Content-Length: 0\r\n"));
    let head_end = text
        .find("\r\n\r\n")
        .expect("response must have header terminator")
        + 4;
    assert!(
        text[head_end..].is_empty(),
        "拒否応答はボディを送出しないはず: {:?}",
        &text[head_end..]
    );

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok());
}

/// 受け入れ基準 2: フックからリクエストヘッダ（`Origin`）を参照できる。
#[tokio::test]
async fn hook_observes_origin_header_and_rejects_disallowed_origin() {
    let disallowed = b"GET /ws HTTP/1.1\r\n\
        Origin: https://evil.example\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
        Sec-WebSocket-Version: 13\r\n\
        \r\n";
    let head = parse_head(disallowed);
    let config = WebSocketConfig::default().with_handshake_check(|ctx: &WsHandshakeContext<'_>| {
        match ctx.header("origin") {
            Some("https://allowed.example") => Ok(()),
            _ => Err(Response::empty(403)),
        }
    });

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            None,
        )
        .await
    });

    let text = String::from_utf8(read_to_eof(&mut client_side).await).unwrap();
    // ステータス行だけでなく `Connection: close`・空ボディ
    // （`Response::empty(403)` 由来）も検証する（AGENTS.md
    // 「アサーション網羅性」規約、イシュー #716 P2 レビュー指摘）。
    assert!(text.starts_with("HTTP/1.1 403 Forbidden\r\n"));
    assert!(text.contains("Connection: close\r\n"));
    assert!(text.contains("Content-Length: 0\r\n"));
    let head_end = text
        .find("\r\n\r\n")
        .expect("response must have header terminator")
        + 4;
    assert!(
        text[head_end..].is_empty(),
        "拒否応答はボディを送出しないはず: {:?}",
        &text[head_end..]
    );

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok());
}

/// 受け入れ基準 2: フックから接続元アドレスを参照できる（`Some`/`None` の
/// 両方を観測できること）。
#[tokio::test]
async fn hook_observes_injected_peer_addr() {
    let head = parse_head(handshake_request_bytes());
    let observed: Arc<Mutex<Option<Option<SocketAddr>>>> = Arc::new(Mutex::new(None));
    let observed_for_hook = Arc::clone(&observed);
    let injected: SocketAddr = "203.0.113.9:12345".parse().unwrap();
    let config =
        WebSocketConfig::default().with_handshake_check(move |ctx: &WsHandshakeContext<'_>| {
            *observed_for_hook.lock().unwrap() = Some(ctx.peer_addr());
            Ok(())
        });

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            Some(injected),
        )
        .await
    });

    let response_head = read_http_response_head(&mut client_side).await;
    // ステータス行だけでなく、フックが `Ok(())` を返した際に通常どおりの
    // 101 応答（RFC 6455 4.2.2 必須ヘッダ）が組み立てられることも検証する
    // （AGENTS.md「アサーション網羅性」規約、イシュー #716 P2 レビュー指摘。
    // 既知ベクタは `hook_observes_path_parameter_and_rejects_unknown_id` と
    // 同一）。
    assert!(response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
    assert!(response_head.contains("Upgrade: websocket\r\n"));
    assert!(response_head.contains("Connection: Upgrade\r\n"));
    assert!(response_head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client.close(None).await.expect("client close should send");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok());

    assert_eq!(*observed.lock().unwrap(), Some(Some(injected)));
}

/// RFC 違反の要求は受理判定フックより先に 400/426 で落ち、フックは呼ばれない。
#[tokio::test]
async fn rfc_violation_is_rejected_before_handshake_check_runs() {
    let bad_version = b"GET /ws HTTP/1.1\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
        Sec-WebSocket-Version: 8\r\n\
        \r\n";
    let head = parse_head(bad_version);
    let check_calls = Arc::new(AtomicUsize::new(0));
    let check_calls_for_hook = Arc::clone(&check_calls);
    let config =
        WebSocketConfig::default().with_handshake_check(move |_ctx: &WsHandshakeContext<'_>| {
            check_calls_for_hook.fetch_add(1, Ordering::SeqCst);
            Err(Response::empty(403))
        });

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            None,
        )
        .await
    });

    let text = String::from_utf8(read_to_eof(&mut client_side).await).unwrap();
    // ステータス行だけでなく、`handshake::serialize_426` が固定で組み立てる
    // ヘッダ（`Sec-WebSocket-Version: 13`・`Connection: close`・
    // `Content-Length: 0`）と空ボディも検証する（AGENTS.md
    // 「アサーション網羅性」規約、イシュー #716 P2 レビュー指摘）。
    assert!(text.starts_with("HTTP/1.1 426 Upgrade Required\r\n"));
    assert!(text.contains("Sec-WebSocket-Version: 13\r\n"));
    assert!(text.contains("Connection: close\r\n"));
    assert!(text.contains("Content-Length: 0\r\n"));
    let head_end = text
        .find("\r\n\r\n")
        .expect("response must have header terminator")
        + 4;
    assert!(
        text[head_end..].is_empty(),
        "426 応答はボディを送出しないはず: {:?}",
        &text[head_end..]
    );

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_err(), "RFC violation must return Err (426)");
    assert_eq!(
        check_calls.load(Ordering::SeqCst),
        0,
        "handshake check must not run before RFC 6455 validation passes"
    );
}

/// キャンセル済みの状態ではフックは呼ばれず、何も書き込まれずに `Ok(())`
/// になる（既存のキャンセル契約を維持）。
#[tokio::test]
async fn already_cancelled_skips_handshake_check() {
    let head = parse_head(handshake_request_bytes());
    let check_calls = Arc::new(AtomicUsize::new(0));
    let check_calls_for_hook = Arc::clone(&check_calls);
    let config =
        WebSocketConfig::default().with_handshake_check(move |_ctx: &WsHandshakeContext<'_>| {
            check_calls_for_hook.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::ready(()),
            None,
        )
        .await
    });

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok());
    assert_eq!(check_calls.load(Ordering::SeqCst), 0);

    // 何も書き込まれずに接続が閉じることを確認する（EOF まで読んで空である
    // こと）。
    let bytes = read_to_eof(&mut client_side).await;
    assert!(bytes.is_empty());
}

/// フック未登録（後方互換）: 既存 e2e が無変更で通ること自体が証跡だが、
/// 本クレートからも最小構成で確認する。
#[tokio::test]
async fn no_handshake_check_registered_upgrades_as_before() {
    let head = parse_head(handshake_request_bytes());
    let config = WebSocketConfig::default();
    assert!(!config.has_handshake_check());

    let (server_side, mut client_side) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handle_upgrade_with_peer_addr(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
            None,
        )
        .await
    });

    let response_head = read_http_response_head(&mut client_side).await;
    // ステータス行だけでなく、フック未登録時も従来どおりの 101 応答
    // （RFC 6455 4.2.2 必須ヘッダ）が組み立てられることも検証する
    // （AGENTS.md「アサーション網羅性」規約、イシュー #716 P2 レビュー指摘）。
    assert!(response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
    assert!(response_head.contains("Upgrade: websocket\r\n"));
    assert!(response_head.contains("Connection: Upgrade\r\n"));
    assert!(response_head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));

    let mut client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    client.close(None).await.expect("client close should send");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish before test timeout")
        .expect("task should not panic");
    assert!(result.is_ok());
}
