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

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::Utf8Bytes;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig as TungsteniteConfig};

use futures_util::{SinkExt, StreamExt};

use crate::config::WebSocketConfig;
use crate::error::WsError;
use crate::handler::{
    CloseReason, DEFAULT_OUTBOUND_CAPACITY, FailureKind, WsConnContext, WsHandlerError, WsMessage,
    WsOutcome,
};
use crate::race_cancel;

/// 101 応答送出済みのストリームを受け取り、WebSocket セッション終了まで
/// 処理する（既存の公開シグネチャを保つ薄いラッパー、イシュー #726）。
///
/// 本体は [`run_session_inner`] に移した。本関数はその戻り値
/// （`(CloseReason, Result<(), WsError>)`）から `CloseReason` を**破棄**し、
/// 従来どおり `Result<(), WsError>` のみを返す（呼び出し元
/// `crate::handle_upgrade` および既存の `#[cfg(test)]` テストは無変更で
/// 動作する）。切断通知 API（`on_close(conn_ctx, reason)` の呼び出し）は
/// 後続イシュー #729 が本ラッパーへ追加する予定（設計は
/// `docs/design/ws-connection-context-and-close.md` 9 節）。
pub(crate) async fn run_session<S, C>(
    stream: S,
    leftover: Vec<u8>,
    config: &WebSocketConfig,
    cancel: Pin<&mut C>,
    outbound: Option<mpsc::Receiver<WsMessage>>,
    conn_ctx: &WsConnContext,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    run_session_inner(stream, leftover, config, cancel, outbound, conn_ctx)
        .await
        .1
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
/// [`handle_idle_timeout`] を呼ぶ**前**に `outbound` を drop し、満杯
/// チャネルでブロック中の [`crate::handler::WsSender::send`] 呼び出しを
/// `close_grace` の満了を待たず即座に解放する。
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
async fn run_session_inner<S, C>(
    stream: S,
    leftover: Vec<u8>,
    config: &WebSocketConfig,
    mut cancel: Pin<&mut C>,
    mut outbound: Option<mpsc::Receiver<WsMessage>>,
    conn_ctx: &WsConnContext,
) -> (CloseReason, Result<(), WsError>)
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    let ws_config = TungsteniteConfig::default()
        .max_message_size(Some(config.max_message_size))
        .max_frame_size(Some(config.max_frame_size));

    let mut ws: WebSocketStream<S> =
        WebSocketStream::from_partially_read(stream, leftover, Role::Server, Some(ws_config)).await;

    // アイドル期限は「クライアントから実際にフレームを受信したとき」にのみ
    // 更新する（モジュール doc を参照。outbound push ではリセットしない）。
    let mut idle_deadline: Option<Instant> = config.idle_timeout.map(|d| Instant::now() + d);

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
        // クライアント受信（+ アイドル期限）を 1 つの Future にまとめる。
        // 新規 `ws.next()` / `sleep_until()` を毎ループ作り直す既存パターン
        // （drop による打ち切りは `ws` 自体の状態に影響しない）を踏襲する。
        let inbound = async {
            match idle_deadline {
                Some(deadline) => {
                    match race2(ws.next(), tokio::time::sleep_until(deadline)).await {
                        Either::Left(message) => InboundEvent::Message(message),
                        Either::Right(()) => InboundEvent::Idle,
                    }
                }
                None => InboundEvent::Message(ws.next().await),
            }
        };

        let event = if let Some(rx) = outbound.as_mut() {
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
                    drop(outbound.take());
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
                    drop(outbound.take());
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
            InboundEvent::Idle => {
                drop(outbound.take());
                return (
                    CloseReason::IdleTimeout,
                    handle_idle_timeout(ws, config.close_grace).await,
                );
            }
            InboundEvent::Outbound(msg) => {
                let frame = to_tungstenite_message(msg);
                match race_cancel(cancel.as_mut(), ws.send(frame)).await {
                    None => {
                        drop(outbound.take());
                        return (
                            CloseReason::Cancelled,
                            handle_cancellation(ws, config.close_grace).await,
                        );
                    }
                    Some(Ok(())) => {}
                    Some(Err(err)) => return SessionFailure::send(err).into_parts(),
                }
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
                        )
                        .await
                        {
                            Ok(SessionFlow::Continue) => {}
                            Ok(SessionFlow::Closed) => break CloseReason::HandlerClose,
                            Ok(SessionFlow::Cancelled) => {
                                drop(outbound.take());
                                return (
                                    CloseReason::Cancelled,
                                    handle_cancellation(ws, config.close_grace).await,
                                );
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
                        )
                        .await
                        {
                            Ok(SessionFlow::Continue) => {}
                            Ok(SessionFlow::Closed) => break CloseReason::HandlerClose,
                            Ok(SessionFlow::Cancelled) => {
                                drop(outbound.take());
                                return (
                                    CloseReason::Cancelled,
                                    handle_cancellation(ws, config.close_grace).await,
                                );
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
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
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

/// クライアント受信待ちの 1 イベント（[`run_session`] のループが処理する
/// 単位）。`Idle` はアイドルタイムアウト発火、`Outbound` はサーバー起点
/// メッセージ（[`crate::handler::WsSender`]、イシュー #670）到着を表す。
enum InboundEvent {
    /// `ws.next()` の結果（`None` は EOF、`Some(Err(_))` はプロトコル/IO
    /// エラー）。
    Message(Option<Result<Message, tokio_tungstenite::tungstenite::Error>>),
    /// `idle_deadline` に到達した。
    Idle,
    /// [`crate::handler::WsSender`] からの push メッセージ。
    Outbound(WsMessage),
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
    /// （`WsOutcome::Close`）。
    Closed,
    /// 送出中にキャンセルが発火し、当該 `Future` を打ち切った。呼び出し元は
    /// [`handle_cancellation`] へ分岐する。
    Cancelled,
}

/// [`crate::handler::WsMessageHandler::on_message`] の戻り値をセッション
/// ループへ反映する。`WsOutcome::Reply` の各 `ws.send` / `WsOutcome::Close`
/// の `ws.close` を `cancel` と race させ（イシュー #499）、キャンセルが
/// 送出中に発火した場合は当該 `Future` を drop して
/// [`SessionFlow::Cancelled`] を返す。`ws` は呼び出し元が引き続き所有する
/// ため、打ち切り後も `WebSocketStream` 内部のフレーミングバッファ状態
/// （書き込み位置）は保たれ、後続の Close 送出が破損したバイト列を生まない
/// （モジュール doc の「ワイヤ安全性」節を参照）。
async fn apply_outcome<S, C>(
    ws: &mut WebSocketStream<S>,
    outcome: WsOutcome,
    mut cancel: Pin<&mut C>,
) -> Result<SessionFlow, SessionFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    match outcome {
        WsOutcome::Reply(messages) => {
            for msg in messages {
                let frame = to_tungstenite_message(msg);
                match race_cancel(cancel.as_mut(), ws.send(frame)).await {
                    None => return Ok(SessionFlow::Cancelled),
                    Some(Ok(())) => {}
                    Some(Err(err)) => return Err(SessionFailure::send(err)),
                }
            }
            Ok(SessionFlow::Continue)
        }
        WsOutcome::Close => {
            match race_cancel(cancel.as_mut(), ws.close(None)).await {
                None => return Ok(SessionFlow::Cancelled),
                Some(Ok(())) => {}
                Some(Err(err)) => return Err(SessionFailure::send(err)),
            }
            Ok(SessionFlow::Closed)
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
/// 消化する者（本関数自身）がハンドラ完了まで戻ってこず、容量
/// （[`DEFAULT_OUTBOUND_CAPACITY`]、既定 8）を超えると送信側・受信側の両方が
/// 進めなくなっていた。本関数はハンドラ Future と outbound 到着を
/// `race2`（cancel を最優先とした 3 者 race）し、到着ごとに即座に `ws.send()`
/// で送出することでこれを解消する。
///
/// # 手順・保証（設計 6 節）
///
/// 1. ハンドラ Future が `Poll::Ready` を返すまで、cancel（最優先）→
///    (ハンドラ完了 | outbound 到着) を反復ポーリングする。到着した
///    outbound push はその都度 `ws.send()` で送出する（ハンドラ Future は
///    1 回しか完了しない単発イベントのため `race2_alternating` 型の交互化
///    は不要）。
/// 2. ハンドラが `Err` を返した場合: 排出・送信を一切行わず、その場で
///    `Err` を返す（既存の `outcome?` と同一の即時終了契約）。
/// 3. `Ok(outcome)` の場合: `try_recv()` を [`DEFAULT_OUTBOUND_CAPACITY`]
///    回まで（`Empty` に達するまで）繰り返し、追加で溜まっていた push を
///    到着順に送出してから [`apply_outcome`] へ委譲する。
///
/// **保証**: 排出ステップ（3.）の開始時点で既にチャネルへ格納済みだった
/// push は、そのハンドラが返す `WsOutcome::Reply`/`Close` の送出より必ず
/// 先に送出される。それ以外（排出開始後に格納された push・送出途中だった
/// push）との相対順序は不定とする（設計 6 節「保証」を参照。対象外の順序を
/// 新たに固定しない）。
///
/// outbound 到着時の `ws.send()` 失敗・cancel 発火時の扱いは
/// [`run_session`] 外側ループの `InboundEvent::Outbound` 分岐と同一
/// （送信失敗は `WsError` へ変換して終了、cancel 発火は
/// [`SessionFlow::Cancelled`] を返す）。`idle_deadline` は本関数の実行中は
/// 更新しない（モジュール doc の「クライアントから実際にフレームを受信した
/// 場合にのみ延長」契約を変えない）。
async fn run_handler_with_outbound_drain<S, C>(
    ws: &mut WebSocketStream<S>,
    mut cancel: Pin<&mut C>,
    outbound: &mut Option<mpsc::Receiver<WsMessage>>,
    handler_fut: futures_util::future::BoxFuture<'_, Result<WsOutcome, WsHandlerError>>,
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
        let progress = match outbound.as_mut() {
            Some(rx) => race_cancel(cancel.as_mut(), race2(&mut handler_fut, rx.recv())).await,
            None => {
                race_cancel(
                    cancel.as_mut(),
                    race2(
                        &mut handler_fut,
                        std::future::pending::<Option<WsMessage>>(),
                    ),
                )
                .await
            }
        };
        match progress {
            None => return Ok(SessionFlow::Cancelled),
            Some(Either::Left(handler_result)) => break handler_result,
            Some(Either::Right(Some(msg))) => {
                let frame = to_tungstenite_message(msg);
                match race_cancel(cancel.as_mut(), ws.send(frame)).await {
                    None => return Ok(SessionFlow::Cancelled),
                    Some(Ok(())) => {}
                    Some(Err(err)) => return Err(SessionFailure::send(err)),
                }
            }
            Some(Either::Right(None)) => {
                // 全 `WsSender` クローンが drop 済み（`run_session_inner` 外側
                // ループの同種分岐と同じ防御的コード。`conn_ctx` がクローンを
                // 保持し続けるためセッション実行中は到達不能）。
                *outbound = None;
            }
        }
    };

    // ステップ 2: ハンドラエラーは排出・送信を行わず即時終了する
    // （[`FailureKind::Handler`] へ分類、`WsError::Handler` を保持）。
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(err) => return Err(SessionFailure::handler(err)),
    };

    // ステップ 3: 排出開始時点で既に格納済みだった push を、
    // `DEFAULT_OUTBOUND_CAPACITY` 回（既定 8）まで `try_recv()` で取り出し、
    // 到着順に送出する。`Receiver::len()` は使わない（bounded mpsc の
    // 実装依存の同期精度に左右されず、呼び出し回数上限で足りるため。
    // `handler.rs` の該当コメント・設計 6 節ステップ 3 を参照）。
    if let Some(rx) = outbound.as_mut() {
        for _ in 0..DEFAULT_OUTBOUND_CAPACITY {
            match rx.try_recv() {
                Ok(msg) => {
                    let frame = to_tungstenite_message(msg);
                    match race_cancel(cancel.as_mut(), ws.send(frame)).await {
                        None => return Ok(SessionFlow::Cancelled),
                        Some(Ok(())) => {}
                        Some(Err(err)) => return Err(SessionFailure::send(err)),
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    *outbound = None;
                    break;
                }
            }
        }
    }

    apply_outcome(ws, outcome, cancel).await
}

/// アイドルタイムアウト発火時の切断シーケンス（正常な Close ハンドシェイク、
/// close code 1000 Normal Closure）。[`close_and_drain`] へ委譲する
/// （呼び出し元 `run_session` の唯一の呼び出し箇所）。
async fn handle_idle_timeout<S>(
    ws: WebSocketStream<S>,
    close_grace: Duration,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    close_and_drain(ws, None, close_grace).await
}

/// キャンセル `Future`（`crate::handle_upgrade` 経由でコアの世代キャンセル
/// シグナルへ接続、イシュー #492）発火時の切断シーケンス。
///
/// `handle_idle_timeout` と同型だが、close code は 1001 Going Away
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
    close_and_drain(ws, Some(close_frame), close_grace).await
}

/// Close フレーム送出 → クライアント応答（または EOF・エラー）のドレインを
/// `close_grace`（`WebSocketConfig::close_grace`、既定 10 秒）で有界化する
/// 共通ヘルパー（[`handle_idle_timeout`] / [`handle_cancellation`] で共有）。
///
/// Close 送出自体が失敗した場合（相手が既に切断済み等）も、切断そのものの
/// 目的は達成されているため、ドレインへ進まず正常終了として扱う。Close
/// 応答を返さないクライアントに接続を無期限保持させないため、送出 →
/// ドレインの全体を `close_grace` で区切る（二次 DoS 対策、Issue #175・
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
    close_grace: Duration,
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

    match tokio::time::timeout(close_grace, sequence).await {
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
        assert_eq!(first, WsMessage::Text("first".to_string()));

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
}
