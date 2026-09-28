//! ハンドシェイク成立後のフレーミング処理（tokio-tungstenite への委譲）。
//!
//! `crate::handle_upgrade` から呼ばれる。101 応答送出直後の生ストリームを
//! `WebSocketStream::from_partially_read` へ渡し、以降の RFC 6455 フレーミング
//! （マスク処理・Ping/Pong 自動応答・Close ハンドシェイク）は tokio-tungstenite
//! に委ねる。Text/Binary メッセージは [`crate::handler::WsMessageHandler`]
//! （`config.handler`、既定 [`crate::handler::EchoHandler`]）へ委譲し、
//! 返り値（[`crate::handler::WsOutcome`]）に従って返信送出・セッション
//! 継続/終了を決める（Issue #179、親 #91。TASK-4.1 時点の「ユーザー定義
//! メッセージハンドラ API は導入しない」制約はここで解消された）。
//!
//! `config.idle_timeout` が有効な場合、フレーム受信を都度
//! `tokio::time::timeout` で監視し、アイドル（無通信）が続く接続を正常な
//! Close ハンドシェイクで切断する（リソース枯渇 DoS 対策、Issue #175。
//! 詳細は [`run_session`] の doc を参照）。
//!
//! Text/Binary メッセージは [`crate::handler::WsMessageHandler::
//! on_message_with_ctx`]（イシュー #704、既定実装は既存の `on_message`
//! へ委譲）へ渡す。`run_session` の呼び出し元（`crate::handle_upgrade`）が
//! 接続確立時に 1 回だけ構築する [`crate::handler::WsConnContext`] を
//! セッション全体で使い回す（`conn_ctx` 引数）。
//!
//! `crate::handle_upgrade` から渡されるキャンセル `Future`（コアの世代
//! キャンセルシグナル、イシュー #492）は受信待ちだけでなく、ユーザー
//! ハンドラ実行中（`WsMessageHandler::on_message` の `await`）・
//! `WsOutcome::Reply` / `WsOutcome::Close` の送出中でも最優先ポーリングし、
//! 発火時は当該処理中の `Future` を即座に drop したうえでアイドル
//! タイムアウトと同型の正常な Close ハンドシェイク（close code 1001
//! Going Away）へ分岐する（イシュー #499。[`handle_cancellation`] の
//! doc・`docs/design/ws-cancellation-propagation.md` 10 節を参照）。
//!
//! [`run_session`] は終了時、[`crate::handler::WsMessageHandler::on_close`]
//! （イシュー #729）を [`crate::handler::CloseReason`] 付きでちょうど 1 回
//! 呼ぶ（[`run_session`] の doc を参照。呼び出し箇所が本モジュール内 1 箇所
//! のみのため個々の脱出点に呼び出しを散らさずに済む）。
//!
//! [`run_session`] は [`crate::handler::WsSender`]（イシュー #670、親
//! #669）が bounded mpsc 経由で送るサーバー起点メッセージも受信ループへ
//! 合流させる。受信ループは cancel（最優先）→ (クライアント受信 or
//! アイドル期限) と outbound（サーバー起点 push）を 1 イベントずつ選ぶ。
//! 両方が同時に Ready な場合にどちらを優先するかはループ反復ごとに
//! 交互（alternating）に入れ替える（[`race2_alternating`]、イシュー
//! #684 レビュー指摘対応）。固定でクライアント受信を優先する `race2` を
//! 使うと、クライアントが連続送信を続ける限り `ws.next()` が常に
//! ポーリング時点で Ready となり得るケースで `rx.recv()`（outbound）が
//! 恒久的に飢餓状態になり、サーバー起点 push が無期限に滞留しうるため
//! （スケジューリング上両方が Ready であることが構造的に起こりうる以上、
//! 固定優先度は公平性を保証しない）。交互化により、連続受信が続いても
//! 最悪 2 反復に 1 回は outbound 側が優先ポーリングされ、送信済み
//! push メッセージが有界回数内に処理される。outbound メッセージは既存の
//! `ws.send()`（[`apply_outcome`] の `WsOutcome::Reply` 送出と同一の
//! `&mut WebSocketStream`）へ直列に送出する（フレームが混ざらないことを
//! 構造的に保証する。単一タスクが `ws` を排他的に所有するため）。
//! `config.idle_timeout` は**クライアントから実際にフレームを受信した
//! 場合にのみ**更新し、outbound 送出はタイマーをリセットしない（無通信の
//! デッドクライアントへ定期 push し続けるとアイドルタイムアウトが永久に
//! 発火しなくなる退行を避けるため。Issue #175 が導入した DoS 対策を
//! 後退させない）。更新タイミングはフレーム受信直後ではなく、ハンドラ
//! 実行（`on_message`）・返信送出（`apply_outcome`）まで完了し次の
//! 受信待ちに入る直前とする（受信直後に更新すると、ハンドラ処理・返信
//! 送出に `idle_timeout` 相当の時間を要した場合にその処理時間がアイドル
//! 待機時間へ算入され、処理完了直後の次の受信待ちで即座に期限切れと
//! なりうるため。レビュー指摘対応、既存の「各 `ws.next()` の待機を
//! 開始する直前に毎回タイムアウトを設定する」契約を回復する）。イシュー
//! #671 で `handle_upgrade` が `WsMessageHandler::on_open` 経由で
//! ハンドラへ `WsSender` を渡す公開経路を追加し、101 応答送出成功後は
//! 常に `Some(rx)` を渡すようになった（本モジュールの合流ロジック自体は
//! イシュー #684 の交互化以外は無変更）。
//!
//! # ハンドラ実行中の送信キュー消化（イシュー #706）
//!
//! 上記の合流は「クライアント受信待ち」の間のみ outbound を消化する。
//! Text/Binary メッセージ受信後に [`WsMessageHandler::on_message_with_ctx`]
//! を単独 `await` すると、ハンドラ本体（または `on_message_with_ctx` が
//! `.await` するタスク）がハンドラ実行中に `WsSender::send` を outbound
//! チャネル容量（設定された容量、`WebSocketConfig::outbound_capacity`。
//! 既定 [`crate::handler::DEFAULT_OUTBOUND_CAPACITY`] = 8、イシュー #709）を
//! 超える回数呼んだ場合、受信側（本モジュール）がその間キューを一切消費
//! しないため `send` が永久にブロックしデッドロックする（CDP の
//! `Page.navigate` のように「応答の前に複数イベントを送る」ハンドラで
//! 現実に起こる）。[`run_handler_with_outbound_drain`] がこれを解消する
//! 内側ループとして働く（手順・送出順序の保証と不定契約は同関数の doc を
//! 参照。本節で重複記述しない）。
//!
//! # ハンドラ Future の中断安全性契約（イシュー #499）
//!
//! `on_message` が返す `Future` は shutdown・rebind 世代 drain の発火時に
//! 任意の `await` 点で drop されうる（Rust async の標準的なキャンセル
//! 意味論、`tokio::select!` / `tokio::time::timeout` と同型）。ハンドラ
//! 実装は中断されても不変条件を壊さない（drop-safe な）ことを要求され、
//! 完了保証が必要な処理（外部への書き込み確定等）は `tokio::spawn` で
//! セッションから切り離して実行する（詳細は [`crate::handler`] モジュール
//! の doc を参照）。`WsOutcome::Reply` の送出打ち切りについても、
//! `WebSocketStream` がフレーミングバッファの書き込み位置をストリーム
//! 本体側で保持するため、打ち切り後に送出する Close フレームが未送出
//! バイトの続きとして破損した状態で流出することはない
//! （ワイヤ安全性、`ws-cancellation-propagation.md` 10 節）。
//!
//! # サーバー起点 Ping keepalive（`WebSocketConfig::with_ping_interval`）
//!
//! 受信は本モジュール内の 1 か所（ループの先頭）で逐次処理するため、ハンドラ
//! 実行中・送出ブロック中に届いたフレームの到着時刻は観測できない。このため
//! Ping/Pong の死活監視は「読んだ時点」と「受信待ちで何も読めない状態」だけで
//! 判定する 2 つの独立した仕組みに分ける。
//!
//! 1. **Ping 送出**（ループ先頭でのみ判定）: 前回送出から `interval` 経過し、
//!    かつ未応答の Ping がなければ [`Message::Ping`] を送る。ハンドラ実行中は
//!    送らない（次の反復まで遅れる）。
//! 2. **Pong 期限**: Ping 送出時刻 + `pong_timeout`。判定は受信待ちの race の
//!    中だけで行い（[`TimerKind::PongDeadline`]）、ws の読み取りを優先する
//!    ため、バッファ済みの Pong があれば先に読んで期限を解除する（Pong を
//!    読んだ時点で期限解除、payload の照合はしない）。期限を過ぎても読める
//!    フレームが残っている間は判定に至らず、受信待ちで読めるフレームが
//!    なくなった時点で切断が確定する（Pong を返さない対向も、フレームが
//!    途切れず届いている間は切断しない）。
//! 3. **送出詰まり**: keepalive 有効時、[`send_bounded`] を経由する 1 回の
//!    送出（Ping・Reply・outbound push・`drain_before_reply`/
//!    `drain_to_close` 中の送出）がその送出の**開始時刻**から
//!    `pong_timeout` を超えてブロックしたら [`CloseReason::PongTimeout`] で
//!    終了する（`send_bounded` の `stall_timeout` 引数）。書き込み途中で
//!    終わるため Close ハンドシェイクは送らずに `ws` を drop する
//!    （送出失敗・`close_grace` 超過と同じ「無理に送らない」流儀）。
//!    終了経路の排出（[`flush_outbound`]、`WsOutcome::Close` 確定後の
//!    終端排出）は対象外（すでに `close_deadline` で全体が有界であり、
//!    Close 経路であるため）。
//!
//! 「期限内 Pong の取りこぼしなし」「期限後 Pong は許可しない」「メモリが
//! 有界」を同時には満たせない（逐次受信のため）。厳密な到着時刻判定には
//! 受信の並行化が必要で別課題とする。

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::frame::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::Utf8Bytes;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig as TungsteniteConfig};
use tokio_tungstenite::tungstenite::{Bytes, Message};

use futures_util::{SinkExt, StreamExt};

use crate::config::{PingKeepalive, WebSocketConfig};
use crate::error::WsError;
use crate::handler::{
    CloseReason, FailureKind, OutboundItem, WsConnContext, WsHandlerError, WsMessage, WsOutcome,
    WsSender,
};
use crate::race_cancel;

/// 101 応答送出済みのストリームを受け取り、WebSocket セッション終了まで
/// 処理する（既存の公開シグネチャを保つ薄いラッパー、イシュー #726）。
///
/// 本体は [`run_session_inner`] に移した。本関数はその戻り値
/// （`(CloseReason, Result<(), WsError>)`）を分解し、[`crate::handler::
/// WsMessageHandler::on_close`] を `CloseReason` 付きでちょうど 1 回呼んだ
/// あと、従来どおり `Result<(), WsError>` のみを返す（呼び出し元
/// `crate::handle_upgrade` および既存の `#[cfg(test)]` テストは無変更で
/// 動作する。イシュー #729）。
///
/// 呼び出し箇所が本関数内の 1 箇所だけであり、`run_session_inner` も
/// ちょうど 1 つの `(CloseReason, _)` を返す構造上、`on_close` は個々の
/// `return`/`break` に散らさずともここで自動的にちょうど 1 回呼ばれる
/// （`docs/design/ws-connection-context-and-close.md` 4 節の不変条件・
/// 9 節を参照）。呼び出し時点では `outbound` の受信側は
/// `run_session_inner` へ move 済みで既に drop されているため、
/// `WsMessageHandler::on_close` の doc が述べる「`ctx.sender().send(..)`
/// は常に失敗する」契約はこの drop に由来する。
pub(crate) async fn run_session<S, C>(
    stream: S,
    leftover: Vec<u8>,
    config: &WebSocketConfig,
    cancel: Pin<&mut C>,
    outbound: Option<mpsc::Receiver<OutboundItem>>,
    conn_ctx: &WsConnContext,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    let (reason, result) =
        run_session_inner(stream, leftover, config, cancel, outbound, conn_ctx).await;
    config.handler.on_close(conn_ctx, reason);
    result
}

/// [`run_session`] の本体（イシュー #726）。セッション終了まで処理し、
/// 終了理由（[`CloseReason`]）と従来の `Result<(), WsError>` を両方返す。
///
/// `leftover` は 101 応答送出前にクライアントから先行到着していた可能性の
/// ある残余バイト列（コア側 `RecvBuffer::unread` 由来）。
/// `WebSocketStream::from_partially_read` へそのまま渡すことで、先行フレーム
/// を取りこぼさない。
///
/// Text/Binary メッセージは [`crate::handler::WsMessageHandler::on_message`]
/// （`config.handler`）へ変換して委譲する。ハンドラは受信メッセージごとに
/// 直列 `await` される（順序保証・自然なバックプレッシャのため。並行処理
/// したいユーザーはハンドラ内で自前に `tokio::spawn` する）。
/// [`WsOutcome::Reply`] は到着順に `ws.send()` で送出してセッションを継続し、
/// [`WsOutcome::Close`] はサーバ起点の Close ハンドシェイクを開始する。
/// ハンドラが `Err` を返した場合は [`WsError::Handler`] へ変換してループを
/// 終える（コア境界を越えて panic させない契約は維持、
/// `.claude/rules/coding-rust.md`）。
///
/// Ping には tokio-tungstenite が自動で Pong を返す（tungstenite の既定
/// 動作）。Close フレーム受信、または I/O エラー・プロトコルエラーで
/// ループを終える。エラーは呼び出し元（`crate::handle_upgrade`）へ伝播する。
///
/// `config.idle_timeout` が `Some(d)` の場合、各受信待ちを `d` で
/// `tokio::time::timeout` する。フレーム（Ping/Pong を含む全種別）を 1 つ
/// 受信するたびにタイマーは実質リセットされる。`d` 以内に何も届かなければ
/// アイドルと判定し、サーバ側から Close フレーム（1000 Normal Closure）を
/// 送出したうえで、`config.close_grace`（既定 10 秒）を上限にクライアントの
/// Close 応答（または EOF）をドレインしてから `(CloseReason::IdleTimeout,
/// Ok(()))` で終了する（ポリシー駆動の正常終了。プロトコル違反ではないため
/// `WsError` の新規 variant は追加しない）。
/// `idle_timeout` が `None`（`without_idle_timeout` による明示的無効化）の
/// 場合は従来どおり無期限に受信を待つ。
///
/// # サイズ上限とハンドラ呼び出し順序（DoS 対策の維持、Issue #179 セキュリティ考慮）
///
/// `max_message_size` / `max_frame_size` は tungstenite 側で強制されるため、
/// 上限超過メッセージはハンドラへ届く前にプロトコルエラーとして拒否される
/// （`ws.next()` が `Err` を返す。`CloseReason::MessageTooLarge` へ分類、
/// [`SessionFailure::recv`] 参照）。ハンドラ呼び出し前のサイズ検証という
/// 既存の安全性方針を後退させない。
///
/// `cancel` は `crate::handle_upgrade` が pin 済みで渡すキャンセル `Future`
/// （イシュー #492）。各受信待ちに加え、ユーザーハンドラ実行中・
/// [`apply_outcome`] による返信/Close 送出中でも最優先ポーリングし、発火時は
/// 実行中の `Future` を drop したうえで [`handle_cancellation`] へ分岐する
/// （優先順位はアイドルタイムアウトより高い。TOCTOU 回避の詳細は
/// `crate::race_cancel` の doc を参照。イシュー #499 で受信待ち以外の区間へ
/// 適用範囲を拡大した）。
///
/// `outbound` は [`crate::handler::WsSender`]（イシュー #670）が送る
/// サーバー起点メッセージの受信側。`Some` の場合、クライアント受信待ちと
/// 合流させて 1 イベントずつ処理する（モジュール doc を参照）。両者が
/// 同時に Ready な場合の優先順はループ反復ごとに交互化し、連続受信
/// クライアントによる outbound 飢餓を防ぐ（[`race2_alternating`]、
/// イシュー #684）。全 `WsSender` クローンが drop されチャネルが閉じた
/// 場合はそのイベント源を無効化するのみでセッション自体は継続する
/// （`None` にはしない設計だと毎回 `recv()` を呼び続けビジーループ化
/// しうるため、内部で `outbound = None` 相当に切り替えて以後は選択
/// しないようにする。イシュー #704 で `conn_ctx`（[`WsConnContext`]）
/// 自身が `WsSender` のクローンをセッション終了まで保持するようになり、
/// この分岐はセッション実行中は到達不能になった。将来の保持方式変更に
/// 備えた防御的コードとして維持する）。
/// cancel 発火時・アイドルタイムアウト発火時は、[`handle_cancellation`] /
/// [`close_normally`] を呼ぶ**前**に `outbound` を drop し、満杯
/// チャネルでブロック中の [`crate::handler::WsSender::send`] 呼び出しを
/// `close_grace` の満了を待たず即座に解放する。
///
/// Text/Binary メッセージ受信後の [`WsMessageHandler::on_message_with_ctx`]
/// 実行中も [`run_handler_with_outbound_drain`] 経由で outbound を消化し
/// 続ける（イシュー #706、モジュール doc の「ハンドラ実行中の送信キュー
/// 消化」節を参照。デッドロック解消のための追加区間で、モジュール doc
/// 上部が既に述べる受信待ち中の合流とは独立した内側ループ）。
///
/// # 終了理由の割り当て（イシュー #726、設計 4 節の脱出点対応表）
///
/// - クライアントの Close フレーム受信 → [`CloseReason::ClientClose`]
/// - 受信 EOF（`ws.next()` が `None`）、または Close ハンドシェイクなしの
///   TCP 切断（`ws.next()` が `Err(Protocol(ResetWithoutClosingHandshake))`
///   を返す、tokio-tungstenite 0.30 での主経路。[`SessionFailure::recv`]
///   が両方を [`CloseReason::Eof`] へ分類する） → [`CloseReason::Eof`]
///   （前者は `Result` 側が `Ok(())`、後者は `Err(WsError::Protocol(_))`）
/// - `config.idle_timeout` 発火 → [`CloseReason::IdleTimeout`]
/// - コアの世代キャンセル発火 → [`CloseReason::Cancelled`]
/// - ハンドラが `WsOutcome::Close` → [`CloseReason::HandlerClose`]
/// - 受信サイズ上限超過 → [`CloseReason::MessageTooLarge`]
/// - その他の受信/送信/ハンドラ失敗 → [`CloseReason::Failed`]（種別は
///   [`SessionFailure::recv`]/[`SessionFailure::send`]/[`SessionFailure::handler`]
///   が決定する）
/// - [`crate::handler::WsSender::close`] によるサーバー起点の Close 指示
///   → [`CloseReason::SenderClose`]（イシュー #710。受信待ち中・ハンドラ
///   実行中のいずれから検出されても同一の `CloseReason` になる）
async fn run_session_inner<S, C>(
    stream: S,
    leftover: Vec<u8>,
    config: &WebSocketConfig,
    mut cancel: Pin<&mut C>,
    outbound: Option<mpsc::Receiver<OutboundItem>>,
    conn_ctx: &WsConnContext,
) -> (CloseReason, Result<(), WsError>)
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    // 受信側をガードに持たせ、本関数を抜けるすべての経路（早期 return・future
    // の drop を含む）で「送信キューの封鎖 → 受信側の drop」の順を保証する
    // （PR #736 レビュー指摘対応。[`OutboundGuard`] の doc を参照）。途中で受信側を
    // 手放す箇所は [`OutboundGuard::release`] を使う。
    let mut outbound = OutboundGuard::new(outbound, conn_ctx.sender(), config.close_grace);

    let ws_config = TungsteniteConfig::default()
        .max_message_size(Some(config.max_message_size))
        .max_frame_size(Some(config.max_frame_size));

    let mut ws: WebSocketStream<S> =
        WebSocketStream::from_partially_read(stream, leftover, Role::Server, Some(ws_config)).await;

    // アイドル期限は「クライアントから実際にフレームを受信したとき」にのみ
    // 更新する（モジュール doc を参照。outbound push ではリセットしない）。
    let mut idle_deadline: Option<Instant> = config.idle_timeout.map(|d| Instant::now() + d);

    // サーバー起点 Ping keepalive の状態（`WebSocketConfig::with_ping_interval`、
    // モジュール doc「サーバー起点 Ping keepalive」節を参照）。`None`（既定）は
    // 無効で、以降のこの変数に関わる分岐はすべて素通りする（既存挙動を変えない）。
    let mut keepalive: Option<Keepalive> = config.ping.map(Keepalive::new);

    // keepalive 有効時、送出 1 回あたりの詰まり検知に使う期限の長さ
    // （`pong_timeout`、モジュール doc「サーバー起点 Ping keepalive」節の
    // 「送出詰まり」）。[`send_bounded`] の呼び出しごとに素通しするだけの値で、
    // ループ中は変化しない。
    let stall_timeout: Option<Duration> = config.ping.map(|p| p.pong_timeout);

    // inbound（クライアント受信 + アイドル期限）と outbound（サーバー起点
    // push）が同時に Ready な場合にどちらを優先ポーリングするかを反復
    // ごとに交互化するフラグ（イシュー #684）。固定でクライアント受信を
    // 優先すると、連続送信クライアント下で outbound が恒久的に飢餓
    // しうるため（モジュール doc を参照）。
    let mut prefer_outbound = false;

    // 脱出点対応表（イシュー #726、上記 doc 参照）: ループは必ず
    // `CloseReason` を伴って抜ける（`break <reason>` または関数からの
    // `return (<reason>, <result>)`）。戻り値型がタプルになったことで、
    // 値なしの `break`・素の `?` はコンパイルエラーとなり、脱出点の
    // 網羅が型で保証される。
    let reason = loop {
        // Ping keepalive の送出判定（モジュール doc「サーバー起点 Ping
        // keepalive」節の「Ping 送出」）。受信待ちに
        // 入る**前**、反復の先頭でのみ判定する（ハンドラ実行中は送らない）。
        // 未応答の Ping（`pong_deadline.is_some()`）がある間は送らない。
        if let Some(ka) = keepalive.as_mut()
            && ka.pong_deadline.is_none()
            && Instant::now() >= ka.next_ping_at
        {
            match send_bounded(
                &mut ws,
                cancel.as_mut(),
                &mut outbound.close,
                Message::Ping(Bytes::new()),
                stall_timeout,
            )
            .await
            {
                SendOutcome::Sent => {
                    let now = Instant::now();
                    ka.pong_deadline = Some(now + ka.pong_timeout);
                    ka.next_ping_at = now + ka.interval;
                }
                SendOutcome::Cancelled => {
                    outbound.release();
                    return (
                        CloseReason::Cancelled,
                        handle_cancellation(ws, config.close_grace).await,
                    );
                }
                SendOutcome::CloseGraceExpired => {
                    outbound.release();
                    return close_grace_expired();
                }
                SendOutcome::Stalled => {
                    outbound.release();
                    return pong_timeout_expired();
                }
                SendOutcome::Failed(err) => return SessionFailure::send(err).into_parts(),
            }
        }

        // クライアント受信（+ アイドル期限 + keepalive の Pong 期限/次回 Ping
        // 予定時刻）を 1 つの Future にまとめる。新規 `ws.next()` /
        // `sleep_until()` を毎ループ作り直す既存パターン（drop による打ち切り
        // は `ws` 自体の状態に影響しない）を踏襲する。
        //
        // idle 期限と keepalive のタイマーの両方が有効な場合、早い方だけを
        // 使う（`ws.next()` が優先される点は変わらない。モジュール doc
        // 「サーバー起点 Ping keepalive」節の「Pong 期限」を参照）。同時刻なら
        // `Idle` を優先する（どちらでもポリシー上の差はない、決定的な順序の
        // ためだけの選択）。
        let idle_leg = idle_deadline.map(|d| (d, TimerKind::Idle));
        let keepalive_leg = keepalive.as_ref().map(Keepalive::timer_leg);
        let timer_leg = match (idle_leg, keepalive_leg) {
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        let inbound = async {
            match timer_leg {
                Some((deadline, kind)) => {
                    match race2(ws.next(), tokio::time::sleep_until(deadline)).await {
                        Either::Left(message) => InboundEvent::Message(message),
                        Either::Right(()) => InboundEvent::Timer(kind),
                    }
                }
                None => InboundEvent::Message(ws.next().await),
            }
        };

        let event = if let Some(rx) = outbound.rx.as_mut() {
            // 反復ごとに優先順を反転する（交互化、モジュール doc を参照）。
            // 両方 Ready でない通常時はこの反転自体が結果へ影響しない
            // （どちらが先にポーリングされても Pending の側は素通りする
            // だけのため）。両方 Ready な場合にのみ順序が意味を持つ。
            prefer_outbound = !prefer_outbound;
            match race_cancel(
                cancel.as_mut(),
                race2_alternating(prefer_outbound, inbound, rx.recv()),
            )
            .await
            {
                None => {
                    outbound.release();
                    return (
                        CloseReason::Cancelled,
                        handle_cancellation(ws, config.close_grace).await,
                    );
                }
                Some(Either::Left(inbound_event)) => inbound_event,
                Some(Either::Right(Some(msg))) => InboundEvent::Outbound(msg),
                Some(Either::Right(None)) => {
                    // 全 WsSender クローンが drop 済み。以後このイベント源を
                    // 選択しないよう無効化し、セッション自体は継続する
                    // （ビジーループ化を防ぐ）。
                    //
                    // イシュー #704: `conn_ctx`（`WsConnContext`）自身が
                    // `WsSender` のクローンを 1 個セッション終了まで保持する
                    // ようになったため、この分岐はセッション実行中は
                    // **到達不能**になった（全クローンが drop されるのは
                    // セッション終了後のみ）。将来 `WsConnContext` の保持
                    // 方式が変わった場合の安全網として、削除せず防御的
                    // コードのまま維持する（`docs/design/
                    // ws-connection-context-and-close.md` 5 節）。
                    outbound.release();
                    continue;
                }
            }
        } else {
            match race_cancel(cancel.as_mut(), inbound).await {
                None => {
                    return (
                        CloseReason::Cancelled,
                        handle_cancellation(ws, config.close_grace).await,
                    );
                }
                Some(inbound_event) => inbound_event,
            }
        };

        match event {
            InboundEvent::Timer(TimerKind::Idle) => {
                outbound.release();
                return (
                    CloseReason::IdleTimeout,
                    close_normally(ws, config.close_grace).await,
                );
            }
            InboundEvent::Timer(TimerKind::PongDeadline) => {
                // 未応答の Ping の期限切れ（モジュール doc「サーバー起点 Ping
                // keepalive」節の「Pong 期限」）。
                // `idle_timeout` と同じ正常な Close ハンドシェイクで終える。
                outbound.release();
                return (
                    CloseReason::PongTimeout,
                    close_normally(ws, config.close_grace).await,
                );
            }
            InboundEvent::Timer(TimerKind::PingDue) => {
                // 次回 Ping 予定時刻に到達しただけ（未応答の Ping はない）。
                // 反復先頭のハード判定へ戻り、そこで送出する。
                continue;
            }
            InboundEvent::Outbound(OutboundItem::Message(msg)) => {
                let frame = to_tungstenite_message(msg);
                match send_bounded(
                    &mut ws,
                    cancel.as_mut(),
                    &mut outbound.close,
                    frame,
                    stall_timeout,
                )
                .await
                {
                    SendOutcome::Cancelled => {
                        outbound.release();
                        return (
                            CloseReason::Cancelled,
                            handle_cancellation(ws, config.close_grace).await,
                        );
                    }
                    SendOutcome::Sent => {}
                    SendOutcome::CloseGraceExpired => {
                        outbound.release();
                        return close_grace_expired();
                    }
                    SendOutcome::Stalled => {
                        outbound.release();
                        return pong_timeout_expired();
                    }
                    SendOutcome::Failed(err) => return SessionFailure::send(err).into_parts(),
                }
            }
            InboundEvent::Outbound(OutboundItem::Close { code, reason }) => {
                // `WsSender::close`（イシュー #710）: 受信ループを抜けて
                // Close ハンドシェイクへ分岐する。close 時点でキュー済み
                // だった push は FIFO 順で本イベントより前に既に送出済み
                // （`WsSender` の順序保証、`handler.rs` の doc を参照）。
                let deadline = outbound.close.deadline();
                outbound.release();
                let frame = to_close_frame(code, reason);
                return (
                    CloseReason::SenderClose,
                    close_and_drain(ws, Some(frame), deadline).await,
                );
            }
            InboundEvent::Message(None) => break CloseReason::Eof,
            InboundEvent::Message(Some(message)) => {
                let message = match message {
                    Ok(message) => message,
                    Err(err) => return SessionFailure::recv(err).into_parts(),
                };
                match message {
                    Message::Text(text) => {
                        let handler_fut = config.handler.on_message_with_ctx(
                            conn_ctx,
                            WsMessage::Text(text.as_str().to_owned()),
                        );
                        match run_handler_with_outbound_drain(
                            &mut ws,
                            cancel.as_mut(),
                            &mut outbound,
                            handler_fut,
                            config.close_grace,
                            config.outbound_capacity,
                            stall_timeout,
                        )
                        .await
                        {
                            Ok(SessionFlow::Continue) => {}
                            Ok(SessionFlow::Closed) => break CloseReason::HandlerClose,
                            Ok(SessionFlow::Cancelled) => {
                                outbound.release();
                                return (
                                    CloseReason::Cancelled,
                                    handle_cancellation(ws, config.close_grace).await,
                                );
                            }
                            Ok(SessionFlow::SenderClose {
                                code,
                                reason,
                                deadline,
                            }) => {
                                // `WsSender::close`（イシュー #710）がハンドラ実行中に
                                // 呼ばれた場合。ハンドラの戻り値（Reply/Close）は
                                // `run_handler_with_outbound_drain` が既に破棄済み
                                // （Close フレームの後にデータフレームを送れない、
                                // RFC 6455 5.5.1 節）。
                                outbound.release();
                                let frame = to_close_frame(code, reason);
                                return (
                                    CloseReason::SenderClose,
                                    close_and_drain(ws, Some(frame), deadline).await,
                                );
                            }
                            Ok(SessionFlow::CloseGraceExpired) => {
                                outbound.release();
                                return close_grace_expired();
                            }
                            Ok(SessionFlow::Stalled) => {
                                outbound.release();
                                return pong_timeout_expired();
                            }
                            Err(failure) => return failure.into_parts(),
                        }
                    }
                    Message::Binary(bin) => {
                        let handler_fut = config
                            .handler
                            .on_message_with_ctx(conn_ctx, WsMessage::Binary(bin.into()));
                        match run_handler_with_outbound_drain(
                            &mut ws,
                            cancel.as_mut(),
                            &mut outbound,
                            handler_fut,
                            config.close_grace,
                            config.outbound_capacity,
                            stall_timeout,
                        )
                        .await
                        {
                            Ok(SessionFlow::Continue) => {}
                            Ok(SessionFlow::Closed) => break CloseReason::HandlerClose,
                            Ok(SessionFlow::Cancelled) => {
                                outbound.release();
                                return (
                                    CloseReason::Cancelled,
                                    handle_cancellation(ws, config.close_grace).await,
                                );
                            }
                            Ok(SessionFlow::SenderClose {
                                code,
                                reason,
                                deadline,
                            }) => {
                                // `WsSender::close`（イシュー #710）がハンドラ実行中に
                                // 呼ばれた場合。上の `Message::Text` 分岐と同一の理由
                                // （RFC 6455 5.5.1 節）でハンドラの戻り値は破棄済み。
                                outbound.release();
                                let frame = to_close_frame(code, reason);
                                return (
                                    CloseReason::SenderClose,
                                    close_and_drain(ws, Some(frame), deadline).await,
                                );
                            }
                            Ok(SessionFlow::CloseGraceExpired) => {
                                outbound.release();
                                return close_grace_expired();
                            }
                            Ok(SessionFlow::Stalled) => {
                                outbound.release();
                                return pong_timeout_expired();
                            }
                            Err(failure) => return failure.into_parts(),
                        }
                    }
                    Message::Close(_) => {
                        break CloseReason::ClientClose;
                    }
                    // Ping/Pong は tungstenite が内部で自動応答するため、Stream
                    // 経由でここへ届くのは診断用の可視化のみ。ハンドラには
                    // 委譲しない（ハンドラ契約は Text/Binary のみを扱う、
                    // `handler` モジュールの doc を参照）。
                    Message::Ping(_) | Message::Frame(_) => {}
                    Message::Pong(_) => {
                        // Pong を読んだ時点で未応答の Ping の期限を解除する
                        // （payload の照合はしない。unsolicited な Pong でも
                        // 解除する。モジュール doc「サーバー起点 Ping
                        // keepalive」節の「Pong 期限」を参照）。
                        if let Some(ka) = keepalive.as_mut() {
                            ka.pong_deadline = None;
                        }
                    }
                }
                // クライアントから実際にフレームを受信し、ハンドラ処理・返信
                // 送出（`apply_outcome`）まで完了したので、次の受信待ちに
                // 入る直前でアイドル期限を延長する（Ping/Pong/Frame を含む
                // 全種別。outbound push ではリセットしない契約はモジュール
                // doc を参照）。受信直後ではなくここで更新することで、
                // ハンドラ処理・返信送出に idle_timeout 相当の時間を要した
                // 場合でもその処理時間をアイドル待機時間に算入しない
                // （レビュー指摘対応。旧来の「受信待ちの開始直前にタイム
                // アウトを設定する」契約を回復する）。
                idle_deadline = config.idle_timeout.map(|d| Instant::now() + d);
            }
        }
    };

    (reason, Ok(()))
}

/// セッションの outbound 受信側と、その送信側（封鎖に使う）を束ねるガード
/// （PR #736 レビュー指摘対応）。[`run_session_inner`] が所有し、下位の関数へは
/// `&mut OutboundGuard` で渡す。
///
/// `WsSender` は permit の確保と確定（`commit`）の間に `.await` を挟まないが、
/// マルチスレッドではその間に受信側が drop されうる。封鎖せずに drop すると
/// 確定は `Ok` を返しつつ値が捨てられるため、受信側を手放す前に必ず
/// [`crate::handler::WsSender::seal_for_session`] を呼ぶ（封鎖後の確定は `Err`）。
///
/// 順序は言語仕様で保証する: `Drop::drop`（封鎖）はフィールドの drop より先に
/// 実行されるため、ガードがどの経路（早期 return・future の drop を含む）で
/// drop されても、`rx` の drop は必ず封鎖の後になる。途中で受信側を手放す場合は
/// [`Self::release`] を使う。
struct OutboundGuard<'a> {
    /// セッションの outbound 受信側（`None` は無効化済み）。
    rx: Option<mpsc::Receiver<OutboundItem>>,
    /// `rx` と同じチャネルの送信側（呼び出し元の契約）。封鎖にのみ使う。
    sender: &'a WsSender,
    /// close 要求後の打ち切り期限（[`CloseBound`]）。`rx` と別フィールドに
    /// 分け、`rx` を借用したままでも同時に借用できるようにする。
    close: CloseBound,
}

impl<'a> OutboundGuard<'a> {
    fn new(
        rx: Option<mpsc::Receiver<OutboundItem>>,
        sender: &'a WsSender,
        close_grace: Duration,
    ) -> Self {
        Self {
            rx,
            sender,
            close: CloseBound::new(sender.subscribe_close_request(), close_grace),
        }
    }

    /// 送信キューを封鎖する（受信側は保持したまま）。
    fn seal(&self) {
        self.sender.seal_for_session();
    }

    /// 送信キューを封鎖してから受信側を drop する。
    fn release(&mut self) {
        self.seal();
        drop(self.rx.take());
    }
}

impl Drop for OutboundGuard<'_> {
    fn drop(&mut self) {
        // フィールド `rx` はこの関数の後に drop される。
        self.seal();
    }
}

/// `WsSender::close` の要求を観測し、そこから `close_grace` 後を Close
/// ハンドシェイクの期限とする（Cursor Bugbot 指摘対応、PR #736）。
///
/// close を要求してもキューには先行する push が残りうり、キューが満杯なら
/// `close()` は空きを待って確定もできない。受信を止めたクライアント相手では
/// その送出が止まり、Close ハンドシェイクに到達しない。このため起点は確定では
/// なく要求（`WsSender::subscribe_close_request`。`close()` が `reserve` の前に
/// 記録する）とし、以後の送出（[`send_bounded`]）・Close 送出・応答待ち
/// （[`close_and_drain`]）をすべて本構造体の期限で打ち切る。close を要求して
/// いない間の送出には期限を設けない（既存の挙動）。
///
/// watch は最新値だけを保持するため、取り下げ → 新規要求（false → true）が
/// 短い間に続くと本構造体が false を見逃し、古い要求の期限が残りうる。その場合も
/// 期限が早まるだけで、`close_grace` の上限は守られる。
struct CloseBound {
    /// close 要求（確定前を含む）の有無（`WsSender::subscribe_close_request`）。
    requested: watch::Receiver<bool>,
    close_grace: Duration,
    /// 要求を観測した時刻 + `close_grace`（要求がなければ `None`）。
    deadline: Option<Instant>,
}

impl CloseBound {
    fn new(requested: watch::Receiver<bool>, close_grace: Duration) -> Self {
        Self {
            requested,
            close_grace,
            deadline: None,
        }
    }

    /// close が要求されていれば期限を返す（要求を初めて観測したときにその時刻を
    /// 起点に期限を定める）。要求がない（確定前の要求がすべて取り下げられた）
    /// なら期限を解除して `None`。
    fn observe(&mut self) -> Option<Instant> {
        if *self.requested.borrow() {
            if self.deadline.is_none() {
                self.deadline = Some(Instant::now() + self.close_grace);
            }
        } else {
            self.deadline = None;
        }
        self.deadline
    }

    /// Close ハンドシェイクに使う期限。要求を観測済みならその期限、未観測なら
    /// 今から `close_grace` 後（Close 指示を取り出した時点では close は確定済み
    /// のため、通常は前者になる）。
    fn deadline(&mut self) -> Instant {
        self.observe()
            .unwrap_or_else(|| Instant::now() + self.close_grace)
    }

    /// close 要求の観測から `close_grace` が経過したら完了する（cancel-safe）。
    /// 要求が取り下げられたら期限を解除して待ち直す。
    async fn expired(&mut self) {
        loop {
            match self.observe() {
                Some(deadline) => {
                    match race2(tokio::time::sleep_until(deadline), self.requested.changed()).await
                    {
                        Either::Left(()) => return,
                        Either::Right(Ok(())) => {}
                        Either::Right(Err(_)) => {
                            // 送信側（`WsSender`）がすべて drop された。期限は保つ。
                            tokio::time::sleep_until(deadline).await;
                            return;
                        }
                    }
                }
                None => {
                    if self.requested.changed().await.is_err() {
                        // `conn_ctx` が送信側を保持するためセッション実行中は
                        // 到達しない。期限は発火させない。
                        std::future::pending::<()>().await;
                    }
                }
            }
        }
    }
}

/// [`send_bounded`] の結果。
enum SendOutcome {
    /// 送出した。
    Sent,
    /// 送出中に cancel が発火した（呼び出し元は [`handle_cancellation`] へ）。
    Cancelled,
    /// close 要求の観測から `close_grace` が経過した（呼び出し元は
    /// [`close_grace_expired`] で終了する）。
    CloseGraceExpired,
    /// `stall_timeout` の開始時刻から期限が経過した（呼び出し元は
    /// [`pong_timeout_expired`] で終了する。Ping keepalive の「送出詰まり」
    /// 検知、モジュール doc「サーバー起点 Ping keepalive」節を参照）。
    Stalled,
    /// 送出に失敗した。
    Failed(tokio_tungstenite::tungstenite::Error),
}

/// push・返信の 1 フレームを、cancel（最優先）・送出・close 要求後の期限・
/// 送出詰まりの期限の順で race して送出する（Cursor Bugbot 指摘対応）。
///
/// `stall_timeout` は Ping keepalive（`WebSocketConfig::with_ping_interval`）
/// 有効時のみ `Some(pong_timeout)`（無効時は `None`）。**本関数の呼び出し
/// 開始時刻**からの期限として毎回新規に計算するため、呼び出し元が
/// Pong 期限をどう管理しているかとは無関係に、1 回の送出そのものが
/// `pong_timeout` を超えてブロックしないことだけを保証する（モジュール doc
/// 「サーバー起点 Ping keepalive」節の「送出詰まり」を参照）。
///
/// 期限超過・cancel で送出中の `ws.send` の future を drop しても安全である:
/// tokio-tungstenite 0.30 の `Sink::start_send` はフレームを丸ごと tungstenite
/// の書き込みバッファへ積み、`poll_flush` がそれを書き出すだけなので、drop は
/// 「未投入のフレームを捨てる」か「投入済みのフレームをバッファに残す」の
/// どちらかになり、`ws` の状態は壊れない（モジュール doc「ワイヤ安全性」節）。
/// 期限超過時は呼び出し元が `ws` への書き込みをせずにそのまま drop する。
async fn send_bounded<S, C>(
    ws: &mut WebSocketStream<S>,
    cancel: Pin<&mut C>,
    close: &mut CloseBound,
    frame: Message,
    stall_timeout: Option<Duration>,
) -> SendOutcome
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    let stall = async {
        match stall_timeout {
            Some(d) => tokio::time::sleep(d).await,
            None => std::future::pending::<()>().await,
        }
    };
    match race_cancel(cancel, race2(ws.send(frame), race2(close.expired(), stall))).await {
        None => SendOutcome::Cancelled,
        Some(Either::Left(Ok(()))) => SendOutcome::Sent,
        Some(Either::Left(Err(err))) => SendOutcome::Failed(err),
        Some(Either::Right(Either::Left(()))) => SendOutcome::CloseGraceExpired,
        Some(Either::Right(Either::Right(()))) => SendOutcome::Stalled,
    }
}

/// close 要求後に `close_grace` 以内に Close ハンドシェイクを終えられなかった
/// ときのセッション結果。[`close_and_drain`] の期限超過と同じ扱い
/// （`SenderClose` + `Ok(())`、`ws` は書き込まずに drop する）にそろえる。
fn close_grace_expired() -> (CloseReason, Result<(), WsError>) {
    (CloseReason::SenderClose, Ok(()))
}

/// Ping keepalive 有効時、1 回の送出が `stall_timeout`（`pong_timeout`）を
/// 超えてブロックしたときのセッション結果。書き込み途中で終わるため
/// Close ハンドシェイクは送らずに `ws` を drop する（送出失敗・
/// `close_grace` 超過と同じ「無理に送らない」流儀、モジュール doc
/// 「サーバー起点 Ping keepalive」節の「送出詰まり」を参照）。
fn pong_timeout_expired() -> (CloseReason, Result<(), WsError>) {
    (CloseReason::PongTimeout, Ok(()))
}

/// Ping keepalive が受信待ちの timer leg で覚醒したときの意味
/// （モジュール doc「サーバー起点 Ping keepalive」節を参照）。
#[derive(Clone, Copy)]
enum TimerKind {
    /// `idle_deadline` に到達した。
    Idle,
    /// 未応答の Ping の Pong 期限に到達した。
    PongDeadline,
    /// 未応答の Ping がない状態で、次回 Ping 予定時刻に到達した
    /// （反復先頭のハード判定へ戻るだけで、単体では終了理由にならない）。
    PingDue,
}

/// Ping keepalive（`WebSocketConfig::with_ping_interval`）のランタイム状態。
/// `run_session_inner` が `config.ping` から構築し、ループの先頭でのみ送出を
/// 判定する（モジュール doc「サーバー起点 Ping keepalive」節を参照）。
struct Keepalive {
    /// Ping 送出間隔。
    interval: Duration,
    /// Ping 送出から Pong を待つ上限。
    pong_timeout: Duration,
    /// 次に Ping を送出すべき時刻（前回送出時刻 + `interval`。未応答の
    /// Ping がある間は参照されない）。
    next_ping_at: Instant,
    /// 未応答の Ping の Pong 期限（`None` は未送出、または Pong 受信済み）。
    pong_deadline: Option<Instant>,
}

impl Keepalive {
    fn new(config: PingKeepalive) -> Self {
        Self {
            interval: config.interval,
            pong_timeout: config.pong_timeout,
            next_ping_at: Instant::now() + config.interval,
            pong_deadline: None,
        }
    }

    /// 受信待ちの timer leg に使う覚醒時刻と、それが意味する [`TimerKind`]。
    /// 未応答の Ping があればその Pong 期限、なければ次回 Ping 予定時刻
    /// （反復先頭のハード判定へ戻すだけの覚醒）を返す。
    fn timer_leg(&self) -> (Instant, TimerKind) {
        match self.pong_deadline {
            Some(deadline) => (deadline, TimerKind::PongDeadline),
            None => (self.next_ping_at, TimerKind::PingDue),
        }
    }
}

/// クライアント受信待ちの 1 イベント（[`run_session`] のループが処理する
/// 単位）。`Timer` はアイドルタイムアウト・Ping keepalive のいずれかの
/// タイマー発火、`Outbound` はサーバー起点メッセージ
/// （[`crate::handler::WsSender`]、イシュー #670）到着を表す。
enum InboundEvent {
    /// `ws.next()` の結果（`None` は EOF、`Some(Err(_))` はプロトコル/IO
    /// エラー）。
    Message(Option<Result<Message, tokio_tungstenite::tungstenite::Error>>),
    /// `idle_deadline`・Ping keepalive のいずれかのタイマーに到達した
    /// （[`TimerKind`] が種別を運ぶ）。
    Timer(TimerKind),
    /// [`crate::handler::WsSender`] からの push メッセージ、または
    /// [`crate::handler::WsSender::close`] の Close 指示（イシュー #710。
    /// 内部表現 [`OutboundItem`] のまま運び、分岐は消費側（呼び出し元の
    /// `match`）に委ねる）。
    Outbound(OutboundItem),
}

/// 2 つの `Future` を手動 race させる汎用ヘルパー（`crate::race_cancel` と
/// 同型、`std::future::poll_fn` + `std::pin::pin!` のみで構成、追加依存
/// なし）。`a` を `b` より先にポーリングする bias を持ち、同時に両方が
/// 完了可能な場合は `a` を優先する決定的な順序を与える（[`run_session`]
/// では `a` にクライアント受信、`b` に outbound 受信を渡し、クライアント
/// 側を優先する）。
async fn race2<A, B>(a: A, b: B) -> Either<A::Output, B::Output>
where
    A: Future,
    B: Future,
{
    let mut a = std::pin::pin!(a);
    let mut b = std::pin::pin!(b);
    std::future::poll_fn(|cx| {
        if let Poll::Ready(output) = a.as_mut().poll(cx) {
            return Poll::Ready(Either::Left(output));
        }
        if let Poll::Ready(output) = b.as_mut().poll(cx) {
            return Poll::Ready(Either::Right(output));
        }
        Poll::Pending
    })
    .await
}

/// [`race2`] の戻り値（どちらの `Future` が先に完了したかを表す）。
enum Either<L, R> {
    Left(L),
    Right(R),
}

/// [`race2`] の公平版。`prefer_b` で「両方の `Future` が同時に Ready な
/// 場合にどちらを先にポーリングするか」を呼び出し側から指定できる
/// （`false` なら `race2` と同じ `a` 優先、`true` なら `b` 優先）。
///
/// [`run_session`] が inbound（クライアント受信 + アイドル期限）と
/// outbound（[`crate::handler::WsSender`] からの push）を合流させる際、
/// 反復ごとに `prefer_b` を反転させて呼ぶことで両者の優先順位を交互化し、
/// 固定優先度による飢餓（連続受信クライアント下で outbound 側が恒久的に
/// 選ばれなくなる退行）を避ける（イシュー #684、モジュール doc の
/// 「両方 Ready 時」節を参照）。`a`・`b` いずれか一方のみが Ready な
/// 通常時は `prefer_b` の値に関わらず結果が変わらない（Pending の側は
/// 単に素通りするため）。
async fn race2_alternating<A, B>(prefer_b: bool, a: A, b: B) -> Either<A::Output, B::Output>
where
    A: Future,
    B: Future,
{
    let mut a = std::pin::pin!(a);
    let mut b = std::pin::pin!(b);
    std::future::poll_fn(move |cx| {
        if prefer_b {
            if let Poll::Ready(output) = b.as_mut().poll(cx) {
                return Poll::Ready(Either::Right(output));
            }
            if let Poll::Ready(output) = a.as_mut().poll(cx) {
                return Poll::Ready(Either::Left(output));
            }
        } else {
            if let Poll::Ready(output) = a.as_mut().poll(cx) {
                return Poll::Ready(Either::Left(output));
            }
            if let Poll::Ready(output) = b.as_mut().poll(cx) {
                return Poll::Ready(Either::Right(output));
            }
        }
        Poll::Pending
    })
    .await
}

/// [`WsMessage`] を tungstenite の `Message` へ変換する（[`apply_outcome`]
/// の `WsOutcome::Reply` 送出・[`run_session`] の outbound 送出の両方で
/// 共有する）。
fn to_tungstenite_message(msg: WsMessage) -> Message {
    match msg {
        WsMessage::Text(t) => Message::Text(t.into()),
        WsMessage::Binary(b) => Message::Binary(b.into()),
    }
}

/// [`crate::handler::WsSender::close`] が enqueue した `(code, reason)` を
/// tungstenite の `CloseFrame` へ変換する（イシュー #710）。`code` の RFC
/// 6455 許可判定・`reason` の長さ検証は `WsSender::close` の呼び出し時点
/// （API 境界、`handler.rs`）で既に完了しているため、本関数では再検証しない
/// （検証の重複を避け、責務を 1 箇所に集約する）。
fn to_close_frame(code: u16, reason: String) -> CloseFrame {
    CloseFrame {
        code: CloseCode::from(code),
        reason: Utf8Bytes::from(reason),
    }
}

/// [`apply_outcome`] / [`run_handler_with_outbound_drain`] の内部失敗表現
/// （イシュー #726、設計 4 節「内部失敗表」）。[`run_session_inner`] が
/// 返すべき [`CloseReason`] と、呼び出し元へ伝播する `WsError` を 1 個の
/// 値としてまとめて運ぶ非公開型（公開 API には出さない）。
///
/// 受信側・送信側で分類器を分ける（[`Self::recv`] / [`Self::send`]）。
/// tungstenite の送信は受信と異なるエラー分布を返しうるため、単一の
/// 分類器を共用すると設計 4 節の対応表からずれる。方向を取り違えないよう
/// `From<tungstenite::Error> for SessionFailure` は意図的に実装しない
/// （`?` による暗黙変換で誤った分類器を通す経路を作らないため。呼び出し元
/// は必ず `SessionFailure::recv(err)` / `SessionFailure::send(err)` を
/// 明示的に選ぶ）。
struct SessionFailure {
    reason: CloseReason,
    error: WsError,
}

impl SessionFailure {
    /// 受信失敗（`ws.next()` が返した `Err`）を分類する。
    ///
    /// `Capacity`（メッセージ/フレームサイズ上限超過）は
    /// [`CloseReason::MessageTooLarge`] へ、`Io` は
    /// [`FailureKind::Io`] へ倒す。`Protocol(ResetWithoutClosingHandshake)`
    /// は tokio-tungstenite 0.30 で Close フレームなしの TCP 切断が実際に
    /// 観測される経路（`ws.next()` が `None` を返す `InboundEvent::
    /// Message(None)` はこの構成では実質到達しない）であり、
    /// [`CloseReason::Eof`] の doc が定義する事象と 1:1 対応するため
    /// [`CloseReason::Eof`] へ分類する（イシュー #726 レビュー指摘対応。
    /// `Result` 側は引き続き `Err`（呼び出し元は読み取り自体が失敗した
    /// ことを判別できる。`MessageTooLarge`/`Err(Capacity(_))` と同型の
    /// 「正常系 reason + Err」の組み合わせ）。その他の分類不能なエラー
    /// （`ConnectionClosed`/`AlreadyClosed`/その他 `Protocol`/`Tls` 等）は
    /// [`FailureKind::Protocol`] へ倒す（設計 4 節の脱出点対応表。
    /// フェイルクローズ: 分類不能なエラーは `Eof`/`ClientClose` へ
    /// 倒さない）。
    fn recv(err: tokio_tungstenite::tungstenite::Error) -> Self {
        use tokio_tungstenite::tungstenite::error::ProtocolError;

        let reason = match &err {
            tokio_tungstenite::tungstenite::Error::Capacity(_) => CloseReason::MessageTooLarge,
            tokio_tungstenite::tungstenite::Error::Io(_) => CloseReason::Failed(FailureKind::Io),
            tokio_tungstenite::tungstenite::Error::Protocol(
                ProtocolError::ResetWithoutClosingHandshake,
            ) => CloseReason::Eof,
            _ => CloseReason::Failed(FailureKind::Protocol),
        };
        Self {
            reason,
            error: WsError::from(err),
        }
    }

    /// 送信失敗（`ws.send`/`ws.close` が返した `Err`）を分類する。
    ///
    /// 受信側と異なり、`Capacity`（送信時は「メッセージがサイズ上限を
    /// 超える」を意味する）・`ConnectionClosed`/`AlreadyClosed` を含む
    /// 非 `Io` エラーはすべて [`FailureKind::Protocol`] へ倒す（設計 4 節
    /// レビュー指摘対応: `ConnectionClosed`/`AlreadyClosed` での送信失敗を
    /// [`CloseReason::ClientClose`] に誤分類しない）。
    fn send(err: tokio_tungstenite::tungstenite::Error) -> Self {
        let reason = match &err {
            tokio_tungstenite::tungstenite::Error::Io(_) => CloseReason::Failed(FailureKind::Io),
            _ => CloseReason::Failed(FailureKind::Protocol),
        };
        Self {
            reason,
            error: WsError::from(err),
        }
    }

    /// ユーザーハンドラ（`WsMessageHandler::on_message_with_ctx`）が
    /// `Err` を返した場合の失敗（[`FailureKind::Handler`] 固定）。
    fn handler(err: WsHandlerError) -> Self {
        Self {
            reason: CloseReason::Failed(FailureKind::Handler),
            error: WsError::from(err),
        }
    }

    /// [`run_session_inner`] の戻り値型 `(CloseReason, Result<(), WsError>)`
    /// への変換ヘルパー。
    fn into_parts(self) -> (CloseReason, Result<(), WsError>) {
        (self.reason, Err(self.error))
    }
}

/// [`apply_outcome`] の戻り値。セッションループ（[`run_session_inner`]）が
/// 次に取るべき動作を表す（イシュー #499 で `Result<bool, WsError>` から
/// 拡張し、キャンセル打ち切りを独立した分岐として表現できるようにした）。
enum SessionFlow {
    /// 返信送出まで完了し、セッションを継続する（`WsOutcome::Reply`）。
    Continue,
    /// Close ハンドシェイクを開始済みで、セッションを正常終了する
    /// （`WsOutcome::Close`）。送信キューは `ws.close()` の前に
    /// `flush_outbound` で flush 済み（イシュー #711）。
    Closed,
    /// 送出中にキャンセルが発火し、当該 `Future` を打ち切った。呼び出し元は
    /// [`handle_cancellation`] へ分岐する。
    Cancelled,
    /// [`crate::handler::WsSender::close`] によるサーバー起点の Close 指示を
    /// 受け取った（イシュー #710。[`run_handler_with_outbound_drain`] が
    /// ハンドラ実行中の outbound 消化中に検出する）。呼び出し元はハンドラの
    /// 戻り値（`WsOutcome::Reply`/`Close`）を破棄し、`code`/`reason` で
    /// Close ハンドシェイクへ分岐する（Close フレームの後にデータフレームを
    /// 送れない、RFC 6455 5.5.1 節）。
    SenderClose {
        /// 検証済みの close code（[`crate::handler::WsSender::close`] の
        /// 呼び出し時点で RFC 6455 の許可判定を通過済み）。
        code: u16,
        /// 検証済みの close reason（123 バイト以内）。
        reason: String,
        /// Close ハンドシェイク（[`close_and_drain`]）に使う期限。close 要求の
        /// 観測から `close_grace` 後（[`CloseBound::deadline`]）で、終了経路の
        /// 排出（[`flush_outbound`]）中に見つかった場合はその排出と共有する期限と
        /// 早い方を使う。いずれの経路でも、close 要求から Close ハンドシェイク
        /// 完了までを `close_grace` 以内に収める。
        deadline: Instant,
    },
    /// close 要求の観測から `close_grace` 以内に送出を終えられなかった
    /// （[`SendOutcome::CloseGraceExpired`]）。呼び出し元は
    /// [`close_grace_expired`] でセッションを終える。
    CloseGraceExpired,
    /// Ping keepalive 有効時、送出が `stall_timeout` を超えてブロックした
    /// （[`SendOutcome::Stalled`]）。呼び出し元は [`pong_timeout_expired`]
    /// でセッションを終える。
    Stalled,
}

/// [`crate::handler::WsMessageHandler::on_message`] の戻り値をセッション
/// ループへ反映する。`WsOutcome::Reply` の各 `ws.send` / `WsOutcome::Close`
/// の `ws.close` を `cancel` と race させ（イシュー #499）、キャンセルが
/// 送出中に発火した場合は当該 `Future` を drop して
/// [`SessionFlow::Cancelled`] を返す。`ws` は呼び出し元が引き続き所有する
/// ため、打ち切り後も `WebSocketStream` 内部のフレーミングバッファ状態
/// （書き込み位置）は保たれ、後続の Close 送出が破損したバイト列を生まない
/// （モジュール doc の「ワイヤ安全性」節を参照）。
///
/// `outbound`（受信側と封鎖用の送信側を束ねた [`OutboundGuard`]）は、
/// `WsOutcome::Close` 分岐では [`flush_outbound`] へ委譲して使う（イシュー
/// #711）。`WsOutcome::Reply` ではセッションが継続するため送信キューを閉じず、
/// 送出を始める直前（間に `.await` を挟まない）に `close()` の確定を
/// `WsSender::close_committed` で判定する（Reply の判定点。確定済みなら Reply を
/// 破棄して [`drain_to_close`] へ進む。PR #736 codex P1 指摘対応）。
///
/// `close_deadline` は Close ハンドシェイク全体が共有する単一の期限
/// （`tokio::time::Instant`）で、呼び出し元が `close_grace` から 1 回だけ計算して
/// 渡す（イシュー #711 PR #735 レビュー指摘対応）。[`flush_outbound`] の排出と
/// 本関数の `ws.close(None)` 送出、および排出中に Close 指示が見つかった場合の
/// その Close ハンドシェイク（[`SessionFlow::SenderClose`] の `deadline` として
/// 引き継ぐ）がこの期限を共有し、合計を `close_grace` 以内に収める。cancel が
/// 発火した場合は [`handle_cancellation`] がその時点から別に `close_grace` を
/// 数える。
async fn apply_outcome<S, C>(
    ws: &mut WebSocketStream<S>,
    outcome: WsOutcome,
    outbound: &mut OutboundGuard<'_>,
    mut cancel: Pin<&mut C>,
    close_deadline: Instant,
    stall_timeout: Option<Duration>,
) -> Result<SessionFlow, SessionFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    match outcome {
        WsOutcome::Reply(messages) => {
            // Reply 送出の判定点（PR #736 codex P1 指摘対応）: 送出を始める直前
            // （この判定と最初の送出の間に `.await` を挟まない）に、`close()` の
            // 確定フラグを `WsSender::commit` と同じロックで確認する。確定済みなら
            // Reply を破棄し、Close 指示まで排出して `SenderClose` へ進む。
            if outbound.sender.close_committed()
                && let Some(flow) =
                    drain_to_close(ws, cancel.as_mut(), outbound, stall_timeout).await?
            {
                return Ok(flow);
            }
            for msg in messages {
                let frame = to_tungstenite_message(msg);
                match send_bounded(
                    ws,
                    cancel.as_mut(),
                    &mut outbound.close,
                    frame,
                    stall_timeout,
                )
                .await
                {
                    SendOutcome::Cancelled => return Ok(SessionFlow::Cancelled),
                    SendOutcome::Sent => {}
                    SendOutcome::CloseGraceExpired => return Ok(SessionFlow::CloseGraceExpired),
                    SendOutcome::Stalled => return Ok(SessionFlow::Stalled),
                    SendOutcome::Failed(err) => return Err(SessionFailure::send(err)),
                }
            }
            Ok(SessionFlow::Continue)
        }
        WsOutcome::Close => {
            // イシュー #711: Close 送出前に、送信キューを閉じてから閉鎖時点の
            // バッファ済み項目を排出する（手順・保証は [`flush_outbound`] の
            // doc を参照）。`close_deadline` を共有するため、排出に要した分だけ
            // 後続の `ws.close` に残る猶予は縮む。
            match flush_outbound(ws, outbound, cancel.as_mut(), close_deadline).await? {
                FlushOutcome::Cancelled => return Ok(SessionFlow::Cancelled),
                // イシュー #711 Codex レビュー指摘対応: `close_deadline` 超過
                // まで排出できなかった場合、クライアントが受信を止めている
                // 可能性が高く、続く `ws.close(None)` の書き込みも同様に
                // 長時間ブロックしうる（二次 DoS）。Close ハンドシェイクの
                // 送出自体を諦め、`SessionFlow::Closed` を返して即座に
                // セッションを終える（呼び出し元がストリームを drop し
                // TCP 接続を終了する。モジュール doc「ワイヤ安全性」節が
                // 述べる「打ち切り後の drop は安全」契約と同型）。
                FlushOutcome::TimedOut => return Ok(SessionFlow::Closed),
                // イシュー #710: 送信キューの封鎖前に `WsSender::close` が確定
                // させた Close 指示を排出中に見つけた場合。`close()` が `Ok` を
                // 返した以上その code/reason を届ける契約（`handler.rs` の
                // `commit` doc）を優先し、`ws.close(None)` 送出はスキップして
                // `SessionFlow::SenderClose` へ分岐する（キュー中の Close は
                // 必ず最後の要素のため、後続の排出対象は残らない）。
                FlushOutcome::SenderClose {
                    code,
                    reason,
                    deadline,
                } => {
                    return Ok(SessionFlow::SenderClose {
                        code,
                        reason,
                        deadline,
                    });
                }
                FlushOutcome::Done => {}
            }
            // イシュー #711 PR #735 レビュー指摘対応（P1 #2）: `flush_outbound`
            // が期限内に完了した後も、`ws.close(None)` 自体の書き込みが
            // クライアントの受信停止で無期限にブロックしうる。cancel との
            // race だけでは打ち切れないため、`flush_outbound` と同じ
            // `close_deadline` で `timeout_at` し、超過時は
            // [`SessionFlow::Closed`] を返してストリームを drop する
            // （送出済みでない Close フレームを諦める。モジュール doc
            // 「ワイヤ安全性」節の契約と同型、DoS 耐性を維持）。
            match race_cancel(
                cancel.as_mut(),
                tokio::time::timeout_at(close_deadline, ws.close(None)),
            )
            .await
            {
                None => return Ok(SessionFlow::Cancelled),
                Some(Ok(Ok(()))) => {}
                Some(Ok(Err(err))) => return Err(SessionFailure::send(err)),
                Some(Err(_timeout_elapsed)) => return Ok(SessionFlow::Closed),
            }
            Ok(SessionFlow::Closed)
        }
    }
}

/// [`flush_outbound`] の戻り値。`SessionFlow` とは意味が異なる（flush 自体の
/// 完了/中断を表すのみで、セッション継続/終了の判断はここでは行わない）ため
/// 専用の小さな型として分ける。
enum FlushOutcome {
    /// キューを閉じ、閉鎖前に確定済みだったメッセージ（閉鎖時点で未返却
    /// だった permit による確定分を含む）をすべて送出した（`outbound` が
    /// `None` の場合を含む）。
    Done,
    /// 送出中にキャンセルが発火し、当該 `Future` を打ち切った。呼び出し元は
    /// [`SessionFlow::Cancelled`] へ分岐する。
    Cancelled,
    /// `close_grace` の期限内に排出を完了できなかった（イシュー #711
    /// Codex レビュー指摘対応）。クライアントが受信を止めている等で
    /// `ws.send()` が長時間ブロックしている状態であり、呼び出し元は
    /// 以後の Close 送出を試みず即座にセッションを終了させる
    /// （[`apply_outcome`] の `FlushOutcome::TimedOut` 分岐を参照）。
    TimedOut,
    /// 排出中に [`crate::handler::WsSender::close`] が確定させた Close
    /// 指示を見つけた（イシュー #710。キュー中の Close は
    /// `WsSender::commit` の契約上必ず最後の要素）。呼び出し元は
    /// [`SessionFlow::SenderClose`] へ分岐し、以後の `ws.close(None)` 送出
    /// をスキップする。
    SenderClose {
        /// 検証済みの close code。
        code: u16,
        /// 検証済みの close reason（123 バイト以内）。
        reason: String,
        /// Close ハンドシェイクに使う期限（排出の期限と、close 要求の観測から
        /// `close_grace` 後の早い方）。
        deadline: Instant,
    },
}

/// セッションを終える経路（ハンドラの `Err`・`WsOutcome::Close`）で、送信
/// キューを封鎖してから封鎖時点のバッファ済み項目を排出する（イシュー #711 で
/// 導入し、PR #736 で終了経路共通の排出に拡張。設計は
/// `docs/design/ws-connection-context-and-close.md` 6 節・12 節）。
///
/// # 手順
///
/// 1. [`OutboundGuard::seal`]（[`crate::handler::WsSender::seal_for_session`]）で
///    封鎖する。
///    `WsSender::commit` と同じロック区間で封鎖状態を立てるため、以後の
///    `WsSender::send`/`close` は `Err` になり、満杯キューで待機中の呼び出しも
///    解放される。
/// 2. `outbound` が `Some` なら [`mpsc::Receiver::close`] で受信側も閉じる
///    （`WsSender::closed()` の完了と、`reserve()` 待ちの解放を確実にする）。
/// 3. `try_recv()` が `Empty`/`Disconnected` を返すまで取り出し、`Message` は
///    到着順に `ws.send()` で送出する（送出は `cancel` と race させる、
///    イシュー #499）。`Close` を見つけたら [`FlushOutcome::SenderClose`] を
///    返す（`WsSender::commit` の契約上、Close は常にキューの最後の要素）。
/// 4. 手順 3 全体を `close_deadline`（呼び出し元が `close_grace` から 1 回だけ
///    計算し、後続の `ws.close(None)` と共有する単一期限、イシュー #711
///    PR #735）で有界化し、超過時は残りを諦めて [`FlushOutcome::TimedOut`] を
///    返す（クライアントが受信を止めた場合の DoS 対策）。封鎖より前に close が
///    確定していれば、その観測から `close_grace` 後（[`CloseBound`]）と早い方を
///    期限にする。
///
/// **保証**: 手順 1 より前に `WsSender::send`/`close` が `Ok` を返した項目は、
/// `close_deadline` 超過・cancel・送出失敗（以後の項目と Close フレームも送出
/// されない）で打ち切られない限り、本関数が返るまでに送出（Close 指示は
/// `SenderClose` として返却）され、手順 1 より後の呼び出しは `Err` を返す。
///
/// # 取りこぼしがなく有界である根拠（tokio のバージョンに依存しない）
///
/// - 封鎖と `WsSender::commit` の `Permit::send` は同じ `Mutex` のロック区間で
///   行われるため、封鎖より前に `Ok` を返した項目は手順 3 の開始時点ですでに
///   キューに入っている。permit を確保済みでも確定前の送信者は封鎖後に
///   `Err` になり、項目を積まない。したがって `try_recv()` の `Empty` で
///   打ち切っても取りこぼさず、受信側の起床（`recv().await`）を待つ必要がない。
/// - 封鎖後は新しい項目が積まれないため、取り出す件数はチャネル容量以下で
///   構造的に有界になる（送り続ける別タスクがいても終わる）。
/// - `Permit::send` はロック区間内で完了するため、封鎖後に書き込み途中の
///   送信者はおらず、`try_recv()` が書き込み完了を待って park することもない。
async fn flush_outbound<S, C>(
    ws: &mut WebSocketStream<S>,
    outbound: &mut OutboundGuard<'_>,
    mut cancel: Pin<&mut C>,
    close_deadline: Instant,
) -> Result<FlushOutcome, SessionFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    // 封鎖より前に close が要求されていれば、その観測から `close_grace` 後と
    // 呼び出し元の期限の早い方を使う（[`CloseBound`]）。
    let close_deadline = outbound
        .close
        .observe()
        .map_or(close_deadline, |observed| observed.min(close_deadline));
    outbound.seal();
    let Some(rx) = outbound.rx.as_mut() else {
        return Ok(FlushOutcome::Done);
    };
    rx.close();

    // `tokio::time::timeout_at` に渡すため排出ループを 1 個の `Future` に
    // まとめる（`ws`・`rx`・`cancel` は借用のみで、await の外へ持ち出さない）。
    let drain = async {
        loop {
            match rx.try_recv() {
                Ok(OutboundItem::Message(msg)) => {
                    let frame = to_tungstenite_message(msg);
                    match race_cancel(cancel.as_mut(), ws.send(frame)).await {
                        None => return Ok(FlushOutcome::Cancelled),
                        Some(Ok(())) => {}
                        Some(Err(err)) => return Err(SessionFailure::send(err)),
                    }
                }
                Ok(OutboundItem::Close { code, reason }) => {
                    return Ok(FlushOutcome::SenderClose {
                        code,
                        reason,
                        deadline: close_deadline,
                    });
                }
                Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                    return Ok(FlushOutcome::Done);
                }
            }
        }
    };

    match tokio::time::timeout_at(close_deadline, drain).await {
        Ok(result) => result,
        Err(_timeout_elapsed) => Ok(FlushOutcome::TimedOut),
    }
}

/// セッションが続く経路（ハンドラが `WsOutcome::Reply` を返した）で、Reply
/// 送出前に行う有界な排出（PR #736 codex P0 レビュー指摘対応）。
///
/// # 手順
///
/// 1. 受信側は閉じずに、`try_recv()` を最大 `capacity` 回行う。`capacity` は
///    呼び出し元が渡すチャネル容量で、`outbound` を生成した `handler::channel`
///    に渡した値と一致させる（呼び出し元の契約。現状は
///    [`run_handler_with_outbound_drain`] が `outbound_capacity` 引数
///    （`crate::lib::handle_upgrade` が `handler::channel` へ渡した値と同じ
///    `WebSocketConfig::outbound_capacity`）を渡す 1 か所のみ、イシュー
///    #709）。
/// 2. `Message` は到着順に送出する（[`send_bounded`]）。`Close` を見つけたら
///    Reply を破棄して `Some(SessionFlow::SenderClose)` を返す。`Empty` で
///    打ち切り、`Disconnected` なら `outbound` を無効化して打ち切る。
/// 3. 打ち切ったら `None` を返す。残りはキューに残ったまま外側ループが処理する
///    ため失われない。close が確定済みかどうか（Reply を送るか）は、呼び出し元
///    [`apply_outcome`] が Reply 送出の直前に判定する。
///
/// **保証**: 本関数の開始時点でキューに格納済みだった push は Reply より先に
/// 送出される（格納済み件数は容量以下で FIFO の先頭に並ぶため、手順 1 の回数で
/// すべて取り出せる。`capacity` がチャネル容量より小さい場合はこの限りでない）。
///
/// これ以外の push と Reply の相対順序は不定。
///
/// `try_recv()` は別送信者の書き込み途中（tokio 内部の `Busy`）に当たると、
/// その書き込みが終わるまでワーカースレッドをごく短時間 park しうる（書き込みは
/// 同期区間で完了するため有界で、回数も `capacity` 以下）。
async fn drain_before_reply<S, C>(
    ws: &mut WebSocketStream<S>,
    mut cancel: Pin<&mut C>,
    outbound: &mut OutboundGuard<'_>,
    capacity: usize,
    stall_timeout: Option<Duration>,
) -> Result<Option<SessionFlow>, SessionFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    for _ in 0..capacity {
        let Some(rx) = outbound.rx.as_mut() else {
            return Ok(None);
        };
        match rx.try_recv() {
            Ok(OutboundItem::Message(msg)) => {
                let frame = to_tungstenite_message(msg);
                match send_bounded(
                    ws,
                    cancel.as_mut(),
                    &mut outbound.close,
                    frame,
                    stall_timeout,
                )
                .await
                {
                    SendOutcome::Cancelled => return Ok(Some(SessionFlow::Cancelled)),
                    SendOutcome::Sent => {}
                    SendOutcome::CloseGraceExpired => {
                        return Ok(Some(SessionFlow::CloseGraceExpired));
                    }
                    SendOutcome::Stalled => return Ok(Some(SessionFlow::Stalled)),
                    SendOutcome::Failed(err) => return Err(SessionFailure::send(err)),
                }
            }
            Ok(OutboundItem::Close { code, reason }) => {
                // Close フレームの後にデータフレームを送れない（RFC 6455 5.5.1 節）
                // ため、ハンドラの Reply は破棄する。
                return Ok(Some(SessionFlow::SenderClose {
                    code,
                    reason,
                    deadline: outbound.close.deadline(),
                }));
            }
            Err(mpsc::error::TryRecvError::Empty) => return Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                // 全 `WsSender` クローンが drop 済み（`conn_ctx` がクローンを
                // 保持するためセッション実行中は到達しない防御的コード）。
                outbound.release();
                return Ok(None);
            }
        }
    }
    Ok(None)
}

/// `close()` の確定を判定した後（[`apply_outcome`] の Reply 送出の判定点）、
/// キューを Close 指示まで排出する（PR #736 codex P1 指摘対応）。
///
/// # 手順
///
/// 1. `try_recv()` で取り出し、`Message` は到着順に送出する（[`send_bounded`]。
///    close 要求の観測から `close_grace` で打ち切る）。
/// 2. `Close` を見つけたら `Some(SessionFlow::SenderClose)` を返す。
///
/// **保証**: 呼び出し元が `WsSender::close_committed` で確定を判定した時点で
/// Close 指示はキューにあり（`commit` と同じロックの前後関係）、確定後は新規の
/// enqueue が `Err` になるため、Close 指示までの件数はチャネル容量以下で必ず
/// 見つかる。
///
/// 受信側が無効化済み・空・`Disconnected`（いずれも継続経路では到達しない
/// 防御分岐）の場合は `None` を返し、呼び出し元は Reply の送出へ進む（以後の
/// 送出は [`CloseBound`] の期限で打ち切られる）。
async fn drain_to_close<S, C>(
    ws: &mut WebSocketStream<S>,
    mut cancel: Pin<&mut C>,
    outbound: &mut OutboundGuard<'_>,
    stall_timeout: Option<Duration>,
) -> Result<Option<SessionFlow>, SessionFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    loop {
        let Some(rx) = outbound.rx.as_mut() else {
            return Ok(None);
        };
        match rx.try_recv() {
            Ok(OutboundItem::Message(msg)) => {
                let frame = to_tungstenite_message(msg);
                match send_bounded(
                    ws,
                    cancel.as_mut(),
                    &mut outbound.close,
                    frame,
                    stall_timeout,
                )
                .await
                {
                    SendOutcome::Cancelled => return Ok(Some(SessionFlow::Cancelled)),
                    SendOutcome::Sent => {}
                    SendOutcome::CloseGraceExpired => {
                        return Ok(Some(SessionFlow::CloseGraceExpired));
                    }
                    SendOutcome::Stalled => return Ok(Some(SessionFlow::Stalled)),
                    SendOutcome::Failed(err) => return Err(SessionFailure::send(err)),
                }
            }
            Ok(OutboundItem::Close { code, reason }) => {
                return Ok(Some(SessionFlow::SenderClose {
                    code,
                    reason,
                    deadline: outbound.close.deadline(),
                }));
            }
            Err(mpsc::error::TryRecvError::Empty) => {
                // Close 指示は確定フラグと同じロック区間でキューへ積まれるため、
                // 確定後に Close 指示より先に空になることはない（到達不能の前提を
                // debug ビルドで固定する）。
                debug_assert!(
                    !outbound.sender.close_committed(),
                    "close() の確定を判定した後に Close 指示より先に送信キューが空になった"
                );
                return Ok(None);
            }
            Err(mpsc::error::TryRecvError::Disconnected) => {
                outbound.release();
                return Ok(None);
            }
        }
    }
}

/// [`crate::handler::WsMessageHandler::on_message_with_ctx`] のハンドラ
/// `Future` を実行し、その `await` 中に到着した outbound push
/// （[`crate::handler::WsSender`]）を都度そのまま送出しつつ完了を待つ
/// （イシュー #706、設計は `docs/design/ws-connection-context-and-close.md`
/// 6 節）。
///
/// # 解決する問題（自己送信デッドロック）
///
/// 旧実装はハンドラ Future を単独 `await` していたため、`on_message_with_ctx`
/// 内で `ctx.sender().send(...).await` を呼んでも、その outbound チャネルを
/// 消化する者（本関数自身）がハンドラ完了まで戻ってこず、設定された容量
/// （`WebSocketConfig::outbound_capacity`、既定 8、イシュー #709）を超えると
/// 送信側・受信側の両方が進めなくなっていた。本関数はハンドラ Future と outbound 到着を
/// `race2`（cancel を最優先とした 3 者 race）し、到着ごとに即座に `ws.send()`
/// で送出することでこれを解消する。
///
/// # 手順（設計 6 節）
///
/// 1. ハンドラ Future が `Poll::Ready` を返すまで、cancel（最優先）→
///    (ハンドラ完了 | outbound 到着) を反復ポーリングし、到着した push は
///    その都度 `ws.send()` で送出する。Close 指示が届いたらハンドラ Future を
///    drop して [`SessionFlow::SenderClose`] を返す。
/// 2. ハンドラの結果で排出方法を分ける（PR #736 codex P0/P1 レビュー指摘対応）。
///    - `Ok(WsOutcome::Reply)`（継続経路）: [`drain_before_reply`] で受信側を
///      閉じずに設定された容量（`outbound_capacity` 引数）回まで
///      排出してから [`apply_outcome`] へ進む。`apply_outcome` は Reply の送出を
///      始める直前に `close()` の確定をロックで判定し（`WsSender::close_committed`）、
///      確定済みなら Reply を破棄して Close 指示まで排出する
///      （[`drain_to_close`]）。未確定なら Reply を送出する。
///    - `Ok(WsOutcome::Close)`（終了経路）: [`apply_outcome`] が
///      [`flush_outbound`]（送信キューを封鎖してから排出）を経て Close
///      フレームを送出する。
///    - `Err`（終了経路）: [`flush_outbound`] で排出する。Close 指示が見つかれば
///      [`SessionFlow::SenderClose`]、cancel が発火すれば
///      [`SessionFlow::Cancelled`] を返し、それ以外（排出完了・期限超過・
///      排出中の送信失敗）は元のハンドラエラーを [`FailureKind::Handler`] と
///      して返す（送信失敗でハンドラエラーを上書きしない）。
///
/// **保証**: 継続経路では排出開始時点で格納済みの push が Reply より先に送出
/// され、Reply 送出直前の判定（`closing` のロックでの確認）より前に `close()` が
/// 確定（同じロック区間でのフラグ更新）していれば Reply は送出されず、
/// 終了経路では封鎖より前に `WsSender::send`/`close` が `Ok` を返した
/// 項目が（`close_grace` 超過・cancel・送出失敗（以後の項目と Close フレームも
/// 送出されない）で打ち切られない限り）すべて送出され、封鎖より後の呼び出しは
/// `Err` を返す。
///
/// 継続経路で排出開始後に格納された push と Reply の相対順序のみ不定（判定より
/// 後に確定した close では Reply が Close より先に送出されうるが、close 要求の
/// 観測から `close_grace` で打ち切る）。
///
/// outbound 到着時の `ws.send()` 失敗・cancel 発火時の扱いは
/// [`run_session`] 外側ループの `InboundEvent::Outbound` 分岐と同一
/// （送信失敗は `WsError` へ変換して終了、cancel 発火は
/// [`SessionFlow::Cancelled`] を返す）。`idle_deadline` は本関数の実行中は
/// 更新しない（モジュール doc の「クライアントから実際にフレームを受信した
/// 場合にのみ延長」契約を変えない）。
///
/// **既知の限界（イシュー #706 スコープ外）**: `idle_deadline` は本関数の
/// 実行中「更新されない」だけでなく「評価もされない」。アイドルタイムアウト
/// の判定（[`run_session_inner`] 外側ループの `tokio::time::timeout`）は
/// ハンドラ完了後に受信待ちへ戻ってから初めて働くため、ハンドラ自体が
/// `idle_timeout` を超える時間 `await` し続けても、その間は無通信であっても
/// アイドルタイムアウトは発火しない（ハンドラの実行時間そのものは監視対象
/// 外というポリシー上の判断であり、本関数が解消する「outbound 送信キューの
/// デッドロック」とは別種の懸念のため、本イシューでは対処しない）。
async fn run_handler_with_outbound_drain<S, C>(
    ws: &mut WebSocketStream<S>,
    mut cancel: Pin<&mut C>,
    outbound: &mut OutboundGuard<'_>,
    handler_fut: futures_util::future::BoxFuture<'_, Result<WsOutcome, WsHandlerError>>,
    close_grace: Duration,
    outbound_capacity: usize,
    stall_timeout: Option<Duration>,
) -> Result<SessionFlow, SessionFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    let mut handler_fut = handler_fut;

    // ステップ 1: ハンドラ完了まで cancel（最優先）→ (ハンドラ完了 |
    // outbound 到着) を反復する。outbound が既に無効化済み（`None`）の場合は
    // 常に Pending なダミー Future を使い、以後選択されないようにする
    // （`run_session_inner` 外側ループの `Right(None)` 分岐と同じ「無効化して
    // ビジーループ化を防ぐ」方針を踏襲）。
    let outcome = loop {
        let progress = match outbound.rx.as_mut() {
            Some(rx) => race_cancel(cancel.as_mut(), race2(&mut handler_fut, rx.recv())).await,
            None => {
                race_cancel(
                    cancel.as_mut(),
                    race2(
                        &mut handler_fut,
                        std::future::pending::<Option<OutboundItem>>(),
                    ),
                )
                .await
            }
        };
        match progress {
            None => return Ok(SessionFlow::Cancelled),
            Some(Either::Left(handler_result)) => break handler_result,
            Some(Either::Right(Some(OutboundItem::Message(msg)))) => {
                let frame = to_tungstenite_message(msg);
                match send_bounded(
                    ws,
                    cancel.as_mut(),
                    &mut outbound.close,
                    frame,
                    stall_timeout,
                )
                .await
                {
                    SendOutcome::Cancelled => return Ok(SessionFlow::Cancelled),
                    SendOutcome::Sent => {}
                    SendOutcome::CloseGraceExpired => return Ok(SessionFlow::CloseGraceExpired),
                    SendOutcome::Stalled => return Ok(SessionFlow::Stalled),
                    SendOutcome::Failed(err) => return Err(SessionFailure::send(err)),
                }
            }
            Some(Either::Right(Some(OutboundItem::Close { code, reason }))) => {
                // `WsSender::close`（イシュー #710）がハンドラ実行中に呼ばれた。
                // ハンドラの `Future`（`handler_fut`）はここで `return` により
                // drop する（#499 の中断安全性契約の範囲内。close 後は送信
                // できないためハンドラ完了を待つ意味がなく、待てば Close
                // 送出が遅れて有界性を損なう）。
                return Ok(SessionFlow::SenderClose {
                    code,
                    reason,
                    deadline: outbound.close.deadline(),
                });
            }
            Some(Either::Right(None)) => {
                // 全 `WsSender` クローンが drop 済み（`run_session_inner` 外側
                // ループの同種分岐と同じ防御的コード。`conn_ctx` がクローンを
                // 保持し続けるためセッション実行中は到達不能）。
                outbound.release();
            }
        }
    };

    // ステップ 2: 継続経路と終了経路で排出方法を分ける（関数 doc の手順 2）。
    // 終了経路の期限は「排出 + Close 送出」全体で共有する単一期限として
    // 各経路で 1 回だけ計算する（イシュー #711 PR #735 レビュー指摘 P1 #1）。
    match outcome {
        Ok(WsOutcome::Reply(messages)) => {
            // 回数上限はチャネル生成（`crate::handle_upgrade` の
            // `handler::channel(config.outbound_capacity)`、イシュー #709）と
            // 同じ容量値（呼び出し元がこの関数の `outbound_capacity` 引数へ
            // `config.outbound_capacity` を渡す契約。本関数のモジュール doc・
            // `drain_before_reply` の doc も参照）。
            if let Some(flow) = drain_before_reply(
                ws,
                cancel.as_mut(),
                outbound,
                outbound_capacity,
                stall_timeout,
            )
            .await?
            {
                return Ok(flow);
            }
            let close_deadline = Instant::now() + close_grace;
            apply_outcome(
                ws,
                WsOutcome::Reply(messages),
                outbound,
                cancel,
                close_deadline,
                stall_timeout,
            )
            .await
        }
        Ok(WsOutcome::Close) => {
            let close_deadline = Instant::now() + close_grace;
            apply_outcome(
                ws,
                WsOutcome::Close,
                outbound,
                cancel,
                close_deadline,
                stall_timeout,
            )
            .await
        }
        Err(err) => {
            let close_deadline = Instant::now() + close_grace;
            match flush_outbound(ws, outbound, cancel, close_deadline).await {
                Ok(FlushOutcome::Cancelled) => Ok(SessionFlow::Cancelled),
                Ok(FlushOutcome::SenderClose {
                    code,
                    reason,
                    deadline,
                }) => {
                    // 送信キューの封鎖前に確定した `WsSender::close` を優先して
                    // 届ける（ハンドラエラーは破棄、設計 12 節）。
                    Ok(SessionFlow::SenderClose {
                        code,
                        reason,
                        deadline,
                    })
                }
                // 排出の完了・期限超過・送信失敗のいずれでも、終了理由は元の
                // ハンドラエラーのまま返す（排出中の送信エラーで上書きしない）。
                Ok(FlushOutcome::Done | FlushOutcome::TimedOut) | Err(_) => {
                    Err(SessionFailure::handler(err))
                }
            }
        }
    }
}

/// アイドルタイムアウト・Ping keepalive の Pong 期限切れ、いずれの発火時も
/// 使う正常な Close ハンドシェイク（close code 1000 Normal Closure）。
/// [`close_and_drain`] へ委譲する（呼び出し元は `run_session` 内の 2 箇所
/// —— `InboundEvent::Timer(TimerKind::Idle)` / `TimerKind::PongDeadline`
/// —— で、どちらも「受信待ちで何も読めない状態が続いた」ことを表す点が
/// 共通する。呼び出し元が返す `CloseReason` で区別する）。
async fn close_normally<S>(ws: WebSocketStream<S>, close_grace: Duration) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    close_and_drain(ws, None, Instant::now() + close_grace).await
}

/// キャンセル `Future`（`crate::handle_upgrade` 経由でコアの世代キャンセル
/// シグナルへ接続、イシュー #492）発火時の切断シーケンス。
///
/// `close_normally` と同型だが、close code は 1001 Going Away
/// （サーバ側都合による切断であることを示す）を使い、reason は固定文字列
/// のみで内部状態・エラー詳細・機密を含めない
/// （`docs/design/ws-cancellation-propagation.md` 8 節）。呼び出し元
/// `run_session` の唯一の呼び出し箇所。
async fn handle_cancellation<S>(
    ws: WebSocketStream<S>,
    close_grace: Duration,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let close_frame = CloseFrame {
        code: CloseCode::Away,
        reason: Utf8Bytes::from_static("going away"),
    };
    close_and_drain(ws, Some(close_frame), Instant::now() + close_grace).await
}

/// Close フレーム送出 → クライアント応答（または EOF・エラー）のドレインを
/// `deadline` で有界化する共通ヘルパー（[`close_normally`] /
/// [`handle_cancellation`] / `WsSender::close` 経路で共有）。呼び出し元は
/// 通常その時点から `close_grace`（`WebSocketConfig::close_grace`、既定 10 秒）
/// 後を渡し、終了経路の排出中に見つかった Close 指示では排出と共有する残りの
/// 期限を渡す（[`SessionFlow::SenderClose`] の `deadline` を参照）。
///
/// Close 送出自体が失敗した場合（相手が既に切断済み等）も、切断そのものの
/// 目的は達成されているため、ドレインへ進まず正常終了として扱う。Close
/// 応答を返さないクライアントに接続を無期限保持させないため、送出 →
/// ドレインの全体を `deadline` で区切る（二次 DoS 対策、Issue #175・
/// イシュー #492 で送出自体の停滞も有界化対象へ拡張、イシュー #500 で
/// 猶予値を `WebSocketConfig` から設定可能にした）。
///
/// `tokio_tungstenite::tungstenite::Error::SendAfterClosing` は
/// `ConnectionClosed` / `AlreadyClosed` と異なり、ドレインへ進めて
/// フラッシュを完遂させる（未送出のまま破棄しない）。
/// [`apply_outcome`] の `WsOutcome::Close` 送出中（`ws.close(None)`）に
/// キャンセルが発火すると [`SessionFlow::Cancelled`] 経由で本関数
/// （[`handle_cancellation`]）へ再度到達し、`ws.close` を 2 回目呼び出す
/// ケースがある。1 回目の呼び出しで Close フレームが既にキューイング済み
/// の場合、tungstenite は 2 回目を `SendAfterClosing` で拒否する。Close
/// フレーム自体は 1 回目の呼び出しで内部バッファへキューイング済みだが、
/// 実際にワイヤへ書き出されフラッシュされたとは限らないため、これを
/// `ConnectionClosed` / `AlreadyClosed`（接続自体が既に消滅済みで
/// ドレイン不要）と同一視して早期リターンすると、Close フレームが
/// 未フラッシュのまま破棄されうる。ドレインループへ進めて `ws.next()`
/// を呼ぶことで、内部的な書き込みフラッシュ・クライアント応答の消費を
/// 完遂させる（イシュー #499、PR #504 レビュー指摘）。
async fn close_and_drain<S>(
    mut ws: WebSocketStream<S>,
    close_frame: Option<CloseFrame>,
    deadline: Instant,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let sequence = async {
        if let Err(err) = ws.close(close_frame).await {
            match err {
                tokio_tungstenite::tungstenite::Error::ConnectionClosed
                | tokio_tungstenite::tungstenite::Error::AlreadyClosed => return Ok(()),
                tokio_tungstenite::tungstenite::Error::Protocol(
                    tokio_tungstenite::tungstenite::error::ProtocolError::SendAfterClosing,
                ) => {
                    // Close フレームは 1 回目の呼び出しで既にキューイング
                    // 済み。ドレインループへ進めてフラッシュ・応答消費を
                    // 完遂させる（早期 return しない）。
                }
                other => return Err(other),
            }
        }

        loop {
            match ws.next().await {
                // Close 応答（または相手からの追加フレーム）を消費し続け、
                // EOF（`None`）でドレイン完了とする。
                Some(Ok(_)) => continue,
                Some(Err(
                    tokio_tungstenite::tungstenite::Error::ConnectionClosed
                    | tokio_tungstenite::tungstenite::Error::AlreadyClosed,
                ))
                | None => return Ok(()),
                Some(Err(other)) => return Err(other),
            }
        }
    };

    match tokio::time::timeout_at(deadline, sequence).await {
        Ok(Ok(())) => Ok(()),
        Err(_timeout_elapsed) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
    }
}

/// `run_session` の outbound 合流経路（イシュー #670）の単体テスト。
///
/// `run_session` は `pub(crate)` であり、直接は呼べない。イシュー #671 で
/// `handle_upgrade` → `WsMessageHandler::on_open` 経由の公開経路
/// （`tests/handler_e2e.rs` 等の統合テストから到達可能）が追加されたが、
/// 本テスト群は `run_session` の合流ロジック自体（cancel → 受信/idle →
/// outbound の優先順位・idle_timeout 非リセット等）を `on_open`/ハンドラを
/// 介さず直接検証する目的で維持する（`.claude/rules/feature-modification.md`
/// の「実装変更には同一クレートのテスト追加を伴わせる」を `#[cfg(test)]`
/// で満たす）。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::{self, WsSendError};

    /// テスト用の `WebSocketConfig`（`EchoHandler`・アイドルタイムアウト無効・
    /// 短い `close_grace`）。フィールドは構造体更新構文で個別に上書きする。
    fn test_config() -> WebSocketConfig {
        WebSocketConfig {
            path: "/ws".to_string(),
            max_message_size: 1024 * 1024,
            max_frame_size: 256 * 1024,
            idle_timeout: None,
            close_grace: Duration::from_millis(300),
            handler: handler::default_handler(),
            pattern: None,
            outbound_capacity: handler::DEFAULT_OUTBOUND_CAPACITY,
            ping: None,
        }
    }

    /// テスト用の `WsConnContext`（イシュー #704）。`run_session` は
    /// `pub(crate)` の非公開シグネチャに `conn_ctx: &WsConnContext` を
    /// 要求するため、本モジュールのテストが直接呼ぶ際に使うヘルパー。
    /// `sender` は呼び出し元が渡した `WsSender`（本番の `handle_upgrade`
    /// と同様、outbound チャネルの送信側クローンを 1 個保持する）。
    fn test_conn_ctx(sender: handler::WsSender) -> WsConnContext {
        WsConnContext::new(handler::WsConnId::next(), sender, Vec::new())
    }

    /// 受け入れ基準 1: `WsSender` からの push とクライアント宛の返信
    /// （`EchoHandler`）が同一 `WebSocketStream` 上で混ざらず、両方とも
    /// 欠落なく届くこと。
    #[tokio::test]
    async fn outbound_messages_interleave_safely_with_client_replies() {
        let config: &'static WebSocketConfig = Box::leak(Box::new(test_config()));
        let (server_side, client_side) = tokio::io::duplex(8192);
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx.clone());

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

        for i in 0..3 {
            tx.send(WsMessage::Text(format!("push-{i}")))
                .await
                .expect("outbound send should succeed");
        }
        for i in 0..2 {
            client
                .send(Message::Text(format!("echo-{i}").into()))
                .await
                .expect("client send should succeed");
        }

        let mut received = Vec::new();
        for _ in 0..5 {
            let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect("should receive a frame within timeout")
                .expect("stream should not end early")
                .expect("frame should not error");
            received.push(msg);
        }

        for i in 0..3 {
            let expected = Message::Text(format!("push-{i}").into());
            assert!(
                received.contains(&expected),
                "missing outbound push-{i}: {received:?}"
            );
        }
        for i in 0..2 {
            let expected = Message::Text(format!("echo-{i}").into());
            assert!(
                received.contains(&expected),
                "missing echoed reply echo-{i}: {received:?}"
            );
        }
        assert_eq!(received.len(), 5, "no frame should be duplicated or merged");

        drop(tx);
        drop(client);
        let _ = tokio::time::timeout(Duration::from_secs(2), session_handle).await;
    }

    /// 受け入れ基準 2: チャネルが満杯（`capacity` 到達）のとき、
    /// `WsSender::send` は受信側が消費するまで `.await` で待機すること。
    #[tokio::test]
    async fn ws_sender_send_blocks_until_capacity_frees() {
        let (tx, mut rx) = handler::channel(1);
        tx.send(WsMessage::Text("first".to_string()))
            .await
            .expect("first send fills capacity 1 without blocking");

        let tx2 = tx.clone();
        let mut second =
            tokio::spawn(async move { tx2.send(WsMessage::Text("second".to_string())).await });

        // 満杯のまま受信側が消費しない限り、2 件目の送信は完了しないはず。
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !second.is_finished(),
            "second send should block while the channel is full"
        );

        let first = rx.recv().await.expect("first message should be queued");
        assert_eq!(
            first,
            OutboundItem::Message(WsMessage::Text("first".to_string()))
        );

        let result = tokio::time::timeout(Duration::from_millis(200), &mut second)
            .await
            .expect("second send should complete once capacity frees up")
            .expect("task should not panic");
        assert!(result.is_ok(), "second send should succeed: {result:?}");
    }

    /// 受け入れ基準 3: セッションの世代キャンセル発火時、満杯チャネルで
    /// ブロック中の `WsSender::send` 呼び出しが `close_grace` の満了を
    /// 待たず即座に [`WsSendError`] で解放されること（`run_session` が
    /// `handle_cancellation` を呼ぶ前に `outbound` を drop するため）。
    #[tokio::test]
    async fn blocked_send_is_released_immediately_on_cancellation() {
        let mut config = test_config();
        // ドレイン待ちを長めに取り、「即座に解放される」ことと
        // 「close_grace 満了まで待たされる」ことを明確に区別できるようにする。
        config.close_grace = Duration::from_secs(2);
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(4096);
        // クライアントは Close 応答を返さない（passive）。接続自体は
        // セッション終了まで保持し、`ws.close()` が接続断エラーで即終了
        // しないようにする。
        let _client_side = client_side;

        let (tx, rx) = handler::channel(1);
        tx.send(WsMessage::Text("first".to_string()))
            .await
            .expect("first send fills capacity 1 without blocking");

        let tx2 = tx.clone();
        let blocked =
            tokio::spawn(async move { tx2.send(WsMessage::Text("second".to_string())).await });

        let conn_ctx = test_conn_ctx(tx.clone());

        // 既に発火済みのキャンセルを渡す（`race_cancel` は cancel を最優先で
        // ポーリングするため、outbound にキュー済みメッセージがあっても
        // 送出よりキャンセル分岐が優先される）。
        let session_handle = tokio::spawn(async move {
            let cancel = std::future::ready(());
            let mut cancel = std::pin::pin!(cancel);
            run_session(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let blocked_result = tokio::time::timeout(Duration::from_millis(500), blocked)
            .await
            .expect(
                "blocked WsSender::send should be released well before close_grace (2s) elapses",
            )
            .expect("task should not panic");
        assert_eq!(
            blocked_result,
            Err(WsSendError),
            "send should fail once the session drops the receiver on cancellation"
        );

        // `run_session` 自体は close_grace を上限にドレインを試みるが、
        // 有界時間内に終了することを確認する。
        let session_result = tokio::time::timeout(Duration::from_secs(5), session_handle)
            .await
            .expect("run_session should finish within close_grace bound")
            .expect("session task should not panic");
        assert!(
            session_result.is_ok(),
            "session should end normally: {session_result:?}"
        );
    }

    /// アイドルタイムアウト回帰防止: outbound push が継続していても、
    /// クライアントからの受信が一切ない限り `idle_timeout` は
    /// push によってリセットされず、設定どおりに発火すること
    /// （Issue #175 の DoS 対策を後退させない）。
    #[tokio::test]
    async fn outbound_push_does_not_reset_idle_timeout() {
        let config = WebSocketConfig {
            idle_timeout: Some(Duration::from_millis(120)),
            ..test_config()
        };
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(8192);
        // クライアントは接続を保持するだけでフレームを一切送らない。
        let _client_side = client_side;

        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx.clone());

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let pusher = tokio::spawn(async move {
            for _ in 0..8 {
                tokio::time::sleep(Duration::from_millis(40)).await;
                if tx.send(WsMessage::Text("push".to_string())).await.is_err() {
                    // セッション終了後（idle timeout でチャネルが drop 済み）は
                    // 送信エラーになる。想定内のため打ち切る。
                    break;
                }
            }
        });

        // idle_timeout（120ms）が push でリセットされていれば、320ms（8 回 ×
        // 40ms）push し続ける間セッションは終了しないはず。正しい実装では
        // クライアント無通信のまま idle 判定され、close_grace（300ms）以内に
        // 終了するため、600ms 以内に完了する。
        let result = tokio::time::timeout(Duration::from_millis(600), session_handle)
            .await
            .expect("idle timeout should fire even while outbound pushes continue")
            .expect("session task should not panic");
        assert!(
            result.is_ok(),
            "session should end normally via idle timeout: {result:?}"
        );

        let _ = pusher.await;
    }

    /// [`race2_alternating`] 単体テスト（イシュー #684、PR #684 レビュー
    /// 指摘対応）: `a`・`b` の両方が即座に Ready な場合、`prefer_b` の値が
    /// そのまま選ばれる側を決めること。`run_session` は反復ごとに
    /// `prefer_b` を反転させて呼ぶため、この性質により固定優先度による
    /// outbound 飢餓（連続受信クライアント下で `rx.recv()` が恒久的に
    /// 選ばれなくなる退行）を避けられる。
    #[tokio::test]
    async fn race2_alternating_respects_prefer_b_when_both_ready() {
        // prefer_b = false（inbound 優先）: 両方 Ready なら a（Left）が選ばれる。
        let result =
            race2_alternating(false, std::future::ready('a'), std::future::ready('b')).await;
        assert!(
            matches!(result, Either::Left('a')),
            "prefer_b=false のとき、両方 Ready なら a が優先されるべき"
        );

        // prefer_b = true（outbound 優先）: 両方 Ready なら b（Right）が選ばれる。
        let result =
            race2_alternating(true, std::future::ready('a'), std::future::ready('b')).await;
        assert!(
            matches!(result, Either::Right('b')),
            "prefer_b=true のとき、両方 Ready なら b が優先されるべき"
        );
    }

    /// 受け入れ基準（PR #684 レビュー指摘）: `run_session` が反復ごとに
    /// `prefer_outbound` を反転させることで、クライアントが連続送信を
    /// 続けている間でも outbound push が有界回数内に届くこと。
    ///
    /// 固定優先度（旧実装の `race2(inbound, rx.recv())`）では、client の
    /// 連続送信によって `ws.next()` が常にポーリング時点で Ready になり
    /// うる場合、`rx.recv()` が恒久的に選ばれない飢餓が構造的に起こり
    /// うる契約だった。交互化後は最悪でも 2 反復に 1 回は outbound 側が
    /// 優先されるため、outbound push は高々「クライアント送信数 + 定数」
    /// 反復以内に届く（無期限の滞留がないことを実測で確認する）。
    #[tokio::test]
    async fn outbound_push_is_not_starved_by_continuous_client_sends() {
        let config: &'static WebSocketConfig = Box::leak(Box::new(test_config()));
        let (server_side, client_side) = tokio::io::duplex(1 << 20);
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx.clone());

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

        // outbound push を先にキューイングしておく。
        tx.send(WsMessage::Text("urgent-push".to_string()))
            .await
            .expect("outbound send should succeed");

        // クライアントは大量のメッセージを連続送信する（応答を読まずに
        // 送りっぱなしにすることで `ws.next()` を継続的に Ready に近づけ、
        // 固定優先度なら飢餓が起きやすい状況を模す）。
        const CLIENT_MESSAGES: usize = 40;
        for i in 0..CLIENT_MESSAGES {
            client
                .send(Message::Text(format!("client-{i}").into()))
                .await
                .expect("client send should succeed");
        }

        // 受信した最初の CLIENT_MESSAGES + 定数 件のうちに outbound push
        // （"urgent-push"）が含まれることを確認する（全件の末尾まで届かない
        // ことをもって「飢餓していない」とみなす）。
        let mut found_push = false;
        let scan_limit = CLIENT_MESSAGES / 2;
        for _ in 0..scan_limit {
            let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect("should receive a frame within timeout")
                .expect("stream should not end early")
                .expect("frame should not error");
            if msg == Message::Text("urgent-push".into()) {
                found_push = true;
                break;
            }
        }
        assert!(
            found_push,
            "outbound push should arrive within the first {scan_limit} frames, \
             not be starved until after all {CLIENT_MESSAGES} client echoes"
        );

        drop(tx);
        drop(client);
        let _ = tokio::time::timeout(Duration::from_secs(2), session_handle).await;
    }

    /// 受け入れ基準 1・2（イシュー #704）: `run_session` が
    /// `on_message_with_ctx` へ渡す `&WsConnContext` が、呼び出し元
    /// （本テストの `test_conn_ctx`）が構築したものと同一の `conn_id` を
    /// 持ち、受信メッセージごとに一貫していること。
    #[tokio::test]
    async fn on_message_with_ctx_receives_the_conn_ctx_passed_to_run_session() {
        use futures_util::future::BoxFuture;
        use std::sync::Mutex;

        /// 受け取った `conn_id` を蓄積するだけの検証用ハンドラ。
        struct RecordingHandler {
            observed: std::sync::Arc<Mutex<Vec<handler::WsConnId>>>,
        }

        impl handler::WsMessageHandler for RecordingHandler {
            fn name(&self) -> &'static str {
                "recording"
            }

            fn on_message(
                &self,
                msg: WsMessage,
            ) -> BoxFuture<'_, Result<WsOutcome, handler::WsHandlerError>> {
                // on_message_with_ctx をオーバーライドしているため実行時には
                // 呼ばれない（トレードオフ、`handler` モジュール doc 参照）。
                Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
            }

            fn on_message_with_ctx<'a>(
                &'a self,
                ctx: &'a WsConnContext,
                msg: WsMessage,
            ) -> BoxFuture<'a, Result<WsOutcome, handler::WsHandlerError>> {
                self.observed.lock().unwrap().push(ctx.conn_id());
                Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
            }
        }

        let observed = std::sync::Arc::new(Mutex::new(Vec::new()));
        let mut config = test_config();
        config.handler = std::sync::Arc::new(RecordingHandler {
            observed: observed.clone(),
        });
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(4096);
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx);
        let expected_conn_id = conn_ctx.conn_id();

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        for i in 0..3 {
            client
                .send(Message::Text(format!("msg-{i}").into()))
                .await
                .expect("client send should succeed");
            let reply = tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect("should receive a reply within timeout")
                .expect("stream should not end early")
                .expect("frame should not error");
            assert_eq!(reply, Message::Text(format!("msg-{i}").into()));
        }

        drop(client);
        let _ = tokio::time::timeout(Duration::from_secs(2), session_handle).await;

        let observed = observed.lock().unwrap();
        assert_eq!(
            observed.len(),
            3,
            "on_message_with_ctx should run once per message"
        );
        assert!(
            observed.iter().all(|id| *id == expected_conn_id),
            "every call should observe the same conn_id passed to run_session: {observed:?}"
        );
    }

    /// イシュー #706（設計 6 節）の受け入れ基準: `on_message_with_ctx` の
    /// 実行中に `ctx.sender().send(...).await` で自身の outbound チャネル
    /// （容量 [`handler::DEFAULT_OUTBOUND_CAPACITY`]、既定 8）へ容量を
    /// 超える件数を送信しても、`run_session` がその都度消化するため
    /// デッドロックしないこと。かつ、それらの push はハンドラが返す
    /// `WsOutcome::Reply` より先にクライアントへ届くこと（設計 6 節の
    /// 保証: 排出開始時点で格納済みの push は Reply より先に送出される）。
    #[tokio::test]
    async fn on_message_with_ctx_self_send_beyond_capacity_does_not_deadlock() {
        use futures_util::future::BoxFuture;

        /// `ctx.sender()` へ容量超の件数を送信してから固定の返信を返す
        /// ハンドラ（PR #725 レビュー指摘対応の回帰テスト）。
        struct SelfSendingHandler {
            push_count: usize,
        }

        impl handler::WsMessageHandler for SelfSendingHandler {
            fn name(&self) -> &'static str {
                "self-sending"
            }

            fn on_message(
                &self,
                msg: WsMessage,
            ) -> BoxFuture<'_, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
            }

            fn on_message_with_ctx<'a>(
                &'a self,
                ctx: &'a WsConnContext,
                _msg: WsMessage,
            ) -> BoxFuture<'a, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move {
                    for i in 0..self.push_count {
                        ctx.sender()
                            .send(WsMessage::Text(format!("push-{i}")))
                            .await
                            .expect("self-send should not fail before session ends");
                    }
                    Ok(WsOutcome::Reply(vec![WsMessage::Text("done".to_string())]))
                })
            }
        }

        // outbound チャネルの容量（4）より多い件数（10）を自己送信させ、
        // 旧実装なら容量到達時点でデッドロックする状況を再現する。
        const OUTBOUND_CAPACITY: usize = 4;
        const PUSH_COUNT: usize = 10;

        let mut config = test_config();
        config.handler = std::sync::Arc::new(SelfSendingHandler {
            push_count: PUSH_COUNT,
        });
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(1 << 16);
        let (tx, rx) = handler::channel(OUTBOUND_CAPACITY);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        client
            .send(Message::Text("trigger".into()))
            .await
            .expect("client send should succeed");

        let mut received = Vec::new();
        for _ in 0..(PUSH_COUNT + 1) {
            let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect(
                    "self-send beyond channel capacity should not deadlock; \
                     each push and the final reply must arrive within timeout",
                )
                .expect("stream should not end early")
                .expect("frame should not error");
            received.push(msg);
        }

        for i in 0..PUSH_COUNT {
            assert_eq!(
                received[i],
                Message::Text(format!("push-{i}").into()),
                "push-{i} should arrive in order before the reply: {received:?}"
            );
        }
        assert_eq!(
            received[PUSH_COUNT],
            Message::Text("done".into()),
            "final reply should arrive after all self-sent pushes: {received:?}"
        );

        drop(client);
        let _ = tokio::time::timeout(Duration::from_secs(2), session_handle).await;
    }

    /// イシュー #709 の回帰テスト: `run_handler_with_outbound_drain` の排出
    /// 回数上限が `WebSocketConfig::outbound_capacity`（設定値）に追随し、
    /// `crate::handler::DEFAULT_OUTBOUND_CAPACITY`（8）に固定されたままに
    /// なっていないことを確認する。
    ///
    /// `outbound_capacity` を既定（8）より大きい 16 に設定し、ハンドラの
    /// `on_message_with_ctx` が `.await` を挟まず（同期的な `try_send` のみ
    /// で）16 件を outbound キューへ積んでから `WsOutcome::Reply` を返す
    /// ようにする。ハンドラ Future が最初のポーリングで完了するため、
    /// `run_handler_with_outbound_drain` は排出（`drain_before_reply`）を
    /// 開始した時点でキューに 16 件すべてが格納済みの状態になる。排出回数
    /// 上限が古い固定値 8 のままだと、残り 8 件より先に Reply が送出されて
    /// しまい本テストは失敗する。
    #[tokio::test]
    async fn outbound_drain_uses_configured_capacity_not_fixed_default() {
        use futures_util::future::BoxFuture;

        const CONFIGURED_CAPACITY: usize = 16;

        /// `.await` を挟まず `try_send` のみで push してから Reply を返す
        /// ハンドラ。
        struct SyncSelfSendingHandler;

        impl handler::WsMessageHandler for SyncSelfSendingHandler {
            fn name(&self) -> &'static str {
                "sync-self-sending"
            }

            fn on_message(
                &self,
                msg: WsMessage,
            ) -> BoxFuture<'_, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
            }

            fn on_message_with_ctx<'a>(
                &'a self,
                ctx: &'a WsConnContext,
                _msg: WsMessage,
            ) -> BoxFuture<'a, Result<WsOutcome, handler::WsHandlerError>> {
                for i in 0..CONFIGURED_CAPACITY {
                    ctx.sender()
                        .try_send(WsMessage::Text(format!("push-{i}")))
                        .expect("channel capacity matches CONFIGURED_CAPACITY");
                }
                Box::pin(
                    async move { Ok(WsOutcome::Reply(vec![WsMessage::Text("done".to_string())])) },
                )
            }
        }

        let mut config = test_config();
        config.handler = std::sync::Arc::new(SyncSelfSendingHandler);
        config.outbound_capacity = CONFIGURED_CAPACITY;
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(1 << 16);
        // 実チャネル容量も設定値と一致させる（本番の `crate::lib::handle_upgrade`
        // が `handler::channel(config.outbound_capacity)` を呼ぶ契約と対応）。
        let (tx, rx) = handler::channel(CONFIGURED_CAPACITY);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        client
            .send(Message::Text("trigger".into()))
            .await
            .expect("client send should succeed");

        let mut received = Vec::new();
        for _ in 0..(CONFIGURED_CAPACITY + 1) {
            let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect("drain should not stall")
                .expect("stream should not end early")
                .expect("frame should not error");
            received.push(msg);
        }

        for i in 0..CONFIGURED_CAPACITY {
            assert_eq!(
                received[i],
                Message::Text(format!("push-{i}").into()),
                "push-{i} should arrive before the reply when capacity is configured to {CONFIGURED_CAPACITY}: {received:?}"
            );
        }
        assert_eq!(
            received[CONFIGURED_CAPACITY],
            Message::Text("done".into()),
            "final reply should arrive only after all {CONFIGURED_CAPACITY} configured-capacity \
             pushes: {received:?}"
        );

        drop(client);
        let _ = tokio::time::timeout(Duration::from_secs(2), session_handle).await;
    }

    /// 受け入れ基準 3（設計 6 節・#706 引き渡し事項の一部先取り確認）:
    /// ハンドラが `Err` を返した場合、`run_session` は排出・送信を行わず
    /// 即時に `Err(WsError::Handler(_))` で終了すること（既存 `outcome?`
    /// の契約、`docs/design/ws-connection-context-and-close.md` 4 節の
    /// 対応表 「outcome? の Err」行）。
    #[tokio::test]
    async fn handler_error_short_circuits_without_sending() {
        use futures_util::future::BoxFuture;

        struct FailingHandler;

        impl handler::WsMessageHandler for FailingHandler {
            fn name(&self) -> &'static str {
                "failing"
            }

            fn on_message(
                &self,
                _msg: WsMessage,
            ) -> BoxFuture<'_, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move { Err(handler::WsHandlerError::new("boom")) })
            }
        }

        let mut config = test_config();
        config.handler = std::sync::Arc::new(FailingHandler);
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(4096);
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        client
            .send(Message::Text("trigger".into()))
            .await
            .expect("client send should succeed");

        let result = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(result, Err(WsError::Handler(_))),
            "handler error should short-circuit run_session: {result:?}"
        );
    }

    // --- イシュー #726: `run_session_inner` の `CloseReason` 検証 ---
    //
    // 以下は `run_session_inner`（`pub(crate)` の `run_session` がタプルの
    // `.1` のみを返す薄いラッパーの内側）を直接呼び、戻り値タプルの両側
    // （`CloseReason` と `Result<(), WsError>`）を検証する。設計 4 節の
    // 脱出点対応表・3 節「対象ファイル・変更箇所」の割り当て表に対応する。

    /// 脱出点対応表: クライアントの Close フレーム受信 →
    /// `(CloseReason::ClientClose, Ok(()))`。
    #[tokio::test]
    async fn client_close_yields_client_close_reason() {
        let config: &'static WebSocketConfig = Box::leak(Box::new(test_config()));
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session_inner(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        client
            .close(None)
            .await
            .expect("client close should succeed");

        let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(reason, CloseReason::ClientClose),
            "expected ClientClose, got {reason:?}"
        );
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
    }

    /// 脱出点対応表: Close ハンドシェイクなしの TCP 切断（クライアント側
    /// duplex を Close 送出なしで drop）は tungstenite 0.30 では
    /// `Protocol(ResetWithoutClosingHandshake)` として観測される。これは
    /// `CloseReason::Eof` の doc が定義する事象そのものであるため
    /// `SessionFailure::recv` が `CloseReason::Eof` へ分類する（`Result`
    /// 側は読み取り失敗を示す `Err(WsError::Protocol(_))` のまま。イシュー
    /// #726 レビュー指摘対応。詳細は `handler::CloseReason::Eof` の doc・
    /// 設計文書 4 節・9 節を参照）。
    #[tokio::test]
    async fn disconnect_without_close_handshake_yields_eof() {
        let config: &'static WebSocketConfig = Box::leak(Box::new(test_config()));
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session_inner(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        // Close フレームを送らずに切断する。
        drop(client_side);

        let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(reason, CloseReason::Eof),
            "expected Eof, got {reason:?}"
        );
        assert!(
            matches!(result, Err(WsError::Protocol(_))),
            "expected Err(WsError::Protocol(_)), got {result:?}"
        );
    }

    /// 脱出点対応表: アイドルタイムアウト発火 →
    /// `(CloseReason::IdleTimeout, Ok(()))`。
    #[tokio::test]
    async fn idle_timeout_yields_idle_timeout_reason() {
        let config = WebSocketConfig {
            idle_timeout: Some(Duration::from_millis(80)),
            ..test_config()
        };
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(4096);
        let _client_side = client_side;
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session_inner(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(reason, CloseReason::IdleTimeout),
            "expected IdleTimeout, got {reason:?}"
        );
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
    }

    /// 脱出点対応表: コアの世代キャンセル発火 →
    /// `(CloseReason::Cancelled, Ok(()))`。
    #[tokio::test]
    async fn cancellation_yields_cancelled_reason() {
        let config: &'static WebSocketConfig = Box::leak(Box::new(test_config()));
        let (server_side, client_side) = tokio::io::duplex(4096);
        // クライアントは Close 応答を返さない（passive）。
        let _client_side = client_side;
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            // 既に発火済みのキャンセルを渡す。
            let cancel = std::future::ready(());
            let mut cancel = std::pin::pin!(cancel);
            run_session_inner(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(reason, CloseReason::Cancelled),
            "expected Cancelled, got {reason:?}"
        );
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
    }

    /// 脱出点対応表: 受信メッセージが `max_message_size` を超過 →
    /// `(CloseReason::MessageTooLarge, Err(WsError::Protocol(Capacity(_))))`。
    #[tokio::test]
    async fn oversized_message_yields_message_too_large_reason() {
        let config = WebSocketConfig {
            max_message_size: 64,
            max_frame_size: 64,
            ..test_config()
        };
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(1 << 16);
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session_inner(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        // `max_message_size`（64 bytes）を超えるテキストを送信する。
        let oversized = "x".repeat(256);
        client
            .send(Message::Text(oversized.into()))
            .await
            .expect("client send should succeed at the transport layer");

        let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(reason, CloseReason::MessageTooLarge),
            "expected MessageTooLarge, got {reason:?}"
        );
        assert!(
            matches!(
                result,
                Err(WsError::Protocol(
                    tokio_tungstenite::tungstenite::Error::Capacity(_)
                ))
            ),
            "expected Err(WsError::Protocol(Capacity(_))), got {result:?}"
        );
    }

    /// 脱出点対応表: ハンドラが `WsOutcome::Close` →
    /// `(CloseReason::HandlerClose, Ok(()))`。
    #[tokio::test]
    async fn handler_close_yields_handler_close_reason() {
        use futures_util::future::BoxFuture;

        struct ClosingHandler;

        impl handler::WsMessageHandler for ClosingHandler {
            fn name(&self) -> &'static str {
                "closing"
            }

            fn on_message(
                &self,
                _msg: WsMessage,
            ) -> BoxFuture<'_, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move { Ok(WsOutcome::Close) })
            }
        }

        let mut config = test_config();
        config.handler = std::sync::Arc::new(ClosingHandler);
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(4096);
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session_inner(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        client
            .send(Message::Text("trigger".into()))
            .await
            .expect("client send should succeed");

        let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(reason, CloseReason::HandlerClose),
            "expected HandlerClose, got {reason:?}"
        );
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
    }

    /// イシュー #710（設計 12 節）: ハンドラ（`on_message_with_ctx`）が
    /// `Future` 内で止まっている間に、外部タスクが `WsSender::close` を
    /// 呼んだ場合。`run_handler_with_outbound_drain` のステップ 1
    /// （ハンドラ完了前に outbound 到着を検出する経路）が Close を検出し、
    /// ハンドラの `Future` を drop して即座に Close ハンドシェイクへ分岐する
    /// こと（返信は送出されない）を確認する。
    #[tokio::test]
    async fn sender_close_during_pending_handler_drops_handler_future() {
        use futures_util::future::BoxFuture;
        use std::sync::Arc;
        use tokio::sync::Notify;

        /// `on_message_with_ctx` が `Notify` で通知するまで無期限に
        /// ブロックするハンドラ。テストが `close` を確実に「ハンドラ実行中」
        /// に呼べるよう、ブロック開始を `started` で外部へ知らせる。
        struct BlockingHandler {
            started: Arc<Notify>,
        }

        impl handler::WsMessageHandler for BlockingHandler {
            fn name(&self) -> &'static str {
                "blocking"
            }

            fn on_message(
                &self,
                msg: WsMessage,
            ) -> BoxFuture<'_, Result<WsOutcome, handler::WsHandlerError>> {
                // `on_message_with_ctx` をオーバーライドしているため実行時
                // には呼ばれない（トレードオフ、`handler` モジュール doc 参照）。
                Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
            }

            fn on_message_with_ctx<'a>(
                &'a self,
                _ctx: &'a WsConnContext,
                _msg: WsMessage,
            ) -> BoxFuture<'a, Result<WsOutcome, handler::WsHandlerError>> {
                let started = Arc::clone(&self.started);
                Box::pin(async move {
                    started.notify_one();
                    // 世代キャンセル・`WsSender::close` のいずれかで打ち切ら
                    // れるまで無期限に `Pending` を返す。
                    std::future::pending().await
                })
            }
        }

        let started = Arc::new(Notify::new());
        let mut config = test_config();
        config.handler = std::sync::Arc::new(BlockingHandler {
            started: Arc::clone(&started),
        });
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(4096);
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx.clone());

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session_inner(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        client
            .send(Message::Text("trigger".into()))
            .await
            .expect("client send should succeed");

        // ハンドラの `Future` が確実に `on_message_with_ctx` 内でブロック
        // したことを待ってから close を呼ぶ（実行順序を保証し flaky を防ぐ。
        // `Notify` は `notify_one` が先行しても後続の `notified().await` が
        // 即座に完了する契約のため、多少の順序の揺れは問題にならない）。
        started.notified().await;
        tx.close(1001, "going").await.expect("close should succeed");

        let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(reason, CloseReason::SenderClose),
            "expected SenderClose, got {reason:?}"
        );
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");

        // ハンドラの `Future` は drop され、返信は送出されない。届くのは
        // 指定した code/reason の Close フレームのみ。
        let close_frame = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("close frame should arrive within timeout")
            .expect("stream should not end before close frame")
            .expect("frame should not error");
        match close_frame {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 1001);
                assert_eq!(frame.reason.as_str(), "going");
            }
            other => panic!("expected a close frame with code/reason, got {other:?}"),
        }
    }

    /// イシュー #710（設計 12 節）: ハンドラが `on_message_with_ctx` 実行中に
    /// push を数件送ってから `WsSender::close` を呼び、その後
    /// `WsOutcome::Reply` を返す場合。押送・close の enqueue がバック
    /// プレッシャなしで完了する（十分な容量）ため、ハンドラの `Future` は
    /// 単独ポーリングで完了し、継続経路の排出（[`drain_before_reply`]。
    /// 排出開始時点で既に格納済みの項目を `try_recv()` で拾う経路）が Close を
    /// 検出してハンドラの戻り値（`Reply`）を破棄することを確認する。
    #[tokio::test]
    async fn sender_close_after_handler_completes_discards_pending_reply() {
        use futures_util::future::BoxFuture;

        struct PushCloseReplyHandler;

        impl handler::WsMessageHandler for PushCloseReplyHandler {
            fn name(&self) -> &'static str {
                "push-close-reply"
            }

            fn on_message(
                &self,
                msg: WsMessage,
            ) -> BoxFuture<'_, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
            }

            fn on_message_with_ctx<'a>(
                &'a self,
                ctx: &'a WsConnContext,
                _msg: WsMessage,
            ) -> BoxFuture<'a, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move {
                    for i in 0..2 {
                        ctx.sender()
                            .send(WsMessage::Text(format!("push-{i}")))
                            .await
                            .expect("push should succeed before close");
                    }
                    ctx.sender()
                        .close(4000, "bye")
                        .await
                        .expect("close should succeed");
                    Ok(WsOutcome::Reply(vec![WsMessage::Text(
                        "should-not-arrive".to_string(),
                    )]))
                })
            }
        }

        let mut config = test_config();
        config.handler = std::sync::Arc::new(PushCloseReplyHandler);
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(8192);
        // 容量（8）は push 2 件 + close 1 件を余裕を持って収められる大きさに
        // する。ハンドラ内の enqueue がバックプレッシャで止まらず単独ポーリ
        // ングで完了することを保証し、検証対象を `drain_before_reply` の経路に
        // 固定する（バックプレッシャが起きるとステップ 1 の経路（前テスト）に
        // 落ちる）。
        let (tx, rx) = handler::channel(8);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session_inner(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        client
            .send(Message::Text("trigger".into()))
            .await
            .expect("client send should succeed");

        for i in 0..2 {
            let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect("push should arrive within timeout")
                .expect("stream should not end before all pushes arrive")
                .expect("frame should not error");
            assert_eq!(
                msg,
                Message::Text(format!("push-{i}").into()),
                "push-{i} should arrive in order before the close frame"
            );
        }

        let close_frame = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("close frame should arrive within timeout")
            .expect("stream should not end before close frame")
            .expect("frame should not error");
        match close_frame {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 4000);
                assert_eq!(frame.reason.as_str(), "bye");
            }
            other => panic!("expected a close frame with code/reason, got {other:?}"),
        }

        let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(reason, CloseReason::SenderClose),
            "expected SenderClose, got {reason:?}"
        );
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
    }

    /// PR #736 レビュー指摘対応（codex P1）の回帰テスト: ハンドラが
    /// `ctx.sender().close(...)` を確定させた直後に `Err` を返しても、
    /// 既に enqueue 済みの push・Close 指示が無言破棄されず先行処理される
    /// こと（`run_handler_with_outbound_drain` の排出ステップが `outcome`
    /// の `Ok`/`Err` に関わらず実行されることの実接続検証）。
    ///
    /// 修正前の実装に戻すと、本テストは `CloseReason::Failed(FailureKind::
    /// Handler)` を観測して FAIL する（push・Close フレームがワイヤへ
    /// 送出されない）。
    #[tokio::test]
    async fn sender_close_then_handler_error_still_delivers_pending_push_and_close() {
        use futures_util::future::BoxFuture;

        struct PushCloseThenErrorHandler;

        impl handler::WsMessageHandler for PushCloseThenErrorHandler {
            fn name(&self) -> &'static str {
                "push-close-then-error"
            }

            fn on_message(
                &self,
                _msg: WsMessage,
            ) -> BoxFuture<'_, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move { Err(handler::WsHandlerError::new("boom")) })
            }

            fn on_message_with_ctx<'a>(
                &'a self,
                ctx: &'a WsConnContext,
                _msg: WsMessage,
            ) -> BoxFuture<'a, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move {
                    ctx.sender()
                        .send(WsMessage::Text("push-0".to_string()))
                        .await
                        .expect("push should succeed before close");
                    ctx.sender()
                        .close(4000, "bye")
                        .await
                        .expect("close should succeed");
                    Err(handler::WsHandlerError::new("boom-after-close"))
                })
            }
        }

        let mut config = test_config();
        config.handler = std::sync::Arc::new(PushCloseThenErrorHandler);
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(8192);
        // 容量（8）は push 1 件 + close 1 件を余裕を持って収められる大きさに
        // する（`sender_close_after_handler_completes_discards_pending_reply`
        // と同じ意図。バックプレッシャなしで単独ポーリングで完了させ、
        // 検証対象を `Err` 経路の排出（`flush_outbound`）に固定する）。
        let (tx, rx) = handler::channel(8);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session_inner(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        client
            .send(Message::Text("trigger".into()))
            .await
            .expect("client send should succeed");

        let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("push should arrive within timeout")
            .expect("stream should not end before push arrives")
            .expect("frame should not error");
        assert_eq!(
            msg,
            Message::Text("push-0".into()),
            "push enqueued before close should still be delivered despite handler Err"
        );

        let close_frame = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("close frame should arrive within timeout")
            .expect("stream should not end before close frame")
            .expect("frame should not error");
        match close_frame {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 4000);
                assert_eq!(frame.reason.as_str(), "bye");
            }
            other => panic!("expected a close frame with code/reason, got {other:?}"),
        }

        let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(reason, CloseReason::SenderClose),
            "expected SenderClose (queued Close instruction takes priority over handler Err), got {reason:?}"
        );
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
    }

    /// Cursor Bugbot 指摘対応の回帰テスト（PR #736 #discussion_r4113894722）:
    /// 排出開始時点でキューへ [`handler::DEFAULT_OUTBOUND_CAPACITY`]（既定 8）
    /// を超える件数の push・Close 指示が既に積まれていた場合でも、Close
    /// 指示が排出漏れせず検出されること。
    ///
    /// 旧実装は `try_recv()` を `DEFAULT_OUTBOUND_CAPACITY` 回に固定して
    /// いたため、9 件目以降（本テストでは 12 件目の Close）が排出されずに
    /// 取り残され、ハンドラの `Err` が排出漏れを覆い隠して
    /// `CloseReason::Failed(FailureKind::Handler)` になっていた（`close()`
    /// 自身は `Ok` を返して確定済みにもかかわらず Close フレームが
    /// ワイヤへ送出されない静かなデータ欠落）。本テストは実際の
    /// `reserve()` 待ちの再現ではなく、チャネル容量を大きく確保して
    /// enqueue をバックプレッシャなしで完了させることで「排出開始時点で
    /// 8 件を超える件数が既に格納済み」という状況を決定的に再現する。
    /// 現行実装では `Err` は終了経路として送信キューを封鎖してから
    /// `try_recv()` が空になるまで排出するため、取り出す件数は固定値ではなくチャネル
    /// 自身の容量で決まる（[`flush_outbound`] の doc を参照）。
    #[tokio::test]
    async fn drain_detects_close_beyond_default_outbound_capacity() {
        use futures_util::future::BoxFuture;

        const PUSH_COUNT: usize = handler::DEFAULT_OUTBOUND_CAPACITY + 3;

        struct ManyPushesThenErrorHandler;

        impl handler::WsMessageHandler for ManyPushesThenErrorHandler {
            fn name(&self) -> &'static str {
                "many-pushes-then-error"
            }

            fn on_message(
                &self,
                _msg: WsMessage,
            ) -> BoxFuture<'_, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move { Err(handler::WsHandlerError::new("boom")) })
            }

            fn on_message_with_ctx<'a>(
                &'a self,
                ctx: &'a WsConnContext,
                _msg: WsMessage,
            ) -> BoxFuture<'a, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move {
                    for i in 0..PUSH_COUNT {
                        ctx.sender()
                            .send(WsMessage::Text(format!("push-{i}")))
                            .await
                            .expect("push should succeed before close");
                    }
                    ctx.sender()
                        .close(4000, "bye")
                        .await
                        .expect("close should succeed");
                    Err(handler::WsHandlerError::new("boom-after-close"))
                })
            }
        }

        let mut config = test_config();
        config.handler = std::sync::Arc::new(ManyPushesThenErrorHandler);
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(1 << 16);
        // 容量は push（`PUSH_COUNT` 件）+ close（1 件）を余裕を持って収める
        // 大きさにし、ハンドラ内の enqueue がバックプレッシャで止まらず
        // 単独ポーリングで完了することを保証する（検証対象を「排出開始時点で
        // 8 件を超える件数が既に格納済み」の状況に固定する）。
        let (tx, rx) = handler::channel(PUSH_COUNT + 8);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session_inner(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        client
            .send(Message::Text("trigger".into()))
            .await
            .expect("client send should succeed");

        for i in 0..PUSH_COUNT {
            let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect("push should arrive within timeout")
                .expect("stream should not end before all pushes arrive")
                .expect("frame should not error");
            assert_eq!(
                msg,
                Message::Text(format!("push-{i}").into()),
                "push-{i} should arrive in order before the close frame"
            );
        }

        let close_frame = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect(
                "close frame should arrive within timeout even though the queue held more \
                 than DEFAULT_OUTBOUND_CAPACITY items at drain start (regression: PR #736 \
                 Cursor Bugbot #discussion_r4113894722)",
            )
            .expect("stream should not end before close frame")
            .expect("frame should not error");
        match close_frame {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 4000);
                assert_eq!(frame.reason.as_str(), "bye");
            }
            other => panic!("expected a close frame with code/reason, got {other:?}"),
        }

        let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(reason, CloseReason::SenderClose),
            "expected SenderClose even though the queue held more than \
             DEFAULT_OUTBOUND_CAPACITY items at drain start, got {reason:?}"
        );
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
    }

    /// 脱出点対応表: ハンドラが `Err` →
    /// `(CloseReason::Failed(FailureKind::Handler), Err(WsError::Handler(_)))`。
    #[tokio::test]
    async fn handler_error_yields_handler_failure_reason() {
        use futures_util::future::BoxFuture;

        struct FailingHandler;

        impl handler::WsMessageHandler for FailingHandler {
            fn name(&self) -> &'static str {
                "failing"
            }

            fn on_message(
                &self,
                _msg: WsMessage,
            ) -> BoxFuture<'_, Result<WsOutcome, handler::WsHandlerError>> {
                Box::pin(async move { Err(handler::WsHandlerError::new("boom")) })
            }
        }

        let mut config = test_config();
        config.handler = std::sync::Arc::new(FailingHandler);
        let config: &'static WebSocketConfig = Box::leak(Box::new(config));

        let (server_side, client_side) = tokio::io::duplex(4096);
        let (tx, rx) = handler::channel(4);
        let conn_ctx = test_conn_ctx(tx);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session_inner(
                server_side,
                Vec::new(),
                config,
                cancel.as_mut(),
                Some(rx),
                &conn_ctx,
            )
            .await
        });

        let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        client
            .send(Message::Text("trigger".into()))
            .await
            .expect("client send should succeed");

        let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
            .await
            .expect("session should finish within timeout")
            .expect("session task should not panic");
        assert!(
            matches!(reason, CloseReason::Failed(FailureKind::Handler)),
            "expected Failed(Handler), got {reason:?}"
        );
        assert!(
            matches!(result, Err(WsError::Handler(_))),
            "expected Err(WsError::Handler(_)), got {result:?}"
        );
    }

    /// [`SessionFailure::recv`] / [`SessionFailure::send`] の分類テーブル
    /// 単体検証（実接続を経由せずエラー値を直接分類器に渡す）。設計 4 節の
    /// 対応表のうち、実接続で起こしにくい経路（送信失敗・`Io`）を
    /// カバーする。
    #[test]
    fn session_failure_recv_classifies_by_error_variant() {
        use tokio_tungstenite::tungstenite::Error as TError;
        use tokio_tungstenite::tungstenite::error::{CapacityError, ProtocolError};

        let capacity = SessionFailure::recv(TError::Capacity(CapacityError::MessageTooLong {
            size: 100,
            max_size: 10,
        }));
        assert!(matches!(capacity.reason, CloseReason::MessageTooLarge));

        let io = SessionFailure::recv(TError::Io(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        )));
        assert!(matches!(io.reason, CloseReason::Failed(FailureKind::Io)));

        // `ResetWithoutClosingHandshake` は `CloseReason::Eof` の doc が
        // 定義する事象（Close なし切断）と 1:1 対応するため `Eof` へ分類
        // する（イシュー #726 レビュー指摘対応。`_` 腕の分類不能エラーとは
        // 区別する）。
        let reset_without_close = SessionFailure::recv(TError::Protocol(
            ProtocolError::ResetWithoutClosingHandshake,
        ));
        assert!(matches!(reset_without_close.reason, CloseReason::Eof));

        // それ以外の `Protocol` variant は分類不能として `Failed(Protocol)`
        // へ倒す（フェイルクローズ）。
        let other_protocol =
            SessionFailure::recv(TError::Protocol(ProtocolError::SendAfterClosing));
        assert!(matches!(
            other_protocol.reason,
            CloseReason::Failed(FailureKind::Protocol)
        ));

        let connection_closed = SessionFailure::recv(TError::ConnectionClosed);
        assert!(matches!(
            connection_closed.reason,
            CloseReason::Failed(FailureKind::Protocol)
        ));
    }

    /// [`SessionFailure::send`] は受信側と異なり、`Capacity`・
    /// `ConnectionClosed`/`AlreadyClosed` を含む非 `Io` エラーをすべて
    /// `Failed(Protocol)` へ倒す（`ClientClose` への誤分類防止、設計 4 節
    /// レビュー指摘対応）。
    #[test]
    fn session_failure_send_never_maps_to_client_close() {
        use tokio_tungstenite::tungstenite::Error as TError;
        use tokio_tungstenite::tungstenite::error::CapacityError;

        let capacity = SessionFailure::send(TError::Capacity(CapacityError::MessageTooLong {
            size: 100,
            max_size: 10,
        }));
        assert!(matches!(
            capacity.reason,
            CloseReason::Failed(FailureKind::Protocol)
        ));

        let connection_closed = SessionFailure::send(TError::ConnectionClosed);
        assert!(matches!(
            connection_closed.reason,
            CloseReason::Failed(FailureKind::Protocol)
        ));

        let already_closed = SessionFailure::send(TError::AlreadyClosed);
        assert!(matches!(
            already_closed.reason,
            CloseReason::Failed(FailureKind::Protocol)
        ));

        let io = SessionFailure::send(TError::Io(std::io::Error::from(
            std::io::ErrorKind::BrokenPipe,
        )));
        assert!(matches!(io.reason, CloseReason::Failed(FailureKind::Io)));
    }

    /// イシュー #711 の回帰テスト群（`flush_outbound`）。
    ///
    /// `tests/handler_push_ordering_e2e.rs` の T3
    /// （`handler_push_beyond_capacity_arrives_before_close`）は既に
    /// 「`on_message` 内で push してから `WsOutcome::Close` を返す」経路で
    /// push が Close フレームより先に届くことを検証済みである（イシュー
    /// #706／PR #725 の `run_handler_with_outbound_drain` がハンドラ完了直後の
    /// 排出を担うため、本イシュー着手前から成立していた性質）。本モジュールの
    /// テストは、ハンドラ内の push 以外のケース（`run_handler_with_outbound_drain` の外、すなわち `on_open` で
    /// 保持した `WsSender` クローンを別タスクが独立に保持し続ける場合の
    /// 満杯チャネル解放・継続的な push の有界終了）に絞る。
    mod flush_outbound_tests {
        use super::*;

        /// 受け入れ基準（設計 2.1）: `on_message` 内で push してから
        /// `WsOutcome::Close` を返した場合、push がすべて Close フレームより
        /// 先に届き、`run_session` が `Ok(())` で終わること。
        ///
        /// 上記モジュール doc が述べるとおり `run_handler_with_outbound_drain`
        /// の排出により本イシュー着手前から成立する性質だが、
        /// `flush_outbound` 導入後も回帰しないことを固定するピンとして
        /// `run_session` を直接駆動する本モジュールの流儀で維持する。
        #[tokio::test]
        async fn queued_pushes_are_flushed_before_close() {
            struct PushThenCloseHandler;

            impl handler::WsMessageHandler for PushThenCloseHandler {
                fn name(&self) -> &'static str {
                    "push-then-close"
                }
                fn on_message(
                    &self,
                    _msg: WsMessage,
                ) -> futures_util::future::BoxFuture<'_, Result<WsOutcome, WsHandlerError>>
                {
                    Box::pin(async move { Ok(WsOutcome::Close) })
                }
                fn on_message_with_ctx<'a>(
                    &'a self,
                    ctx: &'a WsConnContext,
                    _msg: WsMessage,
                ) -> futures_util::future::BoxFuture<'a, Result<WsOutcome, WsHandlerError>>
                {
                    Box::pin(async move {
                        for i in 0..3 {
                            ctx.sender()
                                .send(WsMessage::Text(format!("push-{i}")))
                                .await
                                .expect("push should succeed before Close");
                        }
                        Ok(WsOutcome::Close)
                    })
                }
            }

            let mut config = test_config();
            config.handler = std::sync::Arc::new(PushThenCloseHandler);
            let config: &'static WebSocketConfig = Box::leak(Box::new(config));

            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            let (tx, rx) = handler::channel(4);
            let conn_ctx = test_conn_ctx(tx);

            let session_handle = tokio::spawn(async move {
                let cancel = std::future::pending::<()>();
                let mut cancel = std::pin::pin!(cancel);
                run_session(
                    server_side,
                    Vec::new(),
                    config,
                    cancel.as_mut(),
                    Some(rx),
                    &conn_ctx,
                )
                .await
            });

            let mut client =
                WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
            client
                .send(Message::Text("trigger".into()))
                .await
                .expect("client send should succeed");

            for i in 0..3 {
                let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
                    .await
                    .expect("push should arrive within timeout")
                    .expect("stream should not end early")
                    .expect("frame should not error");
                assert_eq!(
                    msg,
                    Message::Text(format!("push-{i}").into()),
                    "pushes must arrive in order before the close frame"
                );
            }

            let closing = tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect("close frame should arrive after all pushes")
                .expect("stream should not end early")
                .expect("frame should not error");
            assert!(
                matches!(closing, Message::Close(_)),
                "expected a close frame after all pushes, got {closing:?}"
            );

            drop(client);
            let result = tokio::time::timeout(Duration::from_secs(2), session_handle)
                .await
                .expect("session should end within timeout")
                .expect("session task should not panic");
            assert!(result.is_ok(), "session should end normally: {result:?}");
        }

        /// PR #735（イシュー #711）Codex P1 レビュー指摘の回帰テスト:
        /// ハンドラが `WsOutcome::Close` を返したときの排出（ハンドラ完了
        /// 時点で既に格納済みだった push の送出）が、`run_session` 経由でも
        /// `close_grace` で有界化されていること。
        ///
        /// [`flush_times_out_when_client_stops_reading`] は `apply_outcome`/
        /// `flush_outbound` を直接呼び出して検証するが、本テストは
        /// `on_message_with_ctx` 経由でメッセージをチャネル容量以内（かつ
        /// 1 回の poll でハンドラが完了しきる件数）だけ push させ、ステップ 1
        /// （ハンドラ Future と outbound 到着の race）では 1 件も消費されず
        /// ハンドラ完了後の排出の対象として残ることを利用し、クライアントが
        /// 受信を止めた状態でもセッションが `close_grace` を上限に終了する
        /// ことを検証する。
        #[tokio::test]
        async fn step3_drain_before_close_is_bounded_by_close_grace() {
            /// `on_message_with_ctx` の実行中に自身の outbound チャネルへ
            /// `PUSH_COUNT`（チャネル容量以内）件を push してから
            /// `WsOutcome::Close` を返すハンドラ。各 `send` はチャネルに
            /// 空きがある限り即座に解決するため、ハンドラ Future は 1 回の
            /// poll で完結し（`race2` が `rx.recv()` 側を一度も poll しない）、
            /// push した各メッセージはハンドラ完了後の排出対象として残る。
            struct QueueThenCloseHandler;

            const PUSH_COUNT: usize = 4;
            const PAYLOAD_LEN: usize = 4 * 1024;

            impl handler::WsMessageHandler for QueueThenCloseHandler {
                fn name(&self) -> &'static str {
                    "queue-then-close-step3"
                }
                fn on_message(
                    &self,
                    _msg: WsMessage,
                ) -> futures_util::future::BoxFuture<'_, Result<WsOutcome, WsHandlerError>>
                {
                    unreachable!("on_message_with_ctx をオーバーライドしているため呼ばれない")
                }
                fn on_message_with_ctx<'a>(
                    &'a self,
                    ctx: &'a WsConnContext,
                    _msg: WsMessage,
                ) -> futures_util::future::BoxFuture<'a, Result<WsOutcome, WsHandlerError>>
                {
                    Box::pin(async move {
                        for _ in 0..PUSH_COUNT {
                            ctx.sender()
                                .send(WsMessage::Binary(vec![b'x'; PAYLOAD_LEN]))
                                .await
                                .expect("push should succeed while capacity remains");
                        }
                        Ok(WsOutcome::Close)
                    })
                }
            }

            let mut config = test_config();
            config.close_grace = Duration::from_millis(200);
            config.handler = std::sync::Arc::new(QueueThenCloseHandler);
            let config: &'static WebSocketConfig = Box::leak(Box::new(config));

            // duplex バッファを極小にし、クライアントが読み取りを止めた
            // 状態で排出中の `ws.send()` が確実にブロックするように
            // する（`flush_times_out_when_client_stops_reading` と同型）。
            let (server_side, client_side) = tokio::io::duplex(8);
            let (tx, rx) = handler::channel(PUSH_COUNT);
            let conn_ctx = test_conn_ctx(tx);

            let session_handle = tokio::spawn(async move {
                let cancel = std::future::pending::<()>();
                let mut cancel = std::pin::pin!(cancel);
                run_session(
                    server_side,
                    Vec::new(),
                    config,
                    cancel.as_mut(),
                    Some(rx),
                    &conn_ctx,
                )
                .await
            });

            let mut client =
                WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
            client
                .send(Message::Text("trigger".into()))
                .await
                .expect("client send should succeed");

            // クライアント側は接続を保持するが以降一切読み出さない（受信を
            // 止めたクライアント役）。drop すると duplex が EOF を返し
            // close_grace の効果を検証できなくなるため、明示的に forget する。
            std::mem::forget(client);

            let started = tokio::time::Instant::now();
            let result = tokio::time::timeout(Duration::from_secs(5), session_handle)
                .await
                .expect(
                    "session must not hang indefinitely: step3's own drain must be \
                     bounded by close_grace even when the client stops reading",
                )
                .expect("session task should not panic");
            assert!(
                result.is_ok(),
                "a step3 drain timeout should still end the session normally: {result:?}"
            );
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "step3 should give up around close_grace (200ms), took {:?}",
                started.elapsed()
            );
        }

        /// 受け入れ基準（設計 2.1 手順 1）: `on_open` で保持した `WsSender`
        /// クローンを別タスクが独立に保持し、満杯チャネルで `send` に
        /// ブロックしている状態で `WsOutcome::Close` が適用されたとき、
        /// `flush_outbound` の `rx.close()` により待機中の `send` が
        /// 即座に [`WsSendError`] で解放されること。バッファ済みだった
        /// 1 件目 (`"first"`) はクライアントへ届くことも確認する
        /// （`rx.close()` は排出中のメッセージを破棄しない）。
        ///
        /// # レースの排除（Cursor Bugbot レビュー指摘対応）
        ///
        /// 旧実装は `run_session` 経由で駆動し、"first" を事前に
        /// `outbound` チャネルへ積んだ**あとで** `run_session` を起動し
        /// クライアントに `"trigger"` メッセージを送らせていた。しかし
        /// `outbound` の `recv()` は既にキュー済みのため即座に `Ready` に
        /// なる一方、クライアントの `"trigger"` は `io::duplex` 経由の
        /// 実際の非同期往復を要するため、`run_session` 外側ループの
        /// 最初の反復では通常ほぼ確実に outbound 側（`InboundEvent::
        /// Outbound`）が先に選ばれる。この経路は `flush_outbound` を経由
        /// せず素の `ws.send()` で "first" を送出してしまい、その時点で
        /// 満杯チャネルの容量が空いて "second" が（Close を経由せず）
        /// 成功してしまいうる。つまり旧テストは「Close 前に flush される
        /// こと」ではなく「たまたま外側ループの通常経路がその前に容量を
        /// 空けなかったこと」を検証してしまうレースを抱えていた
        /// （PR #735 レビュー指摘、threadId 記録は PR 本文参照）。
        ///
        /// 本テストは `run_session` を経由せず、`apply_outcome`
        /// （`flush_outbound` の唯一の呼び出し元）を直接呼ぶことで、
        /// 外側ループの通常経路が介在する余地を構造的に排除する。
        #[tokio::test]
        async fn blocked_sender_is_released_when_close_flushes() {
            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            let mut server_ws =
                WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            let mut client_ws =
                WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

            let (tx, rx) = handler::channel(1);
            tx.send(WsMessage::Text("first".to_string()))
                .await
                .expect("first send fills capacity 1 without blocking");

            let tx2 = tx.clone();
            let blocked =
                tokio::spawn(async move { tx2.send(WsMessage::Text("second".to_string())).await });

            // `blocked` タスクが満杯チャネルの permit 取得待ちとして実際に
            // 登録されるまでスケジューラへ制御を明示的に譲る。
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }

            let mut outbound = OutboundGuard::new(Some(rx), &tx, Duration::from_secs(10));
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            // ドレイン待ちを長めに取り、「即座に解放される」ことと
            // 「close_grace 満了まで待たされる」ことを明確に区別できるように
            // する（既存テスト `blocked_send_is_released_immediately_on_
            // cancellation` と同じ構成）。
            let close_grace = Duration::from_secs(2);
            let close_deadline = Instant::now() + close_grace;

            let flow = apply_outcome(
                &mut server_ws,
                WsOutcome::Close,
                &mut outbound,
                cancel.as_mut(),
                close_deadline,
                None,
            )
            .await
            .unwrap_or_else(|_| panic!("apply_outcome should not fail"));
            assert!(
                matches!(flow, SessionFlow::Closed),
                "WsOutcome::Close should end the session"
            );

            let blocked_result = tokio::time::timeout(Duration::from_millis(500), blocked)
                .await
                .expect(
                    "blocked WsSender::send should be released well before close_grace \
                     (2s) elapses",
                )
                .expect("task should not panic");
            assert_eq!(
                blocked_result,
                Err(WsSendError),
                "send should fail once flush_outbound closes the receiver"
            );

            let first = tokio::time::timeout(Duration::from_secs(2), client_ws.next())
                .await
                .expect("buffered first message should still be flushed")
                .expect("stream should not end early")
                .expect("frame should not error");
            assert_eq!(
                first,
                Message::Text("first".into()),
                "the message already buffered before close must not be dropped"
            );

            let closing = tokio::time::timeout(Duration::from_secs(2), client_ws.next())
                .await
                .expect("close frame should arrive after flush")
                .expect("stream should not end early")
                .expect("frame should not error");
            assert!(
                matches!(closing, Message::Close(_)),
                "expected a close frame after flush, got {closing:?}"
            );
        }

        /// 受け入れ基準（DoS 回帰防止、設計 7 節）: 満杯チャネルへ継続的に
        /// push し続けるバックグラウンドタスクが存在しても、ハンドラが
        /// `WsOutcome::Close` を返した時点で `flush_outbound` が `rx.close()`
        /// を呼ぶため、セッションは有界時間内に終わること（無制限に
        /// バッファする・無期限に待つことがない）。
        #[tokio::test]
        async fn continuous_pusher_does_not_prevent_close() {
            struct CloseImmediatelyHandler;
            impl handler::WsMessageHandler for CloseImmediatelyHandler {
                fn name(&self) -> &'static str {
                    "close-immediately-under-load"
                }
                fn on_message(
                    &self,
                    _msg: WsMessage,
                ) -> futures_util::future::BoxFuture<'_, Result<WsOutcome, WsHandlerError>>
                {
                    Box::pin(async move { Ok(WsOutcome::Close) })
                }
            }

            let mut config = test_config();
            config.handler = std::sync::Arc::new(CloseImmediatelyHandler);
            config.close_grace = Duration::from_secs(2);
            let config: &'static WebSocketConfig = Box::leak(Box::new(config));

            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            let (tx, rx) = handler::channel(4);
            let conn_ctx = test_conn_ctx(tx.clone());

            let pusher = tokio::spawn(async move {
                let mut i: u64 = 0;
                loop {
                    if tx.send(WsMessage::Text(format!("push-{i}"))).await.is_err() {
                        break;
                    }
                    i += 1;
                }
            });

            let session_handle = tokio::spawn(async move {
                let cancel = std::future::pending::<()>();
                let mut cancel = std::pin::pin!(cancel);
                run_session(
                    server_side,
                    Vec::new(),
                    config,
                    cancel.as_mut(),
                    Some(rx),
                    &conn_ctx,
                )
                .await
            });

            let mut client =
                WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
            client
                .send(Message::Text("trigger".into()))
                .await
                .expect("client send should succeed");

            // クライアントは受信を続け、`ws.send()` 自体が duplex 輻輳で
            // ブロックしないようにする（本テストが検証したい「継続 push が
            // Close を無期限に遅らせないこと」とは別種の遅延を混入させない
            // ため）。
            let drain_client =
                tokio::spawn(async move { while let Some(Ok(_)) = client.next().await {} });

            let result = tokio::time::timeout(Duration::from_secs(1), session_handle)
                .await
                .expect("session should end within a bounded time despite continuous pushes")
                .expect("session task should not panic");
            assert!(result.is_ok(), "session should end normally: {result:?}");

            let _ = tokio::time::timeout(Duration::from_secs(2), pusher).await;
            let _ = tokio::time::timeout(Duration::from_secs(2), drain_client).await;
        }

        /// 受け入れ基準（イシュー #711 Codex P1 レビュー指摘対応）: クライアント
        /// が受信を止めている（`ws.send()` の書き込みが TCP バックプレッシャ
        /// 相当で無期限にブロックする）場合でも、`flush_outbound` は
        /// `close_grace` を上限に排出を諦め、`apply_outcome` が
        /// [`SessionFlow::Closed`] で（`ws.close()` を試みずに）終わること。
        /// [`continuous_pusher_does_not_prevent_close`] は push の継続に
        /// 対する有界性を検証するが、クライアントが `client.next()` で
        /// 受信を続けるため `ws.send()` 自体は一度もブロックしない。本テストは
        /// 逆に、送出そのものが長時間ブロックする状況（P1 指摘の本体）を
        /// 直接再現する。
        #[tokio::test]
        async fn flush_times_out_when_client_stops_reading() {
            // duplex バッファを極小にし、クライアント側を一切読まないままに
            // することで、`ws.send()` の書き込みが TCP 送信バッファ満杯相当で
            // ブロックし続ける状況を再現する。
            let (server_side, client_side) = tokio::io::duplex(8);
            let mut server_ws =
                WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            // クライアント側は接続を保持するが読み出さない（受信を止めた
            // クライアント役）。drop すると即座に EOF/エラーになり
            // 「無期限にブロックする」状況を再現できないため、明示的に
            // 生存させる。
            let _client_side = client_side;

            let (tx, rx) = handler::channel(4);
            for i in 0..4 {
                tx.send(WsMessage::Text(format!("msg-{i}")))
                    .await
                    .expect("send should succeed while channel capacity remains");
            }

            let mut outbound = OutboundGuard::new(Some(rx), &tx, Duration::from_secs(10));
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            let close_grace = Duration::from_millis(200);
            let close_deadline = Instant::now() + close_grace;

            let started = tokio::time::Instant::now();
            let flow = tokio::time::timeout(
                Duration::from_secs(5),
                apply_outcome(
                    &mut server_ws,
                    WsOutcome::Close,
                    &mut outbound,
                    cancel.as_mut(),
                    close_deadline,
                    None,
                ),
            )
            .await
            .expect(
                "apply_outcome must not hang indefinitely when the client stops reading \
                 (flush_outbound must be bounded by close_grace)",
            )
            .unwrap_or_else(|_| panic!("apply_outcome should not fail"));

            assert!(
                matches!(flow, SessionFlow::Closed),
                "a flush timeout should still end the session"
            );
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "flush_outbound should give up around close_grace (200ms), took {:?}",
                started.elapsed()
            );
        }

        /// 受け入れ基準（イシュー #711 PR #735 レビュー指摘 P1 #2 対応）:
        /// `flush_outbound` が排出すべきメッセージなし（`outbound` が
        /// `None`）で即座に完了したとしても、続く `ws.close(None)` 自体の
        /// 書き込みがクライアントの受信停止で無期限にブロックしうる場合、
        /// `close_deadline` によって同じ猶予内で打ち切られ
        /// [`SessionFlow::Closed`] を返すこと（cancel との race だけでは
        /// 打ち切れない書き込みブロックに対する有界化）。
        #[tokio::test]
        async fn close_send_times_out_when_client_stops_reading() {
            // duplex バッファを極小にし、`ws.close(None)` の Close フレーム
            // 書き込み自体が TCP 送信バッファ満杯相当でブロックし続ける状況を
            // 再現する。
            let (server_side, client_side) = tokio::io::duplex(4);
            let mut server_ws =
                WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            // クライアント側は接続を保持するが読み出さない。
            let _client_side = client_side;

            // `outbound` を `None` にして `flush_outbound` を即完了（
            // `FlushOutcome::Done`）させ、`ws.close(None)` 自体の有界化のみを
            // 検証する。
            let (tx, _rx) = handler::channel(1);
            let mut outbound = OutboundGuard::new(None, &tx, Duration::from_secs(10));
            // cancel は発火しないままにする。`ws.close(None)` の打ち切り手段が
            // `close_deadline` しかないことを保証する（cancel が打ち切りの
            // 唯一の手段だった旧実装ではこのテストはハングする）。
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            let close_grace = Duration::from_millis(200);
            let close_deadline = Instant::now() + close_grace;

            let started = tokio::time::Instant::now();
            let flow = tokio::time::timeout(
                Duration::from_secs(5),
                apply_outcome(
                    &mut server_ws,
                    WsOutcome::Close,
                    &mut outbound,
                    cancel.as_mut(),
                    close_deadline,
                    None,
                ),
            )
            .await
            .expect(
                "apply_outcome must not hang indefinitely when the client stops reading \
                 (ws.close(None) must be bounded by close_deadline, not just by cancel)",
            )
            .unwrap_or_else(|_| panic!("apply_outcome should not fail"));

            assert!(
                matches!(flow, SessionFlow::Closed),
                "a close-send timeout should still end the session"
            );
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "ws.close(None) should give up around close_grace (200ms), took {:?}",
                started.elapsed()
            );
        }

        /// 受け入れ基準（イシュー #711 PR #735 レビュー指摘 P1 #1 対応）:
        /// Close ハンドシェイク全体（`flush_outbound` による排出 +
        /// `ws.close(None)`）が単一の
        /// `close_deadline`（`Instant`）を共有し、他ステップが既に予算を
        /// 使い果たしていた場合は追加で `close_grace` 分の猶予を新たに
        /// 得られないこと。`flush_outbound` へ既に期限切れの `close_deadline`
        /// を渡し、`close_grace` 相当の待機を伴わず即座に
        /// [`FlushOutcome::TimedOut`] を返すことを直接検証する（各ステップが
        /// 独立に `close_grace` 全量で `timeout` する旧実装では、ここで
        /// 新たに `close_grace` 分待たされ、他ステップ分と合算で最大約 2 倍の
        /// 時間を要していた）。
        #[tokio::test]
        async fn flush_outbound_respects_already_expired_deadline_immediately() {
            // duplex バッファを極小にし、クライアント側を読み出さないままに
            // することで、`ws.send()` の書き込みが最初の `poll` で `Pending`
            // になる状況を作る（`try_recv()` 自体は同期処理のため即完了する
            // が、続く `ws.send()` の書き込みが必ず一度 await する必要が
            // あるようにする。バッファに余裕があると送出が同期的に完了し、
            // `timeout_at` が期限切れを検知する前にフラッシュ自体が終わって
            // しまい、本テストが検証したい「期限切れなら待たされない」経路を
            // 通らない）。
            let (server_side, client_side) = tokio::io::duplex(4);
            let mut server_ws =
                WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            let _client_side = client_side;

            let (tx, rx) = handler::channel(4);
            tx.send(WsMessage::Text("queued".to_string()))
                .await
                .expect("send should succeed while channel capacity remains");

            let mut outbound = OutboundGuard::new(Some(rx), &tx, Duration::from_secs(10));
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            // 他ステップが
            // 既に `close_grace` 予算を使い果たした状況を模し、過去の時刻を
            // 期限として渡す。
            let already_expired_deadline = Instant::now() - Duration::from_secs(1);

            let started = Instant::now();
            let outcome = flush_outbound(
                &mut server_ws,
                &mut outbound,
                cancel.as_mut(),
                already_expired_deadline,
            )
            .await
            .unwrap_or_else(|_| panic!("flush_outbound should not fail"));

            assert!(
                matches!(outcome, FlushOutcome::TimedOut),
                "an already-expired shared deadline must yield an immediate timeout, \
                 not a fresh close_grace-length wait"
            );
            assert!(
                started.elapsed() < Duration::from_millis(100),
                "flush_outbound must not wait for a fresh close_grace budget when the \
                 shared deadline has already elapsed, took {:?}",
                started.elapsed()
            );
        }
    }

    /// ハンドラ完了後の送信キュー排出（[`run_handler_with_outbound_drain`]）の
    /// 有界性・取りこぼし防止の回帰テスト（PR #736 codex P0/P1 レビュー指摘
    /// 対応）。
    ///
    /// いずれも `sleep` に頼らず、送出（`poll_write`）に同期して補充・失敗を
    /// 起こすテスト用ストリーム [`TestStream`] と、permit 確保と確定を分離する
    /// テスト専用ヘルパー（`WsSender::reserve_close_for_test`/
    /// `commit_close_for_test`）で競合を決定的に再現する。
    mod drain_termination_tests {
        use super::*;
        use futures_util::FutureExt;
        use futures_util::future::BoxFuture;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use tokio::io::{DuplexStream, ReadBuf};

        /// 本モジュールのテストで使う送信キュー容量（継続経路の排出回数上限
        /// `handler::DEFAULT_OUTBOUND_CAPACITY` と一致させる。本番の
        /// `handle_upgrade` と同じ容量でチャネルを作るため）。
        const CAPACITY: usize = handler::DEFAULT_OUTBOUND_CAPACITY;
        /// [`TestStream`] が補充する push の上限件数。旧実装（`Empty` まで
        /// 無制限に排出）ではこの件数ぶん補充 push が Reply/Close より先に
        /// 送出されるため、ハングではなく順序アサーションの失敗として検出
        /// できる。
        const REFILL_LIMIT: usize = 64;

        /// サーバー側ストリームのラッパー。`poll_write` のたびに
        /// (1) `refill` が `Some` なら送信キューへ 1 件 push を試み
        /// （空きがあれば同期的に確定する「送り続ける別タスク」役）、
        /// (2) `fail_writes` が true なら書き込みを `BrokenPipe` で失敗させる。
        struct TestStream {
            inner: DuplexStream,
            refill: Option<handler::WsSender>,
            refilled: Arc<AtomicUsize>,
            fail_writes: Arc<AtomicBool>,
            /// `Some` なら最初の `poll_write` で 1 回だけ `close(4000, "bye")` を
            /// 同期的に試み、結果を `close_result` に残す（排出の途中で close が
            /// 確定する競合の再現用）。
            close_on_first_write: Option<handler::WsSender>,
            close_result: Arc<std::sync::Mutex<Option<Result<(), handler::WsCloseError>>>>,
        }

        impl TestStream {
            fn new(inner: DuplexStream) -> Self {
                Self {
                    inner,
                    refill: None,
                    refilled: Arc::new(AtomicUsize::new(0)),
                    fail_writes: Arc::new(AtomicBool::new(false)),
                    close_on_first_write: None,
                    close_result: Arc::new(std::sync::Mutex::new(None)),
                }
            }
        }

        impl AsyncRead for TestStream {
            fn poll_read(
                mut self: Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                Pin::new(&mut self.inner).poll_read(cx, buf)
            }
        }

        impl AsyncWrite for TestStream {
            fn poll_write(
                mut self: Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                if self.fail_writes.load(Ordering::SeqCst) {
                    return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
                }
                let result = Pin::new(&mut self.inner).poll_write(cx, buf);
                if let Some(closer) = self.close_on_first_write.take() {
                    let closed = closer.close(4000, "bye").now_or_never();
                    *self
                        .close_result
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = closed;
                }
                if let Some(sender) = self.refill.as_ref() {
                    let n = self.refilled.load(Ordering::SeqCst);
                    if n < REFILL_LIMIT {
                        let pushed = sender
                            .send(WsMessage::Text(format!("refill-{n}")))
                            .now_or_never();
                        if matches!(pushed, Some(Ok(()))) {
                            self.refilled.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
                result
            }

            fn poll_flush(
                mut self: Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Pin::new(&mut self.inner).poll_flush(cx)
            }

            fn poll_shutdown(
                mut self: Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Pin::new(&mut self.inner).poll_shutdown(cx)
            }
        }

        /// 自身の送信キューへ [`CAPACITY`] 件を push してから `outcome` を
        /// 返すハンドラ。各 `send` は空きがあるため即座に解決し、ハンドラ
        /// Future は 1 回の poll で完結する（push はすべて排出ステップの
        /// 対象として残る）。`arm_on_return` が `Some` の場合は戻る直前に
        /// true にする（[`TestStream::fail_writes`] の起動用）。
        struct FillThenReturnHandler {
            outcome: fn() -> Result<WsOutcome, WsHandlerError>,
            arm_on_return: Option<Arc<AtomicBool>>,
        }

        impl handler::WsMessageHandler for FillThenReturnHandler {
            fn name(&self) -> &'static str {
                "fill-then-return"
            }

            fn on_message(
                &self,
                _msg: WsMessage,
            ) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
                Box::pin(async move { (self.outcome)() })
            }

            fn on_message_with_ctx<'a>(
                &'a self,
                ctx: &'a WsConnContext,
                _msg: WsMessage,
            ) -> BoxFuture<'a, Result<WsOutcome, WsHandlerError>> {
                Box::pin(async move {
                    for i in 0..CAPACITY {
                        ctx.sender()
                            .send(WsMessage::Text(format!("push-{i}")))
                            .await
                            .expect("push within capacity should succeed");
                    }
                    if let Some(flag) = self.arm_on_return.as_ref() {
                        flag.store(true, Ordering::SeqCst);
                    }
                    (self.outcome)()
                })
            }
        }

        type SessionHandle = tokio::task::JoinHandle<(CloseReason, Result<(), WsError>)>;

        /// `handler` を登録した `run_session_inner` を `stream` 上で起動し、
        /// クライアントから 1 フレーム送ってハンドラを 1 回起動する。
        async fn start_session(
            handler: FillThenReturnHandler,
            stream: TestStream,
            client_side: DuplexStream,
            tx: handler::WsSender,
            rx: mpsc::Receiver<OutboundItem>,
        ) -> (SessionHandle, WebSocketStream<DuplexStream>) {
            let mut config = test_config();
            config.handler = Arc::new(handler);
            config.close_grace = Duration::from_secs(2);
            let config: &'static WebSocketConfig = Box::leak(Box::new(config));
            let conn_ctx = test_conn_ctx(tx);

            let session_handle = tokio::spawn(async move {
                let cancel = std::future::pending::<()>();
                let mut cancel = std::pin::pin!(cancel);
                run_session_inner(
                    stream,
                    Vec::new(),
                    config,
                    cancel.as_mut(),
                    Some(rx),
                    &conn_ctx,
                )
                .await
            });

            let mut client =
                WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
            client
                .send(Message::Text("trigger".into()))
                .await
                .expect("client send should succeed");
            (session_handle, client)
        }

        async fn next_frame(client: &mut WebSocketStream<DuplexStream>) -> Option<Message> {
            tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect("a frame or end of stream should arrive within timeout")
                .and_then(Result::ok)
        }

        /// codex P0（継続経路）: 別タスク役（[`TestStream`] の補充）が
        /// 送信キューへ送り続けても、`WsOutcome::Reply` を返したハンドラの
        /// 排出は容量ぶんで終わり、排出開始時点で格納済みだった push の直後に
        /// Reply が送出されること。旧実装（`Empty` まで無制限）では補充
        /// push が Reply より先に並び続ける。
        #[tokio::test]
        async fn reply_drain_is_bounded_while_another_task_keeps_sending() {
            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            let (tx, rx) = handler::channel(CAPACITY);
            let mut stream = TestStream::new(server_side);
            stream.refill = Some(tx.clone());
            let handler = FillThenReturnHandler {
                outcome: || Ok(WsOutcome::Reply(vec![WsMessage::Text("reply".to_string())])),
                arm_on_return: None,
            };
            let (session_handle, mut client) =
                start_session(handler, stream, client_side, tx, rx).await;

            for i in 0..CAPACITY {
                assert_eq!(
                    next_frame(&mut client).await,
                    Some(Message::Text(format!("push-{i}").into())),
                    "pushes queued before the drain must precede the reply"
                );
            }
            assert_eq!(
                next_frame(&mut client).await,
                Some(Message::Text("reply".into())),
                "the reply must follow right after the pushes queued at drain start \
                 (the drain must not keep consuming items refilled during the drain)"
            );

            session_handle.abort();
        }

        /// codex P0（終了経路、`WsOutcome::Close`）: 送り続ける別タスク役が
        /// いても、送信キューを封鎖してから排出するため補充は失敗し、格納済み push
        /// の直後に Close フレームが届くこと。
        #[tokio::test]
        async fn close_drain_is_bounded_while_another_task_keeps_sending() {
            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            let (tx, rx) = handler::channel(CAPACITY);
            let mut stream = TestStream::new(server_side);
            stream.refill = Some(tx.clone());
            let refilled = Arc::clone(&stream.refilled);
            let handler = FillThenReturnHandler {
                outcome: || Ok(WsOutcome::Close),
                arm_on_return: None,
            };
            let (session_handle, mut client) =
                start_session(handler, stream, client_side, tx, rx).await;

            for i in 0..CAPACITY {
                assert_eq!(
                    next_frame(&mut client).await,
                    Some(Message::Text(format!("push-{i}").into())),
                );
            }
            let closing = next_frame(&mut client).await;
            assert!(
                matches!(closing, Some(Message::Close(_))),
                "the close frame must follow right after the queued pushes, got {closing:?}"
            );
            assert_eq!(
                refilled.load(Ordering::SeqCst),
                0,
                "no push may be enqueued once the termination drain has closed the queue"
            );

            drop(client);
            let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
                .await
                .expect("session should finish within timeout")
                .expect("session task should not panic");
            assert!(
                matches!(reason, CloseReason::HandlerClose),
                "expected HandlerClose, got {reason:?}"
            );
            assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        }

        /// codex P0（終了経路、ハンドラ `Err`）: 送り続ける別タスク役が
        /// いても排出は有界で終わり、格納済み push の後にセッションが
        /// `Failed(Handler)` で終わること。
        #[tokio::test]
        async fn error_drain_is_bounded_while_another_task_keeps_sending() {
            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            let (tx, rx) = handler::channel(CAPACITY);
            let mut stream = TestStream::new(server_side);
            stream.refill = Some(tx.clone());
            let refilled = Arc::clone(&stream.refilled);
            let handler = FillThenReturnHandler {
                outcome: || Err(WsHandlerError::new("boom")),
                arm_on_return: None,
            };
            let (session_handle, mut client) =
                start_session(handler, stream, client_side, tx, rx).await;

            for i in 0..CAPACITY {
                assert_eq!(
                    next_frame(&mut client).await,
                    Some(Message::Text(format!("push-{i}").into())),
                );
            }

            let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
                .await
                .expect("session should finish within timeout")
                .expect("session task should not panic");
            assert!(
                matches!(reason, CloseReason::Failed(FailureKind::Handler)),
                "expected Failed(Handler), got {reason:?}"
            );
            assert!(
                matches!(result, Err(WsError::Handler(_))),
                "expected Err(WsError::Handler(_)), got {result:?}"
            );
            assert_eq!(
                next_frame(&mut client).await,
                None,
                "no refilled push may follow the pushes queued before the queue was closed"
            );
            assert_eq!(refilled.load(Ordering::SeqCst), 0);
        }

        /// ハンドラ `Err` 経路の排出中に送信が失敗しても、戻り値は元の
        /// ハンドラエラー（`Failed(Handler)` + `WsError::Handler`）のまま
        /// であること（送信エラーで上書きしない）。
        #[tokio::test]
        async fn send_failure_during_error_drain_keeps_handler_error() {
            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            let (tx, rx) = handler::channel(CAPACITY);
            let stream = TestStream::new(server_side);
            let handler = FillThenReturnHandler {
                outcome: || Err(WsHandlerError::new("boom")),
                arm_on_return: Some(Arc::clone(&stream.fail_writes)),
            };
            let (session_handle, _client) =
                start_session(handler, stream, client_side, tx, rx).await;

            let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
                .await
                .expect("session should finish within timeout")
                .expect("session task should not panic");
            assert!(
                matches!(reason, CloseReason::Failed(FailureKind::Handler)),
                "a send failure during the drain must not replace the handler failure, \
                 got {reason:?}"
            );
            assert!(
                matches!(result, Err(WsError::Handler(_))),
                "expected the original Err(WsError::Handler(_)), got {result:?}"
            );
        }

        /// codex P1: `WsSender::close` が permit を確保した後・確定する前に
        /// セッションがハンドラ `Err` で終了経路へ入り送信キューを封鎖した
        /// 場合、その後の確定は `Err(Closed)` になり、`Ok` を返したのに届かない
        /// Close が生じないこと。マルチスレッドでは permit 確保と確定の間に
        /// 別スレッドが封鎖しうるため、テスト専用ヘルパーでその順序を決定的に
        /// 再現する（封鎖の後に完了する `closed()` を同期点に使う）。封鎖前に
        /// 確定した Close が届くことは
        /// `sender_close_then_handler_error_still_delivers_pending_push_and_close`
        /// が検証する。
        #[tokio::test]
        async fn close_committed_after_termination_seal_is_rejected() {
            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            let (tx, rx) = handler::channel(CAPACITY);
            let closer = tx.clone();
            let permit = closer
                .reserve_close_for_test()
                .await
                .expect("permit should be available before the session ends");

            struct FailImmediately;
            impl handler::WsMessageHandler for FailImmediately {
                fn name(&self) -> &'static str {
                    "fail-immediately"
                }
                fn on_message(
                    &self,
                    _msg: WsMessage,
                ) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
                    Box::pin(async move { Err(WsHandlerError::new("boom")) })
                }
            }

            let mut config = test_config();
            config.handler = Arc::new(FailImmediately);
            let config: &'static WebSocketConfig = Box::leak(Box::new(config));
            let conn_ctx = test_conn_ctx(tx);
            let session_handle = tokio::spawn(async move {
                let cancel = std::future::pending::<()>();
                let mut cancel = std::pin::pin!(cancel);
                run_session_inner(
                    server_side,
                    Vec::new(),
                    config,
                    cancel.as_mut(),
                    Some(rx),
                    &conn_ctx,
                )
                .await
            });

            let mut client =
                WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
            client
                .send(Message::Text("trigger".into()))
                .await
                .expect("client send should succeed");

            // 受信側が閉じられるまで待つ（封鎖はその前に完了している）。
            tokio::time::timeout(Duration::from_secs(2), closer.closed())
                .await
                .expect("the session should close the outbound queue on handler error");
            let committed = closer.commit_close_for_test(permit, 4000, "late");
            assert_eq!(
                committed,
                Err(handler::WsCloseError::Closed),
                "a commit after the termination seal must be rejected, not silently dropped"
            );

            let (reason, result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
                .await
                .expect("session should finish within timeout")
                .expect("session task should not panic");
            assert!(
                matches!(reason, CloseReason::Failed(FailureKind::Handler)),
                "expected Failed(Handler), got {reason:?}"
            );
            assert!(
                matches!(result, Err(WsError::Handler(_))),
                "expected Err(WsError::Handler(_)), got {result:?}"
            );
            assert_eq!(
                next_frame(&mut client).await,
                None,
                "a rejected Close must not reach the client"
            );
        }

        /// codex P1（`flush_outbound` 単体、`WsOutcome::Close` 経路と共通）:
        /// 封鎖より前に確定した Close 指示は `try_recv()` だけで排出され、
        /// 受信側の起床を待たずに即座に返ること（tokio のバージョンに依存しない）。
        #[tokio::test]
        async fn flush_outbound_drains_close_committed_before_seal() {
            let (server_side, _client_side) = tokio::io::duplex(1 << 16);
            let mut server_ws =
                WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            let (tx, rx) = handler::channel(CAPACITY);
            let permit = tx
                .reserve_close_for_test()
                .await
                .expect("permit should be available");
            tx.commit_close_for_test(permit, 4000, "late")
                .expect("a commit before the seal must succeed");

            let mut outbound = OutboundGuard::new(Some(rx), &tx, Duration::from_secs(10));
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            let close_deadline = Instant::now() + Duration::from_secs(2);
            let outcome = flush_outbound(
                &mut server_ws,
                &mut outbound,
                cancel.as_mut(),
                close_deadline,
            )
            .now_or_never()
            .expect("the drain must not wait for the receiver to be woken")
            .unwrap_or_else(|_| panic!("flush_outbound should not fail"));
            match outcome {
                FlushOutcome::SenderClose { code, reason, .. } => {
                    assert_eq!(code, 4000);
                    assert_eq!(reason, "late");
                }
                _ => panic!("expected the committed Close instruction to be drained"),
            }
        }

        /// codex P1 / P2（`flush_outbound` 単体）: 封鎖時点で permit を保持して
        /// いた送信者がいても排出はその確定を待たずに返り、封鎖後の確定は
        /// `Err(Closed)` になること（`Ok` を返したのに届かない Close が生じない）。
        #[tokio::test]
        async fn flush_outbound_rejects_commit_after_seal_without_waiting() {
            let (server_side, _client_side) = tokio::io::duplex(1 << 16);
            let mut server_ws =
                WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            let (tx, rx) = handler::channel(CAPACITY);
            let permit = tx
                .reserve_close_for_test()
                .await
                .expect("permit should be available");

            let mut outbound = OutboundGuard::new(Some(rx), &tx, Duration::from_secs(10));
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            let close_deadline = Instant::now() + Duration::from_secs(2);
            let outcome = flush_outbound(
                &mut server_ws,
                &mut outbound,
                cancel.as_mut(),
                close_deadline,
            )
            .now_or_never()
            .expect("the drain must not wait for an outstanding permit")
            .unwrap_or_else(|_| panic!("flush_outbound should not fail"));
            assert!(
                matches!(outcome, FlushOutcome::Done),
                "nothing was committed before the seal"
            );
            assert!(tx.is_closed(), "the drain must seal the queue");

            assert_eq!(
                tx.commit_close_for_test(permit, 4000, "late"),
                Err(handler::WsCloseError::Closed),
                "a commit after the seal must be rejected"
            );
            assert_eq!(
                tx.send(WsMessage::Text("after-seal".to_string())).await,
                Err(WsSendError),
                "a send after the seal must be rejected"
            );
        }

        /// 排出しない終了経路（PR #736 レビュー指摘 P3-2）の回帰テスト本体:
        /// permit を確保した送信者がいる状態で `cancel` を発火させるか
        /// （`OutboundGuard::release` 経由）、クライアントを切断して EOF で抜けさせ
        /// （`OutboundGuard` の `Drop` 経由）、受信側が drop された後の確定が `Ok` ではなく
        /// `Err(Closed)` になることを確かめる（封鎖前は `Ok` を返して値が捨てられて
        /// いた）。
        async fn assert_commit_rejected_after_exit(fire_cancel: bool) {
            let config: &'static WebSocketConfig = Box::leak(Box::new(test_config()));
            let (server_side, client_side) = tokio::io::duplex(4096);
            let (tx, rx) = handler::channel(CAPACITY);
            let closer = tx.clone();
            let permit = closer
                .reserve_close_for_test()
                .await
                .expect("permit should be available before the session ends");
            let conn_ctx = test_conn_ctx(tx);

            let session_handle = tokio::spawn(async move {
                let cancel = async move {
                    if !fire_cancel {
                        std::future::pending::<()>().await;
                    }
                };
                let mut cancel = std::pin::pin!(cancel);
                run_session_inner(
                    server_side,
                    Vec::new(),
                    config,
                    cancel.as_mut(),
                    Some(rx),
                    &conn_ctx,
                )
                .await
            });
            // cancel 経路ではクライアントを生かしたまま（Close 応答は返さない）、
            // EOF 経路では切断する。
            let _client_side = if fire_cancel {
                Some(client_side)
            } else {
                drop(client_side);
                None
            };

            // 受信側が drop されるまで待つ（封鎖はその前に完了している）。
            tokio::time::timeout(Duration::from_secs(2), closer.closed())
                .await
                .expect("the session should release the outbound queue");
            assert_eq!(
                closer.commit_close_for_test(permit, 4000, "late"),
                Err(handler::WsCloseError::Closed),
                "a commit after the receiver was released must be rejected, not silently dropped"
            );

            let (reason, _result) = tokio::time::timeout(Duration::from_secs(2), session_handle)
                .await
                .expect("session should finish within timeout")
                .expect("session task should not panic");
            if fire_cancel {
                assert!(
                    matches!(reason, CloseReason::Cancelled),
                    "expected Cancelled, got {reason:?}"
                );
            } else {
                assert!(
                    matches!(reason, CloseReason::Eof),
                    "expected Eof, got {reason:?}"
                );
            }
        }

        /// cancel 経路（`OutboundGuard::release`）で受信側を手放す前に封鎖されること。
        #[tokio::test]
        async fn cancel_path_seals_before_releasing_outbound() {
            assert_commit_rejected_after_exit(true).await;
        }

        /// EOF 経路（`run_session_inner` を抜ける際の `OutboundGuard` の `Drop`）で
        /// 受信側を drop する前に封鎖されること。
        #[tokio::test]
        async fn eof_path_seals_before_dropping_outbound() {
            assert_commit_rejected_after_exit(false).await;
        }

        /// PR #736 レビュー指摘 P3-4: 終了経路の排出中に見つかった Close 指示は、
        /// 排出と共有する期限（`close_deadline`）をそのまま引き継ぎ、Close
        /// ハンドシェイクに新たな `close_grace` を与えないこと。
        #[tokio::test]
        async fn sender_close_found_in_flush_inherits_close_deadline() {
            let (server_side, _client_side) = tokio::io::duplex(1 << 16);
            let mut server_ws =
                WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            let (tx, rx) = handler::channel(CAPACITY);
            tx.close(4000, "bye").await.expect("close should succeed");

            let mut outbound = OutboundGuard::new(Some(rx), &tx, Duration::from_secs(10));
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            let close_deadline = Instant::now() + Duration::from_secs(2);
            let flow = apply_outcome(
                &mut server_ws,
                WsOutcome::Close,
                &mut outbound,
                cancel.as_mut(),
                close_deadline,
                None,
            )
            .await
            .unwrap_or_else(|_| panic!("apply_outcome should not fail"));
            match flow {
                SessionFlow::SenderClose {
                    code,
                    reason,
                    deadline,
                } => {
                    assert_eq!(code, 4000);
                    assert_eq!(reason, "bye");
                    assert_eq!(
                        deadline, close_deadline,
                        "the Close handshake must reuse the remaining shared deadline"
                    );
                }
                _ => panic!("expected SenderClose"),
            }
        }

        /// Cursor Bugbot 指摘（PR #736、Medium）の回帰テスト本体:
        /// 受信を止めたクライアント（書き込みが Pending のまま進まない）に対し、
        /// push を積んだ後に `WsSender::close` が `Ok` を返したら、セッションは
        /// close 要求の観測から `close_grace` 以内に終わること（修正前は先行 push の
        /// `ws.send` が期限なしで止まり、Close ハンドシェイクに到達しなかった）。
        /// 仮想時間（`start_paused`）で決定的に検証する。`close_before_start` が
        /// true なら、セッション開始前に close を確定させておく。
        async fn assert_close_is_bounded_for_stalled_client(close_before_start: bool) {
            const CLOSE_GRACE: Duration = Duration::from_secs(10);
            let mut config = test_config();
            config.close_grace = CLOSE_GRACE;
            let config: &'static WebSocketConfig = Box::leak(Box::new(config));
            // バッファを極小にし、クライアント側を一切読まない（書き込みが
            // Pending のまま進まない）。
            let (server_side, client_side) = tokio::io::duplex(64);
            let _client_side = client_side;
            let (tx, rx) = handler::channel(CAPACITY);
            for i in 0..3 {
                tx.send(WsMessage::Text(format!("push-{i}-{}", "x".repeat(1024))))
                    .await
                    .expect("push within capacity should succeed");
            }
            let closer = tx.clone();
            if close_before_start {
                closer
                    .close(4000, "bye")
                    .await
                    .expect("close should succeed");
            }
            let conn_ctx = test_conn_ctx(tx);
            let session_handle = tokio::spawn(async move {
                let cancel = std::future::pending::<()>();
                let mut cancel = std::pin::pin!(cancel);
                run_session_inner(
                    server_side,
                    Vec::new(),
                    config,
                    cancel.as_mut(),
                    Some(rx),
                    &conn_ctx,
                )
                .await
            });
            if !close_before_start {
                // 先頭の push の送出が止まるところまでセッションを進めてから close する。
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
                // セッションが先頭の push を 1 件だけ取り出し、その送出で止まって
                // いること（空き容量 = 容量 - 積んだ 3 件 + 取り出した 1 件）を
                // 確かめる。yield が足りず「開始前 close」のケースに化けていれば
                // ここで失敗する。
                assert_eq!(
                    closer.capacity_for_test(),
                    CAPACITY - 2,
                    "the session must be stalled sending the first push before close()"
                );
                closer
                    .close(4000, "bye")
                    .await
                    .expect("close should succeed");
            }
            let started = Instant::now();

            let (reason, result) = tokio::time::timeout(Duration::from_secs(600), session_handle)
                .await
                .expect(
                    "the session must end within close_grace after close() returned Ok, \
                     even if the client stops reading",
                )
                .expect("session task should not panic");
            assert!(
                started.elapsed() <= CLOSE_GRACE + Duration::from_millis(1),
                "the session must end within close_grace, took {:?}",
                started.elapsed()
            );
            assert!(
                matches!(reason, CloseReason::SenderClose),
                "expected SenderClose, got {reason:?}"
            );
            assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        }

        /// セッションが先頭の push の送出で止まっている最中に close が確定する場合。
        #[tokio::test(start_paused = true)]
        async fn close_during_stalled_push_is_bounded_by_close_grace() {
            assert_close_is_bounded_for_stalled_client(false).await;
        }

        /// セッション開始前に close が確定している場合（初回の送出で観測する）。
        #[tokio::test(start_paused = true)]
        async fn close_before_stalled_push_is_bounded_by_close_grace() {
            assert_close_is_bounded_for_stalled_client(true).await;
        }

        /// codex P1（PR #736）の回帰テスト: 送信キューが満杯で別タスクの
        /// `close()` が permit を待っている状態でハンドラが `WsOutcome::Reply` を
        /// 返し、継続経路の排出が先頭の項目を取り出した時点で close が確定する
        /// （Close 指示は既存の項目の後ろ、容量回の排出の外に積まれる）場合でも、
        /// Reply は送出されず、キュー済みの push → Close の順で届き、終了理由が
        /// `SenderClose` になること。close は最初の書き込み（先頭 push の送出）に
        /// 同期して確定させ、決定的に再現する。
        #[tokio::test(start_paused = true)]
        async fn close_committed_during_reply_drain_discards_reply() {
            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            let (tx, rx) = handler::channel(CAPACITY);
            let mut stream = TestStream::new(server_side);
            stream.close_on_first_write = Some(tx.clone());
            let close_result = Arc::clone(&stream.close_result);
            let handler = FillThenReturnHandler {
                outcome: || Ok(WsOutcome::Reply(vec![WsMessage::Text("reply".to_string())])),
                arm_on_return: None,
            };
            let (session_handle, mut client) =
                start_session(handler, stream, client_side, tx, rx).await;

            for i in 0..CAPACITY {
                assert_eq!(
                    next_frame(&mut client).await,
                    Some(Message::Text(format!("push-{i}").into())),
                    "pushes queued before close must arrive in order"
                );
            }
            assert_eq!(
                *close_result
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                Some(Ok(())),
                "close() must have been committed during the drain"
            );
            match next_frame(&mut client).await {
                Some(Message::Close(Some(frame))) => {
                    assert_eq!(u16::from(frame.code), 4000);
                    assert_eq!(frame.reason.as_str(), "bye");
                }
                other => panic!(
                    "the Reply must be discarded once close() is committed, expected the Close \
                     frame, got {other:?}"
                ),
            }

            let (reason, result) = tokio::time::timeout(Duration::from_secs(60), session_handle)
                .await
                .expect("session should finish within timeout")
                .expect("session task should not panic");
            assert!(
                matches!(reason, CloseReason::SenderClose),
                "expected SenderClose, got {reason:?}"
            );
            assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        }

        /// codex P1（PR #736、2 回目）の回帰テスト: 継続経路の排出
        /// （`drain_before_reply`）が close 未確定として戻った直後、Reply の送出を
        /// 始める前に別タスクの `close()` が確定した場合でも、Reply は送出されず
        /// Close 指示の処理へ進むこと。実運用では別スレッドとの競合でしか起きない
        /// 順序なので、`drain_before_reply` → `close()` → `apply_outcome` を順に
        /// 直接呼んで決定的に再現する。
        #[tokio::test]
        async fn close_committed_after_reply_drain_discards_reply() {
            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            let mut server_ws =
                WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            let mut client =
                WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
            let (tx, rx) = handler::channel(CAPACITY);
            tx.send(WsMessage::Text("push-0".to_string()))
                .await
                .expect("push should succeed");

            let mut outbound = OutboundGuard::new(Some(rx), &tx, Duration::from_secs(10));
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);

            let drained = drain_before_reply(
                &mut server_ws,
                cancel.as_mut(),
                &mut outbound,
                handler::DEFAULT_OUTBOUND_CAPACITY,
                None,
            )
            .await
            .unwrap_or_else(|_| panic!("drain_before_reply should not fail"));
            assert!(
                drained.is_none(),
                "close is not committed yet, so the drain must hand over to the Reply"
            );

            // 排出の後・Reply の送出前に close が確定する。
            tx.close(4000, "bye").await.expect("close should succeed");

            let close_deadline = Instant::now() + Duration::from_secs(2);
            let flow = apply_outcome(
                &mut server_ws,
                WsOutcome::Reply(vec![WsMessage::Text("reply".to_string())]),
                &mut outbound,
                cancel.as_mut(),
                close_deadline,
                None,
            )
            .await
            .unwrap_or_else(|_| panic!("apply_outcome should not fail"));
            match flow {
                SessionFlow::SenderClose { code, reason, .. } => {
                    assert_eq!(code, 4000);
                    assert_eq!(reason, "bye");
                }
                _ => panic!("a Reply must not be sent once close() is committed before it starts"),
            }

            assert_eq!(
                next_frame(&mut client).await,
                Some(Message::Text("push-0".into())),
                "the push queued before the Reply must still be delivered"
            );
            drop(outbound);
            drop(server_ws);
            let next = next_frame(&mut client).await;
            assert!(
                !matches!(&next, Some(Message::Text(text)) if text.as_str() == "reply"),
                "the Reply must not reach the client, got {next:?}"
            );
        }

        /// Cursor Bugbot 指摘（PR #736、High）の回帰テスト: 送信キューが満杯で
        /// クライアントが受信を止めている状態で、ハンドラが `on_message` 内から
        /// `close().await` を呼ぶと、`close()` は空きを待って確定できず、セッションは
        /// 先頭の push の送出で止まる。この場合でも、close の要求から
        /// `close_grace` 以内にセッションが終わること（修正前は確定を起点にして
        /// いたため、確定しないまま無期限に止まっていた）。仮想時間で決定的に
        /// 検証する。
        #[tokio::test(start_paused = true)]
        async fn close_requested_in_handler_with_full_queue_is_bounded() {
            const CLOSE_GRACE: Duration = Duration::from_secs(10);

            struct FillThenCloseHandler;
            impl handler::WsMessageHandler for FillThenCloseHandler {
                fn name(&self) -> &'static str {
                    "fill-then-close"
                }
                fn on_message(
                    &self,
                    _msg: WsMessage,
                ) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
                    Box::pin(async move { Ok(WsOutcome::Reply(Vec::new())) })
                }
                fn on_message_with_ctx<'a>(
                    &'a self,
                    ctx: &'a WsConnContext,
                    _msg: WsMessage,
                ) -> BoxFuture<'a, Result<WsOutcome, WsHandlerError>> {
                    Box::pin(async move {
                        for i in 0..CAPACITY {
                            ctx.sender()
                                .send(WsMessage::Text(format!("push-{i}-{}", "x".repeat(1024))))
                                .await
                                .expect("push within capacity should succeed");
                        }
                        let _ = ctx.sender().close(4000, "bye").await;
                        Ok(WsOutcome::Reply(Vec::new()))
                    })
                }
            }

            let mut config = test_config();
            config.handler = Arc::new(FillThenCloseHandler);
            config.close_grace = CLOSE_GRACE;
            let config: &'static WebSocketConfig = Box::leak(Box::new(config));
            // クライアント側は 1 フレーム送った後は一切読まない。
            let (server_side, client_side) = tokio::io::duplex(64);
            let (tx, rx) = handler::channel(CAPACITY);
            let conn_ctx = test_conn_ctx(tx);
            let session_handle = tokio::spawn(async move {
                let cancel = std::future::pending::<()>();
                let mut cancel = std::pin::pin!(cancel);
                run_session_inner(
                    server_side,
                    Vec::new(),
                    config,
                    cancel.as_mut(),
                    Some(rx),
                    &conn_ctx,
                )
                .await
            });
            let mut client =
                WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
            client
                .send(Message::Text("trigger".into()))
                .await
                .expect("client send should succeed");
            let started = Instant::now();

            let (reason, result) = tokio::time::timeout(Duration::from_secs(600), session_handle)
                .await
                .expect(
                    "the session must end within close_grace after close() was requested, \
                     even if it cannot be committed because the queue is full",
                )
                .expect("session task should not panic");
            assert!(
                started.elapsed() <= CLOSE_GRACE + Duration::from_millis(1),
                "the session must end within close_grace, took {:?}",
                started.elapsed()
            );
            assert!(
                matches!(reason, CloseReason::SenderClose),
                "expected SenderClose, got {reason:?}"
            );
            assert!(result.is_ok(), "expected Ok(()), got {result:?}");
            drop(client);
        }
    }

    /// `WebSocketConfig::with_ping_interval`（イシュー #713）の白箱テスト。
    /// クライアントとの実際のハンドシェイクを介さず `send_bounded`・
    /// `run_session` を直接駆動し、契約（モジュール doc「サーバー起点
    /// Ping keepalive」節）の各項目を固定する。end-to-end の観点は
    /// `tests/ping_keepalive_e2e.rs` が担う。
    mod keepalive_tests {
        use super::*;

        /// 「送出詰まり」: `stall_timeout` を超えてブロックした送出は
        /// `SendOutcome::Stalled` になり、`stall_timeout` 程度で有界に打ち切
        /// られること（cancel が発火しない構成でも打ち切れることを確認する）。
        #[tokio::test]
        async fn send_bounded_stalled_send_is_bounded_by_stall_timeout() {
            // duplex バッファを極小にし、書き込み自体が TCP 送信バッファ満杯
            // 相当でブロックし続ける状況を再現する（`close_send_times_out_
            // when_client_stops_reading` と同じ手法）。
            let (server_side, client_side) = tokio::io::duplex(4);
            let mut server_ws =
                WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            let _client_side = client_side;

            let (tx, _rx) = handler::channel(1);
            let mut close = CloseBound::new(tx.subscribe_close_request(), Duration::from_secs(10));
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            let stall_timeout = Duration::from_millis(150);

            // 大きめのフレームにして、極小バッファでは 1 回の `ws.send` が
            // 確実にブロックするようにする。
            let frame = Message::Binary(vec![0u8; 4096].into());

            let started = Instant::now();
            let outcome = tokio::time::timeout(
                Duration::from_secs(5),
                send_bounded(
                    &mut server_ws,
                    cancel.as_mut(),
                    &mut close,
                    frame,
                    Some(stall_timeout),
                ),
            )
            .await
            .expect("send_bounded must not hang indefinitely; stall_timeout must bound it");

            assert!(
                matches!(outcome, SendOutcome::Stalled),
                "a blocked send exceeding stall_timeout must yield Stalled"
            );
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "stall_timeout (150ms) should bound the wait, took {:?}",
                started.elapsed()
            );
        }

        /// 「送出詰まり」の end-to-end 固定: 送出詰まりが `run_session_inner` を実際に
        /// `CloseReason::PongTimeout` で終わらせること（上の
        /// `send_bounded_stalled_send_is_bounded_by_stall_timeout` は
        /// `send_bounded` 単体の白箱テストで、`run_session_inner` 側の分岐
        /// （`InboundEvent::Outbound(OutboundItem::Message(_))` の
        /// `SendOutcome::Stalled` → `pong_timeout_expired()`）はここで別に
        /// 固定する）。`interval` は十分長く取り、Ping 送出ではなく
        /// outbound push の送出詰まりであることを確定させる。
        #[tokio::test(start_paused = true)]
        async fn outbound_push_stall_yields_pong_timeout() {
            let config = WebSocketConfig {
                ping: Some(PingKeepalive {
                    interval: Duration::from_secs(10),
                    pong_timeout: Duration::from_millis(150),
                }),
                ..test_config()
            };
            let config: &'static WebSocketConfig = Box::leak(Box::new(config));

            // duplex を極小にし、push の送出自体がブロックし続ける状況を
            // 再現する。
            let (server_side, client_side) = tokio::io::duplex(4);
            let _client_side = client_side;

            let (tx, rx) = handler::channel(4);
            tx.send(WsMessage::Text("x".repeat(4096)))
                .await
                .expect("push within capacity should succeed");
            let conn_ctx = test_conn_ctx(tx);

            let (reason, result) = tokio::time::timeout(Duration::from_secs(5), async move {
                let cancel = std::future::pending::<()>();
                let mut cancel = std::pin::pin!(cancel);
                run_session_inner(
                    server_side,
                    Vec::new(),
                    config,
                    cancel.as_mut(),
                    Some(rx),
                    &conn_ctx,
                )
                .await
            })
            .await
            .expect("session must not hang; stall_timeout must bound the blocked send");

            assert!(
                matches!(reason, CloseReason::PongTimeout),
                "expected PongTimeout, got {reason:?}"
            );
            assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        }

        /// 「送出詰まり」の期限起点（送出**開始時刻**から数える）: 呼び出し前に
        /// どれだけ時間が経っていても、送出自体が `stall_timeout` 以内に
        /// 完了すれば `Stalled` にならないこと。
        #[tokio::test]
        async fn send_bounded_stall_deadline_starts_at_call_not_earlier() {
            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            let mut server_ws =
                WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            let mut client =
                WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

            let (tx, _rx) = handler::channel(1);
            let mut close = CloseBound::new(tx.subscribe_close_request(), Duration::from_secs(10));
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            let stall_timeout = Duration::from_millis(100);

            // 呼び出し前に `stall_timeout` の何倍も経過させる（「以前の期限」が
            // あったとしても、送出そのものの期限とは無関係であることを示す）。
            tokio::time::sleep(stall_timeout * 5).await;

            let outcome = tokio::time::timeout(
                Duration::from_secs(5),
                send_bounded(
                    &mut server_ws,
                    cancel.as_mut(),
                    &mut close,
                    Message::Ping(Bytes::new()),
                    Some(stall_timeout),
                ),
            )
            .await
            .expect("send_bounded should complete promptly on a healthy connection");

            assert!(
                matches!(outcome, SendOutcome::Sent),
                "a send that completes well within stall_timeout must not be Stalled"
            );

            let received = client.next().await.expect("stream should yield a message");
            assert!(matches!(received, Ok(Message::Ping(_))));
        }

        /// 「Ping 送出」: `WebSocketConfig::with_ping_interval` が送出する Ping は
        /// `idle_timeout` をリセットしない（既存の outbound push と同じ契約、
        /// `outbound_push_does_not_reset_idle_timeout` と同型）。仮想時間
        /// （`start_paused`）で駆動し、Ping が idle_timeout をリセットする
        /// バグがあれば interval（40ms）ごとに永久に延長されて終わらず、
        /// 外側の `timeout` で検知できる。
        #[tokio::test(start_paused = true)]
        async fn ping_send_does_not_reset_idle_timeout() {
            let config = WebSocketConfig {
                idle_timeout: Some(Duration::from_millis(150)),
                ping: Some(PingKeepalive {
                    interval: Duration::from_millis(40),
                    pong_timeout: Duration::from_secs(10),
                }),
                ..test_config()
            };
            let config: &'static WebSocketConfig = Box::leak(Box::new(config));

            let (server_side, client_side) = tokio::io::duplex(1 << 16);
            // クライアントは接続を保持するが読み出さない（Ping は書き込み
            // バッファに積まれるだけで idle_timeout に影響してはならない）。
            let _client_side = client_side;

            let (tx, rx) = handler::channel(4);
            let conn_ctx = test_conn_ctx(tx);

            let started = tokio::time::Instant::now();
            let (reason, result) = tokio::time::timeout(Duration::from_secs(5), async move {
                let cancel = std::future::pending::<()>();
                let mut cancel = std::pin::pin!(cancel);
                run_session_inner(
                    server_side,
                    Vec::new(),
                    config,
                    cancel.as_mut(),
                    Some(rx),
                    &conn_ctx,
                )
                .await
            })
            .await
            .expect("session must not hang (ping send must not keep extending idle_timeout)");

            assert!(
                matches!(reason, CloseReason::IdleTimeout),
                "expected IdleTimeout (ping send must not reset it), got {reason:?}"
            );
            assert!(result.is_ok(), "expected Ok(()), got {result:?}");
            // Pong を返さないため Ping は t=40ms の 1 回しか送られない。Ping 送出で
            // idle 期限がリセットされるバグがあると idle 期限は 190ms へずれるので、
            // 終了時刻（idle 期限 + Close 応答待ちの close_grace）で検出する。
            let elapsed = started.elapsed();
            let expected = Duration::from_millis(150) + config.close_grace;
            assert!(
                elapsed >= Duration::from_millis(150)
                    && elapsed < expected + Duration::from_millis(40),
                "session must end at idle_timeout (+ close_grace), not later: elapsed {elapsed:?}"
            );
        }
    }
}
