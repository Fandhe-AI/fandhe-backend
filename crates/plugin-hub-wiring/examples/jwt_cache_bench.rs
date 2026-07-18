//! TASK-9.3（#63）: JWT 検証結果のリクエストスコープキャッシュのコスト計測。
//!
//! `verify_token`（RS256 署名検証を毎回実行）と `Authenticator::authenticate`
//! （キャッシュヒット、署名検証なし）を同一トークンに対して N 回ずつ呼び、
//! 1 回あたりの所要時間（ns/op）を比較する。`benches/README.md` の方針
//! （複数回計測・中央値評価。単発の外れ値に引きずられない）を踏襲し、複数
//! 試行の中央値を採用する。
//!
//! example のためライブラリ本体・依存ツリーへ影響しない
//! （`.claude/rules/pay-for-what-you-use.md`）。
//!
//! ```bash
//! cargo run --release -p bf-plugin-hub-wiring --example jwt_cache_bench
//! ```
//!
//! 環境変数（すべて任意）:
//! - `BF_JWT_CACHE_BENCH_OPS`（既定 20000）: 1 試行あたりの呼び出し回数
//! - `BF_JWT_CACHE_BENCH_TRIALS`（既定 7）: 試行回数（中央値算出用）

use std::env;
use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bf_plugin_hub_wiring::Claims;
use bf_plugin_hub_wiring::gate::TenantGateConfig;
use bf_plugin_hub_wiring::jwks::{JwksKeySet, SharedJwks};
use bf_plugin_hub_wiring::jwt::verify_token;
use ring::rand::SystemRandom;
use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};

const TEST_KID: &str = "bench-kid-1";

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
    let header = format!(r#"{{"alg":"RS256","typ":"JWT","kid":"{kid}"}}"#);
    let payload = format!(r#"{{"org_id":"{org_id}","exp":{exp}}}"#);
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

/// `head_from` 相当のヘッダ組み立て（`bf-http` の `RequestHead` は生バイト列から
/// パースする API のみを公開するため、実リクエストと同じ経路で組み立てる）。
fn head_with_bearer(token: &str) -> bf_http::request::RequestHead {
    use bf_http::request::{ParseOutcome, parse_request_head};
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
    let ops = env_usize("BF_JWT_CACHE_BENCH_OPS", 20_000);
    let trials = env_usize("BF_JWT_CACHE_BENCH_TRIALS", 7);

    let keypair = RsaKeyPair::from_pkcs8(TEST_PKCS8).expect("valid pkcs8 fixture");
    let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).expect("valid jwks");
    let token = make_token(&keypair, TEST_KID, "org-bench", 9_999_999_999);
    let head = head_with_bearer(&token);

    // シナリオ A: 直接 `verify_token` を N 回（RS256 署名検証を毎回実行）。
    let mut direct_ns_per_op = Vec::with_capacity(trials);
    for _ in 0..trials {
        let start = Instant::now();
        for _ in 0..ops {
            let claims: Claims = verify_token(&token, &keys, 0).expect("valid token");
            std::hint::black_box(claims);
        }
        let elapsed = start.elapsed();
        direct_ns_per_op.push(elapsed.as_nanos() as f64 / ops as f64);
    }

    // シナリオ B: `Authenticator` 経由でキャッシュを温めた後、ヒットのみを N 回計測。
    let mut cached_ns_per_op = Vec::with_capacity(trials);
    for _ in 0..trials {
        // `Authenticator::new` は crate 内部専用のため、公開コンストラクタ
        // `TenantGateConfig::new` + `authenticator()`（ハンドラ側の利用手順、
        // `crate::gate::TenantGateConfig::authenticator` doc 参照）経由で作る。
        let authenticator = TenantGateConfig::new(SharedJwks::new(keys.clone())).authenticator();
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
    println!("verify_token direct (ns/op, per trial): {direct_ns_per_op:?}");
    println!("authenticator cache hit (ns/op, per trial): {cached_ns_per_op:?}");
    println!("median verify_token direct: {direct_median:.1} ns/op");
    println!("median authenticator cache hit: {cached_median:.1} ns/op");
    println!("speedup (direct / cached): {speedup:.1}x");
}
