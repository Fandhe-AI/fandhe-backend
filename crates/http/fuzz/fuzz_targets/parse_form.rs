//! `fandhe_backend_http::form::parse_form` を叩く fuzz target（イシュー #308）。
//!
//! 対象は sans-IO な純関数（`&[u8] -> Result<Vec<(String, String)>, FormError>`）。
//! `parse_query`（イシュー #306）の fuzz target と同形式で、任意バイト列を
//! そのまま `parse_form` へ渡す。「パニックしないこと」「無限ループ・過大
//! メモリ消費に陥らないこと」のみを検証し、返り値の意味（分解結果・デコード
//! の正しさ）はテスト（`crates/http/src/form.rs` の `#[cfg(test)]`・doc test）
//! 側の責務とする。
//!
//! `scripts/fuzz.sh` から `cargo +<pinned-nightly> fuzz run parse_form` で
//! 実行する。

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // `parse_form` はボディを `&[u8]` で直接受け取る契約（UTF-8 検証は
    // 関数内部で行う）ため、`parse_query` target と異なり入力の事前フィルタは
    // 不要。戻り値・分解結果は問わない。
    let _ = fandhe_backend_http::form::parse_form(data);
});
