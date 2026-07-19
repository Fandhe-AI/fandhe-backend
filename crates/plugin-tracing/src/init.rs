//! 非同期・バッファ済み I/O を既定とするグローバルサブスクライバの初期化
//! （TASK-10.1、AGENTS.md「規約: ミドルウェア非同期 I/O 必須化」）。

use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::fmt::MakeWriter;

/// [`init_tracing`] が受け取る出力先設定。
///
/// `Stdout`（既定）は運用時のログ収集パイプライン（`stdout` を収集する
/// コンテナランタイム等）を想定する。ファイル出力は本タスクのスコープ外
/// （必要になった場合は別 Issue で `tracing_appender::rolling` を追加する。
/// `.claude/rules/out-of-scope-tracking.md`）。
#[derive(Debug, Clone, Copy, Default)]
pub enum TracingOutput {
    /// 標準出力へ非同期・バッファ済みで書き込む（既定）。
    #[default]
    Stdout,
}

/// `tracing-appender` の `non_blocking` writer を既定とするグローバル
/// サブスクライバを初期化する。
///
/// # 契約（呼び出し元が守るべき不変条件）
///
/// - 戻り値の [`WorkerGuard`] は**プロセス終了までスコープを保持し続けること**。
///   `WorkerGuard` が drop されると non-blocking writer のバックグラウンド
///   フラッシュスレッドが停止し、以降の `tracing` 呼び出しがログを出力しなく
///   なる（`tracing-appender` の契約）。典型的には `main` 関数のローカル変数
///   （`let _guard = init_tracing(config);`）として保持する
/// - 非同期・バッファ済み writer は**バックプレッシャ時にイベントを破棄する
///   （lossy）**。有界チャネルが満杯の場合、`tracing` イベントは黙って失われる
///   （`tracing-appender::non_blocking` の既定動作）。この挙動は TASK-10.6（#90）で
///   決定的統合テスト（`crates/plugin-tracing/tests/backpressure.rs`）により
///   **実測済み**（推測ではない）: `lossy(true)`（本関数が使う既定）は満杯時に
///   呼び出し側をブロックせずドロップし、`NonBlocking::error_counter().
///   dropped_lines()` でドロップ件数を観測できる。高負荷時の欠落率実測・許容基準は
///   `benches/reports/task-10.6-tracing-backpressure.md` を参照。セキュリティ監査
///   イベント等、欠落を許容できないログは本関数の既定構成（lossy）の対象外とし、
///   同レポート「許容基準」節が示す代替設計（ブロッキング経路・同期書き込み）を
///   検討すること（AGENTS.md「ログ欠落の許容可否」節への回答）
/// - グローバルサブスクライバの登録（`tracing::subscriber::set_global_default`）は
///   プロセスにつき 1 回のみ成功する。複数回呼ぶと 2 回目以降は登録に失敗するが、
///   本関数はこれを panic として扱わず、既存の登録をそのまま使わせるため呼び出し元
///   へエラーを伝播しない（テスト等で複数回呼ばれても安全に無視できるようにする
///   ためであり、`.claude/rules/coding-rust.md` の「panic はライブラリ境界を
///   越えさせない」方針に沿う）
///
/// # Examples
///
/// ```no_run
/// use fandhe_backend_plugin_tracing::{init_tracing, TracingOutput};
///
/// // 実際のプロセスでは戻り値をプロセス終了まで保持する
/// // （doc test はグローバルサブスクライバ登録の副作用があるため no_run）。
/// let _guard = init_tracing(TracingOutput::Stdout);
/// ```
#[must_use = "戻り値の WorkerGuard を drop すると非同期 writer のフラッシュスレッドが停止し、以降のログが失われる（本関数 doc の契約セクションを参照）"]
pub fn init_tracing(output: TracingOutput) -> WorkerGuard {
    let (writer, guard) = make_non_blocking_writer(output);
    let subscriber = tracing_subscriber::fmt().with_writer(writer).finish();

    // 2 回目以降の呼び出しでの失敗（既にグローバルサブスクライバが登録済み）は
    // 意図的に無視する。上記契約セクションを参照。
    let _ = tracing::subscriber::set_global_default(subscriber);

    guard
}

/// `output` に応じた `non_blocking` writer を組み立てる。
///
/// 現状 `Stdout` のみだが、将来の出力先追加（ファイル出力等）時にこの関数だけを
/// 拡張すれば済むよう分離しておく（ファイル出力対応自体は TASK-10.6・#90 の
/// スコープ外、`benches/reports/task-10.6-tracing-backpressure.md`「スコープ外」節）。
fn make_non_blocking_writer(
    output: TracingOutput,
) -> (
    impl for<'a> MakeWriter<'a> + Send + Sync + 'static,
    WorkerGuard,
) {
    match output {
        TracingOutput::Stdout => {
            let (non_blocking, guard): (NonBlocking, WorkerGuard) =
                tracing_appender::non_blocking(std::io::stdout());
            (non_blocking, guard)
        }
    }
}
