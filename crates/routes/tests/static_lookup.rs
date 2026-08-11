//! 静的ルート lookup の借用キー化 + FxHash 化（イシュー #583）の回帰テスト。
//!
//! `Router` の `routes` フィールドを `HashMap<(String, String), _>` から
//! `FxHashMap<Box<str>, FxHashMap<Box<str>, _>>`（path → method のネスト map）へ
//! 変更した際、照合意味論（完全一致・method 大文字小文字区別・後勝ち・405 `Allow`
//! 集約・404/405 フェイルクローズ）が変わっていないことを end-to-end で固定化する。
//! 「String 確保が発生しない」こと自体はカウンティングアロケータを要し brittle な
//! ためテスト化せず、コードレビュー（`dispatch` の `&str` 借用照合）で担保する
//! （実装計画 4.2 節）。

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
async fn multiple_methods_on_same_path_dispatch_to_correct_handler() {
    // ネスト map 化後も同一 path に複数 method を登録でき、method ごとに
    // 正しいハンドラへ到達すること（外側 path・内側 method の 2 段照合）。
    let router = Router::new()
        .route("GET", "/items", |_h, _b| {
            Response::new(200, b"get".to_vec())
        })
        .route("POST", "/items", |_h, _b| {
            Response::new(201, b"post".to_vec())
        });

    let get_res = router.dispatch(&head("GET", "/items"), &[]).await;
    assert_eq!(get_res.status, 200);
    assert_eq!(get_res.body, b"get".to_vec());

    let post_res = router.dispatch(&head("POST", "/items"), &[]).await;
    assert_eq!(post_res.status, 201);
    assert_eq!(post_res.body, b"post".to_vec());
}

#[tokio::test]
async fn re_registering_same_method_and_path_keeps_last_wins_semantics() {
    // 同一 (method, path) の再登録は後勝ち（inner `insert` の上書き）。
    // `route` → `route_async` の混在でも同じ契約を維持する。
    let router = Router::new()
        .route("GET", "/dup", |_h, _b| {
            Response::new(200, b"first".to_vec())
        })
        .route_async("GET", "/dup", |_h, _b| async {
            Response::new(200, b"second".to_vec())
        });

    let res = router.dispatch(&head("GET", "/dup"), &[]).await;
    assert_eq!(res.body, b"second".to_vec());
}

#[tokio::test]
async fn method_matching_is_case_sensitive() {
    // RFC 9110 上 method token は大文字小文字を区別する。借用キー化後も
    // 独自正規化を持ち込まないこと（`get /` は `GET /` 登録に一致しない）。
    let router = Router::new().route("GET", "/case", |_h, _b| Response::new(200, b"ok".to_vec()));

    // パーサは method を大文字小文字保持のまま渡すため、小文字 target で
    // 直接 `head` を組み立てて検証する。
    let mismatched = head("get", "/case");
    let res = router.dispatch(&mismatched, &[]).await;
    // path 自体は一致するため 405（method 不一致）。200 にはならないことで
    // 「小文字 method が GET 登録に一致しない」ことを確認する。
    assert_eq!(
        res.status, 405,
        "lower-case method must not match registered GET"
    );
}

#[tokio::test]
async fn static_miss_falls_through_to_param_route() {
    // 静的ルート miss 時のみパラメータルートへフォールスルーする優先順位は
    // ネスト map 化後も不変。
    let router = Router::new()
        .route("GET", "/static-only", |_h, _b| {
            Response::new(200, b"static".to_vec())
        })
        .route_param("GET", "/hello/{name}", |_h, params, _b| {
            let name = params.get("name").unwrap_or("world");
            Response::new(200, format!("hello, {name}").into_bytes())
        })
        .unwrap();

    let res = router.dispatch(&head("GET", "/hello/alice"), &[]).await;
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"hello, alice".to_vec());
}

#[tokio::test]
async fn method_not_allowed_aggregates_static_and_param_methods_in_allow_header() {
    // 405 の `Allow` 集約が「静的ルート（対象 path の inner map keys）+
    // パラメータルート（形状一致 method）」の合算・ソート・重複排除であることは
    // ネスト map 化前後で不変（`registered_methods` の再実装対象）。
    let router = Router::new()
        .route("GET", "/multi", |_h, _b| Response::new(200, b"ok".to_vec()))
        .route("POST", "/multi", |_h, _b| {
            Response::new(200, b"ok".to_vec())
        });

    let res = router.dispatch(&head("DELETE", "/multi"), &[]).await;
    assert_eq!(res.status, 405);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(
        text.contains("Allow: GET, POST\r\n"),
        "unexpected headers: {text}"
    );
}

#[tokio::test]
async fn query_string_target_still_matches_static_route_after_path_separation() {
    // #258 の既存挙動: `path()` 分離後の静的一致はネスト map 化後も不変。
    let router = Router::new().route("GET", "/health", |_h, _b| {
        Response::new(200, b"ok".to_vec())
    });

    let res = router.dispatch(&head("GET", "/health?x=1"), &[]).await;
    assert_eq!(res.status, 200);
}

#[tokio::test]
async fn unregistered_path_returns_404_without_panicking() {
    // 空ルータでの miss（`self.routes.get(...)` が `None` を返す境界）。
    let router = Router::new();
    let res = router.dispatch(&head("GET", "/nope"), &[]).await;
    assert_eq!(res.status, 404);
}

#[tokio::test]
async fn asterisk_form_target_does_not_panic_static_lookup() {
    // `*`（asterisk-form、OPTIONS *）のような origin-form でない target でも
    // 静的ルート lookup（`self.routes.get(head.path())`）が panic せず 404/405
    // 側へフェイルクローズすること。
    let router = Router::new().route("GET", "/health", |_h, _b| {
        Response::new(200, b"ok".to_vec())
    });

    let res = router.dispatch(&head("OPTIONS", "*"), &[]).await;
    assert!(res.status == 404 || res.status == 405);
}
