//! [`CsrfGate`] の構築器と構築時検証。
//!
//! 設定ミス（決して一致しない許可オリジン、`null`、`*` 等）を実行時に黙って全拒否に
//! させず、構築時エラーとして検出する責務を持つ。`CsrfGate::builder` から使われる。

use crate::gate::CsrfGate;
use core::fmt;

/// 許可オリジン数の上限（設定ミス・過大入力対策）。
pub const MAX_ALLOWED_ORIGINS: usize = 64;
/// 除外パス数の上限。
pub const MAX_EXEMPT_PATHS: usize = 64;
/// 許可オリジン・除外パス 1 件あたりの最大バイト長。
pub const MAX_ENTRY_LEN: usize = 2048;

/// 構築時エラー。`Display` は入力値を含めず位置のみ示す。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsrfConfigError {
    /// `index` 番目の許可オリジンが不正な形式。
    InvalidOrigin {
        /// 登録順の 0 起点位置。
        index: usize,
    },
    /// `index` 番目の除外パスが不正な形式。
    InvalidExemptPath {
        /// 登録順の 0 起点位置。
        index: usize,
    },
    /// 許可オリジン数が [`MAX_ALLOWED_ORIGINS`] を超えた。
    TooManyOrigins,
    /// 除外パス数が [`MAX_EXEMPT_PATHS`] を超えた。
    TooManyExemptPaths,
}

impl fmt::Display for CsrfConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOrigin { index } => write!(f, "許可オリジン #{index} の形式が不正です"),
            Self::InvalidExemptPath { index } => {
                write!(f, "除外パス #{index} の形式が不正です")
            }
            Self::TooManyOrigins => {
                write!(f, "許可オリジンが上限 {MAX_ALLOWED_ORIGINS} を超えました")
            }
            Self::TooManyExemptPaths => {
                write!(f, "除外パスが上限 {MAX_EXEMPT_PATHS} を超えました")
            }
        }
    }
}

impl std::error::Error for CsrfConfigError {}

/// [`CsrfGate`] の構築器。
///
/// ```
/// use fandhe_backend_plugin_csrf::{CsrfConfigError, CsrfGate};
///
/// assert!(CsrfGate::builder().allow_origin("https://app.example.com").build().is_ok());
/// // `null`・末尾スラッシュ・既定ポート・大文字は構築エラー。
/// for bad in ["null", "https://a.example/", "https://a.example:443", "https://A.example"] {
///     assert_eq!(
///         CsrfGate::builder().allow_origin(bad).build().unwrap_err(),
///         CsrfConfigError::InvalidOrigin { index: 0 }
///     );
/// }
/// assert_eq!(
///     CsrfGate::builder().exempt_path("token").build().unwrap_err(),
///     CsrfConfigError::InvalidExemptPath { index: 0 }
/// );
/// ```
#[derive(Debug, Default, Clone)]
pub struct CsrfGateBuilder {
    origins: Vec<String>,
    exempt: Vec<String>,
}

impl CsrfGateBuilder {
    /// 許可オリジンを追加する（例 `https://app.example.com`、`http://localhost:8080`）。
    /// 空のままでも構築でき、その場合は `Sec-Fetch-Site` のみで判定する。
    #[must_use]
    pub fn allow_origin(mut self, origin: impl Into<String>) -> Self {
        self.origins.push(origin.into());
        self
    }

    /// CSRF 検査を行わないパスを追加する。`RequestHead::path()` との完全一致のみ
    /// （接頭辞一致ではない。クエリ文字列は無視される）。ブラウザ以外のクライアント用の
    /// エンドポイントを明示的に通すために使う。
    #[must_use]
    pub fn exempt_path(mut self, path: impl Into<String>) -> Self {
        self.exempt.push(path.into());
        self
    }

    /// 検証して [`CsrfGate`] を構築する。
    ///
    /// # Errors
    ///
    /// 件数上限超過、または許可オリジン・除外パスの形式不正のとき。
    pub fn build(self) -> Result<CsrfGate, CsrfConfigError> {
        if self.origins.len() > MAX_ALLOWED_ORIGINS {
            return Err(CsrfConfigError::TooManyOrigins);
        }
        if self.exempt.len() > MAX_EXEMPT_PATHS {
            return Err(CsrfConfigError::TooManyExemptPaths);
        }
        if let Some(index) = self.origins.iter().position(|o| !valid_origin(o)) {
            return Err(CsrfConfigError::InvalidOrigin { index });
        }
        if let Some(index) = self.exempt.iter().position(|p| !valid_exempt_path(p)) {
            return Err(CsrfConfigError::InvalidExemptPath { index });
        }
        Ok(CsrfGate::new(
            self.origins
                .into_iter()
                .map(String::into_boxed_str)
                .collect(),
            self.exempt
                .into_iter()
                .map(String::into_boxed_str)
                .collect(),
        ))
    }
}

/// ブラウザがシリアライズする Origin の形かを検証する（正規化はせず拒否する）。
fn valid_origin(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_ENTRY_LEN {
        return false;
    }
    let (default_port, rest) = if let Some(r) = s.strip_prefix("https://") {
        ("443", r)
    } else if let Some(r) = s.strip_prefix("http://") {
        ("80", r)
    } else {
        return false;
    };
    let port = if let Some(after) = rest.strip_prefix('[') {
        let Some(end) = after.find(']') else {
            return false;
        };
        let inner = &after[..end];
        if !valid_ipv6_literal(inner) {
            return false;
        }
        let tail = &after[end + 1..];
        if tail.is_empty() {
            None
        } else if let Some(p) = tail.strip_prefix(':') {
            Some(p)
        } else {
            return false;
        }
    } else {
        let (host, port) = match rest.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (rest, None),
        };
        if !valid_hostname(host) {
            return false;
        }
        port
    };
    port.is_none_or(|p| valid_port(p, default_port))
}

/// IPv6 リテラルがブラウザのシリアライズ形（小文字・ゼロ圧縮の正規形）と一致するかを検証する。
///
/// 構文が不正なもの、大文字・非正規形（`0:0:0:0:0:0:0:1` 等）、ブラウザが出力しない
/// ドット表記（IPv4 射影）は、実行時に決して一致しないため拒否する。
fn valid_ipv6_literal(inner: &str) -> bool {
    if inner.contains('.') {
        return false;
    }
    let Ok(addr) = inner.parse::<std::net::Ipv6Addr>() else {
        return false;
    };
    // `Ipv6Addr` の Display は IPv4 射影をドット表記にするが、ブラウザは 16 進で出力する。
    let canonical = match addr.to_ipv4_mapped() {
        Some(v4) => {
            let o = v4.octets();
            format!(
                "::ffff:{:x}:{:x}",
                u16::from_be_bytes([o[0], o[1]]),
                u16::from_be_bytes([o[2], o[3]])
            )
        }
        None => addr.to_string(),
    };
    canonical == inner
}

fn valid_hostname(h: &str) -> bool {
    !h.is_empty()
        && h.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn valid_port(p: &str, default_port: &str) -> bool {
    if p.is_empty() || p.len() > 5 || p.starts_with('0') || p == default_port {
        return false;
    }
    p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u32>().is_ok_and(|n| n <= 65535)
}

fn valid_exempt_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= MAX_ENTRY_LEN
        && p.starts_with('/')
        && p.bytes()
            .all(|b| b > 0x20 && b < 0x7f && b != b'?' && b != b'#')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin_err(o: &str) -> Option<CsrfConfigError> {
        CsrfGateBuilder::default().allow_origin(o).build().err()
    }

    #[test]
    fn valid_origins_accepted() {
        for o in [
            "https://app.example.com",
            "http://localhost:8080",
            "https://[::1]:8443",
            "https://[2001:db8::1]",
            "https://[::ffff:102:304]",
            "https://a.example:444",
            "http://a.example:443",
        ] {
            assert_eq!(origin_err(o), None, "{o}");
        }
    }

    #[test]
    fn invalid_origins_rejected_with_index() {
        for o in [
            "",
            "null",
            "*",
            "HTTPS://a.example",
            "https://A.example",
            "https://a.example/",
            "https://a.example/path",
            "https://a.example?x",
            "https://a.example#f",
            "https://u@a.example",
            "ftp://a.example",
            "https://a.example:443",
            "http://a.example:80",
            "https://a.example:0",
            "https://a.example:65536",
            "https://a.example:08080",
            "https://a..example",
            "https://.a.example",
            "https://a b.example",
            "https://é.example",
            "https://a.example:",
            "https://",
            "https://[::1",
            "https://[dead]",
            "https://[0:0:0:0:0:0:0:1]",
            "https://[::FFFF]",
            "https://[2001:DB8::1]",
            "https://[::ffff:1.2.3.4]",
            "https://[2001:db8:0:0:0:0:0:1]",
            "https://[]",
            "https://a.example https://b.example",
        ] {
            let err = CsrfGateBuilder::default()
                .allow_origin("https://ok.example")
                .allow_origin(o)
                .build()
                .unwrap_err();
            assert_eq!(err, CsrfConfigError::InvalidOrigin { index: 1 }, "{o:?}");
        }
    }

    #[test]
    fn exempt_paths_validated() {
        for p in ["", "token", "/a?b", "/a#b", "/a b", "/a\u{1}b", "/é"] {
            let err = CsrfGateBuilder::default()
                .exempt_path(p)
                .build()
                .unwrap_err();
            assert_eq!(
                err,
                CsrfConfigError::InvalidExemptPath { index: 0 },
                "{p:?}"
            );
        }
        assert!(
            CsrfGateBuilder::default()
                .exempt_path("/token")
                .build()
                .is_ok()
        );
    }

    #[test]
    fn limits_enforced() {
        let mut b = CsrfGateBuilder::default();
        for _ in 0..=MAX_ALLOWED_ORIGINS {
            b = b.allow_origin("https://a.example");
        }
        assert_eq!(b.build().unwrap_err(), CsrfConfigError::TooManyOrigins);
        let mut b = CsrfGateBuilder::default();
        for _ in 0..=MAX_EXEMPT_PATHS {
            b = b.exempt_path("/a");
        }
        assert_eq!(b.build().unwrap_err(), CsrfConfigError::TooManyExemptPaths);
        let long = format!("/{}", "a".repeat(MAX_ENTRY_LEN));
        assert!(
            CsrfGateBuilder::default()
                .exempt_path(long)
                .build()
                .is_err()
        );
        let long = format!("https://{}.example", "a".repeat(MAX_ENTRY_LEN));
        assert!(
            CsrfGateBuilder::default()
                .allow_origin(long)
                .build()
                .is_err()
        );
    }

    #[test]
    fn display_does_not_echo_input() {
        let e = CsrfGateBuilder::default()
            .allow_origin("secret-value")
            .build()
            .unwrap_err();
        assert!(!e.to_string().contains("secret-value"));
    }
}
