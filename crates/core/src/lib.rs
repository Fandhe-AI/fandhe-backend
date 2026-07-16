//! backend-framework の最小コア（TASK-1.1 時点は placeholder）。
//!
//! # このクレートの役割
//!
//! `crates/core` は HTTP/1.1 パーサ・keep-alive・3 種拡張点
//! （`Middleware` / `UpgradeHandler` / `RequestGate`）を実装する最小コアの置き場所。
//! TASK-1.1（`cargo workspace`・CI 基盤整備）時点では実体を持たず、以降のタスクの
//! 受け皿として最小構成（依存 0 件・`unsafe` 0 件）を維持する。
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
//!
//! # 今後のタスクとの対応
//!
//! - TASK-1.3: HTTP/1.1 パーサ・keep-alive・バッファ再利用の実体実装
//! - TASK-1.4: `Middleware` / `UpgradeHandler` / `RequestGate` の trait 定義とコアループ
//! - TASK-1.5: 依存方向一方向性の機械的検証

/// このクレートのバージョン文字列を返す。
///
/// TASK-1.1 時点では workspace のビルド・doc test が機能する状態を確認するための
/// 最小公開 API として存在する。以降のタスクで実体実装（HTTP コア）に置き換わる過程でも、
/// `cargo test` が本クレートに対して何かを検証し続けられるようにするための足場。
///
/// # 例
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
