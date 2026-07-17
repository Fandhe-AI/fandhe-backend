//! WebSocket プラグインの静的設定。
//!
//! コア側（`crates/core/src/server.rs` の `Server::websocket`、`websocket`
//! feature 限定 API）がビルダーを通じてこの型を組み立て、
//! `crate::matches` / `crate::handle_upgrade` へ渡す。設定はビルド時・起動時
//! の静的値のみで構成し、リクエスト内容からは導出しない。

/// WebSocket アップグレードを受け付けるパス・DoS 安全側のフレーム制限。
///
/// `Default` はアップグレード対象パスを `/ws` とし、`max_message_size` /
/// `max_frame_size` を安全側の既定値に設定する
/// （`.claude/rules/security.md` のリソース枯渇対策）。
///
/// # Examples
///
/// ```
/// use bf_plugin_websocket::WebSocketConfig;
///
/// let config = WebSocketConfig::default();
/// assert_eq!(config.path, "/ws");
/// assert_eq!(config.max_message_size, 1024 * 1024);
/// ```
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// WebSocket アップグレードを受け付ける request-target（既定 `/ws`）。
    pub path: String,
    /// 受信メッセージ（フレーム結合後）の最大バイト数（既定 1 MiB）。
    /// 超過した接続は tokio-tungstenite 側がプロトコルエラーとして
    /// クローズする（メモリ枯渇 DoS 対策）。
    pub max_message_size: usize,
    /// 受信する単一フレームの最大バイト数（既定 256 KiB）。
    pub max_frame_size: usize,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            path: "/ws".to_string(),
            max_message_size: 1024 * 1024,
            max_frame_size: 256 * 1024,
        }
    }
}

impl WebSocketConfig {
    /// アップグレード対象パスを指定した設定を作る（他フィールドは既定値）。
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// 受信メッセージの最大バイト数を指定する。
    #[must_use]
    pub fn with_max_message_size(mut self, max_message_size: usize) -> Self {
        self.max_message_size = max_message_size;
        self
    }

    /// 受信フレームの最大バイト数を指定する。
    #[must_use]
    pub fn with_max_frame_size(mut self, max_frame_size: usize) -> Self {
        self.max_frame_size = max_frame_size;
        self
    }
}
