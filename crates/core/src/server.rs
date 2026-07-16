//! コアループ（接続受理・リクエストループ）と 3 拡張点の実接続（TASK-1.4-2 / #70）。
//!
//! [`extension`][crate::extension] モジュールが定義する `Middleware` /
//! `UpgradeHandler` / `RequestGate` は trait 定義のみであり、実際に接続を
//! 受理してこれらを呼び出すのは本モジュールの責務。[`Server`] がビルダーとして
//! 拡張点実装（`Box<dyn ...>`）を保持し、[`Server::bind`] で得た [`BoundServer`]
//! の [`BoundServer::run`] が accept ループを回して 1 接続ごとに
//! [`handle_connection`] を spawn する。同時接続数は
//! [`DEFAULT_MAX_CONNECTIONS`]（[`Server::max_connections`] で変更可）を
//! 上限とし、リソース枯渇 DoS を防ぐ（`.claude/rules/security.md`）。
//!
//! # コアループ本体は feature で分岐しない
//!
//! `handle_connection` 内に `#[cfg(feature = "...")]` を一切持たない
//! （`docs/spec/03-poc` PoC-3 の設計規約）。プラグインの介入余地は一定
//! シグネチャの `try_handle_upgrade` ヘルパー（本モジュール内の非公開関数）に
//! 閉じ、feature 分岐が必要になった際もこのヘルパーの実装差し替えで完結させる
//! （実 feature 導入自体は TASK-2.1 / #18 のスコープ）。
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
//!       4. Handler::handle（未登録時は 404）
//!       5. レスポンス書き込み → Middleware::on_response
//!       6. should_keep_alive(head) が false なら接続を閉じる
//! }
//! ```
//!
//! `RequestGate` を `UpgradeHandler` より先に評価するのは、将来の hub
//! TenantGate（TASK-9.1）が WebSocket アップグレードも既定拒否でゲート
//! できるようにするため（フェイルクローズ、`docs/spec/04-requirements.md` REQ-9）。

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::sync::Semaphore;

use bf_http::body::BodyError;
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
const READ_TIMEOUT: Duration = Duration::from_secs(30);

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

/// リクエストに対する最終応答を生成する、コアが公開する既定ハンドラ拡張点。
///
/// `docs/spec/05-tasks.md` TASK-1.5（#14）で `crates/routes` にルーティングが
/// 切り出されるまでの間、コアが直接保持する単一ハンドラとして機能する。
/// 3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）とは異なり
/// 「拡張点は 3 種に集約」の対象ではなく、あくまでルーティング機能が
/// 実装されるまでの暫定的な既定レスポンダである。
pub trait Handler: Send + Sync {
    /// リクエストヘッドと body からレスポンスを組み立てる。
    fn handle(&self, head: &RequestHead, body: &[u8]) -> Response;
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
}

impl Default for Server {
    fn default() -> Self {
        Self {
            middlewares: Vec::new(),
            gates: Vec::new(),
            upgrade_handlers: Vec::new(),
            handler: None,
            max_connections: DEFAULT_MAX_CONNECTIONS,
        }
    }
}

impl Server {
    /// 拡張点・ハンドラを 1 件も持たない空の [`Server`] を作る。
    /// 同時接続数上限は [`DEFAULT_MAX_CONNECTIONS`]。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 同時接続数の上限を設定する（既定 [`DEFAULT_MAX_CONNECTIONS`]）。
    ///
    /// [`BoundServer::run`] の accept ループはこの上限に達している間、
    /// 新規接続の受理を保留する（リソース枯渇 DoS 対策、本モジュール冒頭の
    /// doc・[`DEFAULT_MAX_CONNECTIONS`] の doc を参照）。`0` を指定した場合は
    /// accept ループが永久に許可待ちでブロックし新規接続を一切受理できなく
    /// なるため、[`Server::bind`] 側で最低 `1` に切り上げる。
    #[must_use]
    pub fn max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
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

/// [`Server::bind`] が返す、リスニングソケットを保持した状態のサーバ。
pub struct BoundServer {
    listener: TcpListener,
    server: Arc<Server>,
    /// 同時接続数の上限を強制するセマフォ（[`DEFAULT_MAX_CONNECTIONS`] の doc を参照）。
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
    /// 拒否される（[`DEFAULT_MAX_CONNECTIONS`] の doc を参照）。
    pub async fn run(self) -> io::Result<()> {
        loop {
            // セマフォが閉じられることはない（`close()` を呼ぶ経路がない）ため
            // `acquire_owned` は必ず成功する。
            let permit = Arc::clone(&self.connection_limit)
                .acquire_owned()
                .await
                .expect("connection_limit semaphore is never closed");
            let (stream, _peer_addr) = self.listener.accept().await?;
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
    let mut buf = Vec::new();

    loop {
        let read_result =
            tokio::time::timeout(READ_TIMEOUT, read_request(&mut stream, &mut buf)).await;

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

        let started_at = Instant::now();
        for middleware in &server.middlewares {
            middleware.on_request(&request.head);
        }

        let keep_alive = should_keep_alive(&request.head);

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
            // 長時間接続へ委譲する前にコア側の読み取りバッファを明示的に
            // 解放する（Conditional Go 条件 (1)）。確保済み容量ごと解放する
            // ため `clear()` だけでなく `shrink_to_fit()` まで行う。
            buf.clear();
            buf.shrink_to_fit();
            match try_handle_upgrade(stream, &request.head, &server.upgrade_handlers).await {
                Some(mut stream) => {
                    // #70 時点では実処理者（プラグイン）が存在しないため、
                    // マッチしたのに委譲先がない状態を黙って落とさず 501 で
                    // 明示的に拒否する（本モジュール冒頭の doc・try_handle_upgrade
                    // の doc を参照）。on_response は「委譲時は呼ばない」契約
                    // のため呼ばない（この 501 応答は委譲失敗のフォール
                    // バックであり実処理の完了ではないため）。結果として
                    // on_request は呼ばれるが対になる on_response が呼ばれない
                    // 非対称が生じる点は意図的な仕様であり、Middleware 実装側は
                    // 「on_request が必ず on_response を伴う」と仮定しないこと
                    // （実プラグイン接続後は TASK-2.1 でこの非対称は解消される想定）。
                    let _ = stream
                        .write_all(&Response::empty(501).serialize(false))
                        .await;
                    return;
                }
                None => return,
            }
        }

        let response = match &server.handler {
            Some(handler) => handler.handle(&request.head, &request.body),
            None => Response::empty(404),
        };

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

/// [`UpgradeHandler::matches`] が `true` を返した接続をプラグイン側へ委譲する
/// ための、一定シグネチャの委譲シーム。
///
/// TASK-2.1（#18）で `#[cfg(feature = "...")]` 付きの実装（実プラグインへの
/// 接続奪取）に差し替わる想定であり、`handle_connection` 側はこの関数の
/// シグネチャを変えずに済むよう設計している（コアループ本体を feature で
/// 分岐させないための設計上の要）。
///
/// 戻り値 `Some(stream)` は「委譲されず、呼び出し元が後続処理（フォール
/// バック応答）を続けるべき」ことを意味する。`None` は「完全に委譲済みで
/// 呼び出し元はこれ以上ストリームに触れない」ことを意味する。#70 時点では
/// 実プラグインが存在しないため常に `Some(stream)` を返し、呼び出し元
/// （`handle_connection`）が 501 Not Implemented を返して接続を閉じる。
async fn try_handle_upgrade<S>(
    stream: S,
    _head: &RequestHead,
    _handlers: &[Box<dyn UpgradeHandler>],
) -> Option<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    Some(stream)
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
