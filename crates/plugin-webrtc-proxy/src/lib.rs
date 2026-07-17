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
//! ループ（TASK-1.4-2 / #70）は、`webrtc-proxy` feature（TASK-2.1 / #18、
//! `backend_framework_core::server::Server::webrtc_proxy` で `ProxyConfig` を
//! 登録）を有効化した際に、既定 `Handler` より先に非公開の
//! `plugin::try_intercept` シームから [`handler::try_handle_rtc_offer`] を
//! 呼び出す形で配線済みである（`docs/design/plugin-boundary.md` の
//! プラグイン境界パターン第 1 号）。`webrtc-proxy` feature 無効時（既定）は
//! 本クレート自体が `backend-framework-core` の依存グラフから除外される
//! （`optional = true` + `dep:` 構文）。
//!
//! # workspace 内での依存方向
//!
//! `docs/spec/04-requirements.md` REQ-1 / `docs/spec/05-tasks.md` TASK-11.1 の方針に従い、
//! workspace 全体の依存方向は次の一方向を維持する（依存方向: server → routes → http::*）。
//! 本クレートはプラグイン層（`bf-plugin-*`）に位置し、コアの拡張点を実装する側であり、
//! コア（`backend-framework-core`）・`bf-routes` からプラグインへの逆依存は発生しない
//! （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。本クレートの
//! workspace 内 path 依存は `bf-http`（下位層の sans-IO パーサ）のみで、`webrtc-rs` は
//! 上述のとおり依存に含めない。依存方向の機械検証は `scripts/dep-direction-check.sh`
//! （TASK-1.5 / TASK-11.1）が担う。
//!
//! # Examples
//!
//! [`ProxyConfig`] を構築し、[`try_handle_rtc_offer`] へパスインターセプト
//! させる最小の利用例（対象外パスなので `None` が返り、無関係なリクエストへの
//! 性能影響がないことを示す）。
//!
//! ```
//! use bf_http::request::{parse_request_head, ParseOutcome};
//! use bf_plugin_webrtc_proxy::{ProxyConfig, try_handle_rtc_offer};
//!
//! let buf = b"GET /health HTTP/1.1\r\n\r\n";
//! let head = match parse_request_head(buf).unwrap() {
//!     ParseOutcome::Complete { head, .. } => head,
//!     ParseOutcome::Incomplete => unreachable!(),
//! };
//! let config = ProxyConfig::new("127.0.0.1:9000");
//!
//! let runtime = tokio::runtime::Runtime::new().unwrap();
//! let result = runtime.block_on(try_handle_rtc_offer(&head, b"", &config));
//! assert!(result.is_none());
//! ```

pub mod client;
pub mod config;
pub mod error;
pub mod handler;

pub use config::ProxyConfig;
pub use error::ProxyError;
pub use handler::{Response, try_handle_rtc_offer};
