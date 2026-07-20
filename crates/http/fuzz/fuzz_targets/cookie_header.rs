//! `fandhe_backend_http::cookie::parse_cookie_header` を叩く fuzz target
//! （イシュー #309）。
//!
//! 対象は sans-IO な純関数（`&str -> Result<Vec<(&str, &str)>, CookieError>`）。
//! 任意バイト列を UTF-8 として解釈できた場合のみ `parse_cookie_header` へ渡す。
//! 検証する不変条件は 2 つ:
//! 1. 任意入力でパニックしないこと（`.unwrap()`/`.expect()`・インデックス外
//!    アクセス等が発生しないこと）
//! 2. `Ok(pairs)` の場合、`pairs.len() <= MAX_COOKIE_COUNT` かつ各 name/value
//!    が入力 `cookie` 文字列内の連続部分文字列（ゼロコピー borrow）であること
//!    （返り値の意味の正しさは `crates/http/src/cookie.rs` の
//!    `#[cfg(test)]`・doc test 側の責務とし、本 target は境界条件・分割位置に
//!    よる panic 耐性のみに焦点を絞る）
//!
//! `scripts/fuzz.sh` から `cargo +<pinned-nightly> fuzz run cookie_header` で
//! 実行する。

#![no_main]

use fandhe_backend_http::cookie::{parse_cookie_header, MAX_COOKIE_COUNT};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Cookie ヘッダ値は前段の HTTP ヘッダパーサ（`crates/http/src/request.rs`）
    // を通過済みの `String` から得られる契約のため、非 UTF-8 バイト列は
    // 入力から除外し、パーサ本体（`&str` 受け取り）の堅牢性検証に焦点を絞る
    // （`parse_query` fuzz target と同方針）。
    let Ok(cookie) = std::str::from_utf8(data) else {
        return;
    };

    if let Ok(pairs) = parse_cookie_header(cookie) {
        // 不変条件 2a: 件数上限は返り値に対しても常に成立する。
        assert!(pairs.len() <= MAX_COOKIE_COUNT);

        for (name, value) in pairs {
            // 不変条件 2b: 返す各 name/value は入力 `cookie` のアドレス範囲内の
            // 部分文字列（ゼロコピー borrow）であること。ポインタ演算のみで
            // 判定し、入力バイト列との比較（値の正しさ）は検証しない。
            let cookie_range = cookie.as_ptr() as usize..cookie.as_ptr() as usize + cookie.len();
            let name_start = name.as_ptr() as usize;
            let value_start = value.as_ptr() as usize;
            assert!(cookie_range.contains(&name_start) || name.is_empty());
            assert!(cookie_range.contains(&value_start) || value.is_empty());
        }
    }
});
