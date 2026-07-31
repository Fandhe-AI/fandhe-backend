//! `Interceptor` 拡張点（イシュー #420）の統合テスト。feature 非依存
//! （外部依存ゼロの純コア機能、`.claude/rules/pay-for-what-you-use.md`）。
//!
//! `crates/core/src/server.rs` の `handle_connection_with_permit` へ組み込んだ
//! 2 フック（`intercept` / `map_response`）の評価順序・後方互換・fail-closed
//! 除外を `tokio::io::duplex` で駆動する `handle_connection` を通して検証する
//! （`plugin_static_boundary.rs` 等の既存パターンを踏襲）。

use fandhe_backend_core::interceptor::Interceptor;
use fandhe_backend_core::{GateOutcome, Handler, RequestGate, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `Handler::handle` が呼ばれたら panic するトイハンドラ。
///
/// `Interceptor::intercept` が `Some` を返した場合は既定 `Handler` を呼ばない
/// 契約（`crates/core/src/server.rs` の `handle_connection_with_permit` を参照）
/// の証跡に使う。
struct NotCalledHandler;
impl Handler for NotCalledHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        panic!("Interceptor::intercept が Some を返したのに既定 Handler が呼ばれた");
    }
}

/// 固定 200 応答を返すだけのトイハンドラ。
struct FixedOkHandler;
impl Handler for FixedOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        Box::pin(std::future::ready(Response::new(200, b"ok".to_vec())))
    }
}

async fn roundtrip(server: &Server, request: &[u8]) -> Vec<u8> {
    let (mut client, server_stream) = tokio::io::duplex(1 << 20);
    client.write_all(request).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    out
}

/// `/old` へのリクエストを 301 でリダイレクトするトイ実装（受け入れ基準 1）。
struct RedirectOld;
impl Interceptor for RedirectOld {
    fn name(&self) -> &'static str {
        "redirect-old"
    }

    fn intercept(&self, head: &RequestHead, _body: &[u8]) -> Option<Response> {
        if head.path() == "/old" {
            Response::redirect(301, "/new").ok()
        } else {
            None
        }
    }
}

#[tokio::test]
async fn intercept_returns_redirect_and_bypasses_default_handler() {
    let server = Server::new()
        .interceptor(RedirectOld)
        .handler(NotCalledHandler);

    let response = roundtrip(&server, b"GET /old HTTP/1.1\r\nConnection: close\r\n\r\n").await;
    let text = String::from_utf8_lossy(&response);

    assert!(text.starts_with("HTTP/1.1 301"), "response: {text}");
    assert!(text.contains("Location: /new"), "response: {text}");
}

#[tokio::test]
async fn intercept_none_falls_through_to_default_handler() {
    let server = Server::new()
        .interceptor(RedirectOld)
        .handler(FixedOkHandler);

    let response = roundtrip(
        &server,
        b"GET /elsewhere HTTP/1.1\r\nConnection: close\r\n\r\n",
    )
    .await;
    let text = String::from_utf8_lossy(&response);

    assert!(text.starts_with("HTTP/1.1 200"), "response: {text}");
    assert!(text.ends_with("ok"), "response: {text}");
}

/// 常に別のリダイレクトを返すトイ実装。複数 `Interceptor` 登録時に
/// 「最初に登録した実装の `Some` が勝つ」ことの検証に使う。
struct AlwaysRedirect;
impl Interceptor for AlwaysRedirect {
    fn name(&self) -> &'static str {
        "always-redirect"
    }

    fn intercept(&self, _head: &RequestHead, _body: &[u8]) -> Option<Response> {
        Response::redirect(302, "/first-wins").ok()
    }
}

#[tokio::test]
async fn multiple_interceptors_first_some_wins_registration_order() {
    let server = Server::new()
        .interceptor(AlwaysRedirect)
        .interceptor(RedirectOld)
        .handler(NotCalledHandler);

    let response = roundtrip(&server, b"GET /old HTTP/1.1\r\nConnection: close\r\n\r\n").await;
    let text = String::from_utf8_lossy(&response);

    // 先に登録した AlwaysRedirect が勝ち、location は "/first-wins"（RedirectOld
    // の "/new" ではない）。
    assert!(text.starts_with("HTTP/1.1 302"), "response: {text}");
    assert!(text.contains("Location: /first-wins"), "response: {text}");
}

/// 未登録時の後方互換確認: `Interceptor` を 1 件も登録しない場合、既存挙動
/// （既定 `Handler` へフォールスルー）が変わらないこと。
#[tokio::test]
async fn no_interceptor_registered_preserves_existing_behavior() {
    let server = Server::new().handler(FixedOkHandler);

    let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
    let text = String::from_utf8_lossy(&response);

    assert!(text.starts_with("HTTP/1.1 200"), "response: {text}");
    assert!(text.ends_with("ok"), "response: {text}");
}

/// Handler 未登録時の既定 404 の body を差し替えるトイ実装（受け入れ基準 2）。
struct Custom404 {
    page: &'static [u8],
}
impl Interceptor for Custom404 {
    fn name(&self) -> &'static str {
        "custom-404"
    }

    fn map_response(&self, _head: &RequestHead, response: Response) -> Response {
        if response.status == 404 {
            Response::new(404, self.page.to_vec())
        } else {
            response
        }
    }
}

#[tokio::test]
async fn map_response_rewrites_default_404_body() {
    let server = Server::new().interceptor(Custom404 {
        page: b"<html>custom not found</html>",
    });

    let response = roundtrip(
        &server,
        b"GET /missing HTTP/1.1\r\nConnection: close\r\n\r\n",
    )
    .await;
    let text = String::from_utf8_lossy(&response);

    assert!(text.starts_with("HTTP/1.1 404"), "response: {text}");
    assert!(
        text.ends_with("<html>custom not found</html>"),
        "response: {text}"
    );
}

/// `map_response` は `intercept` が確定させた応答にも適用されることを確認する。
struct RewriteRedirectStatus;
impl Interceptor for RewriteRedirectStatus {
    fn name(&self) -> &'static str {
        "rewrite-redirect-status"
    }

    fn intercept(&self, head: &RequestHead, _body: &[u8]) -> Option<Response> {
        if head.path() == "/legacy" {
            Response::redirect(301, "/current").ok()
        } else {
            None
        }
    }

    fn map_response(&self, _head: &RequestHead, response: Response) -> Response {
        if response.status == 301 {
            Response::new(308, response.body)
        } else {
            response
        }
    }
}

#[tokio::test]
async fn map_response_applies_to_intercept_response() {
    let server = Server::new()
        .interceptor(RewriteRedirectStatus)
        .handler(NotCalledHandler);

    let response = roundtrip(
        &server,
        b"GET /legacy HTTP/1.1\r\nConnection: close\r\n\r\n",
    )
    .await;
    let text = String::from_utf8_lossy(&response);

    assert!(text.starts_with("HTTP/1.1 308"), "response: {text}");
}

/// `Authorization` ヘッダなしを一律拒否するフェイルクローズなゲート
/// （`crates/core/src/extension.rs` の `RequestGate` doc test と同型）。
struct RequireAuthHeader;
impl RequestGate for RequireAuthHeader {
    fn name(&self) -> &'static str {
        "require-auth-header"
    }

    fn check(&self, head: &RequestHead) -> GateOutcome {
        match head.header("authorization") {
            Some(_) => GateOutcome::Allow,
            None => GateOutcome::Reject {
                status: 401,
                body: Vec::new(),
            },
        }
    }
}

/// 呼ばれたら panic するトイ実装。`RequestGate` 拒否応答が `Interceptor` を
/// 一切通らないこと（fail-closed、受け入れ基準 4）の証跡に使う。
struct PanicIfCalled;
impl Interceptor for PanicIfCalled {
    fn name(&self) -> &'static str {
        "panic-if-called"
    }

    fn intercept(&self, _head: &RequestHead, _body: &[u8]) -> Option<Response> {
        panic!("RequestGate 拒否後に Interceptor::intercept が呼ばれた（fail-closed 違反）");
    }

    fn map_response(&self, _head: &RequestHead, _response: Response) -> Response {
        panic!("RequestGate 拒否応答に Interceptor::map_response が適用された（fail-closed 違反）");
    }
}

#[tokio::test]
async fn request_gate_rejection_bypasses_interceptor_entirely() {
    let server = Server::new()
        .gate(RequireAuthHeader)
        .interceptor(PanicIfCalled)
        .handler(NotCalledHandler);

    let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
    let text = String::from_utf8_lossy(&response);

    assert!(text.starts_with("HTTP/1.1 401"), "response: {text}");
}

/// keep-alive 経路が `Interceptor` 登録の有無で壊れないことを確認する
/// （通常経路と同一に動くことの確認）。
#[tokio::test]
async fn keep_alive_serves_two_requests_with_interceptor_registered() {
    let server = Server::new()
        .interceptor(RedirectOld)
        .handler(FixedOkHandler);

    let (mut client, server_stream) = tokio::io::duplex(1 << 20);
    let first = b"GET / HTTP/1.1\r\n\r\n";
    let second = b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n";
    client.write_all(first).await.unwrap();
    client.write_all(second).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(&server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    let text = String::from_utf8_lossy(&out);

    assert_eq!(text.matches("HTTP/1.1 200").count(), 2, "response: {text}");
}
