//! `RequestGate` が入れた検証結果を、同じリクエストのハンドラだけが読めること
//! （イシュー #924、`docs/design/gate-request-extensions.md`）の統合テスト。
//!
//! `Server::gate` + `Server::handler(Router)` の実経路（実 TCP）で、
//! - ゲートが入れた値を同じリクエストのハンドラが読める
//! - keep-alive の後続リクエスト・別接続・ゲート未登録・Reject 後には漏れない
//! - 可視性の契約（`Middleware::on_request` は空、`on_response_with_status`・
//!   `Interceptor` は値が見える）
//!
//! を固定する。

use fandhe_backend_core::extension::{
    GateContext, GateOutcome, Middleware, RequestExtensions, RequestGate,
};
use fandhe_backend_core::{Interceptor, Server};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::Router;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// ゲートが検証して得た値（テスト用の「検証済み主体」）。
#[derive(Debug)]
struct Principal(String);

/// `X-Test-User` があれば `Principal` を入れて通し、なければ値なしで通すゲート。
struct InsertingGate;

impl RequestGate for InsertingGate {
    fn name(&self) -> &'static str {
        "inserting"
    }

    fn check(
        &self,
        head: &RequestHead,
        _ctx: &GateContext,
        extensions: &mut RequestExtensions,
    ) -> GateOutcome {
        if let Some(user) = head.header("x-test-user")
            && extensions.insert(Principal(user.to_string())).is_err()
        {
            return GateOutcome::reject(500, Vec::new());
        }
        GateOutcome::Allow
    }
}

/// `X-Deny` があれば拒否するゲート（先行ゲートが値を入れた後の Reject 検証用）。
struct DenyHeaderGate;

impl RequestGate for DenyHeaderGate {
    fn name(&self) -> &'static str {
        "deny-header"
    }

    fn check(
        &self,
        head: &RequestHead,
        _ctx: &GateContext,
        _extensions: &mut RequestExtensions,
    ) -> GateOutcome {
        if head.header("x-deny").is_some() {
            GateOutcome::reject(403, b"denied".to_vec())
        } else {
            GateOutcome::Allow
        }
    }
}

/// `Principal` を無条件に入れるゲート（重複 insert の検証用）。
struct AlwaysInsertGate;

impl RequestGate for AlwaysInsertGate {
    fn name(&self) -> &'static str {
        "always-insert"
    }

    fn check(
        &self,
        _head: &RequestHead,
        _ctx: &GateContext,
        extensions: &mut RequestExtensions,
    ) -> GateOutcome {
        match extensions.insert(Principal("first".to_string())) {
            Ok(()) => GateOutcome::Allow,
            Err(_) => GateOutcome::reject(500, Vec::new()),
        }
    }
}

/// 値を読むハンドラ。本文は `Principal` の値、なければ `none`。呼び出し回数を数える。
fn router(calls: Arc<Mutex<usize>>) -> Router {
    Router::new().route("GET", "/who", move |head, _body| {
        *calls.lock().unwrap() += 1;
        let body = head
            .extensions()
            .get::<Principal>()
            .map_or_else(|| "none".to_string(), |p| p.0.clone());
        Response::new(200, body.into_bytes())
    })
}

async fn spawn_server(server: Server) -> std::net::SocketAddr {
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = bound.run().await;
    });
    addr
}

fn request(user: Option<&str>, extra: &str, close: bool) -> Vec<u8> {
    let mut s = String::from("GET /who HTTP/1.1\r\nHost: localhost\r\n");
    if let Some(u) = user {
        s.push_str(&format!("X-Test-User: {u}\r\n"));
    }
    s.push_str(extra);
    if close {
        s.push_str("Connection: close\r\n");
    }
    s.push_str("\r\n");
    s.into_bytes()
}

/// 1 応答を読み、(ステータス行, 本文) を返す。`Content-Length` ちょうどだけ読む。
async fn read_response(stream: &mut TcpStream) -> (String, String) {
    let mut acc = Vec::new();
    let mut byte = [0u8; 1];
    while !acc.ends_with(b"\r\n\r\n") {
        let n = timeout(Duration::from_secs(2), stream.read(&mut byte))
            .await
            .expect("応答ヘッドがタイムアウトしないこと")
            .expect("read");
        assert!(n == 1, "応答ヘッド途中で接続が閉じた");
        acc.push(byte[0]);
    }
    let head = String::from_utf8(acc).unwrap();
    let len: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .map_or(0, |v| v.trim().parse().unwrap());
    let mut body = vec![0u8; len];
    timeout(Duration::from_secs(2), stream.read_exact(&mut body))
        .await
        .expect("応答本文がタイムアウトしないこと")
        .expect("read body");
    let status = head.lines().next().unwrap().to_string();
    (status, String::from_utf8(body).unwrap())
}

/// (a) ゲートが入れた値を、同じリクエストのハンドラが読める（実 TCP）。
#[tokio::test]
async fn handler_reads_value_inserted_by_gate_for_same_request() {
    let calls = Arc::new(Mutex::new(0));
    let addr = spawn_server(Server::new().handler(router(calls)).gate(InsertingGate)).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(&request(Some("alice"), "", true))
        .await
        .unwrap();
    let (status, body) = read_response(&mut s).await;
    assert!(status.starts_with("HTTP/1.1 200"));
    assert_eq!(body, "alice");
}

/// (b) 同一 keep-alive 接続の後続リクエストへ値が漏れない。
#[tokio::test]
async fn value_does_not_leak_to_next_request_on_same_connection() {
    let calls = Arc::new(Mutex::new(0));
    let addr = spawn_server(Server::new().handler(router(calls)).gate(InsertingGate)).await;
    let mut s = TcpStream::connect(addr).await.unwrap();

    s.write_all(&request(Some("alice"), "", false))
        .await
        .unwrap();
    assert_eq!(read_response(&mut s).await.1, "alice");

    // 2 本目: ゲートは通すが何も入れない。ハンドラは none を見る。
    s.write_all(&request(None, "", false)).await.unwrap();
    assert_eq!(read_response(&mut s).await.1, "none");

    // 3 本目: 別の値。1 本目の値ではなく自分の値が見える。
    s.write_all(&request(Some("bob"), "", true)).await.unwrap();
    assert_eq!(read_response(&mut s).await.1, "bob");
}

/// (c) 並行する別接続の値が交差しない。
#[tokio::test]
async fn concurrent_connections_do_not_cross_values() {
    let calls = Arc::new(Mutex::new(0));
    let addr = spawn_server(Server::new().handler(router(calls)).gate(InsertingGate)).await;
    let mut tasks = Vec::new();
    for i in 0..16 {
        tasks.push(tokio::spawn(async move {
            let user = format!("user{i}");
            let mut s = TcpStream::connect(addr).await.unwrap();
            for _ in 0..5 {
                s.write_all(&request(Some(&user), "", false)).await.unwrap();
                assert_eq!(read_response(&mut s).await.1, user);
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
}

/// (d) ゲート未登録の経路ではハンドラは値を読めない。
#[tokio::test]
async fn no_gate_means_no_value() {
    let calls = Arc::new(Mutex::new(0));
    let addr = spawn_server(Server::new().handler(router(calls))).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    // ゲートがなければ X-Test-User を送っても値にならない（クライアント入力は器に入らない）。
    s.write_all(&request(Some("mallory"), "", true))
        .await
        .unwrap();
    assert_eq!(read_response(&mut s).await.1, "none");
}

/// (e) 先行ゲートが値を入れても、後続ゲートが Reject すればハンドラは呼ばれず、
/// 同じ接続の次のリクエストにも値は残らない。
#[tokio::test]
async fn rejected_request_does_not_reach_handler_nor_leak() {
    let calls = Arc::new(Mutex::new(0));
    let addr = spawn_server(
        Server::new()
            .handler(router(Arc::clone(&calls)))
            .gate(InsertingGate)
            .gate(DenyHeaderGate),
    )
    .await;
    let mut s = TcpStream::connect(addr).await.unwrap();

    s.write_all(&request(Some("alice"), "X-Deny: 1\r\n", false))
        .await
        .unwrap();
    let (status, body) = read_response(&mut s).await;
    assert!(status.starts_with("HTTP/1.1 403"));
    assert_eq!(body, "denied");
    assert_eq!(*calls.lock().unwrap(), 0, "Reject 時にハンドラが呼ばれた");

    s.write_all(&request(None, "", true)).await.unwrap();
    assert_eq!(read_response(&mut s).await.1, "none");
    assert_eq!(*calls.lock().unwrap(), 1);
}

/// (f) `Middleware::on_request` はゲート前なので空、`on_response_with_status` は
/// ゲート通過時に値が見える（拒否時は空）。
struct VisibilityMiddleware {
    on_request: Arc<Mutex<Vec<bool>>>,
    on_response: Arc<Mutex<Vec<(u16, bool)>>>,
}

impl Middleware for VisibilityMiddleware {
    fn name(&self) -> &'static str {
        "visibility"
    }

    fn on_request(&self, head: &RequestHead) {
        self.on_request
            .lock()
            .unwrap()
            .push(head.extensions().get::<Principal>().is_some());
    }

    fn on_response_with_status(&self, head: &RequestHead, status: u16, _elapsed: Duration) {
        self.on_response
            .lock()
            .unwrap()
            .push((status, head.extensions().get::<Principal>().is_some()));
    }
}

#[tokio::test]
async fn visibility_contract_for_middleware_hooks() {
    let on_request = Arc::new(Mutex::new(Vec::new()));
    let on_response = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(0));
    let addr = spawn_server(
        Server::new()
            .handler(router(calls))
            .gate(InsertingGate)
            .gate(DenyHeaderGate)
            .middleware(VisibilityMiddleware {
                on_request: Arc::clone(&on_request),
                on_response: Arc::clone(&on_response),
            }),
    )
    .await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(&request(Some("alice"), "", false))
        .await
        .unwrap();
    read_response(&mut s).await;
    s.write_all(&request(Some("alice"), "X-Deny: 1\r\n", true))
        .await
        .unwrap();
    read_response(&mut s).await;
    // on_response_with_status は応答送出後に呼ばれるため、閉じを待ってから検証する。
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(*on_request.lock().unwrap(), vec![false, false]);
    assert_eq!(
        *on_response.lock().unwrap(),
        vec![(200, true), (403, false)]
    );
}

/// (g) 同じ型を 2 つのゲートが入れると 2 本目は Err になり、拒否実装なら 500。
#[tokio::test]
async fn duplicate_insert_fails_closed() {
    let calls = Arc::new(Mutex::new(0));
    let addr = spawn_server(
        Server::new()
            .handler(router(Arc::clone(&calls)))
            .gate(AlwaysInsertGate)
            .gate(AlwaysInsertGate),
    )
    .await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(&request(None, "", true)).await.unwrap();
    let (status, _) = read_response(&mut s).await;
    assert!(status.starts_with("HTTP/1.1 500"));
    assert_eq!(*calls.lock().unwrap(), 0);
}

/// (h) `Interceptor::intercept` からも値が読める。
struct ReadingInterceptor;

impl Interceptor for ReadingInterceptor {
    fn name(&self) -> &'static str {
        "reading"
    }

    fn intercept(&self, head: &RequestHead, _body: &[u8]) -> Option<Response> {
        let p = head.extensions().get::<Principal>()?;
        (head.path() == "/intercepted")
            .then(|| Response::new(200, format!("intercepted:{}", p.0).into_bytes()))
    }
}

#[tokio::test]
async fn interceptor_sees_gate_value() {
    let calls = Arc::new(Mutex::new(0));
    let addr = spawn_server(
        Server::new()
            .handler(router(calls))
            .gate(InsertingGate)
            .interceptor(ReadingInterceptor),
    )
    .await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(
        b"GET /intercepted HTTP/1.1\r\nHost: l\r\nX-Test-User: carol\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    assert_eq!(read_response(&mut s).await.1, "intercepted:carol");
}

/// 先行ゲートの値を後続ゲートが読める（認証 → 認可の連携）。
#[test]
fn later_gate_reads_value_of_earlier_gate() {
    struct Reader;
    impl RequestGate for Reader {
        fn name(&self) -> &'static str {
            "reader"
        }
        fn check(
            &self,
            _head: &RequestHead,
            _ctx: &GateContext,
            extensions: &mut RequestExtensions,
        ) -> GateOutcome {
            if extensions.get::<Principal>().is_some() {
                GateOutcome::Allow
            } else {
                GateOutcome::reject(401, Vec::new())
            }
        }
    }
    let head = match fandhe_backend_http::request::parse_request_head(
        b"GET / HTTP/1.1\r\nX-Test-User: dave\r\n\r\n",
    )
    .unwrap()
    {
        fandhe_backend_http::request::ParseOutcome::Complete { head, .. } => head,
        _ => unreachable!(),
    };
    let ctx = GateContext::new(None);
    let mut ext = RequestExtensions::new();
    assert_eq!(
        Reader.check(&head, &ctx, &mut ext),
        GateOutcome::reject(401, Vec::new())
    );
    assert_eq!(
        InsertingGate.check(&head, &ctx, &mut ext),
        GateOutcome::Allow
    );
    assert_eq!(Reader.check(&head, &ctx, &mut ext), GateOutcome::Allow);
}
