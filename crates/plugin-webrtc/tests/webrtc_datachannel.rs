//! TASK-8.1（#26）: `try_handle_rtc_offer` の 1 対 1 DataChannel 疎通確認。
//!
//! シグナリングは本クレートが公開する [`try_handle_rtc_offer`] を実際の HTTP 経由
//! （ローカルループバックの最小 TCP サーバ）で呼び出し、DataChannel 確立後のメッセージ
//! 往復（エコー）を検証する。ネットワークはローカルループバックのみで完結させる
//! （`RTCConfiguration` に STUN/TURN サーバを設定しない、
//! `docs/spec/03-poc/webrtc-plugin/core/tests/webrtc_datachannel.rs` PoC-5 実施制約を
//! 踏襲）。
//!
//! サーバ側は `fandhe_backend_http::connection::read_request` → `try_handle_rtc_offer` →
//! `Response::serialize` という最小のリクエスト/レスポンスループのみを組み立てる
//! （コアの接続受理ループ・feature 配線は別途 `crates/core/tests/plugin_boundary_webrtc.rs`
//! が検証する。本テストはハンドラ単体の実疎通に責務を限定する）。

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use std::sync::Arc;

use bytes::BytesMut;
use fandhe_backend_http::buffer::RecvBuffer;
use fandhe_backend_http::connection::read_request;
use fandhe_backend_plugin_webrtc::{WebRtcConfig, try_handle_rtc_offer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelState};
use webrtc::peer_connection::RTCPeerConnectionState;

use common::{TestClient, watch_data_channel};

/// `POST /rtc/offer` のみに応答する最小 HTTP/1.1 サーバをループバックへ立てる。
///
/// 1 接続 1 リクエストのみを処理し、応答後に接続を閉じる（本テストの疎通確認には
/// keep-alive が不要なため、コア接続ループの複雑さを持ち込まない最小実装）。
async fn spawn_server(config: WebRtcConfig) -> SocketAddr {
    // 0.21 は UDP バインドアドレスが必須。ループバックのみで完結させる。
    let config = config.with_udp_bind_addrs(vec!["127.0.0.1:0".to_owned()]);
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
                        None => fandhe_backend_http::response::Response::empty(404),
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

/// ローカル 2 ピア（クライアント側は本テスト内で構築、サーバ側は
/// [`try_handle_rtc_offer`] が応答）で 1 対 1 の DataChannel を確立し、
/// メッセージ往復（エコー）が成立することを確認する（受け入れ条件: `/rtc/offer` で
/// SDP Offer/Answer・データチャネルが疎通すること）。
#[tokio::test]
async fn datachannel_roundtrip_over_local_loopback() {
    let addr = spawn_server(WebRtcConfig::new()).await;

    let client = TestClient::new().await;
    let data_channel = client
        .pc
        .create_data_channel("task-8-1", None)
        .await
        .unwrap();
    let (mut open_rx, mut msg_rx) = watch_data_channel(data_channel.clone());

    let offer_json = client.offer_json().await;

    // シグナリング: 本クレートの `try_handle_rtc_offer` へ実際に HTTP 経由で送る。
    let (status_line, answer_body) =
        tokio::time::timeout(Duration::from_secs(10), post_offer(addr, &offer_json))
            .await
            .unwrap();
    assert!(
        status_line.starts_with("HTTP/1.1 200 OK"),
        "unexpected status: {status_line}, body: {answer_body}"
    );

    client.apply_answer(&answer_body).await;

    tokio::time::timeout(Duration::from_secs(10), open_rx.recv())
        .await
        .expect("data channel did not open in time");

    data_channel.send_text("task-8-1 hello").await.unwrap();

    let echoed = tokio::time::timeout(Duration::from_secs(10), msg_rx.recv())
        .await
        .expect("no echo received in time")
        .expect("channel closed without message");
    assert_eq!(echoed, "task-8-1 hello");

    let _ = client.pc.close().await;
}

/// レビュー指摘（イシュー #26）の再発防止テスト: `max_peer_connections(1)` で 1 件目の
/// Answer を適用した直後に（ICE/DTLS の確立は待たずに）クライアント側
/// `RTCPeerConnection::close()` を呼び、サーバ側のクローズ検知（ハンドラの
/// `Closed`/`Failed`）でレジストリの枠が解放されて 2 件目の
/// シグナリングが `503` を返さず成功することを確認する（正常利用の蓄積のみで恒久的に
/// 503 化する回帰を防ぐ）。サーバ側 `RTCPeerConnectionState` が `Closed`/`Failed` へ
/// 遷移するのは ICE/DTLS 経由の非同期検知のため、猶予を長めに取りポーリングする
/// （`handler::tests::close_handler_releases_slot_when_state_becomes_closed` が
/// クローズ検知そのものをより直接的に検証する）。
///
/// ポーリング予算は回数ベースではなく `SLOT_RELEASE_BUDGET`（120 秒）の経過時間ベースで
/// 判定する（イシュー #276）。クライアント側 `close()` の通知がサーバへ届かず、サーバ側
/// 検知が ICE failed タイムアウト（webrtc-rs 既定で 30 秒級）へ倒れるケースがあり、
/// 負荷のある self-hosted runner ではさらに遅延しうる。1 周あたりのコスト（PeerConnection
/// 生成 + ICE gathering 完了待ち + HTTP シグナリング）も変動するため、回数固定では実質
/// 予算が読めない。環境要因による残余のばらつきは `.config/nextest.toml` の
/// `profile.ci` 側でこのテストに限定した retry が吸収し、実装側の回帰（枠解放漏れ）の
/// 決定的な検知は `handler::tests::close_handler_releases_slot_when_state_becomes_closed`
/// が担う。
#[tokio::test]
async fn peer_connection_slot_is_released_after_close_allowing_reuse() {
    /// 2 件目のシグナリングが 200 を返す（= 枠解放が検知される）までのポーリング予算。
    /// ICE failed タイムアウト検知（30 秒級）への倒れ込みを見込んだ余裕を含む。
    const SLOT_RELEASE_BUDGET: Duration = Duration::from_secs(120);

    let addr = spawn_server(WebRtcConfig::new().with_max_peer_connections(1)).await;

    // 1 件目: Answer を適用したら接続の確立を待たずにクローズする（サーバは相手消失を
    // ICE checking のまま `Failed`（約 29.5 秒）で検知して枠を解放する）。データチャネルを
    // 1 つ持たせないと SDP に m-line が含まれず `set_remote_description` が失敗するため、
    // `datachannel_roundtrip_over_local_loopback` と同様にチャネルを作成する。
    let first_client = TestClient::new().await;
    let _first_dc = first_client
        .pc
        .create_data_channel("slot-release-1", None)
        .await
        .unwrap();
    let offer_json = first_client.offer_json().await;

    let (status_line, answer_body) =
        tokio::time::timeout(Duration::from_secs(10), post_offer(addr, &offer_json))
            .await
            .unwrap();
    assert!(
        status_line.starts_with("HTTP/1.1 200 OK"),
        "1 件目のシグナリングに失敗: {status_line}, body: {answer_body}"
    );
    first_client.apply_answer(&answer_body).await;
    first_client.pc.close().await.unwrap();

    // クローズ通知（サーバ側 RTCPeerConnectionState の Closed/Failed への遷移）が
    // 届くまで、ICE/DTLS の非同期検知の猶予を持たせつつポーリングする。
    let poll_start = tokio::time::Instant::now();
    let (second_status, second_body) = loop {
        let second_client = TestClient::new().await;
        let _second_dc = second_client
            .pc
            .create_data_channel("slot-release-2", None)
            .await
            .unwrap();
        let offer_json = second_client.offer_json().await;

        let (status_line, body) =
            tokio::time::timeout(Duration::from_secs(10), post_offer(addr, &offer_json))
                .await
                .unwrap();
        let _ = second_client.pc.close().await;

        let succeeded = status_line.starts_with("HTTP/1.1 200 OK");
        let budget_exceeded = poll_start.elapsed() >= SLOT_RELEASE_BUDGET;
        if succeeded || budget_exceeded {
            break (status_line, body);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

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

// ---------------------------------------------------------------------------
// 送信バッファ上限超過時のバックプレッシャ（イシュー #875）
// ---------------------------------------------------------------------------

/// バックプレッシャ検証で送るメッセージ数。1 通 1 KiB で合計約 8 MiB となり、相手の SCTP
/// 受信窓・イベントキューに収まらない量にする（内部定数には依存せず「十分に大きい」だけを
/// 前提にする）。
const BP_MESSAGES: u64 = 8192;
/// 1 通のサイズ。先頭 8 バイトにビッグエンディアンの連番を入れ、残りは埋め草。
const BP_MESSAGE_SIZE: usize = 1024;
/// サーバ側 `echo_loop` を詰まらせるための送信バッファ上限。
const BP_SEND_LIMIT: usize = 64 * 1024;

/// ローカル 2 ピアでデータチャネルを確立し、`OnOpen` を消費した状態で返す。
///
/// `watch_data_channel` は別タスクで `poll` を回し続けて受信を止められないため使わない。
/// 本検証ではクライアントが `poll` を止めること自体が前提（相手が読まない状況を作る）なので、
/// 呼び出し側が `dc.poll()` を直接駆動する。
async fn establish(config: WebRtcConfig) -> (TestClient, Arc<dyn DataChannel>) {
    let addr = spawn_server(config).await;
    let client = TestClient::new().await;
    let dc = client
        .pc
        .create_data_channel("backpressure", None)
        .await
        .unwrap();
    let offer_json = client.offer_json().await;
    let (status_line, answer_body) =
        tokio::time::timeout(Duration::from_secs(10), post_offer(addr, &offer_json))
            .await
            .unwrap();
    assert!(
        status_line.starts_with("HTTP/1.1 200 OK"),
        "unexpected status: {status_line}, body: {answer_body}"
    );
    client.apply_answer(&answer_body).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = dc.poll().await {
            if matches!(event, DataChannelEvent::OnOpen) {
                return;
            }
        }
        panic!("data channel closed before open");
    })
    .await
    .expect("data channel did not open in time");
    (client, dc)
}

/// 連番付きメッセージ `0..BP_MESSAGES` を全て積む（クライアント側の送信上限は無制限で、
/// `send` は待たない）。
async fn send_numbered(dc: &Arc<dyn DataChannel>) {
    tokio::time::timeout(Duration::from_secs(30), async {
        for i in 0..BP_MESSAGES {
            let mut buf = BytesMut::zeroed(BP_MESSAGE_SIZE);
            buf[..8].copy_from_slice(&i.to_be_bytes());
            dc.send(buf).await.unwrap();
        }
    })
    .await
    .expect("sending queued messages timed out");
}

/// `outstanding_bytes` を 200 ms 間隔で採取し、直近 2 秒（10 サンプル）の変動が 16 KiB 以内に
/// なる（= 進行が止まった）まで待って、その最小値を返す。
async fn wait_outstanding_stable(dc: &Arc<dyn DataChannel>) -> usize {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut samples: Vec<usize> = Vec::new();
        loop {
            samples.push(dc.outstanding_bytes().await.unwrap());
            if samples.len() >= 10 {
                let recent = &samples[samples.len() - 10..];
                let (min, max) = (*recent.iter().min().unwrap(), *recent.iter().max().unwrap());
                if max - min <= 16 * 1024 {
                    return min;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("outstanding_bytes did not stabilise in time")
}

/// 上限 `BP_SEND_LIMIT` のサーバへ、クライアントが受信しないまま大量送信すると、サーバの
/// エコーが止まる一方で接続・データチャネルは切れず、受信を再開すると欠落・重複・順序入れ
/// 替えなく全件のエコーが戻ることを確認する（`echo_loop` のバックプレッシャ契約）。
///
/// 「エコーが止まった」ことは、クライアント側の `outstanding_bytes`（SCTP が未解放の送信
/// バイト数）が総送信量の半分以上で安定することで観測する。SCTP の窓サイズ等の内部定数には
/// 依存しない比率判定にしている。
#[tokio::test]
async fn send_buffer_limit_applies_backpressure_without_disconnecting() {
    let (client, dc) =
        establish(WebRtcConfig::new().with_data_channel_send_buffer_limit(BP_SEND_LIMIT)).await;

    send_numbered(&dc).await;

    let total = BP_MESSAGES as usize * BP_MESSAGE_SIZE;
    let stalled = wait_outstanding_stable(&dc).await;
    assert!(
        stalled >= 1024 * 1024,
        "エコーが詰まっていない: outstanding={stalled}, total={total}"
    );

    // 詰まったまま複数の再送タイマ周期をまたいでも、接続は生きたまま進行しない。
    for _ in 0..25 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(client.state(), RTCPeerConnectionState::Connected);
        assert_eq!(dc.ready_state().await.unwrap(), RTCDataChannelState::Open);
        let now = dc.outstanding_bytes().await.unwrap();
        assert!(
            now + 16 * 1024 >= stalled,
            "詰まっている間に outstanding が減った: {now} < {stalled}"
        );
    }

    // 受信を再開すると、全件が連番どおりに戻る。
    tokio::time::timeout(Duration::from_secs(90), async {
        let mut expected = 0u64;
        while expected < BP_MESSAGES {
            match dc.poll().await {
                Some(DataChannelEvent::OnMessage(msg)) => {
                    assert_eq!(msg.data.len(), BP_MESSAGE_SIZE);
                    let seq = u64::from_be_bytes(msg.data[..8].try_into().unwrap());
                    assert_eq!(seq, expected, "欠落・重複・順序入れ替えを検出");
                    expected += 1;
                }
                Some(DataChannelEvent::OnClose) | None => {
                    panic!("data channel closed after {expected} echoes")
                }
                Some(_) => {}
            }
        }
    })
    .await
    .expect("echo did not resume in time");

    tokio::time::timeout(Duration::from_secs(10), async {
        while dc.outstanding_bytes().await.unwrap() > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("outstanding_bytes did not drain after resume");
    assert_eq!(client.state(), RTCPeerConnectionState::Connected);

    let _ = client.pc.close().await;
}

/// 対照実験: 送信バッファ上限を 0（無制限）にすると、同じ量をクライアントが受信しないまま
/// 送っても、サーバが全件を読み切るためクライアントの `outstanding_bytes` が 0 まで減る。
/// 主テストの詰まりが受信側の都合ではなく送信バッファ上限によるものだと示す。
#[tokio::test]
async fn unbounded_send_buffer_lets_client_outstanding_drain_without_reading() {
    let (client, dc) = establish(WebRtcConfig::new().with_data_channel_send_buffer_limit(0)).await;

    send_numbered(&dc).await;

    tokio::time::timeout(Duration::from_secs(30), async {
        while dc.outstanding_bytes().await.unwrap() > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("outstanding_bytes did not drain with unbounded server buffer");

    let _ = client.pc.close().await;
}
