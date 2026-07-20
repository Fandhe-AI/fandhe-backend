//! 利用者アプリ独自の OpenAPI スキーマ登録サンプル（イシュー #320）。
//!
//! `Server::openapi()`（フレームワーク固定スキーマ、`examples/openapi_endpoints.rs`）
//! とは異なり、`Server::openapi_with(doc)` で利用者アプリが自前生成した
//! OpenAPI ドキュメント（本 example では `utoipa::OpenApi` 由来ではない
//! 手書き JSON）を `GET /openapi.json` として配信する。`_/todo-backend`
//! （ローカル検証用の利用者アプリ）が自前ルートで代替していた配信を、
//! `openapi` feature 側の opt-in 配信へ移行する最小サンプルとして使う。
//!
//! # 動作確認手順
//!
//! ```text
//! $ cargo run --example openapi_custom_doc -p fandhe-backend-core --features openapi
//! $ curl -si http://127.0.0.1:3005/openapi.json     # 登録済み JSON を配信
//! $ curl -si http://127.0.0.1:3005/openapi.yaml      # 未登録のため 404
//! $ curl -si http://127.0.0.1:3005/todos             # 無関係パス（既定 Handler）
//! ```

use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_openapi::OpenApiDoc;
use fandhe_backend_routes::Router;

/// 利用者アプリ独自の最小 OpenAPI ドキュメント（JSON 手書き）。
///
/// フレームワーク自身の `ApiDoc`（`crates/plugin-openapi/src/docs.rs`）とは
/// 無関係な、利用者アプリ側のエンドポイント（`GET /todos`）を宣言する。
/// `OpenApiDoc::from_json` が構築時に JSON 妥当性を検証する
/// （不正な JSON なら本関数は `Err` を返し、`main` はここで起動を中断する。
/// fail-closed、`OpenApiDoc::from_json` の doc を参照）。
fn custom_openapi_doc() -> Result<OpenApiDoc, fandhe_backend_plugin_openapi::OpenApiDocError> {
    let json = r#"{
        "openapi": "3.0.0",
        "info": { "title": "todo-backend (custom)", "version": "1.0.0" },
        "paths": {
            "/todos": {
                "get": {
                    "summary": "todo 一覧を返す",
                    "responses": { "200": { "description": "OK" } }
                }
            }
        }
    }"#;
    OpenApiDoc::from_json(json)
}

/// `GET /todos` のみを持つ最小 [`Router`]。
fn build_router() -> Router {
    Router::new().route("GET", "/todos", |_head, _body| {
        Response::new(200, b"[]".to_vec()).with_content_type("application/json")
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3005".to_string());
    // 起動時（バインド前）に一度だけ検証する。不正なスキーマで配信を続ける
    // 事態を構築時点で遮断する（fail-closed、`.claude/rules/security.md`）。
    let doc = custom_openapi_doc().expect("custom_openapi_doc は妥当な固定 JSON を返す");
    let server = Server::new().handler(build_router()).openapi_with(doc);

    println!("openapi_custom_doc: listening on http://{addr}");
    let bound = server.bind(&addr).await?;
    bound.run().await
}

#[cfg(test)]
mod tests {
    use super::{build_router, custom_openapi_doc};
    use fandhe_backend_core::{Handler, Server, handle_connection};
    use fandhe_backend_http::request::RequestHead;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// `Handler::handle` が呼ばれたら panic するトイハンドラ（`try_intercept`
    /// が既定 `Handler` を迂回する契約の証跡、`crates/core/tests/
    /// plugin_openapi_boundary.rs` と同型のパターン）。
    struct NotCalledHandler;
    impl Handler for NotCalledHandler {
        fn handle(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> fandhe_backend_routes::HandlerFuture {
            panic!("plugin::try_intercept が Some を返したのに既定 Handler が呼ばれた");
        }
    }

    async fn roundtrip(server: &Server, request: &[u8]) -> Vec<u8> {
        let (mut client, server_stream) = tokio::io::duplex(1 << 20);
        client.write_all(request).await.unwrap();
        client.shutdown().await.unwrap();
        handle_connection(server, server_stream).await;
        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn serves_registered_custom_json() {
        let doc = custom_openapi_doc().unwrap();
        let server = Server::new().handler(NotCalledHandler).openapi_with(doc);

        let response = roundtrip(
            &server,
            b"GET /openapi.json HTTP/1.1\r\nConnection: close\r\n\r\n",
        )
        .await;
        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Content-Type: application/json\r\n"));
        assert!(response.contains("\"title\": \"todo-backend (custom)\""));
    }

    #[tokio::test]
    async fn unregistered_yaml_falls_through_to_404() {
        let doc = custom_openapi_doc().unwrap();
        let server = Server::new().openapi_with(doc);

        let response = roundtrip(
            &server,
            b"GET /openapi.yaml HTTP/1.1\r\nConnection: close\r\n\r\n",
        )
        .await;
        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    #[tokio::test]
    async fn unrelated_path_reaches_default_handler() {
        let doc = custom_openapi_doc().unwrap();
        let server = Server::new().handler(build_router()).openapi_with(doc);

        let response =
            roundtrip(&server, b"GET /todos HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("[]"));
    }
}
