//! backend-framework の可観測性（トレーシング）プラグイン（TASK-10.1、REQ-10）。
//!
//! 拡張点対応: Middleware
//! （拡張点定義: `crates/core/src/extension.rs`）
//!
//! # 背景・既知の制約
//!
//! PoC-10（`docs/spec/04-requirements.md` REQ-10）の実測により、可観測性
//! ミドルウェアをサンプリングなしで有効化すると RPS が最大 63.0% 劣化し、
//! 非同期 I/O 化のみでも 31.6% 劣化・p95 が 61.7% 悪化することが判明している。
//! そのため REQ-10 は可観測性を「サンプリング前提のオプトイン」として再定義した。
//! 本クレートはこの前提のもと、[`Sampler`] による決定的サンプリングと
//! [`init_tracing`] による非同期・バッファ済み I/O を既定として提供する
//! （AGENTS.md「規約: ミドルウェア非同期 I/O 必須化」）。
//!
//! さらに TASK-10.3（#58）で、ヘルスチェック等の高頻度パスを記録対象から
//! 完全一致で除外する機構（[`TracingConfig::exclude_path`]）を追加した。
//! 除外照合はサンプラーの `AtomicU64` カウンタ判定より前に行われ、除外対象
//! パスは記録コストだけでなくサンプリング周期の消費も回避する
//! （[`TracingLayer::record_response`] の doc を参照）。TASK-10.4 の性能
//! 再検証（RPS 劣化 5% 以内）の前提となる。
//!
//! # 責務境界
//!
//! - [`Sampler`][]: 一定割合のみ記録を許可する決定的カウンタ判定
//! - [`TracingConfig`][]: サンプリング間隔・除外パス設定
//! - [`TracingLayer`][]: 除外照合 + サンプリング判定 + 応答時 1 イベントへの記録実行本体
//!   （TASK-10.2 / #57 で span+2 イベントから統合）
//! - [`init_tracing`] / [`TracingOutput`][]: 非同期・バッファ済み I/O のグローバル
//!   サブスクライバ初期化ヘルパー
//!
//! # 接続契約（pay-for-what-you-use、`.claude/rules/pay-for-what-you-use.md`）
//!
//! 本クレートは `crates/core` に依存しない。`bf-plugin-websocket` と同一の
//! 非循環パターンを踏襲し（`crates/plugin-websocket/src/lib.rs` の doc を
//! 参照）、`Middleware` trait を実装するアダプタ（`TracingMiddleware`）はコア側
//! （`crates/core/src/server.rs`、`tracing` feature 限定）に置く。本クレートは
//! `bf-http::request::RequestHead` の参照 + `tracing` 系クレートへの委譲のみを
//! 提供し、`crates/core` から optional dependency（`dep:bf-plugin-tracing` +
//! `tracing` feature）として配線される。feature 無効時は本クレート・
//! `tracing` / `tracing-subscriber` / `tracing-appender` のいずれも
//! `cargo tree -p backend-framework-core` に現れない。
//!
//! workspace 全体の依存方向は次の一方向を維持する（依存方向: server → routes → http::*）。
//! 本クレートはプラグイン層（`bf-plugin-*`）に位置し、`crates/core` への逆依存は
//! 発生しない（`scripts/dep-direction-check.sh` が機械検証する）。
//!
//! # セキュリティ（OWASP Top 10、`.claude/rules/security.md`）
//!
//! [`TracingLayer::record_response`] が記録するフィールドは method・path・
//! elapsed_ms の 3 つに限定する。ヘッダ値（`Authorization` / `Cookie` 等）・
//! ボディ・クエリ文字列は一切記録しない契約とする（機密情報・PII 漏洩の防止）。

mod config;
mod init;
mod layer;
mod sampler;

pub use config::TracingConfig;
pub use init::{TracingOutput, init_tracing};
pub use layer::TracingLayer;
pub use sampler::Sampler;
