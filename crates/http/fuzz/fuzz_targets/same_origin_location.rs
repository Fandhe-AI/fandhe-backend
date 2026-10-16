//! `fandhe_backend_http::location` の検証関数と `Response::see_other` を叩く
//! fuzz target（イシュー #828）。
//!
//! 対象は sans-IO な純関数 `validate_same_origin_location`。非 UTF-8 は除外する。
//! 検証する不変条件:
//! 1. 任意入力でパニックしないこと
//! 2. 検証関数と `SameOriginLocation::new` の合否が一致すること
//! 3. `Ok` のとき値は入力とバイト同一で、先頭 `/`・2 文字目が `/`/`\` でなく、
//!    `\` と制御文字を含まないこと
//! 4. `Ok` のとき `Response::see_other` の直列化が `Location: <入力>\r\n` を含み、
//!    ヘッダ部に行区切り以外の CR/LF が現れないこと（レスポンス分割なし）
//!
//! `scripts/fuzz.sh` から `cargo +<pinned-nightly> fuzz run same_origin_location` で実行する。

#![no_main]

use fandhe_backend_http::location::{validate_same_origin_location, SameOriginLocation};
use fandhe_backend_http::response::Response;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let validated = validate_same_origin_location(s);
    let built = SameOriginLocation::new(s);
    assert_eq!(validated.is_ok(), built.is_ok());
    let Ok(loc) = built else {
        return;
    };
    assert_eq!(loc.as_str(), s);
    assert!(s.starts_with('/'));
    assert!(!s[1..].starts_with(['/', '\\']));
    assert!(!s.contains('\\'));
    assert!(!s.chars().any(char::is_control));

    let bytes = Response::see_other(loc).serialize(true);
    let text = String::from_utf8(bytes).expect("serialized response is UTF-8");
    assert!(text.contains(&format!("Location: {s}\r\n")));
    let head_end = text.find("\r\n\r\n").expect("head terminator");
    let head = &text[..head_end];
    // CR と LF はすべて CRLF の対として現れる（行区切り以外の CR/LF なし）。
    assert_eq!(head.matches('\r').count(), head.matches('\n').count());
    assert_eq!(head.matches("\r\n").count(), head.matches('\n').count());
});
