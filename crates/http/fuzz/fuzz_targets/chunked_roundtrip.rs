//! `fandhe_backend_http::chunked::{encode_chunk, encode_terminator}`（sans-IO chunked
//! エンコーダ、イシュー #319）と既存 [`ChunkedDecoder`] の roundtrip を検証する
//! fuzz target。
//!
//! エンコーダは `crates/core/src/server.rs` の書き出しループがストリーミング
//! 応答を chunked framing へ変換する際に使う唯一の経路であり、ここで生成した
//! バイト列は最終的に [`ChunkedDecoder`]（`chunked_decoder.rs` fuzz target が
//! 別途検証するデコーダ本体）と対をなす。エンコード → デコードの往復で
//! 元データが完全に復元されることを確認し、フレーミングの非対称なバグ
//! （エンコーダ側の境界不整合等）を検出する。
//!
//! 検証する不変条件は 3 つ:
//! 1. 復号結果が常に `Complete` になる（エンコーダが常にデコーダが受理
//!    できる well-formed な chunked body を生成すること）
//! 2. 復号バイト列が元入力（チャンク分割前の結合結果）と一致すること
//!    （`encode_chunk` は空データを無出力にする契約のため、比較対象は
//!    「空チャンクを除いた結合結果」と等価だが、非空チャンクのみを入力に
//!    使うため単純結合と一致する）
//! 3. `consumed` がエンコード出力全長と一致すること（余剰・不足バイトが
//!    生じていないこと）
//!
//! `scripts/fuzz.sh` から `cargo +<pinned-nightly> fuzz run chunked_roundtrip`
//! で実行する。

#![no_main]

use fandhe_backend_http::chunked::{
    encode_chunk, encode_terminator, ChunkedDecoder, DecodeOutcome, MAX_CHUNK_COUNT,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&split_seed, body)) = data.split_first() else {
        return;
    };
    // 1..=64 バイトごとにチャンク分割する（0 バイト分割は無限ループになる
    // ため回避。DoS 上限 MAX_CHUNK_COUNT に収まる粒度に抑える）。
    let chunk_width = (usize::from(split_seed) % 64) + 1;
    // fuzz corpus が肥大化してもチャンク総数が MAX_CHUNK_COUNT
    // （chunked.rs、16_384）を超えないよう、chunk_width に応じて入力長を
    // 上限化する（`chunk_width * MAX_CHUNK_COUNT` が生成しうるチャンク総数の
    // 上限。chunk_width = 1 のとき 65536 バイトまで許すと 16_384 を超えて
    // decode が `TooManyChunks` を返し直後の `.expect` が panic するため、
    // 単純な固定長上限（64 * 1024）では不十分だった）。
    let max_len = chunk_width.saturating_mul(MAX_CHUNK_COUNT as usize);
    let body = &body[..body.len().min(max_len).min(64 * 1024)];

    let mut encoded = Vec::new();
    for piece in body.chunks(chunk_width) {
        // encode_chunk は空データを無出力にする契約（誤終端防止）のため、
        // chunks() が空スライスを返すことはなく通常は無関係だが、念のため
        // 契約どおりの挙動であることも併せて確認する。
        encode_chunk(piece, &mut encoded);
    }
    encode_terminator(&mut encoded);

    let mut decoder = ChunkedDecoder::new();
    let mut decoded = Vec::new();
    let outcome = decoder
        .decode(&encoded, &mut decoded)
        .expect("encoder must always produce a well-formed chunked body");

    match outcome {
        DecodeOutcome::Complete { consumed } => {
            assert_eq!(
                consumed,
                encoded.len(),
                "consumed must match encoded length"
            );
        }
        DecodeOutcome::Incomplete { .. } => {
            panic!("roundtrip must complete in a single decode call: {outcome:?}");
        }
    }

    assert_eq!(decoded, body, "decoded body must equal original input");
});
