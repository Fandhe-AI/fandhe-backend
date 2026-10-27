//! fuzz target: `dpop::jwk_thumbprint`・`PublicOrigin::new`・`PublicOrigin::expected_htu`
//! （イシュー #902、NFR-11）。
//!
//! 先頭バイトでモードを選ぶ。0 は単独 JWK の検査とサムプリント、1 は公開オリジン文字列の
//! 検証・正規化、2 は固定オリジンに対するリクエストパス（攻撃者が制御できる入力）から
//! 期待 `htu` を作る経路。検証するのはパニックしないことと、`Ok` 値が長さ上限・禁止文字の
//! 不在などの形を満たすこと。
//! 実行: `bash scripts/fuzz.sh`。

#![no_main]

#[path = "common/mod.rs"]
mod common;

use common::*;
use fandhe_backend_plugin_jwt::dpop::{
    jwk_thumbprint, PublicOrigin, MAX_HTU_LEN, MAX_ORIGIN_LEN, MAX_PROOF_LEN,
};
use libfuzzer_sys::fuzz_target;
use std::sync::LazyLock;

static ORIGIN: LazyLock<PublicOrigin> =
    LazyLock::new(|| PublicOrigin::new(DPOP_ORIGIN).expect("fixture origin"));

fn has_forbidden(s: &str) -> bool {
    s.chars()
        .any(|c| c.is_control() || c.is_whitespace() || !c.is_ascii() || matches!(c, '?' | '#'))
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let Ok(s) = std::str::from_utf8(rest) else {
        return;
    };
    match mode % 3 {
        0 => match jwk_thumbprint(s) {
            Ok(t) => {
                assert!(s.len() <= MAX_PROOF_LEN);
                assert_eq!(t.as_str().len(), 43);
                assert!(t
                    .as_str()
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
            }
            Err(e) => {
                let _ = e.to_string();
            }
        },
        1 => match PublicOrigin::new(s) {
            Ok(o) => {
                let o = o.as_str();
                assert!(o.len() <= MAX_ORIGIN_LEN);
                assert!(!has_forbidden(o) && !o.contains('@'));
                assert!(!o.ends_with('/'));
            }
            Err(e) => {
                let _ = e.to_string();
            }
        },
        _ => match ORIGIN.expected_htu(s) {
            Ok(h) => {
                let h = h.as_str();
                assert!(h.starts_with(ORIGIN.as_str()));
                assert!(h.len() <= MAX_HTU_LEN);
                assert!(!has_forbidden(h) && !h.contains('\\'));
            }
            Err(e) => {
                let _ = e.to_string();
            }
        },
    }
});
