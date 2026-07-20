//! HTTP/1.1 レスポンス直列化（TASK-1.4-2 / #70）。
//!
//! コアの接続ループ（`crates/core/src/server.rs`）が `RequestGate::Reject`・
//! ハンドラ結果・エラー応答を HTTP/1.1 ワイヤフォーマットへ直列化する際に使う
//! 唯一の経路。`fandhe_backend_core::extension::GateOutcome` の doc に
//! 明記されているとおり、ステータス行の組み立て（reason phrase 付与等）は
//! このモジュールの責務であり、コアループ自身は文字列組み立てを行わない。
//!
//! # セキュリティ設計（レスポンス分割対策）
//!
//! [`Response`] は任意のヘッダ名・値を外部から受け取る API を**意図的に持たない**。
//! ステータスコードは `u16`、reason phrase は本モジュールの固定テーブルから引き、
//! body は生バイト列として `Content-Length` 付きで送出する。これにより CRLF を
//! 含む文字列がヘッダとして書き出される経路が構造的に存在せず、レスポンス分割・
//! ヘッダインジェクションを型レベルで排除する（`.claude/rules/security.md`）。
//!
//! 例外は 2 つある。1 つ目は [`Response::with_content_type`] であり、値を
//! `&'static str` に限定することで「呼び出し元（このクレート・上位クレートの
//! ソースコード）が静的に書いた文字列以外は絶対に渡せない」という型レベルの
//! 制約を維持したまま `Content-Type` ヘッダの付与を可能にする（TASK-2.1 / #18、
//! `crates/plugin-webrtc-proxy` のようにレスポンス種別ごとに固定の
//! `Content-Type` を返すプラグインの配線で必要になった）。
//!
//! 2 つ目が [`Response::with_allow`] である（TASK-177 / #177）。`Allow` ヘッダは
//! `crates/routes` の `Router::dispatch` が 405 応答時に払い出す許可メソッド
//! 一覧のように、呼び出し元が静的に書けない動的な値（登録済みルートの
//! method 集合）を送出する必要がある。そこで `&'static str` 限定の代わりに
//! [`AllowedMethods`] という**構築時検証済みの専用型**のみを受け取ることで、
//! CRLF を含む値がヘッダとして書き出される経路を型レベルで排除する。
//! `AllowedMethods::from_methods` は各要素を RFC 9110 の tchar（`request.rs`
//! の `is_tchar` を共有）のみで構成される非空文字列として検証し、1 件でも
//! 不正な token があれば構築自体を失敗させる（フェイルクローズ）。
//!
//! 3 つ目が [`Response::with_header`] である（イシュー #301）。CORS
//! （`Access-Control-Allow-Origin`）・`Set-Cookie`・`Location`・
//! `Cache-Control` のように、名前・値の両方が呼び出し元の実行時状態に
//! 依存し `&'static str` にも専用型にも収まらないヘッダが Phase 1 後続
//! 機能（CORS・リダイレクト・Set-Cookie）で必要になった。この API は
//! **静的リテラル限定を撤廃する代わりに構築時検証 + `Result` によるフェイル
//! クローズ**で同じ安全性水準を保つ: ヘッダ名は `is_tchar` 検証、値は
//! CR・LF・NUL に加え HTAB 以外の制御文字を拒否、`Content-Length` /
//! `Connection` / `Transfer-Encoding` はフレームワークがフレーミングを
//! 管理するため上書きを拒否する。検証失敗時は `Response` を変更せず
//! `Err` を返すため、CRLF を含む値がワイヤに出る経路は存在しない。
//!
//! 4 つ目が [`Response::with_set_cookie`] である（イシュー #303）。
//! `Set-Cookie` は `with_header` の CR/LF/NUL 検証だけでは RFC 6265 の
//! cookie-name（token）/ cookie-value（cookie-octet）文法までは検証できず、
//! 値中の `;` 等が `Set-Cookie` の属性区切り構文と衝突しうる。そこで
//! [`AllowedMethods`] と同じ構築時検証済み専用型 [`crate::cookie::SetCookie`]
//! のみを受け取ることで、infallible に（`SetCookie` 側で既に検証済みのため）
//! 安全な `Set-Cookie` 行を追加する。内部的には検証済みの `Set-Cookie` 値を
//! `with_header` と同じ `extra_headers` へ積むため、複数回の呼び出しで
//! 複数 `Set-Cookie` 行を挿入順に出力できる（`with_header` の追記
//! セマンティクスをそのまま利用）。
//!
//! 上記 4 例外（静的リテラル・構築時検証済み専用型 2 種・検証付き動的 API）を
//! 除き、**検証なしで**動的な値をヘッダとして送出する経路は今後も
//! 追加しない方針を維持する。
//!
//! [`Response::redirect`]（イシュー #302）は新たな送出経路を増やすものでは
//! なく、上記 3 つ目の例外である [`Response::with_header`] を薄くラップし、
//! POST-Redirect-GET 等の 3xx リダイレクトパターンを 1 呼び出しで組み立てる
//! ヘルパに過ぎない。Location 値の検証は `with_header` の検証経路をそのまま
//! 再利用する（検証基準の重複・将来的な乖離を防ぐ）。

/// `Allow` ヘッダ用の検証済みメソッド集合（TASK-177 / #177）。
///
/// RFC 9110 §15.5.6 / §10.2.1 は 405 応答に許可メソッド一覧を `Allow` ヘッダで
/// 示すことを要求する。この値は `crates/routes` の `Router::dispatch` が
/// 登録済みルートから動的に集めるため `Response::with_content_type` の
/// `&'static str` 限定手法が使えない。代わりに**構築時検証**で同等の型レベル
/// 保証を実現する: [`AllowedMethods::from_methods`] を通過した値のみが
/// この型のインスタンスになり、CRLF・SP・`:` 等の区切り文字を構造的に
/// 含まない（tchar のみで構成される）ことが保証される。
///
/// 直列化時（[`Response::serialize`]）はソート済み・重複排除済みの順序で
/// `", "` 区切りに結合する（決定的な出力・テスト安定性のため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedMethods {
    // 昇順ソート・重複排除済み。`from_methods` の唯一の構築経路がこの不変条件
    // を保証する（フィールドは非公開、直接構築不可）。
    methods: Vec<String>,
}

impl AllowedMethods {
    /// `methods` から検証済みの [`AllowedMethods`] を構築する。
    ///
    /// 各要素は非空かつ RFC 9110 tchar（`crate::request` の `is_tchar` と
    /// 同一判定基準）のみで構成される必要がある。1 件でも不正な token が
    /// あれば `None` を返す（フェイルクローズ。部分的に妥当な要素だけを
    /// 採用する緩和は行わない）。`methods` が空の場合も `None`（`Allow` は
    /// 最低 1 メソッドを示す必要がある）。
    ///
    /// 結果はソート + 重複排除され、直列化順序が呼び出し順に依存しない
    /// （`crates/routes` の `HashMap` 由来の非決定的な列挙順を安定化する）。
    ///
    /// ```
    /// use fandhe_backend_http::response::AllowedMethods;
    ///
    /// let allowed = AllowedMethods::from_methods(["POST".to_string(), "GET".to_string()]).unwrap();
    /// assert_eq!(allowed.to_header_value(), "GET, POST");
    ///
    /// // CRLF を含む値は構築段階で拒否される（レスポンス分割対策）。
    /// assert!(AllowedMethods::from_methods(["GET\r\nX-Evil: 1".to_string()]).is_none());
    ///
    /// // 空集合も拒否する。
    /// assert!(AllowedMethods::from_methods(Vec::<String>::new()).is_none());
    /// ```
    #[must_use]
    pub fn from_methods(methods: impl IntoIterator<Item = String>) -> Option<Self> {
        let mut methods: Vec<String> = methods.into_iter().collect();
        if methods.is_empty() {
            return None;
        }
        if methods
            .iter()
            .any(|m| m.is_empty() || !m.bytes().all(crate::request::is_tchar))
        {
            return None;
        }
        methods.sort();
        methods.dedup();
        Some(Self { methods })
    }

    /// `Allow` ヘッダ値として直列化する（`", "` 区切り、ソート済み）。
    ///
    /// ```
    /// use fandhe_backend_http::response::AllowedMethods;
    ///
    /// let allowed = AllowedMethods::from_methods(["POST".to_string(), "GET".to_string()]).unwrap();
    /// assert_eq!(allowed.to_header_value(), "GET, POST");
    /// ```
    #[must_use]
    pub fn to_header_value(&self) -> String {
        self.methods.join(", ")
    }
}

/// [`Response::with_header`] の検証失敗理由（フェイルクローズ、イシュー #301）。
///
/// いずれの variant も `Response` を変更せずに `Err` を返す契約であり、
/// 呼び出し元は検証済みでない値がヘッダとして送出される心配をせずに
/// `?` で伝播できる。`Display` は拒否理由のみを述べ、拒否対象の値そのもの
/// は含めない（ログインジェクション・機密混入防止、`.claude/rules/security.md`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    /// ヘッダ名が空、または RFC 9110 tchar 以外の文字を含む。
    InvalidName,
    /// ヘッダ値が CR / LF / NUL、または HTAB 以外の制御文字を含む。
    InvalidValue,
    /// フレームワークがフレーミングを管理するヘッダ（`Content-Length` /
    /// `Connection` / `Transfer-Encoding`）を上書きしようとした。
    ReservedName,
}

impl std::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::InvalidName => "ヘッダ名が空、または RFC 9110 tchar 以外の文字を含む",
            Self::InvalidValue => "ヘッダ値が CR/LF/NUL、または制御文字を含む",
            Self::ReservedName => {
                "Content-Length / Connection / Transfer-Encoding はフレームワーク管理のため上書きできない"
            }
        };
        f.write_str(reason)
    }
}

impl std::error::Error for HeaderError {}

/// [`Response::redirect`] の構築失敗理由（フェイルクローズ、イシュー #302）。
///
/// `HeaderError` と同様、いずれの variant も `Display` は拒否理由のみを述べ、
/// 拒否対象の値そのもの（Location 文字列）は含めない（ログインジェクション・
/// 機密混入防止、`.claude/rules/security.md`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectError {
    /// 301 / 302 / 303 / 307 / 308 以外のステータスコードを渡した。
    /// リダイレクト非対応ステータスでの `Location` 付与は意味を成さないため、
    /// `AllowedMethods::from_methods` と同じくフェイルクローズで拒否する。
    UnsupportedStatus,
    /// `location` が空文字列。リダイレクト先未指定は意味を成さない。
    EmptyLocation,
    /// `location` が [`Response::with_header`] の検証（CR / LF / NUL・HTAB
    /// 以外の制御文字拒否）に落ちた。レスポンス分割対策をそのまま継承する。
    InvalidLocation(HeaderError),
}

impl std::fmt::Display for RedirectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedStatus => {
                f.write_str("301/302/303/307/308 以外のステータスはリダイレクトに使えない")
            }
            Self::EmptyLocation => f.write_str("Location が空文字列（リダイレクト先未指定）"),
            Self::InvalidLocation(inner) => write!(f, "Location の値が不正: {inner}"),
        }
    }
}

impl std::error::Error for RedirectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidLocation(inner) => Some(inner),
            _ => None,
        }
    }
}

/// 直列化対象の 1 レスポンス。
///
/// `status` は HTTP ステータスコード、`body` はレスポンスボディの生バイト列。
/// ヘッダは `Content-Length`（常時）・`Connection`（`serialize` の
/// `keep_alive` 引数に応じて）・`Allow`（[`Response::with_allow`] 設定時）・
/// [`Response::with_header`] で追加した任意ヘッダのみを自動付与し、
/// それ以外のヘッダを持たない最小構成とする（本モジュールの doc を参照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP ステータスコード。
    pub status: u16,
    /// レスポンスボディの生バイト列。
    pub body: Vec<u8>,
    /// `Content-Type` ヘッダ値。`None` の場合はヘッダ自体を出力しない
    /// （TASK-1.4-2 / #70 時点の既定挙動を保つ）。[`Response::with_content_type`]
    /// の doc を参照。
    content_type: Option<&'static str>,
    /// `Allow` ヘッダ値。`None` の場合はヘッダ自体を出力しない。
    /// [`Response::with_allow`] の doc を参照（TASK-177 / #177）。
    allow: Option<AllowedMethods>,
    /// [`Response::with_header`] で追加した検証済み任意ヘッダ（挿入順）。
    /// 構築経路は `with_header` のみのため、ここに入る値は常に検証済み
    /// （`AllowedMethods` と同一の不変条件パターン、イシュー #301）。
    extra_headers: Vec<(String, String)>,
}

impl Response {
    /// `status` と `body` から [`Response`] を組み立てる。`Content-Type` は
    /// 未設定（ヘッダを出力しない）。
    ///
    /// ```
    /// use fandhe_backend_http::response::Response;
    ///
    /// let res = Response::new(200, b"ok".to_vec());
    /// assert_eq!(res.status, 200);
    /// ```
    #[must_use]
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
            content_type: None,
            allow: None,
            extra_headers: Vec::new(),
        }
    }

    /// body なしの `status` レスポンスを組み立てる。
    ///
    /// ```
    /// use fandhe_backend_http::response::Response;
    ///
    /// let res = Response::empty(404);
    /// assert!(res.body.is_empty());
    /// ```
    #[must_use]
    pub fn empty(status: u16) -> Self {
        Self::new(status, Vec::new())
    }

    /// `Content-Type` ヘッダ値を設定する。
    ///
    /// 値を `&'static str` に限定することで、外部入力（リクエストヘッダ・body
    /// 等）に由来する動的な文字列を渡す経路を型レベルで排除する（本モジュール
    /// 冒頭の doc・`.claude/rules/security.md` のレスポンス分割対策を参照）。
    /// 呼び出し元はソースコード上の文字列リテラルのみを渡せるため、値は常に
    /// このクレート・上位クレートの開発者が静的に書いたものに限られる。
    ///
    /// それでも CRLF を含む値が渡された場合（開発者の誤り）は、レスポンス
    /// 分割を未然に防ぐため `debug_assert!` でパニックさせ、デバッグビルドで
    /// 早期に検知する（リリースビルドでは呼び出し元が `&'static str` リテラル
    /// のみを渡す契約を信頼し、コストのかかる実行時チェックを省く）。
    ///
    /// ```
    /// use fandhe_backend_http::response::Response;
    ///
    /// let res = Response::new(200, b"{}".to_vec()).with_content_type("application/json");
    /// let text = String::from_utf8(res.serialize(true)).unwrap();
    /// assert!(text.contains("Content-Type: application/json\r\n"));
    /// ```
    #[must_use]
    pub fn with_content_type(mut self, content_type: &'static str) -> Self {
        debug_assert!(
            !content_type.contains(['\r', '\n']),
            "Content-Type に CRLF を含む値を渡そうとした（レスポンス分割の危険、呼び出し元の実装ミス）"
        );
        self.content_type = Some(content_type);
        self
    }

    /// `Allow` ヘッダを設定する（405 応答用、TASK-177 / #177）。
    ///
    /// [`AllowedMethods`] は構築時に tchar 検証済みのため、ここで CRLF を
    /// 含む値が渡される経路は型レベルで存在しない（本モジュール冒頭の doc・
    /// `.claude/rules/security.md` のレスポンス分割対策を参照）。呼び出し元
    /// （`crates/routes` の `Router::dispatch`）は 405 応答にのみ使う想定だが、
    /// API 上はステータスコードを問わず設定可能。
    ///
    /// ```
    /// use fandhe_backend_http::response::{AllowedMethods, Response};
    ///
    /// let allowed = AllowedMethods::from_methods(["GET".to_string(), "POST".to_string()]).unwrap();
    /// let res = Response::empty(405).with_allow(allowed);
    /// let text = String::from_utf8(res.serialize(false)).unwrap();
    /// assert!(text.contains("Allow: GET, POST\r\n"));
    /// ```
    #[must_use]
    pub fn with_allow(mut self, allow: AllowedMethods) -> Self {
        self.allow = Some(allow);
        self
    }

    /// 検証付きで任意ヘッダを追加する（イシュー #301）。
    ///
    /// CORS・`Set-Cookie`・`Location`・`Cache-Control` のように名前・値の
    /// 両方が実行時状態に依存するヘッダを送出するための拡張点。
    /// [`Response::with_content_type`] / [`Response::with_allow`] が
    /// カバーしない汎用ケースを埋める（本モジュール冒頭 doc の 3 つ目の
    /// 例外）。
    ///
    /// # 検証（フェイルクローズ）
    ///
    /// - ヘッダ名: 非空かつ RFC 9110 tchar（`request.rs` の `is_tchar` と
    ///   同一基準）のみで構成されること。違反は [`HeaderError::InvalidName`]
    /// - ヘッダ値: CR（`\r`）・LF（`\n`）・NUL（`\0`）を含まないこと。
    ///   加えて HTAB（`\t`）を除く制御文字（0x00–0x1F, 0x7F）も拒否する
    ///   （受け入れ基準の CR/LF/NUL 拒否を包含する保守的な強化）。
    ///   違反は [`HeaderError::InvalidValue`]
    /// - 予約名: `Content-Length` / `Connection` / `Transfer-Encoding`
    ///   は大文字小文字を無視して照合し拒否する（`Content-Length` は body
    ///   長と、`Connection` は `serialize` の `keep_alive` 引数と、
    ///   `Transfer-Encoding` はフレーミングと本クレートが一元管理する
    ///   ため）。違反は [`HeaderError::ReservedName`]
    ///
    /// 検証に失敗した場合は `Response` を変更せず `Err` を返す
    /// （`AllowedMethods::from_methods` と同一のフェイルクローズ設計）。
    ///
    /// # 同名ヘッダの複数回設定
    ///
    /// 上書きではなく**追記**する。挿入順に複数行出力するため、
    /// `Set-Cookie` のように複数値を送出する用途に対応する。
    ///
    /// # 専用フィールドとの優先順位
    ///
    /// [`Response::with_content_type`] / [`Response::with_allow`] で
    /// 専用フィールドが設定済みの場合、同名（大文字小文字無視）の
    /// `with_header` 呼び出しは直列化時にスキップされ、専用フィールドが
    /// 優先される（重複ヘッダ行の出力を防ぐ）。
    ///
    /// # 呼び出し元の責務
    ///
    /// 外部入力（リクエストヘッダ・body 等）に由来する値をそのまま渡す
    /// 場合、ヘッダ数・値長の上限はこの API では設けないため、呼び出し元
    /// でサイズ制限すること（DoS 対策、`.claude/rules/security.md`）。
    ///
    /// ```
    /// use fandhe_backend_http::response::{HeaderError, Response};
    ///
    /// // 正常系。
    /// let res = Response::empty(302).with_header("Location", "/login").unwrap();
    /// let text = String::from_utf8(res.serialize(true)).unwrap();
    /// assert!(text.contains("Location: /login\r\n"));
    ///
    /// // CRLF を含む値は拒否される（レスポンス分割対策）。
    /// let err = Response::empty(200).with_header("X-Test", "v\r\nX-Evil: 1").unwrap_err();
    /// assert_eq!(err, HeaderError::InvalidValue);
    ///
    /// // フレームワーク管理ヘッダの上書きは拒否される。
    /// let err = Response::empty(200).with_header("content-length", "0").unwrap_err();
    /// assert_eq!(err, HeaderError::ReservedName);
    ///
    /// // 専用 API（with_content_type）が設定済みなら with_header 側は出力されない。
    /// let res = Response::new(200, b"{}".to_vec())
    ///     .with_content_type("application/json")
    ///     .with_header("content-type", "text/html")
    ///     .unwrap();
    /// let text = String::from_utf8(res.serialize(true)).unwrap();
    /// assert!(text.contains("Content-Type: application/json\r\n"));
    /// assert!(!text.contains("text/html"));
    /// ```
    pub fn with_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, HeaderError> {
        let name = name.into();
        let value = value.into();

        if name.is_empty() || !name.bytes().all(crate::request::is_tchar) {
            return Err(HeaderError::InvalidName);
        }
        if value.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f) {
            return Err(HeaderError::InvalidValue);
        }
        const RESERVED: [&str; 3] = ["content-length", "connection", "transfer-encoding"];
        if RESERVED.iter().any(|r| name.eq_ignore_ascii_case(r)) {
            return Err(HeaderError::ReservedName);
        }

        self.extra_headers.push((name, value));
        Ok(self)
    }

    /// 検証済みの `Set-Cookie` を追加する（イシュー #303）。
    ///
    /// [`crate::cookie::SetCookie`] は構築時に RFC 6265 の cookie-name /
    /// cookie-value / path-value 文法で検証済みのため、この呼び出しは
    /// **infallible**（`with_allow` と同じ型レベル保証パターン）。
    ///
    /// # 複数 cookie の付与
    ///
    /// 複数回呼び出すことで複数 `Set-Cookie` 行を挿入順に出力する
    /// （`with_header` の追記セマンティクスを利用、受け入れ基準の「同一
    /// レスポンスへの複数 Set-Cookie 付与」に対応）。
    ///
    /// # セキュリティ推奨（`.claude/rules/security.md`）
    ///
    /// セッション ID 等のシークレットを載せる cookie には
    /// [`crate::cookie::SetCookie::http_only`] で `HttpOnly` を付けることを
    /// **強く推奨する**（XSS 経由の cookie 窃取防止）。合わせて
    /// [`crate::cookie::SetCookie::secure`]（平文送出防止）・
    /// [`crate::cookie::SetCookie::same_site`]（CSRF 緩和）の付与も推奨する。
    /// これらは既定 off であり、この API 自体は既定を強制しない
    /// （呼び出し元が cookie の性質に応じて選択する）。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::SetCookie;
    /// use fandhe_backend_http::response::Response;
    ///
    /// let cookie = SetCookie::new("session", "abc123")
    ///     .unwrap()
    ///     .path("/")
    ///     .unwrap()
    ///     .max_age(3600)
    ///     .http_only()
    ///     .secure();
    /// let res = Response::empty(200).with_set_cookie(cookie);
    /// let text = String::from_utf8(res.serialize(true)).unwrap();
    /// assert!(text.contains("Set-Cookie: session=abc123; Path=/; Max-Age=3600; Secure; HttpOnly\r\n"));
    ///
    /// // 複数回の呼び出しで複数 Set-Cookie 行を挿入順に出力する。
    /// let res = Response::empty(200)
    ///     .with_set_cookie(SetCookie::new("a", "1").unwrap())
    ///     .with_set_cookie(SetCookie::new("b", "2").unwrap());
    /// let text = String::from_utf8(res.serialize(true)).unwrap();
    /// let first = text.find("Set-Cookie: a=1\r\n").unwrap();
    /// let second = text.find("Set-Cookie: b=2\r\n").unwrap();
    /// assert!(first < second);
    /// ```
    #[must_use]
    pub fn with_set_cookie(mut self, cookie: crate::cookie::SetCookie) -> Self {
        // `SetCookie::to_header_value` が出力しうる文字集合（tchar な名前 /
        // cookie-octet な値 / path-value なパス / 固定属性リテラル / i64 の
        // 数字表現）は `with_header` の値検証（CR/LF/NUL + 制御文字拒否）が
        // 許可する範囲の真部分集合であるため、ここで検証を再実行しなくても
        // `with_header` が拒否するような行がワイヤに出ることはない。
        // 将来 Domain/Expires 等ゆるい入力を追加する際はこの前提が崩れる点に注意。
        self.extra_headers
            .push(("Set-Cookie".to_string(), cookie.to_header_value()));
        self
    }

    /// 3xx リダイレクト応答を組み立てる（イシュー #302）。
    ///
    /// `status` を 301 / 302 / 303 / 307 / 308 のいずれかに限定し、body なし
    /// の [`Response`] に `Location` ヘッダを設定して返す。それ以外の
    /// ステータスやリダイレクト先未指定・不正な値は `Err` で拒否する
    /// （フェイルクローズ。`AllowedMethods::from_methods` と同一設計方針）。
    ///
    /// `location` の検証は [`Response::with_header`] の検証経路をそのまま
    /// 再利用する（CR / LF / NUL・HTAB 以外の制御文字拒否。本モジュール
    /// 冒頭 doc のレスポンス分割対策を参照）。検証に失敗した場合は
    /// [`RedirectError::InvalidLocation`] で理由を包んで返す。
    ///
    /// # 呼び出し元の責務（オープンリダイレクト対策）
    ///
    /// このメソッドはワイヤフォーマット上の妥当性（CRLF 混入がないか等）
    /// のみを検証し、リダイレクト先の意味的妥当性は判定できない。外部入力
    /// （クエリパラメータ・フォーム値等）に由来する `location` をそのまま
    /// 渡すと、任意サイトへ誘導されるオープンリダイレクト脆弱性
    /// （OWASP Top 10、`.claude/rules/security.md`）につながる。呼び出し元
    /// で許可リスト（相対パスのみ許可する等）による検証を行うこと。
    ///
    /// # 例（POST-Redirect-GET パターン）
    ///
    /// フォーム送信（POST）処理完了後、303 See Other で GET へ誘導する
    /// 典型的な PRG パターン:
    ///
    /// ```
    /// use fandhe_backend_http::response::Response;
    ///
    /// // POST /todos の処理完了後、303 See Other で GET /todos へリダイレクトする。
    /// let res = Response::redirect(303, "/todos").unwrap();
    /// let text = String::from_utf8(res.serialize(true)).unwrap();
    /// assert!(text.starts_with("HTTP/1.1 303 See Other\r\n"));
    /// assert!(text.contains("Location: /todos\r\n"));
    /// assert!(text.ends_with("\r\n\r\n"));
    ///
    /// // CRLF を含む Location は拒否される（レスポンス分割対策）。
    /// assert!(Response::redirect(303, "/x\r\nSet-Cookie: evil=1").is_err());
    ///
    /// // リダイレクト系以外のステータスは拒否される（フェイルクローズ）。
    /// assert!(Response::redirect(200, "/x").is_err());
    /// ```
    pub fn redirect(status: u16, location: impl Into<String>) -> Result<Self, RedirectError> {
        if !matches!(status, 301 | 302 | 303 | 307 | 308) {
            return Err(RedirectError::UnsupportedStatus);
        }
        let location = location.into();
        if location.is_empty() {
            return Err(RedirectError::EmptyLocation);
        }
        Self::empty(status)
            .with_header("Location", location)
            .map_err(RedirectError::InvalidLocation)
    }

    /// HTTP/1.1 ワイヤフォーマットへ直列化する。
    ///
    /// `keep_alive` が `false` の場合のみ `Connection: close` を付与する
    /// （keep-alive が既定の HTTP/1.1 では省略するのが一般的であり、明示が
    /// 必要なのはクローズ時のみという方針。呼び出し元はコアループの
    /// `should_keep_alive` 判定結果をそのまま渡す契約）。
    ///
    /// ステータスに関わらず常に `Content-Length` と body を出力する
    /// （ルーティング未実装の #70 時点では影響しない）。将来 `HEAD` メソッド
    /// 対応（`crates/routes`、TASK-1.5 以降）を追加する際は、`HEAD` 応答で
    /// body を省略しつつ `Content-Length` は `GET` 相当の値を保つ必要がある
    /// ため、本メソッドにメソッド情報を渡すか呼び出し元で body 省略を
    /// 制御する拡張が必要になる点に注意する。
    ///
    /// # ヘッダ出力順（イシュー #301）
    ///
    /// status line → `Content-Type`（[`Response::with_content_type`]）→
    /// `Allow`（[`Response::with_allow`]）→ [`Response::with_header`] で
    /// 追加した任意ヘッダ（挿入順。ただし専用フィールドが設定済みの同名
    /// ヘッダは重複出力を避けるためスキップする）→ `Content-Length` →
    /// `Connection`（必要時）→ 空行 → body。
    ///
    /// ```
    /// use fandhe_backend_http::response::Response;
    ///
    /// let res = Response::new(200, b"hi".to_vec());
    /// let bytes = res.serialize(true);
    /// let text = String::from_utf8(bytes).unwrap();
    /// assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
    /// assert!(text.contains("Content-Length: 2\r\n"));
    /// assert!(text.ends_with("\r\n\r\nhi"));
    /// assert!(!text.contains("Connection: close"));
    /// ```
    ///
    /// ```
    /// use fandhe_backend_http::response::Response;
    ///
    /// let res = Response::empty(400);
    /// let bytes = res.serialize(false);
    /// let text = String::from_utf8(bytes).unwrap();
    /// assert!(text.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    /// assert!(text.contains("Connection: close\r\n"));
    /// ```
    #[must_use]
    pub fn serialize(&self, keep_alive: bool) -> Vec<u8> {
        let reason = reason_phrase(self.status);
        let mut out = Vec::with_capacity(64 + self.body.len());
        out.extend_from_slice(b"HTTP/1.1 ");
        out.extend_from_slice(self.status.to_string().as_bytes());
        out.push(b' ');
        out.extend_from_slice(reason.as_bytes());
        out.extend_from_slice(b"\r\n");
        if let Some(content_type) = self.content_type {
            out.extend_from_slice(b"Content-Type: ");
            out.extend_from_slice(content_type.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        if let Some(allow) = &self.allow {
            out.extend_from_slice(b"Allow: ");
            out.extend_from_slice(allow.to_header_value().as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        for (name, value) in &self.extra_headers {
            // 専用フィールド（Content-Type / Allow）が設定済みの同名ヘッダは
            // 重複出力を避けるためスキップし、専用フィールド側を優先する
            // （`with_header` doc の「専用フィールドとの優先順位」を参照）。
            if self.content_type.is_some() && name.eq_ignore_ascii_case("content-type") {
                continue;
            }
            if self.allow.is_some() && name.eq_ignore_ascii_case("allow") {
                continue;
            }
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"Content-Length: ");
        out.extend_from_slice(self.body.len().to_string().as_bytes());
        out.extend_from_slice(b"\r\n");
        if !keep_alive {
            out.extend_from_slice(b"Connection: close\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

/// 既知ステータスコードの reason phrase を返す固定テーブル。
///
/// 未知のコードは空文字列を返す（`HTTP/1.1 <code> \r\n` のように reason
/// phrase 省略として出力される。RFC 7230 上 reason phrase は省略可能）。
/// テーブルはコアループ（`crates/core/src/server.rs`）・`crates/routes`
/// （`fandhe_backend_routes::Router::dispatch`、TASK-1.5 / #14 でメソッド不一致時に 405 を
/// 払い出す）・`crates/plugin-webrtc-proxy`（TASK-2.1 / #18 の配線経由で
/// 502/504 を払い出す。上流中継失敗時のフォールバックステータス）・
/// `crates/plugin-webrtc`（TASK-8.1 / #26 の `try_handle_rtc_offer` が同時接続数
/// 上限到達時に 503 を払い出す）・[`Response::redirect`]（イシュー #302 の
/// 301/302/303/307/308。PRG パターン等の 3xx リダイレクト用）が実際に払い出す
/// ステータスコードに合わせて選定している。
fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_includes_status_and_reason() {
        let res = Response::empty(200);
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[test]
    fn serialize_unknown_status_has_empty_reason() {
        let res = Response::empty(999);
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.starts_with("HTTP/1.1 999 \r\n"));
    }

    #[test]
    fn serialize_content_length_matches_body() {
        let res = Response::new(200, b"hello".to_vec());
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains("Content-Length: 5\r\n"));
        assert!(text.ends_with("hello"));
    }

    #[test]
    fn serialize_close_adds_connection_close_header() {
        let res = Response::empty(200);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Connection: close\r\n"));
    }

    #[test]
    fn serialize_keep_alive_omits_connection_header() {
        let res = Response::empty(200);
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(!text.contains("Connection:"));
    }

    #[test]
    fn serialize_omits_content_type_by_default() {
        let res = Response::new(200, b"hi".to_vec());
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(!text.contains("Content-Type:"));
    }

    #[test]
    fn serialize_includes_content_type_when_set() {
        let res = Response::new(200, b"{}".to_vec()).with_content_type("application/json");
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains("Content-Type: application/json\r\n"));
    }

    #[test]
    fn serialize_bad_gateway_and_gateway_timeout_have_reason_phrase() {
        // TASK-2.1 / #18: crates/plugin-webrtc-proxy が上流中継失敗時に払い出す
        // 502/504 が空 reason phrase に劣化しないことを確認する（PoC-9 教訓）。
        let bad_gateway = String::from_utf8(Response::empty(502).serialize(false)).unwrap();
        assert!(bad_gateway.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));

        let gateway_timeout = String::from_utf8(Response::empty(504).serialize(false)).unwrap();
        assert!(gateway_timeout.starts_with("HTTP/1.1 504 Gateway Timeout\r\n"));
    }

    #[test]
    fn serialize_service_unavailable_has_reason_phrase() {
        // TASK-8.1 / #26: crates/plugin-webrtc が同時接続数上限到達時に払い出す
        // 503 が空 reason phrase に劣化しないことを確認する（PR #138 Bugbot 指摘）。
        let service_unavailable = String::from_utf8(Response::empty(503).serialize(false)).unwrap();
        assert!(service_unavailable.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
    }

    #[test]
    fn allowed_methods_from_methods_sorts_and_dedups() {
        let allowed = AllowedMethods::from_methods([
            "POST".to_string(),
            "GET".to_string(),
            "POST".to_string(),
        ])
        .unwrap();
        assert_eq!(allowed.to_header_value(), "GET, POST");
    }

    #[test]
    fn allowed_methods_rejects_empty_set() {
        assert!(AllowedMethods::from_methods(Vec::<String>::new()).is_none());
    }

    #[test]
    fn allowed_methods_rejects_empty_string_element() {
        assert!(AllowedMethods::from_methods([String::new()]).is_none());
    }

    #[test]
    fn allowed_methods_rejects_crlf_injection() {
        // レスポンス分割回帰テスト（TASK-177 / #177）: CRLF を含む method を
        // 混入させても構築自体が失敗し、`Allow` ヘッダから絶対に出てこない。
        assert!(AllowedMethods::from_methods(["GET\r\nX-Evil: injected".to_string()]).is_none());
    }

    #[test]
    fn allowed_methods_rejects_space_colon_and_control_chars() {
        assert!(AllowedMethods::from_methods(["GET POST".to_string()]).is_none());
        assert!(AllowedMethods::from_methods(["GET:".to_string()]).is_none());
        assert!(AllowedMethods::from_methods(["GE\u{0}T".to_string()]).is_none());
        assert!(AllowedMethods::from_methods(["caf\u{e9}".to_string()]).is_none());
    }

    #[test]
    fn serialize_includes_allow_header_when_set() {
        let allowed =
            AllowedMethods::from_methods(["POST".to_string(), "GET".to_string()]).unwrap();
        let res = Response::empty(405).with_allow(allowed);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Allow: GET, POST\r\n"));
    }

    #[test]
    fn serialize_omits_allow_header_by_default() {
        let res = Response::empty(405);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(!text.contains("Allow:"));
    }

    #[test]
    fn serialize_ends_headers_with_blank_line_before_body() {
        let res = Response::new(201, b"body".to_vec());
        let bytes = res.serialize(true);
        let text = String::from_utf8(bytes).unwrap();
        let header_body_split = text.split_once("\r\n\r\n").expect("blank line separator");
        assert_eq!(header_body_split.1, "body");
    }

    // --- with_header（イシュー #301） ---

    #[test]
    fn with_header_serializes_the_given_name_and_value() {
        let res = Response::empty(302)
            .with_header("Location", "/login")
            .unwrap();
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains("Location: /login\r\n"));
    }

    #[test]
    fn with_header_appends_repeated_calls_in_insertion_order() {
        // Set-Cookie のような複数値ヘッダは追記（上書きしない）。
        let res = Response::empty(200)
            .with_header("Set-Cookie", "a=1")
            .unwrap()
            .with_header("Set-Cookie", "b=2")
            .unwrap();
        let text = String::from_utf8(res.serialize(true)).unwrap();
        let first = text
            .find("Set-Cookie: a=1\r\n")
            .expect("first cookie present");
        let second = text
            .find("Set-Cookie: b=2\r\n")
            .expect("second cookie present");
        assert!(first < second, "挿入順に出力されること");
    }

    #[test]
    fn with_header_rejects_empty_name() {
        assert_eq!(
            Response::empty(200).with_header("", "v").unwrap_err(),
            HeaderError::InvalidName
        );
    }

    #[test]
    fn with_header_rejects_names_with_space_colon_or_non_ascii() {
        assert_eq!(
            Response::empty(200).with_header("X Test", "v").unwrap_err(),
            HeaderError::InvalidName
        );
        assert_eq!(
            Response::empty(200)
                .with_header("X-Test:", "v")
                .unwrap_err(),
            HeaderError::InvalidName
        );
        assert_eq!(
            Response::empty(200)
                .with_header("X-Caf\u{e9}", "v")
                .unwrap_err(),
            HeaderError::InvalidName
        );
    }

    #[test]
    fn with_header_rejects_crlf_injection_in_value() {
        // レスポンス分割回帰テスト: CRLF を含む値は構築段階で拒否され、
        // 追加ヘッダとしてワイヤに出てくることは絶対にない。
        assert_eq!(
            Response::empty(200)
                .with_header("X-Test", "v\r\nX-Evil: injected")
                .unwrap_err(),
            HeaderError::InvalidValue
        );
        assert_eq!(
            Response::empty(200)
                .with_header("X-Test", "v\ronly-cr")
                .unwrap_err(),
            HeaderError::InvalidValue
        );
        assert_eq!(
            Response::empty(200)
                .with_header("X-Test", "v\nonly-lf")
                .unwrap_err(),
            HeaderError::InvalidValue
        );
    }

    #[test]
    fn with_header_rejects_nul_and_other_control_chars_but_allows_htab() {
        assert_eq!(
            Response::empty(200)
                .with_header("X-Test", "v\u{0}nul")
                .unwrap_err(),
            HeaderError::InvalidValue
        );
        assert_eq!(
            Response::empty(200)
                .with_header("X-Test", "v\u{1}ctrl")
                .unwrap_err(),
            HeaderError::InvalidValue
        );
        // HTAB・SP は制御文字扱いせず許可する。
        assert!(Response::empty(200).with_header("X-Test", "v\tw x").is_ok());
    }

    #[test]
    fn with_header_rejects_reserved_names_case_insensitively() {
        assert_eq!(
            Response::empty(200)
                .with_header("Content-Length", "0")
                .unwrap_err(),
            HeaderError::ReservedName
        );
        assert_eq!(
            Response::empty(200)
                .with_header("content-length", "0")
                .unwrap_err(),
            HeaderError::ReservedName
        );
        assert_eq!(
            Response::empty(200)
                .with_header("Connection", "keep-alive")
                .unwrap_err(),
            HeaderError::ReservedName
        );
        assert_eq!(
            Response::empty(200)
                .with_header("Transfer-Encoding", "chunked")
                .unwrap_err(),
            HeaderError::ReservedName
        );
    }

    #[test]
    fn with_header_yields_to_dedicated_content_type_field() {
        let res = Response::new(200, b"{}".to_vec())
            .with_content_type("application/json")
            .with_header("content-type", "text/html")
            .unwrap();
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains("Content-Type: application/json\r\n"));
        assert!(!text.contains("text/html"));
    }

    #[test]
    fn with_header_yields_to_dedicated_allow_field() {
        let allowed = AllowedMethods::from_methods(["GET".to_string()]).unwrap();
        let res = Response::empty(405)
            .with_allow(allowed)
            .with_header("allow", "POST")
            .unwrap();
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Allow: GET\r\n"));
        assert!(!text.contains("POST"));
    }

    #[test]
    fn with_header_is_used_when_dedicated_field_is_unset() {
        // 専用フィールド未設定時は with_header 側がそのまま出力される。
        let res = Response::empty(200)
            .with_header("Content-Type", "text/plain")
            .unwrap();
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains("Content-Type: text/plain\r\n"));
    }

    #[test]
    fn serialize_without_with_header_is_unchanged_from_baseline() {
        // 後方互換回帰: extra_headers 未設定時の出力は現行仕様と同一
        // （既存フィールドのみのレスポンスが無修正で通ることを兼ねる）。
        let res = Response::new(200, b"hi".to_vec());
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert_eq!(text, "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
    }

    // --- with_set_cookie（イシュー #303） ---

    #[test]
    fn with_set_cookie_serializes_name_and_value() {
        let cookie = crate::cookie::SetCookie::new("session", "abc").unwrap();
        let res = Response::empty(200).with_set_cookie(cookie);
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains("Set-Cookie: session=abc\r\n"));
    }

    #[test]
    fn with_set_cookie_allows_multiple_cookies_in_insertion_order() {
        // 受け入れ基準 2: 同一レスポンスへの複数 Set-Cookie 付与。
        let res = Response::empty(200)
            .with_set_cookie(crate::cookie::SetCookie::new("a", "1").unwrap())
            .with_set_cookie(crate::cookie::SetCookie::new("b", "2").unwrap());
        let text = String::from_utf8(res.serialize(true)).unwrap();
        let first = text
            .find("Set-Cookie: a=1\r\n")
            .expect("first cookie present");
        let second = text
            .find("Set-Cookie: b=2\r\n")
            .expect("second cookie present");
        assert!(first < second, "挿入順に出力されること");
    }

    #[test]
    fn with_set_cookie_serializes_all_attributes_in_fixed_order() {
        let cookie = crate::cookie::SetCookie::new("session", "abc")
            .unwrap()
            .path("/")
            .unwrap()
            .max_age(3600)
            .same_site(crate::cookie::SameSite::Lax)
            .secure()
            .http_only();
        let res = Response::empty(200).with_set_cookie(cookie);
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains(
            "Set-Cookie: session=abc; Path=/; Max-Age=3600; SameSite=Lax; Secure; HttpOnly\r\n"
        ));
    }

    #[test]
    fn with_set_cookie_never_produces_a_value_with_crlf() {
        // レスポンス分割回帰: SetCookie は構築段階で CRLF を含む name/value/path
        // を拒否するため、with_set_cookie を経由してワイヤに CRLF が混入する
        // 経路は存在しない（構築失敗する側を確認する）。
        assert!(crate::cookie::SetCookie::new("session\r\nX-Evil", "v").is_err());
        assert!(crate::cookie::SetCookie::new("session", "v\r\nX-Evil: 1").is_err());
    }

    // --- redirect（イシュー #302） ---

    #[test]
    fn serialize_redirect_statuses_have_reason_phrase() {
        // reason phrase が空に劣化しないことを 5 コードすべてで確認する
        // （PoC-9 教訓、`serialize_bad_gateway_and_gateway_timeout_have_reason_phrase` と同型）。
        let cases = [
            (301, "Moved Permanently"),
            (302, "Found"),
            (303, "See Other"),
            (307, "Temporary Redirect"),
            (308, "Permanent Redirect"),
        ];
        for (status, reason) in cases {
            let text = String::from_utf8(Response::empty(status).serialize(true)).unwrap();
            assert!(
                text.starts_with(&format!("HTTP/1.1 {status} {reason}\r\n")),
                "status {status} の reason phrase が想定と異なる: {text}"
            );
        }
    }

    #[test]
    fn redirect_sets_status_and_location() {
        let res = Response::redirect(303, "/todos").unwrap();
        assert_eq!(res.status, 303);
        assert!(res.body.is_empty());
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.starts_with("HTTP/1.1 303 See Other\r\n"));
        assert!(text.contains("Location: /todos\r\n"));
        assert!(text.contains("Content-Length: 0\r\n"));
    }

    #[test]
    fn redirect_rejects_crlf_in_location() {
        // レスポンス分割回帰テスト: `with_header` の検証経路を再利用しているため
        // CRLF を含む Location は構築段階で拒否される。
        let err = Response::redirect(303, "/x\r\nSet-Cookie: evil=1").unwrap_err();
        assert_eq!(
            err,
            RedirectError::InvalidLocation(HeaderError::InvalidValue)
        );
    }

    #[test]
    fn redirect_rejects_unsupported_status() {
        for status in [200, 404, 300, 304] {
            assert_eq!(
                Response::redirect(status, "/x").unwrap_err(),
                RedirectError::UnsupportedStatus,
                "status {status} は UnsupportedStatus で拒否されるべき"
            );
        }
    }

    #[test]
    fn redirect_rejects_empty_location() {
        assert_eq!(
            Response::redirect(302, "").unwrap_err(),
            RedirectError::EmptyLocation
        );
    }

    #[test]
    fn redirect_accepts_all_supported_statuses() {
        for status in [301, 302, 303, 307, 308] {
            let res = Response::redirect(status, "/ok").unwrap();
            assert_eq!(res.status, status);
        }
    }
}
