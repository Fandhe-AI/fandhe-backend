//! `verify` の公開 API だけを使った統合テスト（イシュー #815）。網羅テストは #817。
//! 鍵はすべてテスト専用（ES256 は実行時生成、RS256 は `tests/fixtures`）。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_plugin_jwt::*;
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
    RsaPublicKeyComponents,
};
use serde::Deserialize;

const ISS: &str = "https://issuer.example";
const NOW: u64 = 1_000_000;
const RSA_PK8: &[u8] = include_bytes!("fixtures/test-rsa-2048.pk8");

struct Ec(EcdsaKeyPair);

impl Ec {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let pk8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        Self(
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pk8.as_ref(), &rng).unwrap(),
        )
    }
    fn jwk(&self, kid: &str) -> String {
        let pt = self.0.public_key().as_ref();
        format!(
            r#"{{"kty":"EC","kid":"{kid}","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}"#,
            B.encode(&pt[1..33]),
            B.encode(&pt[33..65])
        )
    }
    fn sign(&self, msg: &str) -> Vec<u8> {
        self.0
            .sign(&SystemRandom::new(), msg.as_bytes())
            .unwrap()
            .as_ref()
            .to_vec()
    }
}

fn rsa_pair() -> RsaKeyPair {
    RsaKeyPair::from_pkcs8(RSA_PK8).unwrap()
}

fn rsa_jwk(kid: &str) -> String {
    let kp = rsa_pair();
    let c = RsaPublicKeyComponents::<Vec<u8>>::from(kp.public());
    format!(
        r#"{{"kty":"RSA","kid":"{kid}","alg":"RS256","n":"{}","e":"{}"}}"#,
        B.encode(&c.n),
        B.encode(&c.e)
    )
}

fn rsa_sign(msg: &str) -> Vec<u8> {
    let kp = rsa_pair();
    let mut sig = vec![0u8; kp.public().modulus_len()];
    kp.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        msg.as_bytes(),
        &mut sig,
    )
    .unwrap();
    sig
}

fn keyset(jwks: &[String]) -> KeySet {
    KeySet::from_json(&format!(r#"{{"keys":[{}]}}"#, jwks.join(","))).unwrap()
}

fn policy_with(algs: &[Algorithm], skew: u64) -> JwtPolicy {
    JwtPolicy::new(
        Issuer::new(ISS).unwrap(),
        Audiences::single("api").unwrap(),
        AllowedAlgorithms::new(algs.iter().copied()).unwrap(),
        TokenType::AccessToken,
    )
    .with_clock_skew(std::time::Duration::from_secs(skew))
    .unwrap()
}

fn policy() -> JwtPolicy {
    policy_with(&[Algorithm::Rs256, Algorithm::Es256], 0)
}

const HDR: &str = r#"{"alg":"ES256","kid":"k","typ":"at+jwt"}"#;

fn payload(extra: &str) -> String {
    format!(
        r#"{{"iss":"{ISS}","aud":"api","exp":{}{extra}}}"#,
        NOW + 100
    )
}

fn ec_token(ec: &Ec, header: &str, body: &str) -> String {
    let msg = format!("{}.{}", B.encode(header), B.encode(body));
    format!("{msg}.{}", B.encode(ec.sign(&msg)))
}

fn run(token: &str, keys: &KeySet) -> Result<Claims, VerifyError> {
    verify::<NoExtraClaims>(token, keys, &policy(), NOW)
}

fn ec_fixture() -> (Ec, KeySet) {
    let ec = Ec::new();
    let ks = keyset(&[ec.jwk("k")]);
    (ec, ks)
}

fn expect(header: &str, body: &str, want: VerifyError) {
    let (ec, ks) = ec_fixture();
    assert_eq!(run(&ec_token(&ec, header, body), &ks).unwrap_err(), want);
}

#[test]
fn accepts_es256_and_rs256() {
    let (ec, ks) = ec_fixture();
    let c = run(
        &ec_token(
            &ec,
            HDR,
            &payload(r#","sub":"u","jti":"j","client_id":"c","scope":"a b","iat":5,"nbf":10"#),
        ),
        &ks,
    )
    .unwrap();
    assert_eq!(c.iss(), ISS);
    assert_eq!(c.sub(), Some("u"));
    assert_eq!(c.aud(), ["api".to_string()]);
    assert_eq!(c.exp(), NOW + 100);
    assert_eq!((c.nbf(), c.iat()), (Some(10), Some(5)));
    assert_eq!(
        (c.jti(), c.client_id(), c.scope()),
        (Some("j"), Some("c"), Some("a b"))
    );

    let ks = keyset(&[rsa_jwk("r")]);
    let msg = format!(
        "{}.{}",
        B.encode(r#"{"alg":"RS256","kid":"r","typ":"at+jwt"}"#),
        B.encode(payload(""))
    );
    let token = format!("{msg}.{}", B.encode(rsa_sign(&msg)));
    assert_eq!(run(&token, &ks).unwrap().iss(), ISS);
}

#[test]
fn typ_variants_and_aud_array() {
    for typ in ["at+jwt", "application/at+jwt", "AT+JWT"] {
        let (ec, ks) = ec_fixture();
        let h = format!(r#"{{"alg":"ES256","kid":"k","typ":"{typ}"}}"#);
        assert!(run(&ec_token(&ec, &h, &payload("")), &ks).is_ok(), "{typ}");
    }
    let (ec, ks) = ec_fixture();
    let body = format!(r#"{{"iss":"{ISS}","aud":["x","api"],"exp":{}}}"#, NOW + 1);
    assert!(run(&ec_token(&ec, HDR, &body), &ks).is_ok());
}

#[test]
fn leeway_boundaries() {
    let (ec, ks) = ec_fixture();
    let p = policy_with(&[Algorithm::Es256], 10);
    // exp = NOW - 9: leeway 内で受理、NOW - 10 で拒否
    let ok = format!(r#"{{"iss":"{ISS}","aud":"api","exp":{}}}"#, NOW - 9);
    assert!(verify::<NoExtraClaims>(&ec_token(&ec, HDR, &ok), &ks, &p, NOW).is_ok());
    let ng = format!(r#"{{"iss":"{ISS}","aud":"api","exp":{}}}"#, NOW - 10);
    assert_eq!(
        verify::<NoExtraClaims>(&ec_token(&ec, HDR, &ng), &ks, &p, NOW).unwrap_err(),
        VerifyError::Expired
    );
    let nbf_ok = payload(&format!(r#","nbf":{}"#, NOW + 10));
    assert!(verify::<NoExtraClaims>(&ec_token(&ec, HDR, &nbf_ok), &ks, &p, NOW).is_ok());
    let nbf_ng = payload(&format!(r#","nbf":{}"#, NOW + 11));
    assert_eq!(
        verify::<NoExtraClaims>(&ec_token(&ec, HDR, &nbf_ng), &ks, &p, NOW).unwrap_err(),
        VerifyError::NotYetValid
    );
}

#[test]
fn key_rotation_same_kid() {
    let (old, new) = (Ec::new(), Ec::new());
    let ks = keyset(&[old.jwk("k"), new.jwk("k")]);
    assert!(run(&ec_token(&old, HDR, &payload("")), &ks).is_ok());
    assert!(run(&ec_token(&new, HDR, &payload("")), &ks).is_ok());
}

#[derive(Debug, Deserialize, PartialEq)]
struct Ext {
    tenant: String,
}

#[test]
fn extra_claims() {
    let (ec, ks) = ec_fixture();
    let t = ec_token(&ec, HDR, &payload(r#","tenant":"t1","unknown":[1]"#));
    let c = verify::<Ext>(&t, &ks, &policy(), NOW).unwrap();
    assert_eq!(c.extra().tenant, "t1");
    assert_eq!(
        c.into_extra(),
        Ext {
            tenant: "t1".into()
        }
    );
    let t = ec_token(&ec, HDR, &payload(""));
    assert_eq!(
        verify::<Ext>(&t, &ks, &policy(), NOW).unwrap_err(),
        VerifyError::InvalidExtraClaims
    );
}

#[test]
fn rejects_structure() {
    let ks = KeySet::empty();
    assert_eq!(run("", &ks).unwrap_err(), VerifyError::MissingToken);
    assert_eq!(
        run(&"a".repeat(MAX_TOKEN_LEN + 1), &ks).unwrap_err(),
        VerifyError::TooLong
    );
    for t in [
        "a.b",
        "a.b.c.d",
        "a.b.c.d.e",
        "..",
        "a..c",
        ".b.c",
        "a.b.",
        "!!.b.c",
    ] {
        assert_eq!(run(t, &ks).unwrap_err(), VerifyError::Malformed, "{t}");
    }
    // ヘッダが JSON オブジェクトでない
    let h = B.encode("[1]");
    assert_eq!(
        run(&format!("{h}.e30.e30"), &ks).unwrap_err(),
        VerifyError::Malformed
    );
    // パディング付き base64 は拒否
    let padded = format!("{}=", B.encode(HDR));
    assert_eq!(
        run(&format!("{padded}.e30.e30"), &ks).unwrap_err(),
        VerifyError::Malformed
    );
}

#[test]
fn rejects_algorithms() {
    let (ec, ks) = ec_fixture();
    for alg in ["none", "HS256", "RS384", "rs256", "ES256 "] {
        let h = format!(r#"{{"alg":"{alg}","kid":"k","typ":"at+jwt"}}"#);
        assert_eq!(
            run(&ec_token(&ec, &h, &payload("")), &ks).unwrap_err(),
            VerifyError::AlgorithmNotAllowed,
            "{alg}"
        );
    }
    let t = ec_token(&ec, HDR, &payload(""));
    let only_rs = policy_with(&[Algorithm::Rs256], 0);
    assert_eq!(
        verify::<NoExtraClaims>(&t, &ks, &only_rs, NOW).unwrap_err(),
        VerifyError::AlgorithmNotAllowed
    );
}

#[test]
fn rejects_key_selection() {
    expect(
        r#"{"alg":"ES256","typ":"at+jwt"}"#,
        &payload(""),
        VerifyError::MissingKeyId,
    );
    expect(
        r#"{"alg":"ES256","kid":"","typ":"at+jwt"}"#,
        &payload(""),
        VerifyError::MissingKeyId,
    );
    expect(
        r#"{"alg":"ES256","kid":"zz","typ":"at+jwt"}"#,
        &payload(""),
        VerifyError::UnknownKeyId,
    );
    let long = "k".repeat(257);
    expect(
        &format!(r#"{{"alg":"ES256","kid":"{long}","typ":"at+jwt"}}"#),
        &payload(""),
        VerifyError::Malformed,
    );
    let (ec, _) = ec_fixture();
    assert_eq!(
        run(&ec_token(&ec, HDR, &payload("")), &KeySet::empty()).unwrap_err(),
        VerifyError::UnknownKeyId
    );
    // kid には RS256 鍵しかないのにヘッダは ES256
    let ks = keyset(&[rsa_jwk("k")]);
    assert_eq!(
        run(&ec_token(&ec, HDR, &payload("")), &ks).unwrap_err(),
        VerifyError::KeyAlgorithmMismatch
    );
}

#[test]
fn rejects_signatures() {
    let (ec, ks) = ec_fixture();
    // 別鍵で署名
    let other = Ec::new();
    assert_eq!(
        run(&ec_token(&other, HDR, &payload("")), &ks).unwrap_err(),
        VerifyError::InvalidSignature
    );
    // 署名・payload の改ざん
    let t = ec_token(&ec, HDR, &payload(""));
    let mut parts: Vec<String> = t.split('.').map(String::from).collect();
    parts[2] = B.encode([0u8; 64]);
    assert_eq!(
        run(&parts.join("."), &ks).unwrap_err(),
        VerifyError::InvalidSignature
    );
    let mut parts: Vec<String> = t.split('.').map(String::from).collect();
    parts[1] = B.encode(payload(r#","sub":"evil""#));
    assert_eq!(
        run(&parts.join("."), &ks).unwrap_err(),
        VerifyError::InvalidSignature
    );
    // 署名長の誤り
    let mut parts: Vec<String> = t.split('.').map(String::from).collect();
    parts[2] = B.encode([1u8; 10]);
    assert_eq!(
        run(&parts.join("."), &ks).unwrap_err(),
        VerifyError::InvalidSignature
    );
}

#[test]
fn rejects_critical_header_before_alg() {
    expect(
        r#"{"alg":"HS256","kid":"k","typ":"at+jwt","crit":["x"]}"#,
        &payload(""),
        VerifyError::CriticalHeader,
    );
    expect(
        r#"{"alg":"ES256","kid":"k","typ":"at+jwt","crit":[]}"#,
        &payload(""),
        VerifyError::CriticalHeader,
    );
}

#[test]
fn rejects_type_and_claims() {
    expect(
        r#"{"alg":"ES256","kid":"k"}"#,
        &payload(""),
        VerifyError::InvalidType,
    );
    for typ in ["JWT", "logout+jwt", "id_token+jwt"] {
        let h = format!(r#"{{"alg":"ES256","kid":"k","typ":"{typ}"}}"#);
        expect(&h, &payload(""), VerifyError::InvalidType);
    }
    let e = NOW + 100;
    expect(
        HDR,
        &format!(r#"{{"iss":"https://other","aud":"api","exp":{e}}}"#),
        VerifyError::IssuerMismatch,
    );
    expect(
        HDR,
        &format!(r#"{{"iss":"HTTPS://ISSUER.EXAMPLE","aud":"api","exp":{e}}}"#),
        VerifyError::IssuerMismatch,
    );
    expect(
        HDR,
        &format!(r#"{{"iss":"{ISS}","aud":"nope","exp":{e}}}"#),
        VerifyError::AudienceMismatch,
    );
    expect(
        HDR,
        &format!(r#"{{"iss":"{ISS}","aud":["a","b"],"exp":{e}}}"#),
        VerifyError::AudienceMismatch,
    );
    expect(
        HDR,
        &format!(r#"{{"iss":"{ISS}","aud":[],"exp":{e}}}"#),
        VerifyError::AudienceMismatch,
    );
    expect(
        HDR,
        &format!(r#"{{"iss":"{ISS}","aud":"api","exp":{}}}"#, NOW - 1),
        VerifyError::Expired,
    );
    expect(
        HDR,
        &format!(r#"{{"iss":"{ISS}","aud":"api","exp":{NOW}}}"#),
        VerifyError::Expired,
    );
    expect(
        HDR,
        &payload(&format!(r#","nbf":{}"#, NOW + 1)),
        VerifyError::NotYetValid,
    );
    expect(
        HDR,
        &format!(r#"{{"aud":"api","exp":{e}}}"#),
        VerifyError::MissingClaim,
    );
    expect(
        HDR,
        &format!(r#"{{"iss":"{ISS}","exp":{e}}}"#),
        VerifyError::MissingClaim,
    );
    expect(
        HDR,
        &format!(r#"{{"iss":"{ISS}","aud":"api"}}"#),
        VerifyError::MissingClaim,
    );
}

#[test]
fn rejects_malformed_signed_payloads() {
    let e = NOW + 100;
    expect(HDR, "not json", VerifyError::Malformed);
    expect(
        HDR,
        &format!(r#"{{"iss":"{ISS}","aud":"api","exp":1.5}}"#),
        VerifyError::Malformed,
    );
    expect(
        HDR,
        &format!(r#"{{"iss":"{ISS}","aud":"api","exp":-1}}"#),
        VerifyError::Malformed,
    );
    expect(
        HDR,
        &format!(r#"{{"iss":"{ISS}","aud":[1],"exp":{e}}}"#),
        VerifyError::Malformed,
    );
    let many = vec!["\"a\""; MAX_TOKEN_AUDIENCES + 1].join(",");
    expect(
        HDR,
        &format!(r#"{{"iss":"{ISS}","aud":[{many}],"exp":{e}}}"#),
        VerifyError::Malformed,
    );
}

#[test]
fn signature_failure_wins_over_payload_problems() {
    let (_, ks) = ec_fixture();
    let other = Ec::new();
    let expired = format!(r#"{{"iss":"{ISS}","aud":"api","exp":1}}"#);
    assert_eq!(
        run(
            &ec_token(&other, r#"{"alg":"ES256","kid":"k","typ":"JWT"}"#, &expired),
            &ks
        )
        .unwrap_err(),
        VerifyError::InvalidSignature
    );
}

#[test]
fn never_panics_on_mutated_input() {
    let (ec, ks) = ec_fixture();
    let t = ec_token(&ec, HDR, &payload(""));
    let bytes = t.as_bytes();
    for i in 0..=bytes.len() {
        let cut = String::from_utf8_lossy(&bytes[..i]).into_owned();
        let _ = run(&cut, &ks);
    }
    for i in 0..bytes.len() {
        let mut m = bytes.to_vec();
        m[i] ^= 0x55;
        let s = String::from_utf8_lossy(&m).into_owned();
        assert!(run(&s, &ks).is_err() || s == t, "byte {i}");
    }
    for s in [
        "\0",
        "\u{ffff}.\u{ffff}.\u{ffff}",
        "....",
        "{}.{}.{}",
        "e30.e30.e30",
    ] {
        assert!(run(s, &ks).is_err());
    }
}

#[test]
fn errors_do_not_leak_values() {
    let all = [
        VerifyError::MissingToken,
        VerifyError::TooLong,
        VerifyError::Malformed,
        VerifyError::AlgorithmNotAllowed,
        VerifyError::MissingKeyId,
        VerifyError::UnknownKeyId,
        VerifyError::KeyAlgorithmMismatch,
        VerifyError::InvalidSignature,
        VerifyError::CriticalHeader,
        VerifyError::InvalidType,
        VerifyError::IssuerMismatch,
        VerifyError::AudienceMismatch,
        VerifyError::Expired,
        VerifyError::NotYetValid,
        VerifyError::MissingClaim,
        VerifyError::InvalidExtraClaims,
    ];
    for e in all {
        let s = format!("{e} {e:?}");
        assert!(!s.contains(ISS) && !s.contains("secret-kid"));
    }
}

#[test]
fn types_are_send_sync_clone() {
    fn check<T: Send + Sync + Clone>() {}
    check::<VerifyError>();
    check::<Claims<NoExtraClaims>>();
}

#[test]
fn claims_debug_does_not_leak_values() {
    let (ec, ks) = ec_fixture();
    let body = payload(
        r#","sub":"SECRET-SUB","jti":"SECRET-JTI","client_id":"SECRET-CID","scope":"SECRET-SCOPE","tenant":"SECRET-TENANT""#,
    );
    let c = verify::<Ext>(&ec_token(&ec, HDR, &body), &ks, &policy(), NOW).unwrap();
    let dbg = format!("{c:?} {c:#?}");
    assert!(!dbg.contains("SECRET"), "{dbg}");
    assert!(dbg.contains("has_sub: true"));
}

#[test]
fn rejects_non_object_header_and_claims() {
    // Deserialize derive は JSON 配列からもフィールド順に復元できるため、
    // オブジェクト以外は Malformed で拒否する（fail-closed）。
    expect(
        r#"["ES256","k","at+jwt"]"#,
        &payload(""),
        VerifyError::Malformed,
    );
    expect(
        HDR,
        &format!(
            r#"["{ISS}",null,"api",{},null,null,null,null,null]"#,
            NOW + 100
        ),
        VerifyError::Malformed,
    );
}
