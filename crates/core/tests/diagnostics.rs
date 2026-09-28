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
    // mode == "default" の場合は既定の `StderrDiagnostics` のまま。

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
