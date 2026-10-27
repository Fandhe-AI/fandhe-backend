//! fuzz target: `dpop::verify_proof` の署名検証前の経路（イシュー #902、NFR-11）。
//!
//! 任意の proof 文字列（3 区画の分割・base64url・ヘッダ JSON・`crit`・`typ`・`alg`・`jwk` の
//! 検査・署名検証）を投入し、パニックしないこと、長さ上限が守られること、`Ok` のとき
//! `jti`・`iat`・サムプリントが契約を満たす（誤受理がない）ことを検証する。
//! 先頭バイトの最下位ビットでモードを選ぶ。A は生文字列、B はハーネスが
//! `header.payload.signature` を組み立て、ヘッダと署名だけを fuzz 入力にして `jwk` 検査と
//! 署名検証へ到達させる。次のビットで許可アルゴリズム（RS256+ES256 / ES256 のみ）を切り替える。
//! 実行: `bash scripts/fuzz.sh`。

#![no_main]

#[path = "common/mod.rs"]
mod common;

use common::*;
use fandhe_backend_plugin_jwt::dpop::{
    verify_proof, DpopError, ExpectedHtu, IatWindow, ProofContext, PublicOrigin, MAX_JTI_LEN,
    MAX_PROOF_LEN,
};
use fandhe_backend_plugin_jwt::{Algorithm, AllowedAlgorithms};
use libfuzzer_sys::fuzz_target;
use std::sync::LazyLock;

struct Fixture {
    htu: ExpectedHtu,
    both: AllowedAlgorithms,
    es_only: AllowedAlgorithms,
    payload: String,
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| Fixture {
    htu: PublicOrigin::new(DPOP_ORIGIN)
        .expect("origin")
        .expected_htu(DPOP_PATH)
        .expect("htu"),
    both: AllowedAlgorithms::new([Algorithm::Rs256, Algorithm::Es256]).expect("algs"),
    es_only: AllowedAlgorithms::only(Algorithm::Es256),
    payload: format!(
        r#"{{"jti":"j","htm":"GET","htu":"{DPOP_ORIGIN}{DPOP_PATH}","iat":{NOW},"ath":"{}"}}"#,
        dpop_ath()
    ),
});

fn check(proof: &str, es_only: bool) {
    let f = &*FIXTURE;
    let algs = if es_only { &f.es_only } else { &f.both };
    let window = IatWindow::default();
    let ctx = ProofContext::new("GET", &f.htu, DPOP_ACCESS_TOKEN, algs, window);
    match verify_proof(proof, &ctx, NOW) {
        Ok(v) => {
            assert!(!v.jti().is_empty() && v.jti().len() <= MAX_JTI_LEN);
            let lo = NOW.saturating_sub(window.past().as_secs());
            let hi = NOW.saturating_add(window.future().as_secs());
            assert!((lo..=hi).contains(&v.iat()), "窓外の iat を受理した");
            assert_eq!(v.thumbprint().as_str().len(), 43);
        }
        Err(e) => {
            if proof.is_empty() {
                assert_eq!(e, DpopError::MissingProof);
            } else if proof.len() > MAX_PROOF_LEN {
                assert_eq!(e, DpopError::TooLong);
            }
            let _ = e.to_string();
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let es_only = mode & 0b10 != 0;
    if mode & 1 == 0 {
        if let Ok(s) = std::str::from_utf8(rest) {
            check(s, es_only);
        }
    } else {
        // 先頭 1 バイトをヘッダ長として、ヘッダと署名のバイト列に分ける。
        let Some((&hl, rest)) = rest.split_first() else {
            return;
        };
        let hl = (hl as usize).min(rest.len());
        let (header, sig) = rest.split_at(hl);
        let proof = format!(
            "{}.{}.{}",
            b64(header),
            b64(FIXTURE.payload.as_bytes()),
            b64(sig)
        );
        check(&proof, es_only);
    }
});
