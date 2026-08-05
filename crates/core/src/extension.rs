//! コアが公開する 3 種の拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）。
//!
//! `docs/spec/04-requirements.md` REQ-1・`.claude/rules/coding-rust.md` の設計原則
//! 「拡張点は 3 種 trait に集約」の実体。`crates/plugin-*`（websocket / graphql /
//! hub-wiring 等）はこのモジュールの trait を実装する側であり、本モジュールが
//! プラグイン固有のシンボル（JWT・`org_id`・WebSocket フレーミング等）に依存する
//! ことは決してない（依存方向は `server → routes → http::*` の一方向、TASK-1.5）。
//!
//! 3 trait の責務分担:
//! - [`Middleware`][]: 観測専用フック。ロギング・計測等の横断的関心事向け
//! - [`UpgradeHandler`][]: 長時間接続（WebSocket 等）への**委譲判定のみ**。
//!   フレーミング・接続奪取後の処理はプラグイン側に閉じる
//! - [`RequestGate`][]: 早期拒否（認証・認可・同意ゲート等）。[`GateOutcome`][] は
//!   許可/拒否の判定結果のみを運び、クレーム等の hub 固有データを持ち込まない
//!   （`docs/spec/03-poc/hub-wiring-middleware` PoC-6 の設計判断）
//!
//! # 本モジュールのスコープ境界
//!
//! 本モジュールは trait 定義のみを提供する（TASK-1.4-1 / #69）。3 拡張点を
//! 実際に呼び出すコアループ（接続受理・リクエストループ、`Vec<Box<dyn ...>>`
//! を保持する実装）は姉妹モジュール [`crate::server`]（TASK-1.4-2 / #70）が
//! 提供する。feature flag + `dep:` 構文によるプラグイン境界の確立は
//! TASK-2.1（#18）のスコープ。
//!
//! trait 自体は無条件でコアの公開 API として存在するが、実装がゼロであれば
//! 実行時コストもゼロであるため、これは pay-for-what-you-use 原則に反しない
//! （`.claude/rules/pay-for-what-you-use.md`）。
//!
//! # 非同期・I/O に関する規約
//!
//! 3 trait とも同期 API として定義する。`async fn` を trait に持ち込むと
//! `Box<dyn Middleware>` 等の trait object としてコアループが拡張点を保持する
//! 構成（dyn 互換性）が壊れるためである。ただし [`Middleware::on_request`] /
//! [`Middleware::on_response`] の実装は同期ブロッキング I/O を行ってはならない
//! （PoC-3 実測でスループットが最大 25% 劣化する）。ロギング等で I/O が必要な
//! 実装は非同期チャネルへの送信に留め、実際の I/O は別タスクで行う契約とする
//! （詳細規約は TASK-2.3 で `AGENTS.md` に整備済み）。

use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use std::net::SocketAddr;
use std::time::Duration;

/// リクエスト/レスポンスを**観測するだけ**のフック。
///
/// ロギング・メトリクス計測等の横断的関心事向けの拡張点であり、実装は
/// `head` の内容を変更してはならない契約とする（コアはこの契約を型では
/// 強制しないため、実装者が守る規約として doc に明記する）。
///
/// 同期 API だが、実装内で同期ブロッキング I/O を行ってはならない
/// （本モジュールの doc を参照）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::extension::Middleware;
/// use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
/// use std::time::Duration;
///
/// /// 呼び出し回数を数えるだけのトイ実装。
/// struct CountingMiddleware {
///     requests: std::sync::atomic::AtomicUsize,
/// }
///
/// impl Middleware for CountingMiddleware {
///     fn name(&self) -> &'static str {
///         "counting-middleware"
///     }
///
///     fn on_request(&self, _head: &fandhe_backend_http::request::RequestHead) {
///         self.requests.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
///     }
///
///     fn on_response(&self, _head: &fandhe_backend_http::request::RequestHead, _elapsed: Duration) {}
/// }
///
/// let mw = CountingMiddleware { requests: std::sync::atomic::AtomicUsize::new(0) };
/// let buf = b"GET / HTTP/1.1\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// mw.on_request(&head);
/// assert_eq!(mw.requests.load(std::sync::atomic::Ordering::Relaxed), 1);
/// ```
pub trait Middleware: Send + Sync {
    /// 診断・ログ表示用の静的識別名。リクエスト内容（トークン・PII）を
    /// 含めてはならない。
    fn name(&self) -> &'static str;

    /// リクエストヘッド受理後、ルーティング前に呼ばれる観測フック。
    fn on_request(&self, head: &RequestHead);

    /// レスポンス送出後に呼ばれる観測フック。`elapsed` はリクエスト受理から
    /// レスポンス送出までの経過時間。
    fn on_response(&self, head: &RequestHead, elapsed: Duration);
}

/// 長時間接続（WebSocket・WebRTC シグナリング等）への**委譲判定のみ**を
/// コアに公開する拡張点。
///
/// [`UpgradeHandler::matches`] が `true` を返した場合、コア（#70 実装分）は
/// 当該接続の以降の処理をプラグイン側に委譲する契約とする。フレーミング・
/// プロトコルアップグレード後の読み書きは本 trait の責務外であり、
/// プラグイン側（`crates/plugin-websocket` 等）に閉じる。
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::extension::UpgradeHandler;
/// use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
///
/// /// `Upgrade: websocket` ヘッダの有無だけを見るトイ実装。
/// struct WebSocketUpgrade;
///
/// impl UpgradeHandler for WebSocketUpgrade {
///     fn name(&self) -> &'static str {
///         "websocket-upgrade"
///     }
///
///     fn matches(&self, head: &fandhe_backend_http::request::RequestHead) -> bool {
///         head.header("upgrade")
///             .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
///     }
/// }
///
/// let handler = WebSocketUpgrade;
/// let buf = b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// assert!(handler.matches(&head));
/// ```
pub trait UpgradeHandler: Send + Sync {
    /// 診断・ログ表示用の静的識別名。
    fn name(&self) -> &'static str;

    /// このリクエストが自分の担当するアップグレードプロトコルに該当するかを
    /// 判定する。`true` を返した場合、以降の接続処理はこの実装（プラグイン）
    /// に委譲される契約とする。
    fn matches(&self, head: &RequestHead) -> bool;
}

/// [`RequestGate::check`] の判定結果。
///
/// 許可/拒否の判定結果のみを運び、JWT クレーム・`org_id` 等の hub 固有データ
/// をコアに持ち込まない（PoC-6 の設計判断）。`Reject` は検証済み
/// [`Response`] をそのまま運ぶ（イシュー #424）。任意文字列を無検証で
/// ステータス行・ヘッダへ書き出す経路は存在しない。`Response` の構築 API
/// （[`Response::new`] / [`Response::with_header`] /
/// [`Response::with_content_type`] 等）が CR/LF/NUL・予約ヘッダ名
/// （`Content-Length` / `Connection` / `Transfer-Encoding`）を構築時に
/// 拒否するフェイルクローズ検証を担うため、これはレスポンス分割・
/// ヘッダインジェクションを型レベルで排除するという従来の設計意図
/// （`status: u16` のみを運んでいた旧設計の根拠）を維持したまま、
/// レート制限の `429 + Retry-After` 等ヘッダ付き拒否応答を可能にする。
/// `Content-Length` / `Connection` はコア側（#70 実装分、`serialize`）が
/// keep-alive 判定に応じて最終決定するため、ゲート実装からは上書きできない
/// （`with_header` の予約名拒否）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// リクエストを許可する。以降の処理（ルーティング等）を続行する。
    Allow,
    /// リクエストを拒否する。`response` をクライアントへそのまま返す
    /// （`Content-Length` / `Connection` はコア側で上書きされる）。
    Reject {
        /// クライアントへ返す検証済みレスポンス。
        response: Response,
    },
}

impl GateOutcome {
    /// `status` と `body` のみを運ぶ最小の [`GateOutcome::Reject`] を組み立てる。
    ///
    /// ヘッダを付与しない従来相当の拒否応答を簡潔に書くためのヘルパ。
    /// ヘッダ（`Retry-After` 等）を付与したい場合は
    /// `GateOutcome::Reject { response: Response::new(status, body).with_header(...)? }`
    /// のように [`Response`] の検証済み構築 API を直接使う。
    ///
    /// ```
    /// use fandhe_backend_core::extension::GateOutcome;
    ///
    /// let outcome = GateOutcome::reject(401, Vec::new());
    /// assert_eq!(outcome, GateOutcome::Reject { response: fandhe_backend_http::response::Response::new(401, Vec::new()) });
    /// ```
    #[must_use]
    pub fn reject(status: u16, body: Vec<u8>) -> Self {
        Self::Reject {
            response: Response::new(status, body),
        }
    }
}

/// [`RequestGate::check`] へ渡す接続コンテキスト（イシュー #486）。
///
/// accept したソケットの実 peer address を gate 実装から参照可能にするための
/// 型。`RequestHead`（`crates/http`、バイト列から構築される sans-IO 型）へ
/// 接続層の情報を混入させると依存方向（`server → routes → http`）と責務境界が
/// 崩れるため、`check` の引数として別途渡す設計とした
/// （`docs/design/gate-peer-addr.md` 3.1 節）。
///
/// フィールドは非公開とし、将来の項目追加（`local_addr` 等）を非破壊にする。
/// `Copy` 型でヒープ割当を持たないため、gate 未登録時は元より、登録時も
/// 接続あたりの追加コストは実質ゼロ（pay-for-what-you-use、
/// `.claude/rules/pay-for-what-you-use.md`）。
///
/// # `peer_addr` が `None` になる経路
///
/// [`crate::handle_connection`]（`tokio::io::duplex` 等の非ソケット統合テスト
/// 経由の呼び出しを含む）は実 peer が存在しないため `None` を渡す。
/// **peer addr を判定に必要とする gate 実装は、`None` の場合は必ず
/// [`GateOutcome::Reject`] を返すこと**（フェイルクローズ、`RequestGate` の
/// doc を参照）。
///
/// # プロキシ配下の意味論
///
/// リバースプロキシ・ロードバランサ配下では `peer_addr` はプロキシ自身の
/// アドレスになる（本フレームワークは v1 では TLS 終端をリバースプロキシに
/// 委ねる方針、`docs/design/v1-scope-tls-multipart.md`）。`X-Forwarded-For` /
/// `Forwarded` ヘッダ（クライアント申告値であり偽装可能）とは別物であり、
/// IP ベースの認可判定では偽装不能な `peer_addr` を用いること。
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::extension::GateContext;
/// use std::net::SocketAddr;
///
/// let addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();
/// let ctx = GateContext::new(Some(addr));
/// assert_eq!(ctx.peer_addr(), Some(addr));
///
/// let ctx_none = GateContext::new(None);
/// assert_eq!(ctx_none.peer_addr(), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateContext {
    peer_addr: Option<SocketAddr>,
}

impl GateContext {
    /// peer addr 付きコンテキストを構築する。
    ///
    /// 実ソケット経路（[`crate::handle_connection_with_peer_addr`]）だけでなく、
    /// gate 実装の単体テストからも直接構築できるよう公開コンストラクタとする。
    #[must_use]
    pub fn new(peer_addr: Option<SocketAddr>) -> Self {
        Self { peer_addr }
    }

    /// accept したソケットの実 peer address を返す。
    ///
    /// 実ソケットを経由しない経路（`tokio::io::duplex` 等）では `None`。
    /// `None` の意味論は本型の doc「`peer_addr` が `None` になる経路」を参照。
    #[must_use]
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer_addr
    }
}

/// 早期拒否可能な拡張点。認証・認可・同意ゲート等、ルーティング前に
/// リクエストを弾く判断をコアに提供する。
///
/// 実装は**フェイルクローズ**を契約とする: 判定に必要な情報が欠落・不正な
/// 場合、あるいは判定不能な場合は必ず [`GateOutcome::Reject`] を返し、
/// 疑わしきは通過させない（`docs/spec/04-requirements.md` REQ-9・
/// `.claude/rules/security.md` の認可既定拒否の方針に従う）。peer address に
/// 基づく判定（CIDR 照合等）を行う実装は、`ctx.peer_addr()` が `None` の場合
/// も同様に必ず [`GateOutcome::Reject`] を返すこと（イシュー #486、
/// [`GateContext`] の doc を参照）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
/// use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
///
/// /// `Authorization` ヘッダの有無だけを見るトイ実装（フェイルクローズ）。
/// struct RequireAuthHeader;
///
/// impl RequestGate for RequireAuthHeader {
///     fn name(&self) -> &'static str {
///         "require-auth-header"
///     }
///
///     fn check(&self, head: &fandhe_backend_http::request::RequestHead, _ctx: &GateContext) -> GateOutcome {
///         match head.header("authorization") {
///             Some(_) => GateOutcome::Allow,
///             None => GateOutcome::reject(401, Vec::new()),
///         }
///     }
/// }
///
/// let gate = RequireAuthHeader;
/// let ctx = GateContext::new(None);
///
/// let buf = b"GET / HTTP/1.1\r\nAuthorization: Bearer x\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// assert_eq!(gate.check(&head, &ctx), GateOutcome::Allow);
///
/// let buf = b"GET / HTTP/1.1\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// assert_eq!(gate.check(&head, &ctx), GateOutcome::reject(401, Vec::new()));
/// ```
///
/// # ヘッダ付き拒否応答の例（`429 Retry-After`、イシュー #424）
///
/// レート制限のように `Retry-After` ヘッダを伴う拒否応答を返す実装は、
/// [`Response`] の検証済み構築 API（[`Response::with_header`]）を使って
/// `GateOutcome::Reject` を組み立てる。
///
/// ```
/// use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
/// use fandhe_backend_http::request::{RequestHead, parse_request_head, ParseOutcome};
/// use fandhe_backend_http::response::Response;
///
/// /// 常に拒否し、`429 + Retry-After` を返すトイのレート制限ゲート。
/// struct AlwaysRateLimited;
///
/// impl RequestGate for AlwaysRateLimited {
///     fn name(&self) -> &'static str {
///         "always-rate-limited"
///     }
///
///     fn check(&self, _head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
///         let response = Response::new(429, b"{\"error\":\"rate limited\"}".to_vec())
///             .with_content_type("application/json")
///             .with_header("Retry-After", "30")
///             .expect("Retry-After はリテラル値のため構築時検証を通る");
///         GateOutcome::Reject { response }
///     }
/// }
///
/// let gate = AlwaysRateLimited;
/// let ctx = GateContext::new(None);
/// let buf = b"GET / HTTP/1.1\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// let GateOutcome::Reject { response } = gate.check(&head, &ctx) else {
///     unreachable!("AlwaysRateLimited は常に Reject を返す");
/// };
/// let wire = response.serialize(false);
/// let text = String::from_utf8(wire).unwrap();
/// assert!(text.starts_with("HTTP/1.1 429"));
/// assert!(text.contains("Retry-After: 30\r\n"));
/// assert!(text.contains("Content-Type: application/json\r\n"));
/// ```
pub trait RequestGate: Send + Sync {
    /// 診断・ログ表示用の静的識別名。
    fn name(&self) -> &'static str;

    /// リクエストヘッドを検査し、許可/拒否を判定する。判定不能・情報欠落時は
    /// 必ず [`GateOutcome::Reject`] を返すこと（フェイルクローズ）。`ctx` は
    /// accept したソケットの実 peer address を運ぶ（イシュー #486、
    /// [`GateContext`] の doc を参照）。
    fn check(&self, head: &RequestHead, ctx: &GateContext) -> GateOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    /// 3 trait すべてが object safe（dyn 互換）であることをコンパイル時に検証する。
    ///
    /// コアループ（#70）は `Box<dyn Middleware>` 等の trait object として拡張点を
    /// 保持する前提であるため、この性質が壊れると設計そのものが成立しない。
    fn _assert_object_safe(
        _mw: &dyn Middleware,
        _uh: &dyn UpgradeHandler,
        _gate: &dyn RequestGate,
    ) {
    }

    /// `Send + Sync` 境界が 3 trait すべてに付与されていることを静的に検証する。
    ///
    /// コアはマルチスレッド実行が前提であり、拡張点の実装は複数ワーカースレッド
    /// から共有参照される（`Arc<dyn Middleware>` 等）ため、この境界を欠くと
    /// ビルドが通らない設計にしている。
    fn _assert_send_sync<T: Send + Sync + ?Sized>() {}
    #[allow(dead_code)]
    fn _assert_bounds() {
        _assert_send_sync::<dyn Middleware>();
        _assert_send_sync::<dyn UpgradeHandler>();
        _assert_send_sync::<dyn RequestGate>();
    }

    fn head_from(buf: &[u8]) -> RequestHead {
        match parse_request_head(buf).expect("parse should succeed") {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("expected Complete"),
        }
    }

    /// フック呼び出し順（`on_request` → `on_response`）を記録するトイ実装。
    struct RecordingMiddleware {
        calls: std::sync::Mutex<Vec<&'static str>>,
    }

    impl Middleware for RecordingMiddleware {
        fn name(&self) -> &'static str {
            "recording-middleware"
        }

        fn on_request(&self, _head: &RequestHead) {
            self.calls.lock().unwrap().push("on_request");
        }

        fn on_response(&self, _head: &RequestHead, _elapsed: Duration) {
            self.calls.lock().unwrap().push("on_response");
        }
    }

    #[test]
    fn middleware_hooks_are_called_in_order() {
        let mw = RecordingMiddleware {
            calls: std::sync::Mutex::new(Vec::new()),
        };
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");

        mw.on_request(&head);
        mw.on_response(&head, Duration::from_millis(1));

        assert_eq!(*mw.calls.lock().unwrap(), vec!["on_request", "on_response"]);
        assert_eq!(mw.name(), "recording-middleware");
    }

    struct AlwaysMatchUpgrade;

    impl UpgradeHandler for AlwaysMatchUpgrade {
        fn name(&self) -> &'static str {
            "always-match"
        }

        fn matches(&self, _head: &RequestHead) -> bool {
            true
        }
    }

    struct NeverMatchUpgrade;

    impl UpgradeHandler for NeverMatchUpgrade {
        fn name(&self) -> &'static str {
            "never-match"
        }

        fn matches(&self, _head: &RequestHead) -> bool {
            false
        }
    }

    #[test]
    fn upgrade_handler_matches_decision() {
        let head = head_from(b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        assert!(AlwaysMatchUpgrade.matches(&head));
        assert!(!NeverMatchUpgrade.matches(&head));
    }

    struct AllowAllGate;

    impl RequestGate for AllowAllGate {
        fn name(&self) -> &'static str {
            "allow-all"
        }

        fn check(&self, _head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
            GateOutcome::Allow
        }
    }

    /// 情報欠落時は必ず拒否する契約（フェイルクローズ）を検証するトイ実装。
    struct FailClosedGate;

    impl RequestGate for FailClosedGate {
        fn name(&self) -> &'static str {
            "fail-closed"
        }

        fn check(&self, head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
            match head.header("authorization") {
                Some(_) => GateOutcome::Allow,
                None => GateOutcome::reject(401, Vec::new()),
            }
        }
    }

    /// `ctx.peer_addr()` が `None` の場合に必ず拒否する契約（イシュー #486の
    /// フェイルクローズ規約）を検証するトイ実装。
    struct PeerRequiredGate;

    impl RequestGate for PeerRequiredGate {
        fn name(&self) -> &'static str {
            "peer-required"
        }

        fn check(&self, _head: &RequestHead, ctx: &GateContext) -> GateOutcome {
            match ctx.peer_addr() {
                Some(_) => GateOutcome::Allow,
                None => GateOutcome::reject(403, Vec::new()),
            }
        }
    }

    #[test]
    fn request_gate_allow_outcome() {
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        let ctx = GateContext::new(None);
        assert_eq!(AllowAllGate.check(&head, &ctx), GateOutcome::Allow);
    }

    #[test]
    fn request_gate_fail_closed_rejects_missing_authorization() {
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        let ctx = GateContext::new(None);
        assert_eq!(
            FailClosedGate.check(&head, &ctx),
            GateOutcome::reject(401, Vec::new())
        );
    }

    #[test]
    fn request_gate_fail_closed_allows_with_authorization() {
        let head = head_from(b"GET / HTTP/1.1\r\nAuthorization: Bearer x\r\n\r\n");
        let ctx = GateContext::new(None);
        assert_eq!(FailClosedGate.check(&head, &ctx), GateOutcome::Allow);
    }

    #[test]
    fn gate_context_new_and_peer_addr_roundtrip() {
        let addr: std::net::SocketAddr = "192.0.2.1:8080".parse().unwrap();
        let ctx = GateContext::new(Some(addr));
        assert_eq!(ctx.peer_addr(), Some(addr));

        let ctx_none = GateContext::new(None);
        assert_eq!(ctx_none.peer_addr(), None);
    }

    #[test]
    fn request_gate_peer_required_rejects_when_peer_addr_missing() {
        // peer addr 必須の gate が `None`（duplex 等の非ソケット経路）で
        // フェイルクローズすることを固定する（イシュー #486）。
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        let ctx = GateContext::new(None);
        assert_eq!(
            PeerRequiredGate.check(&head, &ctx),
            GateOutcome::reject(403, Vec::new())
        );
    }

    #[test]
    fn request_gate_peer_required_allows_when_peer_addr_present() {
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        let addr: std::net::SocketAddr = "203.0.113.5:54321".parse().unwrap();
        let ctx = GateContext::new(Some(addr));
        assert_eq!(PeerRequiredGate.check(&head, &ctx), GateOutcome::Allow);
    }

    #[test]
    fn gate_outcome_reject_carries_status_and_body() {
        // `Reject` が運ぶ `response` の status/body が正しく保持されることを
        // 固定する。コア側（#70）はこの `Response` をそのまま `serialize` する
        // 契約であり、値の欠落・破損は直接クライアント応答に波及する。
        let outcome = GateOutcome::Reject {
            response: Response::new(403, b"forbidden".to_vec()),
        };
        match outcome {
            GateOutcome::Reject { response } => {
                assert_eq!(response.status, 403);
                assert_eq!(response.body, b"forbidden");
            }
            GateOutcome::Allow => panic!("expected Reject"),
        }
    }

    #[test]
    fn gate_outcome_reject_carries_headers() {
        // `Reject` がヘッダ（`Retry-After` 等）を運べることを固定する
        // （イシュー #424、429 + Retry-After が返せない問題の解消）。
        let response = Response::new(429, Vec::new())
            .with_header("Retry-After", "30")
            .expect("リテラル値は構築時検証を通る");
        let outcome = GateOutcome::Reject { response };
        let GateOutcome::Reject { response } = outcome else {
            panic!("expected Reject");
        };
        let wire = response.serialize(false);
        let text = String::from_utf8(wire).unwrap();
        assert!(text.starts_with("HTTP/1.1 429"));
        assert!(text.contains("Retry-After: 30\r\n"));
    }

    #[test]
    fn gate_outcome_reject_helper_matches_manual_construction() {
        // `GateOutcome::reject` ヘルパが `Response::new` 直接呼び出しと
        // 同一の `Reject` を組み立てることを固定する。
        assert_eq!(
            GateOutcome::reject(401, b"denied".to_vec()),
            GateOutcome::Reject {
                response: Response::new(401, b"denied".to_vec())
            }
        );
    }

    #[test]
    fn gate_outcome_allow_and_reject_are_not_equal() {
        // `PartialEq` 導出が variant を跨いで誤って等しいと判定しないことを固定する。
        let allow = GateOutcome::Allow;
        let reject = GateOutcome::reject(401, Vec::new());
        assert_ne!(allow, reject);
    }

    #[test]
    fn gate_outcome_reject_with_different_status_are_not_equal() {
        let a = GateOutcome::reject(401, Vec::new());
        let b = GateOutcome::reject(403, Vec::new());
        assert_ne!(a, b);
    }
}
