//! `bf_http::chunked::ChunkedDecoder`（sans-IO chunked デコーダ）を叩く fuzz
//! target（イシュー #181）。
//!
//! `crates/http/src/chunked.rs` は「入力を 1 回で丸ごと渡す」経路（本体の
//! `crates/http/tests/http_flow.rs` 等）と、「ソケットから届いた分だけ複数回に
//! 分けて渡す」経路（[`bf_http::connection::read_request`] の実運用パス）の
//! 両方で使われる sans-IO 状態機械であり、状態遷移（chunk-size 行 → chunk-data
//! → chunk-data 直後の CRLF → 次チャンク／trailer）を跨いだ分割入力でパニック・
//! メモリ不正・処理結果の不一致が起きないかを検証する。
//!
//! 検証する不変条件は 2 つ:
//! 1. 復号後バイト列は常に [`bf_http::body::MAX_BODY_BYTES`] 以下
//!    （DoS 上限が実際に効いていること）
//! 2. 一括投入（経路 (a)）とインクリメンタル投入（経路 (b)）の結果
//!    （Complete / Incomplete / Err の別、復号済みバイト列）が一致すること
//!    （入力の分割位置によって挙動が変わらないこと = 状態機械の実装が
//!    分割耐性を持つこと）
//!
//! `scripts/fuzz.sh` から `cargo +<pinned-nightly> fuzz run chunked_decoder` で
//! 実行する。

#![no_main]

use bf_http::body::MAX_BODY_BYTES;
use bf_http::chunked::{ChunkedDecoder, DecodeOutcome};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&split_seed, body)) = data.split_first() else {
        return;
    };
    // 1..=256 バイトごとに分割する（0 バイト分割は無限ループになるため回避）。
    let split_width = usize::from(split_seed) + 1;

    // (a) 一括投入。`decode` は 1 回の呼び出し内で入力を尽きるまで処理する
    // ため、これが「分割なし」の基準経路になる。
    let mut decoder_a = ChunkedDecoder::new();
    let mut out_a = Vec::new();
    let result_a = decoder_a.decode(body, &mut out_a);

    // (b) split_width バイトごとに分割し、Incomplete の間は追い読みして
    // 再入力する（read_body_chunked の実運用パターンを模す）。
    let mut decoder_b = ChunkedDecoder::new();
    let mut out_b = Vec::new();
    let mut result_b = None;
    let mut cursor = 0usize;
    loop {
        let end = (cursor + split_width).min(body.len());
        let chunk = &body[cursor..end];
        let step = decoder_b.decode(chunk, &mut out_b);
        match &step {
            Ok(DecodeOutcome::Complete { .. }) => {
                result_b = Some(step);
                break;
            }
            Ok(DecodeOutcome::Incomplete { .. }) => {
                cursor = end;
                if cursor >= body.len() {
                    result_b = Some(step);
                    break;
                }
            }
            Err(_) => {
                result_b = Some(step);
                break;
            }
        }
    }
    let result_b = result_b.expect("loop always sets result_b before breaking");

    // 不変条件 1: DoS 上限（総復号量）は分割方法によらず必ず効く。
    assert!(out_a.len() as u64 <= MAX_BODY_BYTES);
    assert!(out_b.len() as u64 <= MAX_BODY_BYTES);

    // 不変条件 2: 一括投入とインクリメンタル投入で結果の種別・復号済み
    // バイト列が一致する（`consumed` は分割位置に依存し一致しなくて当然の
    // ため比較対象から除く）。
    match (&result_a, &result_b) {
        (Ok(DecodeOutcome::Complete { .. }), Ok(DecodeOutcome::Complete { .. }))
        | (Ok(DecodeOutcome::Incomplete { .. }), Ok(DecodeOutcome::Incomplete { .. })) => {
            assert_eq!(out_a, out_b);
        }
        (Err(err_a), Err(err_b)) => {
            assert_eq!(err_a, err_b);
        }
        _ => panic!("one-shot and incremental decoding diverged: {result_a:?} vs {result_b:?}"),
    }
});
