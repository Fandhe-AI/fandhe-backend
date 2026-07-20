//! RFC 3986 percent-encoding の逆変換（opt-in ヘルパ、イシュー #307）。
//!
//! [`crate::request::RequestHead::path`] / [`crate::request::RequestHead::query`]・
//! `fandhe-backend-routes` のパス照合（`Router::dispatch`・`{name}` パラメータ捕捉）は
//! REQ-1 の正規化バイパス防止方針（OWASP A01: 二重デコードや `%2F` によるルート境界の
//! すり抜けを防ぐ）により**生文字列のまま**照合する契約であり、本モジュールはその契約を
//! 一切変えない。ハンドラが照合確定後に明示的に呼ぶ場合のみデコードする opt-in 純関数
//! として提供する（feature ゲートなし・常時利用可。依存追加なしの数十行のためリンカが
//! 未使用時に除去し pay-for-what-you-use 上の追加コストはない）。
//!
//! # 呼び出し規約（ハンドラ側の責務）
//!
//! - **二重デコード禁止**: デコード済みの値を再度 [`decode_str`] / [`decode_bytes`] に
//!   通さない（多重エンコードによるフィルタ回避を防ぐため）。1 値につき 1 回だけ呼ぶ。
//! - **デコード後の再検証はハンドラの責務**: デコード結果には `%00`（NUL）・制御文字・
//!   `../` 等が現れうる。ファイルパス・ログ・下流システムへ渡す前に呼び出し元で
//!   再検証すること（OWASP A03 インジェクション対策）。
//! - **`+` は空白に変換しない**: `application/x-www-form-urlencoded` の意味論
//!   （`+` → 空白）はここでは扱わない。フォームボディの解釈は別ヘルパ（イシュー #308）
//!   の責務。
//!
//! # DoS 境界
//!
//! デコードは 1 パス・再帰なしの `O(n)` で、出力長は常に入力長以下。入力はこのクレート
//! 上流の `MAX_HEADER_BYTES`（[`crate::request`]、16 KiB）で有界なため、本モジュール
//! 自体に追加のサイズ上限は設けない。

use std::fmt;

/// percent-decode の失敗理由。
///
/// 不正シーケンスは置換文字（U+FFFD）へ黙殺せず、必ず `Err` として呼び出し元に
/// 伝える（フェイルクローズ。REQ-1 の入力検証方針、`.claude/rules/security.md`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PercentDecodeError {
    /// `%` の直後に 2 桁の hex digit が続く前に入力が終端した。
    ///
    /// `at` は `%` バイト自身の入力中の位置（0 始まり）。
    TruncatedEscape {
        /// `%` の位置。
        at: usize,
    },
    /// `%` の後続 2 桁のいずれかが hex digit でない。
    ///
    /// `at` は不正な桁バイト自身の入力中の位置（0 始まり）。
    InvalidHexDigit {
        /// 不正な桁の位置。
        at: usize,
    },
    /// デコード後のバイト列が UTF-8 として不正（[`decode_str`] のみが返す）。
    InvalidUtf8,
}

impl fmt::Display for PercentDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PercentDecodeError::TruncatedEscape { at } => {
                write!(
                    f,
                    "percent-encoding escape が入力終端で不足しています（位置 {at}）"
                )
            }
            PercentDecodeError::InvalidHexDigit { at } => {
                write!(
                    f,
                    "percent-encoding escape の桁が hex digit ではありません（位置 {at}）"
                )
            }
            PercentDecodeError::InvalidUtf8 => {
                write!(f, "percent-decode 後のバイト列が UTF-8 として不正です")
            }
        }
    }
}

impl std::error::Error for PercentDecodeError {}

/// hex digit（`0-9` / `A-F` / `a-f`）を 4 bit 値へ変換する。
///
/// RFC 3986 2.1 は大文字・小文字どちらの hex digit も許容するため両方受理する。
fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// percent-decode の第 1 段（バイト列版）。
///
/// `%XX` を 1 バイトへ復元する。UTF-8 検証は行わないため、`%FF` のような
/// バイナリ値・非 UTF-8 な中間結果もそのまま `Ok` で返す。`%` 以外のバイトは
/// 素通しする。`+` は変換しない（モジュール doc comment の呼び出し規約を参照）。
///
/// # Errors
///
/// - [`PercentDecodeError::TruncatedEscape`]: `%` の後続 2 桁が入力終端で不足
/// - [`PercentDecodeError::InvalidHexDigit`]: `%` の後続が hex digit でない
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::percent::decode_bytes;
///
/// assert_eq!(decode_bytes(b"a%20b").unwrap(), b"a b".to_vec());
/// // 非 UTF-8 な単独バイトもバイト列版はそのまま復元できる。
/// assert_eq!(decode_bytes(b"%FF").unwrap(), vec![0xFFu8]);
/// ```
pub fn decode_bytes(input: &[u8]) -> Result<Vec<u8>, PercentDecodeError> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let b = input[i];
        if b == b'%' {
            let hi = *input
                .get(i + 1)
                .ok_or(PercentDecodeError::TruncatedEscape { at: i })?;
            let lo = *input
                .get(i + 2)
                .ok_or(PercentDecodeError::TruncatedEscape { at: i })?;
            let hi_v = hex_value(hi).ok_or(PercentDecodeError::InvalidHexDigit { at: i + 1 })?;
            let lo_v = hex_value(lo).ok_or(PercentDecodeError::InvalidHexDigit { at: i + 2 })?;
            out.push((hi_v << 4) | lo_v);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    Ok(out)
}

/// percent-decode の第 2 段（str 版）。
///
/// [`decode_bytes`] の結果を UTF-8 として厳密検証する（`String::from_utf8`、
/// lossy 変換は使わない）。日本語等マルチバイト値を URL に載せた場合の復元に使う。
///
/// # Errors
///
/// [`decode_bytes`] のエラーに加え、デコード後のバイト列が UTF-8 として不正な
/// 場合は [`PercentDecodeError::InvalidUtf8`] を返す。
///
/// # Examples
///
/// 日本語値の往復例（受け入れ条件）:
///
/// ```
/// use fandhe_backend_http::percent::decode_str;
///
/// assert_eq!(decode_str("%E6%97%A5%E6%9C%AC%E8%AA%9E").unwrap(), "日本語");
/// ```
///
/// 不正シーケンスはフェイルクローズでエラーになる:
///
/// ```
/// use fandhe_backend_http::percent::{decode_str, PercentDecodeError};
///
/// assert_eq!(decode_str("%A"), Err(PercentDecodeError::TruncatedEscape { at: 0 }));
/// ```
pub fn decode_str(input: &str) -> Result<String, PercentDecodeError> {
    let bytes = decode_bytes(input.as_bytes())?;
    String::from_utf8(bytes).map_err(|_| PercentDecodeError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_bytes_passes_through_plain_ascii() {
        assert_eq!(decode_bytes(b"hello").unwrap(), b"hello".to_vec());
    }

    #[test]
    fn decode_bytes_empty_input() {
        assert_eq!(decode_bytes(b"").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn decode_bytes_decodes_ascii_escape() {
        assert_eq!(decode_bytes(b"a%20b").unwrap(), b"a b".to_vec());
    }

    #[test]
    fn decode_bytes_accepts_lowercase_hex() {
        assert_eq!(decode_bytes(b"%2f").unwrap(), b"/".to_vec());
    }

    #[test]
    fn decode_bytes_accepts_uppercase_hex() {
        assert_eq!(decode_bytes(b"%2F").unwrap(), b"/".to_vec());
    }

    #[test]
    fn decode_bytes_handles_multibyte_sequence() {
        // "日" = E6 97 A5
        assert_eq!(decode_bytes(b"%E6%97%A5").unwrap(), vec![0xE6, 0x97, 0xA5]);
    }

    #[test]
    fn decode_bytes_does_not_touch_plus() {
        // application/x-www-form-urlencoded の `+` → 空白変換は本ヘルパの責務外（#308）。
        assert_eq!(decode_bytes(b"a+b").unwrap(), b"a+b".to_vec());
    }

    #[test]
    fn decode_bytes_rejects_trailing_percent() {
        assert_eq!(
            decode_bytes(b"ab%"),
            Err(PercentDecodeError::TruncatedEscape { at: 2 })
        );
    }

    #[test]
    fn decode_bytes_rejects_truncated_escape_with_one_digit() {
        assert_eq!(
            decode_bytes(b"ab%A"),
            Err(PercentDecodeError::TruncatedEscape { at: 2 })
        );
    }

    #[test]
    fn decode_bytes_rejects_invalid_hex_high_digit() {
        assert_eq!(
            decode_bytes(b"%G0"),
            Err(PercentDecodeError::InvalidHexDigit { at: 1 })
        );
    }

    #[test]
    fn decode_bytes_rejects_invalid_hex_low_digit() {
        assert_eq!(
            decode_bytes(b"%0Z"),
            Err(PercentDecodeError::InvalidHexDigit { at: 2 })
        );
    }

    #[test]
    fn decode_bytes_allows_non_utf8_byte() {
        // バイト列版は UTF-8 検証をしないため %FF 単独でも Ok になる（decode_str との差）。
        assert_eq!(decode_bytes(b"%FF").unwrap(), vec![0xFFu8]);
    }

    #[test]
    fn decode_str_round_trips_japanese() {
        assert_eq!(decode_str("%E6%97%A5%E6%9C%AC%E8%AA%9E").unwrap(), "日本語");
    }

    #[test]
    fn decode_str_rejects_invalid_utf8() {
        // decode_bytes(b"%FF") は Ok だが、decode_str は UTF-8 検証で Err になる。
        assert_eq!(decode_str("%FF"), Err(PercentDecodeError::InvalidUtf8));
    }

    #[test]
    fn decode_str_propagates_hex_error() {
        assert_eq!(
            decode_str("%ZZ"),
            Err(PercentDecodeError::InvalidHexDigit { at: 1 })
        );
    }

    #[test]
    fn error_display_messages_are_non_empty() {
        assert!(
            !PercentDecodeError::TruncatedEscape { at: 0 }
                .to_string()
                .is_empty()
        );
        assert!(
            !PercentDecodeError::InvalidHexDigit { at: 0 }
                .to_string()
                .is_empty()
        );
        assert!(!PercentDecodeError::InvalidUtf8.to_string().is_empty());
    }
}
