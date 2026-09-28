//! `Server::diagnostics`（イシュー #720）の統合テスト。
//!
//! `crates/core/tests/graceful_shutdown.rs` / `rebind.rs` の grace 超過強制
//! クローズシナリオを再利用し、その経路で発火する診断イベントが利用側の
//! シンクへ確かに届く（かつ既定シンクの stderr 出力を置き換えられる）ことを
//! 検証する:
//! - `shutdown_grace_exceeded_reaches_custom_sink`: 最終 graceful shutdown の
//!   grace 超過で `DiagnosticEvent::ShutdownGraceExceeded` が届く
//! - `rebind_drain_grace_exceeded_reaches_custom_sink`: rebind 旧世代 drain の
//!   grace 超過で `DiagnosticEvent::RebindDrainGraceExceeded` が届く
//! - `custom_sink_suppresses_default_stderr_output` /
//!   `default_sink_prints_to_stderr_when_unregistered`: 子プロセス方式で
//!   stderr の実バイト列を検証する（libtest の出力捕捉はスレッドローカルで
//!   tokio ワーカー・detached タスクからの `eprintln!` を捉えられないため、
//!   in-process では判定できない。`std::process::Command::new(current_exe())`
//!   で自分自身を再実行し、対象テストのみを `--exact --nocapture` 指定で
//!   走らせて子プロセスの stderr を観測する）
//! - `shutdown_grace_exceeded_sink_blocking_does_not_delay_run_until_return`:
//!   PR #748 レビュー P1 対応の回帰テスト。契約違反（ブロッキング I/O）の
//!   シンクを登録しても `run_until` が `shutdown_grace_period + ε` 以内に
//!   戻ること（`docs/design/diagnostics-sink.md` 9 節参照）
//! - `shutdown_grace_exceeded_notified_even_when_run_until_is_last_await_on_current_thread_runtime`:
//!   PR #748 Bugbot 指摘の回帰テスト。`run_until` を `current_thread`
//!   ランタイムの `block_on` へ渡す最後の await として呼び出し（典型的な
//!   `graceful_shutdown` サンプルの使い方）、`block_on` から戻った直後に
//!   ランタイムがそれ以上何もポーリングしない状況でも、通知が失われず
//!   届くこと
//! - `default_sink_notifies_even_when_process_exits_immediately_after_run_until_returns`:
//!   PR #748 codex/review 指摘対応の回帰テスト。既定シンクは
//!   `run_until` の返却前に**同期的に**通知を確定させる（`SHUTDOWN_NOTIFY_
//!   FLUSH_WAIT` の有界待機にすら依存しない）ことを、`run_until` が
//!   `Ok(())` を返した直後に子プロセスを `std::process::exit` で即座に
//!   終了させる最も厳しい条件で検証する（`docs/design/diagnostics-sink.md`
//!   9 節参照）

use fandhe_backend_core::{DiagnosticEvent, Handler, Server};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::HandlerFuture;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::time::timeout;

/// 固定 200 応答を返すだけのトイハンドラ（`graceful_shutdown.rs` /
/// `rebind.rs` と同一パターン）。
struct FixedHandler;
impl Handler for FixedHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> HandlerFuture {
        Box::pin(std::future::ready(Response::empty(200)))
    }
}

/// 受け取った [`DiagnosticEvent`] の `Display` 文字列を蓄積するだけのテスト用
/// シンク。`crate::diagnostics::Diagnostics` の契約（ブロッキング I/O 禁止・
/// panic 禁止）を守った最小実装。
#[derive(Clone, Default)]
struct RecordingSink {
    events: Arc<Mutex<Vec<String>>>,
}

impl fandhe_backend_core::Diagnostics for RecordingSink {
    fn report(&self, event: &DiagnosticEvent<'_>) {
        self.events.lock().unwrap().push(event.to_string());
    }
}

/// 受け入れ基準(a)(b): 最終 graceful shutdown の grace 超過で
/// `ShutdownGraceExceeded` が登録済みシンクへ届くこと
/// （`graceful_shutdown.rs::shutdown_force_closes_after_grace_period` と
/// 同一の grace 超過シナリオを利用）。
#[tokio::test]
async fn shutdown_grace_exceeded_reaches_custom_sink() {
    let grace = Duration::from_millis(100);
    let sink = RecordingSink::default();
    let server = Server::new()
        .handler(FixedHandler)
        .shutdown_grace_period(grace)
        .diagnostics(sink.clone());
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let run_task = tokio::spawn(async move {
        bound
            .run_until(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    // アイドル接続（リクエストは送らず張ったままにする）で permit を
    // 占有させ、grace 超過の強制クローズを確実に起こす
    // （`graceful_shutdown.rs` と同一手法）。
    let _idle_stream = TcpStream::connect(addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    shutdown_tx.send(()).unwrap();

    timeout(grace + Duration::from_secs(5), run_task)
        .await
        .expect("run_until は grace 超過後も有界時間内に戻るはず")
        .expect("run_until タスクが panic しないこと")
        .expect("run_until は Ok(()) を返すはず");

    // `ShutdownGraceExceeded` の通知は `run_until` の返却経路から OS スレッド
    // （`std::thread::Builder::spawn`）へ切り離され、完了は有界時間しか待たないため、
    // `run_until` が `Ok(())` を返した時点でシンクへ届いている保証はない
    // （`RebindDrainGraceExceeded` と同じく非同期に届く。9 節参照）。
    // `rebind_drain_grace_exceeded_reaches_custom_sink` と同型の有界ポーリングで待つ。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if !sink.events.lock().unwrap().is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "ShutdownGraceExceeded は有界時間内に届くはず"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let events = sink.events.lock().unwrap();
    assert_eq!(
        events.len(),
        1,
        "ShutdownGraceExceeded がちょうど 1 件届くはず（実際: {events:?}）"
    );
    assert!(
        events[0].contains("graceful shutdown の猶予期間"),
        "登録済みシンクに ShutdownGraceExceeded の本文が届くはず（実際: {events:?}）"
    );
}

/// PR #748 レビュー P1 対応の回帰テスト: `Diagnostics::report` の
/// 「ブロッキング I/O を行わない」契約に違反するシンク（同期的に長時間
/// ブロックする）を登録しても、`ShutdownGraceExceeded` の通知が `run_until`
/// の返却経路から detached タスクへ切り離されているため、`run_until` 自体は
/// `shutdown_grace_period + ε` 以内に戻ること
/// （`docs/design/diagnostics-sink.md` 9 節・`docs/design/graceful-shutdown.md`
/// の「grace + ε 以内に必ず戻る」契約）。ブロックしたシンクを別スレッドで
/// 実行させて `run_until` 側の待機に影響させないため
/// `flavor = "multi_thread"` を使う。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_grace_exceeded_sink_blocking_does_not_delay_run_until_return() {
    let grace = Duration::from_millis(100);
    // grace より十分長い（この時間内に run_until が戻れば、通知の完了を
    // 待たずに戻っている証拠になる）。
    let block_for = Duration::from_secs(2);

    /// `report` 内で同期的にスリープし続ける契約違反シンク。
    struct BlockingSink {
        block_for: Duration,
    }
    impl fandhe_backend_core::Diagnostics for BlockingSink {
        fn report(&self, _event: &DiagnosticEvent<'_>) {
            std::thread::sleep(self.block_for);
        }
    }

    let server = Server::new()
        .handler(FixedHandler)
        .shutdown_grace_period(grace)
        .diagnostics(BlockingSink { block_for });
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let run_task = tokio::spawn(async move {
        bound
            .run_until(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let _idle_stream = TcpStream::connect(addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    shutdown_tx.send(()).unwrap();

    // 実測の壁時計時間で判定する（通知は OS スレッドへ切り離されているため
    // tokio のタイマーには影響しないはずだが、ブロッキングシンクが返却を
    // 遅らせないことを tokio 外の時計で直接確かめる）。
    // `block_for`（2 秒）+ 十分なマージンをハング検知の安全弁として
    // `timeout` に設定しつつ、実際の合否判定は `elapsed` で行う。
    let start = std::time::Instant::now();
    let joined = timeout(block_for + Duration::from_secs(5), run_task)
        .await
        .expect("run_until タスクは有界時間内に完了するはず（ハング検知の安全弁）");
    let elapsed = start.elapsed();
    joined
        .expect("run_until タスクが panic しないこと")
        .expect("run_until は Ok(()) を返すはず");

    assert!(
        elapsed < grace + Duration::from_millis(700),
        "契約違反シンクが `report` 内でブロックしていても、run_until は \
         shutdown_grace_period + ε 以内に戻るはず\
         （実測: {elapsed:?}、block_for: {block_for:?}）"
    );
}

/// PR #748 Bugbot 指摘の回帰テスト:
/// <https://github.com/Fandhe-AI/fandhe-backend/pull/748>（Cursor Bugbot
/// "Shutdown diagnostic dropped on current_thread"）。
///
/// 修正前は `ShutdownGraceExceeded` の通知を `tokio::spawn` した detached
/// タスクへ切り離していたため、tokio ランタイムが以後もそのタスクを
/// ポーリングして初めて実行された。`run_until` を `current_thread`
/// ランタイムの `block_on` へ渡す**最後の await**として呼び出す典型的な
/// 使い方（公式 `graceful_shutdown` サンプルを含む）では、`run_until` が
/// `Ok(())` を返した時点で `block_on` 自体が完了して即座に戻り、他に
/// ポーリングする主体が存在しないため detached タスクは 1 度も実行されずに
/// 破棄され、通知が確実に失われていた。
///
/// 本テストはこの状況を直接再現する: 手動で `current_thread` ランタイムを
/// 構築し、`run_until` を `block_on` の中で最後に await した直後に
/// `block_on` を抜ける。修正後は通知が `std::thread::spawn`（OS スレッド）へ
/// 切り離され、tokio ランタイムの継続ポーリングに依存しなくなったため、
/// ランタイムを drop した後でも有界時間内に届くはず。
#[test]
fn shutdown_grace_exceeded_notified_even_when_run_until_is_last_await_on_current_thread_runtime() {
    let grace = Duration::from_millis(100);
    let sink = RecordingSink::default();
    let sink_for_rt = sink.clone();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current_thread ランタイムの構築に成功するはず");

    rt.block_on(async move {
        let server = Server::new()
            .handler(FixedHandler)
            .shutdown_grace_period(grace)
            .diagnostics(sink_for_rt);
        let bound = server.bind("127.0.0.1:0").await.unwrap();
        let addr = bound.local_addr().unwrap();

        // アイドル接続（リクエストは送らず張ったままにする）で permit を
        // 占有させ、grace 超過の強制クローズを確実に起こす
        // （`shutdown_grace_exceeded_reaches_custom_sink` と同一手法）。
        let _idle_stream = TcpStream::connect(addr).await.unwrap();

        // shutdown シグナルは `run_until` を直接（spawn せず）呼び出す
        // *前* に用意しない: `oneshot::Receiver` を即座に ready にしてしまうと
        // `run_until` 内部の select が「新規接続の accept」と「shutdown」の
        // どちらを先に処理するかは保証されず（両方 ready になりうる）、上記
        // アイドル接続が一度も accept されないまま shutdown 分岐へ進んで
        // しまう場合がある（本テストが検証したい grace 超過分岐に届かない）。
        // ここでは shutdown の送出自体を別タスクへ 20ms 遅延させることで、
        // `run_until` 本体（このルート future 内で直接 await する）が先に
        // 上記接続を accept して permit を確保できるようにする
        // （`shutdown_grace_exceeded_reaches_custom_sink` の「接続 → sleep(20ms)
        // → shutdown 送出」という時間分離と同じ狙いを、`run_until` を spawn
        // しない構成でも成立させるための書き換え）。
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = shutdown_tx.send(());
        });

        // `run_until` をこの `async move` ブロック（= `block_on` へ渡す
        // ルート future）内で最後に await する。ここで `Ok(())` が返れば
        // `block_on` は直ちにこの呼び出し元へ戻り、以後このランタイムは
        // 何もポーリングしない（Bugbot 指摘の再現条件そのもの）。
        bound
            .run_until(async {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("run_until は Ok(()) を返すはず");
    });
    // `block_on` から戻った時点でランタイムは停止しており、以後
    // 明示的に drop してもポーリングは一切発生しない。
    drop(rt);

    // ランタイムのポーリングには一切依存せず（`rt` は既に drop 済み）、
    // OS スレッドへ切り離された通知が届くのを有界時間だけ待つ。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if !sink.events.lock().unwrap().is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "current_thread ランタイムで run_until が block_on の最後の \
             await であっても、ShutdownGraceExceeded は有界時間内に届く \
             はず（PR #748 Bugbot 指摘の回帰）"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let events = sink.events.lock().unwrap();
    assert_eq!(
        events.len(),
        1,
        "ShutdownGraceExceeded がちょうど 1 件届くはず（実際: {events:?}）"
    );
    assert!(
        events[0].contains("graceful shutdown の猶予期間"),
        "登録済みシンクに ShutdownGraceExceeded の本文が届くはず（実際: {events:?}）"
    );
}

/// 受け入れ基準(a)(b): rebind 旧世代 drain の grace 超過で
/// `RebindDrainGraceExceeded` が登録済みシンクへ届くこと
/// （`rebind.rs::rebind_force_closes_old_generation_after_grace_period` と
/// 同一の grace 超過シナリオを利用）。
#[tokio::test]
async fn rebind_drain_grace_exceeded_reaches_custom_sink() {
    let grace = Duration::from_millis(150);
    let sink = RecordingSink::default();
    let server = Server::new()
        .handler(FixedHandler)
        .shutdown_grace_period(grace)
        .diagnostics(sink.clone());
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let old_addr = bound.local_addr().unwrap();
    let rebind = bound.rebind_handle();

    let run_task = tokio::spawn(async move { bound.run().await });

    // 旧アドレスへアイドル接続（リクエストは送らない）を張る。
    let _idle_stream = TcpStream::connect(old_addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let _new_addr = timeout(Duration::from_secs(5), rebind.rebind("127.0.0.1:0"))
        .await
        .expect("rebind はタイムアウトせず完了するはず")
        .expect("bind 可能な新アドレスへの rebind は成功するはず");

    // grace 超過後、旧世代の drain 背景タスクが RebindDrainGraceExceeded を
    // 発火するまでポーリングする（有界時間、self-hosted CI の輻輳を考慮し
    // 寛容な上限を取る）。
    let deadline = tokio::time::Instant::now() + grace + Duration::from_secs(5);
    loop {
        if !sink.events.lock().unwrap().is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "RebindDrainGraceExceeded は有界時間内に届くはず"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let events = sink.events.lock().unwrap();
    assert_eq!(
        events.len(),
        1,
        "RebindDrainGraceExceeded がちょうど 1 件届くはず（実際: {events:?}）"
    );
    assert!(
        events[0].contains("rebind による旧世代接続の drain が猶予期間"),
        "登録済みシンクに RebindDrainGraceExceeded の本文が届くはず（実際: {events:?}）"
    );

    run_task.abort();
}

/// 子プロセス側の実処理本体（環境変数 `FANDHE_DIAG_CHILD` が `custom` /
/// `default` のときのみ実行し、それ以外は即 return する）。
///
/// libtest の出力捕捉はスレッドローカルで tokio ワーカー・detached タスク
/// からの `eprintln!` を捉えられないため、この関数自体は親プロセスから
/// `--exact --nocapture` で直接起動され、stderr は子プロセスの標準エラー
/// としてそのまま親が読み取る。
#[tokio::test]
async fn diagnostics_child_shutdown_grace_scenario() {
    let mode = match std::env::var("FANDHE_DIAG_CHILD") {
        Ok(mode) => mode,
        Err(_) => return, // 通常の `cargo test` 実行では何もしない。
    };

    let grace = Duration::from_millis(100);
    let mut server = Server::new()
        .handler(FixedHandler)
        .shutdown_grace_period(grace);
    if mode == "custom" {
        // stderr へは一切書かないシンクへ差し替える（出力抑止の代表例）。
        server = server.diagnostics(|_event: &DiagnosticEvent<'_>| {});
    }
    // mode == "default" / "default_immediate_exit" の場合は既定の
    // `StderrDiagnostics` のまま。

    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let run_task = tokio::spawn(async move {
        bound
            .run_until(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let _idle_stream = TcpStream::connect(addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    shutdown_tx.send(()).unwrap();

    timeout(grace + Duration::from_secs(5), run_task)
        .await
        .expect("run_until は grace 超過後も有界時間内に戻るはず")
        .expect("run_until タスクが panic しないこと")
        .expect("run_until は Ok(()) を返すはず");

    if mode == "default_immediate_exit" {
        // `run_until` が `Ok(())` を返した直後、tokio ランタイムの通常の
        // shutdown・libtest 側の後処理を一切経由せずプロセスを即座に終了
        // させる。既定シンクは `run_until` の返却前に同期的に通知を確定
        // させる契約のため、この最も厳しい条件でも stderr へ出力済みで
        // あるはず（`SHUTDOWN_NOTIFY_FLUSH_WAIT` の有界待機や OS スレッドの
        // スケジューリングには一切依存しない）。
        std::process::exit(0);
    }
}

/// 子プロセスとして `diagnostics_child_shutdown_grace_scenario` を起動し、
/// その stderr 全体を回収する。
fn run_child_and_capture_stderr(mode: &str) -> String {
    let exe = std::env::current_exe().expect("current_exe は取得できるはず");
    let mut child = Command::new(exe)
        .arg("--exact")
        .arg("diagnostics_child_shutdown_grace_scenario")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env("FANDHE_DIAG_CHILD", mode)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("子プロセスの起動に成功するはず");

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr はパイプ済みのはず")
        .read_to_string(&mut stderr)
        .expect("子プロセスの stderr 読み取りに成功するはず");

    let status = child.wait().expect("子プロセスの終了待ちに成功するはず");
    assert!(
        status.success(),
        "子プロセス（mode={mode}）は正常終了するはず（stderr: {stderr}）"
    );
    stderr
}

/// 受け入れ基準(b): 差し替えたシンク（ここでは出力抑止クロージャ）を登録
/// した場合、stderr に既定文言が一切出ないこと。
#[test]
fn custom_sink_suppresses_default_stderr_output() {
    let stderr = run_child_and_capture_stderr("custom");
    assert!(
        !stderr.contains("fandhe_backend_core::server:"),
        "差し替えたシンクを登録した場合、stderr に既定の接頭辞付き出力が\
         含まれてはならない（実際の stderr: {stderr}）"
    );
}

/// 受け入れ基準(a): 未登録時（既定）は現行の `eprintln!` 出力と完全互換で
/// stderr へ出力されること（陽性対照）。
#[test]
fn default_sink_prints_to_stderr_when_unregistered() {
    let stderr = run_child_and_capture_stderr("default");
    assert!(
        stderr.contains(
            "fandhe_backend_core::server: graceful shutdown の猶予期間（100ms）を超過したため残存接続を強制クローズします"
        ),
        "未登録時は既定シンクが現行文言のまま stderr へ出力するはず\
         （実際の stderr: {stderr}）"
    );
}

/// PR #748 codex/review 指摘対応の回帰テスト: 既定シンク（`StderrDiagnostics`、
/// `Server::diagnostics` 未登録）は、`run_until` の呼び出し元が返却直後に
/// プロセスを終了させる最も厳しい条件でも、通知（`eprintln!`）が
/// **確実に**届くこと。
///
/// `SHUTDOWN_NOTIFY_FLUSH_WAIT`（200ms の有界待機）や `std::thread::spawn`
/// のスケジューリングに依存するカスタムシンク向け経路とは異なり、既定シンク
/// は `run_until` の返却前に `emit` を同期的に完了させてから返る契約
/// （`docs/design/diagnostics-sink.md` 9 節）。本テストはこの契約を、
/// `run_until` が `Ok(())` を返した直後に子プロセスを
/// `std::process::exit` で即座に終了させることで検証する
/// （tokio ランタイムの shutdown・libtest の後処理いずれにも猶予を与えない）。
#[test]
fn default_sink_notifies_even_when_process_exits_immediately_after_run_until_returns() {
    let stderr = run_child_and_capture_stderr("default_immediate_exit");
    assert!(
        stderr.contains(
            "fandhe_backend_core::server: graceful shutdown の猶予期間（100ms）を超過したため残存接続を強制クローズします"
        ),
        "既定シンクは run_until の返却前に通知を同期的に確定させるため、\
         直後のプロセス終了でも出力が届いているはず（実際の stderr: {stderr}）"
    );
}
