//! 世代キャンセル機構の WS 以外の長時間委譲プラグインへの水平展開（イシュー #498）の
//! コア配線側統合テスト。第 1 弾として `webrtc` feature（`crates/plugin-webrtc`、パス
//! インターセプト型）を対象に、最終 graceful shutdown（#313）・rebind 世代 drain
//! （#485/#488）の両経路が `crate::plugin::SessionDrain::fire` を呼んでもコアの accept
//! ループ・シグナリング処理自体が壊れないこと（配線の非破壊性）を検証する
//! （`crates/core/tests/ws_cancellation.rs`（イシュー #491〜#493、WS 側の水平展開元）と
//! 対をなす）。
//!
//! # 実セッション確立の検証は本ファイルの責務外（`crates/plugin-webrtc` 側に委譲）
//!
//! `crates/core/tests/plugin_boundary_webrtc.rs` の既存方針（「実データチャネル疎通の
//! 検証は `crates/plugin-webrtc/tests/webrtc_datachannel.rs` に委ね、core に
//! `webrtc-rs` 由来の dev-dep を持ち込まない」）を踏襲し、本ファイルも実 ICE/DTLS
//! ハンドシェイクを行う `webrtc` クレートを core の dev-dependencies に追加しない。
//!
//! `SessionDrain::fire` が実際にアクティブな `RTCPeerConnection` を close する
//! （`WebRtcConfig::begin_terminal_drain`・`take_active_peers`・`activate_slot` の
//! フェイルクローズ判定を含む）という核心の振る舞いは
//! `crates/plugin-webrtc/tests/session_drain.rs`
//! （`close_active_peers_closes_established_connection`・
//! `drain_for_shutdown_rejects_subsequent_activation`）が実 ICE/DTLS で直接検証する。
//! 本ファイルは「コアの `run_until`（最終 shutdown・rebind 両経路）が
//! `SessionDrain::fire` を正しいタイミング・`is_final` 値で呼び、かつ呼び出し自体が
//! 通常の accept ループ・シグナリング処理を破壊しない」というコア側の配線契約に
//! 責務を限定する。
//!
//! - `final_shutdown_completes_with_webrtc_registered`: `Server::webrtc` 登録済み・
//!   Active な接続が 0 件の状態で最終 shutdown を発火しても `run_until` が
//!   grace 期間内に正常終了することを確認する（`SessionDrain::fire(true)` が
//!   空レジストリに対しても安全に no-op で完了する契約、`docs/design/
//!   ws-cancellation-propagation.md` 10 節参照）
//! - `rebind_keeps_webrtc_signaling_available_on_new_generation`: rebind 発火
//!   （`SessionDrain::fire(false)` が同時に発火）後も、新世代アドレスへの
//!   `POST /rtc/offer` シグナリングが引き続き正常に処理される（フェイルクローズの
//!   誤爆で機能停止しない）ことを確認する
//! - 既存 `graceful_shutdown.rs` / `rebind.rs` / `plugin_boundary_webrtc.rs` が
//!   無変更で pass すること（非退行）はテストスイート全体の実行で別途確認する

#![cfg(feature = "webrtc")]

use std::time::Duration;

use fandhe_backend_core::Server;
use fandhe_backend_plugin_webrtc::WebRtcConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// 不正な Offer（JSON パース不能）を `addr` の `POST /rtc/offer` へ送り、ステータス行を
/// 返す。実 ICE/DTLS ハンドシェイクを伴わないため `webrtc` クレートを要求しない
/// （`try_handle_rtc_offer` の入力検証経路のみを駆動する。
/// `crates/plugin-webrtc/src/handler.rs::tests::invalid_json_offer_is_rejected` と
/// 同型の入力）。この経路は `reserve_slot` → JSON パース失敗 → `release_slot` で
/// 完結し `activate_slot`（Active 化）までは到達しないが、「コアの配線がプラグインへ
/// 正しく到達し続けていること」の確認には十分である。
async fn post_invalid_offer(addr: std::net::SocketAddr) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body = b"not json";
    let raw = format!(
        "POST /rtc/offer HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(raw.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .unwrap()
            .unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8(buf).unwrap();
    text.lines().next().unwrap_or("").to_string()
}

/// `Server::webrtc` 登録済み（Active な接続は 0 件）の状態で最終 graceful shutdown
/// を発火しても、`SessionDrain::fire(true)`（`crate::plugin::SessionDrain`、
/// イシュー #498）が空レジストリに対して安全に no-op で完了し、`run_until` が
/// grace 期間内に正常終了することを確認する（既存の grace・強制クローズ機構への
/// 非破壊性、受け入れ基準 4「pay-for-what-you-use・既存機構の非退行」）。
#[tokio::test(flavor = "multi_thread")]
async fn final_shutdown_completes_with_webrtc_registered() {
    let grace = Duration::from_secs(5);
    let server = Server::new()
        .webrtc(WebRtcConfig::new())
        .shutdown_grace_period(grace);
    let bound = server.bind("127.0.0.1:0").await.unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let run_task = tokio::spawn(async move {
        bound
            .run_until(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    shutdown_tx.send(()).unwrap();

    timeout(grace, run_task)
        .await
        .expect("run_until は grace 期間内に終了するはず")
        .expect("run_until タスクが panic しないこと")
        .expect("run_until は Ok(()) を返すはず");
}

/// rebind（イシュー #485/#488、旧世代 drain で `SessionDrain::fire(false)` が
/// 同時に発火する、イシュー #498）後も、新世代アドレスへの WebRTC シグナリングが
/// 引き続き正常に処理されることを確認する（`WebRtcConfig` は世代を跨いで共有され、
/// `fire(false)` は終端 drain フラグを立てないため、新規シグナリングの受理には
/// 影響しないはずという設計契約の検証）。
#[tokio::test(flavor = "multi_thread")]
async fn rebind_keeps_webrtc_signaling_available_on_new_generation() {
    let grace = Duration::from_secs(5);
    let server = Server::new()
        .webrtc(WebRtcConfig::new())
        .shutdown_grace_period(grace);
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let rebind = bound.rebind_handle();

    let run_task = tokio::spawn(async move { bound.run().await });

    let new_addr = timeout(Duration::from_secs(5), rebind.rebind("127.0.0.1:0"))
        .await
        .expect("rebind はタイムアウトせず完了するはず")
        .expect("bind 可能な新アドレスへの rebind は成功するはず");

    // 新世代アドレスへのシグナリングが引き続き処理される（フォールスルーせず
    // try_intercept が到達する）ことを、入力検証経路の応答で確認する。
    let status_line = timeout(Duration::from_secs(5), post_invalid_offer(new_addr))
        .await
        .expect("新世代アドレスへのシグナリングは有界時間内に応答するはず");
    assert!(
        status_line.starts_with("HTTP/1.1 400"),
        "実際: {status_line}"
    );

    run_task.abort();
}
