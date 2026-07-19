//! 構文解析（`parse_request_head`）が `Complete` を返した `RequestHead` を、意味
//! 解釈層（`body_length` / `should_keep_alive`）まで通すパイプライン fuzz
//! target（TASK-15.3-1、#87）。
//!
//! `parse_request_head` 単体（`parse_request_head.rs` target）では構文解析層の
//! パニック・メモリ不正しか検出できない。`crates/http/src/body.rs` の
//! `Content-Length` 解析（オーバーフロー・重複判定）・`crates/http/src/connection.rs`
//! の `Connection` ヘッダトークン走査は、構文的に妥当なヘッダ列を前提に動く別の
//! 純関数であり、そこに固有のパニック要因（例: 巨大な数値文字列の parse）が
//! ないかを本 target で検証する。
//!
//! `scripts/fuzz.sh` から `cargo +<pinned-nightly> fuzz run head_semantics` で
//! 実行する。本実行は #88（TASK-15.3-2）のスコープ。

#![no_main]

use fandhe_backend_http::body::body_length;
use fandhe_backend_http::connection::should_keep_alive;
use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(ParseOutcome::Complete { head, .. }) = parse_request_head(data) else {
        // 構文解析が Incomplete/Err の場合、意味解釈層には到達しない契約
        // （crates/http/src/request.rs 冒頭コメント参照）。
        return;
    };

    // 戻り値の Ok/Err は問わない。パニック・メモリ不正が起きないことのみを検証する。
    let _ = body_length(&head);
    let _ = should_keep_alive(&head);
});
