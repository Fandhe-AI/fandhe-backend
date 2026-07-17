//! ハンドシェイク成立後のフレーミング処理（tokio-tungstenite への委譲）。
//!
//! `crate::handle_upgrade` から呼ばれる。101 応答送出直後の生ストリームを
//! `WebSocketStream::from_partially_read` へ渡し、以降の RFC 6455 フレーミング
//! （マスク処理・Ping/Pong 自動応答・Close ハンドシェイク）は tokio-tungstenite
//! に委ねる。TASK-4.1 時点のセッション本体はエコーループ（Text/Binary を
//! そのまま返送）に限定し、ユーザー定義メッセージハンドラ API は導入しない
//! （後続 Issue のスコープ、Issue #22 実装計画 8 節）。

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig as TungsteniteConfig};

use futures_util::{SinkExt, StreamExt};

use crate::config::WebSocketConfig;
use crate::error::WsError;

/// 101 応答送出済みのストリームを受け取り、WebSocket セッション終了まで
/// 処理する（エコーループ）。
///
/// `leftover` は 101 応答送出前にクライアントから先行到着していた可能性の
/// ある残余バイト列（コア側 `RecvBuffer::unread` 由来）。
/// `WebSocketStream::from_partially_read` へそのまま渡すことで、先行フレーム
/// を取りこぼさない。
///
/// Text/Binary メッセージはそのまま送り返し、Ping には tokio-tungstenite が
/// 自動で Pong を返す（tungstenite の既定動作）。Close フレーム受信、または
/// I/O エラー・プロトコルエラーでループを終える。エラーは呼び出し元
/// （`crate::handle_upgrade`）へ伝播し、コア境界（`crates/core`）を越えて
/// panic させない。
pub(crate) async fn run_echo_session<S>(
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

    while let Some(message) = ws.next().await {
        let message = message?;
        match message {
            Message::Text(_) | Message::Binary(_) => {
                ws.send(message).await?;
            }
            Message::Close(_) => {
                break;
            }
            // Ping/Pong は tungstenite が内部で自動応答するため、Stream 経由で
            // ここへ届くのは診断用の可視化のみ。エコーループとしては無視する。
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }

    Ok(())
}
