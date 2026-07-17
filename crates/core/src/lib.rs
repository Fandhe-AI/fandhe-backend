//! backend-framework の最小コア。
//!
//! # このクレートの役割
//!
//! `crates/core` は HTTP/1.1 パーサ・keep-alive・3 種拡張点
//! （`Middleware` / `UpgradeHandler` / `RequestGate`、[`extension`] モジュール）を
//! 実装する最小コアの置き場所。TASK-1.4-1（#69）で 3 拡張点の trait 定義を、
//! TASK-1.4-2（#70）でコアループ（接続受理・リクエストループ、[`server`]
//! モジュール）による実接続を提供する。
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
//! `bf-http`（sans-IO な HTTP/1.1 パーサ、TASK-1.3）は workspace 内の下位層クレート
//! であり、外部 crates.io 依存はここでも増やさない。
//!
//! `crates/routes`（`bf-routes`）は TASK-1.5（#14）で新設し、`server → routes`
//! エッジを `Cargo.toml` の依存宣言として実体化した。`bf_routes::Router` は
//! `impl` [`server::Handler`] `for` `bf_routes::Router` により本クレートの
//! 既定ハンドラとしてそのまま登録できる（`crates/core/examples/minimal.rs`
//! 参照）。依存方向の機械検証は `scripts/dep-direction-check.sh`（TASK-1.5）が
//! `cargo metadata` の依存エッジホワイトリスト照合で行う。
//!
//! # feature 一覧
//!
//! - `webrtc-proxy`（TASK-2.1 / #18）: `crates/plugin-webrtc-proxy` を
//!   `optional = true` + `dep:` 構文で組み込み、[`server::Server::webrtc_proxy`]
//!   で登録した `bf_plugin_webrtc_proxy::ProxyConfig` に基づき `POST
//!   /rtc/offer` をパスインターセプトする。無効時（既定）は依存・コード・
//!   `unsafe` を一切含まない（クレート非公開の `plugin` モジュールの doc・
//!   `docs/design/plugin-boundary.md` を参照）。
//! - `webrtc`（TASK-8.1 / #26）: `crates/plugin-webrtc`（in-process 型、
//!   `webrtc-rs` 直接依存）を同じくプラグイン境界パターンで組み込み、
//!   [`server::Server::webrtc`] で登録した `bf_plugin_webrtc::WebRtcConfig` に
//!   基づき `POST /rtc/offer` をパスインターセプトする。`webrtc-proxy` と
//!   同時有効時は `webrtc-proxy` が優先される（`plugin::try_intercept` の
//!   doc）。無効時（既定）は依存・コード・`unsafe` を一切含まない。
//!
//! # 今後のタスクとの対応
//!
//! - `server` モジュール内の `try_handle_upgrade` ヘルパーは、実 WebSocket
//!   プラグイン（TASK-4.1）配線までは常に委譲なしのスタブのまま残る。
//!   TASK-2.1（#18）で確立したプラグイン境界パターン（feature flag + `dep:`
//!   構文、cfg-free なコアループ + 固定シグネチャシーム）は非公開 `plugin`
//!   モジュールの `try_intercept` によるパスインターセプト型として実証済みで
//!   あり、Upgrade 型（`try_handle_upgrade`）を含む後続プラグインは
//!   `docs/design/plugin-boundary.md` の適用手順に従って同パターンを踏襲する

pub mod extension;
pub(crate) mod plugin;
pub mod server;

// 3 拡張点はクレート直下からも参照できるよう re-export する。プラグイン側
// （`crates/plugin-*`）はこの再エクスポート経由で `backend_framework_core::Middleware`
// のように参照でき、`extension` モジュールの存在を意識せずに実装できる。
pub use extension::{GateOutcome, Middleware, RequestGate, UpgradeHandler};

// コアループの主要 API もクレート直下から参照できるよう re-export する。
pub use server::{BoundServer, Handler, Server, handle_connection};

/// このクレートのバージョン文字列を返す。
///
/// TASK-1.1 時点では workspace のビルド・doc test が機能する状態を確認するための
/// 最小公開 API として存在する。以降のタスクで実体実装（HTTP コア）に置き換わる過程でも、
/// `cargo test` が本クレートに対して何かを検証し続けられるようにするための足場。
///
/// # Examples
///
/// ```
/// let version = backend_framework_core::version();
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
