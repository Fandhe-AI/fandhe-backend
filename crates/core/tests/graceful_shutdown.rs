//! `BoundServer::run_until`（graceful shutdown、イシュー #313）の統合テスト。
//!
//! 実ソケットを介した accept ループの挙動そのものを検証対象とするため、
//! `tokio::io::duplex` は使わず実 `TcpListener` を張る（`websocket_respawn.rs`
//! と同一パターン）。受け入れ条件（イシュー #313）を次の 4 テストへ対応させる:
//! - `shutdown_rejects_new_connections_after_signal`: シグナル受信後は新規
//!   接続を受け付けない
//! - `shutdown_waits_for_in_flight_request_to_complete`: in-flight リクエスト
//!   はシグナル後も完走し、`Connection: close` を伴って正常応答する
//! - `shutdown_force_closes_after_grace_period`: grace 超過時に強制クローズし、
//!   `run_until` が有界時間内に戻る
//! - `run_backward_compatible_after_run_until_delegation`: 既存 `run()` が
//!   `run_until` へ委譲した後も後方互換の挙動を保つ
//! - `run_until_future_cancelled_externally_lets_in_flight_connection_complete`:
//!   `run_until` の Future 自体が外部（`tokio::select!` 等）からキャンセル
//!   された場合でも、in-flight 接続は abort されず完走できる（Bugbot 指摘
//!   review comment 3615287445 の回帰防止、"Cancel aborts in-flight
//!   connections"）

use fandhe_backend_core::{Handler, Server};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::time::timeout;

/// 固定 200 応答を返すだけのトイハンドラ。
struct FixedHandler;
impl Handler for FixedHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        Response::empty(200)
    }
}

/// 呼び出しごとに一定時間だけ「処理中」をシミュレートしてから応答する
/// トイハンドラ。`Handler::handle` は同期契約（[[coding-rust]] の 3 拡張点）
/// のため、実際の遅延は `std::thread::sleep` ではなく、呼び出し回数を
/// カウントするだけに留め、遅延は接続側（クライアントがリクエストを分割
/// 送信する）で作る。ここでは応答完了をカウントする用途にのみ使う。
struct CountingHandler {
    handled: Arc<AtomicUsize>,
}
impl Handler for CountingHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        self.handled.fetch_add(1, Ordering::SeqCst);
        Response::empty(200)
    }
}

async fn read_response(stream: &mut TcpStream) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = stream.read_to_end(&mut out).await;
    out
}

/// レスポンスヘッダ終端（空行）までを読む。ヘッダのみ確認したい場面で
/// `read_response`（EOF まで読む＝相手が接続を閉じるまでブロックする）を
/// 使うと、`keep_alive` が `true` の場合に無用にハングするため分離する。
async fn read_response_head(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).await.expect("read response byte");
        assert_ne!(n, 0, "stream closed before response terminator");
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buf).expect("response head must be valid utf-8")
}

/// 受け入れ条件「シグナル受信後に新規接続を受け付けない」の統合テスト。
///
/// shutdown 発火後、`run_until` の完了を待ってから接続を試み、
/// connection refused（新規 accept が一切行われない）ことを確認する。
#[tokio::test]
async fn shutdown_rejects_new_connections_after_signal() {
    let server = Server::new().handler(FixedHandler);
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

    // シグナルを即座に発火し、run_until の完了を待つ。
    shutdown_tx.send(()).unwrap();
    timeout(Duration::from_secs(5), run_task)
        .await
        .expect("run_until はシグナル後すぐ戻るはず")
        .expect("run_until タスクが panic しないこと")
        .expect("run_until は Ok(()) を返すはず");

    // run_until 完了後（リスニングソケットは既に drop 済み）は
    // 新規接続が OS レベルで拒否される。
    let connect_result = TcpStream::connect(addr).await;
    assert!(
        connect_result.is_err(),
        "shutdown 後は新規接続を受け付けないはず"
    );
    if let Err(err) = connect_result {
        assert_eq!(
            err.kind(),
            ErrorKind::ConnectionRefused,
            "接続拒否の理由は connection refused のはず（実際: {err:?}）"
        );
    }
}

/// 受け入れ条件「既存 `run()` の後方互換維持」＋ in-flight 完走の統合テスト。
///
/// shutdown 前に受理済み（TCP 接続確立・permit 取得済み）だが、リクエストを
/// 送り切っていない接続は「in-flight」として扱われる。リクエストを途中まで
/// 送信した状態で shutdown を発火し、その後に残りを送信すると、接続は
/// shutdown 前に受理済みであるため正常に完走し、`Connection: close` 付きの
/// 200 応答を受け取れることを確認する（`shutdown_grace_period` を短く設定し、
/// テスト自体の待ち時間を有界にする）。
#[tokio::test]
async fn shutdown_waits_for_in_flight_request_to_complete() {
    let server = Server::new()
        .handler(FixedHandler)
        .shutdown_grace_period(Duration::from_secs(2));
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

    // shutdown 前にリクエストの先頭のみを送信し、TCP 接続・permit を
    // 確実に確立させる（accept 済み＝in-flight にする）。
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();

    // 接続が accept ループ側で受理されタスクへ渡る猶予を与えてから
    // shutdown を発火する。この時点でリクエストはまだ完結していないため、
    // 接続は「処理中」ではなく「受理済みだが未完結」の in-flight 状態にある。
    tokio::time::sleep(Duration::from_millis(20)).await;
    shutdown_tx.send(()).unwrap();
    // shutdown_flag が立ち、run_until がリスニングソケットを閉じるまでの
    // 猶予を与えてから、残りのリクエストを送信する。これにより本テストの
    // 応答は shutdown_flag=true を観測した状態で決定され、
    // `Connection: close` の付与が決定的になる。
    tokio::time::sleep(Duration::from_millis(20)).await;
    stream
        .write_all(b"Host: example.com\r\n\r\n")
        .await
        .unwrap();

    let response_head = timeout(Duration::from_secs(5), read_response_head(&mut stream))
        .await
        .expect("shutdown 前に受理済みの in-flight 接続は完走し応答を受け取れるはず");
    assert!(
        response_head.starts_with("HTTP/1.1 200 OK\r\n"),
        "in-flight リクエストへの正常応答を期待（実際: {response_head}）"
    );
    assert!(
        response_head.to_lowercase().contains("connection: close"),
        "shutdown 後に完結したリクエストへの応答は Connection: close を伴うはず\
         （実際: {response_head}）"
    );

    timeout(Duration::from_secs(5), run_task)
        .await
        .expect("run_until は in-flight 完了後に戻るはず")
        .expect("run_until タスクが panic しないこと")
        .expect("run_until は Ok(()) を返すはず");
}

/// 受け入れ条件「in-flight 完了待ちに上限時間・超過時強制クローズ」の
/// 統合テスト。
///
/// アイドル接続（リクエストを送らずに張ったまま）を残して shutdown を発火し、
/// `shutdown_grace_period` を超過しても `run_until` が有界時間内に戻り、
/// クライアント側ソケットが強制クローズされることを確認する。
#[tokio::test]
async fn shutdown_force_closes_after_grace_period() {
    let grace = Duration::from_millis(100);
    let server = Server::new()
        .handler(FixedHandler)
        .shutdown_grace_period(grace);
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

    // アイドル接続（リクエストは送らず張ったままにする）を作り、
    // permit を占有させた状態で shutdown を発火する。
    let mut idle_stream = TcpStream::connect(addr).await.unwrap();
    // 接続が accept ループ側で受理されタスクへ渡る猶予を与える。
    tokio::time::sleep(Duration::from_millis(20)).await;
    shutdown_tx.send(()).unwrap();

    // grace 超過後、run_until は有界時間内（grace + 生成な ε）に必ず戻る
    // （self-hosted CI の輻輳を考慮し寛容な上限を取る）。
    timeout(grace + Duration::from_secs(5), run_task)
        .await
        .expect("run_until は grace 超過後も有界時間内に戻るはず")
        .expect("run_until タスクが panic しないこと")
        .expect("run_until は Ok(()) を返すはず");

    // 強制クローズにより、アイドル接続のソケットは閉じられる（read が
    // 0 バイトまたはエラーで終わる）。
    let mut probe = [0u8; 1];
    let read_result = timeout(Duration::from_secs(5), idle_stream.read(&mut probe))
        .await
        .expect("強制クローズ後の read はタイムアウトせず終了するはず");
    match read_result {
        Ok(0) => {} // 正常クローズ（EOF）
        Ok(n) => panic!("強制クローズ後にデータを受信すべきではない（{n} バイト）"),
        Err(_) => {} // リセット等のエラーも強制クローズの一種として許容
    }
}

/// 受け入れ条件「既存 `run()` の後方互換維持」の統合テスト。
///
/// `run()` は `run_until(pending)` へ委譲するだけの薄いラッパーになった
/// 後も、通常のリクエスト処理（accept・応答）が変わらず動作することを
/// 確認する（`shutdown` を渡さないため `run()` 自体は自然終了しない。
/// テスト側でタスクを spawn し、応答確認後に abort して片付ける）。
#[tokio::test]
async fn run_backward_compatible_after_run_until_delegation() {
    let server = Server::new().handler(FixedHandler);
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();

    let run_task = tokio::spawn(async move {
        let _ = bound.run().await;
    });

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(5), read_response(&mut stream))
        .await
        .expect("run() 経由でも通常どおり応答するはず");
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.starts_with("HTTP/1.1 200 OK\r\n"),
        "run() の既存挙動が維持されているはず（実際: {text}）"
    );

    run_task.abort();
}

/// Bugbot 指摘（review comment 3615287445、"Cancel aborts in-flight
/// connections"）の回帰防止テスト。
///
/// `run_until` の Future 自体が呼び出し側の `tokio::select!` で敗退し
/// drop される（一般的な shutdown パターン）と、内部の `JoinSet` が素の
/// `Drop` だと全 in-flight 接続を abort してしまう。`CancelSafeJoinSet`
/// （`server.rs` の doc を参照）でこれを防ぎ、accept 済みだが未完結の接続は
/// `run_until` が打ち切られた後も独立タスクとして完走できることを確認する。
#[tokio::test]
async fn run_until_future_cancelled_externally_lets_in_flight_connection_complete() {
    let handled = Arc::new(AtomicUsize::new(0));
    let server = Server::new().handler(CountingHandler {
        handled: Arc::clone(&handled),
    });
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let run_task = tokio::spawn(async move {
        tokio::select! {
            _ = bound.run_until(std::future::pending::<()>()) => {}
            _ = cancel_rx => {}
        }
    });

    // リクエストの先頭のみを送信し、TCP 接続・permit を確実に確立させる
    // （accept 済み＝in-flight にする）。
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();

    // 接続が accept ループ側で受理されタスクへ渡る猶予を与えてから、
    // `run_until` の Future 自体を外部キャンセルする（`select!` の
    // `cancel_rx` 分岐を勝たせる）。
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel_tx.send(()).unwrap();
    timeout(Duration::from_secs(5), run_task)
        .await
        .expect("外部キャンセル後の select! は速やかに戻るはず")
        .expect("run_task が panic しないこと");

    // run_until 自体は打ち切られたが、既に accept 済みだった in-flight
    // 接続は abort されず、残りのリクエストを送ればそのまま完走できる
    // はず（`CancelSafeJoinSet` により abort ではなく detach される）。
    stream
        .write_all(b"Host: example.com\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(5), read_response(&mut stream))
        .await
        .expect("外部キャンセル後も in-flight 接続は完走し応答を受け取れるはず");
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.starts_with("HTTP/1.1 200 OK\r\n"),
        "abort ではなく detach され、正常応答が返るはず（実際: {text}）"
    );
    assert_eq!(
        handled.load(Ordering::SeqCst),
        1,
        "ハンドラは 1 回だけ呼ばれ、in-flight 接続が完走したはず"
    );
}
