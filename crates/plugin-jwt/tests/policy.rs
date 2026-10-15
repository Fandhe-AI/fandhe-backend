//! 公開 API のみを使った検証ポリシー型の統合テスト（イシュー #813）。

use fandhe_backend_plugin_jwt::*;
use std::time::Duration;

fn policy() -> JwtPolicy {
    JwtPolicy::new(
        Issuer::new("https://issuer.example").unwrap(),
        Audiences::new(["api", "web"]).unwrap(),
        AllowedAlgorithms::new([Algorithm::Rs256, Algorithm::Es256]).unwrap(),
        TokenType::AccessToken,
    )
}

#[test]
fn accessors_return_constructed_values() {
    let p = policy();
    assert_eq!(p.issuer().as_str(), "https://issuer.example");
    assert!(p.audiences().contains("web"));
    assert!(!p.audiences().contains("WEB"));
    assert!(p.algorithms().contains(Algorithm::Es256));
    assert_eq!(p.token_type(), TokenType::AccessToken);
    assert_eq!(p.clock_skew(), Duration::ZERO);
}

#[test]
fn clock_skew_boundary() {
    assert!(policy().with_clock_skew(MAX_CLOCK_SKEW).is_ok());
    assert_eq!(
        policy().with_clock_skew(MAX_CLOCK_SKEW + Duration::from_nanos(1)),
        Err(PolicyError::ClockSkewTooLarge)
    );
}

#[test]
fn algorithms_reject_empty_and_dedupe() {
    assert_eq!(
        AllowedAlgorithms::new([]),
        Err(PolicyError::NoAllowedAlgorithms)
    );
    let a = AllowedAlgorithms::new([Algorithm::Rs256, Algorithm::Rs256]).unwrap();
    assert_eq!(a.iter().count(), 1);
    assert!(!AllowedAlgorithms::only(Algorithm::Rs256).contains(Algorithm::Es256));
}

#[test]
fn from_jose_name_is_strict() {
    for bad in [
        "none", "None", "HS256", "HS384", "HS512", "rs256", "", "RS256 ",
    ] {
        assert_eq!(Algorithm::from_jose_name(bad), None, "{bad}");
    }
    for a in [Algorithm::Rs256, Algorithm::Es256] {
        assert_eq!(Algorithm::from_jose_name(a.as_jose_name()), Some(a));
    }
}

#[test]
fn token_type_matching() {
    for ok in [
        "at+jwt",
        "AT+JWT",
        "application/at+jwt",
        "Application/AT+JWT",
    ] {
        assert!(TokenType::AccessToken.matches_header(ok), "{ok}");
    }
    for bad in [
        "JWT",
        "jwt",
        "logout+jwt",
        "application/jwt",
        "",
        " at+jwt",
        "at+jwt; x=1",
    ] {
        assert!(!TokenType::AccessToken.matches_header(bad), "{bad}");
    }
}
