//! REQ-3 対象 5 エンドポイントの実サービング（イシュー #257）。
//!
//! # このバイナリの役割
//!
//! `crates/plugin-openapi/src/docs.rs` の `ApiDoc` が宣言する 5 エンドポイント
//! （`GET /health` / `GET /hello/{name}` / `GET /users/{id}` / `POST /echo` /
//! `GET /search`）を `fandhe_backend_routes::Router` に実登録し、
//! `docs/acceptance/req3-openapi-generation.md` の手動突合表を BLOCKED から
//! 突合可能な状態にする。`GET /hello/{name}` `GET /users/{id}` は #176 で追加
//! された `Router::route_param` を使い、`GET /search` は #258 で分離された
//! `RequestHead::path()` / `RequestHead::query()` を使う。
//!
//! # ApiDoc との整合
//!
//! レスポンス構造は `ApiDoc` の `#[utoipa::path]` 宣言・`crates/plugin-openapi/
//! src/schemas.rs` のスキーマ型と一致させる（`EchoBody` / `UserResponse` /
//! `SearchResponse` / `ErrorBody`、フィールド構成のみ本ファイル内に複製し、
//! `crates/plugin-openapi` への依存は追加しない。`crates/core` はコアクレート
//! であり、下流の `plugin-openapi` に依存すると依存方向 `server → routes →
//! http::*`（`crates/routes/src/lib.rs` doc 参照）を逆流させてしまうため）。
//!
//! # pay-for-what-you-use（`.claude/rules/pay-for-what-you-use.md`）
//!
//! JSON 処理（`/users/{id}` 応答・`/echo` の JSON パース）には
//! `crates/core/Cargo.toml` の `[dev-dependencies]` に既存の serde/serde_json
//! （`examples/core-bench.rs`、TASK-1.6-3 / #168 で導入済み）をそのまま再利用する。
//! 新規依存の追加は行わない。example は `[dev-dependencies]` のみを参照する
//! ため、本体（lib）の依存グラフ・下流クレートには一切波及しない
//! （`cargo tree -p fandhe-backend-core -e normal` に現れないことで検証可能）。
//! `GET /search` のクエリ文字列解析は `key=value` の `&` 区切りのみを扱う
//! 手書きの最小パーサとし、% デコードは行わない（`crates/routes` の
//! 「パーサが渡したバイト列をそのまま比較する」既存方針を踏襲、
//! `crates/routes/src/lib.rs` モジュール doc「マッチング方針」節）。
//!
//! # 動作確認手順
//!
//! ```text
//! $ cargo run --example openapi_endpoints -p fandhe-backend-core
//! $ curl -v http://127.0.0.1:3003/health
//! $ curl -v http://127.0.0.1:3003/hello/world
//! $ curl -v http://127.0.0.1:3003/users/42
//! $ curl -v http://127.0.0.1:3003/users/abc          # 400
//! $ curl -v -X POST http://127.0.0.1:3003/echo -d '{"message":"hi"}'
//! $ curl -v -X POST http://127.0.0.1:3003/echo -d 'not json'   # 400
//! $ curl -v 'http://127.0.0.1:3003/search?q=rust&limit=5'
//! $ curl -v 'http://127.0.0.1:3003/search'           # 400（q 欠落）
//! $ curl -v 'http://127.0.0.1:3003/search?q=rust&limit=abc'  # 400（limit 不正）
//! ```

use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::{PathParams, Router};
use serde::{Deserialize, Serialize};

/// `POST /echo` のリクエスト/レスポンス body。
/// `crates/plugin-openapi/src/schemas.rs::EchoBody` と同一フィールド構成。
#[derive(Debug, Serialize, Deserialize)]
struct EchoBody {
    message: String,
}

/// `GET /users/{id}` の正常応答 body。
/// `crates/plugin-openapi/src/schemas.rs::UserResponse` と同一フィールド構成。
#[derive(Debug, Serialize)]
struct UserResponse {
    id: u64,
    name: String,
}

/// `GET /search` の正常応答 body。
/// `crates/plugin-openapi/src/schemas.rs::SearchResponse` と同一フィールド構成。
#[derive(Debug, Serialize)]
struct SearchResponse {
    query: String,
    limit: u32,
    results: Vec<String>,
}

/// 400 応答共通 body。
/// `crates/plugin-openapi/src/schemas.rs::ErrorBody` と同一フィールド構成。
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

/// `payload` を JSON へシリアライズして `status` の応答を組み立てる。
///
/// `serde_json::to_vec` は `Vec<u8>` writer に対して失敗しないため
/// （writer エラーのみが失敗要因）、`unwrap_or_else` でフォールバックし
/// ライブラリ境界を越えて panic させない（`.claude/rules/coding-rust.md`）。
fn json_response(status: u16, payload: &impl Serialize) -> Response {
    let body = serde_json::to_vec(payload).unwrap_or_else(|_| b"{}".to_vec());
    Response::new(status, body).with_content_type("application/json")
}

/// `GET /health` ハンドラ。ApiDoc 宣言どおり固定文字列 `OK` を返す。
///
/// `crates/plugin-openapi/openapi.json` の `/health` 200 応答は
/// `body = String` から utoipa が `text/plain` として生成しているため、
/// `Content-Type` を明示して ApiDoc との齟齬をなくす。
fn health(_head: &RequestHead, _body: &[u8]) -> Response {
    Response::new(200, b"OK".to_vec()).with_content_type("text/plain")
}

/// `GET /hello/{name}` ハンドラ。パスパラメータ `name` を挨拶文へ埋め込む。
///
/// `health` と同様、ApiDoc の `body = String` 宣言（`text/plain`）に
/// `Content-Type` を合わせる。
fn hello(_head: &RequestHead, params: &PathParams<'_>, _body: &[u8]) -> Response {
    let name = params.get("name").unwrap_or("world");
    Response::new(200, format!("Hello, {name}!").into_bytes()).with_content_type("text/plain")
}

/// `GET /users/{id}` ハンドラ本体（テスト可能な形に切り出し）。
///
/// `id_str` が非負整数としてパース可能なら 200 + [`UserResponse`]、
/// そうでなければ 400 + [`ErrorBody`]（ApiDoc `users_doc` の 200/400 定義に対応）。
fn users_by_id(id_str: &str) -> Response {
    match id_str.parse::<u64>() {
        Ok(id) => json_response(
            200,
            &UserResponse {
                id,
                name: format!("User {id}"),
            },
        ),
        Err(_) => json_response(
            400,
            &ErrorBody {
                error: "invalid id".to_string(),
            },
        ),
    }
}

/// `GET /users/{id}` ハンドラ。
fn users(_head: &RequestHead, params: &PathParams<'_>, _body: &[u8]) -> Response {
    users_by_id(params.get("id").unwrap_or(""))
}

/// `POST /echo` ハンドラ本体（テスト可能な形に切り出し）。
///
/// body が [`EchoBody`] として妥当な JSON なら 200 でそのまま再シリアライズして
/// 返し、不正なら 400 + [`ErrorBody`]（ApiDoc `echo_doc` の 200/400 定義に対応）。
/// 受信 body を生転写せず serde_json 経由で再シリアライズする
/// （レスポンス分割・生エコーによるインジェクション回避、`.claude/rules/security.md`）。
fn echo_body(body: &[u8]) -> Response {
    match serde_json::from_slice::<EchoBody>(body) {
        Ok(payload) => json_response(200, &payload),
        Err(_) => json_response(
            400,
            &ErrorBody {
                error: "invalid json body".to_string(),
            },
        ),
    }
}

/// `POST /echo` ハンドラ。
fn echo(_head: &RequestHead, body: &[u8]) -> Response {
    echo_body(body)
}

/// クエリ文字列（`key=value` を `&` で連結した生文字列、% デコードなし）から
/// `key` に対応する値を返す。同一キーが複数回出現する場合は最初の一致を返す。
/// 値を持たない `key`（`key` のみ・`key=` いずれも）は空文字列として扱う。
fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key { Some(v) } else { None }
    })
}

/// `GET /search` ハンドラ本体（テスト可能な形に切り出し）。
///
/// `query`（`RequestHead::query()` が返す生のクエリ文字列）から `q`（必須）・
/// `limit`（任意、既定 10）を取り出す。`q` 欠落、または `limit` が非負整数として
/// パースできない場合は 400 + [`ErrorBody`]（ApiDoc `search_doc` の 200/400 定義
/// に対応）。
fn search_query(query: Option<&str>) -> Response {
    let query = query.unwrap_or("");
    let Some(q) = query_param(query, "q").filter(|v| !v.is_empty()) else {
        return json_response(
            400,
            &ErrorBody {
                error: "missing required query parameter 'q'".to_string(),
            },
        );
    };

    let limit = match query_param(query, "limit") {
        None => 10,
        Some(raw) => match raw.parse::<u32>() {
            Ok(limit) => limit,
            Err(_) => {
                return json_response(
                    400,
                    &ErrorBody {
                        error: "invalid 'limit' query parameter".to_string(),
                    },
                );
            }
        },
    };

    json_response(
        200,
        &SearchResponse {
            query: q.to_string(),
            limit,
            results: vec![format!("{q}-result-0")],
        },
    )
}

/// `GET /search` ハンドラ。
fn search(head: &RequestHead, _body: &[u8]) -> Response {
    search_query(head.query())
}

/// ApiDoc 5 エンドポイントを登録した [`Router`] を組み立てる。
///
/// `route_param` はパターン不正時のみ `Err` を返す契約（`crates/routes/src/
/// lib.rs::Router::route_param` doc 参照）。本関数で登録するパターン
/// （`/hello/{name}` `/users/{id}`）は静的リテラルであり、常に `Ok` になる
/// ため `expect` で早期に検証する（起動時のみ到達する経路であり、ライブラリ
/// コードの実行時 panic 回避対象ではない）。
fn build_router() -> Router {
    Router::new()
        .route("GET", "/health", health)
        .route_param("GET", "/hello/{name}", hello)
        .expect("static route_param pattern '/hello/{name}' must be valid")
        .route_param("GET", "/users/{id}", users)
        .expect("static route_param pattern '/users/{id}' must be valid")
        .route("POST", "/echo", echo)
        .route("GET", "/search", search)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3003".to_string());
    let server = fandhe_backend_core::Server::new().handler(build_router());
    // `openapi` feature 有効時は `GET /openapi.json` の静的サービングも登録する
    // （`Server::openapi()`、TASK-2.1 / #256 の opt-in 契約）。これにより本 example
    // 単体で REQ-3 基準 4（OpenAPI 生成有無での `GET /health` 性能 A/B 比較、#259）の
    // 「生成有効」構成（`--features openapi`）と「無効」構成（feature なし）を
    // 同一ソースから作り分けられる。feature 無効時は本行ごとコンパイルから消え、
    // 依存・コードとも残らない（`.claude/rules/pay-for-what-you-use.md`）。
    #[cfg(feature = "openapi")]
    let server = server.openapi();
    let bound = server.bind(&addr).await?;
    println!("openapi_endpoints listening on {}", bound.local_addr()?);
    bound.run().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    /// テスト専用: 生の HTTP リクエストヘッドをパースして `RequestHead` を得る
    /// （`crates/core/examples/core-bench.rs` の同名ヘルパーと同一パターン）。
    fn head_of(raw: &str) -> RequestHead {
        match parse_request_head(raw.as_bytes()).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("incomplete request head in test fixture"),
        }
    }

    fn body_str(response: &Response) -> String {
        String::from_utf8(response.body.clone()).unwrap()
    }

    fn body_json(response: &Response) -> serde_json::Value {
        serde_json::from_slice(&response.body).unwrap()
    }

    // --- GET /health ---

    #[test]
    fn health_returns_200_ok() {
        let router = build_router();
        let res = router.dispatch(&head_of("GET /health HTTP/1.1\r\n\r\n"), b"");
        assert_eq!(res.status, 200);
        assert_eq!(body_str(&res), "OK");
        // ApiDoc（`body = String`）は `text/plain` としてレンダリングされる
        // （`crates/plugin-openapi/openapi.json` の `/health` 200 応答で確認済み）。
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains("Content-Type: text/plain\r\n"));
    }

    #[test]
    fn health_wrong_method_returns_405() {
        let router = build_router();
        let res = router.dispatch(&head_of("POST /health HTTP/1.1\r\n\r\n"), b"");
        assert_eq!(res.status, 405);
    }

    // --- GET /hello/{name} ---

    #[test]
    fn hello_returns_greeting() {
        let router = build_router();
        let res = router.dispatch(&head_of("GET /hello/alice HTTP/1.1\r\n\r\n"), b"");
        assert_eq!(res.status, 200);
        assert_eq!(body_str(&res), "Hello, alice!");
        // ApiDoc（`body = String`）は `text/plain` としてレンダリングされる
        // （`crates/plugin-openapi/openapi.json` の `/hello/{name}` 200 応答で確認済み）。
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains("Content-Type: text/plain\r\n"));
    }

    // --- GET /users/{id} ---

    #[test]
    fn users_valid_id_returns_200_json() {
        let router = build_router();
        let res = router.dispatch(&head_of("GET /users/42 HTTP/1.1\r\n\r\n"), b"");
        assert_eq!(res.status, 200);
        let parsed = body_json(&res);
        assert_eq!(parsed["id"], 42);
        assert_eq!(parsed["name"], "User 42");
    }

    #[test]
    fn users_invalid_id_returns_400_json() {
        let router = build_router();
        let res = router.dispatch(&head_of("GET /users/abc HTTP/1.1\r\n\r\n"), b"");
        assert_eq!(res.status, 400);
        let parsed = body_json(&res);
        assert_eq!(parsed["error"], "invalid id");
    }

    #[test]
    fn users_by_id_directly_rejects_non_numeric() {
        // ハンドラ本体（テスト可能な形に切り出した関数）の直接検証。
        let res = users_by_id("-1");
        assert_eq!(res.status, 400);
    }

    // --- POST /echo ---

    #[test]
    fn echo_valid_json_roundtrips() {
        let router = build_router();
        let res = router.dispatch(
            &head_of("POST /echo HTTP/1.1\r\n\r\n"),
            br#"{"message":"hi"}"#,
        );
        assert_eq!(res.status, 200);
        let parsed = body_json(&res);
        assert_eq!(parsed["message"], "hi");
    }

    #[test]
    fn echo_invalid_json_returns_400() {
        let router = build_router();
        let res = router.dispatch(&head_of("POST /echo HTTP/1.1\r\n\r\n"), b"not json");
        assert_eq!(res.status, 400);
        let parsed = body_json(&res);
        assert_eq!(parsed["error"], "invalid json body");
    }

    #[test]
    fn echo_wrong_method_returns_405() {
        let router = build_router();
        let res = router.dispatch(&head_of("GET /echo HTTP/1.1\r\n\r\n"), b"");
        assert_eq!(res.status, 405);
    }

    // --- GET /search ---

    #[test]
    fn search_returns_200_with_query_and_limit() {
        let router = build_router();
        let res = router.dispatch(&head_of("GET /search?q=rust&limit=5 HTTP/1.1\r\n\r\n"), b"");
        assert_eq!(res.status, 200);
        let parsed = body_json(&res);
        assert_eq!(parsed["query"], "rust");
        assert_eq!(parsed["limit"], 5);
    }

    #[test]
    fn search_missing_q_returns_400() {
        let router = build_router();
        let res = router.dispatch(&head_of("GET /search HTTP/1.1\r\n\r\n"), b"");
        assert_eq!(res.status, 400);
    }

    #[test]
    fn search_missing_q_with_limit_only_returns_400() {
        let router = build_router();
        let res = router.dispatch(&head_of("GET /search?limit=5 HTTP/1.1\r\n\r\n"), b"");
        assert_eq!(res.status, 400);
    }

    #[test]
    fn search_invalid_limit_returns_400() {
        let router = build_router();
        let res = router.dispatch(
            &head_of("GET /search?q=rust&limit=abc HTTP/1.1\r\n\r\n"),
            b"",
        );
        assert_eq!(res.status, 400);
    }

    #[test]
    fn search_default_limit_is_ten_when_omitted() {
        let router = build_router();
        let res = router.dispatch(&head_of("GET /search?q=rust HTTP/1.1\r\n\r\n"), b"");
        assert_eq!(res.status, 200);
        let parsed = body_json(&res);
        assert_eq!(parsed["limit"], 10);
    }

    #[test]
    fn search_unknown_path_still_returns_404() {
        let router = build_router();
        let res = router.dispatch(&head_of("GET /missing HTTP/1.1\r\n\r\n"), b"");
        assert_eq!(res.status, 404);
    }
}
