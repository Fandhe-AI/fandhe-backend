//! `fandhe-backend-plugin-cors`: CORS プラグイン（イシュー #305）。
//!
//! 拡張点対応: レスポンス後処理型（finalize_response）
//! （3 拡張点 trait には非該当。固定シグネチャシームへの閉包根拠は
//! `docs/design/plugin-boundary.md` 5.9 節、機械可読宣言の規約は
//! `docs/design/dependency-graph-contract.md` 3 節、イシュー #305）
//!
//! プリフライトは `crates/core/src/plugin.rs` の `finalize_response` 新設
//! シームではなく、`crates/routes` の `Router::options_fallback`（イシュー
//! #304）へ利用者が直接配線する 2 層構成（下記「背景・境界設計」節を参照）。
//!
//! # 背景・境界設計
//!
//! `Middleware::on_response` はレスポンスへの参照を持たない観測専用契約の
//! ため、CORS ヘッダ付与には使えない（`crates/core/src/extension.rs` の
//! `Middleware` doc を参照）。CORS は 2 つの独立した処理に分解して配線する:
//!
//! 1. **プリフライト（`OPTIONS` + `Origin` + `Access-Control-Request-Method`）**:
//!    利用者が [`preflight_response`] を `fandhe_backend_routes::Router::options_fallback`
//!    （イシュー #304）へ配線する。フォールバックフックは対象パスに登録済みの
//!    method 一覧（`AllowedMethods`）を受け取れるため、`Access-Control-Allow-Methods`
//!    の既定値を実登録メソッドから導出でき、設定と実体の乖離が起きない
//! 2. **実リクエストへのヘッダ付与**: コア側（`crates/core`）が `cors` feature
//!    有効時のみ [`apply_cors_headers`] を「レスポンス後処理型」シーム経由で
//!    全レスポンスに適用する（`crate::plugin::finalize_response` を参照）
//!
//! # コアへの配線について（循環依存の回避）
//!
//! 本クレート単体は `fandhe-backend-core` に依存しない。コアが本クレートへ
//! `optional = true` + `dep:` 構文の依存を張る（`cors` feature 有効時のみ）
//! ため、逆方向の依存を張ると循環依存になる（`crates/plugin-websocket` と
//! 同一の非循環パターン、`docs/design/plugin-boundary.md` 6.1 節・
//! `scripts/dep-direction-check.sh` が機械的に検証する）。依存方向は
//! `server → routes → http::*` の一方向であり、本クレートは `http` のみに
//! 依存する下位層として振る舞う。
//!
//! # フェイルクローズ設計（OWASP A01/A03/A04/A05、`.claude/rules/security.md`）
//!
//! - 許可オリジンは完全一致（バイト一致）のみ。正規化・大文字小文字同一視・
//!   部分一致はしない（`Router` のパス照合方針と同一の「正規化しない」判断）
//! - `*`（[`CorsOrigins::Any`]）は明示 opt-in のみで有効化でき、
//!   `allow_credentials(true)` との併用は [`CorsConfig::builder`] の
//!   `build()` 呼び出し時点で `Err`（実行時の劣化ではなく構築時拒否、
//!   Fetch 仕様が定める「credentials 付き全開放」の禁止を型レベルで強制）
//! - 不許可 Origin は「ヘッダを一切付与しない」（実リクエスト）・
//!   「CORS ヘッダなしの `403`」（プリフライト）のいずれかで拒否理由の詳細を
//!   返さない
//! - Origin エコーは外部入力のレスポンス反映だが、
//!   [`fandhe_backend_http::response::Response::with_header`] の
//!   CR/LF/NUL 検証を必ず経由する（パーサの制御文字拒否と合わせた二重防御、
//!   レスポンス分割対策）。検証に失敗した場合は当該ヘッダを付与しない側へ
//!   倒し、panic させない

use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::{AllowedMethods, Response};

/// 許可オリジンの集合（[`CorsConfig`] の一部）。
///
/// 完全一致（バイト一致）のリストが既定であり、ワイルドカードは
/// [`CorsConfigBuilder::allow_any_origin`] による明示 opt-in でのみ選べる
/// （フェイルクローズ、モジュール冒頭の doc を参照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsOrigins {
    /// 完全一致（バイト一致）で照合するオリジンのリスト。
    List(Vec<String>),
    /// `Access-Control-Allow-Origin: *`。`allow_credentials(true)` との併用は
    /// [`CorsConfig::builder`] の `build()` で拒否される。
    Any,
}

/// [`CorsConfig::builder`] の `build()` が返す構築失敗理由。
///
/// [`fandhe_backend_http::response::HeaderError`] と同様、`Display` は
/// 拒否理由のみを述べる（`.claude/rules/security.md` のログ方針と整合）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsConfigError {
    /// `allow_any_origin()`（[`CorsOrigins::Any`]）と `allow_credentials(true)`
    /// を同時に指定した。credentials 付きで全オリジンへ応答を開放する
    /// 構成は Fetch 仕様上意味を成さず、最悪構成（トークン窃取経路）を
    /// 型レベルで排除するため構築時に拒否する。
    AnyOriginWithCredentials,
}

impl std::fmt::Display for CorsConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AnyOriginWithCredentials => {
                f.write_str("allow_any_origin() と allow_credentials(true) は併用できない")
            }
        }
    }
}

impl std::error::Error for CorsConfigError {}

/// 検証済み CORS 設定（[`CorsConfig::builder`] 経由でのみ構築できる）。
///
/// [`preflight_response`] / [`apply_cors_headers`] の両方が本設定を参照する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorsConfig {
    origins: CorsOrigins,
    /// `Access-Control-Allow-Methods` の明示指定値。`None`（既定）の場合、
    /// [`preflight_response`] は呼び出し元が渡す `AllowedMethods`（対象パスの
    /// 実登録メソッド）を反映する（モジュール冒頭 doc の「設定と実体の
    /// 乖離が起きない」設計意図）。
    methods: Option<Vec<String>>,
    /// `Access-Control-Allow-Headers` に列挙するヘッダ名。空なら出力しない。
    headers: Vec<String>,
    /// `true` の場合 `Access-Control-Allow-Credentials: true` を実リクエスト
    /// 応答に付与する（`CorsOrigins::Any` との併用は構築時に拒否済み）。
    credentials: bool,
    /// `Access-Control-Max-Age`（秒）。`None` なら出力しない。
    max_age: Option<u64>,
    /// `Access-Control-Expose-Headers` に列挙するヘッダ名。空なら出力しない。
    expose_headers: Vec<String>,
}

impl CorsConfig {
    /// [`CorsConfigBuilder`] を返す。
    ///
    /// ```
    /// use fandhe_backend_plugin_cors::CorsConfig;
    ///
    /// let config = CorsConfig::builder()
    ///     .allow_origin("https://app.example.com")
    ///     .allow_credentials(true)
    ///     .build()
    ///     .unwrap();
    /// let _ = config;
    /// ```
    #[must_use]
    pub fn builder() -> CorsConfigBuilder {
        CorsConfigBuilder::default()
    }
}

/// [`CorsConfig`] のビルダー。既定値は「オリジン未登録（何も許可しない）・
/// credentials 無効・`Access-Control-Allow-Methods` は呼び出し元の
/// `AllowedMethods` を反映・追加ヘッダなし・`Max-Age` 未指定」（フェイルクローズ）。
#[derive(Debug, Clone, Default)]
pub struct CorsConfigBuilder {
    origins: Vec<String>,
    any_origin: bool,
    methods: Option<Vec<String>>,
    headers: Vec<String>,
    credentials: bool,
    max_age: Option<u64>,
    expose_headers: Vec<String>,
}

impl CorsConfigBuilder {
    /// 許可オリジンを 1 件追加する（完全一致・バイト一致、複数回呼び出し可）。
    #[must_use]
    pub fn allow_origin(mut self, origin: impl Into<String>) -> Self {
        self.origins.push(origin.into());
        self
    }

    /// `Access-Control-Allow-Origin: *` を有効化する opt-in（モジュール冒頭 doc
    /// の「明示 opt-in のみ」を参照）。`allow_credentials(true)` と併用した
    /// 場合は `build()` が `Err(CorsConfigError::AnyOriginWithCredentials)` を返す。
    #[must_use]
    pub fn allow_any_origin(mut self) -> Self {
        self.any_origin = true;
        self
    }

    /// `Access-Control-Allow-Methods` を明示指定する。未呼び出しなら
    /// [`preflight_response`] 呼び出し元が渡す `AllowedMethods`（実登録
    /// メソッド）を既定値として使う。
    #[must_use]
    pub fn allow_methods(mut self, methods: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.methods = Some(methods.into_iter().map(Into::into).collect());
        self
    }

    /// `Access-Control-Allow-Headers` に列挙するヘッダ名を指定する。
    #[must_use]
    pub fn allow_headers(mut self, headers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.headers = headers.into_iter().map(Into::into).collect();
        self
    }

    /// `Access-Control-Allow-Credentials: true` を実リクエスト応答へ付与するかどうか。
    #[must_use]
    pub fn allow_credentials(mut self, allow: bool) -> Self {
        self.credentials = allow;
        self
    }

    /// `Access-Control-Max-Age`（秒）を指定する。
    #[must_use]
    pub fn max_age(mut self, secs: u64) -> Self {
        self.max_age = Some(secs);
        self
    }

    /// `Access-Control-Expose-Headers` に列挙するヘッダ名を指定する。
    #[must_use]
    pub fn expose_headers(mut self, headers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.expose_headers = headers.into_iter().map(Into::into).collect();
        self
    }

    /// 検証付きで [`CorsConfig`] を構築する（フェイルクローズ）。
    ///
    /// `allow_any_origin()` と `allow_credentials(true)` の併用のみを拒否する
    /// （`AllowedMethods::from_methods` 等と同様、`Response` 構築系 API と
    /// 同一のフェイルクローズ設計。他のヘッダ値の妥当性は送出時に
    /// `Response::with_header` が最終防御として検証する、モジュール冒頭 doc
    /// を参照）。
    ///
    /// ```
    /// use fandhe_backend_plugin_cors::{CorsConfig, CorsConfigError};
    ///
    /// let err = CorsConfig::builder()
    ///     .allow_any_origin()
    ///     .allow_credentials(true)
    ///     .build()
    ///     .unwrap_err();
    /// assert_eq!(err, CorsConfigError::AnyOriginWithCredentials);
    /// ```
    pub fn build(self) -> Result<CorsConfig, CorsConfigError> {
        if self.any_origin && self.credentials {
            return Err(CorsConfigError::AnyOriginWithCredentials);
        }
        let origins = if self.any_origin {
            CorsOrigins::Any
        } else {
            CorsOrigins::List(self.origins)
        };
        Ok(CorsConfig {
            origins,
            methods: self.methods,
            headers: self.headers,
            credentials: self.credentials,
            max_age: self.max_age,
            expose_headers: self.expose_headers,
        })
    }
}

/// 完全一致（バイト一致）でのオリジン許可判定。
fn origin_allowed(config: &CorsConfig, origin: &str) -> bool {
    match &config.origins {
        CorsOrigins::Any => true,
        CorsOrigins::List(list) => list.iter().any(|allowed| allowed == origin),
    }
}

/// `Response::with_header` の `Err` を「当該ヘッダを付与しない」側へ倒す
/// ヘルパ（モジュール冒頭 doc の「フェイルクローズ設計」を参照）。
/// `with_header` は失敗時に `self` を返さない契約のため、呼び出し前に
/// `Response` を複製し、失敗時はその複製（変更前の状態）を返す。
fn try_add_header(response: Response, name: &str, value: impl Into<String>) -> Response {
    let fallback = response.clone();
    response.with_header(name, value).unwrap_or(fallback)
}

/// リクエストが CORS プリフライトか判定する。
///
/// `OPTIONS` メソッド・`Origin` ヘッダ・`Access-Control-Request-Method`
/// ヘッダの 3 条件がすべて揃った場合のみ `true`（Fetch 仕様のプリフライト
/// 判定基準）。`fandhe_backend_routes::Router::options_fallback` へ渡す
/// ハンドラ内で、明示登録 OPTIONS ルートとの切り分けに使う想定。
///
/// ```
/// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// use fandhe_backend_plugin_cors::is_preflight;
///
/// let buf = b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccess-Control-Request-Method: POST\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// assert!(is_preflight(&head));
///
/// let buf = b"OPTIONS /todos HTTP/1.1\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// assert!(!is_preflight(&head));
/// ```
#[must_use]
pub fn is_preflight(head: &RequestHead) -> bool {
    head.method().eq_ignore_ascii_case("OPTIONS")
        && head.header("origin").is_some()
        && head.header("access-control-request-method").is_some()
}

/// CORS プリフライト応答を組み立てる。
///
/// `fandhe_backend_routes::Router::options_fallback` の
/// `Fn(&RequestHead, &AllowedMethods, &[u8]) -> Response` シグネチャに
/// そのまま適合する（`allow` は対象パスの実登録メソッド一覧）。呼び出し元は
/// 通常 `router.options_fallback(|head, allow, body| preflight_response(head, allow, &config))`
/// の形でクロージャ内に閉じ込めて配線する。
///
/// # 判定・応答
///
/// - `Origin` ヘッダがない素の `OPTIONS`（CORS プリフライトではない
///   ディスカバリ目的の `OPTIONS`）は、`Router` の登録済みパスに対する
///   通常の未マッチ `OPTIONS` 応答と同じ `405` + `Allow`（`allow` を反映）
///   を返す。`Origin` 不許可、または `Access-Control-Request-Method` が
///   `config.allow_methods` 相当（未指定時は `allow`）に含まれない場合は
///   CORS ヘッダなしの `403`
/// - 許可された場合は `204` + `Access-Control-Allow-Origin`
///   （`CorsOrigins::Any` なら `*`、リストならオリジンをエコーし
///   `Vary: Origin` も付与）+ `Access-Control-Allow-Methods`
///   （`config` 未指定時は `allow` を反映）+ `Access-Control-Allow-Headers`
///   （`config.allow_headers` 設定時のみ）+ `Access-Control-Max-Age`
///   （`config.max_age` 設定時のみ）+（`config.credentials` が `true` の
///   場合）`Access-Control-Allow-Credentials: true`（ブラウザは credential
///   付きクロスオリジンリクエストを進める前にプリフライト応答へこの
///   ヘッダーを要求するため、`apply_cors_headers` の実リクエスト応答と
///   同じ付与が必須）
///
/// ```
/// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// use fandhe_backend_http::response::AllowedMethods;
/// use fandhe_backend_plugin_cors::{CorsConfig, preflight_response};
///
/// let config = CorsConfig::builder()
///     .allow_origin("https://app.example.com")
///     .build()
///     .unwrap();
/// let allow = AllowedMethods::from_methods(["GET".to_string(), "POST".to_string()]).unwrap();
///
/// let buf = b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccess-Control-Request-Method: POST\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// let res = preflight_response(&head, &allow, &config);
/// assert_eq!(res.status, 204);
///
/// // 不許可オリジンは 403（CORS ヘッダなし）。
/// let buf = b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://evil.example\r\nAccess-Control-Request-Method: POST\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// let res = preflight_response(&head, &allow, &config);
/// assert_eq!(res.status, 403);
///
/// // Origin ヘッダのない素の OPTIONS は Router 既定と同じ 405 + Allow。
/// let buf = b"OPTIONS /todos HTTP/1.1\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// let res = preflight_response(&head, &allow, &config);
/// assert_eq!(res.status, 405);
/// ```
#[must_use]
pub fn preflight_response(
    head: &RequestHead,
    allow: &AllowedMethods,
    config: &CorsConfig,
) -> Response {
    // `Origin` ヘッダがない場合は CORS プリフライトではなく、CORS 目的でない
    // 素の OPTIONS ディスカバリと解釈する。`Router::options_fallback` 未配線時の
    // 既定挙動（405 + `Allow`、`crates/routes/src/lib.rs`）と一致させ、
    // このハンドラを配線しただけで素の OPTIONS が壊れないようにする
    // （Cursor Bugbot 指摘、PR #330）。
    let Some(origin) = head.header("origin") else {
        return Response::empty(405).with_allow(allow.clone());
    };
    let req_method = head.header("access-control-request-method").unwrap_or("");

    if !origin_allowed(config, origin) {
        return Response::empty(403);
    }

    let effective_methods = config.methods.clone().unwrap_or_else(|| {
        allow
            .to_header_value()
            .split(", ")
            .map(str::to_owned)
            .collect()
    });
    if !effective_methods.iter().any(|m| m == req_method) {
        return Response::empty(403);
    }

    let mut response = Response::empty(204);
    response = match &config.origins {
        CorsOrigins::Any => try_add_header(response, "Access-Control-Allow-Origin", "*"),
        CorsOrigins::List(_) => {
            let response = try_add_header(response, "Access-Control-Allow-Origin", origin);
            try_add_header(response, "Vary", "Origin")
        }
    };
    response = try_add_header(
        response,
        "Access-Control-Allow-Methods",
        effective_methods.join(", "),
    );
    if !config.headers.is_empty() {
        response = try_add_header(
            response,
            "Access-Control-Allow-Headers",
            config.headers.join(", "),
        );
    }
    if let Some(max_age) = config.max_age {
        response = try_add_header(response, "Access-Control-Max-Age", max_age.to_string());
    }
    // credentials 付きクロスオリジンリクエストはブラウザがプリフライト応答に
    // このヘッダーを要求する（`apply_cors_headers` と同一条件、Cursor Bugbot
    // 指摘、PR #330）。付与し忘れると許可 origin・method でも credential 付き
    // fetch/cookie フローがブラウザ側でブロックされる。
    if config.credentials {
        response = try_add_header(response, "Access-Control-Allow-Credentials", "true");
    }
    response
}

/// 実リクエストのレスポンスへ CORS ヘッダを付与する。
///
/// コア側（`crates/core`、`cors` feature 有効時）の「レスポンス後処理型」
/// シーム（`crate::plugin::finalize_response`）が全レスポンスに対して呼ぶ
/// 想定の関数（モジュール冒頭 doc を参照）。
///
/// - `Origin` ヘッダなし、または不許可オリジンの場合は `response` を
///   無改変で返す（フェイルクローズ。ブラウザ側でブロックさせる設計）
/// - 許可オリジンの場合は `Access-Control-Allow-Origin`
///   （`CorsOrigins::Any` なら `*`、リストならオリジンをエコーし
///   `Vary: Origin` も付与）+（`config.credentials` が `true` の場合）
///   `Access-Control-Allow-Credentials: true` +（`config.expose_headers`
///   が非空の場合）`Access-Control-Expose-Headers` を付与する
///
/// ```
/// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// use fandhe_backend_http::response::Response;
/// use fandhe_backend_plugin_cors::{CorsConfig, apply_cors_headers};
///
/// let config = CorsConfig::builder()
///     .allow_origin("https://app.example.com")
///     .build()
///     .unwrap();
///
/// let buf = b"GET /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// let res = apply_cors_headers(&head, &config, Response::empty(200));
/// let text = String::from_utf8(res.serialize(false)).unwrap();
/// assert!(text.contains("Access-Control-Allow-Origin: https://app.example.com\r\n"));
///
/// // Origin ヘッダがなければ無改変。
/// let buf = b"GET /todos HTTP/1.1\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// let res = apply_cors_headers(&head, &config, Response::empty(200));
/// let text = String::from_utf8(res.serialize(false)).unwrap();
/// assert!(!text.contains("Access-Control-Allow-Origin"));
/// ```
#[must_use]
pub fn apply_cors_headers(head: &RequestHead, config: &CorsConfig, response: Response) -> Response {
    let Some(origin) = head.header("origin") else {
        return response;
    };
    if !origin_allowed(config, origin) {
        return response;
    }

    let mut response = match &config.origins {
        CorsOrigins::Any => try_add_header(response, "Access-Control-Allow-Origin", "*"),
        CorsOrigins::List(_) => {
            let response = try_add_header(response, "Access-Control-Allow-Origin", origin);
            try_add_header(response, "Vary", "Origin")
        }
    };
    if config.credentials {
        response = try_add_header(response, "Access-Control-Allow-Credentials", "true");
    }
    if !config.expose_headers.is_empty() {
        response = try_add_header(
            response,
            "Access-Control-Expose-Headers",
            config.expose_headers.join(", "),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    fn head_from(buf: &[u8]) -> RequestHead {
        match parse_request_head(buf).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            other => panic!("unexpected parse outcome: {other:?}"),
        }
    }

    // --- CorsConfig / builder ---

    #[test]
    fn build_rejects_any_origin_with_credentials() {
        let err = CorsConfig::builder()
            .allow_any_origin()
            .allow_credentials(true)
            .build()
            .unwrap_err();
        assert_eq!(err, CorsConfigError::AnyOriginWithCredentials);
    }

    #[test]
    fn build_allows_credentials_with_origin_list() {
        let config = CorsConfig::builder()
            .allow_origin("https://app.example.com")
            .allow_credentials(true)
            .build()
            .unwrap();
        assert!(config.credentials);
    }

    #[test]
    fn build_allows_any_origin_without_credentials() {
        let config = CorsConfig::builder().allow_any_origin().build().unwrap();
        assert_eq!(config.origins, CorsOrigins::Any);
    }

    // --- is_preflight ---

    #[test]
    fn is_preflight_requires_all_three_conditions() {
        let head = head_from(
            b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccess-Control-Request-Method: POST\r\n\r\n",
        );
        assert!(is_preflight(&head));

        let missing_origin =
            head_from(b"OPTIONS /todos HTTP/1.1\r\nAccess-Control-Request-Method: POST\r\n\r\n");
        assert!(!is_preflight(&missing_origin));

        let missing_acrm =
            head_from(b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\n\r\n");
        assert!(!is_preflight(&missing_acrm));

        let wrong_method =
            head_from(b"GET /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\n\r\n");
        assert!(!is_preflight(&wrong_method));
    }

    // --- preflight_response ---

    #[test]
    fn preflight_response_allows_registered_origin_and_reflects_router_methods() {
        let config = CorsConfig::builder()
            .allow_origin("https://app.example.com")
            .build()
            .unwrap();
        let allow = AllowedMethods::from_methods(["GET".to_string(), "POST".to_string()]).unwrap();
        let head = head_from(
            b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccess-Control-Request-Method: POST\r\n\r\n",
        );

        let res = preflight_response(&head, &allow, &config);
        assert_eq!(res.status, 204);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Access-Control-Allow-Origin: https://app.example.com\r\n"));
        assert!(text.contains("Access-Control-Allow-Methods: GET, POST\r\n"));
        assert!(text.contains("Vary: Origin\r\n"));
    }

    #[test]
    fn preflight_response_rejects_unlisted_origin_without_cors_headers() {
        let config = CorsConfig::builder()
            .allow_origin("https://app.example.com")
            .build()
            .unwrap();
        let allow = AllowedMethods::from_methods(["GET".to_string()]).unwrap();
        let head = head_from(
            b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://evil.example\r\nAccess-Control-Request-Method: GET\r\n\r\n",
        );

        let res = preflight_response(&head, &allow, &config);
        assert_eq!(res.status, 403);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(!text.contains("Access-Control-Allow-Origin"));
    }

    #[test]
    fn preflight_response_rejects_disallowed_request_method() {
        let config = CorsConfig::builder()
            .allow_origin("https://app.example.com")
            .allow_methods(["GET"])
            .build()
            .unwrap();
        let allow =
            AllowedMethods::from_methods(["GET".to_string(), "DELETE".to_string()]).unwrap();
        let head = head_from(
            b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccess-Control-Request-Method: DELETE\r\n\r\n",
        );

        let res = preflight_response(&head, &allow, &config);
        assert_eq!(res.status, 403);
    }

    #[test]
    fn preflight_response_includes_credentials_header_when_configured() {
        // Cursor Bugbot 指摘（PR #330、High）: allow_credentials(true) 時、
        // apply_cors_headers と同様に preflight_response も
        // Access-Control-Allow-Credentials を付与しなければ、ブラウザは
        // credential 付きクロスオリジンリクエストを進めない。
        let config = CorsConfig::builder()
            .allow_origin("https://app.example.com")
            .allow_credentials(true)
            .build()
            .unwrap();
        let allow = AllowedMethods::from_methods(["GET".to_string()]).unwrap();
        let head = head_from(
            b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccess-Control-Request-Method: GET\r\n\r\n",
        );

        let res = preflight_response(&head, &allow, &config);
        assert_eq!(res.status, 204);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Access-Control-Allow-Credentials: true\r\n"));
    }

    #[test]
    fn preflight_response_omits_credentials_header_when_not_configured() {
        let config = CorsConfig::builder()
            .allow_origin("https://app.example.com")
            .build()
            .unwrap();
        let allow = AllowedMethods::from_methods(["GET".to_string()]).unwrap();
        let head = head_from(
            b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccess-Control-Request-Method: GET\r\n\r\n",
        );

        let res = preflight_response(&head, &allow, &config);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(!text.contains("Access-Control-Allow-Credentials"));
    }

    #[test]
    fn preflight_response_without_origin_returns_405_with_allow() {
        // Cursor Bugbot 指摘（PR #330、Medium）: Origin ヘッダのない素の
        // OPTIONS（CORS 目的でないディスカバリ）は、preflight_response を
        // Router::options_fallback へ配線しても Router 既定の 405 + Allow を
        // 維持しなければならない（403 にすると素の OPTIONS が壊れる）。
        let config = CorsConfig::builder()
            .allow_origin("https://app.example.com")
            .build()
            .unwrap();
        let allow = AllowedMethods::from_methods(["GET".to_string(), "POST".to_string()]).unwrap();
        let head = head_from(b"OPTIONS /todos HTTP/1.1\r\n\r\n");

        let res = preflight_response(&head, &allow, &config);
        assert_eq!(res.status, 405);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Allow: GET, POST\r\n"));
        assert!(!text.contains("Access-Control-Allow-Origin"));
    }

    #[test]
    fn preflight_response_wildcard_does_not_echo_or_vary() {
        let config = CorsConfig::builder().allow_any_origin().build().unwrap();
        let allow = AllowedMethods::from_methods(["GET".to_string()]).unwrap();
        let head = head_from(
            b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://anywhere.example\r\nAccess-Control-Request-Method: GET\r\n\r\n",
        );

        let res = preflight_response(&head, &allow, &config);
        assert_eq!(res.status, 204);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Access-Control-Allow-Origin: *\r\n"));
        assert!(!text.contains("Vary"));
    }

    // --- apply_cors_headers ---

    #[test]
    fn apply_cors_headers_no_origin_leaves_response_untouched() {
        let config = CorsConfig::builder()
            .allow_origin("https://app.example.com")
            .build()
            .unwrap();
        let head = head_from(b"GET /todos HTTP/1.1\r\n\r\n");

        let res = apply_cors_headers(&head, &config, Response::empty(200));
        assert_eq!(res, Response::empty(200));
    }

    #[test]
    fn apply_cors_headers_disallowed_origin_leaves_response_untouched() {
        let config = CorsConfig::builder()
            .allow_origin("https://app.example.com")
            .build()
            .unwrap();
        let head = head_from(b"GET /todos HTTP/1.1\r\nOrigin: https://evil.example\r\n\r\n");

        let res = apply_cors_headers(&head, &config, Response::empty(200));
        assert_eq!(res, Response::empty(200));
    }

    #[test]
    fn apply_cors_headers_allowed_origin_adds_headers() {
        let config = CorsConfig::builder()
            .allow_origin("https://app.example.com")
            .allow_credentials(true)
            .expose_headers(["X-Total-Count"])
            .build()
            .unwrap();
        let head = head_from(b"GET /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\n\r\n");

        let res = apply_cors_headers(&head, &config, Response::empty(200));
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Access-Control-Allow-Origin: https://app.example.com\r\n"));
        assert!(text.contains("Access-Control-Allow-Credentials: true\r\n"));
        assert!(text.contains("Access-Control-Expose-Headers: X-Total-Count\r\n"));
        assert!(text.contains("Vary: Origin\r\n"));
    }

    // レスポンス分割リグレッションテスト（イシュー #305 受け入れ基準・
    // モジュール冒頭 doc の「二重防御」）: パーサ自体は CR/LF を含む
    // ヘッダ値を許容しないため、ここでは `apply_cors_headers` /
    // `preflight_response` が `Response::with_header` の検証結果（`Err` 時に
    // ヘッダを付与しない）を正しく尊重することを、許可オリジンリストへ
    // 直接 CRLF 入り文字列を登録する経路で確認する（設定側からの injection）。
    #[test]
    fn apply_cors_headers_rejects_crlf_in_configured_origin() {
        let config = CorsConfig::builder()
            .allow_origin("https://evil.example\r\nX-Injected: 1")
            .build()
            .unwrap();
        let head = head_from(b"GET /todos HTTP/1.1\r\n\r\n");
        // パーサ自体が CRLF を含むヘッダ値を許容しないため、外部入力経由の
        // Origin ではこのケースを再現できない。設定側 API から誤って CRLF を
        // 含む文字列を登録した場合でも、一致判定はバイト完全一致のため
        // このオリジンはそもそも一致しないことを確認する（縮退防御）。
        let evil_head = head_from(b"GET /todos HTTP/1.1\r\nOrigin: https://evil.example\r\n\r\n");
        let res = apply_cors_headers(&evil_head, &config, Response::empty(200));
        // 設定値（CRLF 入り）と一致しないため無改変。
        assert_eq!(res, Response::empty(200));
        let _ = head;
    }
}
