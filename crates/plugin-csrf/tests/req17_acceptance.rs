//! REQ-17 受け入れ基準 1 の網羅受け入れテスト（イシュー #826）。
//!
//! 公開 API（`CsrfGate` / `CsrfGateBuilder` / `RequestGate::check`）だけを使う。受け入れ
//! レポート（#832）から次の対応表で引用する。
//!
//! | 基準 | 項目 | テスト |
//! |---|---|---|
//! | 1 | 基準要求の許可（拒否原因の特定用） | `baseline_is_allowed` |
//! | 1 | `Sec-Fetch-Site: same-origin` / `none` | `c1_sec_fetch_site_same_origin_allowed` / `c1_sec_fetch_site_none_allowed` |
//! | 1 | `Sec-Fetch-Site: same-site` / `cross-site` | `c1_sec_fetch_site_same_site_rejected` / `c1_sec_fetch_site_cross_site_rejected` |
//! | 1 | `Origin` 一致 | `c1_origin_match_allowed` |
//! | 1 | `Origin` 不一致 | `c1_origin_mismatch_rejected` |
//! | 1 | `Origin: null` | `c1_origin_null_rejected` |
//! | 1 | 両ヘッダ無し | `c1_both_headers_absent_rejected` |
//! | 1 | 安全なメソッドの素通し | `c1_safe_methods_pass_through` |
//! | 1 | パスの除外（完全一致のみ） | `c1_exempt_path_exact_match_only` |
//! | 1 | 拒否応答は一様な素の 403 | `c1_rejection_is_bare_403` |
//!
//! 各拒否テストは「その 1 点だけを変えた」要求で行い、変える前の基準要求が許可される
//! こと（`baseline_is_allowed`）で拒否原因を特定可能にする。

use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_csrf::CsrfGate;

const APP: &str = "https://app.example.com";

fn gate() -> CsrfGate {
    CsrfGate::builder()
        .allow_origin(APP)
        .exempt_path("/token")
        .build()
        .unwrap()
}

/// 要求行とヘッダ行（`Name: value`）から `check` の結果を得る。
fn check(request_line: &str, headers: &[&str]) -> GateOutcome {
    let mut raw = format!("{request_line}\r\nHost: app.example.com\r\n");
    for h in headers {
        raw.push_str(h);
        raw.push_str("\r\n");
    }
    raw.push_str("\r\n");
    let ParseOutcome::Complete { head, .. } = parse_request_head(raw.as_bytes()).unwrap() else {
        unreachable!()
    };
    gate().check(
        &head,
        &GateContext::new(None),
        &mut fandhe_backend_core::extension::RequestExtensions::new(),
    )
}

fn post(headers: &[&str]) -> GateOutcome {
    check("POST /f HTTP/1.1", headers)
}

fn bare_403() -> GateOutcome {
    GateOutcome::reject(403, Vec::new())
}

#[test]
fn baseline_is_allowed() {
    assert_eq!(post(&[&format!("Origin: {APP}")]), GateOutcome::Allow);
}

#[test]
fn c1_sec_fetch_site_same_origin_allowed() {
    assert_eq!(post(&["Sec-Fetch-Site: same-origin"]), GateOutcome::Allow);
}

#[test]
fn c1_sec_fetch_site_none_allowed() {
    assert_eq!(post(&["Sec-Fetch-Site: none"]), GateOutcome::Allow);
}

#[test]
fn c1_sec_fetch_site_same_site_rejected() {
    assert_eq!(post(&["Sec-Fetch-Site: same-site"]), bare_403());
    // 許可済み Origin を併記しても Sec-Fetch-Site が優先される。
    assert_eq!(
        post(&["Sec-Fetch-Site: same-site", &format!("Origin: {APP}")]),
        bare_403()
    );
}

#[test]
fn c1_sec_fetch_site_cross_site_rejected() {
    assert_eq!(post(&["Sec-Fetch-Site: cross-site"]), bare_403());
    assert_eq!(
        post(&["Sec-Fetch-Site: cross-site", &format!("Origin: {APP}")]),
        bare_403()
    );
}

#[test]
fn c1_origin_match_allowed() {
    for m in ["POST", "PUT", "PATCH", "DELETE"] {
        assert_eq!(
            check(&format!("{m} /f HTTP/1.1"), &[&format!("Origin: {APP}")]),
            GateOutcome::Allow,
            "{m}"
        );
    }
}

#[test]
fn c1_origin_mismatch_rejected() {
    for o in [
        "https://evil.example",
        "https://APP.example.com",
        "https://app.example.com/",
        "https://app.example.com:8443",
        "http://app.example.com",
    ] {
        assert_eq!(post(&[&format!("Origin: {o}")]), bare_403(), "{o}");
    }
}

#[test]
fn c1_origin_null_rejected() {
    assert_eq!(post(&["Origin: null"]), bare_403());
}

#[test]
fn c1_both_headers_absent_rejected() {
    assert_eq!(post(&[]), bare_403());
}

#[test]
fn c1_safe_methods_pass_through() {
    let hostile = ["Sec-Fetch-Site: cross-site", "Origin: null"];
    for m in ["GET", "HEAD", "OPTIONS"] {
        assert_eq!(
            check(&format!("{m} /f HTTP/1.1"), &hostile),
            GateOutcome::Allow,
            "{m}"
        );
    }
    // 小文字・拡張メソッドは安全側で検査される。
    for m in ["get", "PROPFIND"] {
        assert_eq!(
            check(&format!("{m} /f HTTP/1.1"), &hostile),
            bare_403(),
            "{m}"
        );
    }
}

#[test]
fn c1_exempt_path_exact_match_only() {
    for t in ["/token", "/token?x=1"] {
        assert_eq!(
            check(&format!("POST {t} HTTP/1.1"), &[]),
            GateOutcome::Allow,
            "{t}"
        );
    }
    for t in ["/token/", "/tokenX", "/token/extra", "/tok%65n"] {
        assert_eq!(check(&format!("POST {t} HTTP/1.1"), &[]), bare_403(), "{t}");
    }
}

#[test]
fn c1_rejection_is_bare_403() {
    let rejections = [
        post(&[]),
        post(&["Origin: null"]),
        post(&["Origin: https://evil.example"]),
        post(&["Sec-Fetch-Site: cross-site"]),
    ];
    let mut first: Option<Vec<u8>> = None;
    for r in rejections {
        let GateOutcome::Reject { response } = r else {
            panic!("expected reject");
        };
        let bytes = response.serialize(false);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{text}");
        assert!(text.contains("Content-Length: 0\r\n"), "{text}");
        assert!(
            !text.to_ascii_lowercase().contains("content-type"),
            "{text}"
        );
        assert!(text.ends_with("\r\n\r\n"), "{text}");
        match &first {
            None => first = Some(bytes),
            Some(f) => assert_eq!(f, &bytes),
        }
    }
}
