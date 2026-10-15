//! fuzz target: `verify` の署名検証後のクレーム解析（イシュー #818）。
//!
//! 入力バイト列をそのまま payload とし、ハーネスが固定テスト鍵で RS256 署名する
//! （PKCS#1 v1.5 は決定的）ことで、公開 API のみで署名後の経路（`typ`・iss・aud・exp・nbf・
//! 追加クレーム型への読み込み）に到達させる。検証するのはパニックしないことと、`Ok` のとき
//! iss・aud・時刻クレームがポリシーを満たす（誤受理がない）こと。
//! RSA 署名のため実行速度は約 1k exec/s に落ちる（smoke 用途としては許容）。
//! 実行: `bash scripts/fuzz.sh`。

#![no_main]

#[path = "common/mod.rs"]
mod common;

use common::*;
use fandhe_backend_plugin_jwt::{
    Algorithm, JwtPolicy, KeySet, MAX_TOKEN_AUDIENCES, NoExtraClaims, verify,
};
use libfuzzer_sys::fuzz_target;
use serde::Deserialize;
use std::sync::LazyLock;

/// `InvalidExtraClaims` へ到達させるための追加クレーム型。
#[derive(Deserialize)]
#[allow(dead_code)]
struct Extra {
    org: Option<String>,
    n: Option<u32>,
}

struct Fixture {
    keys: KeySet,
    strict: JwtPolicy,
    lenient: JwtPolicy,
    head: String,
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| Fixture {
    keys: keyset(&[rsa_jwk("k")]),
    strict: policy(&[Algorithm::Rs256], 0),
    lenient: policy(&[Algorithm::Rs256], 300),
    head: b64(br#"{"alg":"RS256","kid":"k","typ":"at+jwt"}"#),
});

/// `now` の候補。saturating 演算の境界（0・通常・u64 上限付近）を突く。
const NOWS: [u64; 4] = [0, NOW, u64::MAX / 2, u64::MAX];

fuzz_target!(|data: &[u8]| {
    let Some((&sel, payload)) = data.split_first() else {
        return;
    };
    let f = &*FIXTURE;
    let now = NOWS[(sel & 0b11) as usize];
    let (p, leeway) = if sel & 0b100 == 0 {
        (&f.strict, 0u64)
    } else {
        (&f.lenient, 300u64)
    };

    let signing_input = format!("{}.{}", f.head, b64(payload));
    let token = format!(
        "{signing_input}.{}",
        b64(&rsa_sign(signing_input.as_bytes()))
    );

    if let Ok(c) = verify::<NoExtraClaims>(&token, &f.keys, p, now) {
        assert_eq!(c.iss(), ISS);
        assert!(c.aud().iter().any(|a| a == AUD));
        assert!(c.aud().len() <= MAX_TOKEN_AUDIENCES);
        assert!(now.saturating_sub(leeway) < c.exp(), "期限切れを受理した");
        if let Some(nbf) = c.nbf() {
            assert!(now.saturating_add(leeway) >= nbf, "nbf 前を受理した");
        }
    }
    if let Err(e) = verify::<Extra>(&token, &f.keys, p, now) {
        let _ = e.to_string();
    }
});
