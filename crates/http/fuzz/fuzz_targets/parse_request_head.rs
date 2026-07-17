//! `bf_http::request::parse_request_head` の構文解析層のみを叩く fuzz target（TASK-15.3-1、#87）。
//!
//! 対象は sans-IO な純関数（`&[u8] -> Result<ParseOutcome, ParseError>`）であり、
//! ソケット I/O・時間経過を伴わないため libFuzzer の任意バイト列をそのまま入力に
//! 使える（`crates/http/src/request.rs` の doc 冒頭コメント参照）。ここでは
//! 「パニックしないこと」「メモリ不正を起こさないこと」のみを検証し、返り値の
//! 意味（Complete/Incomplete/Err のどれが正しいか）はテスト（`request.rs` の
//! `#[cfg(test)]`・doc test）側の責務とする。
//!
//! `scripts/fuzz.sh` から `cargo +<pinned-nightly> fuzz run parse_request_head` で
//! 実行する。本実行（長時間スクリーニング・検出欠陥の修正）は #88（TASK-15.3-2）の
//! スコープ。

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 戻り値は問わない。パニック（integer overflow・index out of bounds 等）や
    // メモリ不正（ASan 計装で検出）が起きないことのみを libFuzzer に判定させる。
    let _ = bf_http::request::parse_request_head(data);
});
