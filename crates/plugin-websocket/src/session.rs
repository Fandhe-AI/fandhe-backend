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
//! アイドル期限) → outbound（サーバー起点 push）の優先順で 1 イベントを
//! 選び、outbound メッセージは既存の `ws.send()`（[`apply_outcome`] の
//! `WsOutcome::Reply` 送出と同一の `&mut WebSocketStream`）へ直列に送出
//! する（フレームが混ざらないことを構造的に保証する。単一タスクが `ws`
//! を排他的に所有するため）。`config.idle_timeout` は**クライアントから
//! 実際にフレームを受信した場合にのみ**更新し、outbound 送出はタイマーを
//! リセットしない（無通信のデッドクライアントへ定期 push し続けると
//! アイドルタイムアウトが永久に発火しなくなる退行を避けるため。Issue
//! #175 が導入した DoS 対策を後退させない）。更新タイミングはフレーム
//! 受信直後ではなく、ハンドラ実行（`on_message`）・返信送出
//! （`apply_outcome`）まで完了し次の受信待ちに入る直前とする（受信直後に
//! 更新すると、ハンドラ処理・返信送出に `idle_timeout` 相当の時間を要した
//! 場合にその処理時間がアイドル待機時間へ算入され、処理完了直後の次の
//! 受信待ちで即座に期限切れとなりうるため。レビュー指摘対応、既存の
//! 「各 `ws.next()` の待機を開始する直前に毎回タイムアウトを設定する」
//! 契約を回復する）。本 PR（#670）の時点では
//! `handle_upgrade` は常に outbound を `None` で渡すため、この合流経路は
//! 内部配線のみで外部から到達しない（ハンドラへの公開は #671 のスコープ）。
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
use crate::handler::{WsMessage, WsOutcome};
use crate::race_cancel;

/// 101 応答送出済みのストリームを受け取り、WebSocket セッション終了まで
/// 処理する。
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
/// Close 応答（または EOF）をドレインしてから `Ok(())` で終了する（ポリシー駆動の正常終了。
/// プロトコル違反ではないため `WsError` の新規 variant は追加しない）。
/// `idle_timeout` が `None`（`without_idle_timeout` による明示的無効化）の
/// 場合は従来どおり無期限に受信を待つ。
///
/// # サイズ上限とハンドラ呼び出し順序（DoS 対策の維持、Issue #179 セキュリティ考慮）
///
/// `max_message_size` / `max_frame_size` は tungstenite 側で強制されるため、
/// 上限超過メッセージはハンドラへ届く前にプロトコルエラーとして拒否される
/// （`ws.next()` が `Err` を返す）。ハンドラ呼び出し前のサイズ検証という
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
/// 合流させて 1 イベントずつ処理する（モジュール doc を参照）。全
/// `WsSender` クローンが drop されチャネルが閉じた場合はそのイベント源を
/// 無効化するのみでセッション自体は継続する（`None` にはしない設計だと
/// 毎回 `recv()` を呼び続けビジーループ化しうるため、内部で
/// `outbound = None` 相当に切り替えて以後は選択しないようにする）。
/// cancel 発火時・アイドルタイムアウト発火時は、[`handle_cancellation`] /
/// [`handle_idle_timeout`] を呼ぶ**前**に `outbound` を drop し、満杯
/// チャネルでブロック中の [`crate::handler::WsSender::send`] 呼び出しを
/// `close_grace` の満了を待たず即座に解放する。
pub(crate) async fn run_session<S, C>(
    stream: S,
    leftover: Vec<u8>,
    config: &WebSocketConfig,
    mut cancel: Pin<&mut C>,
    mut outbound: Option<mpsc::Receiver<WsMessage>>,
) -> Result<(), WsError>
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

    loop {
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
            match race_cancel(cancel.as_mut(), race2(inbound, rx.recv())).await {
                None => {
                    drop(outbound.take());
                    return handle_cancellation(ws, config.close_grace).await;
                }
                Some(Either::Left(inbound_event)) => inbound_event,
                Some(Either::Right(Some(msg))) => InboundEvent::Outbound(msg),
                Some(Either::Right(None)) => {
                    // 全 WsSender クローンが drop 済み。以後このイベント源を
                    // 選択しないよう無効化し、セッション自体は継続する
                    // （ビジーループ化を防ぐ）。
                    drop(outbound.take());
                    continue;
                }
            }
        } else {
            match race_cancel(cancel.as_mut(), inbound).await {
                None => return handle_cancellation(ws, config.close_grace).await,
                Some(inbound_event) => inbound_event,
            }
        };

        match event {
            InboundEvent::Idle => {
                drop(outbound.take());
                return handle_idle_timeout(ws, config.close_grace).await;
            }
            InboundEvent::Outbound(msg) => {
                let frame = to_tungstenite_message(msg);
                match race_cancel(cancel.as_mut(), ws.send(frame)).await {
                    None => {
                        drop(outbound.take());
                        return handle_cancellation(ws, config.close_grace).await;
                    }
                    Some(Ok(())) => {}
                    Some(Err(err)) => return Err(err.into()),
                }
            }
            InboundEvent::Message(None) => break,
            InboundEvent::Message(Some(message)) => {
                let message = message?;
                match message {
                    Message::Text(text) => {
                        let Some(outcome) = race_cancel(
                            cancel.as_mut(),
                            config
                                .handler
                                .on_message(WsMessage::Text(text.as_str().to_owned())),
                        )
                        .await
                        else {
                            drop(outbound.take());
                            return handle_cancellation(ws, config.close_grace).await;
                        };
                        match apply_outcome(&mut ws, outcome?, cancel.as_mut()).await? {
                            SessionFlow::Continue => {}
                            SessionFlow::Closed => break,
                            SessionFlow::Cancelled => {
                                drop(outbound.take());
                                return handle_cancellation(ws, config.close_grace).await;
                            }
                        }
                    }
                    Message::Binary(bin) => {
                        let Some(outcome) = race_cancel(
                            cancel.as_mut(),
                            config.handler.on_message(WsMessage::Binary(bin.into())),
                        )
                        .await
                        else {
                            drop(outbound.take());
                            return handle_cancellation(ws, config.close_grace).await;
                        };
                        match apply_outcome(&mut ws, outcome?, cancel.as_mut()).await? {
                            SessionFlow::Continue => {}
                            SessionFlow::Closed => break,
                            SessionFlow::Cancelled => {
                                drop(outbound.take());
                                return handle_cancellation(ws, config.close_grace).await;
                            }
                        }
                    }
                    Message::Close(_) => {
                        break;
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
    }

    Ok(())
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

/// [`WsMessage`] を tungstenite の `Message` へ変換する（[`apply_outcome`]
/// の `WsOutcome::Reply` 送出・[`run_session`] の outbound 送出の両方で
/// 共有する）。
fn to_tungstenite_message(msg: WsMessage) -> Message {
    match msg {
        WsMessage::Text(t) => Message::Text(t.into()),
        WsMessage::Binary(b) => Message::Binary(b.into()),
    }
}

/// [`apply_outcome`] の戻り値。セッションループ（[`run_session`]）が次に
/// 取るべき動作を表す（イシュー #499 で `Result<bool, WsError>` から
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
) -> Result<SessionFlow, WsError>
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
                    Some(result) => result?,
                }
            }
            Ok(SessionFlow::Continue)
        }
        WsOutcome::Close => {
            match race_cancel(cancel.as_mut(), ws.close(None)).await {
                None => return Ok(SessionFlow::Cancelled),
                Some(result) => result?,
            }
            Ok(SessionFlow::Closed)
        }
    }
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
/// `run_session` は `pub(crate)` であり、`WsSender` をユーザーハンドラへ
/// 渡す公開経路が存在しない（#671 まで）ため、統合テスト（`tests/*.rs`）
/// からは到達できない。そのためクレート内単体テストとして追加する
/// （`.claude/rules/feature-modification.md` の「実装変更には同一クレートの
/// テスト追加を伴わせる」を `#[cfg(test)]` で満たす）。
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

    /// 受け入れ基準 1: `WsSender` からの push とクライアント宛の返信
    /// （`EchoHandler`）が同一 `WebSocketStream` 上で混ざらず、両方とも
    /// 欠落なく届くこと。
    #[tokio::test]
    async fn outbound_messages_interleave_safely_with_client_replies() {
        let config: &'static WebSocketConfig = Box::leak(Box::new(test_config()));
        let (server_side, client_side) = tokio::io::duplex(8192);
        let (tx, rx) = handler::channel(4);

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session(server_side, Vec::new(), config, cancel.as_mut(), Some(rx)).await
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

        // 既に発火済みのキャンセルを渡す（`race_cancel` は cancel を最優先で
        // ポーリングするため、outbound にキュー済みメッセージがあっても
        // 送出よりキャンセル分岐が優先される）。
        let session_handle = tokio::spawn(async move {
            let cancel = std::future::ready(());
            let mut cancel = std::pin::pin!(cancel);
            run_session(server_side, Vec::new(), config, cancel.as_mut(), Some(rx)).await
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

        let session_handle = tokio::spawn(async move {
            let cancel = std::future::pending::<()>();
            let mut cancel = std::pin::pin!(cancel);
            run_session(server_side, Vec::new(), config, cancel.as_mut(), Some(rx)).await
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
}
