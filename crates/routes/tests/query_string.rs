//! `Router::dispatch` のクエリ文字列分離対応（イシュー #258）の統合テスト。
//!
//! `crates/http/src/request.rs` の `RequestHead::path` / `RequestHead::query`
//! unit テストがパース・分離の単体挙動を、本ファイルは `Router` 経由の
//! end-to-end 挙動（静的ルート・パラメータルート・405 Allow・非デコード契約の
//! 固定化）を検証する。`docs/acceptance/req3-openapi-generation.md` の
//! `GET /search?q=...&limit=...` 手動突合が前提とする挙動そのもの。

use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::Router;

fn head(method: &str, target: &str) -> RequestHead {
    let request_line = format!("{method} {target} HTTP/1.1\r\n\r\n");
    match parse_request_head(request_line.as_bytes()).expect("parse should succeed") {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => panic!("expected Complete"),
    }
}

#[tokio::test]
async fn static_route_matches_request_with_query_string() {
    // 受け入れ条件 1: `GET /search?q=x` が `route("GET", "/search")` に一致し 200。
    let router = Router::new().route("GET", "/search", |_h, _b| {
        Response::new(200, b"ok".to_vec())
    });

    let res = router.dispatch(&head("GET", "/search?q=x"), &[]).await;
    assert_eq!(res.status, 200);
}

#[tokio::test]
async fn handler_can_access_query_via_request_head() {
    // 受け入れ条件 2: ハンドラ内で `head.query()` が生のクエリ文字列を返す。
    let router = Router::new().route("GET", "/search", |head, _b| {
        let query = head.query().unwrap_or("");
        Response::new(200, query.as_bytes().to_vec())
    });

    let res = router.dispatch(&head("GET", "/search?q=x"), &[]).await;
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"q=x".to_vec());
}

#[tokio::test]
async fn static_route_matches_empty_and_absent_query() {
    // 受け入れ条件 3: `?` のみ（空クエリ）・クエリなしのいずれも同一ルートに一致する。
    let router = Router::new().route("GET", "/search", |_h, _b| {
        Response::new(200, b"ok".to_vec())
    });

    assert_eq!(
        router.dispatch(&head("GET", "/search?"), &[]).await.status,
        200
    );
    assert_eq!(
        router.dispatch(&head("GET", "/search"), &[]).await.status,
        200
    );
}

#[tokio::test]
async fn param_route_matches_request_with_query_string_without_capturing_it() {
    // `?` がキャプチャに混入せず `name` パラメータが正しく取れることを確認する。
    let router = Router::new()
        .route_param("GET", "/hello/{name}", |_h, params, _b| {
            let name = params.get("name").unwrap_or("?");
            Response::new(200, name.as_bytes().to_vec())
        })
        .unwrap();

    let res = router.dispatch(&head("GET", "/hello/alice?x=1"), &[]).await;
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"alice".to_vec());
}

#[tokio::test]
async fn method_mismatch_with_query_string_returns_405_with_allow() {
    // 405 + Allow: クエリ付きリクエストでも Allow 集約がパス基準で正しく効く。
    let router = Router::new().route("GET", "/search", |_h, _b| {
        Response::new(200, b"ok".to_vec())
    });

    let res = router.dispatch(&head("POST", "/search?q=x"), &[]).await;
    assert_eq!(res.status, 405);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET\r\n"));
}

#[tokio::test]
async fn unregistered_path_with_query_string_returns_404() {
    let router = Router::new().route("GET", "/search", |_h, _b| {
        Response::new(200, b"ok".to_vec())
    });

    let res = router.dispatch(&head("GET", "/nope?q=x"), &[]).await;
    assert_eq!(res.status, 404);
}

#[tokio::test]
async fn percent_encoded_question_mark_does_not_split_path() {
    // 非デコード契約の固定化: `%3F` はパスの一部として扱われ、`/search` には一致しない。
    let router = Router::new().route("GET", "/search", |_h, _b| {
        Response::new(200, b"ok".to_vec())
    });

    let res = router.dispatch(&head("GET", "/search%3Fq=x"), &[]).await;
    assert_eq!(res.status, 404);
}
