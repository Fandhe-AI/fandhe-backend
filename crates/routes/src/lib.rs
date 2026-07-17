//! `bf-routes`: backend-framework の最小ルータ（TASK-1.5、#14）。
//!
//! # このクレートの役割
//!
//! `crates/core`（`server` モジュール）が接続受理・リクエストループの本体を担い、
//! 本クレートはその既定ハンドラとして「method + `target` の完全一致」でルートを
//! 解決する [`Router`] を提供する。パーサ層（`bf-http`）が生成した検証済みの
//! [`bf_http::request::RequestHead`] を受け取り、[`bf_http::response::Response`]
//! を組み立てるところまでが責務であり、ソケット I/O・接続ライフサイクル管理は
//! 呼び出し元（`crates/core`）の責務のまま変わらない。
//!
//! # workspace 内での依存方向
//!
//! `docs/spec/04-requirements.md` REQ-1 / `docs/spec/05-tasks.md` TASK-1.5 の方針に従い、
//! workspace 全体の依存方向は次の一方向を維持する:
//!
//! ```text
//! server → routes → http::*
//! ```
//!
//! 本クレートはこのグラフの中間層であり、下位層 `bf-http` にのみ依存する
//! （`crates/routes/Cargo.toml` 参照）。`crates/core`（`server`）からのみ参照され、
//! `crates/plugin-*` の固有シンボルには一切依存しない
//! （pay-for-what-you-use、`.claude/rules/pay-for-what-you-use.md`）。
//! この一方向性は `scripts/dep-direction-check.sh` が `cargo metadata` の依存エッジ
//! ホワイトリスト照合・本 doc 宣言行の存在確認・プラグイン固有シンボルの grep ゼロ件
//! 確認の 3 段で機械検証する。
//!
//! # マッチング方針（意図的な設計制約）
//!
//! [`Router::dispatch`] は method・`target` とも**完全一致のみ**で照合する。
//! パスパラメータ・ワイルドカード・末尾スラッシュ正規化・% デコードは一切行わない。
//! `RequestHead::target` は `bf-http` のパーサが SP・制御文字を含まないことを既に
//! 検証済みだが、正規化やデコードの差異はアクセス制御バイパスの典型的な経路
//! （OWASP A01、`.claude/rules/security.md`）になり得るため、本クレートでは
//! 「パーサが渡したバイト列をそのまま文字列として比較する」以上の解釈を持たない
//! 設計とした。高度なルーティング（パスパラメータ等）が必要になった場合は別途
//! 検討する（out-of-scope-tracking 対象）。
//!
//! # フェイルクローズ
//!
//! 登録されたルートに method + target が一致しない場合は 404、target は一致するが
//! method が一致しない場合は 405 を返す。デフォルト許可の経路は存在しない。

use std::collections::HashMap;

use bf_http::request::RequestHead;
use bf_http::response::Response;

/// 登録済みルートのハンドラ型。
///
/// [`bf_http::request::RequestHead`] と body（生バイト列）を受け取り
/// [`bf_http::response::Response`] を返す。`crates/core::server::Handler::handle`
/// と同一シグネチャだが、依存方向（`routes` は `core` に依存できない）の制約上
/// trait は共有せず、本クレート独自の型として定義する。`Send + Sync` は複数
/// コネクションタスクから共有参照される前提（`crates/core` のコアループ）。
pub type RouteHandler = Box<dyn Fn(&RequestHead, &[u8]) -> Response + Send + Sync>;

/// method + `target` の完全一致でハンドラを解決する最小ルータ。
///
/// 登録は起動時のみを想定し、実行時にルートを追加・削除する API は持たない
/// （同期プリミティブ不要・[`Router::dispatch`] は登録数に対して予測可能な
/// コストで応答するため、リクエスト毎の挙動がリソース枯渇 DoS の攻撃対象に
/// なりにくい、`.claude/rules/security.md`）。
///
/// `crates/core::server::Server::handler` にそのまま登録して使う想定
/// （`crates/core/examples/minimal.rs` 参照）。
///
/// `RequestHead` は非公開フィールド（`headers`）を持ち構造体リテラルで直接
/// 組み立てられないため、doc test では `bf_http::request::parse_request_head`
/// で生バイト列から生成する（`crates/core` のコアループが実運用で受け取る
/// 経路と同一）。
///
/// ```
/// use bf_routes::Router;
/// use bf_http::request::{parse_request_head, ParseOutcome};
///
/// let router = Router::new().route("GET", "/", |_head, _body| {
///     bf_http::response::Response::new(200, b"ok".to_vec())
/// });
///
/// let head = match parse_request_head(b"GET / HTTP/1.1\r\n\r\n").unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// let res = router.dispatch(&head, &[]);
/// assert_eq!(res.status, 200);
/// ```
#[derive(Default)]
pub struct Router {
    // key は (method, target) の完全一致タプル。method は登録時の大文字小文字を
    // そのまま保持する（RFC 9110 上メソッド token は大文字小文字を区別するため、
    // 独自の正規化を持ち込まない）。
    routes: HashMap<(String, String), RouteHandler>,
}

impl Router {
    /// 空のルータを作る。
    ///
    /// ```
    /// use bf_routes::Router;
    /// use bf_http::request::{parse_request_head, ParseOutcome};
    ///
    /// let router = Router::new();
    /// let head = match parse_request_head(b"GET / HTTP/1.1\r\n\r\n").unwrap() {
    ///     ParseOutcome::Complete { head, .. } => head,
    ///     ParseOutcome::Incomplete => unreachable!(),
    /// };
    /// assert_eq!(router.dispatch(&head, &[]).status, 404);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
        }
    }

    /// `method` + `path` に完全一致するリクエストを `handler` へ委譲するよう登録する。
    ///
    /// 同一 `(method, path)` を複数回登録した場合は最後の登録が有効になる
    /// （`HashMap` の上書き挙動をそのまま踏襲する。起動時構築のみを想定するため
    /// 警告ログ等は出さない）。
    ///
    /// ```
    /// use bf_routes::Router;
    /// use bf_http::request::{parse_request_head, ParseOutcome};
    ///
    /// let router = Router::new().route("GET", "/health", |_head, _body| {
    ///     bf_http::response::Response::new(200, b"ok".to_vec())
    /// });
    ///
    /// let head = match parse_request_head(b"GET /health HTTP/1.1\r\n\r\n").unwrap() {
    ///     ParseOutcome::Complete { head, .. } => head,
    ///     ParseOutcome::Incomplete => unreachable!(),
    /// };
    /// assert_eq!(router.dispatch(&head, &[]).status, 200);
    /// ```
    #[must_use]
    pub fn route(
        mut self,
        method: impl Into<String>,
        path: impl Into<String>,
        handler: impl Fn(&RequestHead, &[u8]) -> Response + Send + Sync + 'static,
    ) -> Self {
        self.routes
            .insert((method.into(), path.into()), Box::new(handler));
        self
    }

    /// `head` の method + `target` に一致するハンドラへ委譲し、[`Response`] を返す。
    ///
    /// - `target` に一致するルートが 1 件もない場合は 404（Not Found）。
    /// - `target` は一致するが `method` が一致しない場合は 405（Method Not Allowed）。
    ///   `bf_http::response::Response` は任意ヘッダ追加 API を持たないため（response.rs
    ///   の doc 参照）、本メソッドは `Allow` ヘッダを付与しない
    ///   （out-of-scope-tracking 対象、TASK-1.5 スコープ外）。
    /// - 完全一致するルートがあればそのハンドラの戻り値をそのまま返す。
    ///
    /// ```
    /// use bf_routes::Router;
    /// use bf_http::request::{parse_request_head, ParseOutcome};
    ///
    /// fn head(buf: &[u8]) -> bf_http::request::RequestHead {
    ///     match parse_request_head(buf).unwrap() {
    ///         ParseOutcome::Complete { head, .. } => head,
    ///         ParseOutcome::Incomplete => unreachable!(),
    ///     }
    /// }
    ///
    /// let router = Router::new().route("GET", "/", |_head, _body| {
    ///     bf_http::response::Response::new(200, b"ok".to_vec())
    /// });
    ///
    /// // 未登録パス → 404
    /// let miss = head(b"GET /missing HTTP/1.1\r\n\r\n");
    /// assert_eq!(router.dispatch(&miss, &[]).status, 404);
    ///
    /// // 登録済みパスだがメソッド不一致 → 405
    /// let wrong_method = head(b"POST / HTTP/1.1\r\n\r\n");
    /// assert_eq!(router.dispatch(&wrong_method, &[]).status, 405);
    /// ```
    #[must_use]
    pub fn dispatch(&self, head: &RequestHead, body: &[u8]) -> Response {
        if let Some(handler) = self.routes.get(&(head.method.clone(), head.target.clone())) {
            return handler(head, body);
        }

        let target_exists = self.routes.keys().any(|(_, target)| target == &head.target);
        if target_exists {
            Response::empty(405)
        } else {
            Response::empty(404)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_http::request::{ParseOutcome, parse_request_head};

    // `RequestHead` は非公開フィールドを持ち構造体リテラルで直接組み立てられない
    // ため、パーサ（`parse_request_head`）経由で生成する。他クレートのテスト
    // （`crates/core/src/extension.rs` の `head_from` 等）と同一パターン。
    fn head(method: &str, target: &str) -> RequestHead {
        let request_line = format!("{method} {target} HTTP/1.1\r\n\r\n");
        match parse_request_head(request_line.as_bytes()).expect("parse should succeed") {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("expected Complete"),
        }
    }

    #[test]
    fn exact_match_dispatches_to_registered_handler() {
        let router = Router::new().route("GET", "/", |_h, _b| Response::new(200, b"root".to_vec()));
        let res = router.dispatch(&head("GET", "/"), &[]);
        assert_eq!(res.status, 200);
        assert_eq!(res.body, b"root".to_vec());
    }

    #[test]
    fn unregistered_target_returns_404() {
        let router = Router::new().route("GET", "/", |_h, _b| Response::empty(200));
        let res = router.dispatch(&head("GET", "/nope"), &[]);
        assert_eq!(res.status, 404);
        assert!(res.body.is_empty());
    }

    #[test]
    fn registered_target_with_wrong_method_returns_405() {
        let router = Router::new().route("GET", "/", |_h, _b| Response::empty(200));
        let res = router.dispatch(&head("POST", "/"), &[]);
        assert_eq!(res.status, 405);
    }

    #[test]
    fn empty_router_returns_404_for_any_request() {
        let router = Router::new();
        let res = router.dispatch(&head("GET", "/"), &[]);
        assert_eq!(res.status, 404);
    }

    #[test]
    fn multiple_routes_are_independent_and_registration_order_does_not_matter() {
        let router = Router::new()
            .route("GET", "/a", |_h, _b| Response::new(200, b"a".to_vec()))
            .route("POST", "/b", |_h, _b| Response::new(201, b"b".to_vec()))
            .route("GET", "/b", |_h, _b| Response::new(200, b"b-get".to_vec()));

        assert_eq!(router.dispatch(&head("GET", "/a"), &[]).body, b"a".to_vec());
        assert_eq!(router.dispatch(&head("POST", "/b"), &[]).status, 201);
        assert_eq!(
            router.dispatch(&head("GET", "/b"), &[]).body,
            b"b-get".to_vec()
        );
        // /a に登録されていない DELETE は 405（/a 自体は存在するため）。
        assert_eq!(router.dispatch(&head("DELETE", "/a"), &[]).status, 405);
    }

    #[test]
    fn handler_receives_body_bytes() {
        let router = Router::new().route("POST", "/echo", |_h, body| {
            Response::new(200, body.to_vec())
        });
        let res = router.dispatch(&head("POST", "/echo"), b"payload");
        assert_eq!(res.body, b"payload".to_vec());
    }

    #[test]
    fn method_is_case_sensitive_and_not_normalized() {
        // RFC 9110 上メソッド token は大文字小文字を区別する。独自の正規化を
        // 持ち込まないという設計方針（本モジュール doc）を固定化するテスト。
        let router = Router::new().route("GET", "/", |_h, _b| Response::empty(200));
        let res = router.dispatch(&head("get", "/"), &[]);
        assert_eq!(res.status, 405);
    }

    #[test]
    fn re_registering_same_method_and_path_overwrites_previous_handler() {
        let router = Router::new()
            .route("GET", "/", |_h, _b| Response::new(200, b"first".to_vec()))
            .route("GET", "/", |_h, _b| Response::new(200, b"second".to_vec()));
        let res = router.dispatch(&head("GET", "/"), &[]);
        assert_eq!(res.body, b"second".to_vec());
    }
}
