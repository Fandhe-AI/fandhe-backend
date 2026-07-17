//! TASK-10.6（#90）: 非同期 writer（`tracing_appender::non_blocking`）の
//! バックプレッシャー挙動（チャネル満杯時にドロップするかブロックするか）と
//! ログ欠落の勘定整合性を実測する統合テスト。
//!
//! PoC-10（`docs/spec/03-poc/observability-tracing/README.md`）はこの挙動を
//! 「推測」のまま残しており、`crates/plugin-tracing/src/init.rs` の doc comment も
//! 従来は lossy 契約を実測せず記述していた。本テストはその実測根拠を提供する。
//!
//! 新規依存は追加しない。`tracing-appender` の公開 API
//! （`NonBlockingBuilder::lossy` / `buffered_lines_limit` /
//! `NonBlocking::error_counter`）のみを使い、「書き込みをゲートで人為的に停止できる
//! writer」（[`GateWriter`]、std のみで実装）を渡してチャネルを満杯にする。

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tracing_appender::non_blocking::NonBlockingBuilder;

/// 「止められる」`Write` 実装。
///
/// ゲートが閉じている間は `write` 呼び出し（= 非同期 writer のバックグラウンド
/// スレッドが `channel` から取り出したメッセージの書き込み）をブロックし、
/// チャネルを人為的に満杯へ追い込めるようにする。
///
/// `tracing_appender::non_blocking` の worker スレッドから呼ばれる想定
/// （`NonBlockingBuilder::finish` に渡す writer）。
#[derive(Clone)]
struct GateWriter {
    /// ゲート通過（= 実際に書き込んだ）行数。欠落率の勘定検証に使う。
    written: Arc<AtomicUsize>,
    /// `write` に入った（ゲート待ちに入った）回数。worker スレッドが最初の
    /// 呼び出しでブロックしたことをポーリング確認するために使う。
    entered: Arc<AtomicUsize>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl GateWriter {
    fn new() -> Self {
        Self {
            written: Arc::new(AtomicUsize::new(0)),
            entered: Arc::new(AtomicUsize::new(0)),
            gate: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    /// ゲートを開放し、以降の `write` 呼び出しをブロックしないようにする。
    fn open(&self) {
        let (lock, cvar) = &*self.gate;
        let mut is_open = lock.lock().expect("gate mutex poisoned");
        *is_open = true;
        cvar.notify_all();
    }

    fn written(&self) -> usize {
        self.written.load(Ordering::SeqCst)
    }

    fn entered_count(&self) -> usize {
        self.entered.load(Ordering::SeqCst)
    }

    /// `entered_count` が 1 以上になるまで待つ（worker スレッドが最初の
    /// `write` でゲート待ちに入ったことの確認）。
    ///
    /// これを待ってから追加の送出を行うことで、「worker が停止した状態で
    /// チャネルへ送出する」という前提の成立をタイミング競合なしに保証する
    /// （待機自体はポーリングだが、以降のアサーションは決定的になる）。
    fn wait_worker_blocked(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        while self.entered_count() == 0 {
            if start.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }
}

impl Write for GateWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let (lock, cvar) = &*self.gate;
        let mut is_open = lock.lock().expect("gate mutex poisoned");
        while !*is_open {
            is_open = cvar.wait(is_open).expect("gate mutex poisoned");
        }
        self.written.fetch_add(1, Ordering::SeqCst);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// lossy=true（既定）はチャネル満杯時に**ドロップし呼び出し側をブロックしない**。
///
/// `fmt` レイヤを経由せず `NonBlocking::write_all` を直接呼ぶことで「1 write
/// 呼び出し = 1 チャネルメッセージ」の対応を保証し、勘定を決定的にする。
#[test]
fn lossy_true_drops_excess_events_without_blocking_caller() {
    const LIMIT: usize = 16;
    const TOTAL_WRITES: usize = 1_000;

    let gate = GateWriter::new();
    let (mut non_blocking, guard) = NonBlockingBuilder::default()
        .lossy(true)
        .buffered_lines_limit(LIMIT)
        .finish(gate.clone());
    let error_counter = non_blocking.error_counter();

    // 1 行目で worker スレッドをゲート待ちへ落とし込む（以降の送出がチャネルを
    // 満杯に近づけていく前提を成立させる）。
    non_blocking
        .write_all(b"line\n")
        .expect("write_all must not fail for NonBlocking");
    assert!(
        gate.wait_worker_blocked(Duration::from_secs(5)),
        "worker スレッドが GateWriter::write で停止しなかった（テスト前提が崩れている）"
    );

    let start = Instant::now();
    for _ in 1..TOTAL_WRITES {
        non_blocking
            .write_all(b"line\n")
            .expect("write_all must not fail for NonBlocking");
    }
    let elapsed = start.elapsed();
    // lossy=true は満杯時にドロップしブロックしない契約。呼び出し側スレッドは
    // 短時間で完走するはず（CI 環境のスケジューリング遅延を考慮した余裕のある上限）。
    assert!(
        elapsed < Duration::from_secs(5),
        "呼び出し側スレッドが長時間ブロックした（lossy=true の契約違反の疑い）: {elapsed:?}"
    );

    gate.open();
    drop(guard); // WorkerGuard drop でフラッシュを待つ。

    let written = gate.written();
    let dropped = error_counter.dropped_lines();

    assert!(
        written < TOTAL_WRITES,
        "lossy=true でもドロップが発生しなかった（テスト条件を見直す必要がある）: written={written}"
    );
    assert!(
        dropped > 0,
        "dropped_lines が 0（ドロップが実測できていない）"
    );
    assert_eq!(
        written + dropped,
        TOTAL_WRITES,
        "勘定が不整合（欠落率算出の妥当性根拠が崩れる）: written={written} dropped={dropped} total={TOTAL_WRITES}"
    );
}

/// lossy=false はチャネル満杯時に**送出側スレッドをブロックし欠落ゼロを保つ**。
#[test]
fn lossy_false_blocks_caller_and_preserves_all_events() {
    const LIMIT: usize = 16;
    const TOTAL_WRITES: usize = 1_000;

    let gate = GateWriter::new();
    let (non_blocking, guard) = NonBlockingBuilder::default()
        .lossy(false)
        .buffered_lines_limit(LIMIT)
        .finish(gate.clone());
    let error_counter = non_blocking.error_counter();

    // 0 = 送出中、1 = 完走。送出スレッドを join せずに完走有無をポーリングするための
    // 完了フラグ（join はブロックするため、途中経過をタイムアウト付きで確認できない）。
    let finished = Arc::new(AtomicUsize::new(0));
    let finished_writer = finished.clone();
    let handle = std::thread::spawn(move || {
        let mut writer = non_blocking;
        for _ in 0..TOTAL_WRITES {
            writer
                .write_all(b"line\n")
                .expect("write_all must not fail for NonBlocking (lossy=false)");
        }
        finished_writer.store(1, Ordering::SeqCst);
    });

    assert!(
        gate.wait_worker_blocked(Duration::from_secs(5)),
        "worker スレッドが GateWriter::write で停止しなかった（テスト前提が崩れている）"
    );

    // ゲート停止中は送出スレッドが完走しないはず（余裕のある待機時間で確認、flaky 回避）。
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "lossy=false なのに送出スレッドが早期に完走した（バックプレッシャーが効いていない）"
    );

    gate.open();
    handle.join().expect("sender thread must not panic");
    drop(guard); // フラッシュを待つ。

    let written = gate.written();
    let dropped = error_counter.dropped_lines();

    assert_eq!(
        written, TOTAL_WRITES,
        "lossy=false なのに欠落が発生した: written={written} total={TOTAL_WRITES}"
    );
    assert_eq!(
        dropped, 0,
        "lossy=false なのに dropped_lines が 0 でない: {dropped}"
    );
}

/// `tracing_subscriber::fmt` レイヤ経由のイベントが non-blocking writer に
/// 到達することを確認する実経路の煙テスト（チャネル勘定そのものは上記 2 テストが担う）。
///
/// グローバルサブスクライバ登録はプロセスにつき 1 回のみ成功する制約
/// （`crates/plugin-tracing/src/init.rs` の doc comment 参照）があるため、
/// `tracing::subscriber::with_default` でスコープ付きに束ねて他テストと干渉しないようにする。
#[test]
fn traced_events_reach_writer_through_fmt_layer() {
    let gate = GateWriter::new();
    gate.open(); // 到達確認のみが目的のため、ゲートは開けたまま使う。

    let (non_blocking, guard) = tracing_appender::non_blocking(gate.clone());
    let subscriber = tracing_subscriber::fmt().with_writer(non_blocking).finish();

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "backpressure_probe", "smoke test event");
    });

    // 非同期 writer のバックグラウンドスレッドがフラッシュするまで短時間ポーリングする。
    let deadline = Instant::now() + Duration::from_secs(5);
    while gate.written() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(guard);

    assert!(
        gate.written() > 0,
        "fmt レイヤ経由のイベントが writer に到達しなかった"
    );
}
