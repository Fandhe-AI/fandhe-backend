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
//! - `final_shutdown_returns_within_grace_even_if_ws_client_ignores_close`
//!   （イシュー #493）: Close フレームを受信しても応答・切断せず居座る
//!   クライアントに対しても、(a) `run_until` 自体は permit 回収タイムアウトの
//!   フェイルセーフにより有界時間内に必ず復帰し、(b) サーバ側は
//!   `CLOSE_GRACE`（`crates/plugin-websocket/src/session.rs` 固定 10 秒）を
//!   上限に接続を強制終端することを end-to-end で検証する（設計 5.3 節・
//!   8 節。「grace 超過後にクローズ」という受け入れ条件を、確定済み設計の
//!   意味論——非協調クライアントは `CLOSE_GRACE` 有界で終端——で検証する）
//! - `repeated_rebind_releases_ws_permits_without_monotonic_consumption`
//!   （イシュー #493）: `max_connections(1)` の極小構成で rebind を 3 回
//!   反復し、旧世代 WS セッションの permit が世代を跨いで単調消費されず
//!   毎回解放されることを検証する（1 個でもリークすれば 2 周目以降が
//!   有界 timeout で確実に fail する決定的判定）
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

/// イシュー #493 の受け入れ条件 1（居座りクライアントに対する最終 shutdown
/// 経路の end-to-end 検証）。上記 2 テストは Close フレーム受信後にクライア
/// ントが即座に接続を閉じる協調的なケースのみを検証しており、Close に一切
/// 応答しない非協調クライアントに対しては未検証だった。
///
/// クライアントは Close フレーム（1001）を有界時間内に**読むが、応答も
/// 切断もせず居座る**。この状態でも (a) `run_until` が permit 回収
/// タイムアウトのフェイルセーフにより有界時間内に必ず `Ok(())` で復帰し、
/// (b) サーバ側は `CLOSE_GRACE`（固定 10 秒、`crates/plugin-websocket/src/
/// session.rs`）を上限にソケットを強制終端して EOF を観測させることを
/// 確認する（設計 5.3 節「Close に応答しないクライアントも `CLOSE_GRACE`
/// で有界に終端する」、8 節「既知の限界」との対応）。
///
/// grace（1 秒）に対して検証上限を大きく取り（(a) は 8 秒、(b) は
/// `CLOSE_GRACE` 10 秒 + 余裕の 20 秒）、self-hosted runner の輻輳下でも
/// flaky にならないようにする。経過時間の下限は assert しない（輻輳下では
/// 上振れうるため、有界性のみを検証する）。
#[tokio::test(flavor = "multi_thread")]
async fn final_shutdown_returns_within_grace_even_if_ws_client_ignores_close() {
    let grace = Duration::from_secs(1);
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

    shutdown_tx.send(()).unwrap();

    // Close フレームは読むが、応答も切断もしない（居座り）。
    read_close_frame_1001(&mut ws_client, Duration::from_secs(5)).await;

    // (a) `run_until` は permit 回収タイムアウトのフェイルセーフにより、
    // クライアントが居座っていても有界時間内に必ず復帰する。grace（1 秒）
    // に対し緩めの上限（8 秒）で有界性のみを判定する。
    timeout(Duration::from_secs(8), run_task)
        .await
        .expect("run_until は居座りクライアントがいても有界時間内に戻るはず")
        .expect("run_until タスクが panic しないこと")
        .expect("run_until は Ok(()) を返すはず");

    // (b) 居座りクライアントは `CLOSE_GRACE`（10 秒）を上限にサーバ側から
    // 強制終端され EOF（read が 0 バイト）を観測する。「grace 超過後も
    // 接続が生き続ける」旧制約（#492 実装前のハードクローズ経路に依存した
    // 記述）が解消済みであることを end-to-end で確認する。
    let mut trailing = [0u8; 1];
    let n = timeout(Duration::from_secs(20), ws_client.read(&mut trailing))
        .await
        .expect("居座りクライアントも CLOSE_GRACE 有界で強制終端されるはず")
        .expect("強制終端は EOF（Ok(0)）として観測されるはず、エラーではない");
    assert_eq!(
        n, 0,
        "CLOSE_GRACE 超過後はサーバ側から接続が閉じられ EOF になるはず"
    );
}

/// イシュー #493 の受け入れ条件 2（rebind 反復での permit 単調消費なし）。
/// `max_connections(1)` の極小構成で rebind を 3 回反復し、旧世代 WS
/// セッションが握る permit が世代を跨いで解放されないまま累積しないことを
/// 検証する（設計 5.2 節。permit は世代を跨いで共有する `connection_limit`
/// セマフォ経由のため、1 個でもリークすれば 2 周目以降の accept が
/// `max_connections(1)` によりブロックされ、有界 timeout で確実に fail する
/// 決定的判定になる）。
#[tokio::test(flavor = "multi_thread")]
async fn repeated_rebind_releases_ws_permits_without_monotonic_consumption() {
    let server = Server::new()
        .websocket(WebSocketConfig::default())
        .handler(FixedHandler)
        .max_connections(1)
        .shutdown_grace_period(Duration::from_secs(10));
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let mut addr = bound.local_addr().unwrap();
    let rebind = bound.rebind_handle();

    let run_task = tokio::spawn(async move { bound.run().await });

    for iteration in 0..3u32 {
        // 現行アドレスへ WS ハンドシェイクを張る。max_connections(1) の
        // 唯一の permit を WS セッションが保持する。
        let mut ws_client = TcpStream::connect(addr).await.unwrap();
        ws_client.write_all(VALID_HANDSHAKE_REQUEST).await.unwrap();
        let response_head = timeout(Duration::from_secs(5), read_response_head(&mut ws_client))
            .await
            .unwrap_or_else(|_| {
                panic!("iteration={iteration}: ハンドシェイク応答は有界時間内に届くはず")
            });
        assert!(
            response_head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
            "iteration={iteration} 実際: {response_head}"
        );

        // rebind する（rebind コマンドは accept ループより優先ポーリング
        // されるため、permit 枯渇中でも進む。旧世代を切り離し、drain 開始
        // 時にキャンセルを発火する）。
        let new_addr = timeout(Duration::from_secs(5), rebind.rebind("127.0.0.1:0"))
            .await
            .unwrap_or_else(|_| {
                panic!("iteration={iteration}: rebind はタイムアウトせず完了するはず")
            })
            .unwrap_or_else(|e| panic!("iteration={iteration}: rebind は成功するはず: {e}"));

        // 旧世代 WS クライアントで Close フレーム（1001）を読み、即座に
        // drop してサーバ側のドレインを完了させる（permit 解放）。
        read_close_frame_1001(&mut ws_client, Duration::from_secs(5)).await;
        drop(ws_client);

        // 新アドレスへ通常 HTTP リクエストを送る。max_connections(1) の
        // ため、この 200 応答は旧世代 WS の permit が確実に解放された
        // 場合にのみ観測できる（permit リークがあれば accept 自体が
        // ブロックされ、この timeout が fail する）。
        let mut http_client = TcpStream::connect(new_addr).await.unwrap();
        http_client
            .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let new_gen_head = timeout(Duration::from_secs(5), read_response_head(&mut http_client))
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "iteration={iteration}: permit がリークしていれば新世代への \
                     HTTP リクエストが有界時間内に応答しないはず（permit 単調消費の検出）"
                )
            });
        assert!(
            new_gen_head.starts_with("HTTP/1.1 200 OK\r\n"),
            "iteration={iteration} 実際: {new_gen_head}"
        );
        drop(http_client);

        addr = new_addr;
    }

    run_task.abort();
}
