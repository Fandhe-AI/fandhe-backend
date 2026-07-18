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

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig as TungsteniteConfig};

use futures_util::{SinkExt, StreamExt};

use crate::config::WebSocketConfig;
use crate::error::WsError;
use crate::handler::{WsMessage, WsOutcome};

/// タイムアウト発火後、サーバ側が送出した Close フレームへのクライアント
/// 応答（または EOF）を待つ上限（10 秒）。
///
/// Close 応答を返さない（無視する）クライアントが「クローズ送出済みだが
/// 応答待ち」の状態で接続を無期限に保持し続けると、アイドルタイムアウト
/// そのものが二次的な DoS の抜け道になる。固定の猶予で必ず接続を終端させ
/// fail-closed にする（Issue #175）。
const CLOSE_GRACE: Duration = Duration::from_secs(10);

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
/// 送出したうえで、[`CLOSE_GRACE`] を上限にクライアントの Close 応答（または
/// EOF）をドレインしてから `Ok(())` で終了する（ポリシー駆動の正常終了。
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
pub(crate) async fn run_session<S>(
    stream: S,
    leftover: Vec<u8>,
    config: &WebSocketConfig,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let ws_config = TungsteniteConfig::default()
        .max_message_size(Some(config.max_message_size))
        .max_frame_size(Some(config.max_frame_size));

    let mut ws: WebSocketStream<S> =
        WebSocketStream::from_partially_read(stream, leftover, Role::Server, Some(ws_config)).await;

    loop {
        let message = match config.idle_timeout {
            Some(idle_timeout) => match tokio::time::timeout(idle_timeout, ws.next()).await {
                Ok(message) => message,
                Err(_elapsed) => return handle_idle_timeout(ws).await,
            },
            None => ws.next().await,
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

/// アイドルタイムアウト発火時の切断シーケンス（正常な Close ハンドシェイク）。
///
/// サーバ側から Close フレームを送出し、[`CLOSE_GRACE`] を上限にクライアント
/// からの Close 応答（または EOF・エラー）をドレインする。相手が既に
/// 切断済みのケース（`ConnectionClosed` / `AlreadyClosed`）や `CLOSE_GRACE`
/// 超過は、アイドル切断そのものは意図どおり完了しているため異常とは扱わず
/// `Ok(())` を返す（呼び出し元 `run_session` の唯一の呼び出し箇所）。
async fn handle_idle_timeout<S>(mut ws: WebSocketStream<S>) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Close 送出自体が失敗した場合（相手が既に切断済み等）も、アイドル切断の
    // 目的は達成されているため、ドレインへ進まず正常終了として扱う。
    if let Err(err) = ws.close(None).await {
        return match err {
            tokio_tungstenite::tungstenite::Error::ConnectionClosed
            | tokio_tungstenite::tungstenite::Error::AlreadyClosed => Ok(()),
            other => Err(other.into()),
        };
    }

    let drain = async {
        loop {
            match ws.next().await {
                // Close 応答（または相手からの追加フレーム）を消費し続け、
                // EOF（`None`）でドレイン完了とする。
                Some(Ok(_)) => continue,
                Some(Err(
                    tokio_tungstenite::tungstenite::Error::ConnectionClosed
                    | tokio_tungstenite::tungstenite::Error::AlreadyClosed,
                ))
                | None => break,
                Some(Err(other)) => return Err(other),
            }
        }
        Ok(())
    };

    // Close 応答を返さないクライアントに接続を無期限保持させない
    // （[`CLOSE_GRACE`] の doc を参照、二次 DoS 対策）。
    match tokio::time::timeout(CLOSE_GRACE, drain).await {
        Ok(Ok(())) => Ok(()),
        Err(_timeout_elapsed) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
    }
}
