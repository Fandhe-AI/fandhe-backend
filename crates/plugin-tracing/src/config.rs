//! トレーシングプラグインの設定型（TASK-10.1）。
//!
//! [`TracingConfig`] はコア側の `Server::tracing(config)`（`tracing` feature
//! 限定 API、`crates/core/src/server.rs`）が受け取り、[`crate::TracingLayer`] の
//! サンプリング間隔として使う。

use std::num::NonZeroU64;

/// REQ-10 の例示値（「100 リクエストに 1 回」）をそのまま既定値とする。
///
/// 具体値の最終決定は性能再検証（TASK-10.4）に委ねる。値そのものの妥当性は
/// 本タスクのスコープ外（計画書 8 節）。
const DEFAULT_SAMPLE_INTERVAL: u64 = 100;

/// [`crate::TracingLayer`]（コアの `Middleware` アダプタから呼ばれる）の設定。
///
/// `sample_interval` 件のリクエストに 1 件のみ span/event を記録する
/// （[`crate::Sampler`] の doc を参照）。
#[derive(Debug, Clone)]
pub struct TracingConfig {
    /// サンプリング間隔。`1` で全件記録、既定は `100`（100 リクエストに 1 回）。
    pub sample_interval: NonZeroU64,
}

impl Default for TracingConfig {
    /// `sample_interval = 100`（REQ-10 の例示値）を既定とする。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_tracing::TracingConfig;
    /// use std::num::NonZeroU64;
    ///
    /// let config = TracingConfig::default();
    /// assert_eq!(config.sample_interval, NonZeroU64::new(100).unwrap());
    /// ```
    fn default() -> Self {
        Self {
            // SAFETY ではなく単なる定数のため unwrap 可。DEFAULT_SAMPLE_INTERVAL は
            // 非ゼロのリテラルであることがコード上自明（`.claude/rules/coding-rust.md`
            // の `.unwrap()` 回避方針はライブラリの実行時パスに適用され、コンパイル
            // 時に不変条件が保証される定数初期化には適用しない）。
            sample_interval: NonZeroU64::new(DEFAULT_SAMPLE_INTERVAL)
                .expect("DEFAULT_SAMPLE_INTERVAL は非ゼロ定数"),
        }
    }
}

impl TracingConfig {
    /// サンプリング間隔を指定して設定を作る。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_tracing::TracingConfig;
    /// use std::num::NonZeroU64;
    ///
    /// let config = TracingConfig::new(NonZeroU64::new(1).unwrap());
    /// assert_eq!(config.sample_interval, NonZeroU64::new(1).unwrap());
    /// ```
    #[must_use]
    pub const fn new(sample_interval: NonZeroU64) -> Self {
        Self { sample_interval }
    }
}
