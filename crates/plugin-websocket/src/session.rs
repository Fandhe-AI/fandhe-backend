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
//! キャンセルシグナル、イシュー #492）も各受信待ちで最優先ポーリングし、
//! 発火時はアイドルタイムアウトと同型の正常な Close ハンドシェイク
//! （close code 1001 Going Away）で切断する（[`handle_cancellation`] の
//! doc を参照。既存のユーザーハンドラ `await` 中・`WsOutcome::Reply` 送出
//! 中はキャンセルを観測せず、次の受信待ちへ復帰した時点で反映される
//! 既知の制約がある。停滞時の最終フェイルセーフは `run_until` の permit
//! 回収 timeout が既存どおり担保する）。

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
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
/// （イシュー #492）。各受信待ちで最優先ポーリングし、発火時は
/// [`handle_cancellation`] へ分岐する（優先順位はアイドルタイムアウトより
/// 高い。TOCTOU 回避の詳細は `crate::race_cancel` の doc を参照）。
pub(crate) async fn run_session<S, C>(
    stream: S,
    leftover: Vec<u8>,
    config: &WebSocketConfig,
    mut cancel: Pin<&mut C>,
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

    loop {
        let message = match config.idle_timeout {
            Some(idle_timeout) => {
                match race_cancel(
                    cancel.as_mut(),
                    tokio::time::timeout(idle_timeout, ws.next()),
                )
                .await
                {
                    None => return handle_cancellation(ws, config.close_grace).await,
                    Some(Ok(message)) => message,
                    Some(Err(_elapsed)) => {
                        return handle_idle_timeout(ws, config.close_grace).await;
                    }
                }
            }
            None => match race_cancel(cancel.as_mut(), ws.next()).await {
                None => return handle_cancellation(ws, config.close_grace).await,
                Some(message) => message,
            },
        };

        let Some(message) = message else {
            break;
        };
        let message = message?;
        match message {
            Message::Text(text) => {
                let outcome = config
                    .handler
                    .on_message(WsMessage::Text(text.as_str().to_owned()))
                    .await?;
                if apply_outcome(&mut ws, outcome).await? {
                    break;
                }
            }
            Message::Binary(bin) => {
                let outcome = config
                    .handler
                    .on_message(WsMessage::Binary(bin.into()))
                    .await?;
                if apply_outcome(&mut ws, outcome).await? {
                    break;
                }
            }
            Message::Close(_) => {
                break;
            }
            // Ping/Pong は tungstenite が内部で自動応答するため、Stream 経由で
            // ここへ届くのは診断用の可視化のみ。ハンドラには委譲しない
            // （ハンドラ契約は Text/Binary のみを扱う、`handler` モジュール
            // の doc を参照）。
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }

    Ok(())
}

/// [`crate::handler::WsMessageHandler::on_message`] の戻り値をセッション
/// ループへ反映する。`Ok(true)` はセッション終了（`WsOutcome::Close`）を
/// 意味し、呼び出し元はループを抜ける。
async fn apply_outcome<S>(ws: &mut WebSocketStream<S>, outcome: WsOutcome) -> Result<bool, WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match outcome {
        WsOutcome::Reply(messages) => {
            for msg in messages {
                let frame = match msg {
                    WsMessage::Text(t) => Message::Text(t.into()),
                    WsMessage::Binary(b) => Message::Binary(b.into()),
                };
                ws.send(frame).await?;
            }
            Ok(false)
        }
        WsOutcome::Close => {
            ws.close(None).await?;
            Ok(true)
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
            return match err {
                tokio_tungstenite::tungstenite::Error::ConnectionClosed
                | tokio_tungstenite::tungstenite::Error::AlreadyClosed => Ok(()),
                other => Err(other),
            };
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
