//! `graphql` feature（TASK-5.1 / #38）配線の統合テスト（feature 有効側）。
//!
//! `crates/core/src/plugin.rs` の非公開 `try_intercept` シームが実際に
//! `bf_plugin_graphql::try_handle_graphql` へ委譲し、`Server::graphql` で登録
//! 済みのデモスキーマに対して `POST /graphql` が実クエリ実行され、既定
//! `Handler` より先にインターセプトされることを、`tokio::io::duplex` で駆動する
//! `handle_connection` を通して検証する。`webrtc-proxy`・`webrtc` と同じ
//! 「設定登録型」パターンのため、**スキーマ未登録時は feature が有効でも
//! フォールスルー（404）する**ことも併せて確認する（`crates/plugin-graphql` の
//! crate doc・`docs/design/plugin-boundary.md` の検証観点、
//! `crates/core/tests/plugin_boundary.rs` と同型のパターン）。
//!
//! feature 無効時の陰性対照は `plugin_graphql_boundary_disabled.rs` を参照。

#![cfg(feature = "graphql")]

use async_graphql::Value;
use async_graphql::dynamic::{Field, FieldFuture, Object, Schema, TypeRef};
use backend_framework_core::{Handler, Server, handle_connection};
use bf_http::request::RequestHead;
use bf_http::response::Response;
use bf_plugin_graphql::GraphQlConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `Handler::handle` が呼ばれたら panic するトイハンドラ。
///
/// `plugin::try_intercept` が `Some` を返した場合は既定 `Handler` を呼ばない
/// 契約（`crates/core/src/server.rs` の `handle_connection` を参照）の証跡に使う。
struct NotCalledHandler;
impl Handler for NotCalledHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        panic!("plugin::try_intercept が Some を返したのに既定 Handler が呼ばれた");
    }
}

/// 固定 200 応答を返すだけのトイハンドラ（フォールスルー確認用）。
struct FixedOkHandler;
impl Handler for FixedOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        Response::new(200, b"ok".to_vec())
    }
}

/// `POST /graphql` の実行対象とする最小デモスキーマ（`{ hello }` を返すのみ）。
///
/// `async_graphql::dynamic`（実行時スキーマ構築 API、`Schema::build`）で組み立てる。
/// `#[Object]` 派生マクロは使わない（本ファイル冒頭の doc・`crates/core/Cargo.toml`
/// の dev-dependency コメントを参照。マクロが生成コードへ付与する
/// `#[allow(clippy::all)]` が `backend-framework-core` の継承する workspace の
/// forbid lint と衝突するため）。
fn demo_schema() -> GraphQlConfig {
    let query = Object::new("Query").field(Field::new(
        "hello",
        TypeRef::named_nn(TypeRef::STRING),
        |_ctx| FieldFuture::new(async move { Ok(Some(Value::from("world"))) }),
    ));

    let schema = Schema::build(query.type_name(), None, None)
        .register(query)
        .finish()
        .expect("デモスキーマの構築は静的に妥当なので必ず成功する");
    GraphQlConfig::new(schema)
}

async fn roundtrip(server: &Server, request: &[u8]) -> String {
    let (mut client, server_stream) = tokio::io::duplex(8192);
    client.write_all(request).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

#[tokio::test]
async fn registered_schema_executes_query_and_bypasses_default_handler() {
    let server = Server::new()
        .handler(NotCalledHandler)
        .graphql(demo_schema());

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
    // `crates/core/tests/plugin_boundary.rs` と同一原則）。
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("Content-Type: application/json\r\n"));
    assert!(response.contains(r#""hello":"world"#));
}

#[tokio::test]
async fn unregistered_schema_falls_through_to_404() {
    // `graphql` feature は有効だが `Server::graphql` を呼んでいない構成。
    // `webrtc-proxy`・`webrtc` と同じ設定登録型パターンにより、未登録時は
    // 既定 `Handler`（未登録時 404）へフォールスルーする
    // （`crates/core/src/plugin.rs` の doc を参照）。
    let server = Server::new();

    let request = b"POST /graphql HTTP/1.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}

#[tokio::test]
async fn unrelated_path_falls_through_to_default_handler() {
    let server = Server::new().handler(FixedOkHandler).graphql(demo_schema());

    let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("ok"));
}
