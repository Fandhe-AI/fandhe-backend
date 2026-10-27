//! fuzz target: `dpop::verify_proof` の署名検証後のクレーム解析（イシュー #902、NFR-11）。
//!
//! 入力バイト列をそのまま payload とし、ヘッダ（`typ`・`alg`・`jwk` は固定）と署名はハーネスが
//! 固定テスト鍵で RS256 署名する（PKCS#1 v1.5 は決定的で、クラッシュを再現できる。ES256 は
//! 署名が非決定的なので使わない）。公開 API のみで署名後の経路（`jti`・`htm`・`htu` の正規化・
//! `iat` の窓・`ath`）に到達させる。検証するのはパニックしないことと、`Ok` のとき
//! `jti`・`iat`・鍵の結合・`htm`・`ath` が契約を満たす（誤受理がない）こと。
//! RSA 署名のため実行速度は約 2k exec/s に落ちる（smoke 用途としては許容）。
//! 実行: `bash scripts/fuzz.sh`。

#![no_main]

#[path = "common/mod.rs"]
mod common;

use common::*;
use fandhe_backend_plugin_jwt::dpop::{
    ExpectedHtu, IatWindow, JwkThumbprint, MAX_IAT_FUTURE, MAX_IAT_PAST, MAX_JTI_LEN, ProofContext,
    PublicOrigin, jwk_thumbprint, verify_proof,
};
use fandhe_backend_plugin_jwt::{Algorithm, AllowedAlgorithms};
use libfuzzer_sys::fuzz_target;
use std::sync::LazyLock;

struct Fixture {
    htu: ExpectedHtu,
    algs: AllowedAlgorithms,
    default_window: IatWindow,
    max_window: IatWindow,
    head: String,
    thumbprint: String,
    ath: String,
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| {
    let jwk = rsa_jwk("k");
    let thumbprint: JwkThumbprint = jwk_thumbprint(&jwk).expect("thumbprint");
    Fixture {
        htu: PublicOrigin::new(DPOP_ORIGIN)
            .expect("origin")
            .expected_htu(DPOP_PATH)
            .expect("htu"),
        algs: AllowedAlgorithms::only(Algorithm::Rs256),
        default_window: IatWindow::default(),
        max_window: IatWindow::new(MAX_IAT_PAST, MAX_IAT_FUTURE).expect("window"),
        head: b64(format!(r#"{{"typ":"dpop+jwt","alg":"RS256","jwk":{jwk}}}"#).as_bytes()),
        thumbprint: thumbprint.as_str().to_owned(),
        ath: dpop_ath(),
    }
});

/// `now` の候補。saturating 演算の境界（0・通常・u64 上限付近）を突く。
const NOWS: [u64; 4] = [0, NOW, u64::MAX / 2, u64::MAX];

fuzz_target!(|data: &[u8]| {
    let Some((&sel, payload)) = data.split_first() else {
        return;
    };
    let f = &*FIXTURE;
    let now = NOWS[(sel & 0b11) as usize];
    let window = if sel & 0b100 == 0 {
        f.default_window
    } else {
        f.max_window
    };
    let ctx = ProofContext::new("GET", &f.htu, DPOP_ACCESS_TOKEN, &f.algs, window);

    let signing_input = format!("{}.{}", f.head, b64(payload));
    let proof = format!(
        "{signing_input}.{}",
        b64(&rsa_sign(signing_input.as_bytes()))
    );

    match verify_proof(&proof, &ctx, now) {
        Ok(v) => {
            assert!(!v.jti().is_empty() && v.jti().len() <= MAX_JTI_LEN);
            let lo = now.saturating_sub(window.past().as_secs());
            let hi = now.saturating_add(window.future().as_secs());
            assert!((lo..=hi).contains(&v.iat()), "窓外の iat を受理した");
            assert_eq!(
                v.thumbprint().as_str(),
                f.thumbprint,
                "jwk と異なる鍵に結合した"
            );
            // 独立に解いた payload との差分検査（htm・ath の誤受理がないこと）。
            let val: serde_json::Value =
                serde_json::from_slice(payload).expect("Ok なら payload は JSON");
            assert_eq!(val["htm"], "GET");
            assert_eq!(val["ath"], f.ath.as_str());
        }
        Err(e) => {
            let _ = e.to_string();
        }
    }
});
