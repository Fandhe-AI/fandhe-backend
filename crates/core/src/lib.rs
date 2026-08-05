//! fandhe-backend の最小コア。
//!
//! # このクレートの役割
//!
//! `crates/core` は HTTP/1.1 パーサ・keep-alive・3 種拡張点
//! （`Middleware` / `UpgradeHandler` / `RequestGate`、[`extension`] モジュール）を
//! 実装する最小コアの置き場所。TASK-1.4-1（#69）で 3 拡張点の trait 定義を、
//! TASK-1.4-2（#70）でコアループ（接続受理・リクエストループ、[`server`]
//! モジュール）による実接続を提供する。3 拡張点で表現できないリダイレクト・
//! レスポンス改変（[`interceptor::Interceptor`]、イシュー #420）は feature
//! ゲート不要の追加拡張点として同モジュールに置く（詳細は [`interceptor`] を参照）。
//!
//! # workspace 内での依存方向
//!
//! `docs/spec/04-requirements.md` REQ-1 / `docs/spec/05-tasks.md` TASK-1.5 の方針に従い、
//! workspace 全体の依存方向は次の一方向を維持する:
//!
//! ```text
//! server → routes → http::*
//! ```
//!
//! `core`（本クレート）はこの依存グラフの末端に位置し、`plugin-*` の固有シンボルには
//! 一切依存しない。プラグインは feature 経由でコアの拡張点（`Middleware` /
//! `UpgradeHandler` / `RequestGate`）を実装する側であり、コアからプラグインへの
//! 依存は発生しない（pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。
//! `fandhe-backend-http`（sans-IO な HTTP/1.1 パーサ、TASK-1.3）は workspace 内の下位層クレート
//! であり、外部 crates.io 依存はここでも増やさない。
//!
//! `crates/routes`（`fandhe-backend-routes`）は TASK-1.5（#14）で新設し、`server → routes`
//! エッジを `Cargo.toml` の依存宣言として実体化した。`fandhe_backend_routes::Router` は
//! `impl` [`server::Handler`] `for` `fandhe_backend_routes::Router` により本クレートの
//! 既定ハンドラとしてそのまま登録できる（`crates/core/examples/minimal.rs`
//! 参照）。依存方向の機械検証は `scripts/dep-direction-check.sh`（TASK-1.5）が
//! `cargo metadata` の依存エッジホワイトリスト照合で行う。
//!
//! # feature 一覧
//!
//! - `webrtc-proxy`（TASK-2.1 / #18）: `crates/plugin-webrtc-proxy` を
//!   `optional = true` + `dep:` 構文で組み込み、[`server::Server::webrtc_proxy`]
//!   で登録した `fandhe_backend_plugin_webrtc_proxy::ProxyConfig` に基づき `POST
//!   /rtc/offer` をパスインターセプトする。無効時（既定）は依存・コード・
//!   `unsafe` を一切含まない（クレート非公開の `plugin` モジュールの doc・
//!   `docs/design/plugin-boundary.md` を参照）。
//! - `webrtc`（TASK-8.1 / #26）: `crates/plugin-webrtc`（in-process 型、
//!   `webrtc-rs` 直接依存）を同じくプラグイン境界パターンで組み込み、
//!   [`server::Server::webrtc`] で登録した `fandhe_backend_plugin_webrtc::WebRtcConfig` に
//!   基づき `POST /rtc/offer` をパスインターセプトする。`webrtc-proxy` と
//!   同時有効時は `webrtc-proxy` が優先される（`plugin::try_intercept` の
//!   doc）。無効時（既定）は依存・コード・`unsafe` を一切含まない。
//! - `websocket`（TASK-4.1 / #22）: `crates/plugin-websocket` を
//!   `optional = true` + `dep:` 構文で組み込み、[`server::Server::websocket`]
//!   で登録した `fandhe_backend_plugin_websocket::WebSocketConfig` の指すパス（既定
//!   `/ws`）への `GET` + `Upgrade: websocket` を `UpgradeHandler` 拡張点経由で
//!   検知し、`fandhe_backend_plugin_websocket::handle_upgrade` へ完全委譲する（RFC 6455
//!   ハンドシェイク・フレーミングは `crates/plugin-websocket` 側の責務）。設定型
//!   `WebSocketConfig` は [`plugin_websocket`] モジュールとして再エクスポートし
//!   （イシュー #435）、`fandhe-backend-plugin-websocket` へ直接依存せずに構築できる。
//!   無効時（既定）は依存・コード・`unsafe` を一切含まない。
//! - `graphql`（TASK-2.4 / #21）: `crates/plugin-graphql` を組み込み、
//!   [`server::Server::graphql`] へ渡す設定型 `GraphQlConfig` を
//!   [`plugin_graphql`] モジュールとして再エクスポートする（イシュー #435）。
//!   利用者は `fandhe-backend-plugin-graphql` へ直接依存せずに
//!   `fandhe_backend_core::plugin_graphql::GraphQlConfig` を構築できる。
//!   無効時（既定）は依存・コード・`unsafe` を一切含まない。
//! - `cors`（イシュー #305）: `crates/plugin-cors` を組み込み、
//!   [`server::Server::cors`] へ渡す設定型 `CorsConfig` を
//!   [`plugin_cors`] モジュールとして再エクスポートする（イシュー #435）。
//!   利用者は `fandhe-backend-plugin-cors` へ直接依存せずに
//!   `fandhe_backend_core::plugin_cors::CorsConfig` を構築できる。
//!   無効時（既定）は依存・コード・`unsafe` を一切含まない。
//! - `tracing`（TASK-10.1 / #56）: `crates/plugin-tracing` を組み込み、
//!   [`server::Server::tracing`] へ渡す設定型 `TracingConfig` を
//!   [`plugin_tracing`] モジュールとして再エクスポートする（イシュー #435）。
//!   利用者は `fandhe-backend-plugin-tracing` へ直接依存せずに
//!   `fandhe_backend_core::plugin_tracing::TracingConfig` を構築できる。
//!   無効時（既定）は依存・コード・`unsafe` を一切含まない。
//! - `openapi`（TASK-2.1 / #256）: `crates/plugin-openapi` を組み込み、
//!   [`server::Server::openapi_with`] へ渡す設定型 `OpenApiDoc` を
//!   [`plugin_openapi`] モジュールとして再エクスポートする（イシュー #435）。
//!   利用者は `fandhe-backend-plugin-openapi` へ直接依存せずに
//!   `fandhe_backend_core::plugin_openapi::OpenApiDoc` を構築できる。
//!   無効時（既定）は依存・コード・`unsafe` を一切含まない。
//! - `static`（イシュー #318）: `crates/plugin-static` を組み込み、
//!   [`server::Server::static_files`] へ渡す設定型 `StaticFilesConfig` を
//!   [`plugin_static`] モジュールとして再エクスポートする（イシュー #421）。
//!   利用者は `fandhe-backend-plugin-static` へ直接依存せずに
//!   `fandhe_backend_core::plugin_static::StaticFilesConfig` を構築できる。
//!   無効時（既定）は依存・コード・`unsafe` を一切含まない。
//! - `compression`（イシュー #321）: `crates/plugin-compression` を組み込み、
//!   [`server::Server::compression`] へ渡す設定型 `CompressionConfig` を
//!   [`plugin_compression`] モジュールとして再エクスポートする（イシュー #421）。
//!   利用者は `fandhe-backend-plugin-compression` へ直接依存せずに
//!   `fandhe_backend_core::plugin_compression::CompressionConfig` を構築できる。
//!   無効時（既定）は依存・コード・`unsafe` を一切含まない。
//!
//! `webrtc` / `webrtc-proxy` の設定型 `WebRtcConfig` / `ProxyConfig` も同様に
//! それぞれ [`plugin_webrtc`] / [`plugin_webrtc_proxy`] モジュールとして
//! 再エクスポートする（イシュー #435）。
//!
//! # 今後のタスクとの対応
//!
//! - TASK-2.1（#18）で確立したプラグイン境界パターン（feature flag + `dep:`
//!   構文、cfg-free なコアループ + 固定シグネチャシーム）は非公開 `plugin`
//!   モジュールの `try_intercept`（パスインターセプト型）・
//!   `try_handle_upgrade`（Upgrade 型、TASK-4.1 / #22 で実装）の 2 種として
//!   実証済みであり、後続プラグインは `docs/design/plugin-boundary.md` の
//!   適用手順に従って同パターンを踏襲する
//!
//! # クイックスタート（TASK-11.5 / #95、`docs/guide/getting-started.md` の裏取り）
//!
//! [`Server`] に [`fandhe_backend_routes::Router`] を 1 件登録するだけで最小サーバが
//! 組み立てられる。`bind` → `run` の流れは `crates/core/examples/minimal.rs`
//! （`cargo run --example minimal -p fandhe-backend-core` で実行可能）と同一。
//! 本 doc test は `no_run`（実際に listen はしない）だが `cargo test --doc` で
//! コンパイル可能性を機械検証する（AI ファースト保守性、
//! `.claude/rules/feature-modification.md` の「実装にはテスト追加を伴う」）。
//!
//! ```no_run
//! use fandhe_backend_core::Server;
//! use fandhe_backend_http::response::Response;
//! use fandhe_backend_routes::Router;
//!
//! # async fn quickstart() -> std::io::Result<()> {
//! let router = Router::new().route("GET", "/", |_head, _body| {
//!     Response::new(200, b"hello\n".to_vec())
//! });
//!
//! let server = Server::new().handler(router);
//! // 外部公開する場合は呼び出し側の責任でバインドアドレスを明示する
//! // （.claude/rules/security.md の攻撃表面最小化）。ここではループバック限定。
//! let bound = server.bind("127.0.0.1:3000").await?;
//! bound.run().await
//! # }
//! ```
//!
//! プロセス終了を `run()` の kill に頼らず、accept 停止 → in-flight
//! 完了待ち（上限時間・超過時強制クローズ）を伴って安全に停止したい場合は
//! [`server::BoundServer::run_until`]（イシュー #313）を使う。`run()` は
//! `run_until` への薄い委譲であり後方互換を維持する。利用例は
//! `crates/core/examples/graceful_shutdown.rs`、設計判断は
//! `docs/design/graceful-shutdown.md` を参照。

pub mod extension;
pub mod interceptor;
pub(crate) mod plugin;
pub mod server;
pub mod streaming;

// 3 拡張点はクレート直下からも参照できるよう re-export する。プラグイン側
// （`crates/plugin-*`）はこの再エクスポート経由で `fandhe_backend_core::Middleware`
// のように参照でき、`extension` モジュールの存在を意識せずに実装できる。
pub use extension::{GateContext, GateOutcome, Middleware, RequestGate, UpgradeHandler};

// ユーザー向けインターセプト・レスポンス改変拡張点（イシュー #420）。
// 既存 3 拡張点（Middleware/RequestGate/UpgradeHandler）で表現できないリダイレクト・
// レスポンス改変ユースケースの受け皿。詳細契約は `interceptor` モジュール doc を参照。
pub use interceptor::Interceptor;

// コアループの主要 API もクレート直下から参照できるよう re-export する。
// `handle_connection_with_peer_addr` は `RequestGate::check` へ実 peer address
// を伝搬させたいカスタム accept ループ向けの公開 API（イシュー #486）。
// `RebindHandle`（イシュー #485）は稼働中の `BoundServer::run_until` へ
// listener 差し替えを指示するハンドル（`BoundServer::rebind_handle` で取得）。
pub use server::{
    BoundServer, Handler, RebindHandle, Server, handle_connection, handle_connection_with_peer_addr,
};

// レスポンス側 chunked ストリーミング送信（イシュー #319）の opt-in API。
pub use streaming::{BodyWriter, StreamClosed, StreamingResponse};

/// 静的ファイル配信プラグイン（`crates/plugin-static`）の再エクスポート
/// （`static` feature 限定、イシュー #421）。
///
/// [`server::Server::static_files`] へ渡す [`StaticFilesConfig`]（および
/// `StaticFilesConfigBuilder` / `StaticConfigError` 等の付随型）を、
/// `fandhe-backend-plugin-static` への直接依存を追加せずに構築できるようにする
/// 唯一の目的で存在する薄い再エクスポート（whole-crate 形式。プラグイン側に
/// 型が増えても本モジュールの追随は不要）。ハンドラ本体
/// （`try_handle_static` 等）はコア内部の `crate::plugin` シームから
/// 呼ばれる実装詳細であり、本モジュール経由での利用は想定しない。
///
/// [`StaticFilesConfig`]: fandhe_backend_plugin_static::StaticFilesConfig
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::plugin_static::StaticFilesConfig;
///
/// let config = StaticFilesConfig::builder("/static", std::env::temp_dir()).build();
/// assert!(config.is_ok());
/// ```
#[cfg(feature = "static")]
pub use fandhe_backend_plugin_static as plugin_static;

/// レスポンス圧縮プラグイン（`crates/plugin-compression`）の再エクスポート
/// （`compression` feature 限定、イシュー #421）。
///
/// [`server::Server::compression`] へ渡す [`CompressionConfig`]（および
/// `CompressionConfigBuilder` 等の付随型）を、
/// `fandhe-backend-plugin-compression` への直接依存を追加せずに構築できる
/// ようにする唯一の目的で存在する薄い再エクスポート（`plugin_static` と同一の
/// whole-crate パターン）。圧縮適用本体（`apply_compression` 等）はコア内部の
/// `crate::plugin::finalize_response` シームから呼ばれる実装詳細であり、
/// 本モジュール経由での利用は想定しない。
///
/// [`CompressionConfig`]: fandhe_backend_plugin_compression::CompressionConfig
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::plugin_compression::CompressionConfig;
///
/// let config = CompressionConfig::builder().build();
/// let _ = config;
/// ```
#[cfg(feature = "compression")]
pub use fandhe_backend_plugin_compression as plugin_compression;

/// WebSocket プラグイン（`crates/plugin-websocket`）の再エクスポート
/// （`websocket` feature 限定、イシュー #435。#421 で `static` /
/// `compression` に導入したパターンの水平展開）。
///
/// [`server::Server::websocket`] へ渡す [`WebSocketConfig`]（および
/// `WsMessageHandler` 等の付随型）を、`fandhe-backend-plugin-websocket` への
/// 直接依存を追加せずに構築できるようにする唯一の目的で存在する薄い
/// 再エクスポート（`plugin_static` と同一の whole-crate パターン）。
/// アップグレードハンドラ本体（`handle_upgrade` 等）はコア内部の
/// `crate::server` の `UpgradeHandler` アダプタから呼ばれる実装詳細であり、
/// 本モジュール経由での利用は想定しない。
///
/// [`WebSocketConfig`]: fandhe_backend_plugin_websocket::WebSocketConfig
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::plugin_websocket::WebSocketConfig;
///
/// let config = WebSocketConfig::default();
/// assert_eq!(config.path, "/ws");
/// ```
#[cfg(feature = "websocket")]
pub use fandhe_backend_plugin_websocket as plugin_websocket;

/// GraphQL プラグイン（`crates/plugin-graphql`）の再エクスポート
/// （`graphql` feature 限定、イシュー #435）。
///
/// [`server::Server::graphql`] へ渡す [`GraphQlConfig`] を、
/// `fandhe-backend-plugin-graphql` への直接依存を追加せずに構築できるように
/// する唯一の目的で存在する薄い再エクスポート（`plugin_static` と同一の
/// whole-crate パターン）。クエリ実行本体（`try_handle_graphql` 等）はコア
/// 内部の `crate::plugin::try_intercept` シームから呼ばれる実装詳細であり、
/// 本モジュール経由での利用は想定しない。クエリ深さ・複雑度制限は
/// [`GraphQlConfig::new`] の doc のとおりスキーマ登録者（呼び出し元）の責務
/// のまま変わらない。
///
/// [`GraphQlConfig`]: fandhe_backend_plugin_graphql::GraphQlConfig
/// [`GraphQlConfig::new`]: fandhe_backend_plugin_graphql::GraphQlConfig::new
///
/// # Examples
///
/// ```
/// use async_graphql::Value;
/// use async_graphql::dynamic::{Field, FieldFuture, Object, Schema, TypeRef};
/// use fandhe_backend_core::plugin_graphql::GraphQlConfig;
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
#[cfg(feature = "graphql")]
pub use fandhe_backend_plugin_graphql as plugin_graphql;

/// CORS プラグイン（`crates/plugin-cors`）の再エクスポート
/// （`cors` feature 限定、イシュー #435）。
///
/// [`server::Server::cors`] へ渡す [`CorsConfig`]（および
/// `CorsConfigBuilder` 等の付随型）を、`fandhe-backend-plugin-cors` への
/// 直接依存を追加せずに構築できるようにする唯一の目的で存在する薄い
/// 再エクスポート（`plugin_static` と同一の whole-crate パターン）。ヘッダ
/// 付与本体（`apply_cors_headers` 等）はコア内部の
/// `crate::plugin::finalize_response` シームから呼ばれる実装詳細であり、
/// 本モジュール経由での利用は想定しない。プリフライト用 `preflight_response`
/// は利用者が `Router::options_fallback` へ直接配線する契約（`plugin-cors`
/// crate doc 参照）も本再エクスポート経由でそのまま利用できる。
///
/// [`CorsConfig`]: fandhe_backend_plugin_cors::CorsConfig
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::plugin_cors::CorsConfig;
///
/// let config = CorsConfig::builder()
///     .allow_origin("https://app.example.com")
///     .build();
/// assert!(config.is_ok());
/// ```
#[cfg(feature = "cors")]
pub use fandhe_backend_plugin_cors as plugin_cors;

/// 可観測性（トレーシング）プラグイン（`crates/plugin-tracing`）の再エクスポート
/// （`tracing` feature 限定、イシュー #435）。
///
/// [`server::Server::tracing`] へ渡す [`TracingConfig`] を、
/// `fandhe-backend-plugin-tracing` への直接依存を追加せずに構築できるように
/// する唯一の目的で存在する薄い再エクスポート（`plugin_static` と同一の
/// whole-crate パターン）。サンプリング・記録本体（`TracingLayer` 等）は
/// コア内部の `Middleware` アダプタから呼ばれる実装詳細であり、本モジュール
/// 経由での利用は想定しない。本再エクスポートはグローバルサブスクライバの
/// 初期化を一切行わない設定構築のみを提供する。
///
/// [`TracingConfig`]: fandhe_backend_plugin_tracing::TracingConfig
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::plugin_tracing::TracingConfig;
/// use std::num::NonZeroU64;
///
/// let config = TracingConfig::default();
/// assert_eq!(config.sample_interval, NonZeroU64::new(100).unwrap());
/// ```
#[cfg(feature = "tracing")]
pub use fandhe_backend_plugin_tracing as plugin_tracing;

/// OpenAPI ドキュメント配信プラグイン（`crates/plugin-openapi`）の
/// 再エクスポート（`openapi` feature 限定、イシュー #435）。
///
/// [`server::Server::openapi_with`] へ渡す [`OpenApiDoc`] を、
/// `fandhe-backend-plugin-openapi` への直接依存を追加せずに構築できるように
/// する唯一の目的で存在する薄い再エクスポート（`plugin_static` と同一の
/// whole-crate パターン）。`GET /openapi.json` / `GET /openapi.yaml` の
/// 配信本体はコア内部の `crate::plugin::try_intercept` シームから呼ばれる
/// 実装詳細であり、本モジュール経由での利用は想定しない。
///
/// [`OpenApiDoc`]: fandhe_backend_plugin_openapi::OpenApiDoc
/// [`server::Server::openapi_with`]: server::Server::openapi_with
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::plugin_openapi::OpenApiDoc;
///
/// let doc = OpenApiDoc::from_json(r#"{"openapi":"3.0.0"}"#);
/// assert!(doc.is_ok());
/// ```
#[cfg(feature = "openapi")]
pub use fandhe_backend_plugin_openapi as plugin_openapi;

/// in-process WebRTC プラグイン（`crates/plugin-webrtc`）の再エクスポート
/// （`webrtc` feature 限定、イシュー #435）。
///
/// [`server::Server::webrtc`] へ渡す [`WebRtcConfig`] を、
/// `fandhe-backend-plugin-webrtc` への直接依存を追加せずに構築できるように
/// する唯一の目的で存在する薄い再エクスポート（`plugin_static` と同一の
/// whole-crate パターン）。シグナリング処理本体（`try_handle_rtc_offer` 等）
/// はコア内部の `crate::plugin::try_intercept` シームから呼ばれる実装詳細
/// であり、本モジュール経由での利用は想定しない。`webrtc-proxy` feature も
/// 同時有効な場合は `webrtc-proxy` が優先される契約（`plugin::try_intercept`
/// の doc）は本再エクスポートの追加では変わらない。
///
/// [`WebRtcConfig`]: fandhe_backend_plugin_webrtc::WebRtcConfig
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::plugin_webrtc::WebRtcConfig;
///
/// let config = WebRtcConfig::new();
/// assert_eq!(config.max_offer_bytes(), 64 * 1024);
/// ```
#[cfg(feature = "webrtc")]
pub use fandhe_backend_plugin_webrtc as plugin_webrtc;

/// WebRTC シグナリングプロキシプラグイン（`crates/plugin-webrtc-proxy`）の
/// 再エクスポート（`webrtc-proxy` feature 限定、イシュー #435）。
///
/// [`server::Server::webrtc_proxy`] へ渡す [`ProxyConfig`] を、
/// `fandhe-backend-plugin-webrtc-proxy` への直接依存を追加せずに構築できる
/// ようにする唯一の目的で存在する薄い再エクスポート（`plugin_static` と同一
/// の whole-crate パターン）。転送処理本体（`try_handle_rtc_offer` /
/// `forward_offer` 等）はコア内部の `crate::plugin::try_intercept` シームから
/// 呼ばれる実装詳細であり、本モジュール経由での利用は想定しない。上流
/// アドレスをリクエスト由来の値で決めない SSRF 対策（`ProxyConfig` の doc）
/// も本再エクスポート経由でそのまま維持される。
///
/// [`ProxyConfig`]: fandhe_backend_plugin_webrtc_proxy::ProxyConfig
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::plugin_webrtc_proxy::ProxyConfig;
///
/// let config = ProxyConfig::new("127.0.0.1:9000");
/// assert_eq!(config.upstream_addr(), "127.0.0.1:9000");
/// ```
#[cfg(feature = "webrtc-proxy")]
pub use fandhe_backend_plugin_webrtc_proxy as plugin_webrtc_proxy;

/// このクレートのバージョン文字列を返す。
///
/// TASK-1.1 時点では workspace のビルド・doc test が機能する状態を確認するための
/// 最小公開 API として存在する。以降のタスクで実体実装（HTTP コア）に置き換わる過程でも、
/// `cargo test` が本クレートに対して何かを検証し続けられるようにするための足場。
///
/// # Examples
///
/// ```
/// let version = fandhe_backend_core::version();
/// assert!(!version.is_empty());
/// ```
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        assert!(!version().is_empty());
    }
}
