//! ユーザー向けインターセプト・レスポンス改変拡張点 `Interceptor`（イシュー #420）。
//!
//! # 背景・既存 3 拡張点で表現できない理由
//!
//! `crates/core` の既存 3 拡張点（[`crate::extension`]）はいずれも「リダイレクトを
//! 返す」「確定済みレスポンスの body を差し替える」ユースケースを表現できない:
//!
//! - [`crate::extension::Middleware`][]: 観測専用契約。`on_request` は `&RequestHead`
//!   しか受け取らず、`on_response` もレスポンスへの参照を持たない
//! - [`crate::extension::RequestGate`][]: `GateOutcome::Allow` / `Reject { status,
//!   body }` の二択で、`Reject` はヘッダを運べないため 301 + `Location` を表現
//!   できない
//! - [`crate::extension::UpgradeHandler`][]: 長時間接続への委譲判定専用
//!
//! `docs/spec/04-requirements.md` REQ-2 は「既存 3 拡張点で表現できない場合にのみ
//! 新規 trait の追加を検討する」と定めており、本 trait はこの例外条件を満たす
//! （設計判断の詳細は `docs/design/interceptor-extension-point.md`）。[`crate::server::Handler`]
//! が既に確立している「3 拡張点の対象外の既定レスポンダ差し込み口」という
//! 前例と同じ「レスポンダ系シーム」ファミリーとして位置づける。
//!
//! # 契約
//!
//! [`Middleware`][crate::extension::Middleware] と同じく同期 API（dyn 互換のため）。
//! 同期ブロッキング I/O を行ってはならない（PoC-3 実測でスループットが最大 25%
//! 劣化する、`crate::extension` モジュール doc・`AGENTS.md` を参照）。カスタム
//! 404 ページ等、レスポンス body に静的コンテンツを使う場合は起動時にメモリへ
//! プリロードしておく（下の doc test を参照）。
//!
//! # 評価順序（`crate::server::handle_connection_with_permit`）
//!
//! ```text
//! 1. Middleware::on_request
//! 2. RequestGate::check（フェイルクローズ。Reject 応答は Interceptor を一切通さない）
//! 3. UpgradeHandler::matches
//! 3.5. Interceptor::intercept（新規。登録順、最初の Some が勝つ）
//! 4. plugin::try_intercept（intercept が Some なら skip）
//! 5. Handler::handle / handle_streaming（同上 skip）
//! 5.4. Interceptor::map_response（新規。登録順に逐次適用。`handle_streaming` が
//!      `Some` を返した経路では `crate::server::write_streaming_response` の
//!      ヘッド確定時に同じく登録順で適用する。下の「ストリーミング応答への
//!      適用」節を参照、イシュー #434）
//! 5.5. plugin::finalize_response（CORS → 圧縮。map_response の後。ストリーミング
//!      応答には未適用のまま、イシュー #319 のスコープ外指定を維持）
//! 6. レスポンス書き込み → Middleware::on_response
//! ```
//!
//! - **`RequestGate` より後**: ゲートの既定拒否（フェイルクローズ）をユーザー
//!   コードで迂回できないようにする（A01 アクセス制御対策、`RequestGate` の
//!   doc と同じ設計判断）
//! - **`UpgradeHandler` より後**: 確立済みの Upgrade 委譲・permit 引き継ぎ
//!   意味論（TASK-4.2）に触れない
//! - **`plugin::try_intercept` より前**: 利用者が登録済みプラグイン（例:
//!   `plugin-static`）の応答をインターセプトで先取りできる（末尾スラッシュ
//!   301 正規化のユースケースが成立する条件）
//! - **`map_response` は `finalize_response`（CORS → 圧縮）より前**: CORS
//!   ヘッダ付与・gzip 圧縮は改変後の最終 body に対して適用されるべきため
//!
//! # `map_response` を通さない応答（fail-closed）
//!
//! `finalize_response`（イシュー #305）と同一の設計判断として、以下の応答は
//! `Interceptor` の対象外とする:
//!
//! - `RequestGate` 拒否応答
//! - パースエラー応答（400 等、コネクション処理中に確定するもの）
//! - Upgrade 委譲失敗時の 501 応答・shutdown 中の 503 応答
//!
//! # ストリーミング応答への適用（ステータス・ヘッダのみ、body 破棄、イシュー #434）
//!
//! [`crate::server::Handler::handle_streaming`]（イシュー #319）が返す応答は
//! `Response` 型を前提とする通常経路（上の 5.4）を通らないが、`map_response`
//! 自体は `crate::server::write_streaming_response` がヘッド確定時（HTTP/1.0・
//! HTTP/1.1 共通、1 回のみ）に登録順で適用する。ステータス・
//! `Content-Type`・追加ヘッダ（`Response::with_header` 等）の改変はここで
//! 反映されるが、ストリーミング応答の実体（body）は producer タスクが
//! [`crate::streaming::BodyWriter`] 経由で逐次供給し chunked framing は
//! コアが直接組み立てるため、`map_response` が返した `Response` の **body は
//! 反映されず破棄される**。body 差し替えを許すとバックプレッシャ（bounded
//! mpsc）・応答完全性契約（`finish` 省略時は終端チャンクなしで打ち切り
//! クローズ、`crate::streaming` モジュール doc の「応答完全性」節）と
//! 両立できず、body 全体のバッファリングが必要になり #319 の設計と矛盾する
//! ため、この制約は意図的な設計判断（新規フック追加・全 body バッファリング
//! は不採用、`docs/design/interceptor-extension-point.md` を参照）。
//!
//! `map_response` 適用後のステータスは以降のすべての判定（`Response::
//! is_bodyless_status` による 1xx/204/304 の body 送出スキップ含む）に一貫
//! 使用する。インターセプタが 200 → 204 等へ書き換えた場合、ヘッダ側の
//! framing 抑制と body 送出スキップが対で成立し、レスポンス分割類の脅威
//! （`Response::serialize_chunked_head` doc の RFC 9112 §6.3 コメントと同一
//! 脅威）を構造的に防ぐ。
//!
//! `plugin::finalize_response`（CORS → 圧縮）はストリーミング応答には
//! 引き続き適用しない（gzip はストリーム body に適用不能・CORS は別途設計が
//! 必要なため、イシュー #319 のスコープ外指定を維持）。
//!
//! # pay-for-what-you-use との整合
//!
//! feature ゲート **不要**。外部依存ゼロの純コア機能であり、`Handler`・3 拡張点
//! と同じく実装ゼロなら実行時コストもゼロ（未登録時は空 `Vec` の走査のみ）。
//!
//! # Examples
//!
//! 末尾スラッシュを 301 で正規化する `intercept` の例:
//!
//! ```
//! use fandhe_backend_core::interceptor::Interceptor;
//! use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
//! use fandhe_backend_http::response::Response;
//!
//! /// `/foo/` → `/foo` へ 301 リダイレクトするトイ実装。
//! struct TrailingSlashRedirect;
//!
//! impl Interceptor for TrailingSlashRedirect {
//!     fn name(&self) -> &'static str {
//!         "trailing-slash-redirect"
//!     }
//!
//!     fn intercept(&self, head: &RequestHead, _body: &[u8]) -> Option<Response> {
//!         let path = head.path();
//!         if path.len() > 1 && path.ends_with('/') {
//!             let normalized = path.trim_end_matches('/').to_string();
//!             return Response::redirect(301, normalized).ok();
//!         }
//!         None
//!     }
//! }
//!
//! let interceptor = TrailingSlashRedirect;
//! let buf = b"GET /foo/ HTTP/1.1\r\n\r\n";
//! let head = match parse_request_head(buf).unwrap() {
//!     ParseOutcome::Complete { head, .. } => head,
//!     ParseOutcome::Incomplete => unreachable!(),
//! };
//! let response = interceptor.intercept(&head, b"").expect("should redirect");
//! assert_eq!(response.status, 301);
//! ```
//!
//! 404 応答の body をカスタムページへ差し替える `map_response` の例
//! （カスタムページは起動時にプリロードしておく契約）:
//!
//! ```
//! use fandhe_backend_core::interceptor::Interceptor;
//! use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
//! use fandhe_backend_http::response::Response;
//!
//! /// 404 応答の body だけを差し替えるトイ実装。ステータス・ヘッダは既存のまま。
//! struct Custom404 {
//!     page: Vec<u8>,
//! }
//!
//! impl Interceptor for Custom404 {
//!     fn name(&self) -> &'static str {
//!         "custom-404"
//!     }
//!
//!     fn map_response(&self, _head: &RequestHead, response: Response) -> Response {
//!         if response.status == 404 {
//!             Response::new(404, self.page.clone())
//!         } else {
//!             response
//!         }
//!     }
//! }
//!
//! let interceptor = Custom404 { page: b"<html>not found</html>".to_vec() };
//! let buf = b"GET / HTTP/1.1\r\n\r\n";
//! let head = match parse_request_head(buf).unwrap() {
//!     ParseOutcome::Complete { head, .. } => head,
//!     ParseOutcome::Incomplete => unreachable!(),
//! };
//! let mapped = interceptor.map_response(&head, Response::empty(404));
//! assert_eq!(mapped.body, b"<html>not found</html>");
//! ```

use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;

/// ユーザー向けのリクエストインターセプト・レスポンス改変拡張点。
///
/// 本モジュール doc の評価順序・fail-closed 除外・同期契約を必ず参照すること。
/// 既定実装は両フックとも no-op であり、片方のみをオーバーライドして使うことも
/// できる（`Middleware` の 2 フック 1 trait と同じ流儀）。
pub trait Interceptor: Send + Sync {
    /// 診断・ログ表示用の静的識別名。リクエスト内容（トークン・PII）を
    /// 含めてはならない（[`crate::extension::Middleware::name`] と同一契約）。
    fn name(&self) -> &'static str;

    /// ルーティング・プラグイン評価前のインターセプトフック。
    ///
    /// `Some(response)` を返すと、以降の `plugin::try_intercept`・`Handler`
    /// 呼び出しをスキップして応答を確定させる。複数 `Interceptor` が登録
    /// されている場合は登録順に評価し、最初に `Some` を返した実装が勝つ
    /// （以降の実装の `intercept` は呼ばれない）。既定実装は常に `None`
    /// （素通し、後方互換）。
    fn intercept(&self, _head: &RequestHead, _body: &[u8]) -> Option<Response> {
        None
    }

    /// 最終応答（`intercept` 応答・パスインターセプト型プラグイン応答・
    /// `Handler` 応答のいずれか）確定後、`finalize_response`（CORS → 圧縮）
    /// 適用前の改変フック。
    ///
    /// 複数 `Interceptor` が登録されている場合は登録順に逐次適用する
    /// （各実装は前段の戻り値を受け取る）。既定実装は受け取った `response`
    /// をそのまま返す（no-op、後方互換）。本モジュール doc の「`map_response`
    /// を通さない応答」に列挙した応答には適用されない。
    ///
    /// [`crate::server::Handler::handle_streaming`] によるストリーミング応答
    /// にも適用されるが（イシュー #434）、`status`・ヘッダのみが反映され
    /// **body は反映されず破棄される**。詳細な契約は本モジュール doc の
    /// 「ストリーミング応答への適用」節を参照。
    fn map_response(&self, _head: &RequestHead, response: Response) -> Response {
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    /// `Interceptor` が object safe（dyn 互換）であることをコンパイル時に検証する。
    ///
    /// `Server`（`crate::server`）は `Vec<Box<dyn Interceptor>>` として拡張点を
    /// 保持する前提であり、この性質が壊れると設計そのものが成立しない。
    fn _assert_object_safe(_i: &dyn Interceptor) {}

    /// `Send + Sync` 境界が付与されていることを静的に検証する（複数ワーカー
    /// スレッドから `Arc<dyn Interceptor>` として共有参照される前提のため）。
    fn _assert_send_sync<T: Send + Sync + ?Sized>() {}
    #[allow(dead_code)]
    fn _assert_bounds() {
        _assert_send_sync::<dyn Interceptor>();
    }

    fn head_from(buf: &[u8]) -> RequestHead {
        match parse_request_head(buf).expect("parse should succeed") {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("expected Complete"),
        }
    }

    /// 両フックともオーバーライドしないトイ実装。既定実装の no-op 契約を検証する。
    struct NoopInterceptor;

    impl Interceptor for NoopInterceptor {
        fn name(&self) -> &'static str {
            "noop"
        }
    }

    #[test]
    fn default_intercept_is_none() {
        let interceptor = NoopInterceptor;
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        assert!(interceptor.intercept(&head, b"").is_none());
    }

    #[test]
    fn default_map_response_is_identity() {
        let interceptor = NoopInterceptor;
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        let response = Response::new(200, b"hello".to_vec());
        let mapped = interceptor.map_response(&head, response);
        assert_eq!(mapped.status, 200);
        assert_eq!(mapped.body, b"hello");
    }

    struct RedirectRoot;

    impl Interceptor for RedirectRoot {
        fn name(&self) -> &'static str {
            "redirect-root"
        }

        fn intercept(&self, head: &RequestHead, _body: &[u8]) -> Option<Response> {
            if head.path() == "/old" {
                Response::redirect(301, "/new").ok()
            } else {
                None
            }
        }
    }

    #[test]
    fn intercept_returns_redirect_for_matching_path() {
        let interceptor = RedirectRoot;
        let head = head_from(b"GET /old HTTP/1.1\r\n\r\n");
        let response = interceptor.intercept(&head, b"").expect("should redirect");
        assert_eq!(response.status, 301);
        assert_eq!(response.header("location"), Some("/new"));
    }

    #[test]
    fn intercept_returns_none_for_non_matching_path() {
        let interceptor = RedirectRoot;
        let head = head_from(b"GET /elsewhere HTTP/1.1\r\n\r\n");
        assert!(interceptor.intercept(&head, b"").is_none());
    }

    struct StatusRewrite404;

    impl Interceptor for StatusRewrite404 {
        fn name(&self) -> &'static str {
            "status-rewrite-404"
        }

        fn map_response(&self, _head: &RequestHead, response: Response) -> Response {
            if response.status == 404 {
                Response::new(404, b"custom not found".to_vec())
            } else {
                response
            }
        }
    }

    #[test]
    fn map_response_rewrites_matching_status() {
        let interceptor = StatusRewrite404;
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        let mapped = interceptor.map_response(&head, Response::empty(404));
        assert_eq!(mapped.body, b"custom not found");
    }

    #[test]
    fn map_response_leaves_non_matching_status_untouched() {
        let interceptor = StatusRewrite404;
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        let mapped = interceptor.map_response(&head, Response::new(200, b"ok".to_vec()));
        assert_eq!(mapped.status, 200);
        assert_eq!(mapped.body, b"ok");
    }
}
