//! `Router::fallback` / `Router::fallback_with`（イシュー #316）の統合テスト。
//!
//! `crates/routes/src/lib.rs` 内の unit テストがフック分岐の単体挙動を、本ファイルは
//! `Router` 経由の end-to-end 挙動（静的 + パラメータ混在ルータでの優先順位・
//! `FallbackPolicy` 選択・SPA ユースケース想定の空 Router）を検証する。
//! `tests/options_fallback.rs` の様式に合わせる。`Router::dispatch` はイシュー #315
//! で `HandlerFuture`（boxed future）を返す契約へ移行したため、本ファイルの各テストも
//! `#[tokio::test]` + `.await` で検証する（`tests/options_fallback.rs` と同一パターン）。

use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::{FallbackPolicy, Router};

fn head(method: &str, target: &str) -> RequestHead {
    let request_line = format!("{method} {target} HTTP/1.1\r\n\r\n");
    match parse_request_head(request_line.as_bytes()).expect("parse should succeed") {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => panic!("expected Complete"),
    }
}

#[tokio::test]
async fn fallback_unregistered_preserves_existing_404_and_405_behavior() {
    // 受け入れ条件 1: fallback 未登録時の既存挙動（404 / 405 + Allow）を維持する。
    let router = Router::new()
        .route("GET", "/health", |_h, _b| Response::empty(200))
        .route_param("GET", "/hello/{name}", |_h, _p, _b| Response::empty(200))
        .unwrap();

    let res = router.dispatch(&head("GET", "/missing"), &[]).await;
    assert_eq!(res.status, 404);

    let res = router.dispatch(&head("POST", "/health"), &[]).await;
    assert_eq!(res.status, 405);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET\r\n"));

    // パラメータルート形状一致・メソッド不一致でも同様に 405 + Allow を維持する。
    let res = router.dispatch(&head("POST", "/hello/alice"), &[]).await;
    assert_eq!(res.status, 405);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET\r\n"));
}

#[tokio::test]
async fn fallback_default_policy_only_intercepts_404_not_405() {
    // 受け入れ条件 2: 既定ポリシー（NotFoundOnly）は 404 のみ委譲し、405 は
    // 従来どおり Allow 付きで返す。
    let router = Router::new()
        .route("GET", "/health", |_h, _b| Response::empty(200))
        .fallback(|_h, _b| Response::new(404, b"custom-not-found".to_vec()));

    let res = router.dispatch(&head("GET", "/missing"), &[]).await;
    assert_eq!(res.status, 404);
    assert_eq!(res.body, b"custom-not-found".to_vec());

    let res = router.dispatch(&head("POST", "/health"), &[]).await;
    assert_eq!(res.status, 405);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET\r\n"));
}

#[tokio::test]
async fn fallback_include_method_not_allowed_policy_intercepts_405_without_allow_header() {
    // 受け入れ条件 2: IncludeMethodNotAllowed 明示指定時は 405 も fallback へ流れ、
    // Allow ヘッダは付与されない。
    let router = Router::new()
        .route("GET", "/health", |_h, _b| Response::empty(200))
        .fallback_with(FallbackPolicy::IncludeMethodNotAllowed, |_h, _b| {
            Response::new(404, b"custom-catch-all".to_vec())
        });

    let res = router.dispatch(&head("POST", "/health"), &[]).await;
    assert_eq!(res.status, 404);
    assert_eq!(res.body, b"custom-catch-all".to_vec());
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(!text.contains("Allow:"));
}

#[tokio::test]
async fn fallback_dispatch_priority_static_over_param_over_fallback() {
    // 受け入れ条件 3: 静的 → パラメータ → fallback の優先順位を単一テストで固定化する。
    let router = Router::new()
        .route("GET", "/hello/world", |_h, _b| {
            Response::new(200, b"static".to_vec())
        })
        .route_param("GET", "/hello/{name}", |_h, params, _b| {
            let name = params.get("name").unwrap_or("");
            Response::new(200, format!("param:{name}").into_bytes())
        })
        .unwrap()
        .fallback(|_h, _b| Response::new(404, b"fallback".to_vec()));

    assert_eq!(
        router
            .dispatch(&head("GET", "/hello/world"), &[])
            .await
            .body,
        b"static".to_vec()
    );
    assert_eq!(
        router
            .dispatch(&head("GET", "/hello/alice"), &[])
            .await
            .body,
        b"param:alice".to_vec()
    );
    assert_eq!(
        router.dispatch(&head("GET", "/other"), &[]).await.body,
        b"fallback".to_vec()
    );
}

#[tokio::test]
async fn fallback_spa_style_empty_router_serves_all_requests() {
    // SPA ユースケース: ルート未登録の空 Router + fallback で全リクエストが
    // fallback（index.html 相当）に到達する。
    let router = Router::new()
        .fallback(|_h, _b| Response::new(200, b"<!doctype html><html>spa index</html>".to_vec()));

    for (method, path) in [
        ("GET", "/"),
        ("GET", "/app/route"),
        ("GET", "/deep/nested/path"),
    ] {
        let res = router.dispatch(&head(method, path), &[]).await;
        assert_eq!(res.status, 200);
        assert_eq!(res.body, b"<!doctype html><html>spa index</html>".to_vec());
    }
}

#[tokio::test]
async fn fallback_re_registration_last_wins() {
    let router = Router::new()
        .fallback(|_h, _b| Response::new(404, b"first".to_vec()))
        .fallback(|_h, _b| Response::new(404, b"second".to_vec()));

    let res = router.dispatch(&head("GET", "/missing"), &[]).await;
    assert_eq!(res.body, b"second".to_vec());
}
