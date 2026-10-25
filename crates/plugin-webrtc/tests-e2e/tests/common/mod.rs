//! 統合テスト共通: クライアント側 `PeerConnection`（webrtc 0.21 のハンドラ trait 版）。
//!
//! 0.21 には `connection_state()` / `gathering_complete_promise()` / `on_open` 等の
//! コールバック登録 API が無いため、ハンドラが `watch` へ状態を流し、データチャネルは
//! `poll()` ループで読む。ループバックのみで完結させるため `127.0.0.1:0` にバインドする。
//!
//! `../../tests/common/mod.rs`（plugin-webrtc 本体の統合テスト用）と同一内容（本クレートは
//! root workspace から独立した standalone crate のため共有できない。変更時は両方揃えること）。
#![allow(dead_code, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceGatheringState, RTCPeerConnectionState, RTCSessionDescription,
    Registry, register_default_interceptors,
};

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

pub struct TestClient {
    pub pc: Arc<dyn PeerConnection>,
    state_rx: watch::Receiver<RTCPeerConnectionState>,
    gather_rx: watch::Receiver<bool>,
}

impl TestClient {
    pub async fn new() -> Self {
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs().unwrap();
        let registry = register_default_interceptors(Registry::new(), &mut media_engine).unwrap();
        let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
        let (gather_tx, gather_rx) = watch::channel(false);
        let pc = PeerConnectionBuilder::new()
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
            .unwrap();
        Self {
            pc: Arc::new(pc),
            state_rx,
            gather_rx,
        }
    }

    /// 現在の接続状態（0.21 には getter が無いためハンドラ経由の最新値）。
    /// 注意: ローカル `close()` では `Closed` が届く保証がない。
    pub fn state(&self) -> RTCPeerConnectionState {
        *self.state_rx.borrow()
    }

    /// Offer を作って適用し、ICE gathering 完了後の local description を JSON で返す。
    pub async fn offer_json(&self) -> String {
        let offer = self.pc.create_offer(None).await.unwrap();
        self.pc.set_local_description(offer).await.unwrap();
        let mut gather_rx = self.gather_rx.clone();
        tokio::time::timeout(Duration::from_secs(10), gather_rx.wait_for(|done| *done))
            .await
            .expect("ICE gathering が 10 秒以内に完了しなかった")
            .expect("gathering 通知の送信側が先に破棄された");
        let local_desc = self.pc.local_description().await.unwrap();
        serde_json::to_string(&local_desc).unwrap()
    }

    pub async fn apply_answer(&self, answer_body: &str) {
        let answer: RTCSessionDescription = serde_json::from_str(answer_body).unwrap();
        self.pc.set_remote_description(answer).await.unwrap();
    }
}

/// データチャネルの `poll()` を別タスクで回し、`OnOpen` と受信テキストを通知する。
pub fn watch_data_channel(
    dc: Arc<dyn DataChannel>,
) -> (mpsc::Receiver<()>, mpsc::Receiver<String>) {
    let (open_tx, open_rx) = mpsc::channel::<()>(1);
    let (msg_tx, msg_rx) = mpsc::channel::<String>(8);
    tokio::spawn(async move {
        while let Some(event) = dc.poll().await {
            match event {
                DataChannelEvent::OnOpen => {
                    let _ = open_tx.try_send(());
                }
                DataChannelEvent::OnMessage(msg) => {
                    let text = String::from_utf8(msg.data.to_vec()).unwrap_or_default();
                    let _ = msg_tx.send(text).await;
                }
                DataChannelEvent::OnClose => break,
                _ => {}
            }
        }
    });
    (open_rx, msg_rx)
}
