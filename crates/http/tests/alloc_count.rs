//! `parse_request_head` のリクエストあたりヒープアロケーション回数が
//! ヘッダ本数 N に依存しない定数個であることを固定する常設テスト（イシュー
//! #591、性能改善ツリー #579 Phase 3）。
//!
//! `RequestHead` はヘッド部を 1 個の所有バッファ（`Box<str>`）としてコピーし、
//! method / target / 各ヘッダ名・値は当該バッファへの `Range<usize>` として
//! 保持する設計（`docs/design/zero-copy-request-head.md` 案 B）を採る。この
//! 設計の意義（N 非依存の定数 alloc）は本文書のプロファイル実測（設計文書
//! 5.1 節）に依拠しており、退行を防ぐには実測で常時検証する必要がある。
//! そのため本ファイルはカウンティング `GlobalAlloc` ラッパーを常設し、
//! N=1 と N=30 の 2 リクエストで alloc 回数の差分が一致すること（N 非依存）・
//! その回数が小さな定数であることを直接検証する。
//!
//! 本ファイル 1 個 = 1 テストに限定する。`#[global_allocator]` はプロセス
//! 全体で共有されるため、同一バイナリ内に他のテストが同居すると（cargo test
//! はデフォルトでテストをスレッド並列実行するため）他テストの割り込み alloc
//! が計測値に混入しうる。integration test は 1 ファイル = 1 バイナリのため、
//! このファイルにテストを 1 個だけ置くことで測定対象を隔離する。
//!
//! workspace lints（`unsafe_code = "warn"`、CI の `-D warnings` で実質
//! deny）は本テストファイルに限り `#[allow(unsafe_code)]` で明示的に緩める。
//! `std::alloc::GlobalAlloc` トレイトの実装は言語仕様上 `unsafe fn` を要する
//! ため回避できず、`crates/http` ライブラリ本体（`unsafe` 不使用方針、
//! `docs/design/zero-copy-request-head.md` 6.3 節）とは無関係な計測専用の
//! test バイナリに閉じたテストダブルである。各 `unsafe` ブロックは
//! `System`（標準システムアロケータ）の同名メソッドへ引数をそのまま転送する
//! だけで新たな不変条件を導入しない（安全性の根拠は各ブロック直前の説明を参照）。
#![allow(unsafe_code)]

use fandhe_backend_http::request::parse_request_head;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// `System` へすべて委譲しつつ `alloc` / `realloc` の呼び出し回数のみを数える
/// カウンティングアロケータ。
///
/// `dealloc` は数えない（本テストの関心は「1 リクエストの解析で何回確保が
/// 発生するか」であり解放回数ではない）。
struct CountingAllocator;

/// プロセス全体で共有する alloc 回数カウンタ。
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

// SAFETY: `alloc` / `dealloc` / `realloc` はすべて対応する `System`（標準の
// システムアロケータ）の同名メソッドへそのまま委譲するだけで、ポインタ・
// レイアウトの生成や解釈を独自に行わない。`GlobalAlloc` の不変条件
// （`dealloc` に渡すポインタ・レイアウトは対応する `alloc`/`realloc` 呼び出し
// と一致すること）は呼び出し元（Rust のアロケーション機構自体）が保証し、
// 本ラッパーはその引数をそのまま転送するのみで新たな不変条件を導入しない。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `layout` は呼び出し元から渡された値をそのまま転送する。
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` / `layout` は呼び出し元から渡された値をそのまま転送する。
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `ptr` / `layout` / `new_size` は呼び出し元から渡された値を
        // そのまま転送する。
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// `n` 本のヘッダを持つリクエストヘッドのバイト列を組み立てる。
fn build_request(n: usize) -> Vec<u8> {
    let mut buf = b"GET /path HTTP/1.1\r\n".to_vec();
    for i in 0..n {
        buf.extend_from_slice(format!("X-Header-{i}: value-{i}\r\n").as_bytes());
    }
    buf.extend_from_slice(b"\r\n");
    buf
}

/// `f` の実行前後で `ALLOC_COUNT` がいくつ増えたかを返す。
fn alloc_delta<F: FnOnce()>(f: F) -> usize {
    let before = ALLOC_COUNT.load(Ordering::Relaxed);
    f();
    ALLOC_COUNT.load(Ordering::Relaxed) - before
}

#[test]
fn parse_request_head_alloc_count_is_independent_of_header_count() {
    // ウォームアップ呼び出し: 初回のみ発生しうる遅延初期化コスト（アロケータ
    // 内部のスレッドローカルキャッシュ構築等）を計測対象から除外する。
    let warmup = build_request(1);
    let outcome = parse_request_head(&warmup).expect("warmup parse should succeed");
    drop(outcome);

    let small_request = build_request(1);
    let small_delta = alloc_delta(|| {
        let outcome = parse_request_head(&small_request).expect("parse should succeed");
        std::hint::black_box(&outcome);
    });

    let large_request = build_request(30);
    let large_delta = alloc_delta(|| {
        let outcome = parse_request_head(&large_request).expect("parse should succeed");
        std::hint::black_box(&outcome);
    });

    // N（ヘッダ本数）非依存性: N=1 と N=30 で alloc 回数が完全一致すること。
    // `RequestHead::buf`（`Box<str>` 1 回）+ `headers`（`Vec::with_capacity`
    // による事前確保 1 回）の定数 2 alloc/req 設計（イシュー #591）であれば、
    // ヘッダ本数が増えても再確保が発生しないため一致するはずである。
    assert_eq!(
        small_delta, large_delta,
        "alloc count must not depend on header count N (N=1: {small_delta}, N=30: {large_delta})"
    );

    // 小さな定数（設計文書 5.1 節の見込み値 2 に対し、実装差分の余地を見て
    // 上限 3 とする）であることも併せて固定する。
    assert!(
        small_delta <= 3,
        "alloc count per request should be a small constant, got {small_delta}"
    );
}
