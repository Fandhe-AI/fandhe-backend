//! `CsrfGate` の統合テスト（`Server::gate` 登録 + `handle_connection` の実 HTTP 経路、#826）。
//! `crates/plugin-jwt/tests/gate.rs` と同型の `tokio::io::duplex` 駆動。複数ゲートの
//! 登録順（`docs/design/plugin-boundary.md` 5.6.6 節）もスタブの認証ゲートで検証する。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_csrf::CsrfGate;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const APP: &str = "https://app.example.com";

struct Counting(Arc<AtomicUsize>);

impl Handler for Counting {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Response::new(200, b"ok".to_vec())))
    }
}

/// 認証ゲートの代役。`Authorization` が無ければ 401 + チャレンジで拒否し、呼ばれた回数を数える。
struct AuthStub(Arc<AtomicUsize>);

impl RequestGate for AuthStub {
    fn name(&self) -> &'static str {
        "auth-stub"
    }
    fn check(&self, head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        if head.header("Authorization").is_some() {
            return GateOutcome::Allow;
        }
        match Response::new(401, Vec::new()).with_header("WWW-Authenticate", "Bearer") {
            Ok(response) => GateOutcome::Reject { response },
            Err(_) => GateOutcome::reject(401, Vec::new()),
        }
    }
}

fn csrf() -> CsrfGate {
    CsrfGate::builder()
        .allow_origin(APP)
        .exempt_path("/token")
        .build()
        .unwrap()
}

async fn roundtrip(server: &Server, request: &str) -> String {
    let (mut client, server_stream) = tokio::io::duplex(32 * 1024);
    client.write_all(request.as_bytes()).await.unwrap();
    client.shutdown().await.unwrap();
    handle_connection(server, server_stream).await;
    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

fn req(method: &str, path: &str, headers: &[&str]) -> String {
    let mut s = format!("{method} {path} HTTP/1.1\r\nHost: app.example.com\r\n");
    for h in headers {
        s.push_str(h);
        s.push_str("\r\n");
    }
    if method == "POST" {
        s.push_str("Content-Length: 3\r\n");
    }
    s.push_str("Connection: close\r\n\r\n");
    if method == "POST" {
        s.push_str("a=1");
    }
    s
}

fn split(r: &str) -> (Vec<&str>, &str) {
    let (head, body) = r.split_once("\r\n\r\n").expect("head/body separator");
    (head.split("\r\n").collect(), body)
}

fn header_values<'a>(lines: &[&'a str], name: &str) -> Vec<&'a str> {
    lines[1..]
        .iter()
        .filter_map(|l| l.split_once(": "))
        .filter(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
        .collect()
}

fn assert_bare_403(r: &str) {
    let (lines, body) = split(r);
    assert_eq!(lines[0], "HTTP/1.1 403 Forbidden", "{r}");
    assert_eq!(header_values(&lines, "Content-Length"), vec!["0"], "{r}");
    assert!(header_values(&lines, "Content-Type").is_empty(), "{r}");
    assert!(header_values(&lines, "WWW-Authenticate").is_empty(), "{r}");
    assert_eq!(body, "", "{r}");
}

fn assert_200(r: &str) {
    let (lines, body) = split(r);
    assert_eq!(lines[0], "HTTP/1.1 200 OK", "{r}");
    assert_eq!(body, "ok", "{r}");
}

fn server_csrf_only() -> (Server, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let server = Server::new()
        .gate(csrf())
        .handler(Counting(Arc::clone(&hits)));
    (server, hits)
}

const CROSS: [&str; 1] = ["Sec-Fetch-Site: cross-site"];

#[tokio::test]
async fn same_origin_post_reaches_handler_200() {
    let (server, hits) = server_csrf_only();
    let r = roundtrip(
        &server,
        &req("POST", "/f", &["Sec-Fetch-Site: same-origin"]),
    )
    .await;
    assert_200(&r);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn allowed_origin_post_reaches_handler_200() {
    let (server, hits) = server_csrf_only();
    let r = roundtrip(&server, &req("POST", "/f", &[&format!("Origin: {APP}")])).await;
    assert_200(&r);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cross_site_post_is_bare_403_and_handler_not_called() {
    let (server, hits) = server_csrf_only();
    let r = roundtrip(&server, &req("POST", "/f", &CROSS)).await;
    assert_bare_403(&r);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn origin_null_and_missing_headers_are_403() {
    let (server, hits) = server_csrf_only();
    for headers in [&["Origin: null"][..], &[][..]] {
        let r = roundtrip(&server, &req("POST", "/f", headers)).await;
        assert_bare_403(&r);
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn safe_method_get_passes_200() {
    let (server, hits) = server_csrf_only();
    let r = roundtrip(&server, &req("GET", "/f", &CROSS)).await;
    assert_200(&r);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exempt_path_passes_without_headers_200() {
    let (server, hits) = server_csrf_only();
    let r = roundtrip(&server, &req("POST", "/token", &[])).await;
    assert_200(&r);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

fn server_ordered(csrf_first: bool) -> (Server, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let auth_calls = Arc::new(AtomicUsize::new(0));
    let hits = Arc::new(AtomicUsize::new(0));
    let s = Server::new();
    let s = if csrf_first {
        s.gate(csrf()).gate(AuthStub(Arc::clone(&auth_calls)))
    } else {
        s.gate(AuthStub(Arc::clone(&auth_calls))).gate(csrf())
    };
    (s.handler(Counting(Arc::clone(&hits))), auth_calls, hits)
}

#[tokio::test]
async fn csrf_first_short_circuits_later_gate() {
    let (server, auth_calls, hits) = server_ordered(true);
    let r = roundtrip(&server, &req("POST", "/f", &CROSS)).await;
    assert_bare_403(&r);
    assert_eq!(auth_calls.load(Ordering::SeqCst), 0);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn csrf_first_then_auth_gate_evaluated_when_origin_ok() {
    let (server, auth_calls, hits) = server_ordered(true);
    let same = "Sec-Fetch-Site: same-origin";
    let r = roundtrip(&server, &req("POST", "/f", &[same])).await;
    let (lines, _) = split(&r);
    assert_eq!(lines[0], "HTTP/1.1 401 Unauthorized", "{r}");
    assert_eq!(auth_calls.load(Ordering::SeqCst), 1);
    let r = roundtrip(
        &server,
        &req("POST", "/f", &[same, "Authorization: Bearer t"]),
    )
    .await;
    assert_200(&r);
    assert_eq!(auth_calls.load(Ordering::SeqCst), 2);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exempt_path_still_runs_later_auth_gate() {
    let (server, auth_calls, hits) = server_ordered(true);
    let r = roundtrip(&server, &req("POST", "/token", &[])).await;
    let (lines, _) = split(&r);
    assert_eq!(lines[0], "HTTP/1.1 401 Unauthorized", "{r}");
    assert_eq!(auth_calls.load(Ordering::SeqCst), 1);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn reverse_order_exposes_auth_challenge() {
    let (server, auth_calls, hits) = server_ordered(false);
    let r = roundtrip(&server, &req("POST", "/f", &CROSS)).await;
    let (lines, _) = split(&r);
    assert_eq!(lines[0], "HTTP/1.1 401 Unauthorized", "{r}");
    assert_eq!(
        header_values(&lines, "WWW-Authenticate"),
        vec!["Bearer"],
        "{r}"
    );
    assert_eq!(auth_calls.load(Ordering::SeqCst), 1);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn decision_is_order_independent() {
    let cases: [Vec<&str>; 6] = [
        vec![],
        vec!["Sec-Fetch-Site: cross-site"],
        vec!["Sec-Fetch-Site: same-origin"],
        vec!["Sec-Fetch-Site: same-origin", "Authorization: Bearer t"],
        vec!["Sec-Fetch-Site: cross-site", "Authorization: Bearer t"],
        vec!["Authorization: Bearer t"],
    ];
    for headers in &cases {
        let mut passed = Vec::new();
        for csrf_first in [true, false] {
            let (server, _, hits) = server_ordered(csrf_first);
            roundtrip(&server, &req("POST", "/f", headers)).await;
            passed.push(hits.load(Ordering::SeqCst) == 1);
        }
        assert_eq!(passed[0], passed[1], "{headers:?}");
    }
}
