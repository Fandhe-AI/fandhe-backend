//! CORS プラグイン（`cors` feature）の 2 層配線だけを見せる最小サンプル。
//!
//! `crates/core/examples/cors_demo.rs` を土台に、独立して `cargo run` できる
//! standalone crate として `examples/with-cors/` に複製した（Next.js の
//! `examples/` 方式、`examples/README.md` 参照）。CRUD 本体は
//! `GET`/`POST /todos` の 2 ルートのみに絞り、`crates/plugin-cors/src/lib.rs`
//! の crate doc が述べる 2 層構成をそのまま示す:
//!
//! 1. `Router::options_fallback`（イシュー #304）へ
//!    `fandhe_backend_core::plugin_cors::preflight_response` を配線し、プリフライトを
//!    完結させる
//! 2. `Server::cors(config)` を登録し、実リクエストへ CORS ヘッダを付与する
//!    （`crate::plugin::finalize_response`、非公開シーム経由）
//!
//! # 起動方法
//!
//! ```text
//! $ cd examples/with-cors
//! $ cargo run
//! ```
//!
//! 既定で `127.0.0.1:3000` に bind する（`PORT` 環境変数で上書き可能）。
//!
//! # 動作確認手順
//!
//! ```text
//! # プリフライト（204 + Access-Control-Allow-* を確認）
//! $ curl -si -X OPTIONS http://127.0.0.1:3000/todos \
//!     -H 'Origin: https://app.example.com' \
//!     -H 'Access-Control-Request-Method: POST'
//!
//! # 実リクエスト（許可オリジン、Access-Control-Allow-Origin 付与を確認）
//! $ curl -si http://127.0.0.1:3000/todos -H 'Origin: https://app.example.com'
//!
//! # 実リクエスト（不許可オリジン、Access-Control-Allow-Origin なしを確認）
//! $ curl -si http://127.0.0.1:3000/todos -H 'Origin: https://evil.example'
//!
//! # ToDo 作成
//! $ curl -s -X POST http://127.0.0.1:3000/todos \
//!     -H 'Origin: https://app.example.com' -d '{"title":"buy milk"}'
//! ```

use fandhe_backend_core::Server;
use fandhe_backend_core::plugin_cors::{CorsConfig, preflight_response};
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::Router;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Todo {
    id: u64,
    title: String,
}

/// 共有状態。`cors_demo.rs` にはない CRUD の最小共有状態を CORS 配線の外側で
/// 持たせる（ロック保持中の `.await` を避ける、`.claude/rules/coding-rust.md`）。
type Store = Arc<RwLock<Vec<Todo>>>;

#[derive(Debug, Deserialize)]
struct CreateTodoBody {
    title: String,
}

fn error_response(status: u16, message: &str) -> Response {
    let body = serde_json::json!({ "error": message });
    Response::new(
        status,
        serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec()),
    )
    .with_content_type("application/json")
}

/// `https://app.example.com` のみを許可する CORS 設定を組み立てる。
///
/// 実運用では利用者アプリが自身のフロントエンド配信元へ差し替える想定
/// （`CorsConfig::builder` の doc・`crates/core/examples/cors_demo.rs` を参照）。
fn cors_config() -> CorsConfig {
    CorsConfig::builder()
        .allow_origin("https://app.example.com")
        .allow_headers(["Content-Type"])
        .max_age(600)
        .build()
        .expect("固定の許可オリジン設定は allow_any_origin + credentials 併用を含まないため必ず成功する")
}

/// `store` をキャプチャした [`Router`] を組み立てる（`main` とテストの両方から
/// 共有するため関数として切り出す、`templates/app/src/main.rs` と同一パターン）。
/// CORS プリフライト委譲（`options_fallback`）はここで配線する。
/// `Server::cors` 自体の登録は `Router` の責務範囲外のため `main` 側で行う。
fn build_router(store: Store, next_id: Arc<AtomicU64>, cors: CorsConfig) -> Router {
    Router::new()
        .route_async("GET", "/todos", {
            let store = store.clone();
            move |_head, _body| {
                let store = store.clone();
                async move {
                    let todos = store.read().await;
                    let body = serde_json::to_vec(&*todos).unwrap_or_else(|_| b"[]".to_vec());
                    Response::new(200, body).with_content_type("application/json")
                }
            }
        })
        .route_async("POST", "/todos", {
            let store = store.clone();
            let next_id = next_id.clone();
            move |_head, body| {
                let store = store.clone();
                let next_id = next_id.clone();
                let body = body.to_vec();
                async move {
                    let parsed: Result<CreateTodoBody, _> = serde_json::from_slice(&body);
                    let Ok(parsed) = parsed else {
                        return error_response(400, "invalid json body");
                    };
                    let title = parsed.title.trim();
                    if title.is_empty() {
                        return error_response(400, "title must not be blank");
                    }
                    let todo = Todo {
                        id: next_id.fetch_add(1, Ordering::SeqCst),
                        title: title.to_string(),
                    };
                    store.write().await.push(todo.clone());
                    let resp_body = serde_json::to_vec(&todo).unwrap_or_else(|_| b"{}".to_vec());
                    Response::new(201, resp_body).with_content_type("application/json")
                }
            }
        })
        // プリフライト側の配線（1/2）。`Server::cors(config)`（2/2、`main` 側）と
        // 対になる 2 層構成（`crates/plugin-cors/src/lib.rs` の crate doc を参照）。
        .options_fallback(move |head, allow, _body| preflight_response(head, allow, &cors))
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> std::io::Result<()> {
    let store: Store = Arc::new(RwLock::new(Vec::new()));
    let next_id = Arc::new(AtomicU64::new(1));
    let cors = cors_config();

    let router = build_router(store, next_id, cors.clone());
    let server = Server::new()
        .handler(router)
        // 実リクエスト側の CORS 配線（2/2）。未登録時は feature 有効でも
        // フォールスルーする（`Server::cors` の doc を参照）。
        .cors(cors);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("127.0.0.1:{port}");
    let bound = server.bind(&addr).await?;
    println!("fandhe-backend-example-with-cors listening on {addr}");
    bound
        .run_until(async {
            // 登録失敗を握りつぶすと future が即完了し bind 直後にサーバが
            // 終了してしまうため、シグナルハンドラを登録できない環境では
            // 起動継続せず明示的に panic させる（graceful-shutdown ガイドと同方針）
            tokio::signal::ctrl_c()
                .await
                .expect("Ctrl-C シグナルハンドラの登録に失敗した");
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};

    fn head_of(raw: &str) -> RequestHead {
        match parse_request_head(raw.as_bytes()).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("incomplete request head in test fixture"),
        }
    }

    fn new_router() -> Router {
        build_router(
            Arc::new(RwLock::new(Vec::new())),
            Arc::new(AtomicU64::new(1)),
            cors_config(),
        )
    }

    #[tokio::test]
    async fn cors_preflight_allowed_origin_returns_204_with_allow_origin_header() {
        let router = new_router();
        let head = head_of(
            "OPTIONS /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccess-Control-Request-Method: POST\r\n\r\n",
        );
        let res = router.dispatch(&head, b"").await;
        assert_eq!(res.status, 204);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Access-Control-Allow-Origin: https://app.example.com\r\n"));
    }

    #[tokio::test]
    async fn cors_preflight_disallowed_origin_returns_403_without_allow_origin_header() {
        let router = new_router();
        let head = head_of(
            "OPTIONS /todos HTTP/1.1\r\nOrigin: https://evil.example\r\nAccess-Control-Request-Method: POST\r\n\r\n",
        );
        let res = router.dispatch(&head, b"").await;
        assert_eq!(res.status, 403);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(!text.contains("Access-Control-Allow-Origin"));
    }

    #[tokio::test]
    async fn crud_roundtrip_via_async_handlers() {
        let router = new_router();

        let res = router
            .dispatch(
                &head_of("POST /todos HTTP/1.1\r\n\r\n"),
                br#"{"title":"buy milk"}"#,
            )
            .await;
        assert_eq!(res.status, 201);
        let created: Todo = serde_json::from_slice(&res.body).unwrap();
        assert_eq!(created.title, "buy milk");

        let res = router
            .dispatch(&head_of("GET /todos HTTP/1.1\r\n\r\n"), b"")
            .await;
        assert_eq!(res.status, 200);
        let list: Vec<Todo> = serde_json::from_slice(&res.body).unwrap();
        assert_eq!(list.len(), 1);
    }
}
