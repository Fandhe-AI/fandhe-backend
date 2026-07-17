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
//! # 今後のタスクとの対応
//!
//! - TASK-1.5（#14）: 依存方向一方向性の機械的検証・`crates/routes` 新設
//!   （現状 [`server::Handler`] が暫定的に既定レスポンダを担う）
//! - TASK-2.1（#18）: feature flag + `dep:` 構文によるプラグイン境界の確立。
//!   `server` モジュール内の `try_handle_upgrade` ヘルパーは本タスクで
//!   実プラグインへの委譲実装に差し替わるシーム

pub mod extension;
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
