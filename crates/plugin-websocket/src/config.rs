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

/// [`WebSocketConfig::with_outbound_capacity`] が受け付ける送信キュー容量の
/// 上限（イシュー #709）。
///
/// `tokio::sync::mpsc::channel` は内部の `Semaphore::MAX_PERMITS`
/// （`usize::MAX >> 3` 相当）を超える容量で panic するため、`crate::handler::channel`
/// が接続ごとに呼ぶ `mpsc::channel(capacity)` がライブラリ境界で panic しない
/// よう、構築時（本モジュール）に有限の上限で弾く（`.claude/rules/coding-rust.md`
/// の「panic はライブラリ境界を越えさせない」）。
///
/// 値は既定値（8）の 512 倍。送信キューは接続ごとに保持するため、実質的な
/// メモリ上限は「本値 × メッセージサイズ × 同時接続数」で決まり、この上限を
/// 有界に保つことは接続数に対するリソース枯渇 DoS 対策でもある
/// （`.claude/rules/security.md`）。CDP 互換サーバーのような短時間バーストの
/// 吸収には十分な余裕を持たせつつ、無制限に近い値は許可しない。将来値を
/// 引き上げる場合は非破壊変更で行える。
pub const MAX_OUTBOUND_CAPACITY: usize = 4096;

/// サーバー起点 Ping keepalive の設定（イシュー #713）。
///
/// `WebSocketConfig::ping` に保持し、`crate::session::run_session_inner` が
/// `interval` ごとに Ping を送出し `pong_timeout` 以内に Pong が届かない
/// 接続を切断する死活監視の入力になる。`outbound_capacity` と同じ理由
/// （直接代入による構築時検証の迂回防止）で `WebSocketConfig::ping` 自体も
/// `pub(crate)` にとどめ、`with_ping_interval` 経由でのみ設定させる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PingKeepalive {
    pub(crate) interval: Duration,
    pub(crate) pong_timeout: Duration,
}

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
    ///
    /// [`with_path_pattern`][Self::with_path_pattern] でパターンを登録した
    /// 場合、実際の照合（`crate::handshake::matches`）はパターン側が優先
    /// され、この `path` フィールドを直接読み替えても判定には反映されない
    /// （診断・`Debug` 用の文字列表現にとどまる。パターン登録時は
    /// `with_path_pattern` に渡した元の文字列をそのまま保持する）。
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
    ///
    /// # リセット条件（イシュー #714）
    ///
    /// 期限はクライアントから実際にフレームを 1 つ受信するたびに延長される
    /// （Text / Binary / Ping / Pong の全種別。ハンドラ処理・返信送出が
    /// 完了して次の受信待ちに入る直前に更新するため、処理時間そのものは
    /// アイドル待機時間に算入しない）。**サーバー起点の送出では延長されない**:
    /// [`crate::handler::WsSender::send`]/`try_send` による push、
    /// [`with_ping_interval`][Self::with_ping_interval] によるサーバー起点
    /// Ping の送出、[`WsOutcome::Reply`][crate::handler::WsOutcome::Reply]
    /// の送出のいずれも本フィールドをリセットしない。そのため CDP 互換
    /// サーバーのように「サーバーが push するだけでクライアントは受信専用」
    /// の用途では、`idle_timeout` だけでは生存クライアントを維持できず
    /// 切断されてしまう（`crate::session` の単体テスト
    /// `outbound_push_does_not_reset_idle_timeout` が push 側の契約を固定、
    /// `tests/idle_keepalive_e2e.rs::push_only_traffic_triggers_idle_timeout`
    /// が e2e で同じ結果を検証する）。
    ///
    /// # Ping keepalive との併用（推奨設定）
    ///
    /// push を受けているだけの受信専用クライアントも死活監視したい場合は
    /// [`with_ping_interval`][Self::with_ping_interval] を併用する。Pong の
    /// 受信は他の全フレーム種別と同じく本フィールドもリセットするため、
    /// **`interval + pong_timeout` が本フィールドの値より小さくなるように
    /// 設定すれば**、生存クライアントは Ping への自動 Pong で
    /// `idle_timeout` が発火する前に期限が延長され続け、切断されない
    /// （例: 既定 60 秒に対し `with_ping_interval(30s, 10s)`。
    /// `tests/idle_keepalive_e2e.rs::
    /// ping_keepalive_keeps_push_only_client_alive_beyond_idle_timeout`
    /// で検証）。Pong を返さない対向は、`idle_timeout` より先に Pong 期限
    /// （[`CloseReason::PongTimeout`][crate::handler::CloseReason::PongTimeout]）
    /// で切断される（`idle_deadline` と keepalive のタイマーは早い方が
    /// 採用される契約、`crate::session` モジュール doc・
    /// `docs/design/ws-connection-context-and-close.md` 13 節を参照）。
    ///
    /// **誤設定への注意**: `interval` を本フィールドの値以上にすると、
    /// 最初の Ping が送られる前（またはその Pong が届く前）に
    /// `idle_timeout` が発火してしまい、生存している受信専用クライアント
    /// でも切断される
    /// （`tests/idle_keepalive_e2e.rs::
    /// ping_interval_not_shorter_than_idle_timeout_still_hits_idle_timeout`
    /// で固定）。本フィールドは無効化しない（fail-safe、Issue #175 を
    /// 後退させない）ことを推奨する。`without_idle_timeout()` は keepalive
    /// を有効化している場合の補足的な選択肢に留める（死活監視自体は
    /// keepalive の Pong 期限が担う構成になる）。
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
    /// [`with_path_pattern`][Self::with_path_pattern] で登録されたパス
    /// パターン（イシュー #675）。`None`（既定）のときは `path` との完全
    /// 一致で照合する（`crate::handshake::matches` を参照）。
    pub(crate) pattern: Option<crate::pattern::PathPattern>,
    /// サーバー起点送信キュー（`crate::handler::WsSender`）の容量（イシュー
    /// #709、既定 [`crate::handler::DEFAULT_OUTBOUND_CAPACITY`] = 8）。
    ///
    /// `pub` にせず [`with_outbound_capacity`][Self::with_outbound_capacity]
    /// 経由でのみ設定させる（直接代入だと 0・`MAX_OUTBOUND_CAPACITY` 超の
    /// 検証を迂回でき、`crate::handler::channel` の `mpsc::channel` 呼び出しで
    /// panic する余地が残るため。`handler`/`pattern` と同じ非公開フィールド +
    /// アクセサの方針）。
    pub(crate) outbound_capacity: usize,
    /// サーバー起点 Ping keepalive（イシュー #713）。`None`（既定）は無効
    /// （後方互換。既存の `idle_timeout` のみによる死活監視から挙動を
    /// 変えない）。[`with_ping_interval`][Self::with_ping_interval] で
    /// 有効化する。
    pub(crate) ping: Option<PingKeepalive>,
    /// ハンドシェイクの受理判定フック（イシュー #716）。`None`（既定）は
    /// 無効で、RFC 6455 検証（`crate::handshake::validate`）を通過した
    /// 要求はそのまま 101 応答を送出する（後方互換、既存の挙動は変わらない）。
    ///
    /// `dyn WsHandshakeCheck` の直接構築を許すと将来の表現変更の余地を
    /// 狭めるため、`handler`/`pattern` と同じく `pub(crate)` にとどめ
    /// [`with_handshake_check`][Self::with_handshake_check] 経由でのみ
    /// 設定させる。
    pub(crate) handshake_check: Option<Arc<dyn crate::handshake::WsHandshakeCheck>>,
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
            .field("pattern", &self.pattern)
            .field("outbound_capacity", &self.outbound_capacity)
            .field("ping", &self.ping)
            // クロージャ本体・キャプチャした値は出力せず、登録有無のみを
            // 示す（`.claude/rules/security.md`「ログに機密を出さない」。
            // `handler` フィールドと同型の判断）。
            .field("handshake_check", &self.handshake_check.is_some())
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
            pattern: None,
            outbound_capacity: crate::handler::DEFAULT_OUTBOUND_CAPACITY,
            ping: None,
            handshake_check: None,
        }
    }
}

/// [`WebSocketConfig::with_outbound_capacity`] が返す構築時検証エラー
/// （イシュー #709）。
///
/// 将来 variant を追加しても非破壊変更として扱うため
/// `#[non_exhaustive]` を付ける（`crate::handler::WsCloseError` と同一方針）。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundCapacityError {
    /// 容量に 0 を指定した（`tokio::sync::mpsc::channel(0)` は panic するため
    /// 構築時に拒否する）。
    Zero,
    /// 容量が [`MAX_OUTBOUND_CAPACITY`] を超えている。
    TooLarge,
}

impl fmt::Display for OutboundCapacityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Zero => "websocket outbound capacity must be at least 1",
            Self::TooLarge => "websocket outbound capacity exceeds maximum",
        };
        write!(f, "{msg}")
    }
}

impl std::error::Error for OutboundCapacityError {}

/// [`WebSocketConfig::with_ping_interval`] が返す構築時検証エラー
/// （イシュー #713）。`OutboundCapacityError` と同一方針（`#[non_exhaustive]`・
/// panic しない fail-closed 契約）。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PingIntervalError {
    /// `interval` に `Duration::ZERO` を指定した。Ping を送り続けるビジー
    /// ループ（サーバー自身への DoS）になるため構築時に拒否する。
    ZeroInterval,
    /// `pong_timeout` に `Duration::ZERO` を指定した。最初に送出した Ping の
    /// 応答を待つ間もなく即座に全接続が切断される誤設定になるため構築時に
    /// 拒否する。
    ZeroPongTimeout,
}

impl fmt::Display for PingIntervalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::ZeroInterval => "websocket ping interval must not be zero",
            Self::ZeroPongTimeout => "websocket pong timeout must not be zero",
        };
        write!(f, "{msg}")
    }
}

impl std::error::Error for PingIntervalError {}

impl WebSocketConfig {
    /// アップグレード対象パスを指定した設定を作る（他フィールドは既定値）。
    ///
    /// 以前 [`with_path_pattern`][Self::with_path_pattern] で登録された
    /// パターンがあれば破棄し、完全一致契約へ確実に戻す（イシュー #675。
    /// 呼び出し順に関わらず「最後に呼ばれた方が有効」という直感的な契約を
    /// 保つ）。
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self.pattern = None;
        self
    }

    /// アップグレード対象パスを `{name}` パラメータ付きパターンとして登録
    /// する（イシュー #675、親 #673）。
    ///
    /// `{`/`}` を含まない文字列は構築時検証なしの完全一致として扱われ、
    /// [`with_path`][Self::with_path] と同じ挙動になる
    /// （[`crate::pattern::PathPattern::parse`] の契約を参照）。`{name}` を
    /// 含む場合は構築時に検証済みの [`crate::pattern::PathPattern`] として
    /// 保持し、`crate::handshake::matches` はこのパターンを優先して照合する
    /// （完全一致判定より優先、[`path`][Self::path] フィールドの doc も
    /// 参照）。
    ///
    /// 複数の `WebSocketConfig` をコア側 `Server::websocket`
    /// （`fandhe-backend-core`、`websocket` feature）へ複数回登録することで、
    /// `/devtools/browser/{id}` と `/devtools/page/{id}` のような複数
    /// パターンを同時に扱える。**登録順に最初に一致した設定を使う**契約は
    /// コア側（`crates/core/src/plugin.rs` の `try_handle_upgrade` が行う
    /// `.find()`）が担い、本メソッドはそこに影響しない。
    ///
    /// 本メソッドの後に [`with_path`][Self::with_path] を呼ぶとパターンは
    /// 破棄される。
    ///
    /// # Errors
    ///
    /// `pattern` が [`crate::pattern::PathPattern::parse`] の検証（先頭
    /// スラッシュ・パラメータ名の文字集合・重複・セグメント数上限等）に
    /// 違反する場合は [`crate::pattern::PathPatternError`] を返す
    /// （panic しない fail-closed 契約、`.claude/rules/coding-rust.md`）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_websocket::WebSocketConfig;
    ///
    /// let config = WebSocketConfig::default()
    ///     .with_path_pattern("/devtools/page/{id}")
    ///     .unwrap();
    /// assert_eq!(config.path, "/devtools/page/{id}");
    /// ```
    ///
    /// 不正なパターンは `Err` になる（先頭スラッシュなし）:
    ///
    /// ```
    /// use fandhe_backend_plugin_websocket::WebSocketConfig;
    ///
    /// let result = WebSocketConfig::default().with_path_pattern("devtools/{id}");
    /// assert!(result.is_err());
    /// ```
    pub fn with_path_pattern(
        mut self,
        pattern: impl Into<String>,
    ) -> Result<Self, crate::pattern::PathPatternError> {
        let pattern = pattern.into();
        let parsed = crate::pattern::PathPattern::parse(&pattern)?;
        self.path = pattern;
        self.pattern = Some(parsed);
        Ok(self)
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
    /// クライアントからのフレーム受信でのみリセットされ、サーバー起点の
    /// push・Ping 送出では延長されない（[`idle_timeout`][Self::idle_timeout]
    /// フィールドの doc「リセット条件」節を参照）。
    /// [`with_ping_interval`][Self::with_ping_interval] と併用する場合の
    /// 推奨設定・誤設定時の挙動も同節を参照。
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
    ///
    /// 推奨設定（`interval + pong_timeout < idle_timeout`）で
    /// `with_ping_interval` と組み合わせる例（イシュー #714）:
    ///
    /// ```
    /// use std::time::Duration;
    /// use fandhe_backend_plugin_websocket::WebSocketConfig;
    ///
    /// let config = WebSocketConfig::default()
    ///     .with_idle_timeout(Duration::from_secs(60))
    ///     .with_ping_interval(Duration::from_secs(30), Duration::from_secs(10))
    ///     .unwrap();
    /// assert!(
    ///     config.ping_interval().unwrap() + config.pong_timeout().unwrap()
    ///         < config.idle_timeout.unwrap()
    /// );
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
    /// `futures-util` を一切 import せずに実装できる（イシュー #723。
    /// [`BoxFuture`][crate::BoxFuture] は `futures_util::future::BoxFuture`
    /// と同一の型を持つ本クレート独自の型エイリアス）。
    ///
    /// ```
    /// use fandhe_backend_plugin_websocket::{BoxFuture, WebSocketConfig};
    /// use fandhe_backend_plugin_websocket::handler::{WsMessage, WsMessageHandler, WsOutcome};
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

    /// サーバー起点送信キュー（[`crate::handler::WsSender`]）の容量を指定する
    /// （イシュー #709）。
    ///
    /// 既定は 8（`crate::handler::DEFAULT_OUTBOUND_CAPACITY`、非公開定数）。
    /// 満杯時、
    /// [`WsSender::send`][crate::handler::WsSender::send] は空くまで待ち、
    /// [`WsSender::try_send`][crate::handler::WsSender::try_send] は待たずに
    /// `Full` エラーを返す。送信キューは接続ごとに保持するため、実質的な
    /// メモリ上限の目安は「本値 × メッセージサイズ × 同時接続数」になる
    /// （`.claude/rules/security.md`）。
    ///
    /// # Errors
    ///
    /// `capacity` が 0 の場合は
    /// [`OutboundCapacityError::Zero`]、[`MAX_OUTBOUND_CAPACITY`] を超える
    /// 場合は [`OutboundCapacityError::TooLarge`] を返す（いずれも panic
    /// しない fail-closed 契約、`.claude/rules/coding-rust.md`）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_websocket::WebSocketConfig;
    ///
    /// let config = WebSocketConfig::default()
    ///     .with_outbound_capacity(64)
    ///     .unwrap();
    /// assert_eq!(config.outbound_capacity(), 64);
    /// ```
    ///
    /// 0 は拒否される:
    ///
    /// ```
    /// use fandhe_backend_plugin_websocket::{OutboundCapacityError, WebSocketConfig};
    ///
    /// let err = WebSocketConfig::default().with_outbound_capacity(0).unwrap_err();
    /// assert_eq!(err, OutboundCapacityError::Zero);
    /// ```
    ///
    /// 上限超も拒否される:
    ///
    /// ```
    /// use fandhe_backend_plugin_websocket::{
    ///     MAX_OUTBOUND_CAPACITY, OutboundCapacityError, WebSocketConfig,
    /// };
    ///
    /// let err = WebSocketConfig::default()
    ///     .with_outbound_capacity(MAX_OUTBOUND_CAPACITY + 1)
    ///     .unwrap_err();
    /// assert_eq!(err, OutboundCapacityError::TooLarge);
    /// ```
    pub fn with_outbound_capacity(
        mut self,
        capacity: usize,
    ) -> Result<Self, OutboundCapacityError> {
        if capacity == 0 {
            return Err(OutboundCapacityError::Zero);
        }
        if capacity > MAX_OUTBOUND_CAPACITY {
            return Err(OutboundCapacityError::TooLarge);
        }
        self.outbound_capacity = capacity;
        Ok(self)
    }

    /// 現在設定されている送信キュー容量（[`with_outbound_capacity`][Self::with_outbound_capacity]）。
    /// `outbound_capacity` フィールドは
    /// `pub(crate)` のため、外部から確認する手段として公開する。
    #[must_use]
    pub fn outbound_capacity(&self) -> usize {
        self.outbound_capacity
    }

    /// サーバー起点 Ping による死活監視を有効化する（イシュー #713）。
    ///
    /// `interval` ごとにサーバーから `Message::Ping` を送出し、送出時点から
    /// `pong_timeout` 以内にクライアントの Pong が届かなければアイドル
    /// タイムアウトと同型の正常な Close ハンドシェイクで切断する
    /// （[`crate::handler::CloseReason::PongTimeout`]）。各 Ping には
    /// 8 バイト big-endian の単調増加シーケンス番号を識別ペイロードとして
    /// 付与し、Pong はこのペイロードが一致した場合にのみ Pong 期限を解除
    /// する（一致しない Pong は無視して待機を継続する。レビュー指摘対応、
    /// PR #738。`crate::session` の `Keepalive::next_payload` を参照）。
    /// 期限切れは受信待ちで読めるフレームがなくなった時点で判定し、それ
    /// までに届いているフレームは先に読んで処理する（一致する Pong を
    /// 返さない対向も、フレームが途切れず届いている間は切断しない。
    /// `crate::session` モジュール doc「サーバー起点 Ping keepalive」節を
    /// 参照）。1 回の送出（Ping・Reply・outbound push 等）が
    /// `pong_timeout` を超えてブロックした場合も同じ `PongTimeout` で
    /// 終了する。
    ///
    /// `idle_timeout`（既定で有効）はクライアントからの受信でのみリセット
    /// され、サーバー起点の Ping 送出ではリセットしない。そのため受信専用
    /// （サーバー起点 push を受けているだけ）のクライアントは
    /// `idle_timeout` だけでは死活監視できず、本設定が必要になる（親
    /// #712）。Pong の受信自体は他の全フレーム種別と同じく `idle_timeout`
    /// もリセットする（既存挙動）。
    ///
    /// **`idle_timeout` との推奨設定（イシュー #714）**: `interval +
    /// pong_timeout` を `idle_timeout` より小さく設定すると、生存クライアント
    /// は Ping への Pong で `idle_timeout` が延長され続け、Pong を返さない
    /// 対向は `idle_timeout` より先に `PongTimeout` で切断される（詳細・
    /// 検証は [`idle_timeout`][Self::idle_timeout] フィールドの doc
    /// 「Ping keepalive との併用」節を参照）。逆に `interval` を
    /// `idle_timeout` 以上にすると、最初の Ping が送られる前に
    /// `idle_timeout` が発火してしまい、生存クライアントでも切断される
    /// （同節「誤設定への注意」を参照）。
    ///
    /// 既定（未呼び出し時）は無効（後方互換。既存の `idle_timeout` のみに
    /// よる死活監視から挙動を変えない）。
    ///
    /// # Errors
    ///
    /// `interval` が `Duration::ZERO` の場合は
    /// [`PingIntervalError::ZeroInterval`]（Ping を送り続けるビジーループを
    /// 防ぐ）、`pong_timeout` が `Duration::ZERO` の場合は
    /// [`PingIntervalError::ZeroPongTimeout`]（最初の Ping で全接続が切れる
    /// 誤設定を防ぐ）を返す（panic しない fail-closed 契約、
    /// `.claude/rules/coding-rust.md`）。`pong_timeout >= interval` は拒否
    /// しない（未応答の Ping が続いても期限は延長されない契約）。
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use fandhe_backend_plugin_websocket::WebSocketConfig;
    ///
    /// let config = WebSocketConfig::default()
    ///     .with_ping_interval(Duration::from_secs(30), Duration::from_secs(10))
    ///     .unwrap();
    /// assert_eq!(config.ping_interval(), Some(Duration::from_secs(30)));
    /// assert_eq!(config.pong_timeout(), Some(Duration::from_secs(10)));
    /// ```
    ///
    /// `interval` に 0 を指定すると拒否される:
    ///
    /// ```
    /// use std::time::Duration;
    /// use fandhe_backend_plugin_websocket::{PingIntervalError, WebSocketConfig};
    ///
    /// let err = WebSocketConfig::default()
    ///     .with_ping_interval(Duration::ZERO, Duration::from_secs(10))
    ///     .unwrap_err();
    /// assert_eq!(err, PingIntervalError::ZeroInterval);
    /// ```
    ///
    /// `pong_timeout` に 0 を指定すると拒否される:
    ///
    /// ```
    /// use std::time::Duration;
    /// use fandhe_backend_plugin_websocket::{PingIntervalError, WebSocketConfig};
    ///
    /// let err = WebSocketConfig::default()
    ///     .with_ping_interval(Duration::from_secs(30), Duration::ZERO)
    ///     .unwrap_err();
    /// assert_eq!(err, PingIntervalError::ZeroPongTimeout);
    /// ```
    pub fn with_ping_interval(
        mut self,
        interval: Duration,
        pong_timeout: Duration,
    ) -> Result<Self, PingIntervalError> {
        if interval == Duration::ZERO {
            return Err(PingIntervalError::ZeroInterval);
        }
        if pong_timeout == Duration::ZERO {
            return Err(PingIntervalError::ZeroPongTimeout);
        }
        self.ping = Some(PingKeepalive {
            interval,
            pong_timeout,
        });
        Ok(self)
    }

    /// サーバー起点 Ping による死活監視を明示的に無効化する
    /// （[`without_idle_timeout`][Self::without_idle_timeout] と対称、
    /// イシュー #713）。既定も無効のため通常は呼ぶ必要はない。
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use fandhe_backend_plugin_websocket::WebSocketConfig;
    ///
    /// let config = WebSocketConfig::default()
    ///     .with_ping_interval(Duration::from_secs(30), Duration::from_secs(10))
    ///     .unwrap()
    ///     .without_ping_interval();
    /// assert_eq!(config.ping_interval(), None);
    /// ```
    #[must_use]
    pub fn without_ping_interval(mut self) -> Self {
        self.ping = None;
        self
    }

    /// 現在設定されている Ping keepalive の送出間隔
    /// （[`with_ping_interval`][Self::with_ping_interval]）。`ping` フィールドは
    /// `pub(crate)` のため、外部から確認する手段として公開する。
    #[must_use]
    pub fn ping_interval(&self) -> Option<Duration> {
        self.ping.map(|p| p.interval)
    }

    /// 現在設定されている Ping keepalive の Pong 待機上限
    /// （[`with_ping_interval`][Self::with_ping_interval]）。
    #[must_use]
    pub fn pong_timeout(&self) -> Option<Duration> {
        self.ping.map(|p| p.pong_timeout)
    }

    /// ハンドシェイクの受理判定フックを登録する（イシュー #716）。
    ///
    /// `check` は RFC 6455 検証（`crate::handshake::validate`）を通過した
    /// upgrade 要求について、101 応答を送出する直前に一度だけ同期で呼ばれる。
    /// `Err(response)` を返すと `response` を（`crate::handshake::normalize_rejection`
    /// による正規化を経て）クライアントへ送出し、upgrade しない。
    /// [`crate::handshake::WsHandshakeCheck`] の doc に契約（同期・
    /// 非ブロッキング・panic しない）を記載しているので必ず確認すること。
    ///
    /// 後から呼んだものが有効（複数回呼ぶと最後の登録のみが残る）。
    ///
    /// # Examples
    ///
    /// `Origin` ヘッダを検査し、許可されていない場合は `403` で拒否する例:
    ///
    /// ```
    /// use fandhe_backend_http::response::Response;
    /// use fandhe_backend_plugin_websocket::{WebSocketConfig, WsHandshakeContext};
    ///
    /// let config = WebSocketConfig::default().with_handshake_check(
    ///     |ctx: &WsHandshakeContext<'_>| match ctx.header("origin") {
    ///         Some("https://example.com") => Ok(()),
    ///         _ => Err(Response::empty(403)),
    ///     },
    /// );
    /// assert!(config.has_handshake_check());
    /// ```
    #[must_use]
    pub fn with_handshake_check<C>(mut self, check: C) -> Self
    where
        C: crate::handshake::WsHandshakeCheck,
    {
        self.handshake_check = Some(Arc::new(check));
        self
    }

    /// ハンドシェイクの受理判定フックが登録済みかを返す（診断・テスト用）。
    #[must_use]
    pub fn has_handshake_check(&self) -> bool {
        self.handshake_check.is_some()
    }

    /// ハンドシェイクの受理判定フックの登録を解除する（既定の状態に戻す）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_http::response::Response;
    /// use fandhe_backend_plugin_websocket::{WebSocketConfig, WsHandshakeContext};
    ///
    /// let config = WebSocketConfig::default()
    ///     .with_handshake_check(|_ctx: &WsHandshakeContext<'_>| Err(Response::empty(403)))
    ///     .without_handshake_check();
    /// assert!(!config.has_handshake_check());
    /// ```
    #[must_use]
    pub fn without_handshake_check(mut self) -> Self {
        self.handshake_check = None;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_path_pattern_accepts_valid_pattern_and_updates_path() {
        let config = WebSocketConfig::default()
            .with_path_pattern("/devtools/page/{id}")
            .unwrap();
        assert_eq!(config.path, "/devtools/page/{id}");
        assert!(config.pattern.is_some());
    }

    #[test]
    fn with_path_pattern_rejects_invalid_pattern() {
        let err = WebSocketConfig::default()
            .with_path_pattern("devtools/{id}")
            .unwrap_err();
        assert_eq!(err, crate::pattern::PathPatternError::MissingLeadingSlash);
    }

    #[test]
    fn with_path_after_with_path_pattern_resets_to_exact_match() {
        let config = WebSocketConfig::default()
            .with_path_pattern("/devtools/page/{id}")
            .unwrap()
            .with_path("/ws");
        assert!(config.pattern.is_none());
        assert_eq!(config.path, "/ws");
    }

    #[test]
    fn default_outbound_capacity_is_8() {
        assert_eq!(
            WebSocketConfig::default().outbound_capacity(),
            crate::handler::DEFAULT_OUTBOUND_CAPACITY
        );
        assert_eq!(WebSocketConfig::default().outbound_capacity(), 8);
    }

    #[test]
    fn with_outbound_capacity_accepts_valid_values() {
        for capacity in [1, 16, MAX_OUTBOUND_CAPACITY] {
            let config = WebSocketConfig::default()
                .with_outbound_capacity(capacity)
                .unwrap();
            assert_eq!(config.outbound_capacity(), capacity);
        }
    }

    #[test]
    fn with_outbound_capacity_rejects_zero() {
        let err = WebSocketConfig::default()
            .with_outbound_capacity(0)
            .unwrap_err();
        assert_eq!(err, OutboundCapacityError::Zero);
    }

    #[test]
    fn with_outbound_capacity_rejects_over_max() {
        let err = WebSocketConfig::default()
            .with_outbound_capacity(MAX_OUTBOUND_CAPACITY + 1)
            .unwrap_err();
        assert_eq!(err, OutboundCapacityError::TooLarge);

        let err = WebSocketConfig::default()
            .with_outbound_capacity(usize::MAX)
            .unwrap_err();
        assert_eq!(err, OutboundCapacityError::TooLarge);
    }

    #[test]
    fn debug_includes_outbound_capacity() {
        let config = WebSocketConfig::default()
            .with_outbound_capacity(32)
            .unwrap();
        assert!(format!("{config:?}").contains("outbound_capacity: 32"));
    }

    #[test]
    fn outbound_capacity_error_display_is_fixed_text() {
        assert_eq!(
            OutboundCapacityError::Zero.to_string(),
            "websocket outbound capacity must be at least 1"
        );
        assert_eq!(
            OutboundCapacityError::TooLarge.to_string(),
            "websocket outbound capacity exceeds maximum"
        );
    }

    #[test]
    fn default_ping_interval_is_disabled() {
        let config = WebSocketConfig::default();
        assert_eq!(config.ping_interval(), None);
        assert_eq!(config.pong_timeout(), None);
    }

    #[test]
    fn with_ping_interval_accepts_valid_values() {
        let config = WebSocketConfig::default()
            .with_ping_interval(Duration::from_secs(30), Duration::from_secs(10))
            .unwrap();
        assert_eq!(config.ping_interval(), Some(Duration::from_secs(30)));
        assert_eq!(config.pong_timeout(), Some(Duration::from_secs(10)));
    }

    #[test]
    fn with_ping_interval_rejects_zero_interval() {
        let err = WebSocketConfig::default()
            .with_ping_interval(Duration::ZERO, Duration::from_secs(10))
            .unwrap_err();
        assert_eq!(err, PingIntervalError::ZeroInterval);
    }

    #[test]
    fn with_ping_interval_rejects_zero_pong_timeout() {
        let err = WebSocketConfig::default()
            .with_ping_interval(Duration::from_secs(30), Duration::ZERO)
            .unwrap_err();
        assert_eq!(err, PingIntervalError::ZeroPongTimeout);
    }

    #[test]
    fn with_ping_interval_allows_pong_timeout_ge_interval() {
        // pong_timeout >= interval は拒否しない（設計上「延長しない」で意味を
        // 定める。構築時検証としては値の大小関係を制約しない）。
        let config = WebSocketConfig::default()
            .with_ping_interval(Duration::from_secs(5), Duration::from_secs(30))
            .unwrap();
        assert_eq!(config.ping_interval(), Some(Duration::from_secs(5)));
        assert_eq!(config.pong_timeout(), Some(Duration::from_secs(30)));
    }

    #[test]
    fn without_ping_interval_resets_to_disabled() {
        let config = WebSocketConfig::default()
            .with_ping_interval(Duration::from_secs(30), Duration::from_secs(10))
            .unwrap()
            .without_ping_interval();
        assert_eq!(config.ping_interval(), None);
    }

    #[test]
    fn debug_includes_ping() {
        let config = WebSocketConfig::default()
            .with_ping_interval(Duration::from_secs(30), Duration::from_secs(10))
            .unwrap();
        let debug = format!("{config:?}");
        assert!(debug.contains("ping"));
    }

    #[test]
    fn ping_interval_error_display_is_fixed_text() {
        assert_eq!(
            PingIntervalError::ZeroInterval.to_string(),
            "websocket ping interval must not be zero"
        );
        assert_eq!(
            PingIntervalError::ZeroPongTimeout.to_string(),
            "websocket pong timeout must not be zero"
        );
    }

    #[test]
    fn has_handshake_check_defaults_to_false() {
        // 未登録時は既存の挙動（RFC 6455 検証通過で無条件 101）を変えない
        // （イシュー #716、後方互換）。
        assert!(!WebSocketConfig::default().has_handshake_check());
    }

    #[test]
    fn with_handshake_check_registers_hook() {
        let config = WebSocketConfig::default()
            .with_handshake_check(|_ctx: &crate::handshake::WsHandshakeContext<'_>| Ok(()));
        assert!(config.has_handshake_check());
    }

    #[test]
    fn without_handshake_check_clears_hook() {
        let config = WebSocketConfig::default()
            .with_handshake_check(|_ctx: &crate::handshake::WsHandshakeContext<'_>| Ok(()))
            .without_handshake_check();
        assert!(!config.has_handshake_check());
    }

    #[test]
    fn clone_shares_handshake_check() {
        // `Arc` 経由の共有のため、clone 後も同じフックが有効であること
        // （`handler`/`pattern` と同型の契約）。
        let config = WebSocketConfig::default()
            .with_handshake_check(|_ctx: &crate::handshake::WsHandshakeContext<'_>| Ok(()));
        let cloned = config.clone();
        assert!(cloned.has_handshake_check());
    }

    #[test]
    fn debug_does_not_leak_handshake_check_closure_and_shows_registration_bool() {
        let config = WebSocketConfig::default().with_handshake_check(
            |_ctx: &crate::handshake::WsHandshakeContext<'_>| -> Result<(), fandhe_backend_http::response::Response> {
                unreachable!()
            },
        );
        let debug = format!("{config:?}");
        assert!(debug.contains("handshake_check: true"));
    }
}
