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
//! （詳細規約は TASK-2.3 で AGENTS.md 等に整備予定）。

use bf_http::request::RequestHead;
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
/// use backend_framework_core::extension::Middleware;
/// use bf_http::request::{parse_request_head, ParseOutcome};
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
///     fn on_request(&self, _head: &bf_http::request::RequestHead) {
///         self.requests.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
///     }
///
///     fn on_response(&self, _head: &bf_http::request::RequestHead, _elapsed: Duration) {}
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
/// use backend_framework_core::extension::UpgradeHandler;
/// use bf_http::request::{parse_request_head, ParseOutcome};
///
/// /// `Upgrade: websocket` ヘッダの有無だけを見るトイ実装。
/// struct WebSocketUpgrade;
///
/// impl UpgradeHandler for WebSocketUpgrade {
///     fn name(&self) -> &'static str {
///         "websocket-upgrade"
///     }
///
///     fn matches(&self, head: &bf_http::request::RequestHead) -> bool {
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
/// をコアに持ち込まない（PoC-6 の設計判断）。`Reject` の `status` は
/// レスポンスのステータスコードのみを運ぶ数値（`u16`）とし、任意文字列を
/// そのままステータス行に書き出す設計を避ける。これはレスポンス分割・
/// ヘッダインジェクションを型レベルで排除するためであり、ステータス行の
/// 組み立て（reason phrase の付与等）はコア側（#70 実装分）の責務とする。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// リクエストを許可する。以降の処理（ルーティング等）を続行する。
    Allow,
    /// リクエストを拒否する。`status` は HTTP ステータスコード、`body` は
    /// レスポンスボディの生バイト列。
    Reject {
        /// レスポンスとして返す HTTP ステータスコード。
        status: u16,
        /// レスポンスボディの生バイト列。
        body: Vec<u8>,
    },
}

/// 早期拒否可能な拡張点。認証・認可・同意ゲート等、ルーティング前に
/// リクエストを弾く判断をコアに提供する。
///
/// 実装は**フェイルクローズ**を契約とする: 判定に必要な情報が欠落・不正な
/// 場合、あるいは判定不能な場合は必ず [`GateOutcome::Reject`] を返し、
/// 疑わしきは通過させない（`docs/spec/04-requirements.md` REQ-9・
/// `.claude/rules/security.md` の認可既定拒否の方針に従う）。
///
/// # Examples
///
/// ```
/// use backend_framework_core::extension::{GateOutcome, RequestGate};
/// use bf_http::request::{parse_request_head, ParseOutcome};
///
/// /// `Authorization` ヘッダの有無だけを見るトイ実装（フェイルクローズ）。
/// struct RequireAuthHeader;
///
/// impl RequestGate for RequireAuthHeader {
///     fn name(&self) -> &'static str {
///         "require-auth-header"
///     }
///
///     fn check(&self, head: &bf_http::request::RequestHead) -> GateOutcome {
///         match head.header("authorization") {
///             Some(_) => GateOutcome::Allow,
///             None => GateOutcome::Reject { status: 401, body: Vec::new() },
///         }
///     }
/// }
///
/// let gate = RequireAuthHeader;
///
/// let buf = b"GET / HTTP/1.1\r\nAuthorization: Bearer x\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// assert_eq!(gate.check(&head), GateOutcome::Allow);
///
/// let buf = b"GET / HTTP/1.1\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// assert_eq!(
///     gate.check(&head),
///     GateOutcome::Reject { status: 401, body: Vec::new() }
/// );
/// ```
pub trait RequestGate: Send + Sync {
    /// 診断・ログ表示用の静的識別名。
    fn name(&self) -> &'static str;

    /// リクエストヘッドを検査し、許可/拒否を判定する。判定不能・情報欠落時は
    /// 必ず [`GateOutcome::Reject`] を返すこと（フェイルクローズ）。
    fn check(&self, head: &RequestHead) -> GateOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_http::request::{ParseOutcome, parse_request_head};

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

        fn check(&self, _head: &RequestHead) -> GateOutcome {
            GateOutcome::Allow
        }
    }

    /// 情報欠落時は必ず拒否する契約（フェイルクローズ）を検証するトイ実装。
    struct FailClosedGate;

    impl RequestGate for FailClosedGate {
        fn name(&self) -> &'static str {
            "fail-closed"
        }

        fn check(&self, head: &RequestHead) -> GateOutcome {
            match head.header("authorization") {
                Some(_) => GateOutcome::Allow,
                None => GateOutcome::Reject {
                    status: 401,
                    body: Vec::new(),
                },
            }
        }
    }

    #[test]
    fn request_gate_allow_outcome() {
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        assert_eq!(AllowAllGate.check(&head), GateOutcome::Allow);
    }

    #[test]
    fn request_gate_fail_closed_rejects_missing_authorization() {
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        assert_eq!(
            FailClosedGate.check(&head),
            GateOutcome::Reject {
                status: 401,
                body: Vec::new()
            }
        );
    }

    #[test]
    fn request_gate_fail_closed_allows_with_authorization() {
        let head = head_from(b"GET / HTTP/1.1\r\nAuthorization: Bearer x\r\n\r\n");
        assert_eq!(FailClosedGate.check(&head), GateOutcome::Allow);
    }

    #[test]
    fn gate_outcome_reject_carries_status_and_body() {
        // `Reject` の `status`/`body` フィールドが正しく保持されることを固定する。
        // コア側（#70）はこの数値をそのままステータス行に、body をそのまま
        // レスポンスボディに使う契約であり、値の欠落・破損は直接クライアント
        // 応答に波及する。
        let outcome = GateOutcome::Reject {
            status: 403,
            body: b"forbidden".to_vec(),
        };
        match outcome {
            GateOutcome::Reject { status, body } => {
                assert_eq!(status, 403);
                assert_eq!(body, b"forbidden");
            }
            GateOutcome::Allow => panic!("expected Reject"),
        }
    }

    #[test]
    fn gate_outcome_allow_and_reject_are_not_equal() {
        // `PartialEq` 導出が variant を跨いで誤って等しいと判定しないことを固定する。
        let allow = GateOutcome::Allow;
        let reject = GateOutcome::Reject {
            status: 401,
            body: Vec::new(),
        };
        assert_ne!(allow, reject);
    }

    #[test]
    fn gate_outcome_reject_with_different_status_are_not_equal() {
        let a = GateOutcome::Reject {
            status: 401,
            body: Vec::new(),
        };
        let b = GateOutcome::Reject {
            status: 403,
            body: Vec::new(),
        };
        assert_ne!(a, b);
    }
}
