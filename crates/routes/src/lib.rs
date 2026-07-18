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
use bf_http::response::{AllowedMethods, Response};

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
    ///   RFC 9110 §15.5.6 / §10.2.1 に従い、`target` に登録済みの全 method を
    ///   ソート済み・重複排除済みで `Allow` ヘッダに付与する（TASK-177 / #177。
    ///   `bf_http::response::AllowedMethods` の構築時 tchar 検証により CRLF
    ///   インジェクションは型レベルで排除される。登録 method に不正 token が
    ///   含まれる場合は `AllowedMethods::from_methods` が `None` を返すため、
    ///   その分だけ除外する。パーサ（`bf-http`）は tchar のみの method しか
    ///   生成しないため、実運用でこの除外が発生する経路はない。全滅時は
    ///   `Allow` なしの 405 にフォールバックする、フェイルクローズ）。
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
    /// let router = Router::new()
    ///     .route("GET", "/", |_head, _body| {
    ///         bf_http::response::Response::new(200, b"ok".to_vec())
    ///     })
    ///     .route("POST", "/", |_head, _body| {
    ///         bf_http::response::Response::new(201, b"created".to_vec())
    ///     });
    ///
    /// // 未登録パス → 404（Allow は付与されない）
    /// let miss = head(b"GET /missing HTTP/1.1\r\n\r\n");
    /// assert_eq!(router.dispatch(&miss, &[]).status, 404);
    ///
    /// // 登録済みパスだがメソッド不一致 → 405 + Allow: GET, POST
    /// let wrong_method = head(b"DELETE / HTTP/1.1\r\n\r\n");
    /// let res = router.dispatch(&wrong_method, &[]);
    /// assert_eq!(res.status, 405);
    /// let text = String::from_utf8(res.serialize(false)).unwrap();
    /// assert!(text.contains("Allow: GET, POST\r\n"));
    /// ```
    #[must_use]
    pub fn dispatch(&self, head: &RequestHead, body: &[u8]) -> Response {
        if let Some(handler) = self.routes.get(&(head.method.clone(), head.target.clone())) {
            return handler(head, body);
        }

        let registered_methods: Vec<String> = self
            .routes
            .keys()
            .filter(|(_, target)| target == &head.target)
            .map(|(method, _)| method.clone())
            .collect();

        if registered_methods.is_empty() {
            return Response::empty(404);
        }

        // `AllowedMethods::from_methods` は 1 件でも不正 token があれば全体を
        // 構築失敗（`None`）とする all-or-nothing 契約（`response.rs` doc 参照）。
        // Router 側は「不正 token を持つ登録ルートだけを Allow から除外し、
        // 残りの正当な method は開示する」方針のため、要素単位で妥当性を
        // 検証してから正当なものだけをまとめて構築する（パーサ（`bf-http`）
        // は tchar のみの method しか生成しないため、実運用でこの除外が
        // 発生する経路はない）。
        let valid_methods: Vec<String> = registered_methods
            .into_iter()
            .filter(|m| AllowedMethods::from_methods([m.clone()]).is_some())
            .collect();

        match AllowedMethods::from_methods(valid_methods) {
            Some(allow) => Response::empty(405).with_allow(allow),
            // 登録 method が全て不正 token だった場合のフェイルクローズ
            // フォールバック。`Allow` は省略するが 405 自体は変わらない。
            None => Response::empty(405),
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

    #[test]
    fn method_mismatch_405_includes_sorted_allow_header() {
        // TASK-177 / #177: 405 応答に RFC 9110 §15.5.6 の Allow ヘッダを付与する。
        // 登録済み method（DELETE, GET）はソート済みで出力される。
        let router = Router::new()
            .route("GET", "/", |_h, _b| Response::empty(200))
            .route("DELETE", "/", |_h, _b| Response::empty(204));
        let res = router.dispatch(&head("POST", "/"), &[]);
        assert_eq!(res.status, 405);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Allow: DELETE, GET\r\n"));
    }

    #[test]
    fn method_mismatch_405_aggregates_multiple_registered_methods_for_same_target() {
        let router = Router::new()
            .route("GET", "/a", |_h, _b| Response::empty(200))
            .route("POST", "/a", |_h, _b| Response::empty(201))
            .route("PUT", "/a", |_h, _b| Response::empty(200));
        let res = router.dispatch(&head("DELETE", "/a"), &[]);
        assert_eq!(res.status, 405);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Allow: GET, POST, PUT\r\n"));
    }

    #[test]
    fn unregistered_target_404_has_no_allow_header() {
        let router = Router::new().route("GET", "/", |_h, _b| Response::empty(200));
        let res = router.dispatch(&head("GET", "/missing"), &[]);
        assert_eq!(res.status, 404);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(!text.contains("Allow:"));
    }

    #[test]
    fn method_mismatch_405_allow_header_injection_regression() {
        // ヘッダインジェクション回帰テスト（TASK-177 / #177）: 不正な method
        // token（CRLF を含む文字列）が登録されていても、`AllowedMethods` の
        // 構築時検証で除外され、直列化バイト列に絶対に現れない。
        let router = Router::new()
            .route("GET\r\nX-Evil: 1", "/", |_h, _b| Response::empty(200))
            .route("GET", "/", |_h, _b| Response::empty(200));
        let res = router.dispatch(&head("DELETE", "/"), &[]);
        assert_eq!(res.status, 405);
        let bytes = res.serialize(false);
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("X-Evil"));
        // 不正 token は除外され、正当な GET のみが Allow に残る。
        assert!(text.contains("Allow: GET\r\n"));
        // ヘッダ数が増えていない（インジェクションによる余分な行が無い）こと
        // を、ヘッダ/ボディ境界の空行が 1 箇所のみであることで確認する。
        assert_eq!(text.matches("\r\n\r\n").count(), 1);
    }
}
