//! GraphQL プラグイン（`graphql` feature）の配線だけを見せる最小サンプル。
//!
//! `crates/core/examples/graphql_nfr6.rs` を土台に、独立して `cargo run`
//! できる standalone crate として `examples/with-graphql/` に複製した
//! （Next.js の `examples/` 方式、`examples/README.md` 参照）。
//! `crates/plugin-graphql/src/lib.rs` の crate doc が述べるとおり、GraphQL は
//! パスインターセプト型プラグインであり `Router` の責務外である
//! （`Server::graphql(config)` 登録時のみ既定 `Handler` より先に
//! `POST /graphql` を捕捉し、未登録時は feature 有効でもフォールスルーする）:
//!
//! 1. `fn graphql_config()` で `async_graphql::dynamic`（実行時スキーマ構築 API）
//!    を使いデモスキーマ（`hello` / `echo`）を組み立て、クエリ深さ・複雑度制限を
//!    設定する（DoS 対策はスキーマ登録者の責務、`GraphQlConfig::new` の doc・
//!    `.claude/rules/security.md` 参照）
//! 2. `Server::new().handler(router).graphql(config)` で登録する
//!
//! # 起動方法
//!
//! ```text
//! $ cd examples/with-graphql
//! $ cargo run
//! ```
//!
//! 既定で `127.0.0.1:3000` に bind する（`PORT` 環境変数で上書き可能）。
//!
//! # 動作確認手順
//!
//! ```text
//! # クエリ実行（{"data":{"hello":"world"}} を確認）
//! $ curl -s -X POST http://127.0.0.1:3000/graphql -d '{"query":"{ hello }"}'
//!
//! # variables 付きクエリ実行（{"data":{"echo":"hi"}} を確認）
//! $ curl -s -X POST http://127.0.0.1:3000/graphql \
//!     -d '{"query":"query($v: String!) { echo(value: $v) }","variables":{"v":"hi"}}'
//!
//! # 不正 body（400 を確認）
//! $ curl -si -X POST http://127.0.0.1:3000/graphql -d 'not json'
//!
//! # 無関係パス（Router の応答を確認、GraphQL インターセプトが波及しないこと）
//! $ curl -s http://127.0.0.1:3000/
//! ```

use async_graphql::Value;
use async_graphql::dynamic::{Field, FieldFuture, InputValue, Object, Schema, TypeRef};
use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_graphql::GraphQlConfig;
use fandhe_backend_routes::Router;

/// `hello`（引数なし）・`echo(value: String!)` を持つ最小デモスキーマを組み立てる。
///
/// クエリ深さ・複雑度制限（`Schema::limit_depth` / `Schema::limit_complexity`）は
/// `GraphQlConfig::new` の doc が明記するとおりスキーマ登録者（呼び出し元）の
/// 責務であり、本クレートは既定値を提供しない。本サンプルではリソース枯渇 DoS
/// 対策の実演として明示的に設定する（`.claude/rules/security.md` リソース枯渇
/// 観点）。introspection は `async-graphql` の既定で有効のままにしている
/// （開発サンプル用途のため）。本番相当の非開発環境では
/// `Schema::disable_introspection` の追加を検討すること
/// （`.claude/rules/security.md` A05 設定ミス観点、`GraphQlConfig::new` の doc 参照）。
fn graphql_config() -> GraphQlConfig {
    let query = Object::new("Query")
        .field(Field::new(
            "hello",
            TypeRef::named_nn(TypeRef::STRING),
            |_ctx| FieldFuture::new(async move { Ok(Some(Value::from("world"))) }),
        ))
        .field(
            Field::new("echo", TypeRef::named_nn(TypeRef::STRING), |ctx| {
                FieldFuture::new(async move {
                    let value = ctx.args.try_get("value")?.string()?.to_owned();
                    Ok(Some(Value::from(value)))
                })
            })
            .argument(InputValue::new("value", TypeRef::named_nn(TypeRef::STRING))),
        );

    let schema = Schema::build(query.type_name(), None, None)
        .limit_depth(8)
        .limit_complexity(64)
        .register(query)
        .finish()
        .expect("デモスキーマの構築は静的に妥当なので必ず成功する");
    GraphQlConfig::new(schema)
}

/// `GET /` のみを持つ最小 [`Router`]（`main` とテストの両方から共有するため
/// 関数として切り出す、`examples/with-cors/src/main.rs` と同一パターン）。
/// GraphQL は `Router` の責務外（`Server` 層のパスインターセプト）のため、
/// ここには一切配線しない（`main` 側で `Server::graphql` を登録する）。
fn build_router() -> Router {
    Router::new().route("GET", "/", |_head, _body| {
        Response::new(
            200,
            b"fandhe-backend-example-with-graphql: try POST /graphql\n".to_vec(),
        )
    })
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> std::io::Result<()> {
    let router = build_router();
    let server = Server::new().handler(router).graphql(graphql_config());

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("127.0.0.1:{port}");
    let bound = server.bind(&addr).await?;
    println!("fandhe-backend-example-with-graphql listening on {addr}");
    bound
        .run_until(async {
            // 登録失敗を握りつぶすと future が即完了し bind 直後にサーバが
            // 終了してしまうため、シグナルハンドラを登録できない環境では
            // 起動継続せず明示的に panic させる（`examples/with-cors` と同方針）
            tokio::signal::ctrl_c()
                .await
                .expect("Ctrl-C シグナルハンドラの登録に失敗した");
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_core::handle_connection;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// `crates/core/tests/plugin_graphql_boundary.rs` と同型のヘルパ。
    /// GraphQL はパスインターセプト型（`Server` 層）のため、`Router::dispatch`
    /// 単体ではなく `handle_connection` 経由で end-to-end に検証する。
    async fn roundtrip(server: &Server, request: &[u8]) -> String {
        let (mut client, server_stream) = tokio::io::duplex(8192);
        client.write_all(request).await.unwrap();
        client.shutdown().await.unwrap();

        handle_connection(server, server_stream).await;

        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        String::from_utf8(out).unwrap()
    }

    fn new_server() -> Server {
        Server::new()
            .handler(build_router())
            .graphql(graphql_config())
    }

    #[tokio::test]
    async fn graphql_query_is_executed() {
        let server = new_server();
        let body = br#"{"query":"{ hello }"}"#;
        let mut request = format!(
            "POST /graphql HTTP/1.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);

        let response = roundtrip(&server, &request).await;

        // ステータス・Content-Type・body の全件を検証する（PoC-9 教訓:
        // ステータスのみの検証は reason/Content-Type/body の劣化を見逃す。
        // `crates/core/tests/plugin_graphql_boundary.rs` と同一原則）。
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Content-Type: application/json\r\n"));
        assert!(response.contains(r#""hello":"world"#));
    }

    #[tokio::test]
    async fn graphql_variables_are_applied() {
        let server = new_server();
        let body = br#"{"query":"query($v: String!) { echo(value: $v) }","variables":{"v":"hi"}}"#;
        let mut request = format!(
            "POST /graphql HTTP/1.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);

        let response = roundtrip(&server, &request).await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains(r#""echo":"hi"#));
    }

    #[tokio::test]
    async fn invalid_json_body_returns_400() {
        let server = new_server();
        let request =
            b"POST /graphql HTTP/1.1\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot json";

        let response = roundtrip(&server, request).await;

        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    }

    #[tokio::test]
    async fn unrelated_path_falls_through_to_router() {
        let server = new_server();
        let request = b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n";

        let response = roundtrip(&server, request).await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("try POST /graphql"));
    }
}
