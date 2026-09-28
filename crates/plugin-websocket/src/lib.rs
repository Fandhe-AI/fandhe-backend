//! `fandhe-backend-plugin-websocket`: WebSocket プラグイン（TASK-4.1 / #22）。
//!
//! 拡張点対応: UpgradeHandler（try_handle_upgrade）
//! （機械可読宣言の規約・許可語彙は `docs/design/dependency-graph-contract.md` 3 節、
//! TASK-13.2 / #50）
//!
//! # 背景・REQ-4 との対応
//!
//! コアの `UpgradeHandler` 拡張点（`crates/core/src/extension.rs`）は
//! 「委譲判定のみ」を担い、実際のアップグレード処理（RFC 6455 ハンドシェイク
//! 検証・101 応答送出・フレーミング）はプラグイン側に閉じる契約
//! （`docs/spec/04-requirements.md` REQ-4）。本クレートはその WebSocket 実装
//! である。
//!
//! # コアへの配線について（循環依存の回避）
//!
//! 本クレート単体は `fandhe-backend-core` に依存しない。コアが本クレート
//! へ `optional = true` + `dep:` 構文の依存を張る（`websocket` feature 有効時
//! のみ）ため、逆方向の依存を張ると循環依存になる
//! （`docs/design/plugin-boundary.md` 6.1 節・`scripts/dep-direction-check.sh`
//! が機械的に検証する）。そのため [`UpgradeHandler`][core-upgrade-handler]
//! trait を実装するアダプタはコア側（`crates/core/src/server.rs`）に置かれ、
//! 本クレートは [`matches()`] / [`handle_upgrade`] という純関数 + [`WebSocketConfig`]
//! のみを公開する。コア側の配線は `crates/core/src/plugin.rs`（Upgrade 型
//! シーム `try_handle_upgrade`）が担う。
//!
//! [core-upgrade-handler]: https://github.com/Fandhe-AI/fandhe-backend/blob/main/crates/core/src/extension.rs
//!
//! # 処理フロー
//!
//! 1. コアが `UpgradeHandler::matches` 相当の判定として [`matches()`] を呼び、
//!    `GET` + 設定パス + `Upgrade: websocket` の粗い判定を行う
//! 2. マッチした接続はコア側で読み取りバッファ解放（Conditional Go 条件(1)）
//!    後、残余バイト列とともに [`handle_upgrade`] へ完全委譲される
//! 3. [`handle_upgrade`] は RFC 6455 4.2.1 の詳細検証（`handshake::validate`）
//!    を行い、成功時は 101 応答、失敗時は 400/426 応答を送出する
//! 4. RFC 6455 検証を通過した要求について、[`WebSocketConfig::with_handshake_check`]
//!    （イシュー #716）でアプリケーション定義の受理判定フックが登録済みなら
//!    101 応答の送出前に一度だけ同期で評価する。フックは
//!    [`WsHandshakeContext`]（`{name}` パスパラメータ・リクエストヘッダ・
//!    接続元アドレスを参照可能）を受け取り、拒否する場合は指定した
//!    レスポンスを（`handshake::normalize_rejection` によるフェイルクローズ
//!    な正規化を経て）送出して upgrade しない（`conn_id`・`WsSender`・
//!    `on_open`/`on_close` はいずれも呼ばれない）。未登録時は既存の挙動
//!    （無条件で 101 応答）のまま変わらない（後方互換）
//! 5. 101 応答成功が確定した接続についてのみ、一意な接続識別子
//!    [`handler::WsConnId`]（イシュー #704）を発行し、送信ハンドル
//!    [`handler::WsSender`]（イシュー #670）を生成する。`config.pattern`
//!    （[`WebSocketConfig::with_path_pattern`]、イシュー #675）由来の
//!    パスパラメータとともに [`handler::WsOpenContext`] を構築して
//!    [`handler::WsMessageHandler::on_open`]（イシュー #671、既定 no-op。
//!    パラメータ経路はイシュー #676、`conn_id` はイシュー #704）を
//!    同期的に一度だけ呼び出す。パターン未登録（完全一致パス）の場合は
//!    パラメータなしのコンテキストになる。同じ `conn_id`・`WsSender`・
//!    パラメータを保持する [`handler::WsConnContext`]（イシュー #704）も
//!    同時に構築し、以降のセッション全体で使い回す。以降は
//!    `tokio-tungstenite` の `WebSocketStream` へフレーミング処理を委譲し、
//!    セッション終了まで面倒を見る（`session::run_session`）。Text/Binary
//!    メッセージは [`handler::WsMessageHandler::on_message_with_ctx`]
//!    （既定実装は既存の [`handler::WsMessageHandler::on_message`]（既定
//!    実装 [`handler::EchoHandler`]、Issue #179）へ委譲、イシュー #704）へ
//!    委譲され、返り値（[`handler::WsOutcome`]）に従って
//!    返信送出・セッション継続/終了を決める。`WebSocketConfig::idle_timeout`
//!    （既定 60 秒、fail-safe で有効）が設定されている場合、受信アイドルが
//!    続く接続は正常な Close ハンドシェイクで切断する（リソース枯渇 DoS
//!    対策、Issue #175。詳細は `session` モジュールの doc を参照）。
//!    セッション終了時には、終了経路を問わず
//!    [`handler::WsMessageHandler::on_close`]（イシュー #729）が
//!    [`handler::CloseReason`] 付きでちょうど 1 回呼ばれる（`on_open` が
//!    呼ばれた接続についてのみ。フェイルクローズの対称性は
//!    [`handler::WsMessageHandler::on_close`] の doc を参照）
//! 6. コア（`run_until`）から渡されるキャンセル `Future`（`handle_upgrade`
//!    第 5 引数、イシュー #492）が発火した場合も、アイドルタイムアウトと
//!    同型の正常な Close ハンドシェイク（close code 1001 Going Away）で
//!    切断する。ハンドシェイク開始前に既に発火済みなら 101 応答自体を
//!    送出せず即座に終了する。ハンドシェイク成立後は、受信待ちだけでなく
//!    ユーザーハンドラ実行中・返信/Close 送出中でも即座に打ち切って
//!    分岐する（イシュー #499、詳細は [`handle_upgrade`] の doc・`session`
//!    モジュールの doc を参照）
//!
//! # workspace 内での依存方向
//!
//! `docs/spec/04-requirements.md` REQ-1 / `docs/spec/05-tasks.md` TASK-11.1 の方針に従い、
//! workspace 全体の依存方向は次の一方向を維持する（依存方向: server → routes → http::*）。
//! 本クレートはプラグイン層（`fandhe-backend-plugin-*`）に位置し、コアの拡張点を実装する側であり、
//! コア（`fandhe-backend-core`）・`fandhe-backend-routes` からプラグインへの逆依存は発生しない
//! （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。本クレートの
//! workspace 内 path 依存は `fandhe-backend-http`（下位層の sans-IO パーサ）のみであり、
//! `fandhe-backend-core` には依存しない（上記「コアへの配線について」節を参照）。
//! 依存方向の機械検証は `scripts/dep-direction-check.sh` が担う。
//!
//! # pay-for-what-you-use
//!
//! 依存は `fandhe-backend-http`（`RequestHead` 参照のみ）・
//! `tokio`（`io-util`/`time`/`sync`）・`tokio-tungstenite`（`handshake`
//! feature のみ、TLS 系は無効）・`futures-util`（`WebSocketStream` の
//! Stream/Sink 駆動用）に限定する（詳細は `Cargo.toml` のコメントを参照）。
//! `sync` feature はイシュー #670 で追加した（`crate::handler::WsSender`
//! が `crate::session::run_session` へ合流するための bounded mpsc 用。
//! イシュー #710 で `WsSender::close`（サーバー起点の Close 指示）も同一の
//! チャネルへ内部表現 `OutboundItem` として流すようになったが、新規の
//! チャネル・依存は増えない。`tokio` の推移依存として新規クレートは
//! 増えない）。`websocket` feature
//! 無効時はコア（`fandhe-backend-core`）の依存グラフから本クレート自体が
//! 除外される（`cargo tree -p fandhe-backend-core` で確認可能）。イシュー
//! #671 で `WsMessageHandler::on_open` を追加し、[`handle_upgrade`] が
//! 101 応答送出成功後にチャネルを生成してハンドラへ渡すようになったが、
//! 新規クレート依存は増えない（既存の `tokio`/`sync` feature を使うのみ）。
//!
//! # キャンセル `Future` の受け渡し（イシュー #492）
//!
//! [`handle_upgrade`] はコアから世代キャンセルシグナル（最終 graceful
//! shutdown・rebind 世代 drain）を通知する `Future` を受け取る
//! （`docs/design/ws-cancellation-propagation.md` 3.2 節 (i)）。委譲境界を
//! `tokio::sync::watch::Receiver` ではなく `Future` として越える設計自体は
//! 変わらない。本クレートは `tokio` の `sync` feature を（イシュー #670
//! 以降は）本体依存として要求するが、これは [`crate::handler::WsSender`]
//! の bounded mpsc 用途であり、キャンセル `Future` の受け渡し方式とは
//! 無関係（統合テスト `tests/cancellation.rs` は引き続きキャンセル
//! トリガに `tokio::sync::oneshot` を使う）。
//!
//! # 接続元アドレスの受け渡し（イシュー #728）
//!
//! [`handle_upgrade_with_peer_addr`] は [`handle_upgrade`] に接続元の実
//! peer address（`Option<std::net::SocketAddr>`）を追加で渡せる版。コア
//! （`crates/core/src/plugin.rs` の `try_handle_upgrade`）は accept した
//! ソケットの実 peer address（`GateContext::peer_addr` と同じ由来、イシュー
//! #486）を本関数経由で渡し、`handler::WsOpenContext::peer_addr` から
//! `on_open` が観測できるようにする。既存 [`handle_upgrade`]（5 引数）は
//! `peer_addr: None` で本関数へ委譲する薄いラッパーとして残り、公開
//! シグネチャは無変更（非破壊追加）。
//!
//! # ハンドシェイクの受理判定フック（イシュー #716）
//!
//! [`WebSocketConfig::with_handshake_check`] で登録する
//! [`WsHandshakeCheck`] は、RFC 6455 検証を通過した upgrade 要求について
//! アプリケーション定義の認可判定（存在しない `{id}` への接続を 404 で
//! 拒否する、`Host`/`Origin` を検査して DNS rebinding を防ぐ、等）を行う
//! ための拡張点。コアの `RequestGate` 拡張点はパスパラメータを持たない
//! ため、それを代替する `plugin-websocket` 内蔵の拒否経路として設計した
//! （コア拡張点は増やさない、`docs/design/ws-connection-context-and-close.md`
//! 16 節参照）。フックは [`WsHandshakeContext`] 経由で `{name}` パス
//! パラメータ（`config.pattern` 由来、イシュー #675/#676 と同一の非デコード
//! 契約）・`RequestHead`（ヘッダ検査用）・接続元アドレス（イシュー #728 と
//! 同一由来）を参照でき、拒否する場合は返す [`fandhe_backend_http::response::Response`]
//! を送出して upgrade を行わない。未登録時（既定）は挙動が変わらない
//! （後方互換追加）。

mod config;
mod error;
pub mod handler;
mod handshake;
pub mod pattern;
mod session;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

pub use config::{
    MAX_OUTBOUND_CAPACITY, OutboundCapacityError, PingIntervalError, WebSocketConfig,
};
pub use error::WsError;
pub use handshake::{WsHandshakeCheck, WsHandshakeContext};

use fandhe_backend_http::request::RequestHead;

/// リクエストが `config` の指すアップグレード対象に該当するかを判定する。
///
/// コア側 `UpgradeHandler` アダプタから呼ばれる（`handshake` モジュール内の
/// 判定処理への薄いラッパー）。
#[must_use]
pub fn matches(head: &RequestHead, config: &WebSocketConfig) -> bool {
    handshake::matches(head, config)
}

/// アップグレード確定後の接続を引き受け、ハンドシェイク検証・101/400/426
/// 応答送出・フレーミング委譲・セッション終了までを行う。
///
/// `leftover` は 101 応答送出前にクライアントから先行到着していた可能性の
/// ある残余バイト列（コア側 `RecvBuffer::unread` 由来。パイプライン済み
/// フレームを取りこぼさないため `WebSocketStream::from_partially_read` へ
/// そのまま引き渡す）。
///
/// 戻り値 `Ok(())` は接続が正常に終了した（Close フレーム受信・EOF・
/// キャンセル発火に伴う正常な Close ハンドシェイク完了等）ことを意味する。
/// [`WebSocketConfig::with_handshake_check`]（イシュー #716）で登録した
/// 受理判定フックが接続を拒否した場合（拒否応答は送出済み・upgrade は
/// 行わなかった）も `Ok(())` に含まれる（`WsError` に専用の variant は
/// 追加しない。`#[non_exhaustive]` でない `WsError` に variant を追加すると
/// 網羅的な `match` をしている利用者にとって破壊的変更になるため、
/// `docs/design/ws-connection-context-and-close.md` 16 節の判断を参照）。
/// `Err` はハンドシェイク検証違反（400/426 応答は送出済み）またはフレーミング
/// 処理中の I/O・プロトコルエラーを意味する。呼び出し元（`crates/core`）は
/// このエラーを panic に変換せず、接続クローズとして扱う契約とする
/// （コア境界を越えて panic させない、`.claude/rules/coding-rust.md`）。
/// 受信メッセージ/フレームが `WebSocketConfig::max_message_size`/
/// `max_frame_size` を超えた場合、RFC 6455 7.4.1 節の close code 1009
/// （Message Too Big）を送出したうえで
/// `Err(WsError::Protocol(tungstenite::Error::Capacity(_)))` を返す
/// （イシュー #719。以前は Close を送らずに接続を drop していた）。
///
/// `cancel` はコアの世代キャンセルシグナル（最終 graceful shutdown・rebind
/// 世代 drain、イシュー #490〜#492）が発火したときに解決する `Future`。
/// キャンセル不要な呼び出し元（テスト等）は `std::future::pending::<()>()`
/// を渡せる。**BREAKING CHANGE**（イシュー #492。0.1.0 系からの移行は
/// `CHANGELOG.md` を参照）:
/// - ハンドシェイク開始前に既に発火済みなら 101 応答を送出せず即座に
///   `Ok(())` で終了する（クライアントへ Switching Protocols を見せない）
/// - ハンドシェイク応答（101/400/426）の書き込み中に発火した場合も打ち切る
///   （停滞した slow client でも有界時間で解放するため）
/// - セッション確立後に発火した場合は、`config.idle_timeout` 発火時と同型の
///   正常な Close ハンドシェイク（close code 1001 Going Away・固定 reason）
///   を試み、`WebSocketConfig::close_grace`（既定 10 秒、イシュー #500 で
///   設定可能化）を上限に打ち切る。受信待ちだけでなく、ユーザーハンドラ
///   実行中・返信/Close 送出中でも即座に打ち切って分岐する（イシュー
///   #499。ハンドラ `Future` の中断安全性契約は
///   [`handler::WsMessageHandler::on_message`] の doc を参照。詳細は
///   `session` モジュールの doc を参照）
///
/// 101 応答送出が成功した（＝セッションが確立した）接続についてのみ
/// [`handler::WsMessageHandler::on_open`] を一度呼ぶ（イシュー #671）。
/// ハンドシェイク検証失敗（400/426 応答）・101 応答送出前に `cancel` が
/// 発火していた場合・受理判定フック（イシュー #716）による拒否のいずれも
/// 呼ばれない（フェイルクローズ: 確立していないセッションへ
/// [`handler::WsSender`] を渡さない）。
///
/// `on_open` が呼ばれた接続については、終了経路を問わず
/// [`handler::WsMessageHandler::on_close`]（イシュー #729）が
/// [`handler::CloseReason`] 付きでちょうど 1 回呼ばれる（`session::
/// run_session` が担う。`on_open` と対称に、上記のハンドシェイク失敗・
/// 101 送出前キャンセルの場合は `on_close` も呼ばれない。これらの場合、
/// 失敗の詳細は本関数の戻り値（`Err`）からのみ観測できる）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade, matches};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let buf = b"GET /ws HTTP/1.1\r\n\
///     Upgrade: websocket\r\n\
///     Connection: Upgrade\r\n\
///     Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
///     Sec-WebSocket-Version: 13\r\n\
///     \r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// let config = WebSocketConfig::default();
/// assert!(matches(&head, &config));
///
/// // 実ソケットの代わりに duplex を使い、クライアント側が Close を即送信する
/// // ことでハンドシェイク成立後にセッションが正常終了する経路を確認する。
/// let (server_side, mut client_side) = tokio::io::duplex(4096);
/// use tokio::io::AsyncWriteExt;
/// tokio::spawn(async move {
///     // Close フレーム（マスク付き、payload なし）を送ってセッションを閉じる。
///     client_side.write_all(&[0x88, 0x80, 0, 0, 0, 0]).await.unwrap();
/// });
///
/// // キャンセル不要な呼び出しは `std::future::pending` を渡す。
/// let result = handle_upgrade(
///     server_side,
///     &head,
///     Vec::new(),
///     &config,
///     std::future::pending::<()>(),
/// )
/// .await;
/// assert!(result.is_ok());
/// # }
/// ```
pub async fn handle_upgrade<S, C>(
    stream: S,
    head: &RequestHead,
    leftover: Vec<u8>,
    config: &WebSocketConfig,
    cancel: C,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    // 接続元アドレスを渡さない後方互換の薄い委譲（イシュー #728）。
    // `tokio::io::duplex` 等の非ソケット呼び出し元は本関数を使い続けられる
    // （`WsOpenContext::peer_addr()` は常に `None` になる、下記関数 doc
    // 参照）。
    handle_upgrade_with_peer_addr(stream, head, leftover, config, cancel, None).await
}

/// [`handle_upgrade`] に接続元の実 peer address を追加で渡せる版
/// （イシュー #728）。
///
/// コア（`crates/core/src/plugin.rs` の `try_handle_upgrade`）は accept した
/// ソケットの実 peer address（`crates/core/src/server.rs` の
/// `handle_connection_with_permit` が保持する `Option<SocketAddr>`）を
/// `peer_addr` としてそのまま渡す。値は検証せずに運び、確立したセッションの
/// [`handler::WsOpenContext::peer_addr`] から `on_open` が観測できるように
/// する（`GateContext::peer_addr`（`crates/core/src/extension.rs`、
/// イシュー #486）と同型のフェイルクローズ契約: `tokio::io::duplex` 等の
/// 非ソケット経路、または呼び出し元が値を持たない場合は `None` を渡す）。
///
/// 引数の順序・意味は [`handle_upgrade`] の先頭 5 引数と同一で、6 番目に
/// `peer_addr` を追加しただけの非破壊追加（`handle_upgrade` の公開
/// シグネチャは無変更、`docs/design/ws-connection-context-and-close.md`
/// 15 節参照）。戻り値・エラー契約・キャンセル伝播（イシュー #492・#499）・
/// `on_open`/`on_close` の呼び出し契約は [`handle_upgrade`] と完全に同一。
///
/// # プロキシ配下の注意
///
/// リバースプロキシ・ロードバランサ配下では `peer_addr` はプロキシ自身の
/// アドレスになる（TLS 終端をプロキシに任せる v1 スコープ方針、
/// `docs/design/v1-scope-tls-multipart.md`）。クライアントの申告値
/// （`X-Forwarded-For` 等）とは異なり偽装できない値だが、プロキシ配下では
/// 直接クライアントの IP を表さない点に注意する。
///
/// # Examples
///
/// ```
/// use std::net::SocketAddr;
/// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade_with_peer_addr};
/// use fandhe_backend_plugin_websocket::handler::{
///     WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome,
/// };
/// use futures_util::future::BoxFuture;
/// use std::sync::{Arc, Mutex};
///
/// struct RecordPeerAddr(Arc<Mutex<Option<SocketAddr>>>);
///
/// impl WsMessageHandler for RecordPeerAddr {
///     fn name(&self) -> &'static str {
///         "record-peer-addr"
///     }
///
///     fn on_open(&self, ctx: WsOpenContext) {
///         *self.0.lock().unwrap() = ctx.peer_addr();
///     }
///
///     fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
///         Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let buf = b"GET /ws HTTP/1.1\r\n\
///     Upgrade: websocket\r\n\
///     Connection: Upgrade\r\n\
///     Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
///     Sec-WebSocket-Version: 13\r\n\
///     \r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// let observed = Arc::new(Mutex::new(None));
/// let config = WebSocketConfig::default().with_handler(RecordPeerAddr(observed.clone()));
/// let peer_addr: SocketAddr = "127.0.0.1:54321".parse().unwrap();
///
/// let (server_side, mut client_side) = tokio::io::duplex(4096);
/// use tokio::io::AsyncWriteExt;
/// tokio::spawn(async move {
///     client_side.write_all(&[0x88, 0x80, 0, 0, 0, 0]).await.unwrap();
/// });
///
/// let result = handle_upgrade_with_peer_addr(
///     server_side,
///     &head,
///     Vec::new(),
///     &config,
///     std::future::pending::<()>(),
///     Some(peer_addr),
/// )
/// .await;
/// assert!(result.is_ok());
/// assert_eq!(*observed.lock().unwrap(), Some(peer_addr));
/// # }
/// ```
pub async fn handle_upgrade_with_peer_addr<S, C>(
    mut stream: S,
    head: &RequestHead,
    leftover: Vec<u8>,
    config: &WebSocketConfig,
    cancel: C,
    peer_addr: Option<std::net::SocketAddr>,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    let mut cancel = std::pin::pin!(cancel);

    // ハンドシェイク開始前に一度だけ非ブロッキングでキャンセル済みかを
    // 確認する。`crates/core/src/plugin.rs` の中間実装（#491）が採用していた
    // 「キャンセルを最優先でポーリングする」順序を踏襲し、101 応答を送出した
    // 直後にハードクローズする TOCTOU を避ける（発火済みなら 101 を一切
    // 送出しない）。
    let already_cancelled =
        std::future::poll_fn(|cx| Poll::Ready(cancel.as_mut().poll(cx).is_ready())).await;
    if already_cancelled {
        return Ok(());
    }

    let validated = match handshake::validate(head) {
        Ok(validated) => validated,
        Err(WsError::UnsupportedVersion) => {
            write_racing_cancel(&mut stream, cancel.as_mut(), &handshake::serialize_426()).await?;
            return Err(WsError::UnsupportedVersion);
        }
        Err(err @ WsError::InvalidHandshake(_)) => {
            write_racing_cancel(&mut stream, cancel.as_mut(), &handshake::serialize_400()).await?;
            return Err(err);
        }
        Err(err) => return Err(err),
    };

    // イシュー #716: `config.pattern` 由来のパスパラメータを 101 応答の前に
    // 一度だけ計算する（`WsHandshakeContext` へ借用のまま渡すため。
    // パターン未登録・不一致（後者は理論上到達しないはずの防御的
    // フォールバック）ではいずれも空 `PathParams` になる）。101 応答送出後の
    // `WsOpenContext` 用コピーは、下記でこの値を再利用する（二重計算しない）。
    let matched_params = handshake::match_config_path(head, config).unwrap_or_default();

    // イシュー #716: ハンドシェイクの受理判定フック。RFC 6455 検証
    // （上記 `handshake::validate`）を通過した要求のみを対象とし、101 応答の
    // 送出前に一度だけ同期で評価する。`RequestGate` はパスパラメータを
    // 持たないため、これを代替する `plugin-websocket` 内蔵の拒否経路として
    // 設計した（`docs/design/ws-connection-context-and-close.md` 16 節）。
    if let Some(check) = &config.handshake_check {
        let ctx = handshake::WsHandshakeContext::new(head, &matched_params, peer_addr);
        if let Err(rejection) = check.check(&ctx) {
            // 1xx/2xx はフェイルクローズに正規化する（upgrade が成功したと
            // クライアントに誤認させる応答・「拒否」の意味に反する応答を
            // 送出しない、`handshake::normalize_rejection` の doc 参照）。
            // `keep_alive: false` 固定で `Connection: close` を必ず付け、
            // 拒否後の同一接続にバイトが紛れ込む余地をなくす
            // （`.claude/rules/security.md` インジェクション対策）。
            let bytes = handshake::normalize_rejection(rejection).serialize(false);
            write_racing_cancel(&mut stream, cancel.as_mut(), &bytes).await?;
            // `conn_id` の発行・`WsSender` の生成・`on_open`/`on_close` の
            // 呼び出しは行わない（フェイルクローズ: 確立していないセッション
            // へ渡さない。ハンドシェイク検証失敗・101 送出前キャンセルと
            // 同じ対称性、`handle_upgrade` の doc を参照）。
            return Ok(());
        }
    }

    // 101 応答の書き込み自体も cancel と race させる。停滞した slow client
    // （書き込みバッファが埋まり `write_all` が進まない）に対しても有界時間
    // で解放できるようにするため（上記関数 doc「BREAKING CHANGE」節を参照）。
    let cancelled_before_101 = write_racing_cancel(
        &mut stream,
        cancel.as_mut(),
        &handshake::serialize_101(&validated.accept_key),
    )
    .await?;
    if cancelled_before_101 {
        return Ok(());
    }

    // イシュー #671: `WsSender` をハンドラへ渡す公開経路。101 応答送出が
    // 成功した（＝セッションが確立した）接続についてのみチャネルを作り
    // `on_open` を呼ぶ（ハンドシェイク失敗・101 送出前キャンセル・受理判定
    // フックによる拒否では呼ばれない。フェイルクローズ: 確立していない
    // セッションへ `WsSender` を渡さない）。
    //
    // イシュー #676: `config.pattern` 由来のパスパラメータを
    // `WsOpenContext` へ渡す。`matched_params`（`PathParams<'_>`）は
    // `head`（本関数のスタックフレーム内でのみ生存）への借用のため、
    // `on_open` へ渡す前に所有 `Vec<(String, String)>` へコピーする
    // （`handler::WsOpenContext` の doc 参照）。
    let params: Vec<(String, String)> = matched_params
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    // 送信キュー容量はイシュー #709 で利用者調整可能になった
    // （`WebSocketConfig::with_outbound_capacity`、既定は
    // `handler::DEFAULT_OUTBOUND_CAPACITY`）。`session::run_handler_with_outbound_drain`
    // の排出回数上限（`config.outbound_capacity`）もこの値と一致させる契約
    // （`crates/plugin-websocket/src/session.rs` の該当 doc を参照）。
    let (sender, outbound_rx) = handler::channel(config.outbound_capacity);

    // イシュー #704（親 #702）: この接続専用の一意 ID を 1 回だけ発行する。
    // ハンドシェイク失敗・101 送出前キャンセルではこの行に到達しないため
    // `conn_id` は発行されない（`on_open`/`on_close` のフェイルクローズ対称性、
    // `docs/design/ws-connection-context-and-close.md` 4 節を参照）。
    let conn_id = handler::WsConnId::next();
    // イシュー #717: 接続元アドレス・主要リクエストヘッダ（Host/Origin/
    // User-Agent）・query を許可リスト方式で 1 回だけ抽出し、`WsOpenContext`
    // と `WsConnContext` の双方へ同一の `Arc` として共有する（2 回コピー
    // しない契約、`handler::ConnRequestInfo` の型 doc 参照）。この行に
    // 到達するのは 101 応答送出が成功した接続のみ（ハンドシェイク失敗・
    // 101 送出前キャンセルでは構築されないフェイルクローズ契約は `on_open`
    // と対称）。
    let info = Arc::new(handler::ConnRequestInfo::from_head(head, peer_addr));
    // `on_message_with_ctx` へ渡す `WsConnContext` は `on_open` の呼び出し前に
    // 構築する（`WsSender` のクローンを保持するため、セッション終了まで
    // outbound チャネルの送信側が閉じなくなる副作用がある。設計 5 節を参照）。
    let conn_ctx =
        handler::WsConnContext::new(conn_id, sender.clone(), params.clone(), info.clone());
    config
        .handler
        .on_open(handler::WsOpenContext::new(conn_id, sender, params, info));
    session::run_session(
        stream,
        leftover,
        config,
        cancel,
        Some(outbound_rx),
        &conn_ctx,
    )
    .await
}

/// `bytes` を `stream` へ書き込みつつ `cancel` と race する。キャンセルが
/// 先に発火した場合は書き込みを打ち切って `Ok(true)` を返し（呼び出し元は
/// 応答が完了しなかったものとして扱う）、書き込みが完了した場合は
/// `Ok(false)` を返す。書き込み自体の I/O エラーは [`WsError`] へ変換して
/// 伝播する（[`handle_upgrade`] のハンドシェイク応答書き込み 3 箇所
/// （101/400/426）で共有する）。
async fn write_racing_cancel<S, C>(
    stream: &mut S,
    cancel: Pin<&mut C>,
    bytes: &[u8],
) -> Result<bool, WsError>
where
    S: AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    match race_cancel(cancel, stream.write_all(bytes)).await {
        None => Ok(true),
        Some(Ok(())) => Ok(false),
        Some(Err(err)) => Err(err.into()),
    }
}

/// `cancel` と `fut` を race させる。`cancel` を最優先でポーリングし
/// （TOCTOU 回避、上記 doc を参照）、先に発火すれば `None` を返す。`fut` が
/// 先に完了すれば `Some(fut の出力)` を返す。`session` モジュールの受信
/// ループ・アイドルタイムアウト経路と本モジュールのハンドシェイク応答
/// 書き込みで共有する手動 race ヘルパー（`std::future::poll_fn` +
/// `std::pin::pin!` のみで構成、追加依存なし。`crates/core/src/plugin.rs`
/// の中間実装が使っていたパターンと同型）。
pub(crate) async fn race_cancel<C, F>(mut cancel: Pin<&mut C>, fut: F) -> Option<F::Output>
where
    C: Future<Output = ()>,
    F: Future,
{
    let mut fut = std::pin::pin!(fut);
    std::future::poll_fn(|cx| {
        if cancel.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        if let Poll::Ready(output) = fut.as_mut().poll(cx) {
            return Poll::Ready(Some(output));
        }
        Poll::Pending
    })
    .await
}
