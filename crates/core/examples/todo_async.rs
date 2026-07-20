//! async ハンドラ（イシュー #315、`docs/design/async-handler.md`）の実利用例。
//!
//! `_/todo-backend` 相当の最小 todo API を、`Router::route_async` /
//! `Router::route_param_async` で構成する。状態は `Arc<tokio::sync::RwLock<...>>`
//! で共有し、ハンドラ内で `lock().await` する構成を取ることで「ハンドラ本体で
//! 非同期 I/O を直接 `.await` できる」という async ハンドラ契約の実利用パターンを
//! 示す（本 example 自体は実 DB を持たないインメモリ実装だが、`sqlx` 等の非同期
//! DB クライアントに置き換える際の構造は変わらない）。
//!
//! # エンドポイント
//!
//! - `GET /todos`        一覧取得
//! - `GET /todos/{id}`   単体取得（404: 不在 / 非数値 id）
//! - `POST /todos`       作成（`{"title": "..."}`、400: 不正 JSON・空白のみ title）
//! - `PATCH /todos/{id}` 更新（`{"title"?: "...", "done"?: bool}`）
//! - `DELETE /todos/{id}` 削除（404: 不在）
//!
//! # 動作確認手順
//!
//! ```text
//! $ cargo run --example todo_async -p fandhe-backend-core
//! $ curl -s -X POST http://127.0.0.1:3003/todos -d '{"title":"buy milk"}'
//! $ curl -s http://127.0.0.1:3003/todos
//! $ curl -s http://127.0.0.1:3003/todos/1
//! $ curl -s -X PATCH http://127.0.0.1:3003/todos/1 -d '{"done":true}'
//! $ curl -s -X DELETE http://127.0.0.1:3003/todos/1
//! $ curl -s -X POST http://127.0.0.1:3003/todos -d '{"title":"   "}'   # 400
//! $ curl -s http://127.0.0.1:3003/todos/abc                            # 404
//! ```

use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
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

/// 共有状態。`Arc` で複数コネクションタスク間に共有し、`RwLock` で読み書きを
/// 調停する（async ハンドラ内で `.await` する非同期ロック）。各ハンドラは
/// ロック保持区間を最小限（1 回の読み取り／書き込み操作のみ）に区切り、
/// ロックを保持したまま別の `.await` を挟まない
/// （`.claude/rules/coding-rust.md` の「ロック保持中の `.await` を避ける」規約）。
#[derive(Default)]
struct Store {
    todos: BTreeMap<u64, Todo>,
}

#[derive(Debug, Serialize)]
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

/// `path` の末尾セグメントを `u64` としてパースする。非数値・不在は `None`
/// （呼び出し元が 404 を返す、入力検証はフェイルクローズ、`.claude/rules/security.md`）。
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let store: Arc<RwLock<Store>> = Arc::new(RwLock::new(Store::default()));
    let next_id = Arc::new(AtomicU64::new(1));

    let router = build_router(store, next_id);

    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3003".to_string());
    let server = Server::new().handler(router);
    let bound = server.bind(&addr).await?;
    println!("todo-async listening on {}", bound.local_addr()?);
    bound.run().await
}

/// `store` / `next_id` をキャプチャした `Router` を組み立てる（`main` と
/// テストの両方から共有するため関数として切り出す）。
fn build_router(store: Arc<RwLock<Store>>, next_id: Arc<AtomicU64>) -> Router {
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
                    // ロック保持中に await しないよう、書き込みスコープを
                    // 最小限に区切る（`.claude/rules/coding-rust.md`）。
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    fn head_of(raw: &str) -> fandhe_backend_http::request::RequestHead {
        match parse_request_head(raw.as_bytes()).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("incomplete request head in test fixture"),
        }
    }

    fn new_router() -> Router {
        build_router(
            Arc::new(RwLock::new(Store::default())),
            Arc::new(AtomicU64::new(1)),
        )
    }

    #[tokio::test]
    async fn crud_roundtrip_via_async_handlers() {
        let router = new_router();

        // 作成
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

        // 一覧
        let res = router
            .dispatch(&head_of("GET /todos HTTP/1.1\r\n\r\n"), b"")
            .await;
        assert_eq!(res.status, 200);
        let list: Vec<Todo> = serde_json::from_slice(&res.body).unwrap();
        assert_eq!(list.len(), 1);

        // 単体取得
        let path = format!("GET /todos/{} HTTP/1.1\r\n\r\n", created.id);
        let res = router.dispatch(&head_of(&path), b"").await;
        assert_eq!(res.status, 200);

        // 更新
        let path = format!("PATCH /todos/{} HTTP/1.1\r\n\r\n", created.id);
        let res = router.dispatch(&head_of(&path), br#"{"done":true}"#).await;
        assert_eq!(res.status, 200);
        let updated: Todo = serde_json::from_slice(&res.body).unwrap();
        assert!(updated.done);
        assert_eq!(updated.title, "buy milk");

        // 削除
        let path = format!("DELETE /todos/{} HTTP/1.1\r\n\r\n", created.id);
        let res = router.dispatch(&head_of(&path), b"").await;
        assert_eq!(res.status, 204);

        // 削除後は 404
        let path = format!("GET /todos/{} HTTP/1.1\r\n\r\n", created.id);
        let res = router.dispatch(&head_of(&path), b"").await;
        assert_eq!(res.status, 404);
    }

    #[tokio::test]
    async fn create_with_blank_title_returns_400() {
        let router = new_router();
        let res = router
            .dispatch(
                &head_of("POST /todos HTTP/1.1\r\n\r\n"),
                br#"{"title":"   "}"#,
            )
            .await;
        assert_eq!(res.status, 400);
    }

    #[tokio::test]
    async fn create_with_invalid_json_returns_400() {
        let router = new_router();
        let res = router
            .dispatch(&head_of("POST /todos HTTP/1.1\r\n\r\n"), b"not json")
            .await;
        assert_eq!(res.status, 400);
    }

    #[tokio::test]
    async fn get_with_non_numeric_id_returns_404() {
        let router = new_router();
        let res = router
            .dispatch(&head_of("GET /todos/abc HTTP/1.1\r\n\r\n"), b"")
            .await;
        assert_eq!(res.status, 404);
    }

    #[tokio::test]
    async fn delete_missing_todo_returns_404() {
        let router = new_router();
        let res = router
            .dispatch(&head_of("DELETE /todos/999 HTTP/1.1\r\n\r\n"), b"")
            .await;
        assert_eq!(res.status, 404);
    }

    #[tokio::test]
    async fn update_with_blank_title_returns_400() {
        let router = new_router();
        let created_res = router
            .dispatch(
                &head_of("POST /todos HTTP/1.1\r\n\r\n"),
                br#"{"title":"keep"}"#,
            )
            .await;
        let created: Todo = serde_json::from_slice(&created_res.body).unwrap();

        let path = format!("PATCH /todos/{} HTTP/1.1\r\n\r\n", created.id);
        let res = router.dispatch(&head_of(&path), br#"{"title":"  "}"#).await;
        assert_eq!(res.status, 400);
    }
}
