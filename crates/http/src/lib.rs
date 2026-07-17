//! `bf-http`: backend-framework の最小 HTTP コア。
//!
//! 依存方向は `server` → `routes` → `http::*` の末端であり、本クレートは上位層
//! （ルーティング・プラグイン）のシンボルに依存しない。TASK-1.3（#12）の分解:
//! 1. sans-IO なリクエストヘッドパーサ（[`request`]、TASK-1.3-1 / #66）
//! 2. body フレーミング解釈（[`body`]）・keep-alive 判定・ソケット読み取り
//!    ループ（[`connection`]、TASK-1.3-2 / #67）
//! 3. 読み取りバッファの接続単位再利用（[`buffer`]）・`TCP_NODELAY` 最適化
//!    （[`socket`]、feature `net` 前提、TASK-1.3-3 / #68 = 本クレート現在地）
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
pub mod connection;
pub mod request;
#[cfg(feature = "net")]
pub mod socket;
