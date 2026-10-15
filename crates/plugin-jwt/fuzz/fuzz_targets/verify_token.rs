//! fuzz target: `verify` の署名検証前の経路（イシュー #818）。
//!
//! 任意のトークン文字列（3 区画の分割・base64url・ヘッダ JSON・`alg`・`kid`・`crit`・
//! 鍵の特定・署名検証）を投入し、パニックしないこと、長さ上限が守られること、
//! `Ok` なら iss・aud・exp がポリシーを満たす（誤受理がない）ことを検証する。
//! 先頭バイトでモードを選ぶ。A は生文字列、B はハーネスが `header.payload.signature` を
//! 組み立て、ヘッダ・署名だけを fuzz 入力にして `crit`・`alg`・`kid`・署名検証へ到達させる。
//! 実行: `bash scripts/fuzz.sh`。

#![no_main]

#[path = "common/mod.rs"]
mod common;

use common::*;
use fandhe_backend_plugin_jwt::{
    Algorithm, JwtPolicy, KeySet, MAX_TOKEN_AUDIENCES, MAX_TOKEN_LEN, NoExtraClaims, VerifyError,
    verify,
};
use libfuzzer_sys::fuzz_target;
use std::sync::LazyLock;

struct Fixture {
    keys: KeySet,
    policy: JwtPolicy,
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| Fixture {
    // kid "r" は RS256、"e" は ES256、"m" は RS256 のみ（ES256 ヘッダで KeyAlgorithmMismatch）。
    keys: keyset(&[rsa_jwk("r"), ec_jwk("e"), rsa_jwk("m")]),
    policy: policy(&[Algorithm::Rs256, Algorithm::Es256], 0),
});

/// 固定の正しい payload（署名は fuzz 入力側なので通常は InvalidSignature になる）。
const PAYLOAD: &str = r#"{"iss":"https://issuer.example","aud":"api","exp":2000000}"#;

fn check(token: &str) {
    let f = &*FIXTURE;
    match verify::<NoExtraClaims>(token, &f.keys, &f.policy, NOW) {
        Ok(c) => {
            assert_eq!(c.iss(), ISS);
            assert!(c.aud().iter().any(|a| a == AUD));
            assert!(c.aud().len() <= MAX_TOKEN_AUDIENCES);
            assert!(c.exp() > NOW);
        }
        Err(e) => {
            if token.is_empty() {
                assert_eq!(e, VerifyError::MissingToken);
            } else if token.len() > MAX_TOKEN_LEN {
                assert_eq!(e, VerifyError::TooLong);
            }
            let _ = e.to_string();
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    if mode % 2 == 0 {
        if let Ok(s) = std::str::from_utf8(rest) {
            check(s);
        }
    } else {
        // 先頭 1 バイトをヘッダ長として、ヘッダと署名のバイト列に分ける。
        let Some((&hl, rest)) = rest.split_first() else {
            return;
        };
        let hl = (hl as usize).min(rest.len());
        let (header, sig) = rest.split_at(hl);
        let token = format!("{}.{}.{}", b64(header), b64(PAYLOAD.as_bytes()), b64(sig));
        check(&token);
    }
});
