//! イシュー #821: RS256・ES256 の署名検証コストとキャッシュヒットの効果の計測。
//!
//! 仕様の要望整理（`requests/fandhe-auth.md`）の「検証だけを見ると ES256 は RS256 より遅く、
//! その差は署名の検証結果のキャッシュで吸収する」という注記を実測で確かめる。
//! 設計は `docs/design/jwt-verify-plugin.md` 2.6 節、結果は
//! `benches/reports/issue821-jwt-verify-performance.md` に転記する。
//!
//! アルゴリズムごとに次の 3 シナリオを、同一トークンに対して ops 回 x trials 試行で測り、
//! 試行ごとの ns/op と中央値を出す（`benches/README.md` の複数回計測・中央値評価に従う）。
//! - A: `verify` を直接呼ぶ（署名検証を毎回実行）
//! - B: `JwtAuthenticator::authenticate`（キャッシュヒットのみ。ヒット数を assert する）
//! - C: `JwtGate::check`（実ゲート経路のヒット。`Allow` を assert する）
//!
//! 鍵は `tests/fixtures` のテスト専用鍵で、本番使用禁止。example のため本体の依存ツリーへ
//! 影響しない（`.claude/rules/pay-for-what-you-use.md`）。
//!
//! ```bash
//! cargo run --release -p fandhe-backend-plugin-jwt --example jwt_verify_bench
//! ```
//!
//! 環境変数（任意。0・解析不能な値は既定値に戻す）:
//! - `FANDHE_BACKEND_JWT_VERIFY_BENCH_OPS`（既定 20000）: 1 試行あたりの呼び出し回数
//! - `FANDHE_BACKEND_JWT_VERIFY_BENCH_TRIALS`（既定 7）: 試行回数

use std::env;
use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_plugin_jwt::{
    Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtAuthenticator, JwtGate, JwtPolicy, KeySet,
    NoExtraClaims, SharedKeySet, TokenType, verify,
};
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
    RsaPublicKeyComponents,
};

const ISS: &str = "https://bench.example";
const AUD: &str = "bench-api";
const EXP: u64 = 9_999_999_999;

/// テスト・計測専用鍵（PKCS#8 DER）。本番使用禁止（`tests/fixtures/README.md`）。
const RSA_PK8: &[u8] = include_bytes!("../tests/fixtures/test-rsa-2048.pk8");
const EC_PK8: &[u8] = include_bytes!("../tests/fixtures/test-ec-p256.pk8");

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

fn policy() -> JwtPolicy {
    JwtPolicy::new(
        Issuer::new(ISS).expect("valid issuer"),
        Audiences::single(AUD).expect("valid audience"),
        AllowedAlgorithms::new([Algorithm::Rs256, Algorithm::Es256]).expect("non-empty"),
        TokenType::AccessToken,
    )
}

fn payload() -> String {
    format!(r#"{{"iss":"{ISS}","aud":"{AUD}","exp":{EXP}}}"#)
}

fn sign_rs(kp: &RsaKeyPair, kid: &str) -> String {
    let header = format!(r#"{{"alg":"RS256","typ":"at+jwt","kid":"{kid}"}}"#);
    let msg = format!("{}.{}", B.encode(header), B.encode(payload()));
    let mut sig = vec![0u8; kp.public().modulus_len()];
    kp.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        msg.as_bytes(),
        &mut sig,
    )
    .expect("rsa sign");
    format!("{msg}.{}", B.encode(sig))
}

fn sign_es(kp: &EcdsaKeyPair, kid: &str) -> String {
    let header = format!(r#"{{"alg":"ES256","typ":"at+jwt","kid":"{kid}"}}"#);
    let msg = format!("{}.{}", B.encode(header), B.encode(payload()));
    let sig = kp
        .sign(&SystemRandom::new(), msg.as_bytes())
        .expect("ec sign");
    format!("{msg}.{}", B.encode(sig.as_ref()))
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
    // 公開点は非圧縮形式 0x04 || X(32) || Y(32)。
    let pt = kp.public_key().as_ref();
    format!(
        r#"{{"kty":"EC","kid":"{kid}","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}"#,
        B.encode(&pt[1..33]),
        B.encode(&pt[33..65])
    )
}

/// 実リクエストと同じ経路（生バイト列のパース）でヘッダを組み立てる。
fn head_with_bearer(token: &str) -> RequestHead {
    let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
    match parse_request_head(&raw).expect("valid head") {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!("fixture request is always complete"),
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    }
}

fn warmed_auth(keys: &KeySet, head: &RequestHead) -> JwtAuthenticator<NoExtraClaims> {
    let auth =
        JwtAuthenticator::<NoExtraClaims>::builder(SharedKeySet::new(keys.clone()), policy())
            .build();
    // 1 回目はミス（実検証）。計測対象からは除外し、以降のヒットのみ計測する。
    auth.authenticate(head).expect("warms the cache");
    auth
}

/// 1 アルゴリズム分の A/B/C 各シナリオの中央値（ns/op）を返す。
fn bench(label: &str, token: &str, keys: &KeySet, ops: usize, trials: usize) -> (f64, f64, f64) {
    let head = head_with_bearer(token);
    let pol = policy();
    let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());

    for _ in 0..trials {
        let start = Instant::now();
        for _ in 0..ops {
            let claims = verify::<NoExtraClaims>(token, keys, &pol, 0).expect("valid token");
            std::hint::black_box(claims);
        }
        a.push(start.elapsed().as_nanos() as f64 / ops as f64);
    }
    for _ in 0..trials {
        let auth = warmed_auth(keys, &head);
        let start = Instant::now();
        for _ in 0..ops {
            let claims = auth.authenticate(&head).expect("cache hit");
            std::hint::black_box(claims);
        }
        let ns = start.elapsed().as_nanos() as f64 / ops as f64;
        assert_eq!(
            auth.cache_hits(),
            ops as u64,
            "全呼び出しがヒットであること"
        );
        assert_eq!(auth.cache_misses(), 1, "ミスはウォームアップの 1 回のみ");
        b.push(ns);
    }
    let ctx = GateContext::new(None);
    for _ in 0..trials {
        let gate = JwtGate::new(warmed_auth(keys, &head));
        let start = Instant::now();
        for _ in 0..ops {
            let outcome = gate.check(
                &head,
                &ctx,
                &mut fandhe_backend_core::extension::RequestExtensions::new(),
            );
            assert!(
                matches!(outcome, GateOutcome::Allow),
                "ゲートが Allow を返すこと"
            );
            std::hint::black_box(outcome);
        }
        c.push(start.elapsed().as_nanos() as f64 / ops as f64);
    }
    println!("[{label}] A verify direct (ns/op, per trial): {a:?}");
    println!("[{label}] B authenticate cache hit (ns/op, per trial): {b:?}");
    println!("[{label}] C JwtGate::check cache hit (ns/op, per trial): {c:?}");
    (median(a), median(b), median(c))
}

fn main() {
    let ops = env_usize("FANDHE_BACKEND_JWT_VERIFY_BENCH_OPS", 20_000);
    let trials = env_usize("FANDHE_BACKEND_JWT_VERIFY_BENCH_TRIALS", 7);

    let rsa = RsaKeyPair::from_pkcs8(RSA_PK8).expect("valid rsa fixture");
    let ec = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        EC_PK8,
        &SystemRandom::new(),
    )
    .expect("valid ec fixture");
    let keys = KeySet::from_json(&format!(
        r#"{{"keys":[{},{}]}}"#,
        rsa_jwk(&rsa, "bench-rs"),
        ec_jwk(&ec, "bench-es")
    ))
    .expect("valid jwks");

    println!("ops_per_trial={ops} trials={trials}");
    let (rs_a, rs_b, rs_c) = bench("RS256", &sign_rs(&rsa, "bench-rs"), &keys, ops, trials);
    let (es_a, es_b, es_c) = bench("ES256", &sign_es(&ec, "bench-es"), &keys, ops, trials);

    println!("median RS256 verify direct: {rs_a:.1} ns/op");
    println!("median RS256 cache hit (authenticate): {rs_b:.1} ns/op");
    println!("median RS256 cache hit (gate): {rs_c:.1} ns/op");
    println!("median ES256 verify direct: {es_a:.1} ns/op");
    println!("median ES256 cache hit (authenticate): {es_b:.1} ns/op");
    println!("median ES256 cache hit (gate): {es_c:.1} ns/op");
    println!("speedup RS256 (A/B): {:.1}x", rs_a / rs_b);
    println!("speedup ES256 (A/B): {:.1}x", es_a / es_b);
    println!("ratio ES256/RS256 verify direct: {:.2}", es_a / rs_a);
    println!("ratio ES256/RS256 cache hit: {:.2}", es_b / rs_b);
    println!(
        "result rs256_verify_ns={rs_a:.1} rs256_hit_ns={rs_b:.1} rs256_gate_ns={rs_c:.1} \
         es256_verify_ns={es_a:.1} es256_hit_ns={es_b:.1} es256_gate_ns={es_c:.1}"
    );
}
