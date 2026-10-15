//! fuzz target: `KeySet::from_json`（JWKS 文書の解析、イシュー #818）。
//!
//! 攻撃者が制御しうる JWKS 文書（JWKS エンドポイントの応答）を任意バイト列として投入し、
//! パニック・メモリ不正が起きないことと、DoS 上限（文書長・鍵数・同一 kid 鍵数）が
//! 守られることを検証する。`Ok` の中身の意味的正しさは `tests/jwks.rs` の責務。
//! 実行: `bash scripts/fuzz.sh`（`cargo +<pinned-nightly> fuzz run jwks_from_json`）。

#![no_main]

use fandhe_backend_plugin_jwt::{KeySet, MAX_JWKS_DOC_LEN, MAX_KEYS, MAX_KEYS_PER_KID};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // `from_json` は `&str` を受けるため UTF-8 でない入力は呼び出し側で弾かれる経路。
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    match KeySet::from_json(s) {
        Ok(ks) => {
            assert!(s.len() <= MAX_JWKS_DOC_LEN, "文書長上限を超えて受理された");
            assert!(ks.len() <= MAX_KEYS, "鍵数上限を超えて受理された");
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(s)
                && let Some(keys) = v.get("keys").and_then(|k| k.as_array())
            {
                for kid in keys
                    .iter()
                    .filter_map(|k| k.get("kid").and_then(|k| k.as_str()))
                {
                    assert!(
                        ks.keys_for(kid).count() <= MAX_KEYS_PER_KID,
                        "同一 kid の鍵数上限を超えて受理された"
                    );
                }
            }
        }
        Err(e) => {
            if s.len() > MAX_JWKS_DOC_LEN {
                assert_eq!(e, fandhe_backend_plugin_jwt::JwksError::DocumentTooLarge);
            }
            // 固定文言の Display がパニックしないこと。
            let _ = e.to_string();
        }
    }
});
