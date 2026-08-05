//! 世代キャンセルシグナルの WS 委譲タスクへの配線（イシュー #491）の
//! 統合テスト。
//!
//! `docs/design/ws-cancellation-propagation.md` が確定した設計（世代別
//! `tokio::sync::watch` + drain 開始時発火）を、最終 graceful shutdown
//! （#313）・rebind 世代 drain（#485/#488）の両経路について実 TCP 接続で
//! 検証する。`crates/core/src/plugin.rs` の `try_handle_upgrade` は本イシュー
//! の実装で、発火時に `handle_upgrade` の `Future` を drop してタスクを
//! 打ち切る中間ハードクローズを行う（#492 で Close frame 送信へ置換予定、
//! `try_handle_upgrade` の doc「世代キャンセルシグナル」を参照）。
//!
//! - `final_shutdown_cancels_delegated_websocket_session`: 最終 shutdown
//!   発火後、委譲済み WS セッションが有界時間内にクローズされ、`run_until`
//!   も grace を待ち切らず速やかに戻ることを確認する
//! - `rebind_cancels_old_generation_websocket_session`: rebind 発火後、
//!   旧世代の WS セッションが有界時間内にクローズされ、新世代アドレスへの
//!   通常 HTTP リクエストは継続して処理されることを確認する
//! - 既存 `graceful_shutdown.rs` / `rebind.rs` / `websocket_upgrade.rs` /
//!   `websocket_respawn.rs` / `websocket_upgrade_disabled.rs` が無変更で
//!   pass すること（非退行、受け入れ条件 4）はテストスイート全体の実行で
//!   別途確認する

#![cfg(feature = "websocket")]

use fandhe_backend_core::{Handler, Server};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_websocket::WebSocketConfig;
use fandhe_backend_routes::HandlerFuture;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// 固定 200 応答を返すだけのトイハンドラ（`rebind.rs` と同一パターン）。
/// 新世代アドレスでの通常 HTTP リクエスト継続処理を検証するために使う。
struct FixedHandler;
impl Handler for FixedHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> HandlerFuture {
        Box::pin(std::future::ready(Response::empty(200)))
    }
}

const VALID_HANDSHAKE_REQUEST: &[u8] = b"GET /ws HTTP/1.1\r\n\
    Host: example.com\r\n\
    Upgrade: websocket\r\n\
    Connection: Upgrade\r\n\
    Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
    Sec-WebSocket-Version: 13\r\n\
    \r\n";

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

/// 最終 graceful shutdown（イシュー #313）発火が、`UpgradeHandler` により
/// 既に委譲済みの WS セッションへ伝播することを確認する
/// （`docs/design/ws-cancellation-propagation.md` 5.3 節「shutdown_flag を
/// true にする直後に発火する」）。
///
/// WS セッションを張ったまま shutdown を発火し、(a) クライアント側が有界
/// 時間内に EOF を観測すること、(b) `run_until` 自体が
/// `shutdown_grace_period` を待ち切らずに速やかに `Ok(())` で戻ること
/// （permit がキャンセルにより早期解放されるため）の 2 点を検証する。
#[tokio::test(flavor = "multi_thread")]
async fn final_shutdown_cancels_delegated_websocket_session() {
    // grace を意図的に長めに取り、"grace 超過を待たず速やかに戻る" ことを
    // 明確に区別できるようにする。
    let grace = Duration::from_secs(10);
    let server = Server::new()
        .websocket(WebSocketConfig::default())
        .shutdown_grace_period(grace);
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let run_task = tokio::spawn(async move {
        bound
            .run_until(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let mut ws_client = TcpStream::connect(addr).await.unwrap();
    ws_client.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();
    let response_head = timeout(Duration::from_secs(5), read_response_head(&mut ws_client))
        .await
        .expect("ハンドシェイク応答は有界時間内に届くはず");
    assert!(
        response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
        "実際: {response_head}"
    );

    // shutdown を発火する。
    shutdown_tx.send(()).unwrap();

    // (a) WS セッションが有界時間内にクローズされる（キャンセル発火 →
    // `try_handle_upgrade` が `handle_upgrade` の Future を drop）ことを
    // 確認する。grace（10 秒）よりも十分短い上限で観測できるはず。
    let mut probe = [0u8; 1];
    let read_result = timeout(Duration::from_secs(5), ws_client.read(&mut probe)).await;
    match read_result {
        Ok(Ok(0)) => {} // 正常クローズ（EOF）
        Ok(Ok(n)) => panic!("キャンセル後にデータを受信すべきではない（{n} バイト）"),
        Ok(Err(_)) => {} // リセット等のエラーもクローズの一種として許容
        Err(_) => panic!(
            "shutdown 発火後、WS セッションは grace（{grace:?}）を待たず\
             有界時間内にクローズされるはず"
        ),
    }

    // (b) `run_until` 自体も grace を待ち切らず速やかに戻る（既存の
    // 「grace + ε 以内に必ず戻る」フェイルセーフに加え、キャンセルにより
    // permit が早期解放されるため、grace 全体を待たずに戻ることを確認する）。
    let started = std::time::Instant::now();
    timeout(grace, run_task)
        .await
        .expect("run_until は grace 期間内に終了するはず")
        .expect("run_until タスクが panic しないこと")
        .expect("run_until は Ok(()) を返すはず");
    assert!(
        started.elapsed() < grace,
        "run_until はキャンセルによる早期 permit 解放で grace（{grace:?}）\
         を待ち切らずに戻るはず（実際: {:?}）",
        started.elapsed()
    );
}

/// rebind 世代 drain（イシュー #485/#488）発火が、旧世代で委譲済みの WS
/// セッションへ伝播することを確認する（設計 5.2 節「drain 開始時に発火」）。
///
/// 旧世代で WS セッションを張ったまま rebind し、(a) 旧世代の WS クライアント
/// が有界時間内に EOF を観測すること、(b) 新アドレスでの通常 HTTP リクエスト
/// が継続して処理されることを確認する。
#[tokio::test(flavor = "multi_thread")]
async fn rebind_cancels_old_generation_websocket_session() {
    let grace = Duration::from_secs(10);
    let server = Server::new()
        .websocket(WebSocketConfig::default())
        .handler(FixedHandler)
        .shutdown_grace_period(grace);
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let old_addr = bound.local_addr().unwrap();
    let rebind = bound.rebind_handle();

    let run_task = tokio::spawn(async move { bound.run().await });

    // 旧世代で WS セッションを確立する。
    let mut ws_client = TcpStream::connect(old_addr).await.unwrap();
    ws_client.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();
    let response_head = timeout(Duration::from_secs(5), read_response_head(&mut ws_client))
        .await
        .expect("ハンドシェイク応答は有界時間内に届くはず");
    assert!(
        response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
        "実際: {response_head}"
    );

    // rebind する（旧世代を切り離し、drain 開始時にキャンセルを発火する）。
    let new_addr = timeout(Duration::from_secs(5), rebind.rebind("127.0.0.1:0"))
        .await
        .expect("rebind はタイムアウトせず完了するはず")
        .expect("bind 可能な新アドレスへの rebind は成功するはず");

    // (b) 新世代アドレスでの通常 HTTP リクエストが継続して処理されることを
    // 確認する（世代キャンセルが新世代の accept ループに悪影響を与えない
    // ことの確認）。
    let mut http_client = TcpStream::connect(new_addr).await.unwrap();
    http_client
        .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let new_gen_head = timeout(Duration::from_secs(5), read_response_head(&mut http_client))
        .await
        .expect("新世代アドレスへの HTTP リクエストは有界時間内に応答するはず");
    assert!(
        new_gen_head.starts_with("HTTP/1.1 200 OK\r\n"),
        "実際: {new_gen_head}"
    );

    // (a) 旧世代 WS セッションが有界時間内にクローズされることを確認する。
    let mut probe = [0u8; 1];
    let read_result = timeout(Duration::from_secs(5), ws_client.read(&mut probe)).await;
    match read_result {
        Ok(Ok(0)) => {}
        Ok(Ok(n)) => panic!("キャンセル後にデータを受信すべきではない（{n} バイト）"),
        Ok(Err(_)) => {}
        Err(_) => panic!(
            "rebind 発火後、旧世代の WS セッションは grace（{grace:?}）を待たず\
             有界時間内にクローズされるはず"
        ),
    }

    run_task.abort();
}
