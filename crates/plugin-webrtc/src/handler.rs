//! パスインターセプト型のリクエスト/レスポンス完結型ハンドラ（[`try_handle_rtc_offer`]）。
//!
//! `crates/plugin-webrtc-proxy`（TASK-8.2-2）・PoC-5
//! （`docs/spec/03-poc/webrtc-plugin/core/src/plugins/webrtc.rs`）と同型のパターンを
//! 踏襲する: 対象パス以外は即 `None` を返し、無関係なリクエストへの性能影響をゼロに
//! する。新しい拡張点（`Middleware`/`UpgradeHandler`/`RequestGate`）は追加しない
//! （`crate` ルート doc の「設計上の位置づけ」を参照）。
//!
//! `crates/plugin-webrtc-proxy::handler` が独自の中間 `Response` 型を持つのは配線
//! 確立前（TASK-8.2-2 時点）の歴史的経緯であり、本クレートは
//! [`fandhe_backend_http::response::Response`] を直接組み立てて返す（変換層を省く。
//! `docs/design/plugin-boundary.md` 4.3 節）。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::BytesMut;
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use tokio::sync::watch;
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceGatheringState, RTCPeerConnectionState, RTCSessionDescription,
    Registry, register_default_interceptors,
};

use crate::config::{WebRtcConfig, spawn_close};

/// 本プラグインがパスインターセプトの対象とするリクエストパス。
pub const OFFER_PATH: &str = "/rtc/offer";

/// 内部エラー詳細（SDP 本文・webrtc-rs の生エラー等）を含まない定型 JSON エラー応答を
/// 組み立てる（.claude/rules/security.md のエラー情報漏えい対策。
/// `crates/plugin-webrtc-proxy::handler::Response::error` と同一方針）。
fn error_response(status: u16, body: &'static [u8]) -> Response {
    Response::new(status, body.to_vec()).with_content_type("application/json")
}

/// `POST /rtc/offer` をパスインターセプトし、SDP Offer（JSON:
/// `{"type":"offer","sdp":"..."}`）を受け取って `PeerConnection` を生成、
/// データチャネル到着を待ち受けるエコーハンドラを登録したうえで、非トリクル ICE
/// による SDP Answer を JSON で返す。
///
/// - メソッド・パスが対象外なら `None` を返す（呼び出し元は次のハンドラへ
///   フォールスルーする契約。`crates/plugin-webrtc-proxy` と同型）
/// - `Content-Length` が欠如・`body` の実長と不一致なら `400`
/// - `config.max_offer_bytes()` 超過は `413`（リソース枯渇対策、
///   .claude/rules/security.md）
/// - `config.udp_bind_addrs()` が空、または `SocketAddr` として解釈できない要素（ホスト名を
///   含む）が 1 つでもあれば、枠を予約せず `PeerConnection` も生成せずに `500`
///   （`peer_connection_init_failed`）で拒否する（webrtc 0.21 は UDP バインドアドレスが
///   必須。既定値へ黙って戻さないフェイルクローズ。ホスト名は webrtc 側でブロッキングの
///   名前解決がドライバ上で走るため受け付けない）
/// - `config.max_peer_connections()` に達している場合は新規 `PeerConnection` を
///   生成せず `503` で拒否する（フェイルクローズ）。上限判定と予約枠の登録は
///   `WebRtcConfig::reserve_slot` が単一ロック区間で行うため TOCTOU
///   （time-of-check to time-of-use）は生じない。接続クローズ・失敗時は
///   `SignalingHandler` が `release_slot` でレジストリから枠を除去する（Active なら
///   同時に close）ため、正常利用の蓄積のみでレジストリが単調増加し続けることはない
/// - 枠を予約してから `activate_slot` までの後始末（枠の除去と、生成済みなら `close()`）は
///   `SignalingGuard` が `Drop` で保証する。タイムアウト（504）だけでなく、呼び出し元の
///   Future が途中で drop された場合（`JoinSet::shutdown` / `JoinHandle::abort` 等）も同じ
/// - JSON/SDP のパース失敗・シグナリング内部エラーは `400`/`500`（内部情報を含まない
///   固定 JSON body）
/// - シグナリング全体（`set_remote_description` から ICE 候補収集完了まで）が
///   `config.signaling_timeout()` を超えたら `504`
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
/// use fandhe_backend_plugin_webrtc::{WebRtcConfig, try_handle_rtc_offer};
///
/// let buf = b"GET /health HTTP/1.1\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// let config = WebRtcConfig::new();
///
/// // 対象外パスなので None（無関係パスへの性能影響ゼロ）。
/// let runtime = tokio::runtime::Runtime::new().unwrap();
/// let result = runtime.block_on(try_handle_rtc_offer(&head, b"", &config));
/// assert!(result.is_none());
/// ```
pub async fn try_handle_rtc_offer(
    head: &RequestHead,
    body: &[u8],
    config: &WebRtcConfig,
) -> Option<Response> {
    if head.method() != "POST" || head.target() != OFFER_PATH {
        return None;
    }

    let declared_len = match head
        .header("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(len) => len,
        None => {
            return Some(error_response(
                400,
                br#"{"error":"missing_content_length"}"#,
            ));
        }
    };
    if declared_len != body.len() {
        return Some(error_response(
            400,
            br#"{"error":"content_length_mismatch"}"#,
        ));
    }
    if body.len() > config.max_offer_bytes() {
        return Some(error_response(413, br#"{"error":"offer_too_large"}"#));
    }

    // webrtc 0.21 は UDP バインドアドレスが必須（空だと build が `no udp_sockets or
    // tcp_listeners available` で失敗する）。設定の誤りを接続のたびに build 失敗として
    // 繰り返さないよう、枠の予約と MediaEngine 等の初期化の前に拒否する。既定値
    // （`0.0.0.0:0`）へ黙って戻すと運用者が意図しない interface で待ち受けるため、
    // フェイルクローズで 500 にする（.claude/rules/security.md）。`SocketAddr` として解釈
    // できない要素（ホスト名）も同様に拒否する。文字列のまま webrtc へ渡すと、ブロッキングの
    // DNS 解決がドライバ上で走る。
    if config.parsed_udp_bind_addrs().is_none() {
        return Some(error_response(
            500,
            br#"{"error":"peer_connection_init_failed"}"#,
        ));
    }

    // 新規 PeerConnection 生成の前に同時接続数上限を確認し、予約枠を登録する
    // （フェイルクローズ。判定と登録を単一ロック区間で行う reserve_slot により
    // TOCTOU を防ぐ。上限判定を signaling_timeout の外・パース処理の前に置くことで、
    // 上限超過時は webrtc-rs の重い初期化処理（MediaEngine・interceptor 登録）自体を
    // 走らせない）。
    let slot_id = match config.reserve_slot() {
        Some(id) => id,
        None => {
            return Some(error_response(
                503,
                br#"{"error":"peer_connection_limit_reached"}"#,
            ));
        }
    };

    // 予約した枠と、シグナリング中に生成した pc の後始末を `Drop` で保証するガード。
    // webrtc 0.21 は close せずに drop するとドライバと UDP ソケットが残るため（PR #138
    // レビュー指摘の 0.21 版）、タイムアウトによる complete_signaling の Future の drop だけ
    // でなく、本関数自体の Future が drop される経路（`JoinSet::shutdown` / `abort` など、
    // 呼び出し元のタスクのキャンセル）でも枠の除去と close が実行される必要がある。
    let mut guard = SignalingGuard::new(config, slot_id);

    Some(
        within_signaling_deadline(
            config.signaling_timeout(),
            complete_signaling(body, config, &mut guard),
        )
        .await,
    )
    // 本関数を抜けるとき、有効なままのガードの `Drop` が枠の除去と、生成済み pc の close
    // （切り離したタスク）を行う。close はネットワーク I/O を伴いうるため、応答の返却を
    // その完了待ちでブロックしない（PR #138 レビュー指摘: タイムアウト時にスロットは解放
    // されても peer が残り max_peer_connections の実効性が弱まる問題への対策）。
}

/// シグナリング全体に期限を課す。期限を超えたら `504` を返し、`signaling` の Future はここで
/// drop される（以降のコードは実行されない）。後始末は Future の外で生きている
/// [`SignalingGuard`] が担う（呼び出し元の関数を抜けるときの `Drop`）。
async fn within_signaling_deadline<F>(timeout: std::time::Duration, signaling: F) -> Response
where
    F: std::future::Future<Output = Response>,
{
    match tokio::time::timeout(timeout, signaling).await {
        Ok(response) => response,
        Err(_elapsed) => error_response(504, br#"{"error":"signaling_timeout"}"#),
    }
}

/// 枠の予約から `activate_slot` までの間、枠と生成済み `PeerConnection` の後始末を所有する
/// ガード（枠の除去と `close()` の対。`crate::config` モジュール doc 参照）。
///
/// [`try_handle_rtc_offer`] が `reserve_slot` の直後に作り、[`complete_signaling`] が次の
/// 3 つで使う。
///
/// - [`SignalingGuard::publish`]: pc を生成した直後に公開する（以降 `Drop` が close できる）
/// - [`SignalingGuard::finish`]: 失敗経路。pc を close してから枠を解放し、ガードを解除する
/// - [`SignalingGuard::activate`]: 成功経路。ガードを解除して `activate_slot` へ引き渡す
///
/// ガードが有効なまま drop された場合（タイムアウト・呼び出し元 Future の drop・panic の
/// unwind）、`Drop` が `release_slot` で枠を除去し、公開済みの pc を [`spawn_close`] で
/// 切り離して close する。`Drop` は同期関数なので `.await` できないが、`spawn_close` が
/// 実行を tokio ランタイムへ引き渡す。ランタイム外で drop された場合は実行先が無いので
/// close は行われない（枠の除去だけ行い panic しない。ランタイム無しに pc は生成できないため
/// 通常は到達しない）。`release_slot` と `close()` は冪等なので、解除前の失敗経路と
/// 二重に走っても安全。
///
/// # 残余リスク
///
/// `PeerConnectionBuilder::build().await` の途中でキャンセルされると、webrtc 0.21.1 は
/// ドライバを先に spawn してから初期化の完了を待つため、公開前の pc のドライバが 1 件残りうる
/// （`peer_connection/mod.rs` の Drop が closing を立てるのは dedicated reactor のときだけ）。
/// 公開前の pc には本ガードから触れないため防げない。上流（webrtc-rs）側の課題として扱う。
struct SignalingGuard {
    config: WebRtcConfig,
    slot_id: u64,
    /// 生成済みの pc（公開後〜解除まで）。`Drop` から取り出して close するための置き場で、
    /// 失敗経路は取り出さずに複製して close する（close の最中にキャンセルされても残る）。
    pc_cell: Arc<Mutex<Option<Arc<dyn PeerConnection>>>>,
    /// `true` の間は `Drop` が後始末を行う。`finish` / `activate` が解除する。
    armed: bool,
}

impl SignalingGuard {
    fn new(config: &WebRtcConfig, slot_id: u64) -> Self {
        Self {
            config: config.clone(),
            slot_id,
            pc_cell: Arc::new(Mutex::new(None)),
            armed: true,
        }
    }

    /// 生成した pc を公開する。`build().await` の完了から本呼び出しまでに await 点は無い。
    fn publish(&self, pc: Arc<dyn PeerConnection>) {
        *self.pc_cell.lock().unwrap_or_else(|e| e.into_inner()) = Some(pc);
    }

    /// 失敗経路の後始末。公開済みの pc を `close()`（結果は無視してよい。close 失敗は
    /// 「枠の解放」を妨げない）してから枠を解放し、ガードを解除する。
    ///
    /// `close().await` の途中でキャンセルされてもガードは有効なままで pc も残っているので、
    /// `Drop` が枠の除去と再 close を行う。解除は枠の解放と同じ同期区間（await 無し）で行う。
    async fn finish(&mut self) {
        let pc = self
            .pc_cell
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(pc) = pc {
            let _ = pc.close().await;
        }
        self.config.release_slot(self.slot_id);
        self.armed = false;
    }

    /// 成功経路。ガードを解除して pc を `activate_slot` へ渡す（拒否時は `activate_slot`
    /// 自身が枠の除去と close を行う）。解除から `activate_slot` までに await 点は無く、
    /// キャンセルされない。pc が未公開なら何もせず `false`（ガードは有効なまま `Drop` が枠を除去）。
    fn activate(&mut self) -> bool {
        let pc = self
            .pc_cell
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        match pc {
            Some(pc) => {
                self.armed = false;
                self.config.activate_slot(self.slot_id, pc)
            }
            None => false,
        }
    }
}

impl Drop for SignalingGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.config.release_slot(self.slot_id);
        let pc = self
            .pc_cell
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(pc) = pc {
            spawn_close(pc);
        }
    }
}

/// SDP Offer の JSON パースから Answer 生成までのシグナリング本体。
///
/// [`try_handle_rtc_offer`] から `tokio::time::timeout` で包んで呼ばれる
/// （シグナリング全体のタイムアウト適用範囲を本関数の呼び出し全体に一致させるため、
/// 早期 return を含む全経路がこの関数内で完結する設計）。枠は呼び出し元が
/// [`WebRtcConfig::reserve_slot`] で予約済みで、`guard` が所有する。**すべての失敗経路で
/// [`SignalingGuard::finish`] を呼び、予約枠のリークと `pc` の close 漏れを同時に防ぐ契約**と
/// する。成功経路は [`SignalingGuard::activate`] が枠を `Active` にするか、拒否
/// （終端 drain・枠消失）して `pc` を close する。
///
/// `tokio::time::timeout` や呼び出し元のタスクのキャンセルが本関数の Future を drop した
/// 場合、ここから先のコードは一切実行されない。その場合の後始末は `guard` の `Drop` が行う
/// （`guard` は本関数の外、呼び出し元が所有しているため）。
async fn complete_signaling(
    body: &[u8],
    config: &WebRtcConfig,
    guard: &mut SignalingGuard,
) -> Response {
    let offer: RTCSessionDescription = match serde_json::from_slice(body) {
        Ok(offer) => offer,
        Err(_) => {
            guard.finish().await;
            return error_response(400, br#"{"error":"invalid_offer_json"}"#);
        }
    };

    let (pc, mut gather_rx) = match build_peer_connection(config, guard.slot_id).await {
        Ok(built) => built,
        Err(_) => {
            guard.finish().await;
            return error_response(500, br#"{"error":"peer_connection_init_failed"}"#);
        }
    };

    // 以降の await 点でキャンセルされる可能性があるため、pc をガードへ公開する
    // （`SignalingGuard` の doc を参照）。
    guard.publish(Arc::clone(&pc));

    if pc.set_remote_description(offer).await.is_err() {
        guard.finish().await;
        return error_response(400, br#"{"error":"invalid_remote_description"}"#);
    }

    let answer = match pc.create_answer(None).await {
        Ok(answer) => answer,
        Err(_) => {
            guard.finish().await;
            return error_response(500, br#"{"error":"create_answer_failed"}"#);
        }
    };

    // 非トリクル ICE: 候補収集が終わるまで応答を保留し、1 往復のシグナリングで
    // 完結させる（PoC-5 の簡易実装を踏襲。トリクル ICE は REQ-8 でスコープ外）。
    if pc.set_local_description(answer).await.is_err() {
        guard.finish().await;
        return error_response(500, br#"{"error":"set_local_description_failed"}"#);
    }
    // 0.21 には gathering_complete_promise が無い。ハンドラが build 時に渡した watch へ
    // `Complete` を latch するので、set_local_description の後で待っても取りこぼさない。
    let _ = gather_rx.wait_for(|done| *done).await;

    let local_desc = match pc.local_description().await {
        Some(desc) => desc,
        None => {
            guard.finish().await;
            return error_response(500, br#"{"error":"local_description_missing"}"#);
        }
    };

    let answer_bytes = match serde_json::to_vec(&local_desc) {
        Ok(bytes) => bytes,
        Err(_) => {
            guard.finish().await;
            return error_response(500, br#"{"error":"serialization_failed"}"#);
        }
    };

    // シグナリング成功。ここから `activate` までに await 点は無く、キャンセルされない。
    // 予約枠をアクティブな接続へ遷移させ、以降このプロセス内で `PeerConnection` を
    // 保持し続ける。クローズ・失敗検知時の枠除去は `SignalingHandler` が担う
    // （レジストリの単調増加を防ぐ）。
    //
    // イシュー #498: 最終 graceful shutdown（`WebRtcConfig::begin_terminal_drain`）が
    // 本呼び出しと競合した場合、または対象 slot がタイムアウト等で既に除去済みの
    // missing-slot レースの場合、`activate_slot` はレジストリ登録を拒否（`false`）する。
    // 拒否された pc は `activate_slot` 自身が切り離したタスクで close する
    // （フェイルクローズ、`.claude/rules/security.md`。枠の除去と close は同じ同期区間の
    // 対で、200 応答の返却もブロックしない）。応答は拒否時も 200 のまま返す
    // （既存のワイヤ契約。クライアントはデータチャネルが開かないことで検知する）。
    let _registered = guard.activate();

    Response::new(200, answer_bytes).with_content_type("application/json")
}

/// 0.21 のイベントハンドラ（build 時に 1 つだけ固定で渡す）。0.17 の
/// `on_peer_connection_state_change` / `on_data_channel` / `gathering_complete_promise`
/// 相当を 1 つの trait 実装へ集約する（0.22 以降へ再移行するときの変更点をここに閉じ込める）。
///
/// - ICE gathering 完了を `watch` へ latch（非トリクル ICE の応答保留用）
/// - `Closed`/`Failed` で `WebRtcConfig::release_slot` により枠を除去する。`Active` だった
///   接続は `release_slot` が**明示 close** する（0.21 はハンドルを close せず drop すると
///   driver・UDP ソケットが残るため）
/// - 受信データチャネルへエコーループを spawn（ハンドラはドライバ上で走るので await で塞がない）
///
/// # 参照サイクル
///
/// 本構造体は `WebRtcConfig`（= レジストリ）のクローンを持ち、レジストリの `Active(pc)` は
/// `pc`（→ ドライバ → 本ハンドラ）を持つため、`Active` の間は循環参照になる。サイクルは
/// 枠の除去（`Closed`/`Failed`・drain・`activate_slot` の拒否）で必ず切れ、同時に
/// `close()` でドライバが止まる（`crate::config` モジュール doc）。
struct SignalingHandler {
    gather_tx: watch::Sender<bool>,
    config: WebRtcConfig,
    slot_id: u64,
}

#[async_trait]
impl PeerConnectionEventHandler for SignalingHandler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gather_tx.send(true);
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        // `Disconnected` は ICE 再接続で回復しうるため対象外。0.21 では `pc.close()` の
        // ローカル呼び出しで `Closed` が届く保証がないので、ローカルで close する経路
        // （`SignalingGuard` / drain）は呼び出し側が枠を解放する。ここは相手起因の
        // `Closed`/`Failed` 専用で、`Active` なら `release_slot` が close まで行う
        // （`Reserved` の間は pc を `complete_signaling` が保持しているので枠の除去だけ。
        // その後の `activate_slot` が枠なしで拒否して pc を close する）。
        if matches!(
            state,
            RTCPeerConnectionState::Closed | RTCPeerConnectionState::Failed
        ) {
            self.config.release_slot(self.slot_id);
        }
    }

    async fn on_data_channel(&self, data_channel: Arc<dyn DataChannel>) {
        tokio::spawn(echo_loop(data_channel));
    }
}

/// 相手から到着したデータチャネルの受信メッセージをそのまま送り返す。
async fn echo_loop(dc: Arc<dyn DataChannel>) {
    while let Some(event) = dc.poll().await {
        match event {
            DataChannelEvent::OnMessage(msg) => {
                let _ = dc.send(BytesMut::from(&msg.data[..])).await;
            }
            DataChannelEvent::OnClose => break,
            _ => {}
        }
    }
}

/// メディアコーデック・インターセプタを登録した `PeerConnection` を 1 つ生成する。
///
/// 既定の `RTCConfiguration` は ICE サーバ（STUN/TURN）を含まないため、ホスト候補
/// （ローカルアドレス）のみで ICE 疎通を試みる構成になる（PoC-5 の「外部ネットワーク
/// 不要」制約を踏襲。クライアント SDP 由来のアドレスへ UDP 送信が生じる特性は `crate`
/// ルート doc の SSRF 類似観点を参照）。0.21 では UDP バインドアドレスが必須で、
/// `WebRtcConfig::udp_bind_addrs`（既定 `["0.0.0.0:0"]`）を `SocketAddr` に解釈して渡す
/// （文字列のまま渡すとホスト名のブロッキング DNS 解決がドライバ上で走る）。解釈できない
/// 要素があれば生成せず `Err`（通常は `try_handle_rtc_offer` が枠の予約前に拒否済み）。
///
/// 生成に失敗した場合は接続が存在しないので、呼び出し元は枠の解放のみ行う。
async fn build_peer_connection(
    config: &WebRtcConfig,
    slot_id: u64,
) -> webrtc::error::Result<(Arc<dyn PeerConnection>, watch::Receiver<bool>)> {
    let udp_addrs = config.parsed_udp_bind_addrs().ok_or_else(|| {
        webrtc::error::Error::Other(
            "udp_bind_addrs が空、または SocketAddr として解釈できない".to_owned(),
        )
    })?;
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs()?;
    let registry = register_default_interceptors(Registry::new(), &mut media_engine)?;

    let (gather_tx, gather_rx) = watch::channel(false);
    let pc = PeerConnectionBuilder::new()
        .with_configuration(RTCConfigurationBuilder::new().build())
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .with_handler(Arc::new(SignalingHandler {
            gather_tx,
            config: config.clone(),
            slot_id,
        }))
        .with_udp_addrs(udp_addrs)
        .build()
        .await?;
    Ok((Arc::new(pc), gather_rx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::wait_until_closed;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    use std::time::Duration;

    fn head_from(buf: &[u8]) -> RequestHead {
        match parse_request_head(buf).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("expected Complete"),
        }
    }

    #[tokio::test]
    async fn path_mismatch_returns_none() {
        let head = head_from(b"POST /other HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
        let config = WebRtcConfig::new();
        assert!(try_handle_rtc_offer(&head, b"", &config).await.is_none());
    }

    #[tokio::test]
    async fn method_mismatch_returns_none() {
        let head = head_from(b"GET /rtc/offer HTTP/1.1\r\n\r\n");
        let config = WebRtcConfig::new();
        assert!(try_handle_rtc_offer(&head, b"", &config).await.is_none());
    }

    #[tokio::test]
    async fn missing_content_length_is_rejected() {
        let head = head_from(b"POST /rtc/offer HTTP/1.1\r\n\r\n");
        let config = WebRtcConfig::new();
        let response = try_handle_rtc_offer(&head, b"", &config).await.unwrap();
        assert_eq!(response.status, 400);
        assert_eq!(response.body, br#"{"error":"missing_content_length"}"#);
    }

    #[tokio::test]
    async fn content_length_mismatch_is_rejected() {
        let head = head_from(b"POST /rtc/offer HTTP/1.1\r\nContent-Length: 10\r\n\r\n");
        let config = WebRtcConfig::new();
        let response = try_handle_rtc_offer(&head, b"abc", &config).await.unwrap();
        assert_eq!(response.status, 400);
        assert_eq!(response.body, br#"{"error":"content_length_mismatch"}"#);
    }

    #[tokio::test]
    async fn non_numeric_content_length_is_rejected() {
        let head = head_from(b"POST /rtc/offer HTTP/1.1\r\nContent-Length: abc\r\n\r\n");
        let config = WebRtcConfig::new();
        let response = try_handle_rtc_offer(&head, b"x", &config).await.unwrap();
        assert_eq!(response.status, 400);
    }

    #[tokio::test]
    async fn oversized_offer_is_rejected() {
        let body = vec![b'a'; 16];
        let head = head_from(b"POST /rtc/offer HTTP/1.1\r\nContent-Length: 16\r\n\r\n");
        let config = WebRtcConfig::new().with_max_offer_bytes(8);
        let response = try_handle_rtc_offer(&head, &body, &config).await.unwrap();
        assert_eq!(response.status, 413);
        assert_eq!(response.body, br#"{"error":"offer_too_large"}"#);
    }

    #[tokio::test]
    async fn invalid_json_offer_is_rejected() {
        let body = b"not json";
        let head = head_from(b"POST /rtc/offer HTTP/1.1\r\nContent-Length: 8\r\n\r\n");
        let config = WebRtcConfig::new();
        let response = try_handle_rtc_offer(&head, body, &config).await.unwrap();
        assert_eq!(response.status, 400);
        assert_eq!(response.body, br#"{"error":"invalid_offer_json"}"#);
    }

    #[tokio::test]
    async fn malformed_sdp_is_rejected() {
        // JSON としては妥当だが SDP 本文が不正なため set_remote_description に失敗する。
        let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
        let head = format!(
            "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let head = head_from(head.as_bytes());
        let config = WebRtcConfig::new().with_udp_bind_addrs(vec!["127.0.0.1:0".to_owned()]);
        let response = try_handle_rtc_offer(&head, body, &config).await.unwrap();
        assert_eq!(response.status, 400);
        assert_eq!(response.body, br#"{"error":"invalid_remote_description"}"#);
    }

    /// 有効な形のリクエストヘッダ + 任意の本文（SDP の中身は問わない）。
    fn offer_request(body: &[u8]) -> RequestHead {
        head_from(
            format!(
                "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
    }

    /// エラー応答の網羅的検証（AGENTS.md「アサーション網羅性」）: ステータスコード・
    /// `Content-Type`・`Content-Length`（直列化後のヘッダ）・ボディをすべて確認する。
    fn assert_json_error(response: &Response, status: u16, body: &[u8]) {
        assert_eq!(response.status, status);
        assert_eq!(response.header("content-type"), Some("application/json"));
        let wire = String::from_utf8(response.serialize(false)).unwrap();
        assert!(
            wire.contains(&format!("Content-Length: {}\r\n", body.len())),
            "直列化後に Content-Length が本文長と一致するはず: {wire}"
        );
        assert_eq!(response.body, body);
    }

    const INIT_FAILED: &[u8] = br#"{"error":"peer_connection_init_failed"}"#;

    #[tokio::test]
    async fn empty_udp_bind_addrs_is_rejected_with_500_without_consuming_a_slot() {
        // 空リストは既定値へ黙って戻さず、PeerConnection を作らずに 500 で拒否する
        // （フェイルクローズ）。枠も消費しないので、上限 1 でも何度でも同じ 500 になる
        // （枠を消費していれば 2 回目以降は 503 になる）。
        let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
        let head = offer_request(body);
        let config = WebRtcConfig::new()
            .with_max_peer_connections(1)
            .with_udp_bind_addrs(Vec::new());
        for _ in 0..3 {
            let response = try_handle_rtc_offer(&head, body, &config).await.unwrap();
            assert_json_error(&response, 500, INIT_FAILED);
        }
        assert!(config.reserve_slot().is_some(), "拒否は枠を残さないはず");
    }

    #[tokio::test]
    async fn empty_udp_bind_addrs_does_not_mask_earlier_input_validation() {
        // 入力検証（Content-Length・サイズ上限）は設定エラーより先に評価され、
        // クライアント起因のエラーは従来どおり 4xx で返る。
        let config = WebRtcConfig::new()
            .with_max_offer_bytes(8)
            .with_udp_bind_addrs(Vec::new());
        let body = vec![b'a'; 16];
        let response = try_handle_rtc_offer(&offer_request(&body), &body, &config)
            .await
            .unwrap();
        assert_json_error(&response, 413, br#"{"error":"offer_too_large"}"#);
    }

    #[tokio::test]
    async fn unusable_udp_bind_addr_is_rejected_with_500_without_consuming_a_slot() {
        // `SocketAddr` として解釈できない要素が 1 つでもある設定は、PeerConnection を作らず
        // 枠も予約せずに 500 で拒否する。ホスト名（`localhost:0` 等）も同様で、webrtc 側へ
        // 文字列のまま渡すとブロッキングの DNS 解決がドライバ上で走るため受け付けない。
        // 枠を消費していれば上限 1 の 2 回目は 503 になる。
        let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
        let head = offer_request(body);
        let unusable: [Vec<&str>; 5] = [
            vec!["not an address"],
            vec!["localhost:0"],
            vec!["example.invalid:0"],
            vec!["127.0.0.1"],                  // ポート無し
            vec!["127.0.0.1:0", "localhost:0"], // 1 つでも不正なら全体を拒否
        ];
        for addrs in unusable {
            let config = WebRtcConfig::new()
                .with_max_peer_connections(1)
                .with_udp_bind_addrs(addrs.iter().map(|a| (*a).to_owned()).collect());
            for _ in 0..2 {
                let response = try_handle_rtc_offer(&head, body, &config).await.unwrap();
                assert_json_error(&response, 500, INIT_FAILED);
            }
            assert!(
                config.reserve_slot().is_some(),
                "{addrs:?}: 拒否は枠を残さないはず"
            );
        }
    }

    #[tokio::test]
    async fn invalid_udp_bind_config_is_reported_before_the_connection_limit() {
        // 設定の誤りは上限判定（503）より先に 500 で返る（誤設定を負荷の問題に見せない）。
        let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
        let config = WebRtcConfig::new()
            .with_max_peer_connections(0)
            .with_udp_bind_addrs(vec!["localhost:0".to_owned()]);
        let response = try_handle_rtc_offer(&offer_request(body), body, &config)
            .await
            .unwrap();
        assert_json_error(&response, 500, INIT_FAILED);
    }

    /// 後始末の検証用に、ループバックだけにバインドしたクライアントから有効な Offer の JSON を作る
    /// （サーバ側が `set_remote_description` を通せる最小の Offer。ICE 収集の完了は待たない）。
    /// 戻り値のクライアントは呼び出し側が閉じる。
    async fn client_offer_json() -> (Arc<dyn PeerConnection>, String) {
        struct NoopHandler;
        #[async_trait]
        impl PeerConnectionEventHandler for NoopHandler {}

        let client: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::new()
                .with_handler(Arc::new(NoopHandler))
                .with_udp_addrs(vec!["127.0.0.1:0"])
                .build()
                .await
                .expect("クライアントの生成に失敗した"),
        );
        client.create_data_channel("t", None).await.unwrap();
        let offer = client.create_offer(None).await.unwrap();
        client.set_local_description(offer.clone()).await.unwrap();
        (client, serde_json::to_string(&offer).unwrap())
    }

    #[tokio::test]
    async fn failure_after_pc_creation_closes_the_pc_and_releases_the_slot() {
        // 失敗経路（ここでは不正 SDP による set_remote_description の失敗）は、生成済みの
        // pc を close して枠を解放し、ガードを解除する（枠の除去と close() の対。close せず
        // drop すると 0.21 ではドライバが残る）。pc は close 後もガードに残るので取り出して確認する。
        let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
        let config = WebRtcConfig::new()
            .with_max_peer_connections(1)
            .with_udp_bind_addrs(vec!["127.0.0.1:0".to_owned()]);
        let slot_id = config.reserve_slot().expect("予約できる");
        let mut guard = SignalingGuard::new(&config, slot_id);

        let response = complete_signaling(body, &config, &mut guard).await;

        assert_json_error(&response, 400, br#"{"error":"invalid_remote_description"}"#);
        let pc = guard
            .pc_cell
            .lock()
            .unwrap()
            .clone()
            .expect("失敗経路でも pc はガードに残る（close 中のキャンセルに備える）");
        assert!(
            pc.create_offer(None).await.is_err(),
            "失敗経路は pc を close 済みのはず"
        );
        assert!(!guard.armed, "失敗経路はガードを解除するはず");
        assert!(
            config.reserve_slot().is_some(),
            "失敗経路は枠も解放するはず"
        );
    }

    #[tokio::test]
    async fn peer_connection_limit_reached_returns_503() {
        // max_peer_connections を 0 にし、レジストリ判定のみで即 503 になる境界を確認する
        // （webrtc-rs の重い初期化処理を経由しないことも本テストの意図）。
        let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
        let head = format!(
            "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let head = head_from(head.as_bytes());
        let config = WebRtcConfig::new().with_max_peer_connections(0);
        let response = try_handle_rtc_offer(&head, body, &config).await.unwrap();
        assert_eq!(response.status, 503);
        assert_eq!(
            response.body,
            br#"{"error":"peer_connection_limit_reached"}"#
        );
    }

    /// `SignalingHandler` へ終端状態を通知し、`Active` の枠が除去されて pc が閉じられることを
    /// 決定的に検証する共通手順。0.21 では `pc.close()` のローカル呼び出しで `Closed` 状態通知は
    /// 届く保証がなく、実ネットワークの `Failed` 遷移は 30 秒級のため、ハンドラを直接駆動する。
    async fn assert_terminal_state_releases_and_closes(state: RTCPeerConnectionState) {
        let config = WebRtcConfig::new()
            .with_max_peer_connections(1)
            .with_udp_bind_addrs(vec!["127.0.0.1:0".to_owned()]);
        let slot_id = config.reserve_slot().expect("1 件目は予約できる");
        let (pc, _gather_rx) = build_peer_connection(&config, slot_id)
            .await
            .expect("PeerConnection の生成に失敗した");
        assert!(
            config.activate_slot(slot_id, Arc::clone(&pc)),
            "terminal drain 前の activate_slot は登録に成功するはず"
        );
        assert!(
            config.reserve_slot().is_none(),
            "上限到達時（1/1 使用中）は新規予約できないはず"
        );
        assert!(
            pc.create_offer(None).await.is_ok(),
            "通知前の pc は開いている（本テストの前提）"
        );

        let (gather_tx, _rx) = watch::channel(false);
        let handler = SignalingHandler {
            gather_tx,
            config: config.clone(),
            slot_id,
        };
        handler.on_connection_state_change(state).await;

        assert!(
            config.reserve_slot().is_some(),
            "{state:?} 通知後はレジストリの枠が解放され、新規予約できるはず"
        );
        assert!(
            wait_until_closed(&pc).await,
            "{state:?} 通知で Active の pc が明示 close されるはず"
        );
    }

    #[tokio::test]
    async fn close_handler_releases_slot_when_state_becomes_closed() {
        // レビュー指摘（イシュー #26）の再発防止テスト: Closed への遷移を検知して
        // レジストリの枠を確実に解放する（0.21 では pc も同時に close する）。
        assert_terminal_state_releases_and_closes(RTCPeerConnectionState::Closed).await;
    }

    #[tokio::test]
    async fn close_handler_releases_slot_when_state_becomes_failed() {
        assert_terminal_state_releases_and_closes(RTCPeerConnectionState::Failed).await;
    }

    #[tokio::test]
    async fn disconnected_state_keeps_slot_and_connection() {
        // `Disconnected` は ICE 再接続で回復しうる非終端状態。早すぎる除去は回復可能な
        // 接続を閉じてしまうため、枠も接続も保持する。
        let config = WebRtcConfig::new()
            .with_max_peer_connections(1)
            .with_udp_bind_addrs(vec!["127.0.0.1:0".to_owned()]);
        let slot_id = config.reserve_slot().expect("予約できる");
        let (pc, _gather_rx) = build_peer_connection(&config, slot_id)
            .await
            .expect("PeerConnection の生成に失敗した");
        assert!(config.activate_slot(slot_id, Arc::clone(&pc)));

        let (gather_tx, _rx) = watch::channel(false);
        let handler = SignalingHandler {
            gather_tx,
            config: config.clone(),
            slot_id,
        };
        handler
            .on_connection_state_change(RTCPeerConnectionState::Disconnected)
            .await;

        assert!(config.reserve_slot().is_none(), "枠は保持されたまま");
        assert!(pc.create_offer(None).await.is_ok(), "接続は開いたまま");

        // 後始末（枠の除去と close は対）。
        config.release_slot(slot_id);
        assert!(wait_until_closed(&pc).await);
    }

    #[tokio::test]
    async fn terminal_state_while_reserved_only_removes_the_slot() {
        // シグナリング進行中（Reserved）に終端状態が届いても pc は complete_signaling が
        // 保持しているため、ここでは枠の除去だけを行い panic しない。
        let config = WebRtcConfig::new().with_max_peer_connections(1);
        let slot_id = config.reserve_slot().expect("予約できる");
        let (gather_tx, _rx) = watch::channel(false);
        let handler = SignalingHandler {
            gather_tx,
            config: config.clone(),
            slot_id,
        };
        handler
            .on_connection_state_change(RTCPeerConnectionState::Failed)
            .await;
        assert!(config.reserve_slot().is_some());
    }

    #[tokio::test]
    async fn signaling_timeout_returns_504() {
        // タイムアウトを極端に短くし、正常な Offer でもシグナリング完了前に打ち切られる
        // ことを確認する（不正 SDP による早期リターンとは別経路を通す）。
        let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
        let head = format!(
            "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let head = head_from(head.as_bytes());
        let config = WebRtcConfig::new()
            .with_signaling_timeout(Duration::from_nanos(1))
            .with_udp_bind_addrs(vec!["127.0.0.1:0".to_owned()]);
        let response = try_handle_rtc_offer(&head, body, &config).await.unwrap();
        // タイムアウト（504）・不正 SDP による早期の 400 のどちらも許容する
        // （PoC-9 教訓: 環境依存のタイミングで parse が先に終わる場合がある）。
        assert!(response.status == 504 || response.status == 400);
    }

    /// ガードの検証用: 枠を予約し、pc を生成してガードへ公開した状態を作る。
    async fn guard_with_published_pc(
        config: &WebRtcConfig,
    ) -> (SignalingGuard, u64, Arc<dyn PeerConnection>) {
        let slot_id = config.reserve_slot().expect("予約できる");
        let guard = SignalingGuard::new(config, slot_id);
        let (pc, _gather_rx) = build_peer_connection(config, slot_id)
            .await
            .expect("PeerConnection の生成に失敗した");
        guard.publish(Arc::clone(&pc));
        (guard, slot_id, pc)
    }

    fn loopback_config() -> WebRtcConfig {
        WebRtcConfig::new()
            .with_max_peer_connections(1)
            .with_udp_bind_addrs(vec!["127.0.0.1:0".to_owned()])
    }

    #[tokio::test]
    async fn guard_finish_closes_the_published_pc_frees_the_slot_and_disarms() {
        // 失敗経路の後始末（旧 close_and_release）: pc を明示的に close し、枠も解放して、
        // ガードを解除する。
        let config = loopback_config();
        let (mut guard, _slot_id, pc) = guard_with_published_pc(&config).await;
        assert!(
            config.reserve_slot().is_none(),
            "上限到達時（1/1 使用中）は新規予約できないはず"
        );

        guard.finish().await;

        assert!(
            pc.create_offer(None).await.is_err(),
            "finish は pc を明示的に close するはず（0.21 には connection_state() が無いため\
             close 後の操作が Err になることで確認する）"
        );
        assert!(!guard.armed);
        assert!(
            config.reserve_slot().is_some(),
            "finish はレジストリの枠も解放するはず"
        );
    }

    #[tokio::test]
    async fn dropping_an_armed_guard_releases_the_slot_and_closes_the_published_pc() {
        // 呼び出し元の Future の drop（JoinSet::shutdown / abort / タイムアウト）で走る経路:
        // finish も activate も呼ばれないまま drop されても、枠は解放され pc は close される。
        let config = loopback_config();
        let (guard, _slot_id, pc) = guard_with_published_pc(&config).await;
        assert!(config.reserve_slot().is_none(), "drop 前は枠を占有している");
        assert!(
            pc.create_offer(None).await.is_ok(),
            "drop 前の pc は開いている"
        );

        drop(guard);

        assert!(
            config.reserve_slot().is_some(),
            "drop したら枠が解放されているはず"
        );
        assert!(
            wait_until_closed(&pc).await,
            "drop したら公開済みの pc が切り離したタスクで close されるはず"
        );
    }

    #[tokio::test]
    async fn dropping_an_armed_guard_without_a_pc_only_releases_the_slot() {
        // pc の生成前（または生成失敗）にキャンセルされた分岐: 解放すべき枠だけがある。
        let config = loopback_config();
        let slot_id = config.reserve_slot().expect("予約できる");
        let guard = SignalingGuard::new(&config, slot_id);
        assert!(config.reserve_slot().is_none());

        drop(guard);

        assert!(
            config.reserve_slot().is_some(),
            "pc が無くても枠は解放される"
        );
    }

    #[tokio::test]
    async fn activated_guard_keeps_the_active_connection_when_dropped() {
        // 成功経路: activate でガードを解除するので、その後の drop は枠にも pc にも触れない
        // （Active の接続を誤って閉じない）。
        let config = loopback_config();
        let (mut guard, slot_id, pc) = guard_with_published_pc(&config).await;

        assert!(guard.activate(), "activate_slot は登録に成功するはず");
        assert!(!guard.armed, "activate はガードを解除するはず");
        drop(guard);

        assert!(config.reserve_slot().is_none(), "Active の枠は残る");
        assert!(
            pc.create_offer(None).await.is_ok(),
            "Active の pc は開いたまま"
        );

        // 後始末（枠の除去と close は対）。
        config.release_slot(slot_id);
        assert!(wait_until_closed(&pc).await);
    }

    #[tokio::test]
    async fn dropping_a_guard_outside_a_tokio_runtime_does_not_panic() {
        // spawn_close は Runtime 外では実行先が無いので何もしない。Drop が panic せず、
        // 枠の除去だけは行うことを確認する（pc の close はランタイム外では行われない）。
        let config = loopback_config();
        let (guard, _slot_id, pc) = guard_with_published_pc(&config).await;

        std::thread::spawn(move || drop(guard))
            .join()
            .expect("ランタイム外の drop が panic した");

        assert!(config.reserve_slot().is_some(), "枠は解放されるはず");
        // ランタイム外では close されない（残余は呼び出し側の責務）ので、ここで閉じる。
        let _ = pc.close().await;
    }

    /// `fut` を `max_polls` 回までポーリングし、`stop` が真になった時点（かつ Future が未完了）で
    /// 打ち切る。打ち切れたら `true`、`stop` が真になる前に完了したら `false`。呼び出し側は
    /// `fut` を drop して「その地点でのキャンセル」を再現する。ポーリング位置を自分で決める
    /// ことで、spawn + abort では取りこぼす「pc を公開した直後で未完了」の瞬間を決定的に捉える。
    async fn poll_until<F: std::future::Future + Unpin>(
        fut: &mut F,
        mut stop: impl FnMut() -> bool,
    ) -> bool {
        std::future::poll_fn(|cx| match std::pin::Pin::new(&mut *fut).poll(cx) {
            std::task::Poll::Ready(_) => std::task::Poll::Ready(false),
            std::task::Poll::Pending if stop() => std::task::Poll::Ready(true),
            std::task::Poll::Pending => std::task::Poll::Pending,
        })
        .await
    }

    #[tokio::test]
    async fn cancelling_complete_signaling_after_the_pc_is_published_is_cleaned_up_by_the_guard() {
        // 旧 `cancelled_complete_signaling_leaves_pc_in_cell...` の置き換え。キャンセルが
        // 「pc 生成前」に当たると後始末の対象が無く、検証が空振りで通ってしまう。そのため
        // pc がガードへ公開され、かつ Future が未完了の地点でポーリングを止めて drop し
        // （`poll_until` の戻り値で、その地点でキャンセルしたことを断言する）、本番コードと
        // 同じガードの `Drop` だけが枠と pc を後始末することを確認する。
        let (client, offer) = client_offer_json().await;
        let config = loopback_config();
        let slot_id = config.reserve_slot().expect("予約できる");
        let mut guard = SignalingGuard::new(&config, slot_id);
        let pc_cell = Arc::clone(&guard.pc_cell);

        let mut fut = Box::pin(complete_signaling(offer.as_bytes(), &config, &mut guard));
        let cancelled_mid_flight = poll_until(&mut fut, || pc_cell.lock().unwrap().is_some()).await;
        assert!(
            cancelled_mid_flight,
            "pc の公開前、または公開後に完走してしまった（キャンセル経路を通っていない）"
        );
        drop(fut); // タイムアウト・呼び出し元の drop と同じ: 以降の本体コードは実行されない

        let pc = pc_cell
            .lock()
            .unwrap()
            .clone()
            .expect("pc は公開済みのはず");
        assert!(guard.armed, "Future の drop だけではガードは解除されない");
        assert!(
            config.reserve_slot().is_none(),
            "Future を drop しただけでは枠はまだ残る（後始末はガードの責務）"
        );
        assert!(pc.create_offer(None).await.is_ok(), "pc もまだ開いている");

        drop(guard);

        assert!(
            config.reserve_slot().is_some(),
            "ガードの Drop で枠が解放されているはず"
        );
        assert!(
            wait_until_closed(&pc).await,
            "ガードの Drop で公開済みの pc が close されるはず"
        );
        let _ = client.close().await;
    }

    #[tokio::test]
    async fn dropping_the_whole_handler_future_mid_flight_leaves_no_slot() {
        // 呼び出し元の Future 全体が途中で drop された場合（JoinSet::shutdown / abort 相当）に、
        // 枠が残らないこと。ポーリング回数を変えて打ち切る（2 回目以降は pc の公開後）。完走した
        // 反復は Active の接続が残るので閉じてから確認する。途中で打ち切った反復が無ければ空振りと
        // みなして失敗させる。
        let (client, offer) = client_offer_json().await;
        let body = offer.into_bytes();
        let mut dropped_mid_flight = 0;
        for polls in 1..=4 {
            let config = loopback_config();
            let head = offer_request(&body);
            let mut fut = Box::pin(try_handle_rtc_offer(&head, &body, &config));
            let mut seen = 0;
            let mid_flight = poll_until(&mut fut, || {
                seen += 1;
                seen >= polls
            })
            .await;
            if mid_flight {
                assert!(
                    config.reserve_slot().is_none(),
                    "{polls} 回目の時点では枠を予約済みのはず"
                );
                dropped_mid_flight += 1;
            }
            drop(fut);
            for pc in config.take_active_peers() {
                let _ = pc.close().await;
            }
            assert!(
                config.reserve_slot().is_some(),
                "{polls} 回ポーリングして drop しても枠が残らないはず"
            );
        }
        assert!(
            dropped_mid_flight > 0,
            "どの反復も途中で打ち切れなかった（空振り）"
        );
        let _ = client.close().await;
    }

    #[tokio::test]
    async fn signaling_deadline_returns_504_and_the_outliving_guard_cleans_up() {
        // タイムアウト分岐の後始末もガードに一本化した。期限が切れるまで決して完了しない
        // シグナリング（pc を公開したあと待ち続ける）に期限を課すと、504 が返り、Future は
        // drop されるが、その外で生きているガードの Drop が枠と pc を後始末する。
        let config = loopback_config();
        let (guard, _slot_id, pc) = guard_with_published_pc(&config).await;

        let response = within_signaling_deadline(Duration::from_millis(20), async {
            std::future::pending::<Response>().await
        })
        .await;

        assert_json_error(&response, 504, br#"{"error":"signaling_timeout"}"#);
        assert!(guard.armed, "期限切れだけではガードは解除されない");
        assert!(config.reserve_slot().is_none(), "Drop までは枠が残る");
        // try_handle_rtc_offer が応答を返して関数を抜けるときと同じ。
        drop(guard);
        assert!(config.reserve_slot().is_some(), "Drop で枠が解放される");
        assert!(wait_until_closed(&pc).await, "Drop で pc が close される");
    }

    /// 指定した UDP バインドアドレスで `PeerConnection` を作り、ICE 収集を終えて得た
    /// ホスト候補のアドレス部分（IPv4 / IPv6 の文字列）を返す。
    async fn gathered_candidate_addrs(config: &WebRtcConfig) -> Vec<String> {
        let slot_id = config.reserve_slot().expect("予約できる");
        let (pc, mut gather_rx) = build_peer_connection(config, slot_id)
            .await
            .expect("PeerConnection の生成に失敗した");
        pc.create_data_channel("t", None).await.unwrap();
        let offer = pc.create_offer(None).await.unwrap();
        pc.set_local_description(offer).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), gather_rx.wait_for(|done| *done))
            .await
            .expect("ICE 収集が 10 秒以内に完了しなかった")
            .unwrap();
        let sdp = pc.local_description().await.unwrap().sdp;
        let addrs = sdp
            .lines()
            .filter(|line| line.starts_with("a=candidate:"))
            .filter_map(|line| line.split_whitespace().nth(4))
            .map(str::to_owned)
            .collect();
        config.release_slot(slot_id);
        let _ = pc.close().await;
        addrs
    }

    #[tokio::test]
    async fn default_udp_bind_gathers_no_ipv6_candidates() {
        // 0.17 系は IPv4 / IPv6 の両方を集めていたが、0.21 の既定（`0.0.0.0:0`）は IPv4 のみ。
        // ホストに IPv6 があるかどうかに依らず、既定では IPv6 の候補が現れない。
        let candidates = gathered_candidate_addrs(&WebRtcConfig::new()).await;
        assert!(
            candidates.iter().all(|addr| !addr.contains(':')),
            "既定の候補に IPv6 が含まれている: {candidates:?}"
        );
    }

    #[tokio::test]
    async fn ipv6_wildcard_alongside_ipv4_builds_even_if_an_address_cannot_be_bound() {
        // 従来の挙動へ戻す構成（`0.0.0.0:0` + `[::]:0`）が build できることと、バインドできない
        // アドレス（ドキュメント用の `2001:db8::1`）が混じっても webrtc 側でスキップされ、
        // 1 つでもバインドできれば接続を生成できることの実測。IPv6 の無いホストで `[::]:0` を
        // 併記しても全体は失敗しない根拠でもある。
        let config = WebRtcConfig::new().with_udp_bind_addrs(vec![
            "127.0.0.1:0".to_owned(),
            "[::]:0".to_owned(),
            "[2001:db8::1]:0".to_owned(),
        ]);
        let candidates = gathered_candidate_addrs(&config).await;
        assert!(
            candidates.iter().any(|addr| addr == "127.0.0.1"),
            "バインドできたアドレスの候補は得られるはず: {candidates:?}"
        );
        assert!(
            candidates.iter().all(|addr| addr != "2001:db8::1"),
            "バインドできなかったアドレスの候補は得られないはず: {candidates:?}"
        );
    }
}
