//! `JwtAuthenticator` の統合テスト（キャッシュ・鍵差し替え・時刻再判定・未知 kid 通知、#816）。
//! 鍵はテスト専用に実行時生成する ES256。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_plugin_jwt::*;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};

const ISS: &str = "https://issuer.example";
const NOW: u64 = 1_000_000;

struct Ec(EcdsaKeyPair);

impl Ec {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let pk8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        Self(
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pk8.as_ref(), &rng).unwrap(),
        )
    }
    fn keyset(&self, kid: &str) -> KeySet {
        let pt = self.0.public_key().as_ref();
        KeySet::from_json(&format!(
            r#"{{"keys":[{{"kty":"EC","kid":"{kid}","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}]}}"#,
            B.encode(&pt[1..33]),
            B.encode(&pt[33..65])
        ))
        .unwrap()
    }
    fn token_with(&self, header: &str, body: &str) -> String {
        let msg = format!("{}.{}", B.encode(header), B.encode(body));
        let sig = self.0.sign(&SystemRandom::new(), msg.as_bytes()).unwrap();
        format!("{msg}.{}", B.encode(sig.as_ref()))
    }
    fn token(&self, kid: &str, extra: &str) -> String {
        self.token_with(
            &format!(r#"{{"alg":"ES256","kid":"{kid}","typ":"at+jwt"}}"#),
            &format!(
                r#"{{"iss":"{ISS}","aud":"api","exp":{}{extra}}}"#,
                NOW + 100
            ),
        )
    }
}

fn policy(skew: u64) -> JwtPolicy {
    JwtPolicy::new(
        Issuer::new(ISS).unwrap(),
        Audiences::single("api").unwrap(),
        AllowedAlgorithms::only(Algorithm::Es256),
        TokenType::AccessToken,
    )
    .with_clock_skew(Duration::from_secs(skew))
    .unwrap()
}

fn head(token: &str) -> RequestHead {
    let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n");
    match parse_request_head(raw.as_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    }
}

fn auth(shared: &SharedKeySet, skew: u64) -> JwtAuthenticator {
    JwtAuthenticator::builder(shared.clone(), policy(skew)).build()
}

type Log = Arc<Mutex<Vec<(String, u64)>>>;

fn auth_with_log(shared: &SharedKeySet, interval: Duration) -> (JwtAuthenticator, Log) {
    let log: Log = Arc::default();
    let l = Arc::clone(&log);
    let a = JwtAuthenticator::builder(shared.clone(), policy(0))
        .unknown_key_notifier(move |e: &UnknownKeyEvent<'_>| {
            l.lock().unwrap().push((e.kid().to_owned(), e.suppressed()));
        })
        .unknown_key_min_interval(interval)
        .build();
    (a, log)
}

#[test]
fn second_call_hits_cache() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let a = auth(&shared, 0);
    let h = head(&ec.token("k", r#","sub":"u1""#));
    let c1 = a.authenticate_at(&h, NOW).unwrap();
    let c2 = a.authenticate_at(&h, NOW).unwrap();
    assert_eq!(c1, c2);
    assert_eq!(c2.sub(), Some("u1"));
    assert_eq!((a.cache_hits(), a.cache_misses()), (1, 1));
}

#[test]
fn key_set_replacement_invalidates_cache() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let a = auth(&shared, 0);
    let h = head(&ec.token("k", ""));
    a.authenticate_at(&h, NOW).unwrap();
    // 同内容でも新しい Arc になるため、再検証される。
    shared.set(ec.keyset("k"));
    a.authenticate_at(&h, NOW).unwrap();
    assert_eq!((a.cache_hits(), a.cache_misses()), (0, 2));
}

#[test]
fn rotated_key_rejects_previously_cached_token() {
    let old = Ec::new();
    let new = Ec::new();
    let shared = SharedKeySet::new(old.keyset("k"));
    let a = auth(&shared, 0);
    let h = head(&old.token("k", ""));
    a.authenticate_at(&h, NOW).unwrap();
    shared.set(new.keyset("k"));
    assert_eq!(
        a.authenticate_at(&h, NOW).unwrap_err(),
        VerifyError::InvalidSignature
    );
    assert_eq!(a.cache_hits(), 0);
}

#[test]
fn hit_rechecks_exp_and_evicts() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let a = auth(&shared, 0);
    let h = head(&ec.token("k", ""));
    a.authenticate_at(&h, NOW).unwrap();
    assert_eq!(
        a.authenticate_at(&h, NOW + 100).unwrap_err(),
        VerifyError::Expired
    );
    assert_eq!(a.cache_hits(), 0);
    // エントリは破棄済み。以後は再検証して同じく Expired（ヒットにはならない）。
    assert_eq!(
        a.authenticate_at(&h, NOW + 100).unwrap_err(),
        VerifyError::Expired
    );
    assert_eq!((a.cache_hits(), a.cache_misses()), (0, 2));
}

#[test]
fn hit_honors_leeway_for_exp() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let a = auth(&shared, 30);
    let h = head(&ec.token("k", ""));
    a.authenticate_at(&h, NOW).unwrap();
    assert!(a.authenticate_at(&h, NOW + 100 + 29).is_ok());
    assert_eq!(a.cache_hits(), 1);
    assert_eq!(
        a.authenticate_at(&h, NOW + 100 + 30).unwrap_err(),
        VerifyError::Expired
    );
}

#[test]
fn hit_rechecks_nbf() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let a = auth(&shared, 10);
    let h = head(&ec.token("k", &format!(r#","nbf":{}"#, NOW + 50)));
    // nbf - leeway = NOW+40 以降なら受理。
    a.authenticate_at(&h, NOW + 40).unwrap();
    // 時計が戻った場合、キャッシュヒット経路でも NotYetValid になる。
    assert_eq!(
        a.authenticate_at(&h, NOW + 39).unwrap_err(),
        VerifyError::NotYetValid
    );
    assert_eq!(a.cache_hits(), 0);
}

#[test]
fn failures_are_not_cached() {
    let ec = Ec::new();
    let other = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let a = auth(&shared, 0);
    let h = head(&other.token("k", ""));
    for _ in 0..2 {
        assert_eq!(
            a.authenticate_at(&h, NOW).unwrap_err(),
            VerifyError::InvalidSignature
        );
    }
    assert_eq!((a.cache_hits(), a.cache_misses()), (0, 2));
}

#[test]
fn missing_header_is_missing_token() {
    let shared = SharedKeySet::new(KeySet::empty());
    let a = auth(&shared, 0);
    let raw = b"GET / HTTP/1.1\r\n\r\n";
    let ParseOutcome::Complete { head, .. } = parse_request_head(raw).unwrap() else {
        panic!()
    };
    assert_eq!(
        a.authenticate_at(&head, NOW).unwrap_err(),
        VerifyError::MissingToken
    );
}

#[test]
fn unknown_kid_notifies_and_rejects() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let (a, log) = auth_with_log(&shared, Duration::from_secs(10));
    let h = head(&ec.token("rotated-kid", ""));
    assert_eq!(
        a.authenticate_at(&h, NOW).unwrap_err(),
        VerifyError::UnknownKeyId
    );
    assert_eq!(*log.lock().unwrap(), vec![("rotated-kid".to_owned(), 0)]);
    // 間隔内は抑止、間隔後は suppressed 付きで再通知。
    a.authenticate_at(&h, NOW + 1).unwrap_err();
    a.authenticate_at(&h, NOW + 9).unwrap_err();
    assert_eq!(log.lock().unwrap().len(), 1);
    a.authenticate_at(&h, NOW + 10).unwrap_err();
    assert_eq!(log.lock().unwrap()[1], ("rotated-kid".to_owned(), 2));
}

#[test]
fn zero_interval_notifies_every_time() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let (a, log) = auth_with_log(&shared, Duration::ZERO);
    let h = head(&ec.token("x", ""));
    a.authenticate_at(&h, NOW).unwrap_err();
    a.authenticate_at(&h, NOW).unwrap_err();
    assert_eq!(log.lock().unwrap().len(), 2);
}

#[test]
fn empty_key_set_notifies() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(KeySet::empty());
    let (a, log) = auth_with_log(&shared, Duration::from_secs(10));
    let h = head(&ec.token("k", ""));
    assert_eq!(
        a.authenticate_at(&h, NOW).unwrap_err(),
        VerifyError::UnknownKeyId
    );
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[test]
fn other_failures_do_not_notify() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let (a, log) = auth_with_log(&shared, Duration::ZERO);
    // kid なし
    let no_kid = ec.token_with(r#"{"alg":"ES256","typ":"at+jwt"}"#, r#"{"iss":"x"}"#);
    assert_eq!(
        a.authenticate_at(&head(&no_kid), NOW).unwrap_err(),
        VerifyError::MissingKeyId
    );
    // kid は存在するが alg の鍵がない（RS256 を名乗る）
    let rs = ec.token_with(
        r#"{"alg":"RS256","kid":"k","typ":"at+jwt"}"#,
        r#"{"iss":"x"}"#,
    );
    assert_eq!(
        a.authenticate_at(&head(&rs), NOW).unwrap_err(),
        VerifyError::AlgorithmNotAllowed
    );
    // kid が長すぎる
    let long = "a".repeat(MAX_KID_LEN + 1);
    assert_eq!(
        a.authenticate_at(&head(&ec.token(&long, "")), NOW)
            .unwrap_err(),
        VerifyError::Malformed
    );
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn panicking_notifier_does_not_change_outcome() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let a = JwtAuthenticator::<NoExtraClaims>::builder(shared, policy(0))
        .unknown_key_notifier(|_: &UnknownKeyEvent<'_>| panic!("boom"))
        .build();
    let h = head(&ec.token("zzz", ""));
    assert_eq!(
        a.authenticate_at(&h, NOW).unwrap_err(),
        VerifyError::UnknownKeyId
    );
}

#[test]
fn cache_stays_bounded() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let a = auth(&shared, 0);
    // 一意なトークンを容量超過分だけ投入しても、最近のものは常にヒットし、古いものは再検証される。
    let n = CACHE_CAPACITY + 8;
    let heads: Vec<_> = (0..n)
        .map(|i| head(&ec.token("k", &format!(r#","jti":"j{i}""#))))
        .collect();
    for h in &heads {
        a.authenticate_at(h, NOW).unwrap();
    }
    a.authenticate_at(&heads[n - 1], NOW).unwrap();
    assert_eq!(a.cache_hits(), 1);
    let before = a.cache_misses();
    a.authenticate_at(&heads[0], NOW).unwrap();
    assert_eq!(a.cache_misses(), before + 1, "最古は FIFO で追い出される");
}
