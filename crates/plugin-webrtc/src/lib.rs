//! `bf-plugin-webrtc`: in-process WebRTC プラグイン（TASK-8.1、#26）。
//!
//! # 背景・別プロセス切り出しとの対照
//!
//! `webrtc-rs`（0.17.1 系）は依存クレート +189・バイナリサイズ約 10.4 倍・
//! `unsafe` Functions 約 2.2 倍という桁違いの依存インパクトを持つ（PoC-5、
//! docs/spec の Conditional Go 条件(2)）。REQ-8 / `docs/spec/05-tasks.md` TASK-8.2 は
//! これを踏まえ、WebRTC を要するサービスを **別プロセス・別サービスへ切り出す** ことを
//! MVP の推奨設計とし、`crates/plugin-webrtc-proxy`（TASK-8.2-2）がその軽量シグナリング
//! プロキシを担う。
//!
//! 本クレートはそれとは対照的に、**プロセス内で完結する WebRTC ハンドラ実装**
//! （TASK-8.1）を提供する。1 対 1 のデータチャネル確立とメッセージ往復（エコー）の
//! 最小疎通を実現するが、`webrtc-rs` 依存を直接コアプロセスへ持ち込むぶん攻撃表面が
//! 大きい（下記「セキュリティ上の位置づけ」を参照）。`crates/plugin-webrtc-proxy` とは
//! **クレート境界で完全に分離** しており、相互に依存しない
//! （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。
//!
//! # 動作モデル
//!
//! `crates/plugin-webrtc-proxy`・PoC-5（`docs/spec/03-poc/webrtc-plugin/`）と同型の
//! 「リクエスト/レスポンス完結型ハンドラ（パスインターセプト）」パターンを踏襲し、
//! 新しい拡張点は追加しない。[`handler::try_handle_rtc_offer`] が `POST /rtc/offer` を
//! パスインターセプトで受け、SDP Offer から `RTCPeerConnection` を生成、データチャネル
//! 到着を待ち受けるエコーハンドラを登録したうえで、非トリクル ICE による SDP Answer を
//! 返す。
//!
//! WebRTC の DataChannel は SDP 交換後、SCTP/DTLS/ICE の独立した UDP セッション上で
//! 動作し、シグナリングに使った HTTP/TCP 接続そのものを奪取する必要がない。そのため
//! [`crate::handler::try_handle_rtc_offer`] は `UpgradeHandler`（接続奪取）ではなく、
//! GraphQL・`plugin-webrtc-proxy` と同型の「一問一答で完結するプラグイン」として
//! 実装する。
//!
//! # コアループへの配線について
//!
//! 本クレート単体では HTTP サーバのリスンループを持たない。コアの接続受理ループ
//! （TASK-1.4-2 / #70）は、`webrtc` feature（TASK-2.1 / #18 で確立したプラグイン境界
//! パターンに従い、`backend_framework_core::server::Server::webrtc` で
//! [`WebRtcConfig`] を登録）を有効化した際に、既定 `Handler` より先に非公開の
//! `plugin::try_intercept` シームから [`handler::try_handle_rtc_offer`] を呼び出す形で
//! 配線される（`docs/design/plugin-boundary.md` のプラグイン境界パターン）。
//! `webrtc` feature 無効時（既定）は本クレート自体が `backend-framework-core` の
//! 依存グラフから除外される（`optional = true` + `dep:` 構文）。
//!
//! `webrtc-proxy` feature（別プロセス切り出し型）と `webrtc` feature（本クレート、
//! in-process 型）は `--all-features` CI のため共存コンパイル可能とするが、
//! `crate::plugin::try_intercept` は `webrtc-proxy` を先に評価する（REQ-8 の推奨方式を
//! 優先する運用判断。両方を同時に `Server` へ登録した場合は `webrtc-proxy` が優先され、
//! `webrtc` 側の設定は評価されない）。
//!
//! # セキュリティ上の位置づけ（in-process 有効化の攻撃表面）
//!
//! 本プラグインを有効化すると、`webrtc-rs` の巨大な依存グラフ・パーサ群がコア
//! プロセスに直接組み込まれる。ICE 接続性チェックはクライアント SDP 由来のアドレスへ
//! UDP 送信を発生させ得る（WebRTC の構造上不可避）。本タスクでは STUN/TURN を一切
//! 設定せず（`RTCConfiguration::default()`）、この特性を踏まえたうえで
//! **「in-process 有効化は攻撃表面を大きく広げるため、別プロセス切り出し
//! （`crates/plugin-webrtc-proxy`）を MVP 推奨とする」** という REQ-8 の方針を維持する。
//! 詳細な攻撃表面評価は TASK-8.4（#29）のスコープ。
//!
//! # 依存インパクトの計測
//!
//! `cargo tree -p backend-framework-core` で `webrtc` feature 無効時に `webrtc` 系依存が
//! 一切現れないこと、有効時の依存インパクトは `docs/dep-impact/records.md` に記録する
//! （`scripts/dep-impact.sh`）。
//!
//! # workspace 内での依存方向
//!
//! `docs/spec/04-requirements.md` REQ-1 / `docs/spec/05-tasks.md` TASK-11.1 の方針に従い、
//! workspace 全体の依存方向は次の一方向を維持する（依存方向: server → routes → http::*）。
//! 本クレートはプラグイン層（`bf-plugin-*`）に位置し、コアの拡張点を実装する側であり、
//! コア（`backend-framework-core`）・`bf-routes` からプラグインへの逆依存は発生しない
//! （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。本クレートの
//! workspace 内 path 依存は `bf-http`（下位層の sans-IO パーサ）のみ。依存方向の機械
//! 検証は `scripts/dep-direction-check.sh`（TASK-1.5 / TASK-11.1）が担う。
//!
//! # Examples
//!
//! [`WebRtcConfig`] を構築し、[`try_handle_rtc_offer`] へパスインターセプトさせる
//! 最小の利用例（対象外パスなので `None` が返り、無関係なリクエストへの性能影響が
//! ないことを示す）。
//!
//! ```
//! use bf_http::request::{parse_request_head, ParseOutcome};
//! use bf_plugin_webrtc::{WebRtcConfig, try_handle_rtc_offer};
//!
//! let buf = b"GET /health HTTP/1.1\r\n\r\n";
//! let head = match parse_request_head(buf).unwrap() {
//!     ParseOutcome::Complete { head, .. } => head,
//!     ParseOutcome::Incomplete => unreachable!(),
//! };
//! let config = WebRtcConfig::new();
//!
//! let runtime = tokio::runtime::Runtime::new().unwrap();
//! let result = runtime.block_on(try_handle_rtc_offer(&head, b"", &config));
//! assert!(result.is_none());
//! ```

pub mod config;
pub mod handler;

pub use config::WebRtcConfig;
pub use handler::{OFFER_PATH, try_handle_rtc_offer};
