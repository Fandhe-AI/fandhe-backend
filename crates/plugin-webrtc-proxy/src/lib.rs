//! `bf-plugin-webrtc-proxy`: WebRTC シグナリングプロキシプラグイン（TASK-8.2-2 / #74）。
//!
//! # 背景・別プロセス切り出し文脈
//!
//! `webrtc-rs`（0.17.1 系）は依存クレート +189・バイナリサイズ約 10.4 倍・
//! `unsafe` Functions 約 2.2 倍という桁違いの依存インパクトを持つ（PoC-5、
//! docs/spec の Conditional Go 条件(2)）。REQ-8 / TASK-8.2 はこれを踏まえ、
//! WebRTC を要するサービスを **別プロセス・別サービスへ切り出す** ことを MVP の
//! 推奨設計とし、本クレートはフレームワーク側で提供する
//! 「切り出した外部 WebRTC サービスとの連携（軽量シグナリングプロキシ）」を担う。
//!
//! `crates/plugin-webrtc`（TASK-8.1、in-process 実装・`webrtc-rs` 依存）とは
//! **クレート境界で完全に分離** しており、本クレートは `webrtc-rs` に一切依存しない
//! （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。
//! `cargo tree -p bf-plugin-webrtc-proxy` で `webrtc` 系依存が現れないことを
//! 機械的に検証できる。
//!
//! # 動作モデル
//!
//! PoC-5 / TASK-8.1 と同型の「リクエスト/レスポンス完結型ハンドラ」パターンを
//! 踏襲し、新しい拡張点は追加しない。[`handler::try_handle_rtc_offer`] が
//! `POST /rtc/offer` をパスインターセプトで受け、[`client::forward_offer`] が
//! 静的設定された上流 WebRTC サービスへ HTTP/1.1 で中継する。
//!
//! 上流アドレスはビルド時・起動時の [`config::ProxyConfig`] による静的設定のみで
//! 決定し、リクエスト内容からは導出しない（SSRF 防止、.claude/rules/security.md）。
//!
//! # コアループへの配線について
//!
//! 本クレート単体では HTTP サーバのリスンループを持たない。コアの接続受理
//! ループ（TASK-1.4-2 / #70）・feature 配線規約（TASK-2.1 / #18）が確立し
//! 次第、上位クレート（server 相当）から [`handler::try_handle_rtc_offer`] を
//! 呼び出す形で配線する想定であり、本タスク（#74）のスコープはハンドラ本体の
//! 実装・単体/統合テストまでとする。

pub mod client;
pub mod config;
pub mod error;
pub mod handler;

pub use config::ProxyConfig;
pub use error::ProxyError;
pub use handler::{Response, try_handle_rtc_offer};
