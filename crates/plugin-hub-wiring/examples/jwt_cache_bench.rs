//! TASK-9.3（#63）: JWT 検証結果のリクエストスコープキャッシュのコスト計測。
//!
//! `jwt::verify`（RS256 署名検証を毎回実行）と `Authenticator::authenticate`
//! （キャッシュヒット、署名検証なし）を同一トークンに対して N 回ずつ呼び、
//! 1 回あたりの所要時間（ns/op）を比較する。`benches/README.md` の方針
//! （複数回計測・中央値評価。単発の外れ値に引きずられない）を踏襲し、複数
//! 試行の中央値を採用する。
//!
//! 検証・キャッシュの実装は `fandhe-backend-plugin-jwt` にあり、本 example は hub の
//! ラッパー（`Authenticator`）越しの計測として残している。RS256・ES256 を対象にした
//! plugin-jwt 直接の計測は `crates/plugin-jwt/examples/jwt_verify_bench.rs`（#821）にある。
//!
//! example のためライブラリ本体・依存ツリーへ影響しない
//! （`.claude/rules/pay-for-what-you-use.md`）。
//!
//! ```bash
//! cargo run --release -p fandhe-backend-plugin-hub-wiring --example jwt_cache_bench
//! ```
//!
//! 環境変数（すべて任意）:
//! - `FANDHE_BACKEND_JWT_CACHE_BENCH_OPS`（既定 20000）: 1 試行あたりの呼び出し回数
//! - `FANDHE_BACKEND_JWT_CACHE_BENCH_TRIALS`（既定 7）: 試行回数（中央値算出用）

use std::env;
use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fandhe_backend_plugin_hub_wiring::HubClaims;
use fandhe_backend_plugin_hub_wiring::gate::TenantGateConfig;
use fandhe_backend_plugin_hub_wiring::jwt::{
    Algorithm, AllowedAlgorithms, Audiences, Claims, Issuer, JwtPolicy, KeySet, SharedKeySet,
    TokenType, verify,
};
use ring::rand::SystemRandom;
use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};

const TEST_KID: &str = "bench-kid-1";
const BENCH_ISS: &str = "https://bench.example";
const BENCH_AUD: &str = "bench-api";

fn bench_policy() -> JwtPolicy {
    JwtPolicy::new(
        Issuer::new(BENCH_ISS).expect("valid issuer"),
        Audiences::single(BENCH_AUD).expect("valid audience"),
        AllowedAlgorithms::only(Algorithm::Rs256),
        TokenType::AccessToken,
    )
}

/// テスト・計測専用の RSA 2048bit 秘密鍵（PKCS#8 DER）。本番使用禁止
/// （`tests/fixtures/README.md`、.claude/rules/security.md シークレット混入防止）。
const TEST_PKCS8: &[u8] = include_bytes!("../tests/fixtures/test-rsa-2048.pk8");

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

fn jwks_json_for(keypair: &RsaKeyPair, kid: &str) -> String {
    let components: RsaPublicKeyComponents<Vec<u8>> =
        RsaPublicKeyComponents::from(keypair.public_key());
    let n_b64 = URL_SAFE_NO_PAD.encode(&components.n);
    let e_b64 = URL_SAFE_NO_PAD.encode(&components.e);
    format!(
        r#"{{"keys":[{{"kty":"RSA","kid":"{kid}","n":"{n_b64}","e":"{e_b64}","use":"sig","alg":"RS256"}}]}}"#
    )
}

fn make_token(keypair: &RsaKeyPair, kid: &str, org_id: &str, exp: u64) -> String {
    let header = format!(r#"{{"alg":"RS256","typ":"at+jwt","kid":"{kid}"}}"#);
    let payload =
        format!(r#"{{"iss":"{BENCH_ISS}","aud":"{BENCH_AUD}","org_id":"{org_id}","exp":{exp}}}"#);
    let header_b64 = URL_SAFE_NO_PAD.encode(header);
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
    let signing_input = format!("{header_b64}.{payload_b64}");
    let rng = SystemRandom::new();
    let mut sig = vec![0u8; keypair.public().modulus_len()];
    keypair
        .sign(
            &signature::RSA_PKCS1_SHA256,
            &rng,
            signing_input.as_bytes(),
            &mut sig,
        )
        .expect("signing succeeds");
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
    format!("{header_b64}.{payload_b64}.{sig_b64}")
}

/// `head_from` 相当のヘッダ組み立て（`fandhe-backend-http` の `RequestHead` は生バイト列から
/// パースする API のみを公開するため、実リクエストと同じ経路で組み立てる）。
fn head_with_bearer(token: &str) -> fandhe_backend_http::request::RequestHead {
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
    match parse_request_head(&raw).expect("valid head") {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!("fixture request is always complete"),
    }
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

fn main() {
    let ops = env_usize("FANDHE_BACKEND_JWT_CACHE_BENCH_OPS", 20_000);
    let trials = env_usize("FANDHE_BACKEND_JWT_CACHE_BENCH_TRIALS", 7);

    let keypair = RsaKeyPair::from_pkcs8(TEST_PKCS8).expect("valid pkcs8 fixture");
    let keys = KeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).expect("valid jwks");
    let token = make_token(&keypair, TEST_KID, "org-bench", 9_999_999_999);
    let head = head_with_bearer(&token);
    let policy = bench_policy();

    // シナリオ A: 直接 `jwt::verify` を N 回（RS256 署名検証を毎回実行）。
    let mut direct_ns_per_op = Vec::with_capacity(trials);
    for _ in 0..trials {
        let start = Instant::now();
        for _ in 0..ops {
            let claims: Claims<HubClaims> = verify(&token, &keys, &policy, 0).expect("valid token");
            std::hint::black_box(claims);
        }
        let elapsed = start.elapsed();
        direct_ns_per_op.push(elapsed.as_nanos() as f64 / ops as f64);
    }

    // シナリオ B: `Authenticator` 経由でキャッシュを温めた後、ヒットのみを N 回計測。
    let mut cached_ns_per_op = Vec::with_capacity(trials);
    for _ in 0..trials {
        // ハンドラ側の利用手順（`TenantGateConfig::authenticator` doc 参照）どおり、
        // `TenantGateConfig` 経由で認証器を取り出す。
        let authenticator =
            TenantGateConfig::new(SharedKeySet::new(keys.clone()), bench_policy()).authenticator();
        // 1 回目はミス（実検証）。計測対象からは除外し、以降のヒットのみ計測する。
        authenticator.authenticate(&head).expect("warms the cache");
        assert_eq!(authenticator.cache_misses(), 1);

        let start = Instant::now();
        for _ in 0..ops {
            let claims = authenticator.authenticate(&head).expect("cache hit");
            std::hint::black_box(claims);
        }
        let elapsed = start.elapsed();
        assert_eq!(
            authenticator.cache_hits(),
            ops as u64,
            "全呼び出しがヒットであること（ローテーション・期限切れが混入していない）"
        );
        cached_ns_per_op.push(elapsed.as_nanos() as f64 / ops as f64);
    }

    let direct_median = median(direct_ns_per_op.clone());
    let cached_median = median(cached_ns_per_op.clone());
    let speedup = direct_median / cached_median;

    println!("ops_per_trial={ops} trials={trials}");
    println!("jwt::verify direct (ns/op, per trial): {direct_ns_per_op:?}");
    println!("authenticator cache hit (ns/op, per trial): {cached_ns_per_op:?}");
    println!("median jwt::verify direct: {direct_median:.1} ns/op");
    println!("median authenticator cache hit: {cached_median:.1} ns/op");
    println!("speedup (direct / cached): {speedup:.1}x");
}
