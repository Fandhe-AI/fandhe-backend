//! コアループ（接続受理・リクエストループ）と 3 拡張点の実接続（TASK-1.4-2 / #70）。
//!
//! [`extension`][crate::extension] モジュールが定義する `Middleware` /
//! `UpgradeHandler` / `RequestGate` は trait 定義のみであり、実際に接続を
//! 受理してこれらを呼び出すのは本モジュールの責務。[`Server`] がビルダーとして
//! 拡張点実装（`Box<dyn ...>`）を保持し、[`Server::bind`] で得た [`BoundServer`]
//! の [`BoundServer::run`] が accept ループを回して 1 接続ごとに
//! [`handle_connection`] を spawn する。同時接続数は
//! `DEFAULT_MAX_CONNECTIONS`（[`Server::max_connections`] で変更可）を
//! 上限とし、リソース枯渇 DoS を防ぐ（`.claude/rules/security.md`）。
//!
//! # コアループ本体は feature で分岐しない
//!
//! `handle_connection` 内に `#[cfg(feature = "...")]` を一切持たない
//! （`docs/spec/03-poc` PoC-3 の設計規約）。プラグインの介入余地は一定
//! シグネチャの 2 種のヘルパーに閉じる:
//! - `plugin::try_handle_upgrade`（非公開 `plugin` モジュール、TASK-4.1 / #22）:
//!   長時間接続（WebSocket 等）への委譲。`websocket` feature 有効時は
//!   `bf_plugin_websocket` へ完全委譲し、無効時は常に `Some(stream)` を返す
//!   スタブ挙動を維持する
//! - `plugin::try_intercept`（非公開 `plugin` モジュール）: リクエスト/
//!   レスポンス完結型プラグイン（WebRTC シグナリングプロキシ等）へのパス
//!   インターセプト。TASK-2.1（#18）で確立した feature flag + `dep:` 構文の
//!   プラグイン境界パターンの実装（`docs/design/plugin-boundary.md`）
//!
//! いずれも feature 分岐はヘルパー内部に閉じ、`handle_connection` 側は
//! ヘルパーのシグネチャを変えずに済む。
//!
//! `try_handle_upgrade`（本モジュール内の非公開シーム）は TASK-4.1（#22）で
//! `crate::plugin` モジュールへ移設し、`websocket` feature 有効時は
//! `bf_plugin_websocket` へ実委譲する実装に差し替えた（feature 無効時は
//! 従来どおり常に `Some(stream)` を返すスタブ挙動を維持し、
//! `handle_connection` 側は 501 応答を返す）。移設に伴いシグネチャを
//! `&[Box<dyn UpgradeHandler>]` から `Vec<u8>`（残余バイト列）+ `&Server`
//! （設定取得用）へ変更した。これは「シームのシグネチャを変えない」という
//! 本モジュールの設計規約からの意図的な逸脱であり、複数 Upgrade 型プラグイン
//! が設定を必要とする将来を見据え、`&Server` 経由で任意の cfg-gated 設定へ
//! アクセスできるようにするため（`crate::plugin::try_handle_upgrade` の doc・
//! `docs/design/plugin-boundary.md` 5 節を参照）。
//!
//! # 1 接続あたりの処理フロー
//!
//! ```text
//! loop {
//!   read_request（bf_http::connection、ヘッド + body 読了、タイムアウト付き）
//!     Ok(None)          → 正常クローズ
//!     Err(e)            → e に応じた 4xx/5xx（またはエラー応答なし）を返しクローズ
//!     Ok(Some(req)) →
//!       1. Middleware::on_request（登録順）
//!       2. RequestGate::check（登録順、最初の Reject を優先。フェイルクローズ）
//!       3. UpgradeHandler::matches（登録順。マッチしたら読み取りバッファを
//!          明示解放してから try_handle_upgrade へ委譲）
//!       4. plugin::try_intercept（パスインターセプト型プラグイン。
//!          Some(response) なら以降の Handler::handle をスキップ）
//!       5. Handler::handle（未登録時、または try_intercept が None の場合。
//!          未登録時は 404）
//!       6. レスポンス書き込み → Middleware::on_response
//!       7. should_keep_alive(head) が false なら接続を閉じる
//! }
//! ```
//!
//! `RequestGate` を `UpgradeHandler` より先に評価するのは、将来の hub
//! TenantGate（TASK-9.1）が WebSocket アップグレードも既定拒否でゲート
//! できるようにするため（フェイルクローズ、`docs/spec/04-requirements.md` REQ-9）。
//! 同じ理由で `plugin::try_intercept` も `RequestGate` より後（`UpgradeHandler`
//! の後）に評価し、ゲートの既定拒否がパスインターセプト型プラグインにも及ぶ
//! ようにする。

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::sync::Semaphore;

use bf_http::body::BodyError;
use bf_http::buffer::RecvBuffer;
use bf_http::connection::{RequestError, read_request, should_keep_alive};
use bf_http::request::{ParseError, RequestHead};
use bf_http::response::Response;

use crate::extension::{GateOutcome, Middleware, RequestGate, UpgradeHandler};

/// `read_request` 1 回あたりの読み取りタイムアウト（スロークライアント対策）。
///
/// ヘッド・body の読み取り待ち、および keep-alive 接続が次のリクエストを
/// 送ってくるまでのアイドル待ちの両方に同じ値を適用する。値は固定定数に
/// とどめ、チューニング可能化はサーバビルダー拡張の後続スコープとする
/// （`.claude/rules/security.md` のリソース枯渇対策）。
///
/// `READ_TIMEOUT` は「1 回の read 待ち」しか制限しないため、正当なタイムアウト
/// 間隔より短い間隔で送信し続けるクライアントに対しては単体で無力である。
/// この隙間は [`Server::max_connection_lifetime`]（接続の総生存期間上限）と
/// [`Server::max_requests_per_connection`]（keep-alive 中の最大リクエスト数）
/// で埋める（#70 レビュー指摘、`.claude/rules/security.md` のリソース枯渇観点）。
///
/// [`handle_connection`] は実際の read 待ちにこの定数をそのまま使わず、
/// 残り生存期間（`max_connection_lifetime - 経過時間`）とのうち短い方を使う
/// （#70 Bugbot 指摘）。これにより「生存期間チェックの直後に最大
/// `READ_TIMEOUT` だけ read がブロックし、その間 permit を握ったまま
/// 総生存期間を超過する」経路を塞ぐ。
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// 1 接続あたりの総生存期間の既定上限（リソース枯渇 DoS 対策）。
///
/// `READ_TIMEOUT` は「1 回の read 待ち」しか制限しないため、これより短い
/// 間隔で（例えば 1 バイトずつ）送信し続けるクライアントは、本上限がなければ
/// `DEFAULT_MAX_CONNECTIONS` の permit を無期限に占有できてしまう。
/// [`handle_connection`] はコネクション開始時刻からの経過時間がこの値に達した
/// 時点で（読み取り待ちに入る前に）接続を閉じ、permit を解放する。
/// 値のチューニングは [`Server::max_connection_lifetime`] で行う。
const DEFAULT_MAX_CONNECTION_LIFETIME: Duration = Duration::from_secs(300);

/// keep-alive 接続 1 本あたりに処理を許すリクエスト数の既定上限（リソース枯渇 DoS 対策）。
///
/// `DEFAULT_MAX_CONNECTION_LIFETIME` とは独立に、短時間に大量の軽量リクエストを
/// 送り続けて 1 接続でハンドラ処理を占有し続ける経路を塞ぐ。上限に達した
/// リクエストへの応答は `Connection: close` を伴い、以後 [`handle_connection`] は
/// 同じ接続で次のリクエストを待たない。値のチューニングは
/// [`Server::max_requests_per_connection`] で行う。
const DEFAULT_MAX_REQUESTS_PER_CONNECTION: usize = 1_000;

/// 同時接続数の既定上限（リソース枯渇 DoS 対策）。
///
/// accept ループが際限なく `tokio::spawn` すると、`READ_TIMEOUT` による
/// 1 接続あたりのスロークライアント対策があっても、大量の同時接続による
/// fd・メモリ消費（リソース枯渇 DoS）は防げない（`.claude/rules/security.md`）。
/// [`BoundServer::run`] はこの上限を `tokio::sync::Semaphore` で強制し、
/// 上限に達している間は新規 `accept` 自体を保留する（カーネルの listen
/// backlog に滞留させ、あふれた分は OS 側で拒否させるフェイルクローズ設計）。
/// 値のチューニングは [`Server::max_connections`] で行う。
const DEFAULT_MAX_CONNECTIONS: usize = 10_000;

/// `listener.accept()` がエラーを返した際、次の accept 試行までの待機時間。
///
/// EMFILE/ENFILE（fd 枯渇）のように accept エラーが連続しうる状況で、
/// 待機なしにループし続けると CPU を専有するビジーループになる
/// （`.claude/rules/security.md` のリソース枯渇観点）。[`BoundServer::run`]
/// の doc を参照。
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(10);

/// リクエストに対する最終応答を生成する、コアが公開する既定ハンドラ拡張点。
///
/// 3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）とは異なり
/// 「拡張点は 3 種に集約」の対象ではなく、ルーティング結果を最終応答へ
/// 変換する既定レスポンダの差し込み口という位置づけ。`bf_routes::Router`
/// （TASK-1.5 / #14、下記 `impl Handler for bf_routes::Router` 参照）を
/// 直接登録できるほか、トイハンドラ・テスト用の固定レスポンダ等の任意実装も
/// 引き続き受け付ける。
pub trait Handler: Send + Sync {
    /// リクエストヘッドと body からレスポンスを組み立てる。
    fn handle(&self, head: &RequestHead, body: &[u8]) -> Response;
}

/// `bf_routes::Router`（依存方向 `server → routes → http::*` の実体化、
/// TASK-1.5 / #14）をそのままコアの既定ハンドラとして登録できるようにする。
///
/// [`Router::dispatch`][bf_routes::Router::dispatch] へ委譲するだけの薄い
/// アダプタであり、ルーティングの意味論（method + target 完全一致・
/// 404/405 のフェイルクローズ）は `crates/routes` 側の責務のまま変わらない。
impl Handler for bf_routes::Router {
    fn handle(&self, head: &RequestHead, body: &[u8]) -> Response {
        self.dispatch(head, body)
    }
}

/// 3 拡張点・既定ハンドラを登録するビルダー。
///
/// 各登録メソッドは `self` を消費して返すため、メソッドチェーンで組み立てる。
/// [`Server::bind`] を呼ぶと以降は不変（`Arc<Server>`）として複数コネクション
/// タスクから共有参照される。
///
/// ```
/// use backend_framework_core::server::Server;
///
/// let server = Server::new();
/// // bind() はソケットを開くため doctest では実行しない（`no_run` 相当）。
/// // 実際の起動例は crates/core/examples/minimal.rs を参照。
/// let _ = server;
/// ```
pub struct Server {
    middlewares: Vec<Box<dyn Middleware>>,
    gates: Vec<Box<dyn RequestGate>>,
    upgrade_handlers: Vec<Box<dyn UpgradeHandler>>,
    handler: Option<Box<dyn Handler>>,
    max_connections: usize,
    max_connection_lifetime: Duration,
    max_requests_per_connection: usize,
    /// `webrtc-proxy` feature（TASK-2.1 / #18）有効時のみ意味を持つ設定。
    /// `crate::plugin::try_intercept` がこのフィールドを参照して `POST
    /// /rtc/offer` を上流へ中継するかどうかを判定する。feature 無効時は
    /// フィールド自体が構造体から消え、依存・コードともゼロコストになる
    /// （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。
    #[cfg(feature = "webrtc-proxy")]
    webrtc_proxy_config: Option<bf_plugin_webrtc_proxy::ProxyConfig>,
    /// `websocket` feature（TASK-4.1 / #22）有効時のみ意味を持つ設定群。
    /// `crate::plugin::try_handle_upgrade` がこのフィールドを参照して
    /// `UpgradeHandler` 委譲成立後に `bf_plugin_websocket::handle_upgrade` へ
    /// 渡す。`Server::websocket` を複数回呼ぶと複数パスを登録でき、
    /// 登録順に `bf_plugin_websocket::matches` を評価して最初に一致した
    /// 設定を使う（`upgrade_handlers` 側の `WebSocketUpgradeAdapter` も
    /// 同じ登録順で `matches` するため、両者は常に整合する）。単一
    /// `Option` だと 2 回目の呼び出しで 1 回目の設定が上書きされ、最初に
    /// 登録したパスへのアップグレードが 501 になる不整合が生じるため
    /// `Vec` として保持する。feature 無効時はフィールド自体が構造体から
    /// 消え、依存・コードともゼロコストになる（pay-for-what-you-use、
    /// .claude/rules/pay-for-what-you-use.md）。
    #[cfg(feature = "websocket")]
    websocket_configs: Vec<bf_plugin_websocket::WebSocketConfig>,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            middlewares: Vec::new(),
            gates: Vec::new(),
            upgrade_handlers: Vec::new(),
            handler: None,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_connection_lifetime: DEFAULT_MAX_CONNECTION_LIFETIME,
            max_requests_per_connection: DEFAULT_MAX_REQUESTS_PER_CONNECTION,
            #[cfg(feature = "webrtc-proxy")]
            webrtc_proxy_config: None,
            #[cfg(feature = "websocket")]
            websocket_configs: Vec::new(),
        }
    }
}

impl Server {
    /// 拡張点・ハンドラを 1 件も持たない空の [`Server`] を作る。
    /// 同時接続数上限は `DEFAULT_MAX_CONNECTIONS`。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 同時接続数の上限を設定する（既定 `DEFAULT_MAX_CONNECTIONS`）。
    ///
    /// [`BoundServer::run`] の accept ループはこの上限に達している間、
    /// 新規接続の受理を保留する（リソース枯渇 DoS 対策、本モジュール冒頭の
    /// doc・`DEFAULT_MAX_CONNECTIONS` の doc を参照）。`0` を指定した場合は
    /// accept ループが永久に許可待ちでブロックし新規接続を一切受理できなく
    /// なるため、[`Server::bind`] 側で最低 `1` に切り上げる。
    #[must_use]
    pub fn max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// 1 接続あたりの総生存期間の上限を設定する（既定 `DEFAULT_MAX_CONNECTION_LIFETIME`）。
    ///
    /// [`handle_connection`] はコネクション開始からの経過時間がこの値に達すると、
    /// 次のリクエストの読み取り待ちに入る前に接続を閉じる（`READ_TIMEOUT` の
    /// doc・`.claude/rules/security.md` のリソース枯渇観点を参照）。`Duration::ZERO`
    /// を指定すると最初のリクエストを読む前に接続を閉じてしまうため、実運用では
    /// 避けること。
    #[must_use]
    pub fn max_connection_lifetime(mut self, max: Duration) -> Self {
        self.max_connection_lifetime = max;
        self
    }

    /// keep-alive 接続 1 本あたりに処理を許すリクエスト数の上限を設定する
    /// （既定 `DEFAULT_MAX_REQUESTS_PER_CONNECTION`）。
    ///
    /// 上限に達したリクエストへの応答は `Connection: close` を伴い、以後
    /// [`handle_connection`] は同じ接続で次のリクエストを待たない
    /// （`READ_TIMEOUT` の doc・`.claude/rules/security.md` のリソース枯渇観点を
    /// 参照）。`0` を指定した場合でも最低 1 リクエストは処理してから閉じる
    /// （[`handle_connection`] 側で `.max(1)` に切り上げる）。
    #[must_use]
    pub fn max_requests_per_connection(mut self, max: usize) -> Self {
        self.max_requests_per_connection = max;
        self
    }

    /// [`Middleware`] を登録する（登録順に `on_request` / `on_response` が呼ばれる）。
    #[must_use]
    pub fn middleware(mut self, middleware: impl Middleware + 'static) -> Self {
        self.middlewares.push(Box::new(middleware));
        self
    }

    /// [`RequestGate`] を登録する（登録順に評価し、最初の `Reject` を優先する）。
    #[must_use]
    pub fn gate(mut self, gate: impl RequestGate + 'static) -> Self {
        self.gates.push(Box::new(gate));
        self
    }

    /// [`UpgradeHandler`] を登録する（登録順に `matches` を評価する）。
    #[must_use]
    pub fn upgrade_handler(mut self, handler: impl UpgradeHandler + 'static) -> Self {
        self.upgrade_handlers.push(Box::new(handler));
        self
    }

    /// 既定ハンドラ（[`Handler`]）を登録する。未登録時は 404 を返す。
    #[must_use]
    pub fn handler(mut self, handler: impl Handler + 'static) -> Self {
        self.handler = Some(Box::new(handler));
        self
    }

    /// WebRTC シグナリングプロキシプラグイン（`crates/plugin-webrtc-proxy`）を
    /// 有効化する（`webrtc-proxy` feature 限定 API、TASK-2.1 / #18）。
    ///
    /// 登録すると `POST /rtc/offer` が `RequestGate` → `UpgradeHandler` の
    /// 評価を通過した後、既定 [`Handler`] より先にパスインターセプトされ、
    /// `config` が指す上流 WebRTC サービスへ中継される（対象外パスは素通り
    /// し、既定 `Handler` へフォールスルーする。`crate::plugin::try_intercept`
    /// の doc を参照）。
    #[cfg(feature = "webrtc-proxy")]
    #[must_use]
    pub fn webrtc_proxy(mut self, config: bf_plugin_webrtc_proxy::ProxyConfig) -> Self {
        self.webrtc_proxy_config = Some(config);
        self
    }

    /// `plugin::try_intercept` が参照する、登録済み WebRTC プロキシ設定
    /// （`webrtc-proxy` feature 限定、TASK-2.1 / #18）。
    #[cfg(feature = "webrtc-proxy")]
    pub(crate) fn webrtc_proxy_config(&self) -> Option<&bf_plugin_webrtc_proxy::ProxyConfig> {
        self.webrtc_proxy_config.as_ref()
    }

    /// WebSocket プラグイン（`crates/plugin-websocket`）を有効化する
    /// （`websocket` feature 限定 API、TASK-4.1 / #22）。
    ///
    /// 登録すると `config.path`（既定 `/ws`）への `GET` + `Upgrade: websocket`
    /// リクエストが `RequestGate` の評価を通過した後、
    /// [`UpgradeHandler`] 拡張点経由で検知
    /// され（`WebSocketUpgradeAdapter` を内部で自動登録する）、
    /// `crate::plugin::try_handle_upgrade` が
    /// `bf_plugin_websocket::handle_upgrade` へ完全委譲する
    /// （REQ-4「コア自身の HTTP パーサでアップグレードを検知し既存拡張点
    /// 経由で委譲する」という建て付けを維持する。`crates/plugin-websocket/src/lib.rs`
    /// の doc を参照）。異なる `path` で複数回呼び出すと複数パスを登録
    /// できる（`websocket_configs()` の doc を参照）。
    #[cfg(feature = "websocket")]
    #[must_use]
    pub fn websocket(mut self, config: bf_plugin_websocket::WebSocketConfig) -> Self {
        self.upgrade_handlers
            .push(Box::new(WebSocketUpgradeAdapter {
                config: config.clone(),
            }));
        self.websocket_configs.push(config);
        self
    }

    /// `crate::plugin::try_handle_upgrade` が参照する、登録済み WebSocket
    /// 設定群（`websocket` feature 限定、TASK-4.1 / #22）。
    ///
    /// `Server::websocket` を呼んだ順に格納されており、`upgrade_handlers`
    /// 内の `WebSocketUpgradeAdapter` の登録順と一致する。呼び出し元は
    /// 登録順に `bf_plugin_websocket::matches` を評価し、最初に一致した
    /// 設定を使うこと（複数パス登録時に先に登録したパスが後の登録で
    /// 上書きされて失われないようにするための契約）。
    #[cfg(feature = "websocket")]
    pub(crate) fn websocket_configs(&self) -> &[bf_plugin_websocket::WebSocketConfig] {
        &self.websocket_configs
    }

    /// `addr` に TCP リスナーをバインドし、[`BoundServer`] を返す。
    ///
    /// 以降 `self` は `Arc` で包まれ、accept したコネクションタスク間で
    /// 共有される（拡張点実装は `Send + Sync` を要求される理由）。
    pub async fn bind(self, addr: impl ToSocketAddrs) -> io::Result<BoundServer> {
        let listener = TcpListener::bind(addr).await?;
        // 0 を指定すると accept ループが永久に許可待ちでブロックし、
        // 新規接続を一切受理できなくなる（誤用によるデッドロック防止のため
        // 最低 1 に切り上げる）。
        let connection_limit = Arc::new(Semaphore::new(self.max_connections.max(1)));
        Ok(BoundServer {
            listener,
            server: Arc::new(self),
            connection_limit,
        })
    }
}

/// [`Server::websocket`] が内部登録する [`UpgradeHandler`] アダプタ
/// （`websocket` feature 限定、TASK-4.1 / #22）。
///
/// `UpgradeHandler::matches` は「委譲判定のみ」の契約（同期 API、
/// `crates/core/src/extension.rs` の doc）のため、本アダプタは
/// `bf_plugin_websocket::matches`（純関数）を呼ぶだけの薄い委譲先とし、
/// 実際のハンドシェイク検証・フレーミング委譲（非同期処理）は
/// `crate::plugin::try_handle_upgrade` → `bf_plugin_websocket::handle_upgrade`
/// が担う。`config` は `Server::websocket` 呼び出し時にクローンして保持する
/// （`upgrade_handlers: Vec<Box<dyn UpgradeHandler>>` は `Server` 本体と
/// ライフタイムを共有しないため、参照ではなく所有値として持つ）。
#[cfg(feature = "websocket")]
struct WebSocketUpgradeAdapter {
    config: bf_plugin_websocket::WebSocketConfig,
}

#[cfg(feature = "websocket")]
impl UpgradeHandler for WebSocketUpgradeAdapter {
    fn name(&self) -> &'static str {
        "websocket"
    }

    fn matches(&self, head: &RequestHead) -> bool {
        bf_plugin_websocket::matches(head, &self.config)
    }
}

/// [`Server::bind`] が返す、リスニングソケットを保持した状態のサーバ。
pub struct BoundServer {
    listener: TcpListener,
    server: Arc<Server>,
    /// 同時接続数の上限を強制するセマフォ（`DEFAULT_MAX_CONNECTIONS` の doc を参照）。
    /// permit は [`BoundServer::run`] が spawn するコネクションタスクへ move し、
    /// タスク終了（`handle_connection` の戻り）時に自動で解放される。
    connection_limit: Arc<Semaphore>,
}

impl BoundServer {
    /// バインドしたローカルアドレスを返す。`0` ポート指定時の実ポート確認に使う。
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// accept ループを回し、コネクションごとに [`handle_connection`] を spawn する。
    ///
    /// 各コネクションは独立した tokio タスクで処理されるため、1 接続の
    /// 処理停滞（スロークライアント等）が他接続をブロックしない。
    ///
    /// 同時接続数は `connection_limit` セマフォで上限を強制する。上限に
    /// 達している間は `accept` 自体を呼ばずに待機するため、あふれた接続は
    /// カーネルの listen backlog に滞留し、backlog も尽きれば OS 側で
    /// 拒否される（`DEFAULT_MAX_CONNECTIONS` の doc を参照）。
    ///
    /// # accept エラーの扱い（可用性）
    ///
    /// `listener.accept()` が返すエラー（例: `ECONNABORTED` = accept 前の
    /// クライアント切断、`EMFILE`/`ENFILE` = fd 枯渇）は、リスナー自体が
    /// 壊れたことを意味しない一過性のものが大半である。Tokio 公式ドキュメント
    /// （`TcpListener::accept`）も「多くの accept エラーはサーバ全体ではなく
    /// 個々の接続に紐づくものであり、ログに残してループを継続するのが
    /// 一般的な実践」と述べている。そのため本実装は accept エラーで `run()`
    /// を終了させず、`ACCEPT_ERROR_BACKOFF` だけ待ってから次の accept を
    /// 再試行する（`.claude/rules/security.md` の可用性・リソース枯渇観点。
    /// 1 件の一過性エラーでリスナー全体が永久停止するのを防ぐ）。戻り値が
    /// `io::Result` なのは将来の呼び出し側都合による API 安定性のためであり、
    /// 現状の実装は（プロセス終了等の外的要因を除き）`Err` を返さず走り続ける。
    pub async fn run(self) -> io::Result<()> {
        loop {
            // セマフォが閉じられることはない（`close()` を呼ぶ経路がない）ため
            // `acquire_owned` は必ず成功する。
            let permit = Arc::clone(&self.connection_limit)
                .acquire_owned()
                .await
                .expect("connection_limit semaphore is never closed");

            let stream = match self.listener.accept().await {
                Ok((stream, _peer_addr)) => stream,
                Err(err) => {
                    // permit はここで（スコープを抜けると同時に）解放され、
                    // 次のループ先頭で再取得される。上の doc を参照。
                    drop(permit);
                    eprintln!("backend_framework_core::server: accept に失敗しました: {err}");
                    tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                    continue;
                }
            };
            let server = Arc::clone(&self.server);
            tokio::spawn(async move {
                handle_connection(&server, stream).await;
                drop(permit);
            });
        }
    }
}

/// 1 コネクション分の keep-alive ループ本体。
///
/// `S` を [`AsyncRead`] + [`AsyncWrite`] にジェネリック化しているのは、
/// 実ソケット（[`TcpStream`]）だけでなく `tokio::io::duplex` を使った
/// ソケット不要の統合テストを可能にするため（AI ファースト保守性、
/// `.claude/rules/coding-rust.md`）。
///
/// 接続単位で読み取りバッファ `buf` を 1 本だけ確保し、`bf_http::connection`
/// のパイプライン契約（未消費の残余バイトを `buf` に残す）に従って
/// 繰り返し `read_request` を呼ぶ。
///
/// 本関数の中に `#[cfg(feature = "...")]` を一切持たない（本モジュール冒頭の
/// doc を参照）。
pub async fn handle_connection<S>(server: &Server, mut stream: S)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = RecvBuffer::new();
    // 接続の総生存期間・keep-alive 中の総リクエスト数を計測する（#70 レビュー
    // 指摘、`.claude/rules/security.md` のリソース枯渇観点。READ_TIMEOUT の
    // doc・Server::max_connection_lifetime / max_requests_per_connection の
    // doc を参照）。
    let connection_started_at = Instant::now();
    let mut requests_served: usize = 0;
    // 0 を指定しても最低 1 リクエストは処理してから閉じる
    // （Server::max_requests_per_connection の doc を参照）。
    let max_requests = server.max_requests_per_connection.max(1);

    loop {
        // 次のリクエストを読みに行く前に総生存期間の上限を確認する。これにより
        // READ_TIMEOUT より短い間隔で送信し続けるクライアントであっても、
        // 接続が上限を超えて permit を占有し続けることはない。
        let elapsed_since_start = connection_started_at.elapsed();
        if elapsed_since_start >= server.max_connection_lifetime {
            return;
        }

        // read_request のタイムアウトは READ_TIMEOUT と「残り生存期間」の
        // 短い方に丸める（#70 Bugbot 指摘: READ_TIMEOUT をそのまま使うと、
        // 直前の生存期間チェックを通過した直後に読み取りが最大 READ_TIMEOUT
        // だけブロックし、permit を握ったまま接続が max_connection_lifetime を
        // 超過しうる）。これにより 1 回の read 待ちで接続が生存期間上限を
        // 超えて居座ることはなく、超過前に必ずタイムアウトして接続が閉じる
        // （下の `Err(_elapsed) => return` 分岐）。
        let remaining_lifetime = server
            .max_connection_lifetime
            .saturating_sub(elapsed_since_start);
        let read_timeout = READ_TIMEOUT.min(remaining_lifetime);

        let read_result =
            tokio::time::timeout(read_timeout, read_request(&mut stream, &mut buf)).await;

        let request = match read_result {
            // タイムアウト（スロークライアント・keep-alive アイドル超過）:
            // 応答を送らず接続を閉じる。
            Err(_elapsed) => return,
            Ok(Err(err)) => {
                if let Some(response) = error_response(&err) {
                    // エラー応答は常に接続クローズ（フェイルセーフ）。
                    let _ = stream.write_all(&response.serialize(false)).await;
                }
                return;
            }
            // buf が空の状態での EOF は正常なコネクション終了。
            Ok(Ok(None)) => return,
            Ok(Ok(Some(request))) => request,
        };

        requests_served += 1;

        let started_at = Instant::now();
        for middleware in &server.middlewares {
            middleware.on_request(&request.head);
        }

        // クライアントが keep-alive を要求していても、総リクエスト数上限に
        // 達した場合・総生存期間上限に達した場合は `Connection: close` で
        // 応答し、この接続では次のリクエストを待たない（#70 レビュー指摘、
        // Server::max_requests_per_connection / max_connection_lifetime の
        // doc を参照）。
        let keep_alive = should_keep_alive(&request.head)
            && requests_served < max_requests
            && connection_started_at.elapsed() < server.max_connection_lifetime;

        // RequestGate はルーティング・アップグレードより先に評価する
        // （フェイルクローズ、モジュール冒頭の doc を参照）。
        if let Some(rejection) = first_rejection(&server.gates, &request.head) {
            let GateOutcome::Reject { status, body } = rejection else {
                unreachable!("first_rejection only returns Reject outcomes")
            };
            let response = Response::new(status, body);
            if stream
                .write_all(&response.serialize(keep_alive))
                .await
                .is_err()
            {
                return;
            }
            for middleware in &server.middlewares {
                middleware.on_response(&request.head, started_at.elapsed());
            }
            if !keep_alive {
                return;
            }
            continue;
        }

        if server
            .upgrade_handlers
            .iter()
            .any(|handler| handler.matches(&request.head))
        {
            // 長時間接続へ委譲する前に、パイプライン済みの可能性がある残余
            // バイト列（次リクエストの先頭・WebSocket の先行フレーム等）を
            // 退避してからコア側の読み取りバッファを明示的に解放する
            // （Conditional Go 条件 (1)）。`RecvBuffer` は縮小 API を
            // `pub(crate)` にしか公開しない（TASK-1.3-3 / #68）ため、`drop`
            // で旧バッファ（確保済み容量ごと）を丸ごと解放する。以降このループ
            // 反復では `buf` を読まない（両分岐とも `return` する）ため、
            // 代入ではなく明示的な `drop` で意図を示す。`leftover` は
            // `bf_plugin_websocket::handle_upgrade` が
            // `WebSocketStream::from_partially_read` へそのまま渡す
            // （TASK-4.1 / #22、先行到着フレームを取りこぼさないため）。
            let leftover = buf.unread().to_vec();
            drop(buf);
            match crate::plugin::try_handle_upgrade(stream, &request.head, leftover, server).await {
                Some(mut stream) => {
                    // websocket feature 無効時、または `UpgradeHandler` が
                    // マッチしたのに対応する Upgrade 型プラグインが未登録の
                    // 場合、委譲先がない状態を黙って落とさず 501 で明示的に
                    // 拒否する（本モジュール冒頭の doc・
                    // `crate::plugin::try_handle_upgrade` の doc を参照）。
                    // on_response は「委譲時は呼ばない」契約のため呼ばない
                    // （この 501 応答は委譲失敗のフォールバックであり実処理の
                    // 完了ではないため）。結果として on_request は呼ばれるが
                    // 対になる on_response が呼ばれない非対称が生じる点は
                    // 意図的な仕様であり、Middleware 実装側は「on_request が
                    // 必ず on_response を伴う」と仮定しないこと。
                    let _ = stream
                        .write_all(&Response::empty(501).serialize(false))
                        .await;
                    return;
                }
                None => return,
            }
        }

        // パスインターセプト型プラグイン（TASK-2.1 / #18）は既定 Handler より
        // 先に評価する。`try_intercept` が `Some` を返した場合はプラグインが
        // 処理を完結させたことを意味し、既定 Handler は呼ばない
        // （モジュール冒頭の処理フロー doc・`crate::plugin::try_intercept` の
        // doc を参照）。
        let response =
            match crate::plugin::try_intercept(server, &request.head, &request.body).await {
                Some(response) => response,
                None => match &server.handler {
                    Some(handler) => handler.handle(&request.head, &request.body),
                    None => Response::empty(404),
                },
            };

        // #70 Bugbot 指摘（Stale keep-alive after lifetime）: 上の `keep_alive` は
        // `Handler::handle` 呼び出し前、`on_request` 直後の経過時間で決定している。
        // `handle` の処理時間が長引き、その間に `max_connection_lifetime` を
        // 超過した場合、`keep_alive` が古いまま `true` の応答を送ると
        // 「生存期間超過時は応答で `Connection: close` を返す」という契約
        // （上のコメント・`Server::max_connection_lifetime` の doc を参照）に
        // 反する。レスポンス生成直後・送信直前に生存期間のみ再チェックし、
        // 超過していれば `Connection: close` を確実に付与する
        // （`should_keep_alive` と `requests_served` 側の判定は `handle` の
        // 呼び出しで変化しないため再評価不要）。
        let keep_alive =
            keep_alive && connection_started_at.elapsed() < server.max_connection_lifetime;

        if stream
            .write_all(&response.serialize(keep_alive))
            .await
            .is_err()
        {
            return;
        }
        for middleware in &server.middlewares {
            middleware.on_response(&request.head, started_at.elapsed());
        }

        if !keep_alive {
            return;
        }
    }
}

/// 登録順に `gates` を評価し、最初の [`GateOutcome::Reject`] を返す。
/// 全件 `Allow` の場合は `None`。
fn first_rejection(gates: &[Box<dyn RequestGate>], head: &RequestHead) -> Option<GateOutcome> {
    gates.iter().find_map(|gate| match gate.check(head) {
        GateOutcome::Allow => None,
        reject @ GateOutcome::Reject { .. } => Some(reject),
    })
}

/// [`RequestError`] を応答すべき HTTP ステータスへマッピングする。
///
/// `None` はエラー応答を送らず接続を閉じるべきケース（途中 EOF・I/O エラー）
/// を意味する。マッピング根拠は本モジュール冒頭の doc・実装計画のセキュリティ
/// 考慮（入力検証の全面依拠、フェイルセーフなクローズ）を参照。
fn error_response(err: &RequestError) -> Option<Response> {
    match err {
        RequestError::Parse(ParseError::HeaderSectionTooLarge | ParseError::TooManyHeaders) => {
            Some(Response::empty(431))
        }
        RequestError::Parse(ParseError::InvalidRequestLine | ParseError::InvalidHeader) => {
            Some(Response::empty(400))
        }
        RequestError::Parse(ParseError::UnsupportedVersion) => Some(Response::empty(505)),
        RequestError::Body(BodyError::BodyTooLarge) => Some(Response::empty(413)),
        RequestError::Body(BodyError::TransferEncodingUnsupported) => Some(Response::empty(501)),
        RequestError::Body(BodyError::DuplicateContentLength | BodyError::InvalidContentLength) => {
            Some(Response::empty(400))
        }
        RequestError::UnexpectedEof | RequestError::Io(_) => None,
    }
}

// `TcpStream` を明示的に参照し、`handle_connection` の型パラメータ `S` が
// 実ソケットに対しても解決可能であること（`BoundServer::run` の呼び出しが
// 成立すること）をコンパイル時に保証する。
#[allow(dead_code)]
fn _assert_handle_connection_accepts_tcp_stream(
    server: &Server,
    stream: TcpStream,
) -> impl Future + '_ {
    handle_connection(server, stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{GateOutcome, Middleware, RequestGate, UpgradeHandler};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncReadExt;

    /// 呼び出し順・回数を記録するトイ `Middleware`。
    struct RecordingMiddleware {
        events: Mutex<Vec<&'static str>>,
    }

    impl Middleware for RecordingMiddleware {
        fn name(&self) -> &'static str {
            "recording"
        }
        fn on_request(&self, _head: &RequestHead) {
            self.events.lock().unwrap().push("on_request");
        }
        fn on_response(&self, _head: &RequestHead, _elapsed: Duration) {
            self.events.lock().unwrap().push("on_response");
        }
    }

    /// `Authorization` ヘッダ必須のフェイルクローズ `RequestGate`。
    struct RequireAuthGate;
    impl RequestGate for RequireAuthGate {
        fn name(&self) -> &'static str {
            "require-auth"
        }
        fn check(&self, head: &RequestHead) -> GateOutcome {
            match head.header("authorization") {
                Some(_) => GateOutcome::Allow,
                None => GateOutcome::Reject {
                    status: 401,
                    body: b"unauthorized".to_vec(),
                },
            }
        }
    }

    /// 常に拒否する `RequestGate`（複数 gate の「最初の Reject」テスト用）。
    struct AlwaysRejectGate(u16);
    impl RequestGate for AlwaysRejectGate {
        fn name(&self) -> &'static str {
            "always-reject"
        }
        fn check(&self, _head: &RequestHead) -> GateOutcome {
            GateOutcome::Reject {
                status: self.0,
                body: Vec::new(),
            }
        }
    }

    struct AllowAllGate;
    impl RequestGate for AllowAllGate {
        fn name(&self) -> &'static str {
            "allow-all"
        }
        fn check(&self, _head: &RequestHead) -> GateOutcome {
            GateOutcome::Allow
        }
    }

    /// `Upgrade: websocket` ヘッダにマッチするトイ `UpgradeHandler`。
    struct WebSocketUpgrade;
    impl UpgradeHandler for WebSocketUpgrade {
        fn name(&self) -> &'static str {
            "websocket"
        }
        fn matches(&self, head: &RequestHead) -> bool {
            head.header("upgrade")
                .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        }
    }

    /// 固定レスポンスを返すトイ `Handler`。
    struct FixedHandler {
        status: u16,
        body: &'static [u8],
        calls: AtomicUsize,
    }
    impl Handler for FixedHandler {
        fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Response::new(self.status, self.body.to_vec())
        }
    }

    async fn roundtrip(server: &Server, request: &[u8]) -> String {
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;
        client.write_all(request).await.unwrap();
        client.shutdown().await.unwrap();

        handle_connection(server, server_stream).await;

        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        String::from_utf8(out).unwrap()
    }

    #[tokio::test]
    async fn no_handler_registered_returns_404() {
        let server = Server::new();
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    #[tokio::test]
    async fn registered_handler_is_invoked() {
        let handler = FixedHandler {
            status: 200,
            body: b"hi",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("hi"));
    }

    /// TASK-1.5（#14）: `bf_routes::Router` を `Server::handler` にそのまま登録できる
    /// （`impl Handler for bf_routes::Router` の統合確認）。200・404・405 それぞれで
    /// ステータス行・`Content-Length`・body・`Connection: close` まで網羅的に検証する。
    #[tokio::test]
    async fn router_registered_as_handler_dispatches_by_method_and_target() {
        let router = bf_routes::Router::new().route("GET", "/", |_head, _body| {
            Response::new(200, b"root".to_vec())
        });
        let server = Server::new().handler(router);

        let ok = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(ok.contains("Content-Length: 4\r\n"));
        assert!(ok.contains("Connection: close\r\n"));
        assert!(ok.ends_with("root"));

        let missing = roundtrip(
            &server,
            b"GET /missing HTTP/1.1\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(missing.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(missing.contains("Content-Length: 0\r\n"));

        let wrong_method =
            roundtrip(&server, b"POST / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(wrong_method.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
        assert!(wrong_method.contains("Content-Length: 0\r\n"));
    }

    #[tokio::test]
    async fn middleware_hooks_fire_in_order_with_positive_elapsed() {
        let mw = Arc::new(RecordingMiddleware {
            events: Mutex::new(Vec::new()),
        });
        struct MwProxy(Arc<RecordingMiddleware>);
        impl Middleware for MwProxy {
            fn name(&self) -> &'static str {
                "proxy"
            }
            fn on_request(&self, head: &RequestHead) {
                self.0.on_request(head);
            }
            fn on_response(&self, head: &RequestHead, elapsed: Duration) {
                self.0.on_response(head, elapsed);
            }
        }

        let server = Server::new().middleware(MwProxy(Arc::clone(&mw)));
        let _ = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;

        let events = mw.events.lock().unwrap();
        assert_eq!(*events, vec!["on_request", "on_response"]);
    }

    #[tokio::test]
    async fn gate_rejects_missing_authorization_with_401() {
        let server = Server::new().gate(RequireAuthGate);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(response.ends_with("unauthorized"));
    }

    #[tokio::test]
    async fn gate_allows_request_with_authorization_to_reach_handler() {
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().gate(RequireAuthGate).handler(handler);
        let response = roundtrip(
            &server,
            b"GET / HTTP/1.1\r\nAuthorization: Bearer x\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[tokio::test]
    async fn first_gate_rejection_wins_over_later_gates() {
        let server = Server::new()
            .gate(AlwaysRejectGate(403))
            .gate(AlwaysRejectGate(401));
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
    }

    #[tokio::test]
    async fn allow_all_gate_does_not_block_handler() {
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().gate(AllowAllGate).handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[tokio::test]
    async fn upgrade_match_returns_501_and_closes_without_invoking_handler() {
        let handler = FixedHandler {
            status: 200,
            body: b"should-not-be-called",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .upgrade_handler(WebSocketUpgrade)
            .handler(handler);
        let response = roundtrip(
            &server,
            b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\nConnection: keep-alive\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 501 Not Implemented\r\n"));
    }

    #[tokio::test]
    async fn non_matching_upgrade_handler_falls_through_to_handler() {
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .upgrade_handler(WebSocketUpgrade)
            .handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[tokio::test]
    async fn keep_alive_connection_serves_two_requests_sequentially() {
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().handler(handler);
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;

        let write_task = tokio::spawn(async move {
            client
                .write_all(b"GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut out = Vec::new();
            client.read_to_end(&mut out).await.unwrap();
            out
        });

        handle_connection(&server, server_stream).await;
        let out = write_task.await.unwrap();
        let text = String::from_utf8(out).unwrap();

        assert_eq!(text.matches("HTTP/1.1 200 OK").count(), 2);
    }

    #[tokio::test]
    async fn max_requests_per_connection_forces_close_after_limit() {
        // keep-alive を要求し続けるクライアントでも、
        // max_requests_per_connection に達した時点で `Connection: close` に
        // 切り替わり、以後は同じ接続で次のリクエストを待たないことを確認する
        // （#70 レビュー指摘: keep-alive 中の総リクエスト数の上限）。
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .max_requests_per_connection(2)
            .handler(handler);
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;

        let write_task = tokio::spawn(async move {
            // 3 リクエストとも keep-alive を要求するが、上限は 2 件。
            client
                .write_all(
                    b"GET /a HTTP/1.1\r\n\r\n\
GET /b HTTP/1.1\r\n\r\n\
GET /c HTTP/1.1\r\n\r\n",
                )
                .await
                .unwrap();
            let mut out = Vec::new();
            client.read_to_end(&mut out).await.unwrap();
            out
        });

        handle_connection(&server, server_stream).await;
        let out = write_task.await.unwrap();
        let text = String::from_utf8(out).unwrap();

        // 2 件のみ応答され、3 件目は送られる前に接続が閉じられる。
        assert_eq!(text.matches("HTTP/1.1 200 OK").count(), 2);
        // 上限に達した最後の応答は Connection: close を伴う。
        assert!(text.contains("Connection: close"));
    }

    #[tokio::test]
    async fn max_connection_lifetime_closes_before_next_read() {
        // 総生存期間の上限に達した接続は、次のリクエストの読み取り待ちに
        // 入る前に閉じられることを確認する（#70 レビュー指摘: 接続の総生存
        // 期間の上限。READ_TIMEOUT では防げない、短い間隔で送信し続ける
        // クライアントによる permit 占有を防ぐ）。
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .max_connection_lifetime(Duration::from_millis(0))
            .handler(handler);
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;

        let write_task = tokio::spawn(async move {
            // 生存期間 0 のため handle_connection 側が読み取り前に接続を
            // 閉じている可能性があり、write が失敗しても構わない（無視する）。
            let _ = client.write_all(b"GET / HTTP/1.1\r\n\r\n").await;
            let mut out = Vec::new();
            let _ = client.read_to_end(&mut out).await;
            out
        });

        handle_connection(&server, server_stream).await;
        let out = write_task.await.unwrap();

        // 生存期間 0 のため、最初のリクエストすら読まれずに接続が閉じられる。
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn max_connection_lifetime_bounds_slow_read_below_read_timeout() {
        // #70 Bugbot 指摘: read_request のタイムアウトが常に READ_TIMEOUT
        // （30 秒）固定だと、直前の生存期間チェックを通過した直後にスロー
        // クライアントが read をブロックさせた場合、生存期間の上限
        // （本テストでは 50ms）を大幅に超えて（最大で READ_TIMEOUT 分）
        // permit を握り続けてしまう。修正後は残り生存期間で read タイムアウトを
        // 丸めるため、read がブロックしている接続も生存期間の上限近辺で
        // 閉じられることを確認する（30 秒の READ_TIMEOUT を待たない）。
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .max_connection_lifetime(Duration::from_millis(50))
            .handler(handler);
        let (client, server_stream) = tokio::io::duplex(8192);

        // クライアントは何も送らず接続だけ張り続ける（スロークライアント）。
        // read_request は永久にブロックしうるが、生存期間に基づく短縮
        // タイムアウトにより READ_TIMEOUT（30 秒）を待たずに閉じられるはず。
        let started = Instant::now();
        handle_connection(&server, server_stream).await;
        let elapsed = started.elapsed();
        drop(client);

        // READ_TIMEOUT（30 秒）よりも十分短い時間で接続が閉じられたことを
        // 確認する（CI 環境のスケジューリング遅延を許容しつつ、修正前の
        // 挙動（30 秒待ち）とは明確に区別できるしきい値）。
        assert!(
            elapsed < Duration::from_secs(5),
            "生存期間の上限近辺で閉じられるはずが {elapsed:?} かかった（READ_TIMEOUT 固定に戻っていないか確認）"
        );
    }

    /// `Handler::handle` の処理中に指定時間だけスリープしてから固定レスポンスを
    /// 返すトイ `Handler`（生存期間超過タイミングを `handle` の中に作るための道具）。
    struct SlowHandler {
        sleep_for: Duration,
    }
    impl Handler for SlowHandler {
        fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
            // `Handler::handle` は同期 API のため、テスト用に `std::thread::sleep`
            // で処理時間の長期化を模擬する（本体側 await ではないため
            // `.claude/rules/coding-rust.md` の「ブロッキング処理を await
            // スレッドで実行しない」に抵触しない。単体テストのみで使用）。
            std::thread::sleep(self.sleep_for);
            Response::new(200, b"ok".to_vec())
        }
    }

    #[tokio::test]
    async fn keep_alive_becomes_close_when_lifetime_expires_during_handle() {
        // #70 Bugbot 指摘（Stale keep-alive after lifetime）: keep_alive は
        // `on_request` 直後の経過時間だけで決めると、`Handler::handle` の処理が
        // 長引いて `max_connection_lifetime` を超えても応答が keep-alive の
        // ままになってしまう。修正後は `handle` 完了後に生存期間を再チェックし、
        // 超過していれば応答に `Connection: close` が付くことを確認する。
        let lifetime = Duration::from_millis(30);
        let handler = SlowHandler {
            // handle 呼び出しの前は生存期間内、handle 完了時には確実に
            // 超過しているように、生存期間よりわずかに長くスリープする。
            sleep_for: lifetime + Duration::from_millis(50),
        };
        let server = Server::new()
            .max_connection_lifetime(lifetime)
            .handler(handler);

        // keep-alive を要求する HTTP/1.1 リクエスト（Connection ヘッダなしは
        // HTTP/1.1 既定で keep-alive）。
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(
            response.contains("Connection: close"),
            "handle 中に生存期間を超過したにもかかわらず keep-alive のまま応答した: {response:?}"
        );
    }

    #[tokio::test]
    async fn invalid_request_line_returns_400() {
        let server = Server::new();
        let response = roundtrip(&server, b"G@T / HTTP/1.1\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    }

    #[tokio::test]
    async fn too_many_headers_returns_431() {
        let server = Server::new();
        let mut request = b"GET / HTTP/1.1\r\n".to_vec();
        for i in 0..200 {
            request.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
        }
        request.extend_from_slice(b"\r\n");
        let response = roundtrip(&server, &request).await;
        assert!(response.starts_with("HTTP/1.1 431 Request Header Fields Too Large\r\n"));
    }

    #[tokio::test]
    async fn body_too_large_returns_413() {
        let server = Server::new();
        let response = roundtrip(
            &server,
            b"POST / HTTP/1.1\r\nContent-Length: 999999999999\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"));
    }

    #[tokio::test]
    async fn transfer_encoding_returns_501() {
        let server = Server::new();
        let response = roundtrip(
            &server,
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 501 Not Implemented\r\n"));
    }

    #[tokio::test]
    async fn immediate_eof_closes_without_response() {
        let server = Server::new();
        let (client, server_stream) = tokio::io::duplex(64);
        drop(client);
        // panic しないことのみを確認する（正常クローズであり応答は送らない）。
        handle_connection(&server, server_stream).await;
    }

    #[tokio::test]
    async fn max_connections_limits_concurrent_accept() {
        use tokio::time::timeout;

        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().max_connections(1).handler(handler);
        let bound = server.bind("127.0.0.1:0").await.unwrap();
        let addr = bound.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = bound.run().await;
        });

        // client1 は何も送らず接続を張ったままにし、唯一の permit を占有する
        // （handle_connection が read_request の最初の読み取りで待機し続ける）。
        let client1 = TcpStream::connect(addr).await.unwrap();

        // client2 はリクエストを送るが、permit が枯渇しているため run() の
        // accept ループがまだこの接続を受理していないはずで、応答は来ない。
        let mut client2 = TcpStream::connect(addr).await.unwrap();
        client2
            .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut probe = [0u8; 1];
        let no_response_yet = timeout(Duration::from_millis(200), client2.read(&mut probe)).await;
        assert!(
            no_response_yet.is_err(),
            "max_connections が守られていれば client2 はまだ応答を受け取らないはず"
        );

        // client1 を閉じて permit を解放する。
        drop(client1);

        // run() が permit を取得して client2 を受理・処理するのを待つ。
        let mut out = Vec::new();
        timeout(Duration::from_secs(2), client2.read_to_end(&mut out))
            .await
            .expect("permit 解放後は client2 が受理されるはず")
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[test]
    fn bound_server_run_accepts_tcp_stream_type() {
        // `_assert_handle_connection_accepts_tcp_stream` がコンパイルできることが、
        // `handle_connection` が実ソケット（TcpStream）に対しても型解決できる
        // ことの静的な証跡になる。
        fn _type_check() {
            let _ = _assert_handle_connection_accepts_tcp_stream;
        }
    }
}
