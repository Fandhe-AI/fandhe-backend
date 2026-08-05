//! `BoundServer::rebind_handle` / `RebindHandle::rebind`（稼働中の listener
//! 差し替え、イシュー #485）の統合テスト。
//!
//! 実ソケットを介した accept ループの挙動そのものを検証対象とするため、
//! `graceful_shutdown.rs` と同一パターンで実 `TcpListener` を張る。受け入れ
//! 基準（イシュー #485 の実装計画）を次のテストへ対応させる:
//! - `rebind_switches_listening_address`: 新アドレスへの新規リクエストが
//!   200 で応答し、旧アドレスへの新規 connect は拒否される
//! - `rebind_fails_closed_when_bind_target_is_occupied`: bind 失敗時は
//!   `Err` を返し、旧アドレスは引き続き 200 で応答し続ける
//! - `rebind_fails_closed_does_not_disrupt_in_flight_request`: in-flight
//!   リクエスト処理中に失敗する rebind を発行しても、そのリクエストは
//!   正常完走する
//! - `rebind_drains_old_generation_keep_alive_connection`: 旧世代の
//!   keep-alive 接続は rebind 後も in-flight を完走し `Connection: close`
//!   が付与される
//! - `rebind_force_closes_old_generation_after_grace_period`: 短い
//!   `shutdown_grace_period` + 居座り接続で、旧世代接続が grace 超過後に
//!   強制クローズされる
//! - `rebind_after_shutdown_fails_fast_without_waiting_grace_period`:
//!   shutdown 確定後に発行した `rebind` が grace 期間の終了を待たず速やかに
//!   `Err` を返す（Bugbot 指摘対応、`rebind_rx` を shutdown 確定直後に
//!   閉じる修正の回帰テスト）
//! - `rebind_after_shutdown_releases_bound_port_promptly`: shutdown 確定後の
//!   `rebind` が呼び出し元で bind した新 `TcpListener` が、grace 期間を
//!   待たず速やかに drop されポートが解放される（同上）
//! - `rebind_preserves_registered_extension_points`: `Middleware` /
//!   `RequestGate` / `UpgradeHandler` / `Interceptor` / `Handler` を登録した
//!   状態で rebind しても、新アドレスへのリクエストで拡張点が引き続き
//!   呼ばれる（再登録不要）
//! - `run_until_without_rebind_handle_behaves_as_before`: `rebind_handle`
//!   を一度も呼ばない経路では通常どおり動作する（機能的回帰なしの固定。
//!   チャネル非生成（ゼロコスト）自体は非公開フィールドのためここでは
//!   直接検証できず、コードレビューで担保する）

use fandhe_backend_core::extension::{
    GateContext, GateOutcome, Middleware, RequestGate, UpgradeHandler,
};
use fandhe_backend_core::interceptor::Interceptor;
use fandhe_backend_core::{Handler, Server};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::HandlerFuture;
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// 固定 200 応答を返すだけのトイハンドラ。
struct FixedHandler;
impl Handler for FixedHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> HandlerFuture {
        Box::pin(std::future::ready(Response::empty(200)))
    }
}

async fn read_response(stream: &mut TcpStream) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = stream.read_to_end(&mut out).await;
    out
}

/// レスポンスヘッダ終端（空行）までを読む（`graceful_shutdown.rs` と同一ヘルパー）。
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

/// 受け入れ基準「再バインド API」の統合テスト。
///
/// `rebind()` 後、新アドレスへの新規リクエストは 200 で応答し、旧アドレスへの
/// 新規 connect は（旧 listener が即座に閉じられるため）拒否される。
#[tokio::test]
async fn rebind_switches_listening_address() {
    let server = Server::new().handler(FixedHandler);
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let old_addr = bound.local_addr().unwrap();
    let rebind = bound.rebind_handle();

    let run_task = tokio::spawn(async move { bound.run().await });

    let new_addr = timeout(Duration::from_secs(5), rebind.rebind("127.0.0.1:0"))
        .await
        .expect("rebind はタイムアウトせず完了するはず")
        .expect("bind 可能な新アドレスへの rebind は成功するはず");
    assert_ne!(old_addr, new_addr, "新アドレスは旧アドレスと異なるはず");

    // 旧アドレスへの新規 connect は拒否される（listener 差し替え済み）。
    let old_connect = TcpStream::connect(old_addr).await;
    assert!(
        old_connect.is_err(),
        "rebind 後は旧アドレスへの新規接続を受け付けないはず"
    );
    if let Err(err) = old_connect {
        assert_eq!(err.kind(), ErrorKind::ConnectionRefused);
    }

    // 新アドレスへのリクエストは通常どおり 200 で応答する。
    let mut stream = TcpStream::connect(new_addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(5), read_response(&mut stream))
        .await
        .expect("新アドレスへのリクエストは応答するはず");
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.starts_with("HTTP/1.1 200 OK\r\n"),
        "新アドレスへのリクエストは 200 のはず（実際: {text}）"
    );

    run_task.abort();
}

/// 受け入れ基準「fail-closed」の統合テスト（bind 失敗）。
///
/// 別のリスナーで先に占有したポートへ rebind すると `Err` を返し、旧アドレスは
/// 引き続き 200 で応答し続ける。
#[tokio::test]
async fn rebind_fails_closed_when_bind_target_is_occupied() {
    let server = Server::new().handler(FixedHandler);
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let old_addr = bound.local_addr().unwrap();
    let rebind = bound.rebind_handle();

    // rebind 先として使うアドレスをテスト側で先に占有しておく。
    let occupying_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let occupied_addr = occupying_listener.local_addr().unwrap();

    let run_task = tokio::spawn(async move { bound.run().await });

    let rebind_result = timeout(
        Duration::from_secs(5),
        rebind.rebind(occupied_addr.to_string()),
    )
    .await
    .expect("rebind はタイムアウトせず完了するはず");
    assert!(
        rebind_result.is_err(),
        "占有済みアドレスへの rebind は失敗するはず"
    );
    drop(occupying_listener);

    // 旧アドレスは rebind 失敗の影響を受けず、引き続き 200 で応答する。
    let mut stream = TcpStream::connect(old_addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(5), read_response(&mut stream))
        .await
        .expect("旧アドレスは rebind 失敗後も応答し続けるはず");
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.starts_with("HTTP/1.1 200 OK\r\n"),
        "旧アドレスは 200 のはず（実際: {text}）"
    );

    run_task.abort();
}

/// 受け入れ基準「fail-closed」の統合テスト（in-flight への無影響）。
///
/// リクエスト送信を分割し、in-flight（受理済みだが未完結）の間に失敗する
/// rebind を発行しても、そのリクエストは正常完走することを確認する。
#[tokio::test]
async fn rebind_fails_closed_does_not_disrupt_in_flight_request() {
    let server = Server::new().handler(FixedHandler);
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();
    let rebind = bound.rebind_handle();

    let occupying_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let occupied_addr = occupying_listener.local_addr().unwrap();

    let run_task = tokio::spawn(async move { bound.run().await });

    // 1 本目のリクエストを完走させ、この接続が accept 済みであることを
    // 決定的に確認する（self-hosted CI の輻輳を考慮した決定的な accept
    // 確認。固定 sleep による「たぶん accept 済み」推測には頼らない）。
    // `Connection: close` を付けないため keep-alive のまま同一接続を
    // 使い続けられる。
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await
        .unwrap();
    let first_head = timeout(Duration::from_secs(5), read_response_head(&mut stream))
        .await
        .expect("1 本目のリクエストは accept 済みとして完走するはず");
    assert!(
        first_head.starts_with("HTTP/1.1 200 OK\r\n"),
        "実際: {first_head}"
    );

    // 同一 keep-alive 接続で 2 本目のリクエストの先頭のみ送信し、
    // in-flight にする。1 本目の完走により接続の accept 済みは確定して
    // いるため、ここでの送信タイミングは accept レースに左右されない。
    stream.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();

    // in-flight 中に失敗する rebind を発行する。
    let rebind_result = timeout(
        Duration::from_secs(5),
        rebind.rebind(occupied_addr.to_string()),
    )
    .await
    .expect("rebind はタイムアウトせず完了するはず");
    assert!(rebind_result.is_err());
    drop(occupying_listener);

    // 残りのリクエストを送ると正常完走する。
    stream
        .write_all(b"Host: example.com\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let response_head = timeout(Duration::from_secs(5), read_response_head(&mut stream))
        .await
        .expect("in-flight リクエストは rebind 失敗の影響を受けず完走するはず");
    assert!(
        response_head.starts_with("HTTP/1.1 200 OK\r\n"),
        "実際: {response_head}"
    );

    run_task.abort();
}

/// 受け入れ基準「旧 listener の drain」の統合テスト（keep-alive 完走）。
///
/// 旧アドレスの keep-alive 接続を張ったまま rebind すると、そのリクエストは
/// in-flight として完走し `Connection: close` が付与される。
#[tokio::test]
async fn rebind_drains_old_generation_keep_alive_connection() {
    let server = Server::new()
        .handler(FixedHandler)
        .shutdown_grace_period(Duration::from_secs(2));
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let old_addr = bound.local_addr().unwrap();
    let rebind = bound.rebind_handle();

    let run_task = tokio::spawn(async move { bound.run().await });

    // 旧アドレスへ 1 本目のリクエストを完走させ、この接続が accept 済み
    // であることを決定的に確認する（self-hosted CI の輻輳を考慮した
    // 決定的な accept 確認。固定 sleep による「たぶん accept 済み」推測
    // には頼らない）。`Connection: close` を付けないため keep-alive の
    // まま同一接続を使い続けられる。
    let mut stream = TcpStream::connect(old_addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await
        .unwrap();
    let first_head = timeout(Duration::from_secs(5), read_response_head(&mut stream))
        .await
        .expect("1 本目のリクエストは accept 済みとして完走するはず");
    assert!(
        first_head.starts_with("HTTP/1.1 200 OK\r\n"),
        "実際: {first_head}"
    );

    // 同一 keep-alive 接続で 2 本目のリクエストの先頭のみ送信し、
    // in-flight にする。1 本目の完走により接続の accept 済みは確定して
    // いるため、ここでの送信タイミングは accept レースに左右されない。
    stream.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();

    // rebind を発行し、差し替え完了を待つ。
    let _new_addr = timeout(Duration::from_secs(5), rebind.rebind("127.0.0.1:0"))
        .await
        .expect("rebind はタイムアウトせず完了するはず")
        .expect("bind 可能な新アドレスへの rebind は成功するはず");

    // 旧世代フラグが立つ猶予を与えてから残りのリクエストを送信する。
    tokio::time::sleep(Duration::from_millis(20)).await;
    stream
        .write_all(b"Host: example.com\r\n\r\n")
        .await
        .unwrap();

    let response_head = timeout(Duration::from_secs(5), read_response_head(&mut stream))
        .await
        .expect("rebind 前に受理済みの旧世代接続は完走し応答を受け取れるはず");
    assert!(
        response_head.starts_with("HTTP/1.1 200 OK\r\n"),
        "実際: {response_head}"
    );
    assert!(
        response_head.to_lowercase().contains("connection: close"),
        "rebind 後に完結した旧世代リクエストへの応答は Connection: close を伴うはず\
         （実際: {response_head}）"
    );

    run_task.abort();
}

/// 受け入れ基準「旧 listener の drain」の統合テスト（grace 超過時の強制クローズ）。
///
/// 短い `shutdown_grace_period` を設定し、旧世代のアイドル接続（居座り）が
/// grace 超過後に強制クローズされることを確認する。
#[tokio::test]
async fn rebind_force_closes_old_generation_after_grace_period() {
    let grace = Duration::from_millis(150);
    let server = Server::new()
        .handler(FixedHandler)
        .shutdown_grace_period(grace);
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let old_addr = bound.local_addr().unwrap();
    let rebind = bound.rebind_handle();

    let run_task = tokio::spawn(async move { bound.run().await });

    // 旧アドレスへアイドル接続（リクエストは送らない）を張る。
    let mut idle_stream = TcpStream::connect(old_addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let _new_addr = timeout(Duration::from_secs(5), rebind.rebind("127.0.0.1:0"))
        .await
        .expect("rebind はタイムアウトせず完了するはず")
        .expect("bind 可能な新アドレスへの rebind は成功するはず");

    // grace 超過後、旧世代の drain 背景タスクが強制クローズするはず
    // （self-hosted CI の輻輳を考慮し寛容な上限を取る）。
    let mut probe = [0u8; 1];
    let read_result = timeout(grace + Duration::from_secs(5), idle_stream.read(&mut probe)).await;
    match read_result {
        Ok(Ok(0)) => {} // 正常クローズ（EOF）
        Ok(Ok(n)) => panic!("強制クローズ後にデータを受信すべきではない（{n} バイト）"),
        Ok(Err(_)) => {} // リセット等のエラーも強制クローズの一種として許容
        Err(_) => panic!("grace 超過後の強制クローズは有界時間内に起きるはず"),
    }

    run_task.abort();
}

/// 受け入れ基準「shutdown 確定後の rebind は fail-fast」の統合テスト
/// （Bugbot 指摘対応、イシュー #485。`run_until` は shutdown 確定直後
/// （grace drain 開始前）に `rebind_rx` を閉じるため、以後の `rebind` は
/// grace 期間の終了を待たず速やかに `Err` を返す）。
///
/// grace 期間を長め（数秒）に設定したうえで shutdown を発火させ、その直後に
/// 発行した `rebind` が grace 期間の終了を待たず短時間で `Err` を返すことを
/// 確認する。
#[tokio::test]
async fn rebind_after_shutdown_fails_fast_without_waiting_grace_period() {
    let grace = Duration::from_secs(5);
    let server = Server::new()
        .handler(FixedHandler)
        .shutdown_grace_period(grace);
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();
    let rebind = bound.rebind_handle();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let run_task = tokio::spawn(async move {
        bound
            .run_until(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    // shutdown 後の in-flight 完了待ち（grace drain）を実際に grace 期間
    // 一杯まで働かせるため、accept 済みかつ in-flight（未完結）の接続を
    // 1 本作っておく。これがないと待つべき in-flight 接続が存在せず drain が
    // 即座に完了してしまい、`rebind_rx` を早期に閉じない旧実装でも
    // `run_until` 自体が速やかに終了して本テストが偽陽性で通ってしまう
    // （`rebind_fails_closed_does_not_disrupt_in_flight_request` と同一パターン。
    // 1 本目のリクエストを完走させて accept 済みであることを決定的に確認して
    // から、2 本目のリクエストの先頭のみ送って in-flight にする）。
    let mut idle_stream = TcpStream::connect(addr).await.unwrap();
    idle_stream
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await
        .unwrap();
    let first_head = timeout(Duration::from_secs(5), read_response_head(&mut idle_stream))
        .await
        .expect("1 本目のリクエストは accept 済みとして完走するはず");
    assert!(
        first_head.starts_with("HTTP/1.1 200 OK\r\n"),
        "実際: {first_head}"
    );
    idle_stream.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();

    // shutdown を発火させ、accept ループが `Raced::Shutdown` に入るまで
    // 短い猶予を与える。
    shutdown_tx.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let started = std::time::Instant::now();
    let rebind_result = timeout(Duration::from_secs(1), rebind.rebind("127.0.0.1:0"))
        .await
        .expect("shutdown 確定後の rebind は grace 期間を待たず速やかに完了するはず");
    let elapsed = started.elapsed();

    assert!(
        rebind_result.is_err(),
        "shutdown 確定後の rebind は Err を返すはず"
    );
    assert!(
        elapsed < grace,
        "shutdown 確定後の rebind は grace 期間（{grace:?}）を待たずに完了するはず\
         （実際: {elapsed:?}）"
    );

    let _ = timeout(grace + Duration::from_secs(5), run_task)
        .await
        .expect("run_until は grace 期間内に終了するはず");
}

/// 受け入れ基準「rebind で bind したポートは shutdown 後すぐ再利用可能」
/// の統合テスト（Bugbot 指摘対応、イシュー #485）。
///
/// shutdown 確定後に発行した `rebind` が呼び出し元で bind した新
/// `TcpListener` は、`rebind_rx` が shutdown 確定直後に閉じられることで
/// 速やかに drop されポートが解放されているはずである。同一アドレスへの
/// 再 bind が grace 期間を待たず短時間で成功することで確認する。
#[tokio::test]
async fn rebind_after_shutdown_releases_bound_port_promptly() {
    let grace = Duration::from_secs(5);
    let server = Server::new()
        .handler(FixedHandler)
        .shutdown_grace_period(grace);
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();
    let rebind = bound.rebind_handle();

    // rebind 先として使う固定アドレスを一旦確保してから解放し、OS 割当の
    // ポート番号を先に確定させておく。
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_addr = probe.local_addr().unwrap();
    drop(probe);

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let run_task = tokio::spawn(async move {
        bound
            .run_until(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    // shutdown 後の in-flight 完了待ち（grace drain）を実際に grace 期間
    // 一杯まで働かせるため、accept 済みかつ in-flight（未完結）の接続を
    // 1 本作っておく（上のテストと同一パターン・同じ理由。これがないと
    // 偽陽性で通ってしまう）。
    let mut idle_stream = TcpStream::connect(addr).await.unwrap();
    idle_stream
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await
        .unwrap();
    let first_head = timeout(Duration::from_secs(5), read_response_head(&mut idle_stream))
        .await
        .expect("1 本目のリクエストは accept 済みとして完走するはず");
    assert!(
        first_head.starts_with("HTTP/1.1 200 OK\r\n"),
        "実際: {first_head}"
    );
    idle_stream.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();

    shutdown_tx.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let started = std::time::Instant::now();
    let _ = timeout(
        Duration::from_secs(1),
        rebind.rebind(target_addr.to_string()),
    )
    .await
    .expect("shutdown 確定後の rebind は速やかに完了するはず");
    assert!(
        started.elapsed() < grace,
        "shutdown 確定後の rebind は grace 期間を待たずに完了するはず"
    );

    // rebind() 内部で bind した listener は、上記完了時点で速やかに
    // drop されポートが解放されているはず。grace 期間を待たず同じ
    // アドレスへ再 bind できることで確認する。
    let rebound = timeout(
        Duration::from_secs(1),
        tokio::net::TcpListener::bind(target_addr),
    )
    .await
    .expect("bind はタイムアウトせず完了するはず");
    assert!(
        rebound.is_ok(),
        "rebind が使ったポートは grace 期間を待たず解放されているはず"
    );

    let _ = timeout(grace + Duration::from_secs(5), run_task)
        .await
        .expect("run_until は grace 期間内に終了するはず");
}

/// 呼び出しごとにカウントを増やすトイ `Middleware`。
struct CountingMiddleware {
    on_request: Arc<AtomicUsize>,
    on_response: Arc<AtomicUsize>,
}
impl Middleware for CountingMiddleware {
    fn name(&self) -> &'static str {
        "counting-middleware"
    }
    fn on_request(&self, _head: &RequestHead) {
        self.on_request.fetch_add(1, Ordering::SeqCst);
    }
    fn on_response(&self, _head: &RequestHead, _elapsed: Duration) {
        self.on_response.fetch_add(1, Ordering::SeqCst);
    }
}

/// 呼び出しごとにカウントを増やし、常に許可するトイ `RequestGate`。
struct CountingGate {
    checked: Arc<AtomicUsize>,
}
impl RequestGate for CountingGate {
    fn name(&self) -> &'static str {
        "counting-gate"
    }
    fn check(&self, _head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
        self.checked.fetch_add(1, Ordering::SeqCst);
        GateOutcome::Allow
    }
}

/// 呼び出しごとにカウントを増やすトイ `Handler`。
struct CountingHandler {
    handled: Arc<AtomicUsize>,
}
impl Handler for CountingHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> HandlerFuture {
        self.handled.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Response::empty(200)))
    }
}

/// `matches` 呼び出しごとにカウントを増やすトイ `UpgradeHandler`。
///
/// このテストでは常に `false` を返し（Upgrade 委譲経路には踏み込まない）、
/// `matches` が呼ばれたこと自体（拡張点の評価に組み込まれ続けていること）を
/// カウンタで固定する。
struct CountingUpgradeHandler {
    matched_checked: Arc<AtomicUsize>,
}
impl UpgradeHandler for CountingUpgradeHandler {
    fn name(&self) -> &'static str {
        "counting-upgrade-handler"
    }
    fn matches(&self, _head: &RequestHead) -> bool {
        self.matched_checked.fetch_add(1, Ordering::SeqCst);
        false
    }
}

/// `intercept` 呼び出しごとにカウントを増やすトイ `Interceptor`。
///
/// 常に `None` を返し（素通し）、`Handler` まで到達させたうえで `intercept`
/// が呼ばれたことをカウンタで固定する。
struct CountingInterceptor {
    intercepted: Arc<AtomicUsize>,
}
impl Interceptor for CountingInterceptor {
    fn name(&self) -> &'static str {
        "counting-interceptor"
    }
    fn intercept(&self, _head: &RequestHead, _body: &[u8]) -> Option<Response> {
        self.intercepted.fetch_add(1, Ordering::SeqCst);
        None
    }
}

/// 受け入れ基準「拡張点引き継ぎ」の統合テスト。
///
/// `Middleware` / `RequestGate` / `UpgradeHandler` / `Interceptor` /
/// `Handler` を登録した状態で rebind しても、再登録なしに新アドレスへの
/// リクエストで全カウンタが増えることを確認する（`docs/design/rebind.md`
/// §5.3 の 4 拡張点 + `Handler` の記述と対応）。
#[tokio::test]
async fn rebind_preserves_registered_extension_points() {
    let mw_on_request = Arc::new(AtomicUsize::new(0));
    let mw_on_response = Arc::new(AtomicUsize::new(0));
    let gate_checked = Arc::new(AtomicUsize::new(0));
    let upgrade_matched_checked = Arc::new(AtomicUsize::new(0));
    let intercepted = Arc::new(AtomicUsize::new(0));
    let handled = Arc::new(AtomicUsize::new(0));

    let server = Server::new()
        .middleware(CountingMiddleware {
            on_request: Arc::clone(&mw_on_request),
            on_response: Arc::clone(&mw_on_response),
        })
        .gate(CountingGate {
            checked: Arc::clone(&gate_checked),
        })
        .upgrade_handler(CountingUpgradeHandler {
            matched_checked: Arc::clone(&upgrade_matched_checked),
        })
        .interceptor(CountingInterceptor {
            intercepted: Arc::clone(&intercepted),
        })
        .handler(CountingHandler {
            handled: Arc::clone(&handled),
        });
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let rebind = bound.rebind_handle();

    let run_task = tokio::spawn(async move { bound.run().await });

    let new_addr = timeout(Duration::from_secs(5), rebind.rebind("127.0.0.1:0"))
        .await
        .expect("rebind はタイムアウトせず完了するはず")
        .expect("bind 可能な新アドレスへの rebind は成功するはず");

    let mut stream = TcpStream::connect(new_addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(5), read_response(&mut stream))
        .await
        .expect("新アドレスへのリクエストは応答するはず");
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "実際: {text}");

    assert_eq!(
        mw_on_request.load(Ordering::SeqCst),
        1,
        "rebind 後も Middleware::on_request は再登録なしで呼ばれるはず"
    );
    assert_eq!(
        mw_on_response.load(Ordering::SeqCst),
        1,
        "rebind 後も Middleware::on_response は再登録なしで呼ばれるはず"
    );
    assert_eq!(
        gate_checked.load(Ordering::SeqCst),
        1,
        "rebind 後も RequestGate::check は再登録なしで呼ばれるはず"
    );
    assert_eq!(
        upgrade_matched_checked.load(Ordering::SeqCst),
        1,
        "rebind 後も UpgradeHandler::matches は再登録なしで呼ばれるはず"
    );
    assert_eq!(
        intercepted.load(Ordering::SeqCst),
        1,
        "rebind 後も Interceptor::intercept は再登録なしで呼ばれるはず"
    );
    assert_eq!(
        handled.load(Ordering::SeqCst),
        1,
        "rebind 後も Handler::handle は再登録なしで呼ばれるはず"
    );

    run_task.abort();
}

/// 受け入れ基準「回帰なし」の統合テスト。
///
/// `rebind_handle` を一度も呼ばない経路（既存 `run()` の通常利用）は、
/// 従来どおり動作する（機能的回帰がないことの固定）。`BoundServer` の
/// `rebind_tx`/`rebind_rx` フィールドが実際に `None` のまま（rebind
/// チャネルが生成されないこと自体、pay-for-what-you-use）は非公開
/// フィールドのため統合テストからは直接観測できず、コードレビューで
/// 担保する（`BoundServer::rebind_handle` の doc「初回呼び出しで遅延生成」
/// を参照）。
#[tokio::test]
async fn run_until_without_rebind_handle_behaves_as_before() {
    let server = Server::new().handler(FixedHandler);
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();

    let run_task = tokio::spawn(async move { bound.run().await });

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(5), read_response(&mut stream))
        .await
        .expect("rebind_handle 未呼び出しでも通常どおり応答するはず");
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "実際: {text}");

    run_task.abort();
}
