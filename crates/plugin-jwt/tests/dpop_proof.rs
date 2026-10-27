//! `dpop::verify_proof` の公開 API だけを使った統合テスト（イシュー #901）。
//! 鍵はすべてテスト専用（ES256 は実行時生成、RS256 は `tests/fixtures`）。
#![cfg(feature = "dpop")]

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_plugin_jwt::dpop::*;
use fandhe_backend_plugin_jwt::{Algorithm, AllowedAlgorithms};
use ring::digest::{SHA256, digest};
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
    RsaPublicKeyComponents,
};

const NOW: u64 = 1_000_000;
const TOKEN: &str = "access-token-value";
const RSA_PK8: &[u8] = include_bytes!("fixtures/test-rsa-2048.pk8");

enum Signer {
    Ec(EcdsaKeyPair),
    Rsa(RsaKeyPair),
}

impl Signer {
    fn ec() -> Self {
        let rng = SystemRandom::new();
        let pk8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        Self::Ec(
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pk8.as_ref(), &rng).unwrap(),
        )
    }
    fn rsa() -> Self {
        Self::Rsa(RsaKeyPair::from_pkcs8(RSA_PK8).unwrap())
    }
    fn alg(&self) -> &'static str {
        match self {
            Self::Ec(_) => "ES256",
            Self::Rsa(_) => "RS256",
        }
    }
    fn jwk(&self) -> String {
        match self {
            Self::Ec(kp) => {
                let pt = kp.public_key().as_ref();
                format!(
                    r#"{{"kty":"EC","crv":"P-256","x":"{}","y":"{}"}}"#,
                    B.encode(&pt[1..33]),
                    B.encode(&pt[33..65])
                )
            }
            Self::Rsa(kp) => {
                let c = RsaPublicKeyComponents::<Vec<u8>>::from(kp.public());
                format!(
                    r#"{{"kty":"RSA","n":"{}","e":"{}"}}"#,
                    B.encode(&c.n),
                    B.encode(&c.e)
                )
            }
        }
    }
    fn sign(&self, msg: &str) -> Vec<u8> {
        match self {
            Self::Ec(kp) => kp
                .sign(&SystemRandom::new(), msg.as_bytes())
                .unwrap()
                .as_ref()
                .to_vec(),
            Self::Rsa(kp) => {
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
        }
    }
}

fn ath(token: &str) -> String {
    B.encode(digest(&SHA256, token.as_bytes()).as_ref())
}

fn header(s: &Signer, typ: &str, jwk: &str) -> String {
    format!(r#"{{"typ":"{typ}","alg":"{}","jwk":{jwk}}}"#, s.alg())
}

fn payload(htm: &str, htu: &str, iat: &str, ath: &str) -> String {
    format!(r#"{{"jti":"jti-1","htm":"{htm}","htu":"{htu}","iat":{iat},"ath":"{ath}"}}"#)
}

fn good_payload() -> String {
    payload(
        "GET",
        "https://api.example/resource",
        &NOW.to_string(),
        &ath(TOKEN),
    )
}

fn make(s: &Signer, header_json: &str, payload_json: &str) -> String {
    let msg = format!("{}.{}", B.encode(header_json), B.encode(payload_json));
    let sig = s.sign(&msg);
    format!("{msg}.{}", B.encode(sig))
}

fn run(proof: &str, algs: &[Algorithm]) -> Result<VerifiedProof, DpopError> {
    let htu = PublicOrigin::new("https://api.example")
        .unwrap()
        .expected_htu("/resource")
        .unwrap();
    let algs = AllowedAlgorithms::new(algs.iter().copied()).unwrap();
    let ctx = ProofContext::new("GET", &htu, TOKEN, &algs, IatWindow::default());
    verify_proof(proof, &ctx, NOW)
}

fn run_both(proof: &str) -> Result<VerifiedProof, DpopError> {
    run(proof, &[Algorithm::Es256, Algorithm::Rs256])
}

fn good(s: &Signer) -> String {
    make(s, &header(s, "dpop+jwt", &s.jwk()), &good_payload())
}

#[test]
fn accepts_valid_es256_and_rs256_proofs() {
    for s in [Signer::ec(), Signer::rsa()] {
        let v = run_both(&good(&s)).unwrap();
        assert_eq!(v.jti(), "jti-1");
        assert_eq!(v.iat(), NOW);
        assert_eq!(v.thumbprint(), &jwk_thumbprint(&s.jwk()).unwrap());
    }
}

#[test]
fn accepts_typ_and_htu_variants() {
    let s = Signer::ec();
    for typ in ["application/dpop+jwt", "DPoP+JWT"] {
        make(&s, &header(&s, typ, &s.jwk()), &good_payload());
        assert!(run_both(&make(&s, &header(&s, typ, &s.jwk()), &good_payload())).is_ok());
    }
    for htu in [
        "https://api.example/resource?x=1#frag",
        "HTTPS://API.Example:443/resource",
        "https://api.example:443/resource",
    ] {
        let p = payload("GET", htu, &NOW.to_string(), &ath(TOKEN));
        assert!(
            run_both(&make(&s, &header(&s, "dpop+jwt", &s.jwk()), &p)).is_ok(),
            "{htu}"
        );
    }
}

#[test]
fn rejects_bad_typ_alg_and_crit() {
    let s = Signer::ec();
    let p = good_payload();
    for typ in ["JWT", "at+jwt", ""] {
        let h = header(&s, typ, &s.jwk());
        assert_eq!(
            run_both(&make(&s, &h, &p)),
            Err(DpopError::InvalidType),
            "{typ}"
        );
    }
    let no_typ = format!(r#"{{"alg":"ES256","jwk":{}}}"#, s.jwk());
    assert_eq!(
        run_both(&make(&s, &no_typ, &p)),
        Err(DpopError::InvalidType)
    );
    // 許可リスト外・none・HS256。
    assert_eq!(
        run(&good(&Signer::rsa()), &[Algorithm::Es256]),
        Err(DpopError::AlgorithmNotAllowed)
    );
    for alg in ["none", "HS256"] {
        let h = format!(r#"{{"typ":"dpop+jwt","alg":"{alg}","jwk":{}}}"#, s.jwk());
        assert_eq!(
            run_both(&make(&s, &h, &p)),
            Err(DpopError::AlgorithmNotAllowed)
        );
    }
    let h = format!(
        r#"{{"typ":"dpop+jwt","alg":"ES256","crit":["x"],"jwk":{}}}"#,
        s.jwk()
    );
    assert_eq!(run_both(&make(&s, &h, &p)), Err(DpopError::CriticalHeader));
}

#[test]
fn rejects_bad_jwk() {
    let s = Signer::ec();
    let r = Signer::rsa();
    let p = good_payload();
    let ec = s.jwk();
    let with = |extra: &str| ec.replacen('}', &format!(",{extra}}}"), 1);
    let no_jwk = r#"{"typ":"dpop+jwt","alg":"ES256"}"#;
    assert_eq!(run_both(&make(&s, no_jwk, &p)), Err(DpopError::MissingJwk));
    let cases: Vec<(String, DpopError)> = vec![
        (with(r#""d":"AA""#), DpopError::PrivateKeyMaterial),
        (
            r.jwk().replacen('}', r#","d":"AA","p":"AA"}"#, 1),
            DpopError::PrivateKeyMaterial,
        ),
        (
            r#"{"kty":"oct","k":"AA"}"#.into(),
            DpopError::PrivateKeyMaterial,
        ),
        (r#"{"kty":"oct"}"#.into(), DpopError::UnsupportedKey),
        (r.jwk(), DpopError::KeyAlgorithmMismatch),
        (with(r#""alg":"RS256""#), DpopError::KeyAlgorithmMismatch),
        (with(r#""use":"enc""#), DpopError::UnsupportedKey),
        (ec.replace("P-256", "P-384"), DpopError::UnsupportedKey),
        (
            format!(
                r#"{{"kty":"EC","crv":"P-256","x":"{}","y":"{}"}}"#,
                B.encode([1u8; 31]),
                B.encode([1u8; 32])
            ),
            DpopError::InvalidKey,
        ),
        ("[]".into(), DpopError::Malformed),
        (
            ec.replacen("\"kty\":\"EC\"", "\"kty\":\"EC\",\"kty\":\"EC\"", 1),
            DpopError::Malformed,
        ),
    ];
    for (jwk, want) in cases {
        let h = header(&s, "dpop+jwt", &jwk);
        assert_eq!(run_both(&make(&s, &h, &p)), Err(want), "{jwk}");
    }
    // RSA: ES256 のヘッダ + RSA の jwk（上で確認済み）の逆、短い n、先頭 0 の n。
    let rh = |jwk: &str| format!(r#"{{"typ":"dpop+jwt","alg":"RS256","jwk":{jwk}}}"#);
    assert_eq!(
        run_both(&make(&r, &rh(&s.jwk()), &p)),
        Err(DpopError::KeyAlgorithmMismatch)
    );
    let short = format!(
        r#"{{"kty":"RSA","n":"{}","e":"AQAB"}}"#,
        B.encode([7u8; 128])
    );
    assert_eq!(
        run_both(&make(&r, &rh(&short), &p)),
        Err(DpopError::InvalidKey)
    );
    let mut n = vec![0u8];
    n.extend([7u8; 256]);
    let lead = format!(r#"{{"kty":"RSA","n":"{}","e":"AQAB"}}"#, B.encode(&n));
    assert_eq!(
        run_both(&make(&r, &rh(&lead), &p)),
        Err(DpopError::InvalidKey)
    );
}

#[test]
fn rejects_bad_signature() {
    let s = Signer::ec();
    let other = Signer::ec();
    let good_proof = good(&s);
    // payload 改ざん。
    let parts: Vec<&str> = good_proof.split('.').collect();
    let forged = payload(
        "GET",
        "https://api.example/resource",
        &NOW.to_string(),
        &ath("x"),
    );
    let tampered = format!("{}.{}.{}", parts[0], B.encode(forged), parts[2]);
    assert_eq!(run_both(&tampered), Err(DpopError::InvalidSignature));
    // jwk とは別の鍵で署名。
    let h = header(&s, "dpop+jwt", &s.jwk());
    let msg = format!("{}.{}", B.encode(&h), B.encode(good_payload()));
    let bad = format!("{msg}.{}", B.encode(other.sign(&msg)));
    assert_eq!(run_both(&bad), Err(DpopError::InvalidSignature));
    // 署名区画の不正。
    assert_eq!(run_both(&format!("{msg}.!!!")), Err(DpopError::Malformed));
    assert_eq!(run_both(&format!("{msg}.")), Err(DpopError::Malformed));
}

#[test]
fn rejects_claim_mismatches() {
    let s = Signer::ec();
    let h = header(&s, "dpop+jwt", &s.jwk());
    let it = NOW.to_string();
    let a = ath(TOKEN);
    let cases = [
        (
            payload("GET", "https://api.example/resource", &it, &ath("other")),
            DpopError::AccessTokenHashMismatch,
        ),
        (
            payload("get", "https://api.example/resource", &it, &a),
            DpopError::MethodMismatch,
        ),
        (
            payload("POST", "https://api.example/resource", &it, &a),
            DpopError::MethodMismatch,
        ),
        (
            payload("GET", "https://api.example/other", &it, &a),
            DpopError::UriMismatch,
        ),
        (
            payload("GET", "https://evil.example/resource", &it, &a),
            DpopError::UriMismatch,
        ),
        (
            payload("GET", "http://api.example/resource", &it, &a),
            DpopError::UriMismatch,
        ),
        (
            payload("GET", "https://api.example:8443/resource", &it, &a),
            DpopError::UriMismatch,
        ),
        (
            payload("GET", "https://u@api.example/resource", &it, &a),
            DpopError::UriMismatch,
        ),
        (
            payload("GET", "https://api.example/a/../resource", &it, &a),
            DpopError::UriMismatch,
        ),
        (
            payload("GET", "https://api.example/%2e%2e/resource", &it, &a),
            DpopError::UriMismatch,
        ),
        (
            payload("GET", "https://api.example/res%zzource", &it, &a),
            DpopError::UriMismatch,
        ),
        (payload("GET", "/resource", &it, &a), DpopError::UriMismatch),
        (
            format!(r#"{{"jti":"j","htm":"GET","htu":"https://api.example/resource","iat":{it}}}"#),
            DpopError::MissingClaim,
        ),
        (
            format!(
                r#"{{"jti":"","htm":"GET","htu":"https://api.example/resource","iat":{it},"ath":"{a}"}}"#
            ),
            DpopError::InvalidJti,
        ),
        (
            format!(
                r#"{{"jti":"{}","htm":"GET","htu":"https://api.example/resource","iat":{it},"ath":"{a}"}}"#,
                "j".repeat(257)
            ),
            DpopError::InvalidJti,
        ),
    ];
    for (p, want) in cases {
        assert_eq!(run_both(&make(&s, &h, &p)), Err(want), "{p}");
    }
    // ath の比較は大文字小文字を区別する。
    let upper = payload(
        "GET",
        "https://api.example/resource",
        &it,
        &a.to_uppercase(),
    );
    assert!(run_both(&make(&s, &h, &upper)).is_err());
}

#[test]
fn iat_window_boundaries() {
    let s = Signer::ec();
    let h = header(&s, "dpop+jwt", &s.jwk());
    let go = |iat: &str| {
        let p = payload("GET", "https://api.example/resource", iat, &ath(TOKEN));
        run_both(&make(&s, &h, &p))
    };
    assert!(go(&(NOW - 60).to_string()).is_ok());
    assert!(go(&(NOW + 5).to_string()).is_ok());
    assert_eq!(
        go(&(NOW - 61).to_string()),
        Err(DpopError::IssuedAtOutOfWindow)
    );
    assert_eq!(
        go(&(NOW + 6).to_string()),
        Err(DpopError::IssuedAtOutOfWindow)
    );
    for bad in ["1.5", "-1", "\"1000000\"", "null"] {
        let r = go(bad);
        assert!(
            matches!(r, Err(DpopError::Malformed) | Err(DpopError::MissingClaim)),
            "{bad}: {r:?}"
        );
    }
}

#[test]
fn rejects_bad_structure() {
    assert_eq!(run_both(""), Err(DpopError::MissingProof));
    assert_eq!(
        run_both(&"a".repeat(MAX_PROOF_LEN + 1)),
        Err(DpopError::TooLong)
    );
    for bad in ["a.b", "a.b.c.d", "a..c", ".b.c", "!!!.b.c"] {
        assert_eq!(run_both(bad), Err(DpopError::Malformed), "{bad}");
    }
    // payload が配列（署名は有効）。
    let s = Signer::ec();
    let h = header(&s, "dpop+jwt", &s.jwk());
    assert_eq!(run_both(&make(&s, &h, "[]")), Err(DpopError::Malformed));
    // ヘッダが配列。
    assert_eq!(
        run_both(&make(&s, "[]", &good_payload())),
        Err(DpopError::Malformed)
    );
}

#[test]
fn rfc7638_thumbprint_vector() {
    let n = "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw";
    let jwk = format!(r#"{{"e":"AQAB","kty":"RSA","kid":"2011-04-29","n":"{n}"}}"#);
    assert_eq!(
        jwk_thumbprint(&jwk).unwrap().to_string(),
        "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs"
    );
}

#[test]
fn config_types() {
    assert_eq!(
        PublicOrigin::new("HTTPS://API.Example:443/")
            .unwrap()
            .as_str(),
        "https://api.example"
    );
    assert_eq!(
        PublicOrigin::new("[::1]").unwrap_err(),
        OriginError::InvalidScheme
    );
    let o = PublicOrigin::new("https://api.example").unwrap();
    for bad in ["*", "http://api.example/x", "/a?x", "/a#f", "x", "/a/../b"] {
        assert_eq!(
            o.expected_htu(bad),
            Err(DpopError::InvalidRequestTarget),
            "{bad}"
        );
    }
    let secs = Duration::from_secs;
    assert_eq!(
        IatWindow::new(secs(0), secs(0)),
        Err(DpopConfigError::IatPastZero)
    );
    assert_eq!(
        IatWindow::new(secs(301), secs(0)),
        Err(DpopConfigError::IatPastTooLarge)
    );
    assert_eq!(
        IatWindow::new(secs(1), secs(61)),
        Err(DpopConfigError::IatFutureTooLarge)
    );
    assert!(IatWindow::new(secs(300), secs(60)).is_ok());
}

#[test]
fn errors_and_debug_do_not_leak() {
    let s = Signer::ec();
    let v = run_both(&good(&s)).unwrap();
    assert!(!format!("{v:?}").contains("jti-1"));
    let htu = PublicOrigin::new("https://api.example")
        .unwrap()
        .expected_htu("/")
        .unwrap();
    let algs = AllowedAlgorithms::only(Algorithm::Es256);
    let ctx = ProofContext::new("GET", &htu, TOKEN, &algs, IatWindow::default());
    assert!(!format!("{ctx:?}").contains(TOKEN));
    assert_eq!(
        DpopError::UriMismatch.to_string(),
        "DPoP proof の htu が期待 URI と一致しません"
    );
}
