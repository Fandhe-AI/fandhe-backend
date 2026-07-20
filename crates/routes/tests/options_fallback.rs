//! `Router::options_fallback`（イシュー #304）の統合テスト。
//!
//! `crates/routes/src/lib.rs` 内の unit テストがフック分岐の単体挙動を、
//! 本ファイルは `Router` 経由の end-to-end 挙動（静的 + パラメータ混在ルータでの
//! プリフライト応答）を検証する。`tests/path_params.rs` の様式に合わせる。
//! 204 + `Allow` を返す実装例は、次イシューの CORS プラグインが組み立てる
//! 応答（Origin 検証・`Access-Control-Allow-*` 付与）の最小雛形でもある。

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
async fn preflight_options_returns_204_with_allow_header_for_static_route() {
    let router = Router::new()
        .route("GET", "/todos", |_h, _b| Response::empty(200))
        .route("POST", "/todos", |_h, _b| Response::empty(201))
        .route("DELETE", "/todos", |_h, _b| Response::empty(204))
        .options_fallback(|_head, allow, _body| Response::empty(204).with_allow(allow.clone()));

    let res = router.dispatch(&head("OPTIONS", "/todos"), &[]).await;
    assert_eq!(res.status, 204);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: DELETE, GET, POST\r\n"));
}

#[tokio::test]
async fn preflight_options_returns_204_with_allow_header_for_param_route() {
    let router = Router::new()
        .route_param("GET", "/hello/{name}", |_h, _params, _b| {
            Response::empty(200)
        })
        .unwrap()
        .route_param("PUT", "/hello/{name}", |_h, _params, _b| {
            Response::empty(200)
        })
        .unwrap()
        .options_fallback(|_head, allow, _body| Response::empty(204).with_allow(allow.clone()));

    let res = router.dispatch(&head("OPTIONS", "/hello/alice"), &[]).await;
    assert_eq!(res.status, 204);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET, PUT\r\n"));
}

#[tokio::test]
async fn preflight_options_falls_back_to_404_for_unregistered_path_even_with_fallback_registered() {
    let router = Router::new()
        .route("GET", "/todos", |_h, _b| Response::empty(200))
        .options_fallback(|_head, allow, _body| Response::empty(204).with_allow(allow.clone()));

    let res = router.dispatch(&head("OPTIONS", "/unknown"), &[]).await;
    assert_eq!(res.status, 404);
}

#[tokio::test]
async fn preflight_options_without_fallback_registered_stays_405_for_backward_compatibility() {
    let router = Router::new()
        .route("GET", "/todos", |_h, _b| Response::empty(200))
        .route("POST", "/todos", |_h, _b| Response::empty(201));

    let res = router.dispatch(&head("OPTIONS", "/todos"), &[]).await;
    assert_eq!(res.status, 405);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET, POST\r\n"));
}

#[tokio::test]
async fn explicit_options_route_still_wins_over_fallback_in_mixed_router() {
    let router = Router::new()
        .route("GET", "/todos", |_h, _b| Response::empty(200))
        .route("OPTIONS", "/todos", |_h, _b| {
            Response::new(200, b"explicit-options".to_vec())
        })
        .route_param("GET", "/hello/{name}", |_h, _params, _b| {
            Response::empty(200)
        })
        .unwrap()
        .options_fallback(|_head, allow, _body| Response::empty(204).with_allow(allow.clone()));

    // 明示登録された静的 OPTIONS ルートが優先される。
    let explicit = router.dispatch(&head("OPTIONS", "/todos"), &[]).await;
    assert_eq!(explicit.status, 200);
    assert_eq!(explicit.body, b"explicit-options".to_vec());

    // 明示登録のないパラメータルートはフォールバックへ委譲される。
    let via_fallback = router.dispatch(&head("OPTIONS", "/hello/alice"), &[]).await;
    assert_eq!(via_fallback.status, 204);
}
