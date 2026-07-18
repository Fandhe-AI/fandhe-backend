//! `Router::route_param`（TASK-176、#176）の統合テスト。
//!
//! `crates/routes/src/lib.rs` 内の unit テストがパース・照合の単体挙動を、
//! 本ファイルは `Router` 経由の end-to-end 挙動（後方互換・優先順位・
//! フェイルクローズ）を検証する。

use bf_http::request::{ParseOutcome, RequestHead, parse_request_head};
use bf_http::response::Response;
use bf_routes::Router;

fn head(method: &str, target: &str) -> RequestHead {
    let request_line = format!("{method} {target} HTTP/1.1\r\n\r\n");
    match parse_request_head(request_line.as_bytes()).expect("parse should succeed") {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => panic!("expected Complete"),
    }
}

#[test]
fn param_route_binds_single_segment() {
    let router = Router::new()
        .route_param("GET", "/hello/{name}", |_h, params, _b| {
            let name = params.get("name").unwrap_or("world");
            Response::new(200, format!("hello, {name}").into_bytes())
        })
        .unwrap();

    let res = router.dispatch(&head("GET", "/hello/alice"), &[]);
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"hello, alice".to_vec());
}

#[test]
fn param_route_binds_multiple_segments() {
    let router = Router::new()
        .route_param("GET", "/users/{id}/posts/{post_id}", |_h, params, _b| {
            let id = params.get("id").unwrap_or("?");
            let post_id = params.get("post_id").unwrap_or("?");
            Response::new(200, format!("{id}:{post_id}").into_bytes())
        })
        .unwrap();

    let res = router.dispatch(&head("GET", "/users/7/posts/99"), &[]);
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"7:99".to_vec());
}

#[test]
fn static_route_takes_priority_over_param_route() {
    // 静的ルート（完全一致）が常にパラメータルートより優先される
    // （後方互換・モジュール doc「マッチング方針」節）。
    let router = Router::new()
        .route("GET", "/hello/alice", |_h, _b| {
            Response::new(200, b"static".to_vec())
        })
        .route_param("GET", "/hello/{name}", |_h, _params, _b| {
            Response::new(200, b"param".to_vec())
        })
        .unwrap();

    let res = router.dispatch(&head("GET", "/hello/alice"), &[]);
    assert_eq!(res.body, b"static".to_vec());

    // 静的一致しない別の名前はパラメータルートへフォールスルーする。
    let res2 = router.dispatch(&head("GET", "/hello/bob"), &[]);
    assert_eq!(res2.body, b"param".to_vec());
}

#[test]
fn param_routes_are_matched_in_registration_order() {
    let router = Router::new()
        .route_param("GET", "/a/{x}", |_h, _params, _b| {
            Response::new(200, b"first".to_vec())
        })
        .unwrap()
        .route_param("GET", "/{y}/b", |_h, _params, _b| {
            Response::new(200, b"second".to_vec())
        })
        .unwrap();

    // "/a/b" は両パターンの segment 形状に一致し得るが、登録順で最初に
    // マッチした "/a/{x}" が採用される。
    let res = router.dispatch(&head("GET", "/a/b"), &[]);
    assert_eq!(res.body, b"first".to_vec());
}

#[test]
fn segment_count_mismatch_does_not_match() {
    let router = Router::new()
        .route_param("GET", "/hello/{name}", |_h, _params, _b| {
            Response::new(200, b"param".to_vec())
        })
        .unwrap();

    assert_eq!(router.dispatch(&head("GET", "/hello/a/b"), &[]).status, 404);
    assert_eq!(router.dispatch(&head("GET", "/hello"), &[]).status, 404);
}

#[test]
fn empty_segment_does_not_match() {
    let router = Router::new()
        .route_param("GET", "/hello/{name}", |_h, _params, _b| {
            Response::new(200, b"param".to_vec())
        })
        .unwrap();

    assert_eq!(router.dispatch(&head("GET", "/hello//"), &[]).status, 404);
}

#[test]
fn dot_and_dotdot_segments_are_rejected_for_path_traversal_defense() {
    let router = Router::new()
        .route_param("GET", "/files/{name}", |_h, _params, _b| {
            Response::new(200, b"param".to_vec())
        })
        .unwrap();

    assert_eq!(router.dispatch(&head("GET", "/files/."), &[]).status, 404);
    assert_eq!(router.dispatch(&head("GET", "/files/.."), &[]).status, 404);
}

#[test]
fn method_mismatch_on_param_route_returns_405() {
    let router = Router::new()
        .route_param("GET", "/hello/{name}", |_h, _params, _b| {
            Response::new(200, b"param".to_vec())
        })
        .unwrap();

    assert_eq!(
        router.dispatch(&head("POST", "/hello/alice"), &[]).status,
        405
    );
}

#[test]
fn method_mismatch_on_param_route_includes_allow_header() {
    // main 側で追加された 405 の Allow ヘッダ方針（TASK-177、#177）を
    // パラメータルートにマージする際、param_route の method が Allow 候補に
    // 正しく集約されることの回帰テスト（TASK-176 と TASK-177 のマージ地点）。
    let router = Router::new()
        .route_param("GET", "/hello/{name}", |_h, _params, _b| {
            Response::new(200, b"param".to_vec())
        })
        .unwrap();

    let res = router.dispatch(&head("POST", "/hello/alice"), &[]);
    assert_eq!(res.status, 405);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET\r\n"));
}

#[test]
fn method_mismatch_allow_header_combines_static_and_param_routes() {
    // 静的ルートとパラメータルートが同じ target 形状に対して別 method を
    // 登録している場合、405 の Allow ヘッダは両方の method をソート済み・
    // 重複排除済みで含む必要がある。
    let router = Router::new()
        .route("PUT", "/hello/alice", |_h, _b| {
            Response::new(200, b"static".to_vec())
        })
        .route_param("GET", "/hello/{name}", |_h, _params, _b| {
            Response::new(200, b"param".to_vec())
        })
        .unwrap();

    let res = router.dispatch(&head("DELETE", "/hello/alice"), &[]);
    assert_eq!(res.status, 405);
    let text = String::from_utf8(res.serialize(false)).unwrap();
    assert!(text.contains("Allow: GET, PUT\r\n"));
}

#[test]
fn unmatched_shape_returns_404() {
    let router = Router::new()
        .route_param("GET", "/hello/{name}", |_h, _params, _b| {
            Response::new(200, b"param".to_vec())
        })
        .unwrap();

    assert_eq!(
        router.dispatch(&head("GET", "/other/path"), &[]).status,
        404
    );
}

#[test]
fn percent_encoded_value_is_passed_through_without_decoding() {
    // 非デコード契約（モジュール doc「マッチング方針」節）。呼び出し側で
    // デコード・再検証する責務であることを end-to-end で固定化する。
    let router = Router::new()
        .route_param("GET", "/files/{name}", |_h, params, _b| {
            Response::new(200, params.get("name").unwrap_or("").as_bytes().to_vec())
        })
        .unwrap();

    let res = router.dispatch(&head("GET", "/files/%2e%2e"), &[]);
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"%2e%2e".to_vec());
}

#[test]
fn registering_pattern_with_bad_syntax_returns_error_not_panic() {
    let err = Router::new().route_param("GET", "hello/{name}", |_h, _params, _b| {
        Response::empty(200)
    });
    assert!(err.is_err());
}

// PR #191 Bugbot 指摘（comment id 3608815178）の回帰テスト。
// 連続スラッシュ・末尾スラッシュによる空セグメントを含むパターンは登録時に
// 拒否されるべきで、受理すると同じ空セグメントが要求されるため通常のパス
// （`/hello/alice` 等）が誤って 404 になっていた。

#[test]
fn registering_pattern_with_consecutive_slash_returns_error_not_panic() {
    let err = Router::new().route_param("GET", "/hello//{name}", |_h, _params, _b| {
        Response::empty(200)
    });
    assert!(err.is_err());
}

#[test]
fn registering_pattern_with_trailing_slash_returns_error_not_panic() {
    let err = Router::new().route_param("GET", "/hello/{name}/", |_h, _params, _b| {
        Response::empty(200)
    });
    assert!(err.is_err());
}

#[test]
fn normal_path_still_matches_after_rejecting_empty_segment_patterns() {
    // 空セグメントを含むパターンの登録が拒否された後も、正規のパターン登録・
    // 通常パスへのマッチングが影響を受けないことを確認する。
    let router = Router::new()
        .route_param("GET", "/hello/{name}", |_h, params, _b| {
            let name = params.get("name").unwrap_or("world");
            Response::new(200, format!("hello, {name}").into_bytes())
        })
        .unwrap();

    let res = router.dispatch(&head("GET", "/hello/alice"), &[]);
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"hello, alice".to_vec());
}

// --- 既存の完全一致ルートとの後方互換テスト ---

#[test]
fn existing_exact_match_routes_are_unaffected_by_param_routes() {
    let router = Router::new()
        .route("GET", "/", |_h, _b| Response::new(200, b"root".to_vec()))
        .route_param("GET", "/hello/{name}", |_h, _params, _b| {
            Response::new(200, b"param".to_vec())
        })
        .unwrap();

    let res = router.dispatch(&head("GET", "/"), &[]);
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"root".to_vec());
}

#[test]
fn literal_braces_in_route_target_remain_exact_match_only() {
    // route() に `{` を含む文字列を渡してもパターン解釈はされず、従来どおり
    // 完全一致のリテラルとして扱われる（route() の意味論を一切変更しない）。
    let router = Router::new().route("GET", "/literal/{not-a-param}", |_h, _b| {
        Response::new(200, b"literal".to_vec())
    });

    let res = router.dispatch(&head("GET", "/literal/{not-a-param}"), &[]);
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"literal".to_vec());

    // 実際のパスセグメント値としての "actual" には一致しない（パターン扱いされていない証跡）。
    let miss = router.dispatch(&head("GET", "/literal/actual"), &[]);
    assert_eq!(miss.status, 404);
}

#[test]
fn non_origin_form_target_does_not_match_param_route() {
    // Cursor Bugbot 指摘（PR #191）: `target` が origin-form（先頭 `/`）でない場合、
    // セグメント数の偶然の一致だけでパラメータルートに一致してはならない
    // （fail-closed・無正規化契約、`pattern` モジュール doc「request_target_segments」参照）。
    let router = Router::new()
        .route_param("GET", "/{name}", |_h, params, _b| {
            Response::new(200, params.get("name").unwrap_or("?").as_bytes().to_vec())
        })
        .unwrap();

    // asterisk-form（OPTIONS * 相当）。1 セグメント相当に見えるが origin-form ではない。
    let asterisk = router.dispatch(&head("GET", "*"), &[]);
    assert_eq!(asterisk.status, 404);

    // 先頭 `/` を欠く不正な request-target。`/{name}` と偶然セグメント数が一致するが
    // origin-form ではないため一致してはならない。
    let no_leading_slash = router.dispatch(&head("GET", "hello"), &[]);
    assert_eq!(no_leading_slash.status, 404);
}

#[test]
fn non_origin_form_target_does_not_match_multi_segment_param_route() {
    let router = Router::new()
        .route_param("GET", "/hello/{name}", |_h, params, _b| {
            Response::new(200, params.get("name").unwrap_or("?").as_bytes().to_vec())
        })
        .unwrap();

    // 先頭 `/` を欠くが `split('/')` すると偶然 2 セグメントになる不正 target。
    let no_leading_slash = router.dispatch(&head("GET", "hello/alice"), &[]);
    assert_eq!(no_leading_slash.status, 404);

    // 正規の origin-form は引き続き一致する（回帰確認）。
    let ok = router.dispatch(&head("GET", "/hello/alice"), &[]);
    assert_eq!(ok.status, 200);
    assert_eq!(ok.body, b"alice".to_vec());
}
