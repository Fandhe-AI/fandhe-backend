//! `application/x-www-form-urlencoded` ボディパーサ（sans-IO、イシュー #308）。
//!
//! [`crate::routes::RouteHandler`]（`Box<dyn Fn(&RequestHead, &[u8]) -> Response>`）は
//! ボディを `&[u8]` のまま渡す契約であり、フォーム POST（`k=v&k2=v2`）の解釈は
//! ハンドラ側の責務として残されていた。本モジュールはその分解処理を
//! `crates/http` クレート内へ集約し、[`crate::query`]（`&`/`=` 分解、イシュー #306）・
//! [`crate::percent`]（percent-decode、イシュー #307）の既存 sans-IO 純関数を
//! 合成して提供する。新規依存は追加しない（pay-for-what-you-use）。
//!
//! # 呼び出し契約
//!
//! [`parse_form`] はハンドラが Content-Type を [`is_form_content_type`] で
//! 確認した**後**にのみ、生の body `&[u8]` を渡して呼ぶ想定。
//! Content-Type を確認せずに任意ボディを渡すとパーサ混乱（意図しない
//! media type を form として誤解釈するリスク）につながるため避けること。
//!
//! ```text
//! head.header("content-type").is_some_and(form::is_form_content_type)
//!     .then(|| form::parse_form(body))
//! ```
//!
//! # form-urlencoded 固有のデコード仕様
//!
//! [`crate::query::parse_query`]・[`crate::percent`] は `+` を変換しない
//! 非デコード契約だが、`application/x-www-form-urlencoded`（WHATWG URL 仕様）
//! は `+` を半角スペースへ変換する固有仕様を持つ。[`parse_form`] は
//! **`+` → 半角スペース置換を percent-decode より先に適用する**。この順序が
//! 逆だと `%2B`（percent-encode された literal `+`）が誤って空白化される
//! （`%2B` → `+` → decode で誤って空白になる）ため、実装上の不変条件として
//! 固定する。
//!
//! # 二重デコード禁止・デコード後の再検証（[`crate::percent`] と同一契約）
//!
//! [`parse_form`] は key/value それぞれに 1 回だけデコードを適用する。返した
//! `Vec<(String, String)>` の値を再度デコードに通さないこと（多重エンコードに
//! よるフィルタ回避を防ぐ、OWASP A03）。デコード結果には `%00`・制御文字等が
//! 現れうるため、ファイルパス・ログ・下流システムへ渡す前の再検証は呼び出し元
//! （ハンドラ）の責務とする。
//!
//! # DoS 耐性（`.claude/rules/security.md` リソース枯渇対策）
//!
//! [`chunked`](crate::chunked)・[`query`](crate::query) と同じ「バッファ確保前に
//! 上限を検査し fail-closed で拒否する」方針を踏襲する。[`MAX_FORM_BYTES`]・
//! [`MAX_FORM_PAIRS`] は [`crate::query`] の対応する上限と同値で固定し、
//! `parse_query` への委譲で上限判定が食い違わないようにする（値を分離する
//! 場合は別イシューとして起票する）。上限超過時は部分結果を一切返さない。

use crate::percent::{PercentDecodeError, decode_str};
use crate::query::{MAX_QUERY_BYTES, MAX_QUERY_PAIRS, QueryError, parse_query};

/// フォームボディとして許容する最大バイト数。
///
/// [`crate::query::MAX_QUERY_BYTES`] と同値（本モジュール doc comment 参照）。
pub const MAX_FORM_BYTES: usize = MAX_QUERY_BYTES;

/// フォームボディとして許容する key-value 組数の上限。
///
/// [`crate::query::MAX_QUERY_PAIRS`] と同値（本モジュール doc comment 参照）。
pub const MAX_FORM_PAIRS: usize = MAX_QUERY_PAIRS;

/// [`parse_form`] が返しうるエラー。
///
/// `Display` 実装はボディ内容・キー値を含めない（位置情報のみ。ログへの
/// 機密混入防止、`.claude/rules/security.md`）。
#[derive(Debug, PartialEq, Eq)]
pub enum FormError {
    /// フォームボディ全体が [`MAX_FORM_BYTES`] を超過した。
    BodyTooLong,
    /// key-value 組数が [`MAX_FORM_PAIRS`] を超過した。
    TooManyPairs,
    /// 生ボディが UTF-8 として不正（percent-decode 前の検査）。
    InvalidUtf8Body,
    /// key または value の percent-decode に失敗した。
    Decode(PercentDecodeError),
}

impl std::fmt::Display for FormError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormError::BodyTooLong => f.write_str("form body exceeds MAX_FORM_BYTES"),
            FormError::TooManyPairs => f.write_str("form pair count exceeds MAX_FORM_PAIRS"),
            FormError::InvalidUtf8Body => f.write_str("form body is not valid UTF-8"),
            FormError::Decode(err) => write!(f, "form field percent-decode failed: {err}"),
        }
    }
}

impl std::error::Error for FormError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FormError::Decode(err) => Some(err),
            _ => None,
        }
    }
}

impl From<QueryError> for FormError {
    fn from(err: QueryError) -> Self {
        match err {
            QueryError::QueryTooLong => FormError::BodyTooLong,
            QueryError::TooManyPairs => FormError::TooManyPairs,
        }
    }
}

/// `+` → 半角スペース置換ののち percent-decode を適用する（モジュール doc
/// comment の「form-urlencoded 固有のデコード仕様」節の順序を実装する内部
/// ヘルパ）。
fn decode_form_component(component: &str) -> Result<String, PercentDecodeError> {
    // `+` は percent-encode されないため単純な文字置換で安全に処理できる
    // （`replace` は `%2B` のような既存の percent-encode 列には触れない）。
    let space_normalized = component.replace('+', " ");
    decode_str(&space_normalized)
}

/// `application/x-www-form-urlencoded` ボディを key-value 組へ分解する
/// sans-IO 純関数。
///
/// 処理順（WHATWG URL `application/x-www-form-urlencoded` パーサ準拠）:
///
/// 1. `body.len() > `[`MAX_FORM_BYTES`]` → `Err(BodyTooLong)`（バッファ確保前に検査）
/// 2. `body` を UTF-8 として検証（失敗時 `Err(InvalidUtf8Body)`）
/// 3. [`crate::query::parse_query`] で `&`/`=` 分解（組数上限も同関数が検査）
/// 4. 各 key/value に `+` → 空白 → percent-decode を適用
///
/// 出現順の `Vec<(String, String)>` を返す。重複キーは保持し、除重・上書きは
/// 呼び出し側の責務（[`crate::query::parse_query`] と同一セマンティクス）。
///
/// # Errors
///
/// 上限超過・不正な UTF-8・不正な percent-encoding のいずれかで `Err` を返し、
/// 部分結果は一切生成しない（fail-closed、モジュール doc comment参照）。
///
/// # Examples
///
/// todo 追加フォームの解析例（受け入れ条件 4）:
///
/// ```
/// use fandhe_backend_http::form::{is_form_content_type, parse_form};
///
/// assert!(is_form_content_type("application/x-www-form-urlencoded; charset=UTF-8"));
/// let pairs = parse_form(b"title=Buy+milk&done=false").unwrap();
/// assert_eq!(
///     pairs,
///     vec![
///         ("title".to_string(), "Buy milk".to_string()),
///         ("done".to_string(), "false".to_string()),
///     ]
/// );
/// ```
///
/// percent-decode された日本語値の復元例:
///
/// ```
/// use fandhe_backend_http::form::parse_form;
///
/// let pairs = parse_form("title=%E7%89%9B%E4%B9%B3".as_bytes()).unwrap();
/// assert_eq!(pairs, vec![("title".to_string(), "牛乳".to_string())]);
/// ```
///
/// 上限超過は fail-closed で `Err` を返す:
///
/// ```
/// use fandhe_backend_http::form::{parse_form, FormError, MAX_FORM_BYTES};
///
/// let body = "a".repeat(MAX_FORM_BYTES + 1);
/// assert_eq!(parse_form(body.as_bytes()).unwrap_err(), FormError::BodyTooLong);
/// ```
pub fn parse_form(body: &[u8]) -> Result<Vec<(String, String)>, FormError> {
    if body.len() > MAX_FORM_BYTES {
        return Err(FormError::BodyTooLong);
    }
    let body_str = std::str::from_utf8(body).map_err(|_| FormError::InvalidUtf8Body)?;
    let pairs = parse_query(body_str)?;

    let mut out = Vec::new();
    for (key, value) in pairs {
        let decoded_key = decode_form_component(key).map_err(FormError::Decode)?;
        let decoded_value = decode_form_component(value).map_err(FormError::Decode)?;
        out.push((decoded_key, decoded_value));
    }
    Ok(out)
}

/// `Content-Type` ヘッダ値が `application/x-www-form-urlencoded`（パラメータ
/// 付き可）かどうかを判定する。
///
/// media-type 部分（最初の `;` より前）を前後 OWS（optional whitespace）
/// トリムのうえ ASCII 大文字小文字非区別で厳密一致比較する（RFC 9110:
/// media type は case-insensitive）。前置一致（`application/
/// x-www-form-urlencoded-extra` 等）や別 media type（`multipart/form-data`
/// 等）はパーサ混乱防止のため `false` を返す。
///
/// 呼び出し側は次の形で使う想定:
///
/// ```text
/// head.header("content-type").is_some_and(is_form_content_type)
/// ```
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::form::is_form_content_type;
///
/// assert!(is_form_content_type("application/x-www-form-urlencoded"));
/// assert!(is_form_content_type(
///     "application/x-www-form-urlencoded; charset=UTF-8"
/// ));
/// assert!(is_form_content_type(
///     "  APPLICATION/X-WWW-FORM-URLENCODED  ; charset=UTF-8"
/// ));
/// assert!(!is_form_content_type("multipart/form-data"));
/// assert!(!is_form_content_type(
///     "application/x-www-form-urlencoded-extra"
/// ));
/// assert!(!is_form_content_type(""));
/// ```
pub fn is_form_content_type(content_type: &str) -> bool {
    let media_type = content_type.split(';').next().unwrap_or("").trim();
    media_type.eq_ignore_ascii_case("application/x-www-form-urlencoded")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plus_is_decoded_to_space() {
        assert_eq!(
            parse_form(b"title=Buy+milk").unwrap(),
            vec![("title".to_string(), "Buy milk".to_string())]
        );
    }

    #[test]
    fn percent_encoded_plus_survives_as_literal_plus() {
        // `%2B` → decode 前に `+` へ置換されてはならない（順序の固定）。
        assert_eq!(
            parse_form(b"expr=1%2B2").unwrap(),
            vec![("expr".to_string(), "1+2".to_string())]
        );
    }

    #[test]
    fn percent_decoded_japanese_value_round_trips() {
        assert_eq!(
            parse_form("k=%E6%97%A5%E6%9C%AC%E8%AA%9E".as_bytes()).unwrap(),
            vec![("k".to_string(), "日本語".to_string())]
        );
    }

    #[test]
    fn duplicate_keys_are_all_returned_in_order() {
        assert_eq!(
            parse_form(b"a=1&a=2").unwrap(),
            vec![
                ("a".to_string(), "1".to_string()),
                ("a".to_string(), "2".to_string())
            ]
        );
    }

    #[test]
    fn empty_segments_are_skipped() {
        assert_eq!(
            parse_form(b"a=1&&b=2").unwrap(),
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string())
            ]
        );
    }

    #[test]
    fn key_without_equals_has_empty_value() {
        assert_eq!(
            parse_form(b"a").unwrap(),
            vec![("a".to_string(), "".to_string())]
        );
    }

    #[test]
    fn empty_key_and_empty_value_are_preserved() {
        assert_eq!(
            parse_form(b"=v&b=").unwrap(),
            vec![
                ("".to_string(), "v".to_string()),
                ("b".to_string(), "".to_string())
            ]
        );
    }

    #[test]
    fn empty_body_yields_no_pairs() {
        assert_eq!(parse_form(b"").unwrap(), Vec::<(String, String)>::new());
    }

    #[test]
    fn body_exactly_at_max_bytes_is_accepted() {
        let body = "a".repeat(MAX_FORM_BYTES);
        assert!(parse_form(body.as_bytes()).is_ok());
    }

    #[test]
    fn body_exceeding_max_bytes_is_rejected() {
        let body = "a".repeat(MAX_FORM_BYTES + 1);
        assert_eq!(
            parse_form(body.as_bytes()).unwrap_err(),
            FormError::BodyTooLong
        );
    }

    #[test]
    fn pair_count_exactly_at_max_is_accepted() {
        let body = vec!["a=1"; MAX_FORM_PAIRS].join("&");
        assert!(parse_form(body.as_bytes()).is_ok());
    }

    #[test]
    fn pair_count_exceeding_max_is_rejected() {
        let body = vec!["a=1"; MAX_FORM_PAIRS + 1].join("&");
        assert_eq!(
            parse_form(body.as_bytes()).unwrap_err(),
            FormError::TooManyPairs
        );
    }

    #[test]
    fn fail_closed_rejection_yields_no_partial_pairs() {
        let body = vec!["a=1"; MAX_FORM_PAIRS + 1].join("&");
        assert!(parse_form(body.as_bytes()).is_err());
    }

    #[test]
    fn non_utf8_body_is_rejected() {
        let body: &[u8] = &[b'a', b'=', 0xFF, 0xFE];
        assert_eq!(parse_form(body).unwrap_err(), FormError::InvalidUtf8Body);
    }

    #[test]
    fn truncated_percent_escape_is_rejected() {
        // `at` は value 部分文字列（`%2`）内での位置。`%` が先頭のため 0。
        assert_eq!(
            parse_form(b"a=%2").unwrap_err(),
            FormError::Decode(PercentDecodeError::TruncatedEscape { at: 0 })
        );
    }

    #[test]
    fn invalid_hex_digit_is_rejected() {
        // `at` は value 部分文字列（`%ZZ`）内での位置。不正な桁は index 1。
        assert_eq!(
            parse_form(b"a=%ZZ").unwrap_err(),
            FormError::Decode(PercentDecodeError::InvalidHexDigit { at: 1 })
        );
    }

    #[test]
    fn invalid_utf8_after_decode_is_rejected() {
        // %FF は単独では有効な UTF-8 バイト列にならない。
        assert_eq!(
            parse_form(b"a=%FF").unwrap_err(),
            FormError::Decode(PercentDecodeError::InvalidUtf8)
        );
    }

    #[test]
    fn form_error_display_messages_are_stable() {
        assert_eq!(
            FormError::BodyTooLong.to_string(),
            "form body exceeds MAX_FORM_BYTES"
        );
        assert_eq!(
            FormError::TooManyPairs.to_string(),
            "form pair count exceeds MAX_FORM_PAIRS"
        );
        assert_eq!(
            FormError::InvalidUtf8Body.to_string(),
            "form body is not valid UTF-8"
        );
    }

    #[test]
    fn form_error_implements_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<FormError>();
    }

    #[test]
    fn is_form_content_type_matches_exact() {
        assert!(is_form_content_type("application/x-www-form-urlencoded"));
    }

    #[test]
    fn is_form_content_type_is_case_insensitive() {
        assert!(is_form_content_type("APPLICATION/X-WWW-FORM-URLENCODED"));
    }

    #[test]
    fn is_form_content_type_accepts_parameters() {
        assert!(is_form_content_type(
            "application/x-www-form-urlencoded; charset=UTF-8"
        ));
    }

    #[test]
    fn is_form_content_type_trims_ows() {
        assert!(is_form_content_type(
            "  application/x-www-form-urlencoded  ; charset=UTF-8"
        ));
    }

    #[test]
    fn is_form_content_type_rejects_multipart() {
        assert!(!is_form_content_type("multipart/form-data"));
    }

    #[test]
    fn is_form_content_type_rejects_prefix_trap() {
        assert!(!is_form_content_type(
            "application/x-www-form-urlencoded-extra"
        ));
    }

    #[test]
    fn is_form_content_type_rejects_empty_string() {
        assert!(!is_form_content_type(""));
    }
}
