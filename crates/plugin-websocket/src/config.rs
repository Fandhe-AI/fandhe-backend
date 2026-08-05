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

/// Close handshake ドレイン猶予の既定値（10 秒）。
///
/// アイドルタイムアウト切断（Issue #175）・コアからのキャンセル切断
/// （イシュー #492）の両経路が共有する `crate::session::close_and_drain` の
/// 上限。Close フレーム送出からクライアント応答（または EOF）待ちまでの
/// 全体をこの値で有界化し、Close 応答を返さないクライアントが接続を
/// 無期限保持する二次的な DoS の抜け道を塞ぐ（`.claude/rules/security.md`）。
/// イシュー #500 でこの値を利用者が調整できるビルダー
/// （[`WebSocketConfig::with_close_grace`]）へ切り出した。
const DEFAULT_CLOSE_GRACE: Duration = Duration::from_secs(10);

/// WebSocket アップグレードを受け付けるパス・DoS 安全側のフレーム制限。
///
/// `Default` はアップグレード対象パスを `/ws` とし、`max_message_size` /
/// `max_frame_size` を安全側の既定値に設定する
/// （`.claude/rules/security.md` のリソース枯渇対策）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_websocket::WebSocketConfig;
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
    /// Close handshake（サーバ側からの Close フレーム送出 → クライアント
    /// 応答またはEOF待ち）を打ち切るまでの猶予（既定 10 秒）。
    ///
    /// アイドルタイムアウト発火時（`idle_timeout`）・コアからのキャンセル
    /// シグナル発火時（イシュー #492）の両経路で共有される
    /// `crate::session::close_and_drain` がこの値で
    /// `tokio::time::timeout` する。
    ///
    /// **`Option<Duration>` にしていない（無効化不可）**: この上限は
    /// 「Close 応答を返さないクライアントが接続を無期限保持する」二次的な
    /// DoS を防ぐ安全性の下限そのものであり、`idle_timeout` のような明示的
    /// 無効化手段は提供しない（fail-closed、`.claude/rules/security.md`）。
    ///
    /// - `Duration::ZERO` を設定すると Close 送出後すぐにドレインを打ち切り
    ///   即座に接続を終端する。Close フレームの配送は保証されなくなるが、
    ///   接続自体は即終端されるため安全側に倒れる。下限のクランプはしない。
    /// - 既定 10 秒より大幅に大きい値を設定すると、Close 応答を返さない
    ///   クライアントがその時間だけ接続（fd・タスク・メモリ）を保持し続け、
    ///   二次 DoS の猶予窓が拡大する。利用者の明示 opt-in であることを
    ///   前提に上限のクランプはしないが、既定値（10 秒）からの大幅な
    ///   引き上げは推奨しない。
    pub close_grace: Duration,
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
            .field("close_grace", &self.close_grace)
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
            close_grace: DEFAULT_CLOSE_GRACE,
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
    /// use fandhe_backend_plugin_websocket::WebSocketConfig;
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
    /// use fandhe_backend_plugin_websocket::WebSocketConfig;
    ///
    /// let config = WebSocketConfig::default().without_idle_timeout();
    /// assert_eq!(config.idle_timeout, None);
    /// ```
    #[must_use]
    pub fn without_idle_timeout(mut self) -> Self {
        self.idle_timeout = None;
        self
    }

    /// Close handshake ドレイン猶予（[`close_grace`][Self::close_grace]）を
    /// 指定した値に変更する（イシュー #500）。
    ///
    /// `Duration::ZERO` や既定値（10 秒）より大幅に大きい値も受け付ける
    /// （クランプなし）。それぞれの挙動・DoS 観点の考慮は
    /// [`close_grace`][Self::close_grace] フィールドの doc を参照。
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use fandhe_backend_plugin_websocket::WebSocketConfig;
    ///
    /// let config = WebSocketConfig::default().with_close_grace(Duration::from_secs(3));
    /// assert_eq!(config.close_grace, Duration::from_secs(3));
    /// ```
    #[must_use]
    pub fn with_close_grace(mut self, close_grace: Duration) -> Self {
        self.close_grace = close_grace;
        self
    }

    /// Text/Binary メッセージ受信ごとに呼ばれるユーザー定義ハンドラを登録する
    /// （Issue #179）。既定（未呼び出し時）は
    /// [`EchoHandler`][crate::handler::EchoHandler] のまま（後方互換）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_websocket::WebSocketConfig;
    /// use fandhe_backend_plugin_websocket::handler::{WsMessage, WsMessageHandler, WsOutcome};
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
    ///     ) -> BoxFuture<'_, Result<WsOutcome, fandhe_backend_plugin_websocket::handler::WsHandlerError>> {
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
