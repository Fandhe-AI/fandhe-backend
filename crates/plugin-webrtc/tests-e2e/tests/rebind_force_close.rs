//! `BoundServer::rebind_handle().rebind()` が確立済み `RTCPeerConnection` を
//! 実際に強制切断することのエンドツーエンド検証（イシュー #498 レビュー対応・
//! イシュー #507 で standalone crate として再設計）。
//!
//! `crates/core/tests/webrtc_cancellation.rs` は「core の配線契約」（`SessionDrain::fire`
//! が正しいタイミングで呼ばれ accept ループ自体を破壊しない）に責務を限定し、実
//! ICE/DTLS ハンドシェイクを伴う `webrtc` クレートを dev-dependencies に持ち込まない
//! 方針を明記している。一方 `crates/plugin-webrtc/tests/session_drain.rs` は実接続を
//! 確立するが、`close_active_peers`/`drain_for_shutdown` を直接呼ぶのみで、
//! `crates/core` の `RebindHandle::rebind` 経由の配線（`RebindHandle::rebind` の
//! doc「rebind と無関係な進行中の WebRTC 通話も強制切断される」という契約）は
//! どちらのテストからも検証されない欠落があった。
//!
//! 本クレートは `fandhe-backend-core`（`webrtc` feature）とクライアント側
//! `RTCPeerConnection` 用の `webrtc` クレートを同時に依存として持つ。これを
//! `crates/plugin-webrtc` の通常 dev-dependencies に置くと package レベルの循環
//! （core → plugin-webrtc は通常依存、plugin-webrtc → core は dev-dependency）が
//! `cargo metadata` の resolve グラフ上に常時現れてしまい、pay-for-what-you-use
//! 検証（`scripts/pay-for-what-you-use-check.sh` (c) `cargo geiger`）を偽陽性 FAIL
//! させる（PR #506 参照）。そのため本クレートは root workspace から独立した
//! standalone crate（`crates/http/fuzz` と同パターン、root `Cargo.toml` の
//! `[workspace] exclude` + 本クレート自身の空 `[workspace]` テーブル）として配置し、
//! `fandhe_backend_core::plugin_webrtc::WebRtcConfig`（イシュー #435 の再エクスポート）
//! 経由で `fandhe-backend-plugin-webrtc` への直接依存を持たずに構成する。
//! 実行は `scripts/webrtc-e2e.sh` 経由（CI は `webrtc-e2e` ジョブ）。

use std::net::SocketAddr;
use std::time::Duration;

use fandhe_backend_core::Server;
use fandhe_backend_core::plugin_webrtc::WebRtcConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

async fn post_offer(addr: SocketAddr, offer_json: &str) -> (String, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let raw = format!(
        "POST /rtc/offer HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        offer_json.len(),
        offer_json
    );
    stream.write_all(raw.as_bytes()).await.unwrap();

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
    let mut parts = text.splitn(2, "\r\n\r\n");
    let status_line = parts
        .next()
        .unwrap()
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    let body = parts.next().unwrap_or("").to_string();
    (status_line, body)
}

async fn build_client_peer_connection() -> webrtc::peer_connection::RTCPeerConnection {
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs().unwrap();
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media_engine).unwrap();
    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .build();
    api.new_peer_connection(RTCConfiguration::default())
        .await
        .unwrap()
}

/// クライアント側 `RTCPeerConnection` を構築し、`addr` の `Server::webrtc` 登録済み
/// エンドポイントとのシグナリングを実行、データチャネルが開くまで待つ
/// （`crates/plugin-webrtc/tests/session_drain.rs::signal_established_peer` と同型。
/// サーバ実装がループバック手製サーバか `fandhe-backend-core::Server` かの違いのみ）。
async fn signal_established_peer(addr: SocketAddr) -> webrtc::peer_connection::RTCPeerConnection {
    let client = build_client_peer_connection().await;
    let data_channel = client
        .create_data_channel("rebind-force-close", None)
        .await
        .unwrap();
    let (open_tx, mut open_rx) = tokio::sync::mpsc::channel::<()>(1);
    data_channel.on_open(Box::new(move || {
        let _ = open_tx.try_send(());
        Box::pin(async {})
    }));

    let offer = client.create_offer(None).await.unwrap();
    let mut gather_complete = client.gathering_complete_promise().await;
    client.set_local_description(offer).await.unwrap();
    let _ = gather_complete.recv().await;
    let local_desc = client.local_description().await.unwrap();
    let offer_json = serde_json::to_string(&local_desc).unwrap();

    let (status_line, answer_body) =
        timeout(Duration::from_secs(10), post_offer(addr, &offer_json))
            .await
            .unwrap();
    assert!(
        status_line.starts_with("HTTP/1.1 200 OK"),
        "signaling failed: {status_line}, body: {answer_body}"
    );
    let answer: RTCSessionDescription = serde_json::from_str(&answer_body).unwrap();
    client.set_remote_description(answer).await.unwrap();

    timeout(Duration::from_secs(10), open_rx.recv())
        .await
        .expect("data channel did not open in time");

    client
}

/// `BoundServer::rebind_handle().rebind()` が、rebind と無関係な確立済み
/// `RTCPeerConnection` を強制切断することを end-to-end で確認する
/// （`RebindHandle::rebind` の doc「rebind と無関係な進行中の WebRTC 通話も
/// 強制切断される」契約、イシュー #498 レビュー対応）。
#[tokio::test(flavor = "multi_thread")]
async fn rebind_force_closes_established_webrtc_session() {
    let grace = Duration::from_secs(5);
    let server = Server::new()
        .webrtc(WebRtcConfig::new())
        .shutdown_grace_period(grace);
    let mut bound = server.bind("127.0.0.1:0").await.unwrap();
    let initial_addr = bound.local_addr().unwrap();
    let rebind = bound.rebind_handle();

    let run_task = tokio::spawn(async move { bound.run().await });

    // 初期アドレスへ実 ICE/DTLS シグナリングを行い、サーバ側レジストリに
    // Active エントリを 1 件確立する。
    let client = signal_established_peer(initial_addr).await;
    assert_eq!(client.connection_state(), RTCPeerConnectionState::Connected);

    // rebind 発火(新規リスニングアドレスへの切り替えのみが目的で、この
    // `RTCPeerConnection` とは無関係)。`SessionDrain::fire(false)` が
    // 同時に発火し、レジストリ上のアクティブ接続全件へ明示的 close() を試みる。
    let _new_addr = timeout(Duration::from_secs(5), rebind.rebind("127.0.0.1:0"))
        .await
        .expect("rebind はタイムアウトせず完了するはず")
        .expect("bind 可能な新アドレスへの rebind は成功するはず");

    // rebind と無関係だったはずのクライアント側接続が、サーバ側からの明示的
    // close() を受けて有界時間内に終端状態へ遷移することを確認する
    // （`RebindHandle::rebind` doc の副作用契約が実際に配線されていることの証跡）。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let state = client.connection_state();
        if matches!(
            state,
            RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected
                | RTCPeerConnectionState::Failed
        ) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "rebind 後もクライアント側が有界時間内に終端状態へ遷移しなかった \
             （現在の状態: {state:?}）。RebindHandle::rebind から SessionDrain への \
             配線が壊れている可能性がある"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let _ = client.close().await;
    run_task.abort();
}
