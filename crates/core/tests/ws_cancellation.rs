//! 世代キャンセルシグナルの WS 委譲タスクへの配線（イシュー #491・#492）の
//! 統合テスト。
//!
//! `docs/design/ws-cancellation-propagation.md` が確定した設計（世代別
//! `tokio::sync::watch` + drain 開始時発火）を、最終 graceful shutdown
//! （#313）・rebind 世代 drain（#485/#488）の両経路について実 TCP 接続で
//! 検証する。`crates/core/src/plugin.rs` の `try_handle_upgrade` はキャンセル
//! `Future` を `fandhe_backend_plugin_websocket::handle_upgrade` の第 5 引数
//! として渡すのみで、切断シーケンス（正常な Close ハンドシェイク、close
//! code 1001 Going Away → `CLOSE_GRACE` 上限で応答待ち）は `handle_upgrade`
//! 側が担う（イシュー #492、`try_handle_upgrade` の doc「世代キャンセル
//! シグナル」を参照）。
//!
//! - `final_shutdown_cancels_delegated_websocket_session`: 最終 shutdown
//!   発火後、委譲済み WS セッションが有界時間内に Close フレーム（1001）を
//!   受信して EOF に至り、`run_until` も grace を待ち切らず速やかに戻ることを
//!   確認する
//! - `rebind_cancels_old_generation_websocket_session`: rebind 発火後、
//!   旧世代の WS セッションが有界時間内に Close フレーム（1001）を受信して
//!   EOF に至り、新世代アドレスへの通常 HTTP リクエストは継続して処理される
//!   ことを確認する
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

/// クライアント側の生 TCP ストリームから Close フレーム（RFC 6455）を
/// 有界時間内に読み切り、close code が 1001（Going Away）であることを
/// 検証する（`crates/plugin-websocket/src/session.rs` の
/// `handle_cancellation` が送出するフレームと対応、イシュー #492）。
///
/// フレーミング実装自体（reason 文字列・応答ドレイン等）は
/// `plugin-websocket` 側の `tests/cancellation.rs` が既に検証済みのため、
/// 本テストはヘッダ + close code の 4 バイトのみを検証する最小限の手読み
/// パーサとする（`tokio-tungstenite` 等のクライアントライブラリは使わず、
/// 本テストが検証したい対象 — `try_handle_upgrade` からの配線 — に絞る）。
/// 呼び出し元は検証後、`ws_client` を drop してサーバ側のドレインを即座に
/// 完了させる（クライアントが Close 応答を返さないケースの検証は
/// `plugin-websocket` 側の `cancellation.rs` が担う。本テストの主眼は
/// `run_until` の早期復帰・permit 解放であり、`CLOSE_GRACE` 全体を待たせ
/// ないため）。
async fn read_close_frame_1001(stream: &mut TcpStream, bound: Duration) {
    let mut header = [0u8; 2];
    timeout(bound, stream.read_exact(&mut header))
        .await
        .expect("Close フレームヘッダは有界時間内に届くはず")
        .expect("Close フレームヘッダの読み取りに失敗した");
    // RFC 6455: 先頭バイトは FIN(1) + opcode(0x8 = Close)、2 バイト目は
    // MASK(0, サーバ→クライアントは非マスク) + payload length。
    assert_eq!(
        header[0] & 0x0f,
        0x8,
        "opcode は Close(0x8) のはず: {header:?}"
    );
    let payload_len = (header[1] & 0x7f) as usize;
    assert!(
        payload_len >= 2,
        "close code を含む payload のはず（payload_len={payload_len}）"
    );
    let mut payload = vec![0u8; payload_len];
    timeout(bound, stream.read_exact(&mut payload))
        .await
        .expect("Close フレーム payload は有界時間内に届くはず")
        .expect("Close フレーム payload の読み取りに失敗した");
    let code = u16::from_be_bytes([payload[0], payload[1]]);
    assert_eq!(
        code, 1001,
        "close code は 1001 Going Away のはず: {payload:?}"
    );
}

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
/// 時間内に Close フレーム（1001 Going Away）を受信すること、(b) `run_until`
/// 自体が `shutdown_grace_period` を待ち切らずに速やかに `Ok(())` で戻ること
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

    // (a) WS セッションが有界時間内に Close フレーム（1001 Going Away）を
    // 送出する（キャンセル発火 → `handle_upgrade` が正常な Close
    // ハンドシェイクを開始、イシュー #492）ことを確認する。grace（10 秒）
    // よりも十分短い上限で観測できるはず。
    read_close_frame_1001(&mut ws_client, Duration::from_secs(5)).await;
    // クライアント側から即座に接続を閉じ、サーバ側のドレインを
    // `CLOSE_GRACE` 全体を待たずに完了させる（run_until の早期復帰を
    // 検証する本テストの主眼のため）。
    drop(ws_client);

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
/// が有界時間内に Close フレーム（1001 Going Away）を受信すること、(b) 新
/// アドレスでの通常 HTTP リクエストが継続して処理されることを確認する。
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

    // (a) 旧世代 WS セッションが有界時間内に Close フレーム（1001 Going
    // Away）を受信することを確認する（イシュー #492）。
    read_close_frame_1001(&mut ws_client, Duration::from_secs(5)).await;
    drop(ws_client);

    run_task.abort();
}
