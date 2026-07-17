//! TASK-8.1（#26）: `try_handle_rtc_offer` の 1 対 1 DataChannel 疎通確認。
//!
//! シグナリングは本クレートが公開する [`try_handle_rtc_offer`] を実際の HTTP 経由
//! （ローカルループバックの最小 TCP サーバ）で呼び出し、DataChannel 確立後のメッセージ
//! 往復（エコー）を検証する。ネットワークはローカルループバックのみで完結させる
//! （`RTCConfiguration` に STUN/TURN サーバを設定しない、
//! `docs/spec/03-poc/webrtc-plugin/core/tests/webrtc_datachannel.rs` PoC-5 実施制約を
//! 踏襲）。
//!
//! サーバ側は `bf_http::connection::read_request` → `try_handle_rtc_offer` →
//! `Response::serialize` という最小のリクエスト/レスポンスループのみを組み立てる
//! （コアの接続受理ループ・feature 配線は別途 `crates/core/tests/plugin_boundary_webrtc.rs`
//! が検証する。本テストはハンドラ単体の実疎通に責務を限定する）。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bf_http::buffer::RecvBuffer;
use bf_http::connection::read_request;
use bf_plugin_webrtc::{WebRtcConfig, try_handle_rtc_offer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

/// `POST /rtc/offer` のみに応答する最小 HTTP/1.1 サーバをループバックへ立てる。
///
/// 1 接続 1 リクエストのみを処理し、応答後に接続を閉じる（本テストの疎通確認には
/// keep-alive が不要なため、コア接続ループの複雑さを持ち込まない最小実装）。
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
                let response =
                    match try_handle_rtc_offer(&request.head, &request.body, &config).await {
                        Some(response) => response,
                        None => bf_http::response::Response::empty(404),
                    };
                let _ = stream.write_all(&response.serialize(false)).await;
            });
        }
    });
    addr
}

/// `POST /rtc/offer` を送り、`(ステータス行, レスポンスボディ)` を返す。
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

/// クライアント側の `RTCPeerConnection`・データチャネルを構築するヘルパー。
async fn build_client_peer_connection() -> Arc<webrtc::peer_connection::RTCPeerConnection> {
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs().unwrap();
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media_engine).unwrap();
    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .build();

    // STUN/TURN サーバは設定しない（ホスト候補のみでループバック内疎通させる、
    // `crate` ルート doc「セキュリティ上の位置づけ」を参照）。
    Arc::new(
        api.new_peer_connection(RTCConfiguration::default())
            .await
            .unwrap(),
    )
}

/// ローカル 2 ピア（クライアント側は本テスト内で構築、サーバ側は
/// [`try_handle_rtc_offer`] が応答）で 1 対 1 の DataChannel を確立し、
/// メッセージ往復（エコー）が成立することを確認する（受け入れ条件: `/rtc/offer` で
/// SDP Offer/Answer・データチャネルが疎通すること）。
#[tokio::test]
async fn datachannel_roundtrip_over_local_loopback() {
    let addr = spawn_server(WebRtcConfig::new()).await;

    let client_pc = build_client_peer_connection().await;
    let data_channel = client_pc
        .create_data_channel("task-8-1", None)
        .await
        .unwrap();

    let (open_tx, mut open_rx) = mpsc::channel::<()>(1);
    data_channel.on_open(Box::new(move || {
        let _ = open_tx.try_send(());
        Box::pin(async {})
    }));

    let (msg_tx, mut msg_rx) = mpsc::channel::<String>(1);
    data_channel.on_message(Box::new(move |msg: DataChannelMessage| {
        let msg_tx = msg_tx.clone();
        Box::pin(async move {
            let text = String::from_utf8(msg.data.to_vec()).unwrap_or_default();
            let _ = msg_tx.send(text).await;
        })
    }));

    let offer = client_pc.create_offer(None).await.unwrap();
    let mut gather_complete = client_pc.gathering_complete_promise().await;
    client_pc.set_local_description(offer).await.unwrap();
    let _ = gather_complete.recv().await;
    let local_desc = client_pc.local_description().await.unwrap();
    let offer_json = serde_json::to_string(&local_desc).unwrap();

    // シグナリング: 本クレートの `try_handle_rtc_offer` へ実際に HTTP 経由で送る。
    let (status_line, answer_body) =
        tokio::time::timeout(Duration::from_secs(10), post_offer(addr, &offer_json))
            .await
            .unwrap();
    assert!(
        status_line.starts_with("HTTP/1.1 200 OK"),
        "unexpected status: {status_line}, body: {answer_body}"
    );

    let answer: RTCSessionDescription = serde_json::from_str(&answer_body).unwrap();
    client_pc.set_remote_description(answer).await.unwrap();

    tokio::time::timeout(Duration::from_secs(10), open_rx.recv())
        .await
        .expect("data channel did not open in time");

    data_channel.send_text("task-8-1 hello").await.unwrap();

    let echoed = tokio::time::timeout(Duration::from_secs(10), msg_rx.recv())
        .await
        .expect("no echo received in time")
        .expect("channel closed without message");
    assert_eq!(echoed, "task-8-1 hello");

    let _ = client_pc.close().await;
}

/// レビュー指摘（イシュー #26）の再発防止テスト: `max_peer_connections(1)` で 1 件目の
/// 接続を確立してからクライアント側 `RTCPeerConnection::close()` を呼び、サーバ側の
/// クローズ検知（`register_close_handler`）でレジストリの枠が解放されて 2 件目の
/// シグナリングが `503` を返さず成功することを確認する（正常利用の蓄積のみで恒久的に
/// 503 化する回帰を防ぐ）。サーバ側 `RTCPeerConnectionState` が `Closed`/`Failed` へ
/// 遷移するのは ICE/DTLS 経由の非同期検知のため、猶予を長めに取りポーリングする
/// （`handler::tests::close_handler_releases_slot_when_state_becomes_closed` が
/// クローズ検知そのものをより直接的に検証する）。
#[tokio::test]
async fn peer_connection_slot_is_released_after_close_allowing_reuse() {
    let addr = spawn_server(WebRtcConfig::new().with_max_peer_connections(1)).await;

    // 1 件目: 確立してからクローズする（データチャネルを 1 つ持たせないと SDP に
    // m-line が含まれず `set_remote_description` が失敗するため、
    // `datachannel_roundtrip_over_local_loopback` と同様にチャネルを作成する）。
    let first_client = build_client_peer_connection().await;
    let _first_dc = first_client
        .create_data_channel("slot-release-1", None)
        .await
        .unwrap();
    let offer = first_client.create_offer(None).await.unwrap();
    let mut gather_complete = first_client.gathering_complete_promise().await;
    first_client.set_local_description(offer).await.unwrap();
    let _ = gather_complete.recv().await;
    let local_desc = first_client.local_description().await.unwrap();
    let offer_json = serde_json::to_string(&local_desc).unwrap();

    let (status_line, answer_body) =
        tokio::time::timeout(Duration::from_secs(10), post_offer(addr, &offer_json))
            .await
            .unwrap();
    assert!(
        status_line.starts_with("HTTP/1.1 200 OK"),
        "1 件目のシグナリングに失敗: {status_line}, body: {answer_body}"
    );
    let answer: RTCSessionDescription = serde_json::from_str(&answer_body).unwrap();
    first_client.set_remote_description(answer).await.unwrap();
    first_client.close().await.unwrap();

    // クローズ通知（サーバ側 RTCPeerConnectionState の Closed/Failed への遷移）が
    // 届くまで、ICE/DTLS の非同期検知の猶予を持たせつつポーリングする。
    let mut second_status = String::new();
    let mut second_body = String::new();
    for _ in 0..150 {
        let second_client = build_client_peer_connection().await;
        let _second_dc = second_client
            .create_data_channel("slot-release-2", None)
            .await
            .unwrap();
        let offer = second_client.create_offer(None).await.unwrap();
        let mut gather_complete = second_client.gathering_complete_promise().await;
        second_client.set_local_description(offer).await.unwrap();
        let _ = gather_complete.recv().await;
        let local_desc = second_client.local_description().await.unwrap();
        let offer_json = serde_json::to_string(&local_desc).unwrap();

        let (status_line, body) =
            tokio::time::timeout(Duration::from_secs(10), post_offer(addr, &offer_json))
                .await
                .unwrap();
        let _ = second_client.close().await;

        if status_line.starts_with("HTTP/1.1 200 OK") {
            second_status = status_line;
            second_body = body;
            break;
        }
        second_status = status_line;
        second_body = body;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    assert!(
        second_status.starts_with("HTTP/1.1 200 OK"),
        "1 件目クローズ後も枠が解放されず 2 件目が拒否された: {second_status}, body: {second_body}"
    );
}

/// 不正な Offer（JSON パース不能）を実際に HTTP 経由で送ると 400 が返ることを確認する
/// （受け入れ条件の裏付け: 入力検証がプラグイン層で機能していること）。
#[tokio::test]
async fn invalid_offer_over_http_returns_400() {
    let addr = spawn_server(WebRtcConfig::new()).await;

    let (status_line, body) =
        tokio::time::timeout(Duration::from_secs(5), post_offer(addr, "not json"))
            .await
            .unwrap();
    assert!(status_line.starts_with("HTTP/1.1 400"), "{status_line}");
    assert_eq!(body, r#"{"error":"invalid_offer_json"}"#);
}
