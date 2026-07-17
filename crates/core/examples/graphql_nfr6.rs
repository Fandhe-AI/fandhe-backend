//! TASK-5.2（#53）NFR 計測専用サーバ。
//!
//! `graphql` feature 有効時に `Server::graphql` へ [`GraphQlConfig`] を登録した構成で
//! `examples/minimal.rs` と同一の `GET /`（`bf_routes::Router`）を提供する。
//! `docs/acceptance/req5-graphql.md`（REQ-5 受け入れ基準 2: 性能影響誤差範囲）が、
//! 本 example と `examples/minimal.rs`（`graphql` feature 無効のベースライン）へ
//! それぞれ無関係パス（`/`）へ負荷をかけ、RPS・p95 の比が誤差範囲に収まることを
//! 検証するために使う。production 配線（`Server::graphql` の呼び出し判断自体）には
//! 触れず、計測専用の example として追加する（TASK-5.2 は test スコープ、
//! production コード変更を含まない。`examples/webrtc_nfr6.rs`＝TASK-8.4／#29 と
//! 同型のパターン）。
//!
//! `plugin::try_intercept` は `graphql` feature 有効時、`POST /graphql` 宛て
//! リクエストのみをパス完全一致で捕捉し、それ以外（本計測対象の `GET /`）は
//! 1 回のパス比較のみで `Handler::handle` へフォールスルーする
//! （`crates/core/src/plugin.rs` の doc を参照）。
//!
//! デモスキーマは `crates/core/tests/plugin_graphql_boundary.rs` の
//! `demo_schema()` と同一構成（`{ hello }` を返すのみ）。`async_graphql::dynamic`
//! （実行時スキーマ構築 API、`Schema::build`）で組み立てる。`#[Object]` 派生
//! マクロは使わない（`crates/core/Cargo.toml` の dev-dependency コメントを参照。
//! マクロが生成コードへ付与する `#[allow(clippy::all)]` が `backend-framework-core`
//! の継承する workspace の forbid lint と衝突するため）。
//!
//! 動作確認手順:
//! ```text
//! $ cargo run --release --example graphql_nfr6 -p backend-framework-core --features graphql
//! $ curl -v http://127.0.0.1:3003/                                   # 200 応答（無関係パス）
//! $ curl -v -X POST http://127.0.0.1:3003/graphql -d '{"query":"{ hello }"}'  # クエリ実行
//! ```

use async_graphql::Value;
use async_graphql::dynamic::{Field, FieldFuture, Object, Schema, TypeRef};
use backend_framework_core::Server;
use bf_http::response::Response;
use bf_plugin_graphql::GraphQlConfig;
use bf_routes::Router;

/// `POST /graphql` の実行対象とする最小デモスキーマ（`{ hello }` を返すのみ）。
///
/// `crates/core/tests/plugin_graphql_boundary.rs` の `demo_schema()` と同一構成
/// （計測対象の疎通確認・NFR 計測いずれも同じスキーマで揃え、計測結果の解釈を
/// 単純にするため）。
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let router = Router::new().route("GET", "/", |_head, _body| {
        Response::new(200, b"backend-framework: graphql nfr6 example\n".to_vec())
    });

    let server = Server::new().handler(router).graphql(demo_schema());
    let bound = server.bind("127.0.0.1:3003").await?;
    println!("listening on http://{}", bound.local_addr()?);
    bound.run().await
}
