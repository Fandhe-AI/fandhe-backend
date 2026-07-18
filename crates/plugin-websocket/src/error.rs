//! [`crate::handle_upgrade`] が返すエラー型。
//!
//! コア（`crates/core/src/plugin.rs` の Upgrade シーム）は本エラーを panic に
//! 変換せず、接続を静かにクローズする契約（コア境界を越えて panic させない、
//! `.claude/rules/coding-rust.md`）。エラー診断名にリクエスト内容（ヘッダ値・
//! ボディ）を含めない（ログ・秘密情報の混入防止、`.claude/rules/security.md`）。

use std::fmt;

/// WebSocket ハンドシェイク・セッション処理で発生し得るエラー。
#[derive(Debug)]
pub enum WsError {
    /// ハンドシェイクリクエストが RFC 6455 4.2.1 の要件を満たさない
    /// （`400 Bad Request` を返して接続を閉じる）。
    InvalidHandshake(&'static str),
    /// `Sec-WebSocket-Version` が `13` 以外
    /// （`426 Upgrade Required` を返して接続を閉じる）。
    UnsupportedVersion,
    /// 101 応答・以降のフレーミング送受信で I/O エラーが発生した。
    Io(std::io::Error),
    /// tokio-tungstenite 側のプロトコルエラー（不正フレーム等）。
    Protocol(tokio_tungstenite::tungstenite::Error),
    /// ユーザー定義 `WsMessageHandler::on_message` がエラーを返した
    /// （Issue #179）。呼び出し元は接続をクローズ扱いにする（panic に
    /// 変換しない契約は本 variant にも適用される）。
    Handler(crate::handler::WsHandlerError),
}

impl fmt::Display for WsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WsError::InvalidHandshake(reason) => {
                write!(f, "invalid websocket handshake: {reason}")
            }
            WsError::UnsupportedVersion => write!(f, "unsupported websocket version"),
            WsError::Io(_) => write!(f, "websocket io error"),
            WsError::Protocol(_) => write!(f, "websocket protocol error"),
            WsError::Handler(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for WsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WsError::Io(e) => Some(e),
            WsError::Protocol(e) => Some(e),
            WsError::Handler(e) => Some(e),
            WsError::InvalidHandshake(_) | WsError::UnsupportedVersion => None,
        }
    }
}

impl From<std::io::Error> for WsError {
    fn from(e: std::io::Error) -> Self {
        WsError::Io(e)
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for WsError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        WsError::Protocol(e)
    }
}

impl From<crate::handler::WsHandlerError> for WsError {
    fn from(e: crate::handler::WsHandlerError) -> Self {
        WsError::Handler(e)
    }
}
