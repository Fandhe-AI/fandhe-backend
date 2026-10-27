//! イシュー #906: DPoP を有効にした `JwtGate` の検証コストの計測（NFR-6、プロセス内）。
//!
//! REQ-16 の `dpop` の受け入れ基準「DPoP を有効にしたゲートの性能への影響を計測し、記録する」
//! のための example。`jwt_verify_bench`（#821、DPoP なしの署名検証とキャッシュヒット）と同じ方式で、
//! 結果は `benches/reports/issue906-jwt-dpop-gate-performance.md` に転記する。設計は
//! `docs/design/jwt-verify-plugin.md` 9 章。
//!
//! proof の `jti` は窓の間は再使用を拒否されるため、同じリクエストを繰り返す #821 の方式は使えない。
//! そこで **計測区間の外で** `jti` が一意な proof と `RequestHead` を ops 個作り、試行ごとに新しい
//! 認証器（= 新しい `ReplayStore`）を作る。計測区間には対象の呼び出しだけを置く。
//!
//! シナリオ（アクセストークンは ES256 でキャッシュに載せ、proof の方式だけを変える）:
//! - D0: DPoP 無効の認証器・ゲートが、同じ `cnf` 付きトークンを `Bearer` で受理するキャッシュヒット（基準）
//! - D1: `dpop::verify_proof` の直接呼び出し（proof 単体の検証コスト）
//! - D2: `DpopMode::Required` の認証器 `authenticate_at`（トークンのヒット + proof 検証 + `register`）
//! - D3: `DpopMode::Required` の `JwtGate::check`（実ゲート経路。D3 - D0 が DPoP 無効時との差）
//! - D4: `DpopMode::AllowUnboundBearer` の認証器・ゲートが `cnf` のないトークンを `Bearer` で受理する
//!   経路と、DPoP 無効で同じトークンを渡した場合（HTTP 系列と対になる）
//! - D5: `ReplayStore::register` 単体（充填中の 1 回あたり、満杯後に窓が過ぎた最初の 1 回の掃除）
//!
//! 鍵は `tests/fixtures` のテスト専用鍵で、本番使用禁止。トークン・proof・`jti` は出力しない。
//! `dpop` feature 必須（`Cargo.toml` の `required-features`）。本体の依存ツリーへ影響しない
//! （`.claude/rules/pay-for-what-you-use.md`）。
//!
//! ```bash
//! cargo run --release -p fandhe-backend-plugin-jwt --features dpop --example jwt_dpop_bench
//! ```
//!
//! 環境変数（任意。0・解析不能な値は既定値に戻す）:
//! - `FANDHE_BACKEND_JWT_DPOP_BENCH_OPS`（既定 5000）: 1 試行あたりの呼び出し回数
//! - `FANDHE_BACKEND_JWT_VERIFY_BENCH_TRIALS`（既定 7）: 試行回数（`jwt_verify_bench` と共通）
//! - `FANDHE_BACKEND_JWT_DPOP_BENCH_REPLAY_FILL`（既定 131072）: D5 で `ReplayStore` へ入れる件数
//!   （既定は `DEFAULT_REPLAY_CAPACITY`。容量を超える値は上限で打ち切る）

use std::env;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestExtensions, RequestGate};
use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_plugin_jwt::dpop::{
    DEFAULT_REPLAY_CAPACITY, DpopConfig, DpopMode, IatWindow, MAX_IAT_FUTURE, MAX_IAT_PAST,
    ProofContext, PublicOrigin, ReplayStore, VerifiedProof, jwk_thumbprint, verify_proof,
};
use fandhe_backend_plugin_jwt::{
    Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtAuthenticator, JwtGate, JwtPolicy, KeySet,
    NoExtraClaims, SharedKeySet, TokenType,
};
use ring::digest::{SHA256, digest};
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
    RsaPublicKeyComponents,
};

const ISS: &str = "https://bench.example";
const AUD: &str = "bench-api";
const ORIGIN: &str = "https://api.bench.example";
const EXP: u64 = 9_999_999_999;

/// テスト・計測専用鍵（PKCS#8 DER）。本番使用禁止（`tests/fixtures/README.md`）。
/// アクセストークンの発行鍵（ES256）と、proof を署名する鍵（ES256・RS256）。
const ISSUER_EC_PK8: &[u8] = include_bytes!("../tests/fixtures/test-ec-p256.pk8");
const PROOF_EC_PK8: &[u8] = include_bytes!("../tests/fixtures/test-ec-p256-rotated.pk8");
const PROOF_RSA_PK8: &[u8] = include_bytes!("../tests/fixtures/test-rsa-2048-rotated.pk8");

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
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

fn algs() -> AllowedAlgorithms {
    AllowedAlgorithms::new([Algorithm::Rs256, Algorithm::Es256]).expect("non-empty")
}

fn policy() -> JwtPolicy {
    JwtPolicy::new(
        Issuer::new(ISS).expect("valid issuer"),
        Audiences::single(AUD).expect("valid audience"),
        algs(),
        TokenType::AccessToken,
    )
}

fn window() -> IatWindow {
    // 事前生成から計測終了までの経過で窓を外れないよう、過去側を上限にする。窓の検査は
    // 整数比較 2 回でコストに影響しない。
    IatWindow::new(MAX_IAT_PAST, MAX_IAT_FUTURE).expect("valid window")
}

/// proof を署名する鍵。
enum ProofKey {
    Ec(EcdsaKeyPair),
    Rsa(RsaKeyPair),
}

impl ProofKey {
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
                .expect("ec sign")
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
                .expect("rsa sign");
                sig
            }
        }
    }

    /// `token` に結び付いた proof。`jti` は呼び出し側が一意に指定する。`header_jwk` は署名ごとに
    /// 作り直さないよう、呼び出し側が [`ProofKey::jwk`] を 1 回だけ作って渡す。
    fn proof(&self, header_jwk: &str, token: &str, jti: u64, iat: u64) -> String {
        let header = format!(
            r#"{{"typ":"dpop+jwt","alg":"{}","jwk":{header_jwk}}}"#,
            self.alg()
        );
        let ath = B.encode(digest(&SHA256, token.as_bytes()).as_ref());
        let payload = format!(
            r#"{{"jti":"j{jti}","htm":"GET","htu":"{ORIGIN}/","iat":{iat},"ath":"{ath}"}}"#
        );
        let msg = format!("{}.{}", B.encode(header), B.encode(payload));
        format!("{msg}.{}", B.encode(self.sign(&msg)))
    }
}

/// アクセストークンの発行側（ES256、`kid` は `bench-es`）。
struct TokenIssuer(EcdsaKeyPair);

impl TokenIssuer {
    fn keyset(&self) -> KeySet {
        let pt = self.0.public_key().as_ref();
        KeySet::from_json(&format!(
            r#"{{"keys":[{{"kty":"EC","kid":"bench-es","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}]}}"#,
            B.encode(&pt[1..33]),
            B.encode(&pt[33..65])
        ))
        .expect("valid jwks")
    }

    /// `jkt` が `Some` なら `cnf.jkt` を持つトークン、`None` なら `cnf` のないトークン。
    fn token(&self, jkt: Option<&str>) -> String {
        let cnf = jkt
            .map(|j| format!(r#","cnf":{{"jkt":"{j}"}}"#))
            .unwrap_or_default();
        let payload = format!(r#"{{"iss":"{ISS}","aud":"{AUD}","exp":{EXP}{cnf}}}"#);
        let header = r#"{"alg":"ES256","typ":"at+jwt","kid":"bench-es"}"#;
        let msg = format!("{}.{}", B.encode(header), B.encode(payload));
        let sig = self
            .0
            .sign(&SystemRandom::new(), msg.as_bytes())
            .expect("ec sign");
        format!("{msg}.{}", B.encode(sig.as_ref()))
    }
}

fn parse(raw: String) -> RequestHead {
    match parse_request_head(raw.as_bytes()).expect("valid head") {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!("fixture request is always complete"),
    }
}

fn head_bearer(token: &str) -> RequestHead {
    parse(format!(
        "GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n"
    ))
}

fn head_dpop(token: &str, proof: &str) -> RequestHead {
    parse(format!(
        "GET / HTTP/1.1\r\nAuthorization: DPoP {token}\r\nDPoP: {proof}\r\n\r\n"
    ))
}

fn auth_plain(keys: &KeySet) -> JwtAuthenticator<NoExtraClaims> {
    JwtAuthenticator::<NoExtraClaims>::builder(SharedKeySet::new(keys.clone()), policy()).build()
}

fn auth_dpop(keys: &KeySet, mode: DpopMode) -> JwtAuthenticator<NoExtraClaims> {
    let cfg = DpopConfig::new(
        mode,
        PublicOrigin::new(ORIGIN).expect("valid origin"),
        algs(),
    )
    .with_iat_window(window());
    JwtAuthenticator::<NoExtraClaims>::builder(SharedKeySet::new(keys.clone()), policy())
        .dpop(cfg)
        .build()
}

fn ns_per_op(start: Instant, ops: usize) -> f64 {
    start.elapsed().as_nanos() as f64 / ops as f64
}

/// `Bearer` で提示するトークンのキャッシュヒット（`authenticate` と `JwtGate::check`）の中央値。
fn bench_bearer_hits(
    new_auth: &dyn Fn() -> JwtAuthenticator<NoExtraClaims>,
    head: &RequestHead,
    ops: usize,
    trials: usize,
) -> (f64, f64) {
    let (mut a, mut g) = (Vec::new(), Vec::new());
    for _ in 0..trials {
        let auth = new_auth();
        auth.authenticate(head).expect("warms the cache");
        let start = Instant::now();
        for _ in 0..ops {
            std::hint::black_box(auth.authenticate(head).expect("cache hit"));
        }
        a.push(ns_per_op(start, ops));
        assert_eq!(
            auth.cache_hits(),
            ops as u64,
            "全呼び出しがヒットであること"
        );
        assert_eq!(auth.cache_misses(), 1, "ミスはウォームアップの 1 回のみ");
    }
    let ctx = GateContext::new(None);
    for _ in 0..trials {
        let auth = new_auth();
        auth.authenticate(head).expect("warms the cache");
        let gate = JwtGate::new(auth);
        let start = Instant::now();
        for _ in 0..ops {
            let o = gate.check(head, &ctx, &mut RequestExtensions::new());
            assert!(matches!(o, GateOutcome::Allow), "ゲートが Allow を返すこと");
            std::hint::black_box(o);
        }
        g.push(ns_per_op(start, ops));
    }
    (median(a), median(g))
}

struct ProofResult {
    d0_auth: f64,
    d0_gate: f64,
    d1: f64,
    d2: f64,
    d3: f64,
}

fn bench_proof_alg(
    label: &str,
    pk: &ProofKey,
    issuer: &TokenIssuer,
    keys: &KeySet,
    ops: usize,
    trials: usize,
) -> ProofResult {
    let jwk = pk.jwk();
    let jkt = jwk_thumbprint(&jwk).expect("thumbprint");
    let token = issuer.token(Some(jkt.as_str()));
    let iat = now_unix();

    // 計測区間の外: 一意な `jti` の proof とヘッダ。先頭（jti=0）はウォームアップ用。
    let proofs: Vec<String> = (0..=ops as u64)
        .map(|i| pk.proof(&jwk, &token, i, iat))
        .collect();
    let heads: Vec<RequestHead> = proofs.iter().map(|p| head_dpop(&token, p)).collect();

    // D0: DPoP 無効、同じ `cnf` 付きトークンを `Bearer` で提示。
    let bearer = head_bearer(&token);
    let (d0_auth, d0_gate) = bench_bearer_hits(&|| auth_plain(keys), &bearer, ops, trials);

    // D1: proof 単体の検証。
    let htu = PublicOrigin::new(ORIGIN)
        .expect("origin")
        .expected_htu("/")
        .expect("htu");
    let allowed = algs();
    let ctx = ProofContext::new("GET", &htu, &token, &allowed, window());
    let mut d1 = Vec::new();
    for _ in 0..trials {
        let start = Instant::now();
        for p in &proofs[1..] {
            std::hint::black_box(verify_proof(p, &ctx, iat).expect("valid proof"));
        }
        d1.push(ns_per_op(start, ops));
    }

    // D2: 認証器（トークンのヒット + proof 検証 + register）。
    let mut d2 = Vec::new();
    for _ in 0..trials {
        let auth = auth_dpop(keys, DpopMode::Required);
        auth.authenticate_at(&heads[0], iat)
            .expect("warms the cache");
        let start = Instant::now();
        for h in &heads[1..] {
            std::hint::black_box(auth.authenticate_at(h, iat).expect("accepted"));
        }
        d2.push(ns_per_op(start, ops));
        assert_eq!(auth.cache_misses(), 1, "ミスはウォームアップの 1 回のみ");
        assert_eq!(
            auth.cache_hits(),
            ops as u64,
            "トークンは全件ヒットであること"
        );
    }

    // D3: 実ゲート経路。
    let gctx = GateContext::new(None);
    let mut d3 = Vec::new();
    for _ in 0..trials {
        let auth = auth_dpop(keys, DpopMode::Required);
        auth.authenticate(&heads[0]).expect("warms the cache");
        let gate = JwtGate::new(auth);
        let start = Instant::now();
        for h in &heads[1..] {
            let o = gate.check(h, &gctx, &mut RequestExtensions::new());
            assert!(matches!(o, GateOutcome::Allow), "ゲートが Allow を返すこと");
            std::hint::black_box(o);
        }
        d3.push(ns_per_op(start, ops));
    }

    println!("[{label}] D1 verify_proof direct (ns/op, per trial): {d1:?}");
    println!("[{label}] D2 authenticate_at DPoP Required (ns/op, per trial): {d2:?}");
    println!("[{label}] D3 JwtGate::check DPoP Required (ns/op, per trial): {d3:?}");
    ProofResult {
        d0_auth,
        d0_gate,
        d1: median(d1),
        d2: median(d2),
        d3: median(d3),
    }
}

/// D5: `ReplayStore::register` 単体。充填中の 1 回あたりと、窓が過ぎた最初の 1 回の掃除時間。
fn bench_replay(pk: &ProofKey, fill: usize, trials: usize) -> (f64, f64) {
    let jwk = pk.jwk();
    let win = IatWindow::default();
    let iat = now_unix();
    let token = "replay-bench-token";
    let htu = PublicOrigin::new(ORIGIN)
        .expect("origin")
        .expected_htu("/")
        .expect("htu");
    let allowed = algs();
    let ctx = ProofContext::new("GET", &htu, token, &allowed, win);
    // `VerifiedProof` は `verify_proof` からしか作れない。計測区間の外で署名・検証を済ませる。
    let verified: Vec<VerifiedProof> = (0..=fill as u64)
        .map(|i| {
            let p = pk.proof(&jwk, token, i, iat);
            verify_proof(&p, &ctx, iat).expect("valid proof")
        })
        .collect();
    let (tail, body) = verified.split_last().expect("non-empty");

    let (mut fill_ns, mut sweep_ns) = (Vec::new(), Vec::new());
    for _ in 0..trials {
        let store = ReplayStore::new(win);
        let start = Instant::now();
        for v in body {
            store.register(v, iat).expect("registered");
        }
        fill_ns.push(ns_per_op(start, fill));
        assert_eq!(store.len(), fill);

        // 窓（過去側）を 1 秒超えた時点の最初の 1 回で、全件の期限切れ項目を掃除する。
        let later = iat + win.past().as_secs() + 1;
        let start = Instant::now();
        store.register(tail, later).expect("registered after sweep");
        sweep_ns.push(start.elapsed().as_nanos() as f64);
        assert_eq!(
            store.len(),
            1,
            "期限切れが全件除かれ、新しい 1 件だけが残ること"
        );
    }
    println!("[D5] register fill {fill} (ns/op, per trial): {fill_ns:?}");
    println!("[D5] first register after window, sweeping {fill} (ns, per trial): {sweep_ns:?}");
    (median(fill_ns), median(sweep_ns))
}

fn main() {
    let ops = env_usize("FANDHE_BACKEND_JWT_DPOP_BENCH_OPS", 5_000);
    let trials = env_usize("FANDHE_BACKEND_JWT_VERIFY_BENCH_TRIALS", 7);
    let fill = env_usize(
        "FANDHE_BACKEND_JWT_DPOP_BENCH_REPLAY_FILL",
        DEFAULT_REPLAY_CAPACITY,
    )
    .min(DEFAULT_REPLAY_CAPACITY);

    let rng = SystemRandom::new();
    let issuer = TokenIssuer(
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, ISSUER_EC_PK8, &rng)
            .expect("valid ec fixture"),
    );
    let keys = issuer.keyset();
    let ec = ProofKey::Ec(
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, PROOF_EC_PK8, &rng)
            .expect("valid ec fixture"),
    );
    let rsa = ProofKey::Rsa(RsaKeyPair::from_pkcs8(PROOF_RSA_PK8).expect("valid rsa fixture"));

    println!("ops_per_trial={ops} trials={trials} replay_fill={fill}");
    let rs = bench_proof_alg("proof RS256", &rsa, &issuer, &keys, ops, trials);
    let es = bench_proof_alg("proof ES256", &ec, &issuer, &keys, ops, trials);

    // D4: AllowUnboundBearer 下の `cnf` なしトークン（Bearer）と、DPoP 無効で同じトークンを渡した場合。
    let unbound = issuer.token(None);
    let unbound_head = head_bearer(&unbound);
    let (d4_base_auth, d4_base_gate) =
        bench_bearer_hits(&|| auth_plain(&keys), &unbound_head, ops, trials);
    let (d4_auth, d4_gate) = bench_bearer_hits(
        &|| auth_dpop(&keys, DpopMode::AllowUnboundBearer),
        &unbound_head,
        ops,
        trials,
    );
    println!(
        "[D4] plain unbound bearer: authenticate {d4_base_auth:.1} gate {d4_base_gate:.1} ns/op"
    );
    println!(
        "[D4] AllowUnboundBearer unbound bearer: authenticate {d4_auth:.1} gate {d4_gate:.1} ns/op"
    );

    let (d5_fill, d5_sweep) = bench_replay(&ec, fill, trials);

    for (l, r) in [("RS256", &rs), ("ES256", &es)] {
        println!(
            "median {l} proof D0 bearer hit (authenticate): {:.1} ns/op",
            r.d0_auth
        );
        println!(
            "median {l} proof D0 bearer hit (gate): {:.1} ns/op",
            r.d0_gate
        );
        println!("median {l} proof D1 verify_proof: {:.1} ns/op", r.d1);
        println!("median {l} proof D2 authenticate_at: {:.1} ns/op", r.d2);
        println!("median {l} proof D3 gate: {:.1} ns/op", r.d3);
        println!("delta {l} D3-D0 (gate): {:.1} ns/op", r.d3 - r.d0_gate);
        println!(
            "delta {l} D2-D0 (authenticate): {:.1} ns/op",
            r.d2 - r.d0_auth
        );
    }
    println!("ratio RS256/ES256 proof D1: {:.2}", rs.d1 / es.d1);
    println!(
        "delta D4 gate (AllowUnboundBearer - plain): {:.1} ns/op",
        d4_gate - d4_base_gate
    );
    println!(
        "D5 register fill median {d5_fill:.1} ns/op, sweep median {:.3} ms",
        d5_sweep / 1_000_000.0
    );
    println!(
        "result d0_gate_ns={:.1} rs256_d1_ns={:.1} rs256_d2_ns={:.1} rs256_d3_ns={:.1} \
         es256_d1_ns={:.1} es256_d2_ns={:.1} es256_d3_ns={:.1} \
         d4_base_gate_ns={d4_base_gate:.1} d4_gate_ns={d4_gate:.1} \
         d5_fill_ns={d5_fill:.1} d5_sweep_ns={d5_sweep:.0}",
        rs.d0_gate, rs.d1, rs.d2, rs.d3, es.d1, es.d2, es.d3
    );
}
