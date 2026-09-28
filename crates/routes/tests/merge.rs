//! `Router::merge`（イシュー #722）の統合テスト。
//!
//! `crates/routes/src/lib.rs` 内の unit テストが衝突検査・引き継ぎロジックの
//! 単体挙動を、本ファイルは `Router` 経由の end-to-end 挙動（複数クレートが
//! それぞれ組み立てたサブルータを合成し、`dispatch` で解決できることの確認）を
//! 検証する。`tests/fallback.rs` の様式に合わせる。

use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::{FallbackPolicy, Router, RouterMergeError};

fn head(method: &str, target: &str) -> RequestHead {
    let request_line = format!("{method} {target} HTTP/1.1\r\n\r\n");
    match parse_request_head(request_line.as_bytes()).expect("parse should succeed") {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => panic!("expected Complete"),
    }
}

// `Router` は `Debug` を実装しないため、`Result<Router, RouterMergeError>::
// unwrap_err()` は使えない。テスト専用の手動抽出ヘルパで代替する。
fn expect_merge_err(result: Result<Router, RouterMergeError>) -> RouterMergeError {
    match result {
        Ok(_) => panic!("expected merge to fail"),
        Err(e) => e,
    }
}

/// AC1: 静的ルートとパラメータルートが混在する 2 つのサブルータを合成し、
/// どちらも解決できる（複数クレートがそれぞれ `Router` を公開する想定の
/// end-to-end シナリオ）。
#[tokio::test]
async fn merge_resolves_mixed_static_and_param_routes_from_both_sides() {
    let todos = Router::new()
        .route("GET", "/todos", |_h, _b| {
            Response::new(200, b"todos".to_vec())
        })
        .route_param("GET", "/todos/{id}", |_h, params, _b| {
            let id = params.get("id").unwrap_or("");
            Response::new(200, format!("todo:{id}").into_bytes())
        })
        .unwrap();
    let users = Router::new().route("GET", "/users", |_h, _b| {
        Response::new(200, b"users".to_vec())
    });

    let router = todos.merge(users).unwrap();

    assert_eq!(
        router.dispatch(&head("GET", "/todos"), &[]).await.body,
        b"todos".to_vec()
    );
    assert_eq!(
        router.dispatch(&head("GET", "/todos/42"), &[]).await.body,
        b"todo:42".to_vec()
    );
    assert_eq!(
        router.dispatch(&head("GET", "/users"), &[]).await.body,
        b"users".to_vec()
    );
    // 未登録パスは合成後も 404 のまま（フェイルクローズ）。
    assert_eq!(
        router.dispatch(&head("GET", "/missing"), &[]).await.status,
        404
    );
}

/// AC1: 空の `Router` との合成は恒等（両方向で確認）。
#[tokio::test]
async fn merge_with_empty_router_is_identity_both_directions() {
    let a = Router::new().route("GET", "/x", |_h, _b| Response::new(200, b"x".to_vec()));
    let left = Router::new().merge(a).unwrap();
    assert_eq!(
        left.dispatch(&head("GET", "/x"), &[]).await.body,
        b"x".to_vec()
    );

    let b = Router::new().route("GET", "/y", |_h, _b| Response::new(200, b"y".to_vec()));
    let right = b.merge(Router::new()).unwrap();
    assert_eq!(
        right.dispatch(&head("GET", "/y"), &[]).await.body,
        b"y".to_vec()
    );
}

/// AC1: 同じパスで method が異なる 2 つのサブルータを合成すると、両方が
/// 動作し、未登録 method は 405 + 集約された `Allow` を返す。
#[tokio::test]
async fn merge_same_path_different_methods_both_work_and_allow_is_aggregated() {
    let reads = Router::new().route("GET", "/todos", |_h, _b| Response::empty(200));
    let writes = Router::new().route("POST", "/todos", |_h, _b| Response::empty(201));
    let router = reads.merge(writes).unwrap();

    assert_eq!(
        router.dispatch(&head("GET", "/todos"), &[]).await.status,
        200
    );
    assert_eq!(
        router.dispatch(&head("POST", "/todos"), &[]).await.status,
        201
    );
    let res = router.dispatch(&head("DELETE", "/todos"), &[]).await;
    assert_eq!(res.status, 405);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET, POST\r\n"));
}

/// AC2: 静的ルートの `(method, path)` 重複は `DuplicateRoute` エラーになる
/// （フィールド値まで検証）。
#[tokio::test]
async fn merge_duplicate_static_route_is_rejected() {
    let a = Router::new().route("GET", "/x", |_h, _b| Response::empty(200));
    let b = Router::new().route("GET", "/x", |_h, _b| Response::empty(201));

    let err = expect_merge_err(a.merge(b));
    assert_eq!(
        err,
        RouterMergeError::DuplicateRoute {
            method: "GET".to_string(),
            path: "/x".to_string(),
        }
    );
}

/// AC2: パラメータルートが名前違いで形状等価な場合も衝突として検出する。
#[tokio::test]
async fn merge_param_routes_with_different_param_names_but_same_shape_conflict() {
    let a = Router::new()
        .route_param("GET", "/a/{id}", |_h, _p, _b| Response::empty(200))
        .unwrap();
    let b = Router::new()
        .route_param("GET", "/a/{name}", |_h, _p, _b| Response::empty(200))
        .unwrap();

    let err = expect_merge_err(a.merge(b));
    assert!(matches!(
        err,
        RouterMergeError::DuplicateParamRoute { ref method, ref pattern }
            if method == "GET" && pattern == "/a/{id}"
    ));
}

/// AC2: 部分的な重なり（`/a/{x}` と `/a/{*rest}`）は衝突ではなく成功し、
/// 登録順（self → other）で解決される。
#[tokio::test]
async fn merge_partial_overlap_param_and_wildcard_succeeds_with_registration_order() {
    let single = Router::new()
        .route_param("GET", "/a/{x}", |_h, _p, _b| {
            Response::new(200, b"single".to_vec())
        })
        .unwrap();
    let wildcard = Router::new()
        .route_param("GET", "/a/{*rest}", |_h, params, _b| {
            let rest = params.get("rest").unwrap_or("");
            Response::new(200, format!("wildcard:{rest}").into_bytes())
        })
        .unwrap();
    let router = single.merge(wildcard).unwrap();

    assert_eq!(
        router.dispatch(&head("GET", "/a/b"), &[]).await.body,
        b"single".to_vec()
    );
    assert_eq!(
        router.dispatch(&head("GET", "/a/b/c"), &[]).await.body,
        b"wildcard:b/c".to_vec()
    );
}

/// AC2: self のパラメータルートと other の静的ルートが重なっても衝突とは
/// 扱わず、既存の優先順位（静的 → パラメータ）で静的側が解決される。
#[tokio::test]
async fn merge_self_param_and_other_static_overlap_prefers_static() {
    let param = Router::new()
        .route_param("GET", "/a/{x}", |_h, _p, _b| {
            Response::new(200, b"param".to_vec())
        })
        .unwrap();
    let static_router = Router::new().route("GET", "/a/b", |_h, _b| {
        Response::new(200, b"static".to_vec())
    });
    let router = param.merge(static_router).unwrap();

    assert_eq!(
        router.dispatch(&head("GET", "/a/b"), &[]).await.body,
        b"static".to_vec()
    );
    assert_eq!(
        router.dispatch(&head("GET", "/a/c"), &[]).await.body,
        b"param".to_vec()
    );
}

/// AC2: エラーの `Display` は空でなく method・path を含み、
/// `Box<dyn std::error::Error>` に変換できる。
#[tokio::test]
async fn merge_error_display_contains_method_and_path_and_converts_to_boxed_error() {
    let a = Router::new().route("GET", "/x", |_h, _b| Response::empty(200));
    let b = Router::new().route("GET", "/x", |_h, _b| Response::empty(200));

    let err = expect_merge_err(a.merge(b));
    let text = err.to_string();
    assert!(!text.is_empty());
    assert!(text.contains("GET"));
    assert!(text.contains("/x"));

    let boxed: Box<dyn std::error::Error> = Box::new(err);
    assert!(!boxed.to_string().is_empty());
}

/// AC3: self にのみ fallback がある場合、other にのみある場合のどちらでも
/// 引き継がれ、`FallbackPolicy::IncludeMethodNotAllowed` も保持される。
#[tokio::test]
async fn merge_fallback_only_on_either_side_is_inherited() {
    let with_fallback = Router::new()
        .route("GET", "/a", |_h, _b| Response::empty(200))
        .fallback(|_h, _b| Response::new(404, b"from-a".to_vec()));
    let plain = Router::new().route("GET", "/b", |_h, _b| Response::empty(200));
    let router = with_fallback.merge(plain).unwrap();
    assert_eq!(
        router.dispatch(&head("GET", "/missing"), &[]).await.body,
        b"from-a".to_vec()
    );

    let plain2 = Router::new().route("GET", "/a", |_h, _b| Response::empty(200));
    let with_fallback2 = Router::new()
        .route("GET", "/b", |_h, _b| Response::empty(200))
        .fallback_with(FallbackPolicy::IncludeMethodNotAllowed, |_h, _b| {
            Response::new(404, b"from-b".to_vec())
        });
    let router2 = plain2.merge(with_fallback2).unwrap();
    // 405 相当のリクエストも IncludeMethodNotAllowed のまま fallback に流れる。
    let res = router2.dispatch(&head("POST", "/a"), &[]).await;
    assert_eq!(res.status, 404);
    assert_eq!(res.body, b"from-b".to_vec());
}

/// AC3: 両方に fallback があると `ConflictingFallback` エラーになる。
#[tokio::test]
async fn merge_both_fallback_conflicts() {
    let a = Router::new().fallback(|_h, _b| Response::new(404, b"a".to_vec()));
    let b = Router::new().fallback(|_h, _b| Response::new(404, b"b".to_vec()));

    let err = expect_merge_err(a.merge(b));
    assert_eq!(err, RouterMergeError::ConflictingFallback);
}

/// AC3: options_fallback も self のみ・other のみ・両方でエラー、の 3 パターン。
/// 引き継いだ options_fallback の `Allow` には合成後の全 method が入る。
#[tokio::test]
async fn merge_options_fallback_three_patterns() {
    let a = Router::new()
        .route("GET", "/todos", |_h, _b| Response::empty(200))
        .options_fallback(|_head, allow, _body| Response::empty(204).with_allow(allow.clone()));
    let b = Router::new().route("POST", "/todos", |_h, _b| Response::empty(201));
    let router = a.merge(b).unwrap();
    let res = router.dispatch(&head("OPTIONS", "/todos"), &[]).await;
    assert_eq!(res.status, 204);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET, POST\r\n"));

    let c = Router::new().route("GET", "/x", |_h, _b| Response::empty(200));
    let d = Router::new()
        .route("POST", "/x", |_h, _b| Response::empty(201))
        .options_fallback(|_head, allow, _body| Response::empty(204).with_allow(allow.clone()));
    let router2 = c.merge(d).unwrap();
    let res2 = router2.dispatch(&head("OPTIONS", "/x"), &[]).await;
    assert_eq!(res2.status, 204);

    let e = Router::new()
        .options_fallback(|_head, allow, _body| Response::empty(204).with_allow(allow.clone()));
    let f = Router::new()
        .options_fallback(|_head, allow, _body| Response::empty(204).with_allow(allow.clone()));
    let err = expect_merge_err(e.merge(f));
    assert_eq!(err, RouterMergeError::ConflictingOptionsFallback);
}

/// AC3: どちらにも fallback がなければ既定の 404 / 405 + `Allow` が保たれる。
#[tokio::test]
async fn merge_neither_side_has_fallback_default_behavior_is_preserved() {
    let a = Router::new().route("GET", "/a", |_h, _b| Response::empty(200));
    let b = Router::new().route("GET", "/b", |_h, _b| Response::empty(200));
    let router = a.merge(b).unwrap();

    assert_eq!(
        router.dispatch(&head("GET", "/missing"), &[]).await.status,
        404
    );
    let res = router.dispatch(&head("POST", "/a"), &[]).await;
    assert_eq!(res.status, 405);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET\r\n"));
}

/// AC3: サブルータ由来の fallback が、もう一方のサブルータの未マッチパスにも
/// 適用される（fallback がルータ全体に適用される意味論の固定化）。
#[tokio::test]
async fn merge_fallback_from_one_subrouter_applies_to_other_subrouter_unmatched_paths() {
    let ai = Router::new()
        .route("GET", "/ai/status", |_h, _b| Response::empty(200))
        .fallback(|_h, _b| Response::new(404, b"ai-not-found".to_vec()));
    let cdp = Router::new().route("GET", "/cdp/status", |_h, _b| Response::empty(200));
    let router = ai.merge(cdp).unwrap();

    // cdp 側にしか存在しないはずのパスの未マッチも、ai 由来の fallback に流れる。
    let res = router.dispatch(&head("GET", "/cdp/missing"), &[]).await;
    assert_eq!(res.body, b"ai-not-found".to_vec());
}

/// async ハンドラ（`route_async` / `route_param_async`）も合成後に動作する。
#[tokio::test]
async fn merge_preserves_async_handlers() {
    let a = Router::new().route_async("GET", "/slow", |_h, _b| async {
        Response::new(200, b"slow-ok".to_vec())
    });
    let b = Router::new()
        .route_param_async("GET", "/hello/{name}", |_h, params, _b| {
            let name = params.get("name").unwrap_or("world").to_string();
            async move { Response::new(200, format!("hello, {name}").into_bytes()) }
        })
        .unwrap();
    let router = a.merge(b).unwrap();

    assert_eq!(
        router.dispatch(&head("GET", "/slow"), &[]).await.body,
        b"slow-ok".to_vec()
    );
    assert_eq!(
        router
            .dispatch(&head("GET", "/hello/alice"), &[])
            .await
            .body,
        b"hello, alice".to_vec()
    );
}
