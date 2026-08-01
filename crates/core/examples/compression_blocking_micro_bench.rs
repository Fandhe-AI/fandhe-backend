//! イシュー #468 のしきい値決定・ストリーミング適用要否判定に使った
//! マイクロベンチ計測専用バイナリ（production コード変更を含まない、
//! `examples/tracing_nfr.rs` 等と同型の計測専用 example パターン）。
//!
//! `fandhe_backend_plugin_compression::compress_body`（gzip 圧縮本体）の
//! 所要時間を body サイズ別に計測し、`tokio::task::spawn_blocking` 1 回の
//! ディスパッチ往復コスト（no-op クロージャ）と比較する。結果は
//! `benches/reports/issue468-compression-blocking.md` へ転記し、
//! `docs/design/plugin-boundary.md` 5.10.7 節の採否根拠として記録する
//! （`benches/compression-blocking-bench.sh` から呼ばれる）。
//!
//! 実行方法:
//! ```text
//! $ cargo run --release --example compression_blocking_micro_bench \
//!     -p fandhe-backend-core --features compression
//! ```

use std::time::Instant;

use fandhe_backend_plugin_compression::compress_body;

/// 指定サイズの疑似 body（実運用の JSON/text 応答に近い、繰り返しパターン
/// による中程度の圧縮率を持つデータ）を生成する。
fn make_body(size: usize) -> Vec<u8> {
    let unit = b"{\"id\":1,\"name\":\"issue-468-bench\",\"note\":\"payload\"} ";
    unit.iter().copied().cycle().take(size).collect()
}

/// `iterations` 回 `f` を実行し、中央値（マイクロ秒）を返す（単発計測の
/// 外れ値を避けるため。`benches/README.md` の「複数回計測/中央値評価」
/// 規約に合わせる）。
fn median_micros(iterations: usize, mut f: impl FnMut()) -> f64 {
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        f();
        samples.push(start.elapsed().as_secs_f64() * 1_000_000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    println!("# イシュー #468 マイクロベンチ結果");
    println!();
    println!("## compress_body（gzip 圧縮本体、インライン実行時間）");
    println!();
    println!("| body サイズ | 中央値（µs、100 回） |");
    println!("|---|---|");
    for size in [
        1024usize, 4096, 16384, 32768, 65536, 131072, 262144, 1048576,
    ] {
        let body = make_body(size);
        let micros = median_micros(100, || {
            let _ = compress_body(&body);
        });
        println!("| {size} バイト | {micros:.1} |");
    }

    println!();
    println!("## spawn_blocking ディスパッチ往復コスト（no-op クロージャ、1000 回）");
    println!();
    let mut async_samples = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let start = Instant::now();
        let _ = tokio::task::spawn_blocking(|| ()).await;
        async_samples.push(start.elapsed().as_secs_f64() * 1_000_000.0);
    }
    async_samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let dispatch_median = async_samples[async_samples.len() / 2];
    println!("| 中央値（µs） |");
    println!("|---|");
    println!("| {dispatch_median:.1} |");
}
