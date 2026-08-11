//! `parse_request_head` がヘッダ本数の事前確保（`Vec::with_capacity`）を
//! `MAX_HEADER_COUNT` 以下にクランプすることを固定する常設テスト（イシュー
//! #591 PR #602 レビュー指摘 P0 対応）。
//!
//! `header_count`（`\r\n` の単純カウント）は最大 16 KiB の未信頼なヘッダ部
//! から得られる値であり、`MAX_HEADER_COUNT`（100）の検査より前に
//! `Vec::with_capacity` へそのまま渡すと、最終的に `TooManyHeaders` として
//! 拒否されるリクエストであっても検査前に上限を大きく超える容量を毎回
//! 確保できてしまう（並行接続によるメモリ枯渇 DoS の増幅）。本テストは
//! ヘッダ本数 N が `MAX_HEADER_COUNT` を大きく超えても、確保バイト数が
//! N に比例せず一定であること（= `MAX_HEADER_COUNT` でクランプ済みである
//! こと）を `stats_alloc` の実測で直接検証する。
//!
//! `alloc_count.rs` と同じ理由（`#[global_allocator]` はプロセス全体で
//! 共有され、cargo test の並列実行下では他テストの alloc が計測値に混入
//! しうる）で、本ファイル 1 個 = 1 テストに限定する。
use fandhe_backend_http::request::{MAX_HEADER_COUNT, ParseError, parse_request_head};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;

#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// `n` 本の最小ヘッダ（`A:1\r\n`、5 バイト）を持つリクエストヘッドの
/// バイト列を組み立てる。`n` を大きくしても `MAX_HEADER_BYTES`（16 KiB）
/// 予算内に収まるサイズを選ぶ。`n > MAX_HEADER_COUNT` であれば
/// `parse_request_head` はループ内の `MAX_HEADER_COUNT` チェックで
/// `TooManyHeaders` を返して早期リターンし、`RequestHead::buf`（`Box<str>`）
/// は構築されない（`Ok` 経路にのみ到達するコード）。そのためこの入力での
/// 計測対象アロケーションは `headers: Vec<_>` の 1 回のみに絞られる。
fn build_request(n: usize) -> Vec<u8> {
    let mut buf = b"GET / HTTP/1.1\r\n".to_vec();
    for _ in 0..n {
        buf.extend_from_slice(b"A:1\r\n");
    }
    buf.extend_from_slice(b"\r\n");
    buf
}

/// `f` の実行前後の `bytes_allocated`（要求バイト数の総和）差分を返す。
fn bytes_allocated_delta<F: FnOnce()>(f: F) -> usize {
    let region = Region::new(ALLOCATOR);
    f();
    region.change().bytes_allocated
}

#[test]
fn header_vec_capacity_is_clamped_to_max_header_count() {
    // ウォームアップ: 初回のみ発生しうる遅延初期化コストを計測対象から除外する。
    let warmup = build_request(1);
    let _ = parse_request_head(&warmup);

    // N = MAX_HEADER_COUNT + 少数超過。ちょうど 100 件目で `TooManyHeaders`
    // として拒否される最小に近い攻撃入力。
    let just_over = MAX_HEADER_COUNT + 10;
    let small_request = build_request(just_over);
    assert!(
        small_request.len() < 16 * 1024,
        "test fixture must fit within MAX_HEADER_BYTES"
    );
    let small_delta = bytes_allocated_delta(|| {
        let outcome = parse_request_head(&small_request);
        assert_eq!(outcome, Err(ParseError::TooManyHeaders));
        std::hint::black_box(&outcome);
    });

    // N を 10 倍（1000 超）にしても、`MAX_HEADER_BYTES` 予算内に収まる範囲で
    // 攻撃入力を拡大する。修正前は `header_count`（≒N）へ比例して
    // `headers: Vec<_>` の確保バイト数が線形増加していたが、修正後は
    // `MAX_HEADER_COUNT` でクランプされるため増加しない。
    let far_over = just_over * 10;
    let large_request = build_request(far_over);
    assert!(
        large_request.len() < 16 * 1024,
        "test fixture must fit within MAX_HEADER_BYTES"
    );
    let large_delta = bytes_allocated_delta(|| {
        let outcome = parse_request_head(&large_request);
        assert_eq!(outcome, Err(ParseError::TooManyHeaders));
        std::hint::black_box(&outcome);
    });

    // `headers: Vec<_>` に起因する確保バイト数は N に依存せず一定である
    // こと（= `MAX_HEADER_COUNT` でクランプ済みであること）を検証する。
    // クランプ漏れがあれば `large_delta` は `small_delta` の約 10 倍（N 倍）
    // に膨れ上がり、この比較で検出できる。
    assert_eq!(
        small_delta, large_delta,
        "headers Vec allocation bytes must not scale with header count N \
         (N={just_over}: {small_delta} bytes, N={far_over}: {large_delta} bytes) \
         — Vec::with_capacity must be clamped to MAX_HEADER_COUNT before the size check"
    );

    // 具体的な上限も固定する: `MAX_HEADER_COUNT` 件分の `(Range<usize>,
    // Range<usize>)`（各 16 バイト × 2 = 32 バイト）を大きく超えないこと。
    let max_expected =
        MAX_HEADER_COUNT * std::mem::size_of::<(std::ops::Range<usize>, std::ops::Range<usize>)>();
    assert!(
        small_delta <= max_expected,
        "headers Vec allocation ({small_delta} bytes) exceeds MAX_HEADER_COUNT-bounded capacity ({max_expected} bytes)"
    );
}
