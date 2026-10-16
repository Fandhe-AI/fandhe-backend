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

/// ホスト名がブラウザの Origin シリアライズ形（小文字 ASCII）と一致するかを検証する。
///
/// 絶対 FQDN 形式の末尾ドット 1 個（`app.example.com.`）はブラウザがそのまま送るため許可する。
/// 空ラベル（`a..b`・先頭ドット・末尾ドット 2 個以上）は拒否する。
fn valid_hostname(h: &str) -> bool {
    let had_dot = h.ends_with('.');
    let h = h.strip_suffix('.').unwrap_or(h);
    if ends_in_number(h) {
        // ブラウザは IPv4 として解釈されるホストを正規形（ドット 10 進 4 オクテット）へ
        // シリアライズするため、非正規形・末尾ドット付きは決して一致しない。
        return !had_dot && canonical_ipv4(h);
    }
    !h.is_empty()
        && h.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// WHATWG URL の「ends in a number」判定: 最後のラベルが 10 進数字列、または `0x` + 16 進数字列。
fn ends_in_number(h: &str) -> bool {
    let last = h.rsplit('.').next().unwrap_or("");
    if last.is_empty() {
        return false;
    }
    if last.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    last.strip_prefix("0x")
        .is_some_and(|r| r.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// 正規形のドット 10 進 IPv4（4 オクテット・各 0..=255・先行ゼロなし）か。
fn canonical_ipv4(h: &str) -> bool {
    let mut n = 0;
    for part in h.split('.') {
        n += 1;
        let ok = !part.is_empty()
            && part.len() <= 3
            && part.bytes().all(|b| b.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'))
            && part.parse::<u16>().is_ok_and(|v| v <= 255);
        if !ok {
            return false;
        }
    }
    n == 4
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
    fn non_canonical_ipv4_hosts_rejected() {
        for o in [
            "http://127.1",
            "http://2130706433",
            "http://0x7f.1",
            "http://0x7f000001",
            "http://127.0.0.01",
            "http://127.0.0.1.",
            "http://256.0.0.1",
            "http://1.2.3",
            "http://1.2.3.4.5",
        ] {
            assert!(origin_err(o).is_some(), "{o} は拒否されるべき");
        }
        assert!(origin_err("http://127.0.0.1").is_none());
        assert!(origin_err("http://127.0.0.1:8080").is_none());
        assert!(origin_err("https://1a.example.com").is_none());
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
            "https://app.example.com.",
            "http://localhost.:8080",
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
            "https://a.example..",
            "https://.",
            "https://.:8080",
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
