//! [`CsrfGate`]: `RequestGate` 拡張点の実装。
//!
//! コアの接続ループが `Server::gate` 登録順にリクエストヘッド到達直後（ルーティング前）に
//! 呼び出す。判定規則と設計判断はクレート doc を参照。I/O・ロック・ヒープ確保を行わない。

use crate::config::CsrfGateBuilder;
use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
use fandhe_backend_http::request::RequestHead;

/// CSRF 検査ゲート。`CsrfGate::builder()` で構築する不変オブジェクト。
///
/// ```
/// use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
/// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// use fandhe_backend_plugin_csrf::CsrfGate;
///
/// let gate = CsrfGate::builder()
///     .allow_origin("https://app.example.com")
///     .exempt_path("/token")
///     .build()
///     .unwrap();
/// let ctx = GateContext::new(None);
/// let check = |raw: &[u8]| {
///     let ParseOutcome::Complete { head, .. } = parse_request_head(raw).unwrap() else {
///         unreachable!()
///     };
///     gate.check(&head, &ctx)
/// };
/// // 安全なメソッドは素通し。
/// assert_eq!(check(b"GET / HTTP/1.1\r\nSec-Fetch-Site: cross-site\r\n\r\n"), GateOutcome::Allow);
/// // 同一オリジンの POST は許可、cross-site は 403。
/// assert_eq!(check(b"POST /f HTTP/1.1\r\nSec-Fetch-Site: same-origin\r\n\r\n"), GateOutcome::Allow);
/// assert_eq!(
///     check(b"POST /f HTTP/1.1\r\nSec-Fetch-Site: cross-site\r\n\r\n"),
///     GateOutcome::reject(403, Vec::new())
/// );
/// // Sec-Fetch-Site が無ければ Origin を許可リストと照合する。
/// assert_eq!(check(b"POST /f HTTP/1.1\r\nOrigin: https://app.example.com\r\n\r\n"), GateOutcome::Allow);
/// assert_eq!(
///     check(b"POST /f HTTP/1.1\r\nOrigin: null\r\n\r\n"),
///     GateOutcome::reject(403, Vec::new())
/// );
/// // 除外パスは検査しない。ヘッダが無ければ拒否。
/// assert_eq!(check(b"POST /token HTTP/1.1\r\n\r\n"), GateOutcome::Allow);
/// assert_eq!(check(b"POST /f HTTP/1.1\r\n\r\n"), GateOutcome::reject(403, Vec::new()));
/// ```
#[derive(Debug, Clone)]
pub struct CsrfGate {
    allowed_origins: Box<[Box<str>]>,
    exempt_paths: Box<[Box<str>]>,
}

impl CsrfGate {
    /// 構築器を返す。
    #[must_use]
    pub fn builder() -> CsrfGateBuilder {
        CsrfGateBuilder::default()
    }

    pub(crate) fn new(allowed_origins: Box<[Box<str>]>, exempt_paths: Box<[Box<str>]>) -> Self {
        Self {
            allowed_origins,
            exempt_paths,
        }
    }

    fn deny() -> GateOutcome {
        GateOutcome::reject(403, Vec::new())
    }
}

/// 指定ヘッダの件数（2 で頭打ち）と最初の値。
fn count_header<'a>(head: &'a RequestHead, name: &str) -> (u8, Option<&'a str>) {
    let mut n = 0u8;
    let mut first = None;
    for (k, v) in head.headers() {
        if k.eq_ignore_ascii_case(name) {
            if n == 0 {
                first = Some(v);
            }
            n = (n + 1).min(2);
        }
    }
    (n, first)
}

impl RequestGate for CsrfGate {
    fn name(&self) -> &'static str {
        "csrf-gate"
    }

    fn check(&self, head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
        if matches!(head.method(), "GET" | "HEAD" | "OPTIONS") {
            return GateOutcome::Allow;
        }
        let path = head.path();
        if self.exempt_paths.iter().any(|p| &**p == path) {
            return GateOutcome::Allow;
        }
        match count_header(head, "sec-fetch-site") {
            (0, _) => {}
            (1, Some("same-origin" | "none")) => return GateOutcome::Allow,
            _ => return Self::deny(),
        }
        match count_header(head, "origin") {
            (1, Some(o)) if self.allowed_origins.iter().any(|a| &**a == o) => GateOutcome::Allow,
            _ => Self::deny(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    fn gate() -> CsrfGate {
        CsrfGate::builder()
            .allow_origin("https://app.example.com")
            .exempt_path("/token")
            .build()
            .unwrap()
    }

    fn run(g: &CsrfGate, req: &str) -> GateOutcome {
        let raw = format!("{req}\r\n\r\n");
        let ParseOutcome::Complete { head, .. } = parse_request_head(raw.as_bytes()).unwrap()
        else {
            panic!("incomplete")
        };
        g.check(&head, &GateContext::new(None))
    }

    fn allowed(req: &str) -> bool {
        run(&gate(), req) == GateOutcome::Allow
    }

    #[test]
    fn name_is_fixed() {
        assert_eq!(gate().name(), "csrf-gate");
    }

    #[test]
    fn safe_methods_pass() {
        for m in ["GET", "HEAD", "OPTIONS"] {
            assert!(allowed(&format!(
                "{m} / HTTP/1.1\r\nSec-Fetch-Site: cross-site\r\nOrigin: https://evil.example"
            )));
        }
        assert!(!allowed("get / HTTP/1.1"));
    }

    #[test]
    fn sec_fetch_site_values() {
        for v in ["same-origin", "none"] {
            assert!(allowed(&format!("POST /f HTTP/1.1\r\nSec-Fetch-Site: {v}")));
        }
        for v in ["same-site", "cross-site", "Same-Origin", "", "weird"] {
            assert!(
                !allowed(&format!("POST /f HTTP/1.1\r\nSec-Fetch-Site: {v}")),
                "{v:?}"
            );
        }
        assert!(!allowed(
            "POST /f HTTP/1.1\r\nSec-Fetch-Site: same-origin\r\nSec-Fetch-Site: same-origin"
        ));
    }

    #[test]
    fn sec_fetch_site_takes_precedence() {
        assert!(allowed(
            "POST /f HTTP/1.1\r\nSec-Fetch-Site: same-origin\r\nOrigin: https://evil.example"
        ));
        assert!(!allowed(
            "POST /f HTTP/1.1\r\nSec-Fetch-Site: cross-site\r\nOrigin: https://app.example.com"
        ));
    }

    #[test]
    fn origin_matching() {
        for m in ["POST", "PUT", "DELETE", "PATCH"] {
            assert!(allowed(&format!(
                "{m} /f HTTP/1.1\r\nOrigin: https://app.example.com"
            )));
        }
        for o in [
            "https://evil.example",
            "https://APP.example.com",
            "https://app.example.com/",
            "null",
            "https://app.example.com https://evil.example",
        ] {
            assert!(!allowed(&format!("POST /f HTTP/1.1\r\nOrigin: {o}")), "{o}");
        }
        assert!(!allowed(
            "POST /f HTTP/1.1\r\nOrigin: https://app.example.com\r\nOrigin: https://app.example.com"
        ));
    }

    #[test]
    fn missing_headers_rejected() {
        assert!(!allowed("POST /f HTTP/1.1"));
    }

    #[test]
    fn exempt_path_exact_only() {
        assert!(allowed("POST /token HTTP/1.1"));
        assert!(allowed("POST /token?x=1 HTTP/1.1"));
        for p in ["/token/", "/token/extra", "/tokenX", "/tok%65n"] {
            assert!(!allowed(&format!("POST {p} HTTP/1.1")), "{p}");
        }
    }

    #[test]
    fn empty_allow_list_uses_sec_fetch_only() {
        let g = CsrfGate::builder().build().unwrap();
        assert_eq!(
            run(&g, "POST /f HTTP/1.1\r\nSec-Fetch-Site: same-origin"),
            GateOutcome::Allow
        );
        assert_ne!(
            run(&g, "POST /f HTTP/1.1\r\nOrigin: https://a.example"),
            GateOutcome::Allow
        );
    }

    #[test]
    fn all_rejections_are_identical_bare_403() {
        let g = gate();
        let mut wires = Vec::new();
        for req in [
            "POST /f HTTP/1.1",
            "POST /f HTTP/1.1\r\nSec-Fetch-Site: cross-site",
            "POST /f HTTP/1.1\r\nOrigin: null",
            "POST /f HTTP/1.1\r\nOrigin: https://evil.example",
        ] {
            let GateOutcome::Reject { response } = run(&g, req) else {
                panic!("expected reject: {req}")
            };
            wires.push(response.serialize(false));
        }
        assert!(wires.windows(2).all(|w| w[0] == w[1]));
        let text = String::from_utf8(wires.remove(0)).unwrap();
        assert!(text.starts_with("HTTP/1.1 403"));
        assert!(text.contains("Content-Length: 0\r\n"));
        assert!(!text.contains("Content-Type"));
        assert!(text.ends_with("\r\n\r\n"));
    }
}
