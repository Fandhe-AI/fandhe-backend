//! `Router::route_async` / `Router::route_param_async` の受け入れテスト（イシュー #315）。
//!
//! `docs/design/async-handler.md` 採用案 (c) の実証: async ハンドラが登録・解決でき、
//! 同期ハンドラ（`route` / `route_param`）と混在させても優先順位（静的完全一致 →
//! パラメータルート登録順）が崩れないことを確認する。`crates/core/tests/async_handler.rs`
//! は実 TCP 接続経由の統合テスト（sleep・並行性・panic 境界）を担い、本ファイルは
//! `Router` 単体の登録・dispatch の契約検証に責務を絞る。

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::Router;

fn head(method: &str, target: &str) -> fandhe_backend_http::request::RequestHead {
    let request_line = format!("{method} {target} HTTP/1.1\r\n\r\n");
    match parse_request_head(request_line.as_bytes()).expect("parse should succeed") {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => panic!("expected Complete"),
    }
}

#[tokio::test]
async fn route_async_dispatches_and_awaits_future() {
    let router = Router::new().route_async("GET", "/async", |_h, _b| async {
        // 実利用の非同期 I/O（DB クエリ等）を模した一呼吸置く処理。
        tokio::task::yield_now().await;
        Response::new(200, b"async-ok".to_vec())
    });

    let res = router.dispatch(&head("GET", "/async"), &[]).await;
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"async-ok".to_vec());
}

#[tokio::test]
async fn route_param_async_binds_params_and_awaits_future() {
    let router = Router::new()
        .route_param_async("GET", "/hello/{name}", |_h, params, _b| {
            // `Fut: 'static` 契約のため、借用（`params`）から必要な値を
            // 同期部で複製してから `async move` へ渡す。
            let name = params.get("name").unwrap_or("world").to_string();
            async move {
                tokio::task::yield_now().await;
                Response::new(200, format!("hello, {name}").into_bytes())
            }
        })
        .unwrap();

    let res = router.dispatch(&head("GET", "/hello/alice"), &[]).await;
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"hello, alice".to_vec());
}

#[tokio::test]
async fn static_async_route_still_takes_priority_over_param_route() {
    // 静的ルート優先の意味論（モジュール doc「マッチング方針」節）は
    // async ルート同士でも崩れない。
    let router = Router::new()
        .route_async("GET", "/hello/alice", |_h, _b| async {
            Response::new(200, b"static".to_vec())
        })
        .route_param_async("GET", "/hello/{name}", |_h, _params, _b| async {
            Response::new(200, b"param".to_vec())
        })
        .unwrap();

    let res = router.dispatch(&head("GET", "/hello/alice"), &[]).await;
    assert_eq!(res.body, b"static".to_vec());

    // /hello/bob は静的ルートに一致しないためパラメータルートへフォールバック。
    let res2 = router.dispatch(&head("GET", "/hello/bob"), &[]).await;
    assert_eq!(res2.body, b"param".to_vec());
}

#[tokio::test]
async fn sync_and_async_routes_coexist_without_interference() {
    // route()（同期）と route_async()（非同期）を同一 Router に混在登録しても
    // 互いに干渉しない（内部的にはどちらも同一の `RouteHandler` 型へ収束するため）。
    let router = Router::new()
        .route("GET", "/sync", |_h, _b| {
            Response::new(200, b"sync-ok".to_vec())
        })
        .route_async("GET", "/async", |_h, _b| async {
            Response::new(200, b"async-ok".to_vec())
        });

    assert_eq!(
        router.dispatch(&head("GET", "/sync"), &[]).await.body,
        b"sync-ok".to_vec()
    );
    assert_eq!(
        router.dispatch(&head("GET", "/async"), &[]).await.body,
        b"async-ok".to_vec()
    );
}

#[tokio::test]
async fn async_route_method_mismatch_still_returns_405_with_allow() {
    // async ハンドラ経由の登録でも 404/405 のフェイルクローズ・Allow 集約
    // （TASK-177 / #177）は変わらない。
    let router =
        Router::new().route_async("GET", "/async", |_h, _b| async { Response::empty(200) });

    let res = router.dispatch(&head("POST", "/async"), &[]).await;
    assert_eq!(res.status, 405);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET\r\n"));
}
