//! JWT fuzz target 3 本（`jwks_from_json`・`verify_token`・`verify_claims`）が共有するハーネス部品。
//!
//! 役割: テスト専用鍵（`crates/plugin-jwt/tests/fixtures`）から JWKS 文書・署名を組み立てる
//! だけで、検証ロジック自体は持たない（検証は `fandhe_backend_plugin_jwt` の公開 API のみを
//! 呼ぶ。fuzz 専用の公開 API や feature は足さない、設計 2.8 節）。鍵はリポジトリに平文で
//! 置かれた使い捨てで、本番鍵・実トークンは扱わない。
//!
//! 各 target から `#[path]` で取り込まれるため、target ごとに未使用の関数が出る。

#![allow(dead_code)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_plugin_jwt::{
    Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtPolicy, KeySet, TokenType,
};
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
    RsaPublicKeyComponents,
};

pub const ISS: &str = "https://issuer.example";
pub const AUD: &str = "api";
/// 検証時刻の基準（UNIX 秒）。
pub const NOW: u64 = 1_000_000;

const RSA_PK8: &[u8] = include_bytes!("../../../tests/fixtures/test-rsa-2048.pk8");
const EC_PK8: &[u8] = include_bytes!("../../../tests/fixtures/test-ec-p256.pk8");

pub fn rsa_pair() -> RsaKeyPair {
    // 固定のテスト鍵なので失敗しない（失敗は fuzz 入力ではなくハーネスの欠陥）。
    RsaKeyPair::from_pkcs8(RSA_PK8).expect("fixture RSA key must parse")
}

pub fn ec_pair() -> EcdsaKeyPair {
    EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        EC_PK8,
        &SystemRandom::new(),
    )
    .expect("fixture EC key must parse")
}

/// RSA 公開鍵の JWK（RS256）。
pub fn rsa_jwk(kid: &str) -> String {
    let c = RsaPublicKeyComponents::<Vec<u8>>::from(rsa_pair().public());
    format!(
        r#"{{"kty":"RSA","kid":"{kid}","alg":"RS256","n":"{}","e":"{}"}}"#,
        B.encode(&c.n),
        B.encode(&c.e)
    )
}

/// EC P-256 公開鍵の JWK（ES256）。
pub fn ec_jwk(kid: &str) -> String {
    let kp = ec_pair();
    let pt = kp.public_key().as_ref();
    format!(
        r#"{{"kty":"EC","kid":"{kid}","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}"#,
        B.encode(&pt[1..33]),
        B.encode(&pt[33..65])
    )
}

pub fn keyset(jwks: &[String]) -> KeySet {
    KeySet::from_json(&format!(r#"{{"keys":[{}]}}"#, jwks.join(",")))
        .expect("fixture JWKS must parse")
}

/// RS256（PKCS#1 v1.5、決定的）で署名する。
pub fn rsa_sign(msg: &[u8]) -> Vec<u8> {
    let kp = rsa_pair();
    let mut sig = vec![0u8; kp.public().modulus_len()];
    kp.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), msg, &mut sig)
        .expect("RSA signing must succeed");
    sig
}

pub fn policy(algs: &[Algorithm], skew_secs: u64) -> JwtPolicy {
    JwtPolicy::new(
        Issuer::new(ISS).expect("issuer"),
        Audiences::single(AUD).expect("audience"),
        AllowedAlgorithms::new(algs.iter().copied()).expect("algorithms"),
        TokenType::AccessToken,
    )
    .with_clock_skew(std::time::Duration::from_secs(skew_secs))
    .expect("skew within limit")
}

pub fn b64(bytes: &[u8]) -> String {
    B.encode(bytes)
}
