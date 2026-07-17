//! `bf-plugin-graphql`: パスインターセプト型 GraphQL プラグイン実装（TASK-5.1 / #38）。
//!
//! 拡張点対応: パスインターセプト型（try_intercept）
//! （3 拡張点 trait には非該当。固定シグネチャシームへの閉包根拠は
//! `docs/design/extension-closure-verification.md` 3.4 節、機械可読宣言の規約は
//! `docs/design/dependency-graph-contract.md` 3 節、TASK-13.2 / #50）
//!
//! # 位置づけ
//!
//! TASK-2.4（#21）は `POST /graphql` への固定 JSON 応答のみを提供するプラグイン
//! 境界スタブとして本クレートを確立した（REQ-2 の「2 種のプラグイン着脱」受け入れ
//! 基準を webrtc-proxy と共に実証する第 2 インスタンス）。本タスク（TASK-5.1）は
//! そのスタブに `async-graphql` による**実クエリ実行**を実装する。パスインター
//! セプト型・`Option` フォールスルーというプラグイン境界パターン自体は変更せず
//! （`docs/design/plugin-boundary.md` 4 節）、内部実装のみを差し替える。
//!
//! # コアループへの配線について
//!
//! 本クレート単体では HTTP サーバのリスンループを持たない。`graphql` feature
//! （`optional = true` + `dep:` 構文、`.claude/rules/pay-for-what-you-use.md`）
//! 有効時のみ `backend_framework_core::plugin::try_intercept` から
//! [`try_handle_graphql`] が呼ばれる（`crates/core/src/plugin.rs` を参照）。
//! feature 無効時（既定）は本クレート自体が `backend-framework-core` の
//! 依存グラフから除外される。
//!
//! TASK-2.4 時代と異なり、TASK-5.1 以降は**スキーマが未登録の場合は
//! フォールスルー（404）** に挙動が変わる（`webrtc-proxy`・`webrtc` プラグイン
//! と同じ「設定登録型」パターンへ揃えるため）。[`GraphQlConfig`] を
//! `backend_framework_core::Server::graphql` で登録しない限り、`graphql`
//! feature が有効でも `POST /graphql` は既定 `Handler` へフォールスルーする
//! （`crates/core/src/server.rs` の `graphql_config` doc を参照）。
//!
//! # workspace 内での依存方向
//!
//! `docs/spec/04-requirements.md` REQ-1 / `docs/spec/05-tasks.md` TASK-11.1 の
//! 方針に従い、依存方向は `server → routes → http::*` の一方向を維持する。
//! 本クレートはプラグイン層（`bf-plugin-*`）に位置し、workspace 内 path 依存は
//! `bf-http`（下位層の sans-IO パーサ）のみ。依存方向の機械検証は
//! `scripts/dep-direction-check.sh` が担う。
//!
//! # pay-for-what-you-use
//!
//! `async-graphql`（`default-features = false`）とその推移的依存は `graphql`
//! feature 有効時のみ依存グラフへ載る（本クレート自体が `bf-plugin-graphql`
//! という 1 つの `dep:` 単位のため）。feature 無効構成では本クレートごと
//! 依存グラフから消え、`async-graphql` 系クレートは一切ビルドされない
//! （`cargo tree -p backend-framework-core --no-default-features` で確認可能、
//! `.claude/rules/pay-for-what-you-use.md`）。
//!
//! # Examples
//!
//! 対象外パスは `None` を返し、無関係なリクエストへの性能影響がないことを示す。
//!
//! ```
//! use bf_http::request::{parse_request_head, ParseOutcome};
//! use bf_plugin_graphql::{try_handle_graphql, GraphQlConfig};
//! use async_graphql::Value;
//! use async_graphql::dynamic::{Field, FieldFuture, Object, Schema, TypeRef};
//!
//! // `async_graphql::dynamic`（実行時スキーマ構築 API）で組み立てる。
//! // `#[Object]` 派生マクロは使わない（本クレート Cargo.toml の doc・
//! // `docs/design/unsafe-deny-lints.md` を参照。マクロが生成コードへ無条件
//! // 付与する `#[allow(clippy::all, ...)]` は workspace の forbid lint と
//! // 衝突するため、本クレートは動的スキーマ API のみを doc test・単体テストで
//! // 使い、workspace の forbid lint をそのまま継承する）。
//! let query = Object::new("Query").field(Field::new(
//!     "hello",
//!     TypeRef::named_nn(TypeRef::STRING),
//!     |_ctx| FieldFuture::new(async move { Ok(Some(Value::from("world"))) }),
//! ));
//! let schema = Schema::build(query.type_name(), None, None)
//!     .register(query)
//!     .finish()
//!     .unwrap();
//!
//! let buf = b"GET /health HTTP/1.1\r\n\r\n";
//! let head = match parse_request_head(buf).unwrap() {
//!     ParseOutcome::Complete { head, .. } => head,
//!     ParseOutcome::Incomplete => unreachable!(),
//! };
//!
//! let config = GraphQlConfig::new(schema);
//!
//! let runtime = tokio::runtime::Runtime::new().unwrap();
//! let result = runtime.block_on(try_handle_graphql(&head, b"", &config));
//! assert!(result.is_none());
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_graphql::{Executor, Variables};
use bf_http::request::RequestHead;
use serde::Deserialize;

/// 本プラグインがパスインターセプトの対象とするリクエストパス。
pub const GRAPHQL_PATH: &str = "/graphql";

/// [`try_handle_graphql`] が返す完結済み HTTP レスポンスの中間表現。
///
/// `crates/plugin-webrtc-proxy::handler::Response` と同型（ステータス・
/// `Content-Type`・body のみを保持する軽量な中間表現）。ソケットへの実書き込みは
/// 呼び出し元（コア接続ループ側）の責務とする。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP ステータスコード。
    pub status: u16,
    /// `Content-Type` ヘッダ値（`&'static str` 限定。
    /// `bf_http::response::Response::with_content_type` の制約に合わせる）。
    pub content_type: &'static str,
    /// レスポンス body。
    pub body: Vec<u8>,
}

impl Response {
    fn json(status: u16, body: Vec<u8>) -> Self {
        Response {
            status,
            content_type: "application/json",
            body,
        }
    }
}

/// 型消去済みの GraphQL 実行クロージャ。
///
/// `async_graphql::Executor::execute` は RPITIT（`impl Future` 戻り値）で
/// `dyn Executor` として扱えないため、`Schema<Q, M, S>` を `Fn` クロージャへ
/// 閉じ込めて型消去する。クロージャは `Executor` を `clone()`（`Arc` ベースの
/// 軽量クローンが `async-graphql` の実装契約）してから `async move` ブロックへ
/// move することで、`&self` を借用したまま `'static` な `Future` を返す借用
/// エラーを避ける。
type BoxExecuteFn = Arc<
    dyn Fn(async_graphql::Request) -> Pin<Box<dyn Future<Output = async_graphql::Response> + Send>>
        + Send
        + Sync,
>;

/// [`try_handle_graphql`] が実行する GraphQL スキーマの登録設定。
///
/// `backend_framework_core::Server::graphql` に渡して有効化する（`webrtc-proxy`
/// feature の `ProxyConfig` 登録パターンと同型）。未登録時は `graphql` feature
/// が有効でも `POST /graphql` はフォールスルーする（本クレート crate doc を
/// 参照）。
///
/// # Examples
///
/// ```
/// use async_graphql::Value;
/// use async_graphql::dynamic::{Field, FieldFuture, Object, Schema, TypeRef};
/// use bf_plugin_graphql::GraphQlConfig;
///
/// let query = Object::new("Query").field(Field::new(
///     "hello",
///     TypeRef::named_nn(TypeRef::STRING),
///     |_ctx| FieldFuture::new(async move { Ok(Some(Value::from("world"))) }),
/// ));
/// let schema = Schema::build(query.type_name(), None, None)
///     .register(query)
///     .finish()
///     .unwrap();
/// let _config = GraphQlConfig::new(schema);
/// ```
#[derive(Clone)]
pub struct GraphQlConfig {
    execute: BoxExecuteFn,
}

impl GraphQlConfig {
    /// 任意の `Schema<Q, M, S>`（[`Executor`] を実装する型）からスキーマ登録
    /// 設定を作る。
    ///
    /// `Executor` は `Clone + Send + Sync + 'static` を要求する
    /// （`async_graphql::Schema` はこれを満たす）。クエリ深さ・複雑度制限
    /// （`Schema::limit_depth` / `Schema::limit_complexity`）はスキーマ登録者
    /// （呼び出し元）の責務とする。本クレートは既定値を提供しない
    /// （リソース枯渇 DoS 対策の詳細は呼び出し元のスキーマ構築時に設定する。
    /// `.claude/rules/security.md`）。また introspection は `async-graphql`
    /// の既定で有効であるため、非開発環境で無効化したい場合は
    /// `Schema::disable_introspection` を呼び出し元が使う（`.claude/rules/security.md`
    /// A05 設定ミス観点）。
    pub fn new<E>(executor: E) -> Self
    where
        E: Executor + Clone + Send + Sync + 'static,
    {
        GraphQlConfig {
            execute: Arc::new(move |request| {
                let executor = executor.clone();
                Box::pin(async move { executor.execute(request).await })
            }),
        }
    }
}

impl std::fmt::Debug for GraphQlConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphQlConfig").finish_non_exhaustive()
    }
}

/// HTTP body から受け取る GraphQL over HTTP POST リクエストの JSON 形状。
///
/// `GET /graphql`（GraphQL over HTTP の GET クエリ形式）は本タスクのスコープ
/// 外（`try_handle_graphql` の doc を参照）。
#[derive(Debug, Deserialize)]
struct GraphQlRequestBody {
    query: String,
    #[serde(default)]
    variables: Option<serde_json::Value>,
    #[serde(default, rename = "operationName")]
    operation_name: Option<String>,
}

/// パース失敗時の固定エラー応答 body（GraphQL エラー形式、
/// `{"errors":[{"message": "..."}]}`）。リクエスト由来の値を一切埋め込まない
/// （レスポンス分割・JSON インジェクション対策、`.claude/rules/security.md`）。
const INVALID_REQUEST_BODY: &str = "{\"errors\":[{\"message\":\"invalid request body\"}]}";

/// `POST /graphql` をパスインターセプトし、`config` に登録済みのスキーマで
/// GraphQL クエリを実行して結果 JSON を返す。
///
/// - メソッド・パスが対象外なら `None` を返す（呼び出し元は次のハンドラへ
///   フォールスルーする契約。`crates/plugin-webrtc-proxy::try_handle_rtc_offer`
///   と同型）
/// - body を `{"query": String, "variables"?: Value, "operationName"?: String}`
///   としてパースする。JSON として不正、または `query` フィールドを欠く場合は
///   `400` + 固定 body `INVALID_REQUEST_BODY`（非公開定数）を返す（リクエスト
///   由来の値を一切エコーしない）
/// - パース成功後は `config` のスキーマで実行し、実行時エラー（GraphQL バリ
///   デーション・resolver エラー等）を含めて `200` + `application/json` で
///   返す（GraphQL over HTTP の慣行どおり、実行時エラーは応答 body の
///   `"errors"` フィールドで表現し、HTTP ステータスは変えない）
///
/// # Examples
///
/// ```
/// use async_graphql::Value;
/// use async_graphql::dynamic::{Field, FieldFuture, Object, Schema, TypeRef};
/// use bf_http::request::{parse_request_head, ParseOutcome};
/// use bf_plugin_graphql::{try_handle_graphql, GraphQlConfig};
///
/// let query = Object::new("Query").field(Field::new(
///     "hello",
///     TypeRef::named_nn(TypeRef::STRING),
///     |_ctx| FieldFuture::new(async move { Ok(Some(Value::from("world"))) }),
/// ));
/// let schema = Schema::build(query.type_name(), None, None)
///     .register(query)
///     .finish()
///     .unwrap();
///
/// let body = br#"{"query":"{ hello }"}"#;
/// let request = format!(
///     "POST /graphql HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
///     body.len()
/// );
/// let head = match parse_request_head(request.as_bytes()).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
///
/// let config = GraphQlConfig::new(schema);
///
/// let runtime = tokio::runtime::Runtime::new().unwrap();
/// let response = runtime
///     .block_on(try_handle_graphql(&head, body, &config))
///     .expect("対象パスなので Some");
/// assert_eq!(response.status, 200);
/// assert_eq!(response.content_type, "application/json");
/// assert!(String::from_utf8(response.body).unwrap().contains("world"));
/// ```
pub async fn try_handle_graphql(
    head: &RequestHead,
    body: &[u8],
    config: &GraphQlConfig,
) -> Option<self::Response> {
    if head.method != "POST" || head.target != GRAPHQL_PATH {
        return None;
    }

    let parsed: GraphQlRequestBody = match serde_json::from_slice(body) {
        Ok(parsed) => parsed,
        Err(_) => {
            return Some(Response::json(
                400,
                INVALID_REQUEST_BODY.as_bytes().to_vec(),
            ));
        }
    };

    let mut request = async_graphql::Request::new(parsed.query);
    if let Some(variables) = parsed.variables {
        request = request.variables(Variables::from_json(variables));
    }
    if let Some(operation_name) = parsed.operation_name {
        request = request.operation_name(operation_name);
    }

    let result = (config.execute)(request).await;
    // `serde_json` の直列化に一貫して委ねる（手組み JSON を書かない、
    // `.claude/rules/security.md` A03 インジェクション対策）。
    let serialized =
        serde_json::to_vec(&result).unwrap_or_else(|_| INVALID_REQUEST_BODY.as_bytes().to_vec());
    Some(Response::json(200, serialized))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_graphql::Value;
    use async_graphql::dynamic::{Field, FieldFuture, InputValue, Object, Schema, TypeRef};
    use bf_http::request::{ParseOutcome, parse_request_head};

    /// `hello`（引数なし）・`echo(value: String)` を持つ最小デモスキーマ。
    ///
    /// `async_graphql::dynamic`（実行時スキーマ構築 API、`Schema::build`）で
    /// 組み立てる。`#[Object]` 派生マクロは使わない（`crates/core/tests/
    /// plugin_graphql_boundary.rs` と同型のパターン。マクロが生成コードへ
    /// 無条件付与する `#[allow(clippy::all, ...)]` は workspace の forbid
    /// lint（`docs/design/unsafe-deny-lints.md` 第 1 層）と衝突するため、
    /// 本クレートは動的スキーマ API のみをテストで使い、workspace の forbid
    /// lint をそのまま継承する。本クレート自身の Cargo.toml も参照）。
    fn demo_config() -> GraphQlConfig {
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
            .register(query)
            .finish()
            .expect("デモスキーマの構築は静的に妥当なので必ず成功する");
        GraphQlConfig::new(schema)
    }

    fn head(raw: &[u8]) -> RequestHead {
        match parse_request_head(raw).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => unreachable!(),
        }
    }

    fn post_request(body: &[u8]) -> Vec<u8> {
        let mut buf = format!(
            "POST /graphql HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        buf.extend_from_slice(body);
        buf
    }

    #[tokio::test]
    async fn executes_query_and_returns_result() {
        let body = br#"{"query":"{ hello }"}"#;
        let raw = post_request(body);
        let h = head(&raw);
        let config = demo_config();

        let response = try_handle_graphql(&h, body, &config)
            .await
            .expect("対象パスなので Some");
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        let json: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(json["data"]["hello"], "world");
    }

    #[tokio::test]
    async fn variables_are_applied() {
        let body = br#"{"query":"query($v: String!) { echo(value: $v) }","variables":{"v":"hi"}}"#;
        let raw = post_request(body);
        let h = head(&raw);
        let config = demo_config();

        let response = try_handle_graphql(&h, body, &config).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(json["data"]["echo"], "hi");
    }

    #[tokio::test]
    async fn operation_name_selects_named_operation() {
        let body = br#"{
            "query": "query A { hello } query B { echo(value: \"b\") }",
            "operationName": "B"
        }"#;
        let raw = post_request(body);
        let h = head(&raw);
        let config = demo_config();

        let response = try_handle_graphql(&h, body, &config).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(json["data"]["echo"], "b");
    }

    #[tokio::test]
    async fn invalid_json_body_is_rejected() {
        let body = b"not json";
        let raw = post_request(body);
        let h = head(&raw);
        let config = demo_config();

        let response = try_handle_graphql(&h, body, &config).await.unwrap();
        assert_eq!(response.status, 400);
        assert_eq!(response.content_type, "application/json");
        assert_eq!(response.body, INVALID_REQUEST_BODY.as_bytes());
    }

    #[tokio::test]
    async fn missing_query_field_is_rejected() {
        let body = br#"{"variables":{}}"#;
        let raw = post_request(body);
        let h = head(&raw);
        let config = demo_config();

        let response = try_handle_graphql(&h, body, &config).await.unwrap();
        assert_eq!(response.status, 400);
        assert_eq!(response.body, INVALID_REQUEST_BODY.as_bytes());
    }

    #[tokio::test]
    async fn graphql_execution_error_is_reported_as_200_with_errors_field() {
        // 未知フィールドへのクエリは GraphQL バリデーションエラーとなり、
        // HTTP ステータスは変えず `errors` フィールドで表現する
        // （GraphQL over HTTP の慣行、本関数 doc を参照）。
        let body = br#"{"query":"{ unknownField }"}"#;
        let raw = post_request(body);
        let h = head(&raw);
        let config = demo_config();

        let response = try_handle_graphql(&h, body, &config).await.unwrap();
        assert_eq!(response.status, 200);
        let json: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert!(json["errors"].is_array());
        assert!(!json["errors"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn falls_through_on_unrelated_path() {
        let h = head(b"GET /health HTTP/1.1\r\n\r\n");
        let config = demo_config();
        assert!(try_handle_graphql(&h, b"", &config).await.is_none());
    }

    #[tokio::test]
    async fn falls_through_on_wrong_method() {
        // GET /graphql（GraphQL over HTTP の GET クエリ形式）は本実装の
        // 対象外とする（TASK-5.1 スコープ外、crate doc を参照）。
        let h = head(b"GET /graphql HTTP/1.1\r\n\r\n");
        let config = demo_config();
        assert!(try_handle_graphql(&h, b"", &config).await.is_none());
    }
}
