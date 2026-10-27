//! DPoP の `htu` 照合に使う公開オリジン・期待 URI の型と、絶対 URI の正規化（イシュー #901）。
//!
//! 役割: 設計 9.5 節の「`htu` は公開オリジン（設定値）+ リクエストパスの完全一致」を型で
//! 表現する。`Host`・`X-Forwarded-*` は使わず、デプロイ設定で渡された値だけを根拠にする。
//! `crate::dpop::verify_proof` が proof の `htu` を同じ正規化規則で解き、[`ExpectedHtu`] と
//! バイト単位で比べる。
//!
//! # 正規化の規則
//!
//! - scheme は `https` / `http`（ASCII の大文字小文字を無視）。小文字にする。
//! - host は ASCII のみ。小文字にする。IPv6 は角括弧の形で、16 進数字・`:`・`.` のみ。
//! - port は 1〜65535。既定のポート（https は 443、http は 80）は省く。
//! - userinfo は拒否する。パスはパーセントデコードせず、`%XX` の 16 進数字だけ大文字にする。
//! - `.` / `..` セグメントは拒否する（`%2E` を含む表記も拒否し、デコード差を突く迂回を防ぐ）。
//! - 制御文字・空白・非 ASCII・`\` は拒否する。

use std::fmt;

/// 公開オリジン文字列の最大バイト長。
pub const MAX_ORIGIN_LEN: usize = 512;
/// `htu`（期待 URI・proof の `htu`）の最大バイト長。
pub const MAX_HTU_LEN: usize = 4096;

/// 公開オリジンの検証エラー。固定文言で、入力値を含まない。
///
/// ```
/// use fandhe_backend_plugin_jwt::dpop::{OriginError, PublicOrigin};
/// assert_eq!(PublicOrigin::new("ftp://a").unwrap_err(), OriginError::InvalidScheme);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OriginError {
    /// 長さが [`MAX_ORIGIN_LEN`] を超えた。
    TooLong,
    /// scheme が `https` / `http` ではない、または `://` がない。
    InvalidScheme,
    /// host が空、または許可しない文字を含む。
    InvalidHost,
    /// port が 1〜65535 の 10 進数ではない。
    InvalidPort,
    /// userinfo（`user@`）を含む。
    UserInfo,
    /// パスを含む（空と `/` 以外）。
    PathNotAllowed,
    /// クエリまたはフラグメントを含む。
    QueryOrFragment,
}

impl fmt::Display for OriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::TooLong => "公開オリジンが長さ上限を超えています",
            Self::InvalidScheme => "公開オリジンの scheme は https または http のみです",
            Self::InvalidHost => "公開オリジンの host が不正です",
            Self::InvalidPort => "公開オリジンの port が不正です",
            Self::UserInfo => "公開オリジンに userinfo は含められません",
            Self::PathNotAllowed => "公開オリジンにパスは含められません",
            Self::QueryOrFragment => "公開オリジンにクエリ・フラグメントは含められません",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for OriginError {}

/// デプロイ設定で与える公開オリジン（`scheme://host[:port]`）。構築時に検証・正規化済み。
///
/// 本番では `https` を使うこと（`http` は開発・テスト用に受理するのみ）。
/// 要求の `Host` ヘッダや `X-Forwarded-*` から導出してはならない。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_jwt::dpop::PublicOrigin;
///
/// let o = PublicOrigin::new("HTTPS://API.Example:443/")?;
/// assert_eq!(o.as_str(), "https://api.example");
/// assert!(PublicOrigin::new("https://user@api.example").is_err());
/// # Ok::<(), fandhe_backend_plugin_jwt::dpop::OriginError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicOrigin(Box<str>);

impl PublicOrigin {
    /// 文字列から公開オリジンを構築する。末尾の `/` 1 個は許す。
    pub fn new(origin: &str) -> Result<Self, OriginError> {
        if origin.len() > MAX_ORIGIN_LEN {
            return Err(OriginError::TooLong);
        }
        let (normalized, rest) = parse_origin(origin)?;
        match rest {
            "" | "/" => Ok(Self(normalized.into_boxed_str())),
            r if r.contains(['?', '#']) => Err(OriginError::QueryOrFragment),
            _ => Err(OriginError::PathNotAllowed),
        }
    }

    /// 正規化済みの文字列（末尾に `/` を付けない）。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// リクエストパスから期待 `htu` を作る。
    ///
    /// `path` は `/` で始まる origin-form のパスで、クエリ・フラグメントを含まないこと
    /// （呼び出し側が `RequestHead::path` 等で分離済みの値を渡す）。absolute-form
    /// （`http://…`）・`*` は拒否する。
    ///
    /// ```
    /// use fandhe_backend_plugin_jwt::dpop::PublicOrigin;
    /// let o = PublicOrigin::new("https://api.example")?;
    /// assert_eq!(o.expected_htu("/a%2fb")?.as_str(), "https://api.example/a%2Fb");
    /// assert!(o.expected_htu("*").is_err());
    /// assert!(o.expected_htu("/a?x=1").is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn expected_htu(&self, path: &str) -> Result<ExpectedHtu, super::DpopError> {
        if !path.starts_with('/') || path.contains(['?', '#']) {
            return Err(super::DpopError::InvalidRequestTarget);
        }
        let path = normalize_path(path).ok_or(super::DpopError::InvalidRequestTarget)?;
        let full = format!("{}{}", self.0, path);
        if full.len() > MAX_HTU_LEN {
            return Err(super::DpopError::InvalidRequestTarget);
        }
        Ok(ExpectedHtu(full.into_boxed_str()))
    }
}

impl fmt::Display for PublicOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 期待する `htu`（正規化済みの公開オリジン + 正規化済みパス）。
///
/// 要求ごとに [`PublicOrigin::expected_htu`] で作る。proof の `htu` は同じ規則で正規化され、
/// 本値とバイト単位で完全一致したときだけ受理される。
///
/// ```
/// use fandhe_backend_plugin_jwt::dpop::PublicOrigin;
/// let h = PublicOrigin::new("https://api.example")?.expected_htu("/")?;
/// assert_eq!(h.as_str(), "https://api.example/");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedHtu(Box<str>);

impl ExpectedHtu {
    /// 正規化済みの文字列。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// proof の `htu` を正規化する（クエリ・フラグメントは捨てる）。解析できなければ `None`。
pub(crate) fn normalize_proof_htu(htu: &str) -> Option<String> {
    if htu.len() > MAX_HTU_LEN {
        return None;
    }
    let (origin, rest) = parse_origin(htu).ok()?;
    let rest = rest.split('#').next().unwrap_or("");
    let rest = rest.split('?').next().unwrap_or("");
    let path = normalize_path(rest)?;
    Some(format!("{origin}{path}"))
}

/// `scheme://authority` を解き、正規化した `scheme://host[:port]` と残り（パス以降）を返す。
fn parse_origin(s: &str) -> Result<(String, &str), OriginError> {
    let (scheme, after) = s.split_once("://").ok_or(OriginError::InvalidScheme)?;
    let (scheme, default_port) = if scheme.eq_ignore_ascii_case("https") {
        ("https", 443u16)
    } else if scheme.eq_ignore_ascii_case("http") {
        ("http", 80u16)
    } else {
        return Err(OriginError::InvalidScheme);
    };
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (authority, rest) = after.split_at(end);
    if authority.contains('@') {
        return Err(OriginError::UserInfo);
    }
    let (host, port_str) = if authority.starts_with('[') {
        let close = authority.find(']').ok_or(OriginError::InvalidHost)?;
        let inner = &authority[1..close];
        let ok = !inner.is_empty()
            && inner.contains(':')
            && inner
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
        if !ok {
            return Err(OriginError::InvalidHost);
        }
        let tail = &authority[close + 1..];
        let port = match tail {
            "" => None,
            t => Some(t.strip_prefix(':').ok_or(OriginError::InvalidHost)?),
        };
        (authority[..=close].to_ascii_lowercase(), port)
    } else {
        let (h, p) = match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        };
        let ok = !h.is_empty()
            && h.len() <= 253
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        if !ok {
            return Err(OriginError::InvalidHost);
        }
        (h.to_ascii_lowercase(), p)
    };
    let port = match port_str {
        None => None,
        Some(p) => {
            if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(OriginError::InvalidPort);
            }
            let n: u16 = p.parse().map_err(|_| OriginError::InvalidPort)?;
            if n == 0 {
                return Err(OriginError::InvalidPort);
            }
            (n != default_port).then_some(n)
        }
    };
    let mut out = format!("{scheme}://{host}");
    if let Some(n) = port {
        out.push(':');
        out.push_str(&n.to_string());
    }
    Ok((out, rest))
}

/// パスを正規化する。空は `/`。不正なら `None`。デコードはしない。
fn normalize_path(path: &str) -> Option<String> {
    if path.is_empty() {
        return Some("/".to_owned());
    }
    if !path.starts_with('/') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if !(0x21..=0x7E).contains(&b) || b == b'\\' {
            return None;
        }
        if b == b'%' {
            let (h1, h2) = (bytes.get(i + 1)?, bytes.get(i + 2)?);
            if !h1.is_ascii_hexdigit() || !h2.is_ascii_hexdigit() {
                return None;
            }
            out.push('%');
            out.push(h1.to_ascii_uppercase() as char);
            out.push(h2.to_ascii_uppercase() as char);
            i += 3;
        } else {
            out.push(b as char);
            i += 1;
        }
    }
    for seg in out.split('/') {
        let dots = seg.replace("%2E", ".");
        if dots == "." || dots == ".." {
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_normalization() {
        let cases = [
            ("https://api.example", "https://api.example"),
            ("https://api.example/", "https://api.example"),
            ("HTTPS://API.Example:443", "https://api.example"),
            ("http://a.example:80", "http://a.example"),
            ("https://a.example:8443", "https://a.example:8443"),
            ("https://[FE80::1]:443/", "https://[fe80::1]"),
            ("http://[::1]:8080", "http://[::1]:8080"),
        ];
        for (input, want) in cases {
            assert_eq!(PublicOrigin::new(input).unwrap().as_str(), want, "{input}");
        }
    }

    #[test]
    fn origin_rejections() {
        let cases = [
            ("ftp://a.example", OriginError::InvalidScheme),
            ("a.example", OriginError::InvalidScheme),
            ("https://u@a.example", OriginError::UserInfo),
            ("https://a.example:0", OriginError::InvalidPort),
            ("https://a.example:65536", OriginError::InvalidPort),
            ("https://a.example:", OriginError::InvalidPort),
            ("https://a.example:8a", OriginError::InvalidPort),
            ("https://a.example/x", OriginError::PathNotAllowed),
            ("https://a.example?x=1", OriginError::QueryOrFragment),
            ("https://a.example/#f", OriginError::QueryOrFragment),
            ("https://", OriginError::InvalidHost),
            ("https://é.example", OriginError::InvalidHost),
            ("https://[fe80::1%25eth0]", OriginError::InvalidHost),
            ("https://[zz::1]", OriginError::InvalidHost),
            ("https://a b.example", OriginError::InvalidHost),
        ];
        for (input, want) in cases {
            assert_eq!(PublicOrigin::new(input).unwrap_err(), want, "{input}");
        }
        let long = format!("https://{}.example", "a".repeat(MAX_ORIGIN_LEN));
        assert_eq!(PublicOrigin::new(&long).unwrap_err(), OriginError::TooLong);
    }

    #[test]
    fn path_normalization_and_rejections() {
        assert_eq!(normalize_path("/a%2fb").as_deref(), Some("/a%2Fb"));
        assert_eq!(normalize_path("").as_deref(), Some("/"));
        for bad in [
            "/a/../b", "/./a", "/%2e%2E/", "/.%2e", "/a%zz", "/a%2", "/a b", "/é", "/a\\b",
            "/a\u{0}",
        ] {
            assert!(normalize_path(bad).is_none(), "{bad:?}");
        }
        assert!(normalize_path("/a/.b/..c/...").is_some());
    }

    #[test]
    fn proof_htu_drops_query_and_fragment() {
        assert_eq!(
            normalize_proof_htu("HTTPS://Api.Example:443/p?x=1#f").as_deref(),
            Some("https://api.example/p")
        );
        assert_eq!(
            normalize_proof_htu("https://api.example").as_deref(),
            Some("https://api.example/")
        );
        assert!(normalize_proof_htu("https://u@api.example/").is_none());
        assert!(normalize_proof_htu("/p").is_none());
    }
}
