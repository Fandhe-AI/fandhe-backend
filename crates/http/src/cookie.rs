//! Cookie ヘッダ読み取りパーサ（sans-IO、イシュー #309）。
//!
//! [`crate::request::RequestHead::header`] / [`crate::request::RequestHead::headers`]
//! は生ヘッダ値を返すのみで、`Cookie: k=v; k2=v2` を key-value 組へ分解する
//! 処理は呼び出し元の責務だった（[`crate::query`]・[`crate::form`] と同型の
//! 未整備領域）。本モジュールはその分解処理を RFC 6265 の cookie-pair 構文に
//! 準拠する形で提供し、セッション・認証実装（今後のプラグイン）が個別に
//! 自前 split を実装するのを防ぐ。
//!
//! # 呼び出し契約
//!
//! [`parse_cookie_header`] は単一 `Cookie` ヘッダ値（1 本）を入力に取る
//! sans-IO 純関数。複数 `Cookie` ヘッダが届いた場合の結合は
//! [`crate::request::RequestHead::cookies`] が担う（受け入れ条件 2）。
//!
//! # 構文仕様（RFC 6265 §4.1.1 cookie-pair 準拠）
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
//! # 不正組の扱い（fail-closed。受け入れ条件 1）
//!
//! 構文違反の pair（`=` 欠落・空名・tchar 違反・cookie-octet 違反・空 pair）を
//! 検出した場合は **明示スキップではなく [`CookieError::InvalidCookiePair`] を
//! 返す**。他パーサ（[`crate::query`] 等）と異なり Cookie は認証・セッション
//! 情報を運ぶことが多く、構文違反を暗黙に読み飛ばすと「攻撃者が意図的に
//! 壊した pair の陰に正規 pair を隠す」類の解析不一致（smuggling）を招きうる
//! ため、境界検証の穴を作らない fail-closed を採る（`.claude/rules/security.md`）。
//!
//! # DoS 耐性（`.claude/rules/security.md` リソース枯渇対策）
//!
//! [`crate::query`] と同じ「分解前に上限を検査し fail-closed で拒否する」
//! 方針を踏襲する。[`parse_cookie_header`] は組数・全長の 2 上限を検証し、
//! 超過時は部分結果を返さない。
//!
//! # 非デコード契約
//!
//! % デコードは行わない（[`crate::query`]・
//! [`crate::request::RequestHead::path`] と同じ「無正規化のまま返す」契約）。
//! デコード・信頼判断・同名 Cookie の採否（first-wins 等）・`__Host-`/
//! `__Secure-` プレフィックス検証は呼び出し元の責務とする。

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

/// [`parse_cookie_header`] が返しうるエラー。
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
}

impl std::fmt::Display for CookieError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            CookieError::CookieStringTooLarge => "cookie string exceeds MAX_COOKIE_STRING_BYTES",
            CookieError::TooManyCookies => "cookie pair count exceeds MAX_COOKIE_COUNT",
            CookieError::InvalidCookiePair => "cookie pair violates RFC 6265 cookie-pair syntax",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for CookieError {}

/// cookie-octet（RFC 6265 §4.1.1）判定。
///
/// CTL・SP・DQUOTE（`0x22`）・カンマ（`0x2C`）・セミコロン（`0x3B`）・
/// バックスラッシュ（`0x5C`）を除く可視 ASCII のみを許容する。
fn is_cookie_octet(b: u8) -> bool {
    matches!(b, 0x21 | 0x23..=0x2B | 0x2D..=0x3A | 0x3C..=0x5B | 0x5D..=0x7E)
}

/// 単一の cookie-pair（`name=value` の 1 組）を検証・分解する。
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

/// 単一 `Cookie` ヘッダ値を cookie-pair の列へ分解する sans-IO 純関数。
///
/// `;` 区切りで pair を分割し、各 pair 前後の OWS を trim してから
/// [`parse_cookie_pair`] で検証する。区切りは `"; "`（RFC 6265 §5.4 の
/// サーバ側寛容化）・`";"` 単独のいずれも受理する。
///
/// 上限超過・構文違反時は `Vec` を一切構築せず `Err` を返す（fail-closed。
/// モジュール doc の「不正組の扱い」「DoS 耐性」節を参照）。
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
