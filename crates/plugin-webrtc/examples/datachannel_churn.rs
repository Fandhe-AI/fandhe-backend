//! イシュー #887: 上限超過で拒否されるデータチャネルを大量に開閉したときの残留とメモリ使用量の計測。
//!
//! `WebRtcConfig::with_max_data_channels_per_connection`（#877）は「同時に開いている本数」
//! を数えるだけで、webrtc 0.21 / rtc 内部（SCTP ストリーム・統計集計器・イベント送信表）の
//! 後始末は検証していなかった。1 接続で「開く → 拒否される → 閉じる」を繰り返す攻撃者に対し
//! プロセスの RSS が有界に収まるかを、2 つのプロセス役で再現する。
//!
//! - `server`: ループバックで `POST /rtc/offer` だけを `try_handle_rtc_offer` に渡す最小サーバ。
//!   起動したら `PID=<pid> ADDR=<addr>` を 1 行出力する。標準入力が EOF になったら終了する。
//! - `client`: server へ接続して 1 本目のチャネル（受理される枠）を維持しつつ、上限を超える
//!   チャネルの開閉を `DURATION_SEC` 秒繰り返し、区間ごとに CSV を標準出力する。
//!
//! server 側の `PeerConnection` はレジストリ（`pub(crate)`）の中にあり外から見えないため、
//! 内部状態の代理指標として client 側の `get_stats()` に含まれる DataChannel 統計エントリ数を
//! 出す（client も同じ rtc コードで動く）。RSS の採取と要約は `benches/webrtc-dc-churn.sh` が担う。
//!
//! ```bash
//! cargo run --release -p fandhe-backend-plugin-webrtc --example datachannel_churn -- server
//! cargo run --release -p fandhe-backend-plugin-webrtc --example datachannel_churn -- client 127.0.0.1:PORT
//! ```
//!
//! 環境変数（任意。0・解析不能な値は既定値に戻す。`CONCURRENCY` のみ 0 を許可し churn なしの対照になる）:
//! - `FANDHE_DC_CHURN_MAX_CHANNELS`（server、既定 1）: 1 接続あたりの同時データチャネル数上限
//! - `DURATION_SEC`（client、既定 600）、`CONCURRENCY`（既定 4）、`LABEL_BYTES`（既定 8、最大 60000）
//! - `REPORT_INTERVAL_SEC`（既定 10）、`CLOSE_PC_AT_END`（既定 1）、`POST_CLOSE_IDLE_SEC`（既定 60）
//!
//! 本 example は 127.0.0.1 にだけ bind し STUN/TURN を設定しない。lib の依存グラフには影響しない
//! （`.claude/rules/pay-for-what-you-use.md`）。計測結果は `benches/reports/issue887-webrtc-dc-churn.md`。

use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use fandhe_backend_http::buffer::RecvBuffer;
use fandhe_backend_http::connection::read_request;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_webrtc::{WebRtcConfig, try_handle_rtc_offer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceGatheringState, RTCPeerConnectionState, RTCSessionDescription,
    Registry, StatsSelector, register_default_interceptors,
};

/// 1 サイクル（作成から `OnClose` まで）の待ち上限。超過は `timeouts` に数える。
const CYCLE_TIMEOUT: Duration = Duration::from_secs(10);

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("server") => run_server().await,
        Some("client") => {
            let addr = args.get(2).expect("usage: client <host:port>");
            run_client(addr).await;
        }
        _ => {
            eprintln!("usage: datachannel_churn server | client <host:port>");
            std::process::exit(2);
        }
    }
}

// ---------------------------------------------------------------------------
// server 役
// ---------------------------------------------------------------------------

async fn run_server() {
    let max = env_u64("FANDHE_DC_CHURN_MAX_CHANNELS", 1) as usize;
    let config = WebRtcConfig::new()
        .with_udp_bind_addrs(vec!["127.0.0.1:0".to_owned()])
        .with_max_data_channels_per_connection(max);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind signaling listener");
    let addr = listener.local_addr().expect("local addr");
    println!("PID={} ADDR={addr}", std::process::id());

    // 標準入力の EOF（呼び出し側スクリプトの終了）で止まる。
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut sink);
        let _ = stop_tx.send(());
    });

    let accept = async {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                continue;
            };
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
                        None => Response::empty(404),
                    };
                let _ = stream.write_all(&response.serialize(false)).await;
            });
        }
    };
    tokio::select! {
        () = accept => {}
        _ = stop_rx => {}
    }
}

// ---------------------------------------------------------------------------
// client 役
// ---------------------------------------------------------------------------

struct ClientHandler {
    state_tx: watch::Sender<RTCPeerConnectionState>,
    gather_tx: watch::Sender<bool>,
}

#[async_trait]
impl PeerConnectionEventHandler for ClientHandler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gather_tx.send(true);
        }
    }
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.state_tx.send(state);
    }
}

async fn post_offer(addr: &str, offer_json: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect server");
    let raw = format!(
        "POST /rtc/offer HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        offer_json.len(),
        offer_json
    );
    stream.write_all(raw.as_bytes()).await.expect("send offer");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read answer");
    let text = String::from_utf8(buf).expect("utf8 response");
    let (head, body) = text.split_once("\r\n\r\n").expect("response framing");
    assert!(head.starts_with("HTTP/1.1 200"), "signaling failed: {head}");
    body.to_owned()
}

/// `len` バイトの ASCII ラベル。先頭へ連番を入れ、同名ラベルの再利用を避ける。
fn make_label(len: usize, n: u64) -> String {
    let mut s = n.to_string();
    while s.len() < len {
        s.push('x');
    }
    s
}

async fn echo_first(first: Arc<dyn DataChannel>) -> bool {
    if first.send_text("ping").await.is_err() {
        return false;
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = first.poll().await {
            match ev {
                DataChannelEvent::OnMessage(m) => return m.data[..] == *b"ping",
                DataChannelEvent::OnClose => return false,
                _ => {}
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

async fn run_client(addr: &str) {
    let duration = Duration::from_secs(env_u64("DURATION_SEC", 600));
    let concurrency = env::var("CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(4);
    let label_bytes = (env_u64("LABEL_BYTES", 8) as usize).min(60_000);
    let interval = Duration::from_secs(env_u64("REPORT_INTERVAL_SEC", 10));
    // 0 は「接続を維持する」を意味する有効値のため env_u64（0 を既定値へ戻す）を使わない。
    let close_at_end = env::var("CLOSE_PC_AT_END")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1)
        != 0;
    let post_close_idle = Duration::from_secs(env_u64("POST_CLOSE_IDLE_SEC", 60));

    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs().expect("codecs");
    let registry =
        register_default_interceptors(Registry::new(), &mut media_engine).expect("interceptors");
    let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
    let (gather_tx, mut gather_rx) = watch::channel(false);
    let pc: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::new()
            .with_configuration(RTCConfigurationBuilder::new().build())
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_handler(Arc::new(ClientHandler {
                state_tx,
                gather_tx,
            }))
            .with_udp_addrs(vec!["127.0.0.1:0"])
            .build()
            .await
            .expect("build pc"),
    );

    // 1 本目（受理される枠）。以後ずっと維持してエコーで生存確認する。
    let first = pc
        .create_data_channel("first", None)
        .await
        .expect("create first channel");
    let offer = pc.create_offer(None).await.expect("create offer");
    pc.set_local_description(offer).await.expect("set local");
    tokio::time::timeout(Duration::from_secs(10), gather_rx.wait_for(|d| *d))
        .await
        .expect("ICE gathering timeout")
        .expect("gather sender dropped");
    let local = pc.local_description().await.expect("local description");
    let answer_body = post_offer(addr, &serde_json::to_string(&local).expect("json")).await;
    let answer: RTCSessionDescription = serde_json::from_str(&answer_body).expect("answer json");
    pc.set_remote_description(answer).await.expect("set remote");
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = first.poll().await {
            if matches!(ev, DataChannelEvent::OnOpen) {
                return;
            }
        }
        panic!("first channel closed before open");
    })
    .await
    .expect("first channel did not open");

    let cycles = Arc::new(AtomicU64::new(0));
    let create_errors = Arc::new(AtomicU64::new(0));
    let timeouts = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let deadline = started + duration;

    println!("elapsed_s,cycles,create_errors,timeouts,first_echo_ok,state,client_dc_stats");
    let mut workers = Vec::new();
    for _ in 0..concurrency {
        let pc = pc.clone();
        let (cycles, create_errors, timeouts) =
            (cycles.clone(), create_errors.clone(), timeouts.clone());
        workers.push(tokio::spawn(async move {
            while Instant::now() < deadline {
                let n = cycles.fetch_add(1, Ordering::Relaxed);
                let label = make_label(label_bytes, n);
                let Ok(dc) = pc.create_data_channel(&label, None).await else {
                    create_errors.fetch_add(1, Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let closed = tokio::time::timeout(CYCLE_TIMEOUT, async {
                    while let Some(ev) = dc.poll().await {
                        if matches!(ev, DataChannelEvent::OnClose) {
                            return;
                        }
                    }
                })
                .await;
                if closed.is_err() {
                    timeouts.fetch_add(1, Ordering::Relaxed);
                    let _ = dc.close().await;
                }
            }
        }));
    }

    let report = |echo: bool, stats: usize| {
        println!(
            "{},{},{},{},{},{:?},{}",
            started.elapsed().as_secs(),
            cycles.load(Ordering::Relaxed),
            create_errors.load(Ordering::Relaxed),
            timeouts.load(Ordering::Relaxed),
            u8::from(echo),
            *state_rx.borrow(),
            stats
        );
    };
    let sample = async || {
        pc.get_stats(Instant::now(), StatsSelector::None)
            .await
            .data_channels()
            .count()
    };

    while Instant::now() < deadline {
        tokio::time::sleep(interval.min(deadline.saturating_duration_since(Instant::now()))).await;
        let echo = echo_first(first.clone()).await;
        report(echo, sample().await);
    }
    for w in workers {
        let _ = w.await;
    }
    let echo = echo_first(first.clone()).await;
    report(echo, sample().await);

    if close_at_end {
        let _ = pc.close().await;
        println!("PC_CLOSED");
    } else {
        println!("PC_KEPT");
    }
    // 呼び出し側が close 後（または維持中）の server RSS を採れるよう待つ。
    tokio::time::sleep(post_close_idle).await;
}
