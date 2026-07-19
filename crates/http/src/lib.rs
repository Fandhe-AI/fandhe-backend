//! `fandhe-backend-http`: backend-framework の最小 HTTP コア。
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
//! 本クレートはこのグラフの末端であり、上位層（ルーティング・プラグイン）の
//! シンボルには一切依存しない（`scripts/dep-direction-check.sh` が機械検証する）。
//! TASK-1.3（#12）の分解:
//! 1. sans-IO なリクエストヘッドパーサ（[`request`]、TASK-1.3-1 / #66）
//! 2. body フレーミング解釈（[`body`]）・keep-alive 判定・ソケット読み取り
//!    ループ（[`connection`]、TASK-1.3-2 / #67）
//! 3. 読み取りバッファの接続単位再利用（[`buffer`]）・`TCP_NODELAY` 最適化
//!    （[`socket`]、feature `net` 前提、TASK-1.3-3 / #68）
//! 4. HTTP/1.1 レスポンス直列化（[`response`]、TASK-1.4-2 / #70）。コアの
//!    接続ループ（`crates/core/src/server.rs`）が唯一の呼び出し元。
//! 5. chunked transfer-coding のデコード（[`chunked`]、イシュー #181）。
//!    [`body`] が `Transfer-Encoding: chunked` を受理した場合にのみ
//!    [`connection::read_request`] から呼ばれる sans-IO 状態機械。
//!
//! 本クレートの実行時依存は tokio の `io-util`（`AsyncRead`/`AsyncReadExt`）
//! のみであり、それ以外の依存は持たない（pay-for-what-you-use。
//! `crates/http/Cargo.toml` 参照）。オプション feature `net` を有効化すると
//! tokio `net` のみが追加され、[`socket`] モジュールが公開される。
//!
//! エラー型に `thiserror` 等を使わず手実装しているのは、依存最小化を優先した
//! ため。コア全体のエラー設計は TASK-1.4（#13）で再検討する。

pub mod body;
pub mod buffer;
pub mod chunked;
pub mod connection;
pub mod request;
pub mod response;
#[cfg(feature = "net")]
pub mod socket;
