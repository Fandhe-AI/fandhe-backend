//! `dpop::ReplayStore` の公開 API だけを使った統合テスト（イシュー #903）。
//! 鍵はすべてテスト専用（ES256 は実行時生成、RS256 は `tests/fixtures`）。
#![cfg(feature = "dpop")]

use std::sync::Arc;
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

const TOKEN: &str = "access-token-value";
const RSA_PK8: &[u8] = include_bytes!("fixtures/test-rsa-2048.pk8");
/// 既定の窓の過去側（秒）。
const PAST: u64 = 60;

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

/// `jti` と `iat` を指定した proof を作り、`now` で検証して `VerifiedProof` を返す。
fn verified(s: &Signer, jti: &str, iat: u64, now: u64) -> VerifiedProof {
    let header = format!(
        r#"{{"typ":"dpop+jwt","alg":"{}","jwk":{}}}"#,
        s.alg(),
        s.jwk()
    );
    let ath = B.encode(digest(&SHA256, TOKEN.as_bytes()).as_ref());
    let payload = format!(
        r#"{{"jti":"{jti}","htm":"GET","htu":"https://api.example/resource","iat":{iat},"ath":"{ath}"}}"#
    );
    let msg = format!("{}.{}", B.encode(header), B.encode(payload));
    let proof = format!("{msg}.{}", B.encode(s.sign(&msg)));
    let htu = PublicOrigin::new("https://api.example")
        .unwrap()
        .expected_htu("/resource")
        .unwrap();
    let algs = AllowedAlgorithms::new([Algorithm::Es256, Algorithm::Rs256]).unwrap();
    let ctx = ProofContext::new("GET", &htu, TOKEN, &algs, IatWindow::default());
    verify_proof(&proof, &ctx, now).unwrap()
}

#[test]
fn rejects_reuse_within_window_for_es256_and_rs256() {
    for s in [Signer::ec(), Signer::rsa()] {
        let store = ReplayStore::new(IatWindow::default());
        let p = verified(&s, "j1", 1000, 1000);
        assert_eq!(store.register(&p, 1000), Ok(()));
        let again = verified(&s, "j1", 1000, 1000 + PAST);
        assert_eq!(
            store.register(&again, 1000 + PAST),
            Err(DpopError::Replayed)
        );
        assert_eq!(store.len(), 1);
    }
}

#[test]
fn unit_is_thumbprint_and_jti_pair() {
    let (a, b) = (Signer::ec(), Signer::ec());
    let store = ReplayStore::new(IatWindow::default());
    assert_eq!(
        store.register(&verified(&a, "j1", 1000, 1000), 1000),
        Ok(())
    );
    // 同じ jti でも別の鍵なら別の組。
    assert_eq!(
        store.register(&verified(&b, "j1", 1000, 1000), 1000),
        Ok(())
    );
    // 同じ鍵で別の jti も別の組。
    assert_eq!(
        store.register(&verified(&a, "j2", 1000, 1000), 1000),
        Ok(())
    );
    assert_eq!(store.len(), 3);
}

#[test]
fn expired_entries_are_removed_and_reusable() {
    let s = Signer::ec();
    let store = ReplayStore::new(IatWindow::default());
    assert_eq!(
        store.register(&verified(&s, "j1", 1000, 1000), 1000),
        Ok(())
    );
    assert_eq!(store.len(), 1);
    // 窓を過ぎると取り除かれる（別の登録の際に掃除される）。
    let later = 1000 + PAST + 1;
    assert_eq!(
        store.register(&verified(&s, "j2", later, later), later),
        Ok(())
    );
    assert_eq!(store.len(), 1);
}

#[test]
fn len_never_exceeds_capacity_as_time_advances() {
    let s = Signer::ec();
    let store = ReplayStore::with_capacity(IatWindow::default(), 3).unwrap();
    for i in 0..40u64 {
        let now = 1000 + i * 30;
        let _ = store.register(&verified(&s, &format!("j{i}"), now, now), now);
        assert!(store.len() <= 3);
    }
}

#[test]
fn rejects_new_proof_when_full_then_recovers() {
    let s = Signer::ec();
    let store = ReplayStore::with_capacity(IatWindow::default(), 3).unwrap();
    for i in 0..3 {
        let p = verified(&s, &format!("j{i}"), 1000, 1000);
        assert_eq!(store.register(&p, 1000), Ok(()));
    }
    let extra = verified(&s, "extra", 1000, 1000);
    assert_eq!(
        store.register(&extra, 1000),
        Err(DpopError::ReplayStoreFull)
    );
    assert_eq!(store.len(), 3);
    // 期限が過ぎれば受理される。
    let later = 1000 + PAST + 1;
    let fresh = verified(&s, "extra", later, later);
    assert_eq!(store.register(&fresh, later), Ok(()));
}

#[test]
fn capacity_is_validated() {
    let w = IatWindow::default();
    assert_eq!(
        ReplayStore::with_capacity(w, 0).unwrap_err(),
        DpopConfigError::ReplayCapacityZero
    );
    assert_eq!(
        ReplayStore::with_capacity(w, MAX_REPLAY_CAPACITY + 1).unwrap_err(),
        DpopConfigError::ReplayCapacityTooLarge
    );
    assert!(ReplayStore::with_capacity(w, MAX_REPLAY_CAPACITY).is_ok());
    assert_eq!(ReplayStore::new(w).capacity(), DEFAULT_REPLAY_CAPACITY);
    assert!(ReplayStore::new(w).is_empty());
}

#[test]
fn concurrent_registration_accepts_exactly_one() {
    let s = Signer::ec();
    let p = Arc::new(verified(&s, "j1", 1000, 1000));
    let store = Arc::new(ReplayStore::new(IatWindow::default()));
    let handles: Vec<_> = (0..16)
        .map(|_| {
            let (p, store) = (Arc::clone(&p), Arc::clone(&store));
            std::thread::spawn(move || store.register(&p, 1000))
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter(|r| r.is_err())
            .all(|r| *r == Err(DpopError::Replayed))
    );
}

#[test]
fn is_send_sync_and_hides_secrets() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ReplayStore>();

    let s = Signer::ec();
    let p = verified(&s, "secret-jti-value", 1000, 1000);
    let store = ReplayStore::with_capacity(
        IatWindow::new(Duration::from_secs(30), Duration::ZERO).unwrap(),
        10,
    )
    .unwrap();
    store.register(&p, 1000).unwrap();
    let d = format!("{store:?}");
    assert!(!d.contains("secret-jti-value"));
    assert!(!d.contains(p.thumbprint().as_str()));
    for e in [DpopError::Replayed, DpopError::ReplayStoreFull] {
        assert!(!e.to_string().contains("jti-value"));
    }
}
