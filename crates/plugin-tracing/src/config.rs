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
/// （[`crate::Sampler`] の doc を参照）。`exclude_paths`（TASK-10.3、#58）に
/// 完全一致するパスはサンプリング判定より前に除外され、記録もサンプラーの
/// カウンタ消費も発生しない（[`crate::TracingLayer::record_response`] の doc
/// を参照）。
#[derive(Debug, Clone)]
pub struct TracingConfig {
    /// サンプリング間隔。`1` で全件記録、既定は `100`（100 リクエストに 1 回）。
    pub sample_interval: NonZeroU64,

    /// 記録対象から除外するパスの集合（クエリ文字列除去後のパスと完全一致で
    /// 照合、TASK-10.3 / #58）。
    ///
    /// ヘルスチェック等の高頻度パスをここに登録すると、`record_response` は
    /// サンプラーの `AtomicU64` カウンタを消費する前に即座に return する
    /// （高頻度パスの記録コスト削減と、他パスのサンプリング周期を歪めない
    /// ことの両方が目的）。既定は空（従来どおり全パスがサンプリング対象）。
    ///
    /// 照合セマンティクス: バイト単位の完全一致。大文字小文字は区別し、
    /// 末尾スラッシュの有無も別パス扱いとする（`/health` と `/health/` は
    /// 同一視しない）。プレフィックス一致・glob は意図的に非対応
    /// （ログ抑制範囲の意図しない拡大＝可観測性の穴を防ぐ安全側の設計、
    /// `.claude/rules/security.md` の可観測性観点）。エントリ数は通常
    /// 数件程度を想定し、線形走査で照合する。
    pub exclude_paths: Vec<String>,
}

impl Default for TracingConfig {
    /// `sample_interval = 100`（REQ-10 の例示値）・`exclude_paths` 空を既定とする。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_tracing::TracingConfig;
    /// use std::num::NonZeroU64;
    ///
    /// let config = TracingConfig::default();
    /// assert_eq!(config.sample_interval, NonZeroU64::new(100).unwrap());
    /// assert!(config.exclude_paths.is_empty());
    /// ```
    fn default() -> Self {
        Self {
            // SAFETY ではなく単なる定数のため unwrap 可。DEFAULT_SAMPLE_INTERVAL は
            // 非ゼロのリテラルであることがコード上自明（`.claude/rules/coding-rust.md`
            // の `.unwrap()` 回避方針はライブラリの実行時パスに適用され、コンパイル
            // 時に不変条件が保証される定数初期化には適用しない）。
            sample_interval: NonZeroU64::new(DEFAULT_SAMPLE_INTERVAL)
                .expect("DEFAULT_SAMPLE_INTERVAL は非ゼロ定数"),
            exclude_paths: Vec::new(),
        }
    }
}

impl TracingConfig {
    /// サンプリング間隔を指定して設定を作る（`exclude_paths` は空で始まる）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_tracing::TracingConfig;
    /// use std::num::NonZeroU64;
    ///
    /// let config = TracingConfig::new(NonZeroU64::new(1).unwrap());
    /// assert_eq!(config.sample_interval, NonZeroU64::new(1).unwrap());
    /// assert!(config.exclude_paths.is_empty());
    /// ```
    #[must_use]
    pub const fn new(sample_interval: NonZeroU64) -> Self {
        Self {
            sample_interval,
            exclude_paths: Vec::new(),
        }
    }

    /// 記録対象から除外するパスを 1 件追記する（チェーン可能、TASK-10.3 / #58）。
    ///
    /// 完全一致照合の契約は [`TracingConfig::exclude_paths`] を参照。
    /// 呼び出し元は `Server::tracing(config)`（`crates/core/src/server.rs`、
    /// `tracing` feature 限定）にそのまま渡せる。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_tracing::TracingConfig;
    ///
    /// let config = TracingConfig::default()
    ///     .exclude_path("/health")
    ///     .exclude_path("/metrics");
    /// assert_eq!(config.exclude_paths, vec!["/health", "/metrics"]);
    /// ```
    #[must_use]
    pub fn exclude_path(mut self, path: impl Into<String>) -> Self {
        self.exclude_paths.push(path.into());
        self
    }
}
