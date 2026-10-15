//! REQ-16 受け入れ基準 1・2 の網羅受け入れテスト（イシュー #817）。
//!
//! 公開 API（`verify` / `JwtAuthenticator` / `SharedKeySet`）だけを使い、鍵はすべて
//! `tests/fixtures` のテスト専用鍵（公開物へ同梱可、README 参照）。受け入れレポート
//! （#823）から次の対応表で引用する。
//!
//! | 基準 | 項目 | テスト |
//! |---|---|---|
//! | 1 | `iss` 不一致 | `c1_rejects_iss_mismatch` |
//! | 1 | `aud` 不一致 | `c1_rejects_aud_mismatch` |
//! | 1 | `typ` 不一致（ID トークン相当・`logout+jwt` 含む） | `c1_rejects_typ_mismatch_and_missing` |
//! | 1 | `nbf` 前 | `c1_rejects_before_nbf` |
//! | 1 | `exp` 後 | `c1_rejects_after_exp` |
//! | 1 | 知らない `kid` | `c1_rejects_unknown_kid` |
//! | 1 | 許可リスト外の `alg` | `c1_rejects_alg_outside_allowlist` |
//! | 1 | 鍵種別と `alg` の不一致 | `c1_rejects_key_kind_alg_mismatch` |
//! | 1 | `alg: none` | `c1_rejects_alg_none` |
//! | 1 | HS 系 | `c1_rejects_hs_family` |
//! | 1 | 署名形式の誤用（DER の ECDSA） | `c1_rejects_der_encoded_ecdsa_signature` |
//! | 2 | RS256 正常 | `c2_accepts_rs256_with_fixture_key` |
//! | 2 | ES256 正常 | `c2_accepts_es256_with_fixture_key` |
//! | 2 | RS・ES 混在 JWKS | `c2_accepts_mixed_rs_es_keyset` |
//! | 2 | 鍵回転（同一 kid） | `c2_rotation_same_kid_rs256` / `c2_rotation_same_kid_es256` |
//! | 2 | 鍵回転（別 kid） | `c2_rotation_distinct_kids` |
//! | 2 | 鍵回転のライフサイクル | `c2_rotation_lifecycle_via_shared_key_set` |
//!
//! 各拒否テストは「その 1 点だけを壊した」トークンで行い、壊す前の基準トークンが
//! 受理されること（`baseline_is_accepted`）で拒否原因を特定可能にする。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_plugin_jwt::*;
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
    RsaPublicKeyComponents,
};

const ISS: &str = "https://issuer.example";
const NOW: u64 = 1_000_000;

const RSA_OLD: &[u8] = include_bytes!("fixtures/test-rsa-2048.pk8");
const RSA_NEW: &[u8] = include_bytes!("fixtures/test-rsa-2048-rotated.pk8");
const EC_OLD: &[u8] = include_bytes!("fixtures/test-ec-p256.pk8");
const EC_NEW: &[u8] = include_bytes!("fixtures/test-ec-p256-rotated.pk8");

fn rsa(pk8: &[u8]) -> RsaKeyPair {
    RsaKeyPair::from_pkcs8(pk8).unwrap()
}

fn ec(pk8: &[u8]) -> EcdsaKeyPair {
    EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pk8, &SystemRandom::new()).unwrap()
}

fn rsa_n(kp: &RsaKeyPair) -> Vec<u8> {
    RsaPublicKeyComponents::<Vec<u8>>::from(kp.public()).n
}

fn rsa_jwk(kp: &RsaKeyPair, kid: &str) -> String {
    let c = RsaPublicKeyComponents::<Vec<u8>>::from(kp.public());
    format!(
        r#"{{"kty":"RSA","kid":"{kid}","alg":"RS256","n":"{}","e":"{}"}}"#,
        B.encode(&c.n),
        B.encode(&c.e)
    )
}

fn ec_jwk(kp: &EcdsaKeyPair, kid: &str) -> String {
    let pt = kp.public_key().as_ref();
    format!(
        r#"{{"kty":"EC","kid":"{kid}","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}"#,
        B.encode(&pt[1..33]),
        B.encode(&pt[33..65])
    )
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

fn both() -> JwtPolicy {
    policy_with(&[Algorithm::Rs256, Algorithm::Es256], 0)
}

fn run_with(token: &str, keys: &KeySet, p: &JwtPolicy) -> Result<Claims, VerifyError> {
    verify::<NoExtraClaims>(token, keys, p, NOW)
}

fn run(token: &str, keys: &KeySet) -> Result<Claims, VerifyError> {
    run_with(token, keys, &both())
}

fn hdr(alg: &str, kid: &str, typ: Option<&str>) -> String {
    match typ {
        Some(t) => format!(r#"{{"alg":"{alg}","kid":"{kid}","typ":"{t}"}}"#),
        None => format!(r#"{{"alg":"{alg}","kid":"{kid}"}}"#),
    }
}

fn body(extra: &str) -> String {
    format!(
        r#"{{"iss":"{ISS}","aud":"api","exp":{}{extra}}}"#,
        NOW + 100
    )
}

fn sign_rs(kp: &RsaKeyPair, header: &str, payload: &str) -> String {
    let msg = format!("{}.{}", B.encode(header), B.encode(payload));
    let mut sig = vec![0u8; kp.public().modulus_len()];
    kp.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        msg.as_bytes(),
        &mut sig,
    )
    .unwrap();
    format!("{msg}.{}", B.encode(sig))
}

fn sign_es(kp: &EcdsaKeyPair, header: &str, payload: &str) -> String {
    let msg = format!("{}.{}", B.encode(header), B.encode(payload));
    let sig = kp.sign(&SystemRandom::new(), msg.as_bytes()).unwrap();
    format!("{msg}.{}", B.encode(sig.as_ref()))
}

/// ES256 の基準鍵（kid=`k`）と鍵集合。
fn es_fixture() -> (EcdsaKeyPair, KeySet) {
    let kp = ec(EC_OLD);
    let ks = keyset(&[ec_jwk(&kp, "k")]);
    (kp, ks)
}

fn es_expect(header: &str, payload: &str, want: VerifyError) {
    let (kp, ks) = es_fixture();
    assert_eq!(run(&sign_es(&kp, header, payload), &ks).unwrap_err(), want);
}

#[test]
fn baseline_is_accepted() {
    let (kp, ks) = es_fixture();
    let t = sign_es(&kp, &hdr("ES256", "k", Some("at+jwt")), &body(""));
    assert_eq!(run(&t, &ks).unwrap().iss(), ISS);
}

#[test]
fn c1_rejects_iss_mismatch() {
    for iss in [
        "https://other.example",
        "HTTPS://ISSUER.EXAMPLE",
        "https://issuer.example/",
    ] {
        es_expect(
            &hdr("ES256", "k", Some("at+jwt")),
            &format!(r#"{{"iss":"{iss}","aud":"api","exp":{}}}"#, NOW + 100),
            VerifyError::IssuerMismatch,
        );
    }
}

#[test]
fn c1_rejects_aud_mismatch() {
    for aud in [r#""other""#, r#"["a","b"]"#, "[]"] {
        es_expect(
            &hdr("ES256", "k", Some("at+jwt")),
            &format!(r#"{{"iss":"{ISS}","aud":{aud},"exp":{}}}"#, NOW + 100),
            VerifyError::AudienceMismatch,
        );
    }
}

#[test]
fn c1_rejects_typ_mismatch_and_missing() {
    es_expect(
        &hdr("ES256", "k", None),
        &body(""),
        VerifyError::InvalidType,
    );
    for typ in [
        "JWT",
        "id_token+jwt",
        "logout+jwt",
        "application/logout+jwt",
        "at+jwt; x",
        "",
    ] {
        es_expect(
            &hdr("ES256", "k", Some(typ)),
            &body(""),
            VerifyError::InvalidType,
        );
    }
}

#[test]
fn c1_rejects_before_nbf() {
    let h = hdr("ES256", "k", Some("at+jwt"));
    es_expect(
        &h,
        &body(&format!(r#","nbf":{}"#, NOW + 1)),
        VerifyError::NotYetValid,
    );
    let (kp, ks) = es_fixture();
    let p = policy_with(&[Algorithm::Es256], 10);
    let t = sign_es(&kp, &h, &body(&format!(r#","nbf":{}"#, NOW + 11)));
    assert_eq!(run_with(&t, &ks, &p).unwrap_err(), VerifyError::NotYetValid);
}

#[test]
fn c1_rejects_after_exp() {
    let h = hdr("ES256", "k", Some("at+jwt"));
    for exp in [NOW, NOW - 1] {
        es_expect(
            &h,
            &format!(r#"{{"iss":"{ISS}","aud":"api","exp":{exp}}}"#),
            VerifyError::Expired,
        );
    }
    let (kp, ks) = es_fixture();
    let p = policy_with(&[Algorithm::Es256], 10);
    let t = sign_es(
        &kp,
        &h,
        &format!(r#"{{"iss":"{ISS}","aud":"api","exp":{}}}"#, NOW - 10),
    );
    assert_eq!(run_with(&t, &ks, &p).unwrap_err(), VerifyError::Expired);
}

#[test]
fn c1_rejects_unknown_kid() {
    es_expect(
        &hdr("ES256", "unknown", Some("at+jwt")),
        &body(""),
        VerifyError::UnknownKeyId,
    );
    let (kp, _) = es_fixture();
    let t = sign_es(&kp, &hdr("ES256", "k", Some("at+jwt")), &body(""));
    assert_eq!(
        run(&t, &KeySet::empty()).unwrap_err(),
        VerifyError::UnknownKeyId
    );
}

#[test]
fn c1_rejects_alg_outside_allowlist() {
    let (kp, ks) = es_fixture();
    for alg in [
        "RS384", "PS256", "ES384", "EdDSA", "rs256", "es256", "ES256 ",
    ] {
        let t = sign_es(&kp, &hdr(alg, "k", Some("at+jwt")), &body(""));
        assert_eq!(
            run(&t, &ks).unwrap_err(),
            VerifyError::AlgorithmNotAllowed,
            "{alg}"
        );
    }
    // 署名自体は有効な ES256 を RS256 のみのポリシーで受ける
    let t = sign_es(&kp, &hdr("ES256", "k", Some("at+jwt")), &body(""));
    let only_rs = policy_with(&[Algorithm::Rs256], 0);
    assert_eq!(
        run_with(&t, &ks, &only_rs).unwrap_err(),
        VerifyError::AlgorithmNotAllowed
    );
    // 逆方向: 有効な RS256 を ES256 のみのポリシーで受ける
    let r = rsa(RSA_OLD);
    let rks = keyset(&[rsa_jwk(&r, "r")]);
    let t = sign_rs(&r, &hdr("RS256", "r", Some("at+jwt")), &body(""));
    assert!(run(&t, &rks).is_ok());
    let only_es = policy_with(&[Algorithm::Es256], 0);
    assert_eq!(
        run_with(&t, &rks, &only_es).unwrap_err(),
        VerifyError::AlgorithmNotAllowed
    );
}

#[test]
fn c1_rejects_key_kind_alg_mismatch() {
    // (i) kid には RSA 鍵しかないがヘッダは ES256
    let r = rsa(RSA_OLD);
    let k = ec(EC_OLD);
    let t = sign_es(&k, &hdr("ES256", "k", Some("at+jwt")), &body(""));
    assert_eq!(
        run(&t, &keyset(&[rsa_jwk(&r, "k")])).unwrap_err(),
        VerifyError::KeyAlgorithmMismatch
    );
    // (ii) kid には EC 鍵しかないがヘッダは RS256（ポリシーは両方許可）
    let t = sign_rs(&r, &hdr("RS256", "k", Some("at+jwt")), &body(""));
    assert_eq!(
        run(&t, &keyset(&[ec_jwk(&k, "k")])).unwrap_err(),
        VerifyError::KeyAlgorithmMismatch
    );
    // (iii) JWKS 構築時点で kty と alg の矛盾を拒否する
    let c = RsaPublicKeyComponents::<Vec<u8>>::from(r.public());
    let bad_rsa = format!(
        r#"{{"keys":[{{"kty":"RSA","kid":"x","alg":"ES256","n":"{}","e":"{}"}}]}}"#,
        B.encode(&c.n),
        B.encode(&c.e)
    );
    assert!(KeySet::from_json(&bad_rsa).is_err());
    let pt = k.public_key().as_ref();
    let bad_ec = format!(
        r#"{{"keys":[{{"kty":"EC","kid":"x","alg":"RS256","crv":"P-256","x":"{}","y":"{}"}}]}}"#,
        B.encode(&pt[1..33]),
        B.encode(&pt[33..65])
    );
    assert!(KeySet::from_json(&bad_ec).is_err());
}

#[test]
fn c1_rejects_alg_none() {
    let (_, ks) = es_fixture();
    for alg in ["none", "None", "NONE"] {
        let h = B.encode(hdr(alg, "k", Some("at+jwt")));
        let p = B.encode(body(""));
        // 署名区画が空: 構造不正
        assert_eq!(
            run(&format!("{h}.{p}."), &ks).unwrap_err(),
            VerifyError::Malformed,
            "{alg} 空署名"
        );
        // ダミー署名つき: alg 許可リストで拒否
        assert_eq!(
            run(&format!("{h}.{p}.{}", B.encode([0u8; 64])), &ks).unwrap_err(),
            VerifyError::AlgorithmNotAllowed,
            "{alg} ダミー署名"
        );
    }
}

fn hmac_token(alg: &str, algo: ring::hmac::Algorithm, secret: &[u8]) -> String {
    let msg = format!(
        "{}.{}",
        B.encode(hdr(alg, "k", Some("at+jwt"))),
        B.encode(body(""))
    );
    let key = ring::hmac::Key::new(algo, secret);
    let tag = ring::hmac::sign(&key, msg.as_bytes());
    format!("{msg}.{}", B.encode(tag.as_ref()))
}

#[test]
fn c1_rejects_hs_family() {
    let (_, ks) = es_fixture();
    for (alg, algo) in [
        ("HS256", ring::hmac::HMAC_SHA256),
        ("HS384", ring::hmac::HMAC_SHA384),
        ("HS512", ring::hmac::HMAC_SHA512),
    ] {
        let t = hmac_token(alg, algo, b"shared-secret");
        assert_eq!(
            run(&t, &ks).unwrap_err(),
            VerifyError::AlgorithmNotAllowed,
            "{alg}"
        );
    }
    // 鍵混同攻撃の形: RSA 公開鍵のバイト列を HMAC 秘密にした HS256
    let r = rsa(RSA_OLD);
    let rks = keyset(&[rsa_jwk(&r, "k")]);
    let t = hmac_token("HS256", ring::hmac::HMAC_SHA256, &rsa_n(&r));
    assert_eq!(run(&t, &rks).unwrap_err(), VerifyError::AlgorithmNotAllowed);
}

/// 固定長 R||S（64 バイト）を DER `SEQUENCE{INTEGER r, INTEGER s}` へ変換する。
fn rs_to_der(sig: &[u8]) -> Vec<u8> {
    fn int(v: &[u8]) -> Vec<u8> {
        let mut v: Vec<u8> = v.iter().copied().skip_while(|b| *b == 0).collect();
        if v.first().is_none_or(|b| b & 0x80 != 0) {
            v.insert(0, 0);
        }
        let mut out = vec![0x02, v.len() as u8];
        out.extend(v);
        out
    }
    let (r, s) = sig.split_at(sig.len() / 2);
    let body: Vec<u8> = int(r).into_iter().chain(int(s)).collect();
    let mut out = vec![0x30, body.len() as u8];
    out.extend(body);
    out
}

#[test]
fn c1_rejects_der_encoded_ecdsa_signature() {
    let (kp, ks) = es_fixture();
    let header = hdr("ES256", "k", Some("at+jwt"));
    let payload = body(r#","sub":"u""#);
    // 同じ鍵・同じメッセージの有効な R||S は受理される（対照）。
    let valid = sign_es(&kp, &header, &payload);
    assert!(run(&valid, &ks).is_ok());
    // 同一署名を DER に変換しただけで拒否される（JWS は R||S 固定長が正）。
    let (msg, sig_b64) = valid.rsplit_once('.').unwrap();
    let raw = B.decode(sig_b64).unwrap();
    assert_eq!(raw.len(), 64);
    let t = format!("{msg}.{}", B.encode(rs_to_der(&raw)));
    assert_eq!(run(&t, &ks).unwrap_err(), VerifyError::InvalidSignature);
}

#[test]
fn c2_accepts_rs256_with_fixture_key() {
    let r = rsa(RSA_OLD);
    let ks = keyset(&[rsa_jwk(&r, "r")]);
    let t = sign_rs(
        &r,
        &hdr("RS256", "r", Some("at+jwt")),
        &body(r#","sub":"u""#),
    );
    let c = run(&t, &ks).unwrap();
    assert_eq!((c.iss(), c.sub(), c.exp()), (ISS, Some("u"), NOW + 100));
    assert_eq!(c.aud(), ["api".to_string()]);
}

#[test]
fn c2_accepts_es256_with_fixture_key() {
    let (kp, ks) = es_fixture();
    let t = sign_es(
        &kp,
        &hdr("ES256", "k", Some("at+jwt")),
        &body(r#","sub":"u""#),
    );
    let c = run(&t, &ks).unwrap();
    assert_eq!((c.iss(), c.sub(), c.exp()), (ISS, Some("u"), NOW + 100));
    assert_eq!(c.aud(), ["api".to_string()]);
}

#[test]
fn c2_accepts_mixed_rs_es_keyset() {
    let (r, e) = (rsa(RSA_OLD), ec(EC_OLD));
    let ks = keyset(&[rsa_jwk(&r, "r"), ec_jwk(&e, "e")]);
    let tr = sign_rs(&r, &hdr("RS256", "r", Some("at+jwt")), &body(""));
    let te = sign_es(&e, &hdr("ES256", "e", Some("at+jwt")), &body(""));
    assert!(run(&tr, &ks).is_ok());
    assert!(run(&te, &ks).is_ok());
}

#[test]
fn c2_rotation_same_kid_rs256() {
    let (old, new) = (rsa(RSA_OLD), rsa(RSA_NEW));
    let ks = keyset(&[rsa_jwk(&old, "k"), rsa_jwk(&new, "k")]);
    let h = hdr("RS256", "k", Some("at+jwt"));
    assert!(run(&sign_rs(&old, &h, &body("")), &ks).is_ok());
    assert!(run(&sign_rs(&new, &h, &body("")), &ks).is_ok());
}

#[test]
fn c2_rotation_same_kid_es256() {
    let (old, new) = (ec(EC_OLD), ec(EC_NEW));
    let ks = keyset(&[ec_jwk(&old, "k"), ec_jwk(&new, "k")]);
    let h = hdr("ES256", "k", Some("at+jwt"));
    assert!(run(&sign_es(&old, &h, &body("")), &ks).is_ok());
    assert!(run(&sign_es(&new, &h, &body("")), &ks).is_ok());
}

#[test]
fn c2_rotation_distinct_kids() {
    let (ro, rn) = (rsa(RSA_OLD), rsa(RSA_NEW));
    let (eo, en) = (ec(EC_OLD), ec(EC_NEW));
    let ks = keyset(&[
        rsa_jwk(&ro, "r-old"),
        rsa_jwk(&rn, "r-new"),
        ec_jwk(&eo, "e-old"),
        ec_jwk(&en, "e-new"),
    ]);
    for (kp, kid) in [(&ro, "r-old"), (&rn, "r-new")] {
        let t = sign_rs(kp, &hdr("RS256", kid, Some("at+jwt")), &body(""));
        assert!(run(&t, &ks).is_ok(), "{kid}");
    }
    for (kp, kid) in [(&eo, "e-old"), (&en, "e-new")] {
        let t = sign_es(kp, &hdr("ES256", kid, Some("at+jwt")), &body(""));
        assert!(run(&t, &ks).is_ok(), "{kid}");
    }
}

fn head(token: &str) -> RequestHead {
    let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n");
    match parse_request_head(raw.as_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    }
}

#[test]
fn c2_rotation_lifecycle_via_shared_key_set() {
    let (old, new) = (ec(EC_OLD), ec(EC_NEW));
    let h_old = head(&sign_es(
        &old,
        &hdr("ES256", "old", Some("at+jwt")),
        &body(""),
    ));
    let h_new = head(&sign_es(
        &new,
        &hdr("ES256", "new", Some("at+jwt")),
        &body(""),
    ));
    let shared = SharedKeySet::new(keyset(&[ec_jwk(&old, "old")]));
    let a: JwtAuthenticator =
        JwtAuthenticator::builder(shared.clone(), policy_with(&[Algorithm::Es256], 0)).build();

    // v1 = {旧}: 旧のみ受理
    assert!(a.authenticate_at(&h_old, NOW).is_ok());
    assert_eq!(
        a.authenticate_at(&h_new, NOW).unwrap_err(),
        VerifyError::UnknownKeyId
    );
    // v2 = {旧, 新}: 回転中は新旧どちらも受理
    shared.set(keyset(&[ec_jwk(&old, "old"), ec_jwk(&new, "new")]));
    assert!(a.authenticate_at(&h_old, NOW).is_ok());
    assert!(a.authenticate_at(&h_new, NOW).is_ok());
    // v3 = {新}: 旧は失効（キャッシュ済みでも通らない）、新は受理
    shared.set(keyset(&[ec_jwk(&new, "new")]));
    assert_eq!(
        a.authenticate_at(&h_old, NOW).unwrap_err(),
        VerifyError::UnknownKeyId
    );
    assert!(a.authenticate_at(&h_new, NOW).is_ok());

    // 同一 kid での回転: 旧鍵を外すと旧トークンは署名不一致になる
    let h_same_old = head(&sign_es(
        &old,
        &hdr("ES256", "k", Some("at+jwt")),
        &body(""),
    ));
    let h_same_new = head(&sign_es(
        &new,
        &hdr("ES256", "k", Some("at+jwt")),
        &body(""),
    ));
    shared.set(keyset(&[ec_jwk(&old, "k"), ec_jwk(&new, "k")]));
    assert!(a.authenticate_at(&h_same_old, NOW).is_ok());
    assert!(a.authenticate_at(&h_same_new, NOW).is_ok());
    shared.set(keyset(&[ec_jwk(&new, "k")]));
    assert_eq!(
        a.authenticate_at(&h_same_old, NOW).unwrap_err(),
        VerifyError::InvalidSignature
    );
    assert!(a.authenticate_at(&h_same_new, NOW).is_ok());
}
