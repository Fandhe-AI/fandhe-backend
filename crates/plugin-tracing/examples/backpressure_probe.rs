//! TASK-10.6（#90）: 既定構成（lossy=true・`buffered_lines_limit` 既定値）の
//! non-blocking writer に高負荷でイベントを送出し、欠落率を定量計測するプローブ。
//!
//! example のためライブラリ本体・依存ツリーへ影響しない
//! （`crates/plugin-tracing/tests/backpressure.rs` の決定的テストとは役割が異なり、
//! こちらは実測値そのものの収集が目的）。`benches/tracing-backpressure-bench.sh` から
//! 複数回・複数負荷段階で呼び出され、1 行の JSON を stdout に出す契約
//! （レポート転記・複数回計測の自動化のため）。
//!
//! 入力は環境変数のみ:
//! - `BF_TRACING_PROBE_OUTPUT`（必須）: 非同期 writer の書き込み先ファイルパス
//! - `BF_TRACING_PROBE_EVENTS`（既定 100000）: 総送出イベント数
//! - `BF_TRACING_PROBE_THREADS`（既定 1）: 送出スレッド数（EVENTS を均等分配）
//! - `BF_TRACING_PROBE_LINE_BYTES`（既定 64）: 1 イベントのメッセージ本体の目標バイト長
//!
//! 出力先パスは `mktemp -d` 等で用意した一時ディレクトリを渡す運用を想定し、本体は
//! リポジトリへコミットしない（`.claude/rules/security.md` シークレット・一時生成物の
//! 混入防止）。

use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::time::Instant;

use tracing_appender::non_blocking::NonBlockingBuilder;

/// 環境変数を正の `usize` として読む。未指定・不正値・0 は `default` にフォールバックする。
fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

/// 目標バイト長に近づけたメッセージ本体を組み立てる。
///
/// ASCII 固定文字の繰り返しのみで外部入力を含まないため、ログインジェクション・PII
/// 混入の懸念がない（`.claude/rules/security.md`）。
fn build_message(target_bytes: usize) -> String {
    "x".repeat(target_bytes.max(1))
}

/// 計測プローブの本体。本 example の `unwrap`/`expect` は、計測条件が満たせない場合に
/// 誤った結果を静かに返すより即座に失敗させる方が安全なため、ここに限定して用いる
/// （ライブラリコードでは `.claude/rules/coding-rust.md` により避ける方針）。
fn main() {
    let output_path = env::var("BF_TRACING_PROBE_OUTPUT")
        .expect("BF_TRACING_PROBE_OUTPUT（出力先ファイルパス）を指定してください");
    let total_events = env_usize("BF_TRACING_PROBE_EVENTS", 100_000);
    let threads = env_usize("BF_TRACING_PROBE_THREADS", 1).max(1);
    let line_bytes = env_usize("BF_TRACING_PROBE_LINE_BYTES", 64);

    // 出力先ディレクトリの存在を事前検証してから使う（存在しないディレクトリ等での
    // 原因不明な失敗を防ぐ。呼び出し元がパスを検証してから使う契約は計画§3.2 参照）。
    let output_dir = std::path::Path::new(&output_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty());
    if let Some(dir) = output_dir {
        assert!(
            dir.is_dir(),
            "出力先ディレクトリが存在しません: {}",
            dir.display()
        );
    }
    let file = File::create(&output_path)
        .unwrap_or_else(|e| panic!("出力先ファイルを作成できません: {output_path}: {e}"));

    // 既定構成（lossy=true・buffered_lines_limit 既定値）をそのまま使う。
    // init_tracing（crates/plugin-tracing/src/init.rs）が使う
    // `tracing_appender::non_blocking` の既定と同一（NonBlockingBuilder::default() は
    // lossy=true・DEFAULT_BUFFERED_LINES_LIMIT）であることを明示するため、あえて
    // ビルダー経由で組み立てる。
    let (non_blocking, guard) = NonBlockingBuilder::default().finish(file);
    let error_counter = non_blocking.error_counter();

    let subscriber = tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_target(false)
        .finish();
    // 本プローブは単一プロセス・単発実行のため、グローバル登録の 1 回制約
    // （init.rs の doc comment 参照）は問題にならない。
    tracing::subscriber::set_global_default(subscriber)
        .expect("グローバルサブスクライバの登録に失敗しました");

    let message = build_message(line_bytes);
    let per_thread = total_events / threads;
    let remainder = total_events % threads;

    let start = Instant::now();
    std::thread::scope(|scope| {
        for t in 0..threads {
            let count = per_thread + usize::from(t < remainder);
            let msg = message.as_str();
            scope.spawn(move || {
                for _ in 0..count {
                    tracing::info!(message = msg);
                }
            });
        }
    });
    let emit_elapsed = start.elapsed();

    // WorkerGuard drop でフラッシュを待つ（drop 完了後に初めて書き込み済み行数が確定する）。
    drop(guard);

    let dropped = error_counter.dropped_lines();
    let written = {
        let f = File::open(&output_path).expect("出力先ファイルを再オープンできません");
        BufReader::new(f).lines().count()
    };

    let elapsed_secs = emit_elapsed.as_secs_f64();
    // タイマー分解能未満で完了した場合（elapsed_secs が 0.0）、素朴に割ると
    // `events_per_sec` が `f64::INFINITY` になり `{:.2}` フォーマットで `inf` という
    // 非 JSON トークンを出力してしまい、下流の `benches/tracing-backpressure-bench.sh`
    // の `jq` パースが失敗する（Cursor Bugbot 指摘）。分母に極小の下限（1 ナノ秒相当）を
    // 設けて有限値に丸め込み、常に valid JSON を出す契約を維持する。`elapsed_secs`
    // フィールド自体は実測値（0.000000 のまま）を維持し、レート算出にのみ下限を適用する。
    let elapsed_secs_for_rate = elapsed_secs.max(1e-9);
    let events_per_sec = total_events as f64 / elapsed_secs_for_rate;
    let drop_rate_pct = if total_events > 0 {
        (dropped as f64 / total_events as f64) * 100.0
    } else {
        0.0
    };

    // 機械可読な 1 行 JSON を stdout に出す（benches/tracing-backpressure-bench.sh が
    // jq でパースする契約）。
    println!(
        "{{\"emitted\":{total_events},\"written\":{written},\"dropped_lines\":{dropped},\"drop_rate_pct\":{drop_rate_pct:.6},\"threads\":{threads},\"line_bytes\":{line_bytes},\"elapsed_secs\":{elapsed_secs:.6},\"events_per_sec\":{events_per_sec:.2}}}"
    );
}
