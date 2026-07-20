//! `Set-Cookie` ヘッダの構築時検証済みヘルパ（イシュー #303）。
//!
//! [`crate::response::Response::with_header`]（イシュー #301）は CR/LF/NUL・
//! 制御文字のみを拒否する汎用ヘッダ検証であり、RFC 6265 の cookie-name /
//! cookie-value の文法（`;` や `,` を含む値がヘッダ構造を壊す等）までは検証
//! しない。本モジュールは axum の `CookieJar` 相当の最小サブセットとして、
//! [`SetCookie`] という**構築時検証済みの専用型**を提供し、
//! [`crate::response::Response::with_set_cookie`] の唯一の入力とすることで、
//! `with_header` と同一のフェイルクローズ設計思想を cookie 属性に拡張する
//! （`response.rs` モジュール冒頭 doc の「構築時検証済み専用型」パターンの
//! 第 2 号、[`crate::response::AllowedMethods`] と同型）。
//!
//! `Domain` / `Expires` / `Partitioned` 属性、DQUOTE 囲み cookie-value、
//! リクエスト側 `Cookie` ヘッダのパースは最小サブセット方針によりスコープ外
//! （必要になった時点で別イシュー化する、`.claude/rules/out-of-scope-tracking.md`）。

/// [`SetCookie::new`] / [`SetCookie::path`] の構築時検証エラー（フェイルクローズ）。
///
/// [`crate::response::HeaderError`] と同一方針で、`Display` は拒否理由のみを
/// 述べ拒否対象の値そのものは含めない（セッション ID 等の機密がエラー
/// メッセージ経由でログに漏れるのを防ぐ、`.claude/rules/security.md`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieError {
    /// cookie 名が空、または RFC 9110 tchar（RFC 6265 の cookie-name = token
    /// と同一基準）以外の文字を含む。
    InvalidName,
    /// cookie 値が RFC 6265 cookie-octet の範囲外の文字を含む。
    InvalidValue,
    /// `Path` 属性値が RFC 6265 path-value の範囲外の文字を含む。
    InvalidPath,
}

impl std::fmt::Display for CookieError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::InvalidName => "cookie 名が空、または RFC 9110 tchar 以外の文字を含む",
            Self::InvalidValue => "cookie 値が RFC 6265 cookie-octet の範囲外の文字を含む",
            Self::InvalidPath => "Path 属性値が RFC 6265 path-value の範囲外の文字を含む",
        };
        f.write_str(reason)
    }
}

impl std::error::Error for CookieError {}

/// `SameSite` 属性（型付き指定）。
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

/// 構築時検証済みの `Set-Cookie` 1 件（イシュー #303）。
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

/// RFC 6265 cookie-octet: `%x21 / %x23-2B / %x2D-3A / %x3C-5B / %x5D-7E`。
///
/// 印字可能 US-ASCII（`0x21`–`0x7E`）から CTL 域外の 5 バイト
/// （SP `0x20`・`"` `0x22`・`,` `0x2C`・`;` `0x3B`・`\` `0x5C`）を除いた集合。
/// これらの除外文字は `Set-Cookie` の `; ` 区切り構文・値の引用構文と衝突する
/// ため、混入するとヘッダ構造が壊れる（レスポンス分割ではないが cookie 属性の
/// 意図しない注入経路になりうる）。空値（cookie-value は RFC 6265 上省略可）
/// は呼び出し元の `bytes().all(...)` が空イテレータに対し `true` を返すため
/// 別途許可している。
fn is_cookie_octet(b: u8) -> bool {
    matches!(b, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)
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
    ///   cookie-name = token と同一。[`crate::request::is_tchar`] を共有）。
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
    /// 検証する。違反は [`CookieError::InvalidPath`]。
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
    /// ```
    pub fn path(mut self, path: impl Into<String>) -> Result<Self, CookieError> {
        let path = path.into();
        if path.is_empty() || !path.bytes().all(is_path_value) {
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

    // --- 名前検証 ---

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

    // --- 値検証 ---

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

    // --- path 検証 ---

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

    // --- 属性直列化 ---

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
