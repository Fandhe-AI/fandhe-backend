//! fandhe-backend の実運用形サンプル ToDo API テンプレート。
//!
//! `crates/core/examples/todo_async.rs`（CRUD 本体）を土台に、
//! `cors` / `compression` / `static` / `openapi` の 4 feature を 1 プロジェクトへ
//! 組み合わせて配線する。個々の feature は各 crate の example
//! （`cors_demo.rs` / `compression_demo.rs` / `static_demo.rs` /
//! `openapi_custom_doc.rs`）が単独で示すが、実運用アプリでは複数 feature を
//! 同時に組み合わせる必要があり、配線順序（CORS プリフライト →
//! 実リクエスト CORS → 圧縮 → 静的配信 → OpenAPI → 404 fallback →
//! graceful shutdown）にドリフトが生じやすい。本テンプレートはその配線の
//! 雛形として `templates/app/` に置き、フレームワーク利用者が
//! `cargo new` 相当の出発点として複製・改変できるようにする（root workspace
//! から独立した standalone crate、`templates/app/Cargo.toml` の doc を参照）。
//!
//! # 使い方
//!
//! ```text
//! $ cd templates/app
//! $ cargo run
//! ```
//!
//! 起動後、ブラウザで <http://127.0.0.1:3000/index.html> を開くと素の
//! HTML+JS の ToDo UI が表示される（`templates/app/static/index.html`、
//! `static` feature 経由で配信）。
//!
//! # 組み込んだ feature
//!
//! - CRUD 本体: `route_async` / `route_param_async`（`Arc<RwLock<...>>` 共有状態）
//! - `cors`: `Router::options_fallback` + `Server::cors` の 2 層構成
//! - `compression`: `Server::compression`（gzip、既定しきい値）
//! - `static`: `Server::static_files`（`static/index.html` を配信）
//! - `openapi`: `Server::openapi_with`（手書き `openapi.json` を配信）
//! - 404 fallback: `Router::fallback`（JSON エラーボディ）
//! - graceful shutdown: `BoundServer::run_until` + `Server::shutdown_grace_period`
//!
//! # 動作確認手順
//!
//! ```text
//! $ curl -s -X POST http://127.0.0.1:3000/todos -d '{"title":"buy milk"}'
//! $ curl -s http://127.0.0.1:3000/todos
//! $ curl -s http://127.0.0.1:3000/openapi.json
//! $ curl -si http://127.0.0.1:3000/nope   # 404 + JSON エラーボディ
//! $ curl -si -X OPTIONS http://127.0.0.1:3000/todos \
//!     -H 'Origin: http://localhost:5173' \
//!     -H 'Access-Control-Request-Method: POST'
//! ```

use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_cors::{CorsConfig, preflight_response};
use fandhe_backend_routes::Router;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Todo {
    id: u64,
    title: String,
    done: bool,
}

/// 共有状態。CRUD ハンドラは `Arc<RwLock<Store>>` を `.clone()` して
/// キャプチャし、ロック保持区間を 1 回の読み取り／書き込み操作のみに
/// 区切る（`todo_async.rs` と同一方針、`.claude/rules/coding-rust.md`
/// の「ロック保持中の `.await` を避ける」規約）。
#[derive(Default)]
struct Store {
    todos: BTreeMap<u64, Todo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ErrorBody {
    error: String,
}

fn error_response(status: u16, message: &str) -> Response {
    let body = serde_json::to_vec(&ErrorBody {
        error: message.to_string(),
    })
    .unwrap_or_else(|_| b"{}".to_vec());
    Response::new(status, body).with_content_type("application/json")
}

fn json_response(status: u16, payload: &impl Serialize) -> Response {
    let body = serde_json::to_vec(payload).unwrap_or_else(|_| b"{}".to_vec());
    Response::new(status, body).with_content_type("application/json")
}

fn parse_id(id_str: &str) -> Option<u64> {
    id_str.parse::<u64>().ok()
}

#[derive(Debug, Deserialize)]
struct CreateTodoBody {
    title: String,
}

#[derive(Debug, Default, Deserialize)]
struct UpdateTodoBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    done: Option<bool>,
}

/// テンプレート開発向けの CORS 許可設定（開発サーバの典型ポート）。
///
/// 実運用では利用者アプリが自身のフロントエンド配信元へ差し替える想定
/// （`CorsConfig::builder` の doc・`crates/core/examples/cors_demo.rs` を参照）。
fn cors_config() -> CorsConfig {
    CorsConfig::builder()
        .allow_origin("http://localhost:5173")
        .allow_methods(["GET", "POST", "PATCH", "DELETE"])
        .allow_headers(["Content-Type"])
        .max_age(600)
        .build()
        .expect("固定の許可オリジン設定は allow_any_origin + credentials 併用を含まないため必ず成功する")
}

/// `store` / `next_id` をキャプチャした [`Router`] を組み立てる（`main` と
/// テストの両方から共有するため関数として切り出す、`todo_async.rs` と
/// 同一パターン）。CORS プリフライト委譲（`options_fallback`）・404
/// fallback もここで配線する（`Server::cors` 自体の登録・`static`・
/// `compression`・`openapi` は `main` 側、`Router` の責務範囲外のため）。
fn build_router(store: Arc<RwLock<Store>>, next_id: Arc<AtomicU64>, cors: CorsConfig) -> Router {
    Router::new()
        .route_async("GET", "/todos", {
            let store = store.clone();
            move |_head, _body| {
                let store = store.clone();
                async move {
                    let store = store.read().await;
                    let todos: Vec<&Todo> = store.todos.values().collect();
                    json_response(200, &todos)
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
                    let id = next_id.fetch_add(1, Ordering::SeqCst);
                    let todo = Todo {
                        id,
                        title: title.to_string(),
                        done: false,
                    };
                    {
                        let mut store = store.write().await;
                        store.todos.insert(id, todo.clone());
                    }
                    json_response(201, &todo)
                }
            }
        })
        .route_param_async("GET", "/todos/{id}", {
            let store = store.clone();
            move |_head, params, _body| {
                let store = store.clone();
                let id_str = params.get("id").unwrap_or("").to_string();
                async move {
                    let Some(id) = parse_id(&id_str) else {
                        return error_response(404, "todo not found");
                    };
                    let store = store.read().await;
                    match store.todos.get(&id) {
                        Some(todo) => json_response(200, todo),
                        None => error_response(404, "todo not found"),
                    }
                }
            }
        })
        .unwrap()
        .route_param_async("PATCH", "/todos/{id}", {
            let store = store.clone();
            move |_head, params, body| {
                let store = store.clone();
                let id_str = params.get("id").unwrap_or("").to_string();
                let body = body.to_vec();
                async move {
                    let Some(id) = parse_id(&id_str) else {
                        return error_response(404, "todo not found");
                    };
                    let parsed: Result<UpdateTodoBody, _> = if body.is_empty() {
                        Ok(UpdateTodoBody::default())
                    } else {
                        serde_json::from_slice(&body)
                    };
                    let Ok(parsed) = parsed else {
                        return error_response(400, "invalid json body");
                    };
                    if let Some(title) = &parsed.title
                        && title.trim().is_empty()
                    {
                        return error_response(400, "title must not be blank");
                    }

                    let mut store = store.write().await;
                    let Some(todo) = store.todos.get_mut(&id) else {
                        return error_response(404, "todo not found");
                    };
                    if let Some(title) = parsed.title {
                        todo.title = title.trim().to_string();
                    }
                    if let Some(done) = parsed.done {
                        todo.done = done;
                    }
                    let updated = todo.clone();
                    drop(store);
                    json_response(200, &updated)
                }
            }
        })
        .unwrap()
        .route_param_async("DELETE", "/todos/{id}", {
            let store = store.clone();
            move |_head, params, _body| {
                let store = store.clone();
                let id_str = params.get("id").unwrap_or("").to_string();
                async move {
                    let Some(id) = parse_id(&id_str) else {
                        return error_response(404, "todo not found");
                    };
                    let mut store = store.write().await;
                    match store.todos.remove(&id) {
                        Some(_) => Response::empty(204),
                        None => error_response(404, "todo not found"),
                    }
                }
            }
        })
        .unwrap()
        // CORS プリフライト側の配線（1/2）。`Server::cors(config)`（2/2、`main`
        // 側）と対になる 2 層構成（`crates/plugin-cors/src/lib.rs` の crate doc・
        // `crates/core/examples/cors_demo.rs` を参照）。
        .options_fallback(move |head, allow, _body| preflight_response(head, allow, &cors))
        // 静的・パラメータいずれのルートにも一致しなかったリクエストの共通
        // 404（イシュー #316）。`static` の配信対象パス（`/index.html`）は
        // `try_intercept` が `Router::dispatch` より先に処理するため、ここには
        // 到達しない（`crates/core/src/server.rs` の処理フロー doc を参照）。
        .fallback(|_head, _body| error_response(404, "not found"))
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> std::io::Result<()> {
    let store: Arc<RwLock<Store>> = Arc::new(RwLock::new(Store::default()));
    let next_id = Arc::new(AtomicU64::new(1));
    let cors = cors_config();

    let router = build_router(store, next_id, cors.clone());

    // 静的ファイル配信: mount をルート `"/"` にはしない。`try_intercept`
    // （静的配信を含むパスインターセプト型プラグイン）は `Router::dispatch`
    // より先に評価されるため、mount `"/"` は全 GET パスに一致してしまい
    // `GET /todos` 等の CRUD API を静的配信が横取りしてしまう
    // （`crates/plugin-static/src/lib.rs` の `strip_mount` doc を参照）。
    // mount をファイルパスそのもの（`"/index.html"`）にすることで、
    // 一覧ページ 1 ファイルのみを配信対象に限定しつつ、README・本 doc が
    // 案内する URL とも一致させる。
    let static_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static");
    let static_config =
        fandhe_backend_plugin_static::StaticFilesConfig::builder("/index.html", &static_root)
            .build()
            .expect("templates/app/static/ はリポジトリに同梱されているため構築に成功する");

    let openapi_doc =
        fandhe_backend_plugin_openapi::OpenApiDoc::from_json(include_str!("../openapi.json"))
            .expect("templates/app/openapi.json は手書き検証済みの妥当な JSON オブジェクト");

    let server = Server::new()
        .handler(router)
        // 実リクエスト側の CORS 配線（2/2）。未登録時は feature 有効でも
        // フォールスルーする（`Server::cors` の doc を参照）。
        .cors(cors)
        // CORS の後、body を確定させる最後の後処理として圧縮を適用する
        // （`Server::compression` の doc・`crates/plugin-compression` の
        // crate doc「圧縮は必ず最後」を参照）。既定しきい値のまま使う。
        .compression(fandhe_backend_plugin_compression::CompressionConfig::builder().build())
        .static_files(static_config)
        .openapi_with(openapi_doc)
        // graceful shutdown の待機上限（既定 30 秒から本テンプレートでは
        // 明示的に 10 秒へ短縮し、開発時の Ctrl-C 応答性を優先する）。
        .shutdown_grace_period(std::time::Duration::from_secs(10));

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("127.0.0.1:{port}");
    let bound = server.bind(&addr).await?;
    println!("fandhe-backend-template-app listening on {addr}");
    bound
        .run_until(async {
            let _ = tokio::signal::ctrl_c().await;
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
            Arc::new(RwLock::new(Store::default())),
            Arc::new(AtomicU64::new(1)),
            cors_config(),
        )
    }

    #[test]
    fn bundled_openapi_doc_is_valid_json() {
        // openapi.json の手書きミスは main() の実行時 panic ではなく
        // `cargo test` の失敗として検出したいための最小ガード
        // （`OpenApiDoc::from_json` は構文検証 + トップレベルオブジェクト
        // 検証のみ行う、`crates/plugin-openapi/src/custom.rs` の doc を参照）。
        let doc =
            fandhe_backend_plugin_openapi::OpenApiDoc::from_json(include_str!("../openapi.json"));
        assert!(doc.is_ok());
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
        assert!(!created.done);

        let res = router
            .dispatch(&head_of("GET /todos HTTP/1.1\r\n\r\n"), b"")
            .await;
        assert_eq!(res.status, 200);
        let list: Vec<Todo> = serde_json::from_slice(&res.body).unwrap();
        assert_eq!(list.len(), 1);

        let path = format!("GET /todos/{} HTTP/1.1\r\n\r\n", created.id);
        let res = router.dispatch(&head_of(&path), b"").await;
        assert_eq!(res.status, 200);

        let path = format!("PATCH /todos/{} HTTP/1.1\r\n\r\n", created.id);
        let res = router.dispatch(&head_of(&path), br#"{"done":true}"#).await;
        assert_eq!(res.status, 200);
        let updated: Todo = serde_json::from_slice(&res.body).unwrap();
        assert!(updated.done);

        let path = format!("DELETE /todos/{} HTTP/1.1\r\n\r\n", created.id);
        let res = router.dispatch(&head_of(&path), b"").await;
        assert_eq!(res.status, 204);

        let path = format!("GET /todos/{} HTTP/1.1\r\n\r\n", created.id);
        let res = router.dispatch(&head_of(&path), b"").await;
        assert_eq!(res.status, 404);
    }

    #[tokio::test]
    async fn cors_preflight_allowed_origin_returns_204_with_allow_origin_header() {
        let router = new_router();
        let head = head_of(
            "OPTIONS /todos HTTP/1.1\r\nOrigin: http://localhost:5173\r\nAccess-Control-Request-Method: POST\r\n\r\n",
        );
        let res = router.dispatch(&head, b"").await;
        assert_eq!(res.status, 204);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Access-Control-Allow-Origin: http://localhost:5173\r\n"));
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
    async fn unknown_path_returns_json_404_fallback() {
        let router = new_router();
        let res = router
            .dispatch(&head_of("GET /nope HTTP/1.1\r\n\r\n"), b"")
            .await;
        assert_eq!(res.status, 404);
        let body: ErrorBody = serde_json::from_slice(&res.body).unwrap();
        assert_eq!(body.error, "not found");
    }
}
