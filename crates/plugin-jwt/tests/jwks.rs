//! `jwks` モジュールの公開 API だけを使った統合テスト（イシュー #814）。
//! 鍵値はすべて合成値で、実運用鍵は含まない。

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fandhe_backend_plugin_jwt::{
    Algorithm, JwksError, KeySet, MAX_JWKS_DOC_LEN, MAX_KEYS, MAX_KID_LEN, SharedKeySet,
};

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn rsa_with(kid: &str, alg: &str, n: &[u8], e: &[u8]) -> String {
    format!(
        r#"{{"kty":"RSA","kid":"{kid}","alg":"{alg}","n":"{}","e":"{}"}}"#,
        b64(n),
        b64(e)
    )
}

fn rsa(kid: &str, seed: u8) -> String {
    rsa_with(kid, "RS256", &[seed; 256], &[1, 0, 1])
}

fn ec_with(kid: &str, alg: &str, crv: &str, x: &[u8], y: &[u8]) -> String {
    format!(
        r#"{{"kty":"EC","kid":"{kid}","alg":"{alg}","crv":"{crv}","x":"{}","y":"{}"}}"#,
        b64(x),
        b64(y)
    )
}

fn ec(kid: &str, seed: u8) -> String {
    ec_with(kid, "ES256", "P-256", &[seed; 32], &[seed; 32])
}

fn jwks(entries: &[String]) -> String {
    format!(r#"{{"keys":[{}]}}"#, entries.join(","))
}

fn err(entries: &[String]) -> JwksError {
    KeySet::from_json(&jwks(entries)).unwrap_err()
}

#[test]
fn parses_rsa_ec_and_mixed() {
    let rsa_only = KeySet::from_json(&jwks(&[rsa("r", 1)])).unwrap();
    let k = rsa_only.keys_for("r").next().unwrap();
    assert_eq!((k.kid(), k.algorithm()), ("r", Algorithm::Rs256));

    let ec_only = KeySet::from_json(&jwks(&[ec("e", 1)])).unwrap();
    assert_eq!(
        ec_only.keys_for("e").next().unwrap().algorithm(),
        Algorithm::Es256
    );

    let mixed = KeySet::from_json(&jwks(&[rsa("r", 1), ec("e", 2)])).unwrap();
    assert_eq!(mixed.len(), 2);
    assert_eq!(mixed.keys_for("missing").count(), 0);
}

#[test]
fn empty_keys_array_is_empty_set() {
    let set = KeySet::from_json(r#"{"keys":[]}"#).unwrap();
    assert!(set.is_empty());
    assert_eq!(set.skipped(), 0);
}

#[test]
fn same_kid_rotation_up_to_three() {
    let two = KeySet::from_json(&jwks(&[rsa("k", 1), rsa("k", 2)])).unwrap();
    let ks: Vec<_> = two.keys_for("k").collect();
    assert_eq!(ks.len(), 2);

    let mixed = KeySet::from_json(&jwks(&[rsa("k", 1), ec("k", 2)])).unwrap();
    let algs: Vec<_> = mixed.keys_for("k").map(|k| k.algorithm()).collect();
    assert_eq!(algs, [Algorithm::Rs256, Algorithm::Es256]);

    assert_eq!(
        KeySet::from_json(&jwks(&[rsa("k", 1), rsa("k", 2), rsa("k", 3)]))
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        err(&[rsa("k", 1), rsa("k", 2), rsa("k", 3), rsa("k", 4)]),
        JwksError::TooManyKeysForKeyId
    );
}

#[test]
fn identical_duplicate_is_rejected_but_same_material_other_kid_is_allowed() {
    assert_eq!(err(&[rsa("k", 1), rsa("k", 1)]), JwksError::DuplicateKey);
    assert_eq!(
        KeySet::from_json(&jwks(&[rsa("a", 1), rsa("b", 1)]))
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn rejects_missing_kid_and_alg() {
    let no_kid = r#"{"kty":"RSA","alg":"RS256","n":"AQAB","e":"AQAB"}"#.to_string();
    assert_eq!(err(&[no_kid]), JwksError::MissingKeyId);
    assert_eq!(
        err(&[rsa_with("", "RS256", &[1; 256], &[1])]),
        JwksError::MissingKeyId
    );
    let long = "k".repeat(MAX_KID_LEN + 1);
    assert_eq!(err(&[rsa(&long, 1)]), JwksError::KeyIdTooLong);
    assert!(KeySet::from_json(&jwks(&[rsa(&"k".repeat(MAX_KID_LEN), 1)])).is_ok());

    let rsa_no_alg = r#"{"kty":"RSA","kid":"a","n":"AQAB","e":"AQAB"}"#.to_string();
    assert_eq!(err(&[rsa_no_alg]), JwksError::MissingAlgorithm);
    let ec_no_alg = format!(
        r#"{{"kty":"EC","kid":"a","crv":"P-256","x":"{}","y":"{}"}}"#,
        b64(&[1; 32]),
        b64(&[1; 32])
    );
    assert_eq!(err(&[ec_no_alg]), JwksError::MissingAlgorithm);
}

#[test]
fn rejects_kind_alg_mismatch_and_unsupported_curve() {
    let x = [1u8; 32];
    assert_eq!(
        err(&[rsa_with("a", "ES256", &[1; 256], &[1])]),
        JwksError::KeyAlgorithmMismatch
    );
    assert_eq!(
        err(&[ec_with("a", "RS256", "P-256", &x, &x)]),
        JwksError::KeyAlgorithmMismatch
    );
    assert_eq!(
        err(&[ec_with("a", "ES384", "P-256", &x, &x)]),
        JwksError::KeyAlgorithmMismatch
    );
    assert_eq!(
        err(&[rsa_with("a", "FOO", &[1; 256], &[1])]),
        JwksError::KeyAlgorithmMismatch
    );
    assert_eq!(
        err(&[ec_with("a", "RS256", "P-256", &x, &x)]),
        JwksError::KeyAlgorithmMismatch
    );
    assert_eq!(
        err(&[ec_with("a", "ES256", "P-384", &x, &x)]),
        JwksError::UnsupportedCurve
    );
    let no_crv = format!(
        r#"{{"kty":"EC","kid":"a","alg":"ES256","x":"{}","y":"{}"}}"#,
        b64(&x),
        b64(&x)
    );
    assert_eq!(err(&[no_crv]), JwksError::UnsupportedCurve);
    let no_crv_no_alg = format!(
        r#"{{"kty":"EC","kid":"a","x":"{}","y":"{}"}}"#,
        b64(&x),
        b64(&x)
    );
    assert_eq!(err(&[no_crv_no_alg]), JwksError::UnsupportedCurve);
}

#[test]
fn rejects_size_limits() {
    let pad = " ".repeat(MAX_JWKS_DOC_LEN);
    assert_eq!(
        KeySet::from_json(&format!(r#"{{"keys":[]}}{pad}"#)).unwrap_err(),
        JwksError::DocumentTooLarge
    );

    let many: Vec<String> = (0..=MAX_KEYS).map(|i| rsa(&format!("k{i}"), 1)).collect();
    assert_eq!(err(&many), JwksError::TooManyKeys);
    assert_eq!(
        KeySet::from_json(&jwks(&many[..MAX_KEYS])).unwrap().len(),
        MAX_KEYS
    );

    assert_eq!(
        err(&[rsa_with("a", "RS256", &[1; 1025], &[1])]),
        JwksError::KeyTooLarge
    );
    assert!(KeySet::from_json(&jwks(&[rsa_with("a", "RS256", &[1; 1024], &[1])])).is_ok());
    assert_eq!(
        err(&[rsa_with("a", "RS256", &[1; 256], &[1; 9])]),
        JwksError::KeyTooLarge
    );

    for len in [31usize, 33] {
        let c = vec![1u8; len];
        assert_eq!(
            err(&[ec_with("a", "ES256", "P-256", &c, &[1; 32])]),
            JwksError::InvalidKeyLength
        );
        assert_eq!(
            err(&[ec_with("a", "ES256", "P-256", &[1; 32], &c)]),
            JwksError::InvalidKeyLength
        );
    }
}

#[test]
fn rejects_dangerous_keys() {
    let oct = r#"{"kty":"oct","kid":"a","alg":"HS256","k":"AQAB"}"#.to_string();
    assert_eq!(err(&[oct]), JwksError::SymmetricKey);
    for alg in ["none", "HS256", "HS512"] {
        assert_eq!(
            err(&[rsa_with("a", alg, &[1; 256], &[1])]),
            JwksError::ForbiddenAlgorithm
        );
    }
    for extra in [r#""d":"AQAB""#, r#""d":null"#, r#""p":"AQAB""#] {
        let k = format!(
            r#"{{"kty":"RSA","kid":"a","alg":"RS256","n":"{}","e":"AQAB",{extra}}}"#,
            b64(&[1; 256])
        );
        assert_eq!(err(&[k]), JwksError::PrivateKeyMaterial);
    }
    // 読み飛ばし対象の鍵でも危険な鍵は文書全体を拒否する。
    let enc = r#"{"kty":"RSA","kid":"a","use":"enc","d":"AQAB"}"#.to_string();
    assert_eq!(err(&[enc]), JwksError::PrivateKeyMaterial);
}

#[test]
fn rejects_malformed_components_and_json() {
    let bad_b64 = r#"{"kty":"RSA","kid":"a","alg":"RS256","n":"!!!","e":"AQAB"}"#.to_string();
    assert_eq!(err(&[bad_b64]), JwksError::InvalidKeyEncoding);
    let padded = r#"{"kty":"RSA","kid":"a","alg":"RS256","n":"AQAB=","e":"AQAB"}"#.to_string();
    assert_eq!(err(&[padded]), JwksError::InvalidKeyEncoding);
    let no_e = format!(
        r#"{{"kty":"RSA","kid":"a","alg":"RS256","n":"{}"}}"#,
        b64(&[1; 256])
    );
    assert_eq!(err(&[no_e]), JwksError::MissingKeyComponent);
    let no_y = format!(
        r#"{{"kty":"EC","kid":"a","alg":"ES256","crv":"P-256","x":"{}"}}"#,
        b64(&[1; 32])
    );
    assert_eq!(err(&[no_y]), JwksError::MissingKeyComponent);

    for bad in [
        "",
        "not json",
        r#"{"keys":{}}"#,
        r#"{}"#,
        r#"{"keys":[{"kid":"a"}]}"#,
    ] {
        assert_eq!(
            KeySet::from_json(bad).unwrap_err(),
            JwksError::InvalidJson,
            "{bad}"
        );
    }
    let dup_field = r#"{"keys":[{"kty":"RSA","kty":"RSA"}]}"#;
    assert_eq!(
        KeySet::from_json(dup_field).unwrap_err(),
        JwksError::InvalidJson
    );
}

#[test]
fn skips_unsupported_but_consistent_keys_and_counts_them() {
    let skipped = [
        r#"{"kty":"RSA","kid":"a","alg":"RS256","use":"enc","n":"AQAB","e":"AQAB"}"#.to_string(),
        rsa_with("b", "RS384", &[1; 256], &[1]),
        rsa_with("c", "PS256", &[1; 256], &[1]),
        ec_with("d", "ES384", "P-384", &[1; 48], &[1; 48]),
        format!(
            r#"{{"kty":"EC","crv":"P-384","x":"{}","y":"{}"}}"#,
            b64(&[1; 48]),
            b64(&[1; 48])
        ),
        r#"{"kty":"OKP","crv":"Ed25519","x":"AQAB"}"#.to_string(),
    ];
    let mut all = vec![rsa("ok", 1)];
    all.extend(skipped.iter().cloned());
    let set = KeySet::from_json(&jwks(&all)).unwrap();
    assert_eq!((set.len(), set.skipped()), (1, skipped.len()));

    // 読み飛ばしは採用数の上限に数えない。
    let mut many: Vec<String> = (0..MAX_KEYS).map(|i| rsa(&format!("k{i}"), 1)).collect();
    many.extend((0..MAX_KEYS).map(|_| r#"{"kty":"OKP","crv":"Ed25519","x":"AQAB"}"#.to_string()));
    assert_eq!(KeySet::from_json(&jwks(&many)).unwrap().len(), MAX_KEYS);

    // 採用できる鍵が 0 件なら構築エラー。
    assert_eq!(err(&skipped[..1]), JwksError::NoUsableKeys);
}

#[test]
fn escaped_kid_is_decoded() {
    let k = r#"{"kty":"RSA","kid":"ab","alg":"RS256","n":"AQAB","e":"AQAB"}"#.to_string();
    let set = KeySet::from_json(&jwks(&[k])).unwrap();
    assert_eq!(set.keys_for("ab").count(), 1);
}

#[test]
fn shared_key_set_swaps_atomically() {
    let shared = SharedKeySet::from_json(&jwks(&[rsa("old", 1)])).unwrap();
    let before = shared.snapshot();
    shared.set(KeySet::from_json(&jwks(&[rsa("new", 2)])).unwrap());
    let after = shared.snapshot();
    assert!(!Arc::ptr_eq(&before, &after));
    assert_eq!(before.keys_for("old").count(), 1);
    assert_eq!(after.keys_for("new").count(), 1);
    assert!(Arc::ptr_eq(&after, &shared.clone().snapshot()));
}

#[test]
fn auto_traits_and_display_do_not_leak_input() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<KeySet>();
    assert_send_sync::<SharedKeySet>();
    assert_send_sync::<JwksError>();

    let secret = "secret-kid-value";
    let e = err(&[rsa_with(secret, "ES256", &[1; 256], &[1])]);
    assert!(!e.to_string().contains(secret));
    assert!(!format!("{e:?}").contains(secret));
    let set = KeySet::from_json(&jwks(&[rsa("visible", 1)])).unwrap();
    assert!(format!("{set:?}").contains("visible"));
}
