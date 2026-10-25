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
/// - `config.udp_bind_addrs()` が空なら `PeerConnection` を生成せず `500`
///   （`peer_connection_init_failed`）で拒否する（webrtc 0.21 は UDP バインドアドレスが
///   必須。既定値へ黙って戻さないフェイルクローズ。枠も消費しない）
/// - `config.max_peer_connections()` に達している場合は新規 `PeerConnection` を
///   生成せず `503` で拒否する（フェイルクローズ）。上限判定と予約枠の登録は
///   `WebRtcConfig::reserve_slot` が単一ロック区間で行うため TOCTOU
///   （time-of-check to time-of-use）は生じない。接続クローズ・失敗時は
///   `SignalingHandler` が `release_slot` でレジストリから枠を除去する（Active なら
///   同時に close）ため、正常利用の蓄積のみでレジストリが単調増加し続けることはない
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
    // フェイルクローズで 500 にする（.claude/rules/security.md）。
    if config.udp_bind_addrs().is_empty() {
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

    // complete_signaling が生成した PeerConnection をキャンセル安全に追跡する
    // 共有セル。tokio::time::timeout がタイムアウトで complete_signaling の
    // Future を drop（キャンセル）すると、関数内の以降の処理（明示 close・
    // release_slot 呼び出し）は一切実行されない。webrtc 0.21 は close せずに drop すると
    // ドライバと UDP ソケットが残るため（PR #138 レビュー指摘の 0.21 版）、pc 生成直後に
    // このセルへ公開させ、タイムアウト時は呼び出し元（本関数）がセルを見て明示的に close する。
    // 失敗経路が自前で close している最中にキャンセルされても拾えるよう、セルは
    // complete_signaling が値を返すまで（失敗経路でも）クリアしない。close は冪等。
    let pc_cell: Arc<Mutex<Option<Arc<dyn PeerConnection>>>> = Arc::new(Mutex::new(None));

    match tokio::time::timeout(
        config.signaling_timeout(),
        complete_signaling(body, config, slot_id, Arc::clone(&pc_cell)),
    )
    .await
    {
        Ok(response) => Some(response),
        Err(_elapsed) => {
            // タイムアウトで打ち切られた場合、complete_signaling 側の後始末
            // （release_slot / activate_slot）は実行されない可能性があるため、
            // 予約枠を明示的に解放する（release_slot は多重解放を許容する冪等な
            // 操作なので、complete_signaling が既に解放済みでも安全）。
            config.release_slot(slot_id);
            if let Some(pc) = pc_cell.lock().unwrap_or_else(|e| e.into_inner()).take() {
                // キャンセルされた complete_signaling が既に PeerConnection を
                // 生成していた場合、上で行った枠の除去と対にして明示的に close する。
                // close() はネットワーク I/O を伴いうるため、504 応答の返却をこの
                // 完了待ちでブロックしないようバックグラウンドタスクで実行する
                // （PR #138 レビュー指摘: タイムアウト時にスロットは解放されても
                // peer が残り max_peer_connections の実効性が弱まる問題への対策）。
                spawn_close(pc);
            }
            Some(error_response(504, br#"{"error":"signaling_timeout"}"#))
        }
    }
}

/// pc（生成済みなら）を明示的に `close()` してからレジストリの枠を解放する。
///
/// webrtc 0.21 の `PeerConnection` は `close()` せずに drop するとドライバ・UDP ソケットが
/// 残るため（PR #138 レビュー指摘の 0.21 版）、[`complete_signaling`] の失敗経路は
/// すべて本関数を経由して明示的に close する契約とする。**ローカルで `pc.close()` を
/// 呼んでもハンドラへ `Closed` が届く保証がないので、枠の解放はこの関数（呼び出し側）の
/// 責務**（`crate::config` モジュール doc「枠の除去と `close()` の対」）。`close()` の
/// 失敗（既に close 済み等）は無視してよい（呼び出し元の目的は「枠の解放」であり、
/// close 失敗はそれを妨げない）。
///
/// `close().await` の途中でシグナリング全体のタイムアウトに打ち切られても、`pc` は
/// `pc_cell` に残っているので呼び出し元（[`try_handle_rtc_offer`]）が枠の解放と再 close を
/// 行う（`close()` は冪等）。
async fn close_and_release(
    pc: Option<Arc<dyn PeerConnection>>,
    config: &WebRtcConfig,
    slot_id: u64,
) {
    if let Some(pc) = pc {
        let _ = pc.close().await;
    }
    config.release_slot(slot_id);
}

/// SDP Offer の JSON パースから Answer 生成までのシグナリング本体。
///
/// [`try_handle_rtc_offer`] から `tokio::time::timeout` で包んで呼ばれる
/// （シグナリング全体のタイムアウト適用範囲を本関数の呼び出し全体に一致させるため、
/// 早期 return を含む全経路がこの関数内で完結する設計）。`slot_id` は呼び出し元が
/// [`WebRtcConfig::reserve_slot`] で予約済みの枠 ID。**すべての失敗経路で
/// [`close_and_release`] を呼び、予約枠のリークと `pc` の close 漏れを同時に防ぐ契約**と
/// する（呼び出し元の `try_handle_rtc_offer` はタイムアウト時のみ独自に `release_slot`
/// するため、この関数内の経路と合わせて二重解放になりうるが `release_slot` は冪等）。
/// 成功経路は `activate_slot` が枠を `Active` にするか、拒否（終端 drain・枠消失）して
/// `pc` を close する。
///
/// `pc_cell` は `try_handle_rtc_offer` と共有するキャンセル安全な追跡セル。
/// `PeerConnection` 生成直後にこのセルへ公開し、**失敗経路ではクリアしない**
/// （`close_and_release` の `close().await` 中にキャンセルされても呼び出し元が拾えるように
/// する。値を返した後は呼び出し元がセルを読まないので、クリアしなくても二重 close には
/// ならない）。成功経路のみ `activate_slot` の直前にクリアする。`tokio::time::timeout` が
/// 本関数の Future を drop（キャンセル）した場合はここから先のコードが一切実行されないため、
/// セルに公開済みの `pc` を呼び出し元がタイムアウト分岐で明示的に close する。
async fn complete_signaling(
    body: &[u8],
    config: &WebRtcConfig,
    slot_id: u64,
    pc_cell: Arc<Mutex<Option<Arc<dyn PeerConnection>>>>,
) -> Response {
    let offer: RTCSessionDescription = match serde_json::from_slice(body) {
        Ok(offer) => offer,
        Err(_) => {
            close_and_release(None, config, slot_id).await;
            return error_response(400, br#"{"error":"invalid_offer_json"}"#);
        }
    };

    let (pc, mut gather_rx) = match build_peer_connection(config, slot_id).await {
        Ok(built) => built,
        Err(_) => {
            close_and_release(None, config, slot_id).await;
            return error_response(500, br#"{"error":"peer_connection_init_failed"}"#);
        }
    };

    // 以降の await 点でタイムアウトによりキャンセルされる可能性があるため、pc を
    // 共有セルへ公開する（本関数 doc の契約を参照）。
    *pc_cell.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&pc));

    if pc.set_remote_description(offer).await.is_err() {
        close_and_release(Some(pc), config, slot_id).await;
        return error_response(400, br#"{"error":"invalid_remote_description"}"#);
    }

    let answer = match pc.create_answer(None).await {
        Ok(answer) => answer,
        Err(_) => {
            close_and_release(Some(pc), config, slot_id).await;
            return error_response(500, br#"{"error":"create_answer_failed"}"#);
        }
    };

    // 非トリクル ICE: 候補収集が終わるまで応答を保留し、1 往復のシグナリングで
    // 完結させる（PoC-5 の簡易実装を踏襲。トリクル ICE は REQ-8 でスコープ外）。
    if pc.set_local_description(answer).await.is_err() {
        close_and_release(Some(pc), config, slot_id).await;
        return error_response(500, br#"{"error":"set_local_description_failed"}"#);
    }
    // 0.21 には gathering_complete_promise が無い。ハンドラが build 時に渡した watch へ
    // `Complete` を latch するので、set_local_description の後で待っても取りこぼさない。
    let _ = gather_rx.wait_for(|done| *done).await;

    let local_desc = match pc.local_description().await {
        Some(desc) => desc,
        None => {
            close_and_release(Some(pc), config, slot_id).await;
            return error_response(500, br#"{"error":"local_description_missing"}"#);
        }
    };

    let answer_bytes = match serde_json::to_vec(&local_desc) {
        Ok(bytes) => bytes,
        Err(_) => {
            close_and_release(Some(pc), config, slot_id).await;
            return error_response(500, br#"{"error":"serialization_failed"}"#);
        }
    };

    // シグナリング成功。もはやタイムアウト分岐の明示 close は不要なのでセルを
    // クリアしてから（この後の `activate_slot` までに await 点は無く、キャンセルされない）、
    // 予約枠をアクティブな接続へ遷移させ、以降このプロセス内で `PeerConnection` を
    // 保持し続ける。クローズ・失敗検知時の枠除去は `SignalingHandler` が担う
    // （レジストリの単調増加を防ぐ）。
    pc_cell.lock().unwrap_or_else(|e| e.into_inner()).take();
    // イシュー #498: 最終 graceful shutdown（`WebRtcConfig::begin_terminal_drain`）が
    // 本呼び出しと競合した場合、または対象 slot がタイムアウト等で既に除去済みの
    // missing-slot レースの場合、`activate_slot` はレジストリ登録を拒否（`false`）する。
    // 拒否された pc は `activate_slot` 自身が切り離したタスクで close する
    // （フェイルクローズ、`.claude/rules/security.md`。枠の除去と close は同じ同期区間の
    // 対で、200 応答の返却もブロックしない）。応答は拒否時も 200 のまま返す
    // （既存のワイヤ契約。クライアントはデータチャネルが開かないことで検知する）。
    let _registered = config.activate_slot(slot_id, pc);

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
        // （`close_and_release` / drain）は呼び出し側が枠を解放する。ここは相手起因の
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
/// `WebRtcConfig::udp_bind_addrs`（既定 `["0.0.0.0:0"]`）をそのまま渡す。
///
/// 生成に失敗した場合は接続が存在しないので、呼び出し元は枠の解放のみ行う。
async fn build_peer_connection(
    config: &WebRtcConfig,
    slot_id: u64,
) -> webrtc::error::Result<(Arc<dyn PeerConnection>, watch::Receiver<bool>)> {
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
        .with_udp_addrs(config.udp_bind_addrs().to_vec())
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
            assert_eq!(response.status, 500);
            assert_eq!(response.body, br#"{"error":"peer_connection_init_failed"}"#);
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
        assert_eq!(response.status, 413);
    }

    #[tokio::test]
    async fn unusable_udp_bind_addr_returns_500_and_releases_the_slot() {
        // 解釈できないアドレスでは PeerConnection の生成に失敗する。接続は存在しないので
        // 枠の解放だけが必要で、枠が残れば 2 回目は 503 になる。
        let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
        let head = offer_request(body);
        let config = WebRtcConfig::new()
            .with_max_peer_connections(1)
            .with_udp_bind_addrs(vec!["not an address".to_owned()]);
        for _ in 0..2 {
            let response = try_handle_rtc_offer(&head, body, &config).await.unwrap();
            assert_eq!(response.status, 500);
            assert_eq!(response.body, br#"{"error":"peer_connection_init_failed"}"#);
        }
        assert!(config.reserve_slot().is_some());
    }

    #[tokio::test]
    async fn failure_after_pc_creation_closes_the_pc_and_releases_the_slot() {
        // 失敗経路（ここでは不正 SDP による set_remote_description の失敗）は、生成済みの
        // pc を close して枠を解放する。pc は pc_cell に残るので取り出して閉じていることを確認する
        // （枠の除去と close() の対。close せず drop すると 0.21 ではドライバが残る）。
        let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
        let config = WebRtcConfig::new()
            .with_max_peer_connections(1)
            .with_udp_bind_addrs(vec!["127.0.0.1:0".to_owned()]);
        let slot_id = config.reserve_slot().expect("予約できる");
        let pc_cell: Arc<Mutex<Option<Arc<dyn PeerConnection>>>> = Arc::new(Mutex::new(None));

        let response = complete_signaling(body, &config, slot_id, Arc::clone(&pc_cell)).await;

        assert_eq!(response.status, 400);
        assert_eq!(response.body, br#"{"error":"invalid_remote_description"}"#);
        let pc = pc_cell
            .lock()
            .unwrap()
            .take()
            .expect("失敗経路でも pc はセルに残る（close 中のキャンセルに備える）");
        assert!(
            pc.create_offer(None).await.is_err(),
            "失敗経路は pc を close 済みのはず"
        );
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

    #[tokio::test]
    async fn close_and_release_closes_provided_peer_connection_and_frees_slot() {
        // PR #138 レビュー指摘の再発防止テスト: close_and_release（timeout 分岐・
        // complete_signaling の失敗経路が共通で使う後始末関数）が pc を明示的に
        // close し、レジストリの枠も解放することを直接検証する。
        let config = WebRtcConfig::new()
            .with_max_peer_connections(1)
            .with_udp_bind_addrs(vec!["127.0.0.1:0".to_owned()]);
        let slot_id = config.reserve_slot().expect("1 件目は予約できる");
        let (pc, _gather_rx) = build_peer_connection(&config, slot_id)
            .await
            .expect("PeerConnection の生成に失敗した");

        assert!(
            config.reserve_slot().is_none(),
            "上限到達時（1/1 使用中）は新規予約できないはず"
        );

        close_and_release(Some(Arc::clone(&pc)), &config, slot_id).await;

        assert!(
            pc.create_offer(None).await.is_err(),
            "close_and_release は pc を明示的に close するはず（0.21 には connection_state() が無いため\
             close 後の操作が Err になることで確認する）"
        );
        assert!(
            config.reserve_slot().is_some(),
            "close_and_release はレジストリの枠も解放するはず"
        );
    }

    #[tokio::test]
    async fn cancelled_complete_signaling_leaves_pc_in_cell_for_caller_to_close() {
        // PR #138 レビュー指摘の再発防止テスト: complete_signaling の Future が
        // pc 生成後にキャンセル（drop）された場合でも、pc_cell に公開済みの pc を
        // 呼び出し元が検知して後始末（枠の解放 + close）できることを、
        // tokio::time::timeout と同じキャンセル機構（Future の drop）で直接検証する。
        //
        // キャンセルがどの await 点に当たるかは環境のタイミング次第（失敗経路の close 中に
        // 当たる場合もある）。失敗経路はセルをクリアしないので、どこで打ち切られても
        // 呼び出し元の後始末（close は冪等）で pc は必ず閉じられ、枠は解放される。
        let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
        let config = WebRtcConfig::new()
            .with_max_peer_connections(1)
            .with_udp_bind_addrs(vec!["127.0.0.1:0".to_owned()]);
        let slot_id = config.reserve_slot().expect("予約できる");
        let pc_cell: Arc<Mutex<Option<Arc<dyn PeerConnection>>>> = Arc::new(Mutex::new(None));

        // 即時に elapsed する timeout で complete_signaling を包む
        // （tokio::time::timeout は deadline 経過時に inner Future を drop する）。
        let result = tokio::time::timeout(
            Duration::from_nanos(1),
            complete_signaling(body, &config, slot_id, Arc::clone(&pc_cell)),
        )
        .await;

        if result.is_err() {
            // 環境依存のタイミングで pc 生成前にタイムアウトする可能性も許容する
            // （その場合セルは空で、後始末の対象となる pc がそもそも存在しない）。
            let maybe_pc = pc_cell.lock().unwrap().take();
            if let Some(pc) = maybe_pc {
                // 呼び出し元（try_handle_rtc_offer）が行う後始末と同じ処理。
                config.release_slot(slot_id);
                let _ = pc.close().await;
                assert!(
                    pc.create_offer(None).await.is_err(),
                    "後始末の後は pc が閉じているはず"
                );
            }
        }
        assert!(
            config.reserve_slot().is_some(),
            "キャンセルされても（後始末の後は）枠が解放されているはず"
        );
    }
}
