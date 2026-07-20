//! Cookie 関連のヘルパ（読み取りパーサ・構築時検証済み書き込み型）。
//!
//! 本モジュールは 2 つの独立した責務を持つ。
//!
//! - **読み取り**（[`parse_cookie_header`]、イシュー #309）: [`crate::request::RequestHead::header`] /
//!   [`crate::request::RequestHead::headers`] は生ヘッダ値を返すのみで、
//!   `Cookie: k=v; k2=v2` を key-value 組へ分解する処理は呼び出し元の責務
//!   だった（[`crate::query`]・[`crate::form`] と同型の未整備領域）。本モジュール
//!   はその分解処理を RFC 6265 の cookie-pair 構文に準拠する形で提供し、
//!   セッション・認証実装（今後のプラグイン）が個別に自前 split を実装するのを防ぐ。
//! - **書き込み**（[`SetCookie`]、イシュー #303）: [`crate::response::Response::with_header`]
//!   は CR/LF/NUL・制御文字のみを拒否する汎用ヘッダ検証であり、RFC 6265 の
//!   cookie-name / cookie-value の文法（`;` や `,` を含む値がヘッダ構造を壊す等）
//!   までは検証しない。[`SetCookie`] は axum の `CookieJar` 相当の最小サブセット
//!   として構築時検証済みの専用型を提供し、
//!   [`crate::response::Response::with_set_cookie`] の唯一の入力とすることで、
//!   `with_header` と同一のフェイルクローズ設計思想を cookie 属性に拡張する
//!   （`response.rs` モジュール冒頭 doc の「構築時検証済み専用型」パターンの
//!   第 2 号、[`crate::response::AllowedMethods`] と同型）。
//!
//! # 読み取り側: 呼び出し契約
//!
//! [`parse_cookie_header`] は単一 `Cookie` ヘッダ値（1 本）を入力に取る
//! sans-IO 純関数。複数 `Cookie` ヘッダが届いた場合の結合は
//! [`crate::request::RequestHead::cookies`] が担う（受け入れ条件 2）。
//!
//! # 読み取り側: 構文仕様（RFC 6265 §4.1.1 cookie-pair 準拠）
//!
//! ```text
//! cookie-pair   = cookie-name "=" cookie-value
//! cookie-name   = token（RFC 9110 tchar のみ、1 文字以上）
//! cookie-value  = *cookie-octet / ( DQUOTE *cookie-octet DQUOTE )
//! cookie-octet  = %x21 / %x23-2B / %x2D-3A / %x3C-5B / %x5D-7E
//!                 （CTL・SP・DQUOTE・カンマ・セミコロン・バックスラッシュを除く）
//! ```
//!
//! - `cookie-name` は空を許容しない（空名は構文違反）
//! - `cookie-value` は空を許容する（`k=` は valid）
//! - DQUOTE で囲んだ値は両端の引用符を除去した内側を返す（除去する契約）
//! - pair 前後の OWS（`; ` 区切りの空白）は trim する
//!
//! # 読み取り側: 不正組の扱い（fail-closed。受け入れ条件 1）
//!
//! 構文違反の pair（`=` 欠落・空名・tchar 違反・cookie-octet 違反・空 pair）を
//! 検出した場合は **明示スキップではなく [`CookieError::InvalidCookiePair`] を
//! 返す**。他パーサ（[`crate::query`] 等）と異なり Cookie は認証・セッション
//! 情報を運ぶことが多く、構文違反を暗黙に読み飛ばすと「攻撃者が意図的に
//! 壊した pair の陰に正規 pair を隠す」類の解析不一致（smuggling）を招きうる
//! ため、境界検証の穴を作らない fail-closed を採る（`.claude/rules/security.md`）。
//!
//! # 読み取り側: DoS 耐性（`.claude/rules/security.md` リソース枯渇対策）
//!
//! [`crate::query`] と同じ「分解前に上限を検査し fail-closed で拒否する」
//! 方針を踏襲する。[`parse_cookie_header`] は組数・全長の 2 上限を検証し、
//! 超過時は部分結果を返さない。
//!
//! # 読み取り側: 非デコード契約
//!
//! % デコードは行わない（[`crate::query`]・
//! [`crate::request::RequestHead::path`] と同じ「無正規化のまま返す」契約）。
//! デコード・信頼判断・同名 Cookie の採否（first-wins 等）・`__Host-`/
//! `__Secure-` プレフィックス検証は呼び出し元の責務とする。
//!
//! # 書き込み側: スコープ外
//!
//! `Domain` / `Expires` / `Partitioned` 属性は最小サブセット方針によりスコープ外
//! （必要になった時点で別イシュー化する、`.claude/rules/out-of-scope-tracking.md`）。

/// 単一 `Cookie` ヘッダから許容する cookie-pair 数の上限。
///
/// [`crate::query::MAX_QUERY_PAIRS`] と同水準。[`crate::request::RequestHead::cookies`]
/// はこの上限を複数 `Cookie` ヘッダに跨る累積値として適用し、ヘッダ分割による
/// 上限迂回を防ぐ。
pub const MAX_COOKIE_COUNT: usize = 100;

/// 単一 `Cookie` ヘッダとして許容する最大バイト数。
///
/// [`crate::request::MAX_HEADER_BYTES`]（16 KiB）の内数として妥当な値。
/// [`crate::request::RequestHead::cookies`] はこの上限を複数 `Cookie` ヘッダに
/// 跨る累積値として適用する。
pub const MAX_COOKIE_STRING_BYTES: usize = 8 * 1024;

/// [`parse_cookie_header`]（読み取り側）・[`SetCookie`]（書き込み側）が
/// 返しうるエラー。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieError {
    /// Cookie 文字列（累積を含む）が [`MAX_COOKIE_STRING_BYTES`] を超過した。
    CookieStringTooLarge,
    /// cookie-pair 数（累積を含む）が [`MAX_COOKIE_COUNT`] を超過した。
    TooManyCookies,
    /// cookie-pair が RFC 6265 構文に違反した（`=` 欠落・空名・tchar 違反・
    /// cookie-octet 違反・空 pair 等）。値そのものは含めない（セッション
    /// トークン等の機密が誤ってエラーメッセージへ漏れるのを防ぐ）。
    InvalidCookiePair,
    /// [`SetCookie::new`] / [`SetCookie::path`]（書き込み側）: cookie 名が空、
    /// または RFC 9110 tchar（RFC 6265 の cookie-name = token と同一基準）
    /// 以外の文字を含む。
    InvalidName,
    /// [`SetCookie::new`]（書き込み側）: cookie 値が RFC 6265 cookie-octet の
    /// 範囲外の文字を含む。
    InvalidValue,
    /// [`SetCookie::path`]（書き込み側）: `Path` 属性値が RFC 6265 path-value
    /// の範囲外の文字を含む、または `/` で始まらない（後者は文法上は
    /// path-value として許容されうるが、RFC 6265 5.2.4 のクッキーパス抽出
    /// アルゴリズムによりユーザーエージェント側でデフォルトパスへ
    /// フォールバックし黙って無視される。呼び出し元が cookie スコープを
    /// 絞ったつもりでも実際には絞られない不整合を招くため、構築時点で
    /// フェイルクローズに拒否する）。
    InvalidPath,
}

impl std::fmt::Display for CookieError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            CookieError::CookieStringTooLarge => "cookie string exceeds MAX_COOKIE_STRING_BYTES",
            CookieError::TooManyCookies => "cookie pair count exceeds MAX_COOKIE_COUNT",
            CookieError::InvalidCookiePair => "cookie pair violates RFC 6265 cookie-pair syntax",
            CookieError::InvalidName => "cookie 名が空、または RFC 9110 tchar 以外の文字を含む",
            CookieError::InvalidValue => "cookie 値が RFC 6265 cookie-octet の範囲外の文字を含む",
            CookieError::InvalidPath => {
                "Path 属性値が RFC 6265 path-value の範囲外の文字を含む、または '/' で始まらない"
            }
        };
        f.write_str(msg)
    }
}

impl std::error::Error for CookieError {}

/// RFC 6265 cookie-octet: `%x21 / %x23-2B / %x2D-3A / %x3C-5B / %x5D-7E`。
///
/// 印字可能 US-ASCII（`0x21`–`0x7E`）から CTL 域外の 5 バイト
/// （SP `0x20`・`"` `0x22`・`,` `0x2C`・`;` `0x3B`・`\` `0x5C`）を除いた集合。
/// これらの除外文字は Cookie ヘッダの `; ` 区切り構文・値の引用構文と衝突する
/// ため、混入するとヘッダ構造が壊れる（レスポンス分割ではないが cookie 属性の
/// 意図しない注入経路になりうる）。空値（cookie-value は RFC 6265 上省略可）
/// は呼び出し元の `bytes().all(...)` が空イテレータに対し `true` を返すため
/// 別途許可している。読み取り側（[`parse_cookie_pair`]）・書き込み側
/// （[`SetCookie::new`]）の両方が共有する。
fn is_cookie_octet(b: u8) -> bool {
    matches!(b, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)
}

/// 単一の cookie-pair（`name=value` の 1 組）を検証・分解する（読み取り側）。
///
/// 前後の OWS は呼び出し元（[`parse_cookie_header`]）が trim 済みの前提。
/// DQUOTE 囲みの値は引用符を除去した内側を返す。
///
/// `pub(crate)`: [`crate::request::RequestHead::cookies`] が複数 `Cookie`
/// ヘッダを跨る累積 DoS 上限をチェックしたうえで、ヘッダごとの `&str` を
/// 所有権コピーなし（zero-copy）で直接検証するために共有する。
pub(crate) fn parse_cookie_pair(pair: &str) -> Result<(&str, &str), CookieError> {
    let (name, raw_value) = pair.split_once('=').ok_or(CookieError::InvalidCookiePair)?;
    if name.is_empty() || !name.bytes().all(crate::request::is_tchar) {
        return Err(CookieError::InvalidCookiePair);
    }
    let value = match raw_value.strip_prefix('"') {
        Some(inner) => inner
            .strip_suffix('"')
            .ok_or(CookieError::InvalidCookiePair)?,
        None => raw_value,
    };
    if !value.bytes().all(is_cookie_octet) {
        return Err(CookieError::InvalidCookiePair);
    }
    Ok((name, value))
}

/// 単一 `Cookie` ヘッダ値を cookie-pair の列へ分解する sans-IO 純関数（読み取り側）。
///
/// `;` 区切りで pair を分割し、各 pair 前後の OWS を trim してから
/// [`parse_cookie_pair`] で検証する。区切りは `"; "`（RFC 6265 §5.4 の
/// サーバ側寛容化）・`";"` 単独のいずれも受理する。
///
/// 上限超過・構文違反時は `Vec` を一切構築せず `Err` を返す（fail-closed。
/// モジュール doc の「読み取り側: 不正組の扱い」「読み取り側: DoS 耐性」節を参照）。
///
/// 戻り値は入力 `value` への借用（ゼロコピー）。
///
/// # Examples
///
/// 複数 pair を `"; "` 区切りで分解する:
///
/// ```
/// use fandhe_backend_http::cookie::parse_cookie_header;
///
/// let pairs = parse_cookie_header("a=1; b=2").unwrap();
/// assert_eq!(pairs, vec![("a", "1"), ("b", "2")]);
/// ```
///
/// DQUOTE で囲んだ値は引用符を除去して返す（内側は SP を含まない cookie-octet
/// のみで構成する必要がある。SP は DQUOTE 内でも cookie-octet ではない）:
///
/// ```
/// use fandhe_backend_http::cookie::parse_cookie_header;
///
/// let pairs = parse_cookie_header(r#"a="hello-world""#).unwrap();
/// assert_eq!(pairs, vec![("a", "hello-world")]);
/// ```
///
/// 空値は許容する（`k=` は valid）:
///
/// ```
/// use fandhe_backend_http::cookie::parse_cookie_header;
///
/// assert_eq!(parse_cookie_header("k=").unwrap(), vec![("k", "")]);
/// ```
///
/// 不正な pair は明示スキップせずエラーを返す（fail-closed）:
///
/// ```
/// use fandhe_backend_http::cookie::{parse_cookie_header, CookieError};
///
/// // `=` を含まない pair は構文違反。
/// assert_eq!(
///     parse_cookie_header("a=1; broken; b=2").unwrap_err(),
///     CookieError::InvalidCookiePair
/// );
/// ```
///
/// 空文字列（`Cookie:` のみ）も 1 pair 以上が必要なため構文違反として拒否する:
///
/// ```
/// use fandhe_backend_http::cookie::{parse_cookie_header, CookieError};
///
/// assert_eq!(
///     parse_cookie_header("").unwrap_err(),
///     CookieError::InvalidCookiePair
/// );
/// ```
pub fn parse_cookie_header(value: &str) -> Result<Vec<(&str, &str)>, CookieError> {
    if value.len() > MAX_COOKIE_STRING_BYTES {
        return Err(CookieError::CookieStringTooLarge);
    }
    let segments: Vec<&str> = value.split(';').map(str::trim).collect();
    if segments.len() > MAX_COOKIE_COUNT {
        return Err(CookieError::TooManyCookies);
    }
    let mut pairs = Vec::with_capacity(segments.len());
    for segment in segments {
        pairs.push(parse_cookie_pair(segment)?);
    }
    Ok(pairs)
}

/// `SameSite` 属性（型付き指定、書き込み側）。
///
/// `None` は cookie 未設定ではなく RFC 6265bis の `SameSite=None`（クロス
/// サイト送出を許可）を表す型 variant であり、`Option<SameSite>` の `None`
/// （属性自体を出力しない）とは別概念であることに注意。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    /// `SameSite=Strict`。同一サイトのナビゲーションでのみ送出する最も厳格な設定。
    Strict,
    /// `SameSite=Lax`（多くのブラウザの既定相当）。トップレベルナビゲーションの
    /// GET では送出するが、大半のクロスサイトリクエストでは送出しない。
    Lax,
    /// `SameSite=None`。クロスサイトでも送出する。[`SetCookie::same_site`] は
    /// この variant 指定時に `Secure` を自動付与する（下記 doc 参照）。
    None,
}

impl SameSite {
    fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "Strict",
            Self::Lax => "Lax",
            Self::None => "None",
        }
    }
}

/// 構築時検証済みの `Set-Cookie` 1 件（書き込み側、イシュー #303）。
///
/// フィールドは非公開で、唯一の構築経路が [`SetCookie::new`] であることにより
/// 「cookie-name は tchar のみ・cookie-value は cookie-octet のみ・Path は
/// path-value のみ」という不変条件を型レベルで保証する（[`crate::response::AllowedMethods`]
/// と同一パターン）。[`crate::response::Response::with_set_cookie`] へ渡すと、
/// 検証済みであることを前提に infallible にヘッダへ追加される。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetCookie {
    name: String,
    value: String,
    http_only: bool,
    secure: bool,
    same_site: Option<SameSite>,
    path: Option<String>,
    max_age: Option<i64>,
}

/// RFC 6265 path-value: CTL（`0x00`–`0x1F`, `0x7F`）と `;`（`0x3B`）を除く
/// `%x20`–`0x7E`。空パスは許可しない（`Path` 属性を設定する以上、値は
/// 非空であるべきという保守的な制約。RFC 6265 自体は空 path-value を規定
/// していないため、フェイルクローズ側に倒す）。
fn is_path_value(b: u8) -> bool {
    b != b';' && (0x20..=0x7e).contains(&b)
}

impl SetCookie {
    /// `name` と `value` から検証済みの [`SetCookie`] を構築する。
    ///
    /// - `name`: 非空かつ全バイトが RFC 9110 tchar（RFC 6265 の
    ///   cookie-name = token と同一。`crate::request::is_tchar` を共有）。
    ///   違反は [`CookieError::InvalidName`]
    /// - `value`: 全バイトが RFC 6265 cookie-octet
    ///   （`is_cookie_octet` 参照）。空値は許可する（RFC 6265 上
    ///   cookie-value は空でありうる）。違反は [`CookieError::InvalidValue`]
    ///
    /// 既定では `HttpOnly` / `Secure` / `SameSite` / `Path` / `Max-Age` の
    /// いずれも設定しない。**セッション ID 等のシークレットを載せる cookie
    /// には [`SetCookie::http_only`] で `HttpOnly` を付けることを強く推奨する**
    /// （XSS 経由の cookie 窃取防止、`.claude/rules/security.md` のシークレット
    /// 管理方針）。合わせて [`SetCookie::secure`]（平文送出防止）・
    /// [`SetCookie::same_site`]（CSRF 緩和）の付与も推奨する。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{CookieError, SetCookie};
    ///
    /// let cookie = SetCookie::new("session", "abc123").unwrap();
    /// assert_eq!(cookie.to_header_value(), "session=abc123");
    ///
    /// // `;` を含む値は cookie-octet 外のため構築段階で拒否される。
    /// assert_eq!(SetCookie::new("session", "a;b").unwrap_err(), CookieError::InvalidValue);
    ///
    /// // CRLF を含む名前も拒否される（tchar 外）。
    /// assert_eq!(
    ///     SetCookie::new("session\r\nX-Evil", "v").unwrap_err(),
    ///     CookieError::InvalidName
    /// );
    /// ```
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Result<Self, CookieError> {
        let name = name.into();
        let value = value.into();

        if name.is_empty() || !name.bytes().all(crate::request::is_tchar) {
            return Err(CookieError::InvalidName);
        }
        if !value.bytes().all(is_cookie_octet) {
            return Err(CookieError::InvalidValue);
        }

        Ok(Self {
            name,
            value,
            http_only: false,
            secure: false,
            same_site: None,
            path: None,
            max_age: None,
        })
    }

    /// `HttpOnly` 属性を付与する（JavaScript からの cookie 読み取りを禁止し、
    /// XSS 経由のセッション窃取を防ぐ。既定 off）。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::SetCookie;
    ///
    /// let cookie = SetCookie::new("session", "abc").unwrap().http_only();
    /// assert_eq!(cookie.to_header_value(), "session=abc; HttpOnly");
    /// ```
    #[must_use]
    pub fn http_only(mut self) -> Self {
        self.http_only = true;
        self
    }

    /// `Secure` 属性を付与する（HTTPS 接続でのみ送出させ平文送出を防ぐ。既定 off）。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::SetCookie;
    ///
    /// let cookie = SetCookie::new("session", "abc").unwrap().secure();
    /// assert_eq!(cookie.to_header_value(), "session=abc; Secure");
    /// ```
    #[must_use]
    pub fn secure(mut self) -> Self {
        self.secure = true;
        self
    }

    /// `SameSite` 属性を付与する（CSRF 緩和。既定は属性自体を出力しない）。
    ///
    /// `SameSite::None` を指定した場合は `Secure` を自動的に有効化する。
    /// RFC 6265bis・主要ブラウザは `SameSite=None` の cookie に `Secure` の
    /// 同時付与を要求する（付与しない cookie はブラウザ側で拒否・無視され
    /// うる）ため、呼び出し元が付け忘れても安全側の組み合わせに倒す決定的
    /// 挙動として実装する。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{SameSite, SetCookie};
    ///
    /// let cookie = SetCookie::new("session", "abc").unwrap().same_site(SameSite::Lax);
    /// assert_eq!(cookie.to_header_value(), "session=abc; SameSite=Lax");
    ///
    /// // SameSite=None は Secure を自動付与する。
    /// let cookie = SetCookie::new("session", "abc").unwrap().same_site(SameSite::None);
    /// assert_eq!(cookie.to_header_value(), "session=abc; SameSite=None; Secure");
    /// ```
    #[must_use]
    pub fn same_site(mut self, same_site: SameSite) -> Self {
        if same_site == SameSite::None {
            self.secure = true;
        }
        self.same_site = Some(same_site);
        self
    }

    /// `Path` 属性を設定する。
    ///
    /// `path` は RFC 6265 path-value（CTL と `;` を除く `%x20`–`%x7E`）で
    /// 検証し、加えて `/` で始まることを要求する。違反は
    /// [`CookieError::InvalidPath`]。
    ///
    /// `/` で始まらない値を文法上は path-value として許容してしまうと、
    /// RFC 6265 5.2.4 のクッキーパス抽出アルゴリズムによりユーザーエージェント
    /// がその `Path` 属性を無視してデフォルトパスへフォールバックする
    /// （ブラウザからは指定した `Path` が効いていないように見える不整合が
    /// 生じる）ため、構築時点でフェイルクローズに拒否する。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{CookieError, SetCookie};
    ///
    /// let cookie = SetCookie::new("session", "abc").unwrap().path("/api").unwrap();
    /// assert_eq!(cookie.to_header_value(), "session=abc; Path=/api");
    ///
    /// // `;` を含む path は構築段階で拒否される。
    /// let err = SetCookie::new("session", "abc").unwrap().path("/a;b").unwrap_err();
    /// assert_eq!(err, CookieError::InvalidPath);
    ///
    /// // `/` で始まらない path はユーザーエージェント側で無視されうるため拒否される。
    /// let err = SetCookie::new("session", "abc").unwrap().path("api").unwrap_err();
    /// assert_eq!(err, CookieError::InvalidPath);
    /// ```
    pub fn path(mut self, path: impl Into<String>) -> Result<Self, CookieError> {
        let path = path.into();
        if path.is_empty() || !path.starts_with('/') || !path.bytes().all(is_path_value) {
            return Err(CookieError::InvalidPath);
        }
        self.path = Some(path);
        Ok(self)
    }

    /// `Max-Age` 属性を秒単位で設定する。
    ///
    /// 負値は cookie の即時削除を指示する慣用手法として許可する（呼び出し元
    /// の意図的な用途）。`i64` を `to_string()` で直列化するため、出力に
    /// 数字と `-` 以外の文字が現れる経路は存在しない（インジェクション経路なし）。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::SetCookie;
    ///
    /// let cookie = SetCookie::new("session", "abc").unwrap().max_age(3600);
    /// assert_eq!(cookie.to_header_value(), "session=abc; Max-Age=3600");
    ///
    /// // 負値は削除用途として許可する。
    /// let cookie = SetCookie::new("session", "abc").unwrap().max_age(-1);
    /// assert_eq!(cookie.to_header_value(), "session=abc; Max-Age=-1");
    /// ```
    #[must_use]
    pub fn max_age(mut self, seconds: i64) -> Self {
        self.max_age = Some(seconds);
        self
    }

    /// `Set-Cookie` ヘッダ値として直列化する。
    ///
    /// `name=value` の後に設定済み属性を固定順（`Path` → `Max-Age` →
    /// `SameSite` → `Secure` → `HttpOnly`）で `"; "` 区切り結合する
    /// （決定的出力・テスト安定性のため、[`crate::response::AllowedMethods::to_header_value`]
    /// と同方針）。未設定の属性は出力しない。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{SameSite, SetCookie};
    ///
    /// let cookie = SetCookie::new("session", "abc")
    ///     .unwrap()
    ///     .path("/")
    ///     .unwrap()
    ///     .max_age(3600)
    ///     .same_site(SameSite::Lax)
    ///     .secure()
    ///     .http_only();
    /// assert_eq!(
    ///     cookie.to_header_value(),
    ///     "session=abc; Path=/; Max-Age=3600; SameSite=Lax; Secure; HttpOnly"
    /// );
    /// ```
    #[must_use]
    pub fn to_header_value(&self) -> String {
        let mut out = format!("{}={}", self.name, self.value);
        if let Some(path) = &self.path {
            out.push_str("; Path=");
            out.push_str(path);
        }
        if let Some(max_age) = self.max_age {
            out.push_str("; Max-Age=");
            out.push_str(&max_age.to_string());
        }
        if let Some(same_site) = self.same_site {
            out.push_str("; SameSite=");
            out.push_str(same_site.as_str());
        }
        if self.secure {
            out.push_str("; Secure");
        }
        if self.http_only {
            out.push_str("; HttpOnly");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_cookie_header（読み取り側） ---

    #[test]
    fn single_pair_is_parsed() {
        assert_eq!(parse_cookie_header("a=1").unwrap(), vec![("a", "1")]);
    }

    #[test]
    fn multiple_pairs_with_standard_separator_are_parsed() {
        assert_eq!(
            parse_cookie_header("a=1; b=2; c=3").unwrap(),
            vec![("a", "1"), ("b", "2"), ("c", "3")]
        );
    }

    #[test]
    fn semicolon_without_following_space_is_accepted() {
        // RFC 6265 §5.4 のサーバ側寛容化: `";"` 単独区切りも受理する。
        assert_eq!(
            parse_cookie_header("a=1;b=2").unwrap(),
            vec![("a", "1"), ("b", "2")]
        );
    }

    #[test]
    fn empty_value_is_valid() {
        assert_eq!(parse_cookie_header("k=").unwrap(), vec![("k", "")]);
    }

    #[test]
    fn quoted_value_has_quotes_stripped() {
        assert_eq!(
            parse_cookie_header(r#"a="hello-world""#).unwrap(),
            vec![("a", "hello-world")]
        );
    }

    #[test]
    fn space_inside_quoted_value_is_rejected() {
        // SP は DQUOTE で囲んでも cookie-octet ではないため構文違反。
        assert_eq!(
            parse_cookie_header(r#"a="hello world""#).unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn empty_quoted_value_is_valid() {
        assert_eq!(parse_cookie_header(r#"a="""#).unwrap(), vec![("a", "")]);
    }

    #[test]
    fn duplicate_names_are_all_returned_in_order() {
        assert_eq!(
            parse_cookie_header("a=1; a=2").unwrap(),
            vec![("a", "1"), ("a", "2")]
        );
    }

    #[test]
    fn percent_encoded_sequences_are_not_decoded() {
        assert_eq!(
            parse_cookie_header("q=a%20b").unwrap(),
            vec![("q", "a%20b")]
        );
    }

    #[test]
    fn missing_equals_is_rejected() {
        assert_eq!(
            parse_cookie_header("noequals").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn empty_name_is_rejected() {
        assert_eq!(
            parse_cookie_header("=v").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn tchar_violation_in_name_is_rejected() {
        assert_eq!(
            parse_cookie_header("a b=1").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn space_in_unquoted_value_is_rejected() {
        assert_eq!(
            parse_cookie_header("a=b c").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn double_quote_in_unquoted_value_is_rejected() {
        assert_eq!(
            parse_cookie_header(r#"a=b"c"#).unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn comma_in_value_is_rejected() {
        assert_eq!(
            parse_cookie_header("a=b,c").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn semicolon_in_value_is_rejected_as_separator_ambiguity() {
        // `;` は cookie-pair 区切りであり cookie-octet からも除外されるため、
        // `a=b;c` は 2 pair（`a=b`・`c`）に分解された上で後者が構文違反となる。
        assert_eq!(
            parse_cookie_header("a=b;c").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn backslash_in_value_is_rejected() {
        assert_eq!(
            parse_cookie_header(r"a=b\c").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn unterminated_quote_is_rejected() {
        assert_eq!(
            parse_cookie_header(r#"a="unterminated"#).unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn empty_pair_from_stray_semicolon_is_rejected() {
        assert_eq!(
            parse_cookie_header("a=1;;b=2").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn empty_string_is_rejected() {
        assert_eq!(
            parse_cookie_header("").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn cookie_string_exactly_at_max_bytes_is_accepted() {
        // "a=" (2 bytes) の cookie-name/value 部を埋めて MAX_COOKIE_STRING_BYTES
        // ちょうどに合わせる。
        let value = "a".repeat(MAX_COOKIE_STRING_BYTES - 2);
        let cookie = format!("a={value}");
        assert_eq!(cookie.len(), MAX_COOKIE_STRING_BYTES);
        assert!(parse_cookie_header(&cookie).is_ok());
    }

    #[test]
    fn cookie_string_exceeding_max_bytes_is_rejected() {
        let value = "a".repeat(MAX_COOKIE_STRING_BYTES - 1);
        let cookie = format!("a={value}");
        assert_eq!(cookie.len(), MAX_COOKIE_STRING_BYTES + 1);
        assert_eq!(
            parse_cookie_header(&cookie).unwrap_err(),
            CookieError::CookieStringTooLarge
        );
    }

    #[test]
    fn pair_count_exactly_at_max_is_accepted() {
        let cookie = (0..MAX_COOKIE_COUNT)
            .map(|i| format!("k{i}=v"))
            .collect::<Vec<_>>()
            .join("; ");
        assert!(parse_cookie_header(&cookie).is_ok());
    }

    #[test]
    fn pair_count_exceeding_max_is_rejected() {
        let cookie = (0..=MAX_COOKIE_COUNT)
            .map(|i| format!("k{i}=v"))
            .collect::<Vec<_>>()
            .join("; ");
        assert_eq!(
            parse_cookie_header(&cookie).unwrap_err(),
            CookieError::TooManyCookies
        );
    }

    #[test]
    fn fail_closed_rejection_yields_no_partial_pairs_on_syntax_error() {
        // 先頭 pair が正常でも、後続 pair が構文違反なら結果全体を捨てる。
        assert!(parse_cookie_header("a=1; broken").is_err());
    }

    #[test]
    fn cookie_error_display_messages_are_stable() {
        assert_eq!(
            CookieError::CookieStringTooLarge.to_string(),
            "cookie string exceeds MAX_COOKIE_STRING_BYTES"
        );
        assert_eq!(
            CookieError::TooManyCookies.to_string(),
            "cookie pair count exceeds MAX_COOKIE_COUNT"
        );
        assert_eq!(
            CookieError::InvalidCookiePair.to_string(),
            "cookie pair violates RFC 6265 cookie-pair syntax"
        );
    }

    #[test]
    fn cookie_error_implements_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<CookieError>();
    }

    // --- SetCookie::new 名前検証（書き込み側） ---

    #[test]
    fn new_rejects_empty_name() {
        assert_eq!(
            SetCookie::new("", "v").unwrap_err(),
            CookieError::InvalidName
        );
    }

    #[test]
    fn new_rejects_name_with_space_semicolon_equals_crlf_non_ascii() {
        assert_eq!(
            SetCookie::new("a b", "v").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::new("a;b", "v").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::new("a=b", "v").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::new("a\r\nb", "v").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::new("caf\u{e9}", "v").unwrap_err(),
            CookieError::InvalidName
        );
    }

    // --- SetCookie::new 値検証（書き込み側） ---

    #[test]
    fn new_allows_empty_value() {
        let cookie = SetCookie::new("session", "").unwrap();
        assert_eq!(cookie.to_header_value(), "session=");
    }

    #[test]
    fn new_rejects_value_with_space_dquote_comma_semicolon_backslash() {
        assert_eq!(
            SetCookie::new("session", "a b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a\"b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a,b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a;b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a\\b").unwrap_err(),
            CookieError::InvalidValue
        );
    }

    #[test]
    fn new_rejects_value_with_crlf_nul_control_and_high_bytes() {
        assert_eq!(
            SetCookie::new("session", "a\r\nb").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a\u{0}b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a\u{1}b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a\u{7f}b").unwrap_err(),
            CookieError::InvalidValue
        );
        // 0x80 以上のバイト（非 ASCII）は cookie-octet 外。
        assert_eq!(
            SetCookie::new("session", "caf\u{e9}").unwrap_err(),
            CookieError::InvalidValue
        );
    }

    #[test]
    fn new_accepts_cookie_octet_boundary_values() {
        // cookie-octet の両端点（0x21 '!' と 0x7e '~'）を受理する。
        assert!(SetCookie::new("session", "!").is_ok());
        assert!(SetCookie::new("session", "~").is_ok());
        // `=` (0x3d) は cookie-value としては許可される（base64 値の想定）。
        assert!(SetCookie::new("session", "a=b").is_ok());
    }

    // --- SetCookie::path 検証（書き込み側） ---

    #[test]
    fn path_rejects_empty_semicolon_crlf_and_control_chars() {
        assert_eq!(
            SetCookie::new("s", "v").unwrap().path("").unwrap_err(),
            CookieError::InvalidPath
        );
        assert_eq!(
            SetCookie::new("s", "v").unwrap().path("/a;b").unwrap_err(),
            CookieError::InvalidPath
        );
        assert_eq!(
            SetCookie::new("s", "v")
                .unwrap()
                .path("/a\r\nb")
                .unwrap_err(),
            CookieError::InvalidPath
        );
        assert_eq!(
            SetCookie::new("s", "v")
                .unwrap()
                .path("/a\u{0}b")
                .unwrap_err(),
            CookieError::InvalidPath
        );
    }

    #[test]
    fn path_accepts_root_and_nested_path() {
        assert!(SetCookie::new("s", "v").unwrap().path("/").is_ok());
        assert!(SetCookie::new("s", "v").unwrap().path("/api/v1").is_ok());
    }

    /// RFC 6265 5.2.4 のクッキーパス抽出アルゴリズムにより、`/` で始まらない
    /// `Path` 属性値はユーザーエージェント側でデフォルトパスへフォールバック
    /// され黙って無視される（Cursor Bugbot 指摘、PR #328）。呼び出し側が
    /// cookie スコープを絞ったつもりでも実際には絞られない不整合を防ぐため、
    /// 構築時点で拒否することを確認する。
    #[test]
    fn path_rejects_values_not_starting_with_slash() {
        assert_eq!(
            SetCookie::new("s", "v").unwrap().path("api").unwrap_err(),
            CookieError::InvalidPath
        );
        assert_eq!(
            SetCookie::new("s", "v")
                .unwrap()
                .path("api/v1")
                .unwrap_err(),
            CookieError::InvalidPath
        );
    }

    // --- SetCookie 属性直列化（書き込み側） ---

    #[test]
    fn to_header_value_omits_unset_attributes() {
        let cookie = SetCookie::new("session", "abc").unwrap();
        assert_eq!(cookie.to_header_value(), "session=abc");
    }

    #[test]
    fn to_header_value_uses_fixed_attribute_order() {
        let cookie = SetCookie::new("session", "abc")
            .unwrap()
            .http_only()
            .secure()
            .same_site(SameSite::Strict)
            .max_age(60)
            .path("/x")
            .unwrap();
        assert_eq!(
            cookie.to_header_value(),
            "session=abc; Path=/x; Max-Age=60; SameSite=Strict; Secure; HttpOnly"
        );
    }

    #[test]
    fn same_site_none_auto_enables_secure() {
        let cookie = SetCookie::new("session", "abc")
            .unwrap()
            .same_site(SameSite::None);
        assert_eq!(
            cookie.to_header_value(),
            "session=abc; SameSite=None; Secure"
        );
    }

    #[test]
    fn max_age_serializes_zero_and_negative_values() {
        let cookie = SetCookie::new("session", "abc").unwrap().max_age(0);
        assert_eq!(cookie.to_header_value(), "session=abc; Max-Age=0");

        let cookie = SetCookie::new("session", "abc").unwrap().max_age(-1);
        assert_eq!(cookie.to_header_value(), "session=abc; Max-Age=-1");
    }
}
