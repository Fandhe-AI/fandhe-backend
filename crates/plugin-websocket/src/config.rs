//! WebSocket プラグインの静的設定。
//!
//! コア側（`crates/core/src/server.rs` の `Server::websocket`、`websocket`
//! feature 限定 API）がビルダーを通じてこの型を組み立て、
//! `crate::matches` / `crate::handle_upgrade` へ渡す。設定はビルド時・起動時
//! の静的値のみで構成し、リクエスト内容からは導出しない。

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::handler::{WsMessageHandler, default_handler};

/// アイドルタイムアウトの既定値（60 秒）。
///
/// 一般的なリバースプロキシの読み取りタイムアウト既定
/// （例: nginx `proxy_read_timeout` 60s）と同水準に揃え、正当なクライアントは
/// 通常の通信または Ping で容易に接続を維持できる一方、無通信のまま接続
/// （fd・タスク・メモリ）を無期限に保持させない（リソース枯渇 DoS 対策、
/// `.claude/rules/security.md`。Issue #175）。
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

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
#[derive(Clone)]
pub struct WebSocketConfig {
    /// WebSocket アップグレードを受け付ける request-target（既定 `/ws`）。
    pub path: String,
    /// 受信メッセージ（フレーム結合後）の最大バイト数（既定 1 MiB）。
    /// 超過した接続は tokio-tungstenite 側がプロトコルエラーとして
    /// クローズする（メモリ枯渇 DoS 対策）。
    pub max_message_size: usize,
    /// 受信する単一フレームの最大バイト数（既定 256 KiB）。
    pub max_frame_size: usize,
    /// クライアントからのフレーム受信が一定時間ないアイドル状態を検知し
    /// 切断するまでの猶予（既定 `Some(60 秒)`、fail-safe: 既定で有効）。
    ///
    /// `crate::session::run_session` がこの値でフレーム受信を
    /// `tokio::time::timeout` し、発火時は正常な Close ハンドシェイクで
    /// 切断する（Issue #175）。`None` にするとアイドルタイムアウトを無効化
    /// する（[`without_idle_timeout`][Self::without_idle_timeout] による
    /// 明示操作でのみ無効化を許し、暗黙に保護が外れないようにする）。
    pub idle_timeout: Option<Duration>,
    /// Text/Binary メッセージ受信ごとに呼ばれるユーザー定義ハンドラ
    /// （Issue #179）。既定は [`crate::handler::EchoHandler`]（後方互換）。
    ///
    /// `dyn WsMessageHandler` の直接構築を許すと将来の表現変更（例:
    /// 複数ハンドラの合成）の余地を狭めるため、`pub(crate)` にとどめ
    /// [`with_handler`][Self::with_handler] 経由でのみ差し替えを許す。
    pub(crate) handler: Arc<dyn WsMessageHandler>,
}

impl fmt::Debug for WebSocketConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSocketConfig")
            .field("path", &self.path)
            .field("max_message_size", &self.max_message_size)
            .field("max_frame_size", &self.max_frame_size)
            .field("idle_timeout", &self.idle_timeout)
            .field("handler", &self.handler.name())
            .finish()
    }
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            path: "/ws".to_string(),
            max_message_size: 1024 * 1024,
            max_frame_size: 256 * 1024,
            idle_timeout: Some(DEFAULT_IDLE_TIMEOUT),
            handler: default_handler(),
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

    /// アイドルタイムアウトを指定した値に変更する。
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use bf_plugin_websocket::WebSocketConfig;
    ///
    /// let config = WebSocketConfig::default().with_idle_timeout(Duration::from_secs(30));
    /// assert_eq!(config.idle_timeout, Some(Duration::from_secs(30)));
    /// ```
    #[must_use]
    pub fn with_idle_timeout(mut self, idle_timeout: Duration) -> Self {
        self.idle_timeout = Some(idle_timeout);
        self
    }

    /// アイドルタイムアウトを無効化する（明示操作でのみ許可、既定は有効）。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_websocket::WebSocketConfig;
    ///
    /// let config = WebSocketConfig::default().without_idle_timeout();
    /// assert_eq!(config.idle_timeout, None);
    /// ```
    #[must_use]
    pub fn without_idle_timeout(mut self) -> Self {
        self.idle_timeout = None;
        self
    }

    /// Text/Binary メッセージ受信ごとに呼ばれるユーザー定義ハンドラを登録する
    /// （Issue #179）。既定（未呼び出し時）は
    /// [`EchoHandler`][crate::handler::EchoHandler] のまま（後方互換）。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_websocket::WebSocketConfig;
    /// use bf_plugin_websocket::handler::{WsMessage, WsMessageHandler, WsOutcome};
    /// use futures_util::future::BoxFuture;
    ///
    /// struct Uppercase;
    ///
    /// impl WsMessageHandler for Uppercase {
    ///     fn name(&self) -> &'static str {
    ///         "uppercase"
    ///     }
    ///
    ///     fn on_message(
    ///         &self,
    ///         msg: WsMessage,
    ///     ) -> BoxFuture<'_, Result<WsOutcome, bf_plugin_websocket::handler::WsHandlerError>> {
    ///         Box::pin(async move {
    ///             let reply = match msg {
    ///                 WsMessage::Text(t) => WsMessage::Text(t.to_uppercase()),
    ///                 other => other,
    ///             };
    ///             Ok(WsOutcome::Reply(vec![reply]))
    ///         })
    ///     }
    /// }
    ///
    /// let config = WebSocketConfig::default().with_handler(Uppercase);
    /// assert_eq!(config.handler_name(), "uppercase");
    /// ```
    #[must_use]
    pub fn with_handler<H: WsMessageHandler>(mut self, handler: H) -> Self {
        self.handler = Arc::new(handler);
        self
    }

    /// 現在登録されているハンドラの診断名（[`WsMessageHandler::name`]）。
    /// `handler` フィールドは `pub(crate)` のため、外部から確認する手段として
    /// 公開する。
    #[must_use]
    pub fn handler_name(&self) -> &'static str {
        self.handler.name()
    }
}
