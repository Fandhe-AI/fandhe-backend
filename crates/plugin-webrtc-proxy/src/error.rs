//! シグナリングプロキシのエラー型（[`ProxyError`]）。
//!
//! [`crate::client::forward_offer`] が返し、[`crate::handler::try_handle_rtc_offer`]
//! が HTTP ステータスへマッピングする。`thiserror` 等は依存最小化のため使わず
//! `crates/http` の既存エラー型（`crates/http/src/connection.rs` の
//! `RequestError`）と同様に手実装する（.claude/rules/coding-rust.md）。
//!
//! 各バリアントは上流の内部情報（アドレス・生エラーメッセージ）をクライアント
//! 応答に含めないための最小限の分類にとどめる（.claude/rules/security.md の
//! エラー情報漏えい対策）。

/// 上流 WebRTC サービスとの中継で発生しうるエラー。
#[derive(Debug)]
pub enum ProxyError {
    /// 上流への TCP 接続確立に失敗した（接続拒否・名前解決失敗等）。
    UpstreamConnect,
    /// 上流への接続確立・リクエスト送信・応答受信のいずれかがタイムアウトした
    /// （[`crate::config::ProxyConfig::connect_timeout`] /
    /// [`crate::config::ProxyConfig::request_timeout`]）。
    UpstreamTimeout,
    /// 上流からの応答が HTTP/1.1 として不正、または `Content-Length` を欠く。
    UpstreamProtocol,
    /// 上流が 2xx 以外のステータスを返した。
    UpstreamStatus,
    /// 上流応答の body が [`crate::config::ProxyConfig::max_answer_bytes`] を超過した。
    AnswerTooLarge,
    /// ソケット I/O 自体のエラー（接続確立後の読み書き失敗）。
    Io,
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            ProxyError::UpstreamConnect => "upstream connect failed",
            ProxyError::UpstreamTimeout => "upstream request timed out",
            ProxyError::UpstreamProtocol => "upstream response is not valid HTTP/1.1",
            ProxyError::UpstreamStatus => "upstream returned a non-success status",
            ProxyError::AnswerTooLarge => "upstream answer exceeds max_answer_bytes",
            ProxyError::Io => "I/O error while talking to upstream",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for ProxyError {}
