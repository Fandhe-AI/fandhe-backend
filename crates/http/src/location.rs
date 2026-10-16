//! 同一オリジンに限るリダイレクト先（`Location`）の構築時検証済み型（イシュー #828）。
//!
//! [`crate::response::Response::redirect`] はワイヤ上の妥当性しか検証せず、外部入力由来の
//! 値を渡すとオープンリダイレクト（OWASP A01）になりうる。本モジュールは
//! **同一オリジンの絶対パス（`/` 始まり）だけ**を型として表現可能にし、
//! [`crate::response::Response::see_other`] が受け取る唯一の入力とする。
//!
//! # 受理条件（すべて満たすもののみ）
//!
//! - 空でない
//! - 先頭文字が `/`（絶対 URL・`javascript:`・`evil.com/x`・`x/y` はここで拒否）
//! - 2 文字目が `/` でも `\` でもない（scheme-relative `//host`・`/\host` を拒否）
//! - 全体に `\` を含まない（ブラウザは `\` を `/` と同一視するため）
//! - 全体に制御文字（[`char::is_control`]：C0・DEL・C1）を含まない。HTAB も拒否する。
//!   WHATWG URL パーサは ASCII タブ・改行を除去するため `/<TAB>/evil.com` が
//!   `//evil.com` と解釈されうるため。スペースと制御文字以外の非 ASCII は受理する。
//!
//! # 非デコード契約
//!
//! 型は percent デコード・`.`/`..` 解決・大文字小文字正規化を **一切行わない**。
//! `/%2F%2Fevil.com` や `/%5Cevil.com` は不透明なバイト列としてそのまま受理され、
//! 送出値も検証値とバイト同一である（ブラウザは `%2F` をパス区切りへ復号しないため
//! 同一オリジン内パスに留まる）。外部入力をデコードする場合は **デコード後の最終値**
//! を本型へ通すこと。本型の値を後段でデコードして再利用してはならない（二重デコード禁止）。
//!
//! 検証は sans-IO 純関数 [`validate_same_origin_location`]（割り当てなし・O(n)・panic なし）
//! として公開し、fuzz target `same_origin_location` の対象とする。

use std::fmt;

/// [`SameOriginLocation`] の構築失敗理由（フェイルクローズ）。
///
/// `Display` は拒否理由のみを出し、入力値は含めない（ログ汚染・機密混入防止）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SameOriginLocationError {
    /// 空文字列。
    Empty,
    /// 先頭が `/` でない（絶対 URL・相対参照・`\` 始まり等）。
    NotRootRelative,
    /// 2 文字目が `/` または `\`（scheme-relative URL）。
    SchemeRelative,
    /// `\` を含む。
    Backslash,
    /// 制御文字（HTAB 含む）を含む。
    ControlCharacter,
}

impl fmt::Display for SameOriginLocationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "redirect location must not be empty",
            Self::NotRootRelative => "redirect location must start with '/'",
            Self::SchemeRelative => "redirect location must not be scheme-relative",
            Self::Backslash => "redirect location must not contain a backslash",
            Self::ControlCharacter => "redirect location must not contain control characters",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for SameOriginLocationError {}

/// 同一オリジン相対パスとして安全かを判定する sans-IO 純関数。
///
/// 受理条件・非デコード契約は本モジュールの doc を参照。判定順は
/// 空 → 先頭 `/` → scheme-relative → 全体走査（`\`・制御文字の先に見つかった方）。
///
/// ```
/// use fandhe_backend_http::location::{validate_same_origin_location, SameOriginLocationError};
///
/// assert!(validate_same_origin_location("/todos?x=1").is_ok());
/// assert_eq!(
///     validate_same_origin_location("//evil.com"),
///     Err(SameOriginLocationError::SchemeRelative)
/// );
/// ```
pub fn validate_same_origin_location(value: &str) -> Result<(), SameOriginLocationError> {
    let mut chars = value.chars();
    match chars.next() {
        None => return Err(SameOriginLocationError::Empty),
        Some('/') => {}
        Some(_) => return Err(SameOriginLocationError::NotRootRelative),
    }
    if matches!(chars.clone().next(), Some('/' | '\\')) {
        return Err(SameOriginLocationError::SchemeRelative);
    }
    for c in chars {
        if c == '\\' {
            return Err(SameOriginLocationError::Backslash);
        }
        if c.is_control() {
            return Err(SameOriginLocationError::ControlCharacter);
        }
    }
    Ok(())
}

/// 構築時検証済みの同一オリジン相対パス（`Location` 値）。
///
/// 唯一の構築経路は [`SameOriginLocation::new`]（または `TryFrom`）で、
/// [`validate_same_origin_location`] を通過した値のみ保持する。値は入力と
/// バイト同一で、デコード・正規化は行わない（モジュール doc 参照）。
/// [`crate::response::Response::see_other`] から使われる。
///
/// ```
/// use fandhe_backend_http::location::SameOriginLocation;
///
/// let loc = SameOriginLocation::new("/%2F%2Fevil.com").unwrap();
/// assert_eq!(loc.as_str(), "/%2F%2Fevil.com");
/// assert!(SameOriginLocation::new("https://evil.com").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SameOriginLocation {
    value: String,
}

impl SameOriginLocation {
    /// 検証して構築する。失敗時は理由を返す（フェイルクローズ）。
    pub fn new(value: impl Into<String>) -> Result<Self, SameOriginLocationError> {
        let value = value.into();
        validate_same_origin_location(&value)?;
        Ok(Self { value })
    }

    /// 検証済みの値を `&str` で返す（入力とバイト同一）。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// 検証済みの値を `String` として取り出す。
    #[must_use]
    pub fn into_string(self) -> String {
        self.value
    }
}

impl TryFrom<&str> for SameOriginLocation {
    type Error = SameOriginLocationError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for SameOriginLocation {
    type Error = SameOriginLocationError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl AsRef<str> for SameOriginLocation {
    fn as_ref(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for SameOriginLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SameOriginLocationError as E;

    #[test]
    fn rejects_with_expected_reason() {
        let cases: &[(&str, E)] = &[
            ("", E::Empty),
            ("//evil.com", E::SchemeRelative),
            ("/\\evil.com", E::SchemeRelative),
            ("\\", E::NotRootRelative),
            ("\\\\evil.com", E::NotRootRelative),
            ("/a\\b", E::Backslash),
            ("/x\r\ny", E::ControlCharacter),
            ("/x\ty", E::ControlCharacter),
            ("/\t/evil.com", E::ControlCharacter),
            ("/x\0", E::ControlCharacter),
            ("/x\x7f", E::ControlCharacter),
            ("/x\u{85}", E::ControlCharacter),
            ("https://evil.com", E::NotRootRelative),
            ("http:/evil.com", E::NotRootRelative),
            ("evil.com/x", E::NotRootRelative),
            ("javascript:alert(1)", E::NotRootRelative),
            ("x/y", E::NotRootRelative),
            ("?a=b", E::NotRootRelative),
            ("#frag", E::NotRootRelative),
        ];
        for (input, want) in cases {
            assert_eq!(SameOriginLocation::new(*input), Err(*want), "{input:?}");
        }
    }

    #[test]
    fn accepts_and_keeps_bytes_unchanged() {
        for input in [
            "/",
            "/todos",
            "/a/b?c=d#e",
            "/%2F%2Fevil.com",
            "/%2fevil.com",
            "/%5C",
            "/%5cevil.com",
            "/a b",
            "/日本語",
            "/./x",
            "/../x",
        ] {
            let loc = SameOriginLocation::new(input).unwrap();
            assert_eq!(loc.as_str(), input);
            assert_eq!(loc.to_string(), input);
            assert_eq!(loc.into_string(), input);
        }
    }

    #[test]
    fn error_display_does_not_echo_input() {
        let err = SameOriginLocation::new("https://evil.com").unwrap_err();
        assert!(!err.to_string().contains("evil"));
    }

    #[test]
    fn try_from_matches_new() {
        assert!(SameOriginLocation::try_from("/ok").is_ok());
        assert_eq!(
            SameOriginLocation::try_from(String::from("//x")),
            Err(E::SchemeRelative)
        );
    }
}
