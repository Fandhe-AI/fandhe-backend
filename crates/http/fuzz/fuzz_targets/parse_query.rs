//! `fandhe_backend_http::query::parse_query` を叩く fuzz target（イシュー #306）。
//!
//! 対象は sans-IO な純関数（`&str -> Result<QueryPairs<'_>, QueryError>`）。
//! 任意バイト列を UTF-8 として解釈できた場合のみ `parse_query` へ渡し、
//! `Ok` を返した場合はさらに [`fandhe_backend_http::query::QueryPairs`] を
//! 最後まで走査する。「パニックしないこと」「無限ループ・過大メモリ消費に
//! 陥らないこと」のみを検証し、返り値の意味（分解結果の正しさ）はテスト
//! （`crates/http/src/query.rs` の `#[cfg(test)]`・doc test）側の責務とする。
//!
//! `scripts/fuzz.sh` から `cargo +<pinned-nightly> fuzz run parse_query` で
//! 実行する。

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // クエリ文字列は request-target の一部として UTF-8 相当を前提とする
    // （`RequestHead::query()` は `String` から split した `&str` を返す契約）。
    // 非 UTF-8 バイト列は入力から除外し、パーサ本体（`&str` 受け取り）の
    // 堅牢性検証に焦点を絞る。
    let Ok(query) = std::str::from_utf8(data) else {
        return;
    };
    // 戻り値・分解結果は問わない。パニック（index out of bounds 等）や
    // 無限ループ・過大メモリ消費が起きないことのみを libFuzzer に判定させる。
    if let Ok(pairs) = fandhe_backend_http::query::parse_query(query) {
        for _ in pairs {
            // イテレータの完走のみを確認する（各要素の値検証はしない）。
        }
    }
});
