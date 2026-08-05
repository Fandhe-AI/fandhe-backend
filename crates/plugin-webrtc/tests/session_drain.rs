//! イシュー #498: 世代キャンセル機構の水平展開（コア側の最終 shutdown・rebind 両経路が
//! 呼ぶ [`close_active_peers`] / [`drain_for_shutdown`]）の実 `RTCPeerConnection` を
//! 使った統合テスト。
//!
//! `crates/plugin-webrtc/tests/webrtc_datachannel.rs` と同型のローカルループバック
//! シグナリングでサーバ側 `Active` エントリを 1 件確立し、drain API 呼び出し後に
//! レジストリが空になる・接続状態が `Closed` へ遷移することを検証する
//! （`crates/core` 側の配線は `crates/core/tests/webrtc_cancellation.rs` が担う）。

use std::net::SocketAddr;
use std::time::Duration;

use fandhe_backend_http::buffer::RecvBuffer;
use fandhe_backend_http::connection::read_request;
use fandhe_backend_plugin_webrtc::{WebRtcConfig, close_active_peers, drain_for_shutdown};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

/// `webrtc_datachannel.rs::spawn_server` と同一の最小 1 リクエスト完結サーバ。
async fn spawn_server(config: WebRtcConfig) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let config = config.clone();
            tokio::spawn(async move {
                let mut buf = RecvBuffer::new();
                let request = match read_request(&mut stream, &mut buf).await {
                    Ok(Some(request)) => request,
                    _ => return,
                };
                let response = match fandhe_backend_plugin_webrtc::try_handle_rtc_offer(
                    &request.head,
                    &request.body,
                    &config,
                )
                .await
                {
                    Some(response) => response,
                    None => fandhe_backend_http::response::Response::empty(404),
                };
                let _ = stream.write_all(&response.serialize(false)).await;
            });
        }
    });
    addr
}

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
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
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

/// クライアント側 `RTCPeerConnection` を構築し、サーバ側 `try_handle_rtc_offer` との
/// シグナリングを 1 往復完了させる（サーバ側にレジストリ `Active` エントリを 1 件
/// 作る共通手順）。データチャネル `on_open` 通知用の受信側も返す（呼び出し元が
/// 「確立を待つ」か「確立しないことを確認する」かを選べるようにするため）。
async fn signal_one_peer(
    addr: SocketAddr,
) -> (
    webrtc::peer_connection::RTCPeerConnection,
    tokio::sync::mpsc::Receiver<()>,
) {
    let client = build_client_peer_connection().await;
    let data_channel = client
        .create_data_channel("session-drain", None)
        .await
        .unwrap();
    let (open_tx, open_rx) = tokio::sync::mpsc::channel::<()>(1);
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
        tokio::time::timeout(Duration::from_secs(10), post_offer(addr, &offer_json))
            .await
            .unwrap();
    assert!(
        status_line.starts_with("HTTP/1.1 200 OK"),
        "signaling failed: {status_line}, body: {answer_body}"
    );
    let answer: RTCSessionDescription = serde_json::from_str(&answer_body).unwrap();
    client.set_remote_description(answer).await.unwrap();

    (client, open_rx)
}

/// [`signal_one_peer`] に加え、ICE/DTLS が実際に確立してデータチャネルが開くまで
/// 待つ（サーバ側の drain 対象を「確立済み Active 接続」に確定させたい呼び出し元向け。
/// ここで待たずに drain すると、まだ ICE checking 中の接続に対して close() を呼ぶ
/// ことになり、client 側が有界時間内に終端状態へ遷移せず「Connecting」のまま固まり
/// うる。`webrtc_datachannel.rs::datachannel_roundtrip_over_local_loopback` と同じ
/// 待ち方）。
async fn signal_established_peer(addr: SocketAddr) -> webrtc::peer_connection::RTCPeerConnection {
    let (client, mut open_rx) = signal_one_peer(addr).await;
    tokio::time::timeout(Duration::from_secs(10), open_rx.recv())
        .await
        .expect("data channel did not open in time");
    client
}

/// rebind 相当の drain（[`close_active_peers`]）: Active な接続がサーバ側で
/// close されるまでポーリングする（クライアント側は close しない状態からサーバ側が
/// 能動的に close することを確認する、イシュー #498 の主眼）。
#[tokio::test]
async fn close_active_peers_closes_established_connection() {
    let config = WebRtcConfig::new();
    let addr = spawn_server(config.clone()).await;

    let client = signal_established_peer(addr).await;

    close_active_peers(&config, Duration::from_secs(5)).await;

    // レジストリが空になっていることを、新規シグナリングが引き続き受理・確立できる
    // （＝上限判定に旧エントリが残っていない）ことで間接的に確認する
    // （`reserve_slot` は `pub(crate)` のため統合テストから直接参照できない）。
    let second_client = signal_established_peer(addr).await;
    let _ = second_client.close().await;

    // サーバ側からの close によりクライアント側も最終的に Closed/Disconnected へ
    // 遷移することをポーリング確認する（DTLS クローズシーケンスの非同期検知）。
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
            "client 側が有界時間内に終端状態へ遷移しなかった（現在の状態: {state:?}）"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = client.close().await;
}

/// 最終 shutdown 相当の drain（[`drain_for_shutdown`]）: 既存の Active 接続を
/// close するだけでなく、以降の新規シグナリングもレジストリへ登録されなくなることを
/// 確認する（`WebRtcConfig::activate_slot` のフェイルクローズ判定）。
#[tokio::test]
async fn drain_for_shutdown_rejects_subsequent_activation() {
    let config = WebRtcConfig::new();
    let addr = spawn_server(config.clone()).await;

    let first_client = signal_established_peer(addr).await;

    drain_for_shutdown(&config, Duration::from_secs(5)).await;

    // 終端 drain 開始後もシグナリング自体（reserve_slot・SDP 交換）は継続受理するが、
    // complete_signaling 内の activate_slot がレジストリ登録を拒否するため、
    // 200 応答自体は返る（クライアントは Answer を受け取れる）ものの、サーバ側は
    // 直後に pc を明示的に close する（`handler::complete_signaling` の doc を参照）。
    // サーバ側が早期に close するため、このデータチャネルは決して `open` へ到達
    // しない（有界時間内に到達しないことを確認する）。
    let (second_client, mut second_open_rx) = signal_one_peer(addr).await;
    let opened = tokio::time::timeout(Duration::from_secs(5), second_open_rx.recv()).await;
    assert!(
        opened.is_err(),
        "終端 drain 後の新規接続はレジストリへ登録されず即座に close されるため、\
         データチャネルが開いてはならない"
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let state = second_client.connection_state();
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
            "終端 drain 後の新規接続が有界時間内に終端状態へ遷移しなかった（現在の状態: {state:?}）"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let _ = first_client.close().await;
    let _ = second_client.close().await;
}
