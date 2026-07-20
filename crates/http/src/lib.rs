//! `fandhe-backend-http`: fandhe-backend の最小 HTTP コア。
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
//! 6. クエリ文字列 key-value パーサ（[`query`]、イシュー #306）。
//!    [`request::RequestHead::query`] が返す生文字列を受け取り、`&`/`=` へ
//!    分解する sans-IO 純関数。呼び出し元（`crates/routes` のハンドラ・
//!    `crates/plugin-*`）が個別実装していた同型コードの重複を解消する。
//! 7. `Set-Cookie` ヘッダの構築時検証済みヘルパ（[`cookie`]、イシュー #303）。
//!    [`response::Response::with_header`] の汎用検証（CR/LF/NUL 拒否）だけでは
//!    カバーしない RFC 6265 cookie-name / cookie-value の文法検証を、
//!    構築時検証済み専用型 [`cookie::SetCookie`] として提供する
//!    （認証・セッション実装、親イシュー #296 の前提整備）。
//! 8. percent-decode ヘルパ（[`percent`]、イシュー #307）。ルーティング照合の
//!    非デコード契約（[`request::RequestHead::path`]）は変えず、ハンドラが
//!    照合確定後に明示的に呼ぶ場合のみデコードする opt-in 純関数。
//! 9. `application/x-www-form-urlencoded` ボディパーサ（[`form`]、イシュー
//!    #308）。[`query`]・[`percent`] を合成し、`+` → 空白変換を含む
//!    form-urlencoded 固有のデコード仕様・DoS 上限・Content-Type 検証ヘルパを
//!    提供する sans-IO 純関数。
//! 9. Cookie ヘッダ読み取りパーサ（[`cookie`]、イシュー #309）。RFC 6265
//!    cookie-pair 構文に準拠して `Cookie` ヘッダ値を key-value 組へ分解する
//!    sans-IO 純関数。[`request::RequestHead::cookies`] が複数 `Cookie`
//!    ヘッダの結合・累積 DoS 上限適用を担う。
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
pub mod cookie;
pub mod form;
pub mod percent;
pub mod query;
pub mod request;
pub mod response;
#[cfg(feature = "net")]
pub mod socket;
