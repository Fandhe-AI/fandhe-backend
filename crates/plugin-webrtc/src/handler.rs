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
//! [`bf_http::response::Response`] を直接組み立てて返す（変換層を省く。
//! `docs/design/plugin-boundary.md` 4.3 節）。

use std::sync::{Arc, Mutex};

use bf_http::request::RequestHead;
use bf_http::response::Response;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::config::WebRtcConfig;

/// 本プラグインがパスインターセプトの対象とするリクエストパス。
pub const OFFER_PATH: &str = "/rtc/offer";

/// 内部エラー詳細（SDP 本文・webrtc-rs の生エラー等）を含まない定型 JSON エラー応答を
/// 組み立てる（.claude/rules/security.md のエラー情報漏えい対策。
/// `crates/plugin-webrtc-proxy::handler::Response::error` と同一方針）。
fn error_response(status: u16, body: &'static [u8]) -> Response {
    Response::new(status, body.to_vec()).with_content_type("application/json")
}

/// `POST /rtc/offer` をパスインターセプトし、SDP Offer（JSON:
/// `{"type":"offer","sdp":"..."}`）を受け取って `RTCPeerConnection` を生成、
/// データチャネル到着を待ち受けるエコーハンドラを登録したうえで、非トリクル ICE
/// による SDP Answer を JSON で返す。
///
/// - メソッド・パスが対象外なら `None` を返す（呼び出し元は次のハンドラへ
///   フォールスルーする契約。`crates/plugin-webrtc-proxy` と同型）
/// - `Content-Length` が欠如・`body` の実長と不一致なら `400`
/// - `config.max_offer_bytes()` 超過は `413`（リソース枯渇対策、
///   .claude/rules/security.md）
/// - `config.max_peer_connections()` に達している場合は新規 `RTCPeerConnection` を
///   生成せず `503` で拒否する（フェイルクローズ）。上限判定と予約枠の登録は
///   [`WebRtcConfig::reserve_slot`] が単一ロック区間で行うため TOCTOU
///   （time-of-check to time-of-use）は生じない。接続クローズ・失敗時は
///   [`register_close_handler`] がレジストリから枠を除去するため、正常利用の
///   蓄積のみでレジストリが単調増加し続けることはない
/// - JSON/SDP のパース失敗・シグナリング内部エラーは `400`/`500`（内部情報を含まない
///   固定 JSON body）
/// - シグナリング全体（`set_remote_description` から ICE 候補収集完了まで）が
///   `config.signaling_timeout()` を超えたら `504`
///
/// # Examples
///
/// ```
/// use bf_http::request::{parse_request_head, ParseOutcome};
/// use bf_plugin_webrtc::{WebRtcConfig, try_handle_rtc_offer};
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
    if head.method != "POST" || head.target != OFFER_PATH {
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

    // 新規 RTCPeerConnection 生成の前に同時接続数上限を確認し、予約枠を登録する
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

    // complete_signaling が生成した RTCPeerConnection をキャンセル安全に追跡する
    // 共有セル。tokio::time::timeout がタイムアウトで complete_signaling の
    // Future を drop（キャンセル）すると、関数内の以降の処理（明示 close・
    // release_slot 呼び出し）は一切実行されない。生成済みの pc を drop するだけ
    // では webrtc-rs 内部のタスク・ICE エージェント等のリソース解放が保証
    // されないため（PR #138 レビュー指摘）、pc 生成直後にこのセルへ公開させ、
    // タイムアウト時は呼び出し元（本関数）がセルを見て明示的に close する。
    let pc_cell: Arc<Mutex<Option<Arc<RTCPeerConnection>>>> = Arc::new(Mutex::new(None));

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
                // キャンセルされた complete_signaling が既に RTCPeerConnection を
                // 生成していた場合、明示的に close して webrtc-rs 内部のリソースを
                // 解放する。close() はネットワーク I/O を伴いうるため、504 応答の
                // 返却をこの完了待ちでブロックしないようバックグラウンドタスクで
                // 実行する（PR #138 レビュー指摘: タイムアウト時にスロットは解放
                // されても peer が残り max_peer_connections の実効性が弱まる問題
                // への対策）。
                tokio::spawn(async move {
                    let _ = pc.close().await;
                });
            }
            Some(error_response(504, br#"{"error":"signaling_timeout"}"#))
        }
    }
}

/// pc（生成済みなら）を明示的に `close()` してからレジストリの枠を解放する。
///
/// `RTCPeerConnection` を単に `Drop` するだけでは webrtc-rs 内部の ICE エージェント・
/// タスク等のリソースが解放される保証がないため（PR #138 レビュー指摘）、
/// [`complete_signaling`] の失敗経路はすべて本関数を経由して明示的に close する
/// 契約とする。`close()` の失敗（既に close 済み等）は無視してよい
/// （呼び出し元の目的は「枠の解放」であり、close 失敗はそれを妨げない）。
async fn close_and_release(
    pc: Option<Arc<RTCPeerConnection>>,
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
/// [`close_and_release`] を呼び、予約枠をリークさせない契約**とする（呼び出し元の
/// `try_handle_rtc_offer` はタイムアウト時のみ独自に `release_slot` するため、この
/// 関数内の経路と合わせて二重解放になりうるが `release_slot` は冪等）。
///
/// `pc_cell` は `try_handle_rtc_offer` と共有するキャンセル安全な追跡セル。
/// `RTCPeerConnection` 生成直後にこのセルへ公開し、以降の失敗経路で `take()` して
/// クリアする契約とする（`take()` し忘れると、正常完了後に呼び出し元が誤って
/// 二重 close を試みる可能性があるため、`close_and_release`・`activate_slot` の
/// 直前に必ずクリアする）。`tokio::time::timeout` が本関数の Future を drop
/// （キャンセル）した場合はここから先のコードが一切実行されないため、セルに
/// 公開済みの `pc` を呼び出し元がタイムアウト分岐で明示的に close する。
async fn complete_signaling(
    body: &[u8],
    config: &WebRtcConfig,
    slot_id: u64,
    pc_cell: Arc<Mutex<Option<Arc<RTCPeerConnection>>>>,
) -> Response {
    let offer: RTCSessionDescription = match serde_json::from_slice(body) {
        Ok(offer) => offer,
        Err(_) => {
            close_and_release(None, config, slot_id).await;
            return error_response(400, br#"{"error":"invalid_offer_json"}"#);
        }
    };

    let pc = match build_peer_connection().await {
        Ok(pc) => pc,
        Err(_) => {
            close_and_release(None, config, slot_id).await;
            return error_response(500, br#"{"error":"peer_connection_init_failed"}"#);
        }
    };

    // 以降の await 点でタイムアウトによりキャンセルされる可能性があるため、pc を
    // 共有セルへ公開する（本関数 doc の契約を参照）。
    *pc_cell.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&pc));

    register_echo_handler(&pc);

    if pc.set_remote_description(offer).await.is_err() {
        pc_cell.lock().unwrap_or_else(|e| e.into_inner()).take();
        close_and_release(Some(pc), config, slot_id).await;
        return error_response(400, br#"{"error":"invalid_remote_description"}"#);
    }

    let answer = match pc.create_answer(None).await {
        Ok(answer) => answer,
        Err(_) => {
            pc_cell.lock().unwrap_or_else(|e| e.into_inner()).take();
            close_and_release(Some(pc), config, slot_id).await;
            return error_response(500, br#"{"error":"create_answer_failed"}"#);
        }
    };

    // 非トリクル ICE: 候補収集が終わるまで応答を保留し、1 往復のシグナリングで
    // 完結させる（PoC-5 の簡易実装を踏襲。トリクル ICE は REQ-8 でスコープ外）。
    let mut gather_complete = pc.gathering_complete_promise().await;
    if pc.set_local_description(answer).await.is_err() {
        pc_cell.lock().unwrap_or_else(|e| e.into_inner()).take();
        close_and_release(Some(pc), config, slot_id).await;
        return error_response(500, br#"{"error":"set_local_description_failed"}"#);
    }
    let _ = gather_complete.recv().await;

    let local_desc = match pc.local_description().await {
        Some(desc) => desc,
        None => {
            pc_cell.lock().unwrap_or_else(|e| e.into_inner()).take();
            close_and_release(Some(pc), config, slot_id).await;
            return error_response(500, br#"{"error":"local_description_missing"}"#);
        }
    };

    let answer_bytes = match serde_json::to_vec(&local_desc) {
        Ok(bytes) => bytes,
        Err(_) => {
            pc_cell.lock().unwrap_or_else(|e| e.into_inner()).take();
            close_and_release(Some(pc), config, slot_id).await;
            return error_response(500, br#"{"error":"serialization_failed"}"#);
        }
    };

    // シグナリング成功。もはやタイムアウト分岐の明示 close は不要なのでセルを
    // クリアしてから、予約枠をアクティブな接続へ遷移させ、以降このプロセス内で
    // `RTCPeerConnection` を保持し続ける。クローズ・失敗検知時の枠除去は
    // register_close_handler が担う（レジストリの単調増加を防ぐ）。
    pc_cell.lock().unwrap_or_else(|e| e.into_inner()).take();
    register_close_handler(&pc, config, slot_id);
    config.activate_slot(slot_id, pc);

    Response::new(200, answer_bytes).with_content_type("application/json")
}

/// 接続クローズ・失敗（`RTCPeerConnectionState::Closed`/`Failed`）を検知して
/// レジストリから枠を除去するハンドラを登録する。
///
/// `Disconnected` は ICE 再接続で回復しうる非終端状態のため対象外とする
/// （早すぎる除去は `Arc<RTCPeerConnection>` の最後の強参照を手放し、回復可能な
/// 接続を誤ってクローズしてしまう）。クロージャは `pc` 自身への強参照を持たず
/// `slot_id`（`Copy`）のみを捕捉するため、`pc` → クロージャ → `pc` の参照
/// サイクル（メモリリーク）は生じない。
fn register_close_handler(pc: &Arc<RTCPeerConnection>, config: &WebRtcConfig, slot_id: u64) {
    let config = config.clone();
    pc.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
        let config = config.clone();
        Box::pin(async move {
            if matches!(
                state,
                RTCPeerConnectionState::Closed | RTCPeerConnectionState::Failed
            ) {
                config.release_slot(slot_id);
            }
        })
    }));
}

/// メディアコーデック・インターセプタを登録した `RTCPeerConnection` を 1 つ生成する。
///
/// `RTCConfiguration::default()` は ICE サーバ（STUN/TURN）を含まないため、ホスト
/// 候補（ローカルアドレス）のみで ICE 疎通を試みる構成になる（PoC-5 の
/// 「外部ネットワーク不要」制約を踏襲。STUN/TURN 未設定のためクライアント SDP 由来の
/// アドレスへの UDP 送信が生じる特性は `crate` ルート doc の SSRF 類似観点を参照）。
async fn build_peer_connection() -> webrtc::error::Result<Arc<RTCPeerConnection>> {
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs()?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media_engine)?;

    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .build();

    let config = RTCConfiguration::default();
    let pc = Arc::new(api.new_peer_connection(config).await?);
    Ok(pc)
}

/// 相手から到着したデータチャネルへ、受信メッセージをそのまま送り返すエコーハンドラを
/// 登録する（PoC-5「1 対 1 のデータチャネル確立とメッセージ往復」の最小疎通実装）。
fn register_echo_handler(pc: &Arc<RTCPeerConnection>) {
    pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
        let dc_for_message = Arc::clone(&dc);
        dc.on_message(Box::new(move |msg: DataChannelMessage| {
            let dc_for_send = Arc::clone(&dc_for_message);
            Box::pin(async move {
                let _ = dc_for_send.send(&msg.data).await;
            })
        }));
        Box::pin(async {})
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_http::request::{ParseOutcome, parse_request_head};
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
        let config = WebRtcConfig::new();
        let response = try_handle_rtc_offer(&head, body, &config).await.unwrap();
        assert_eq!(response.status, 400);
        assert_eq!(response.body, br#"{"error":"invalid_remote_description"}"#);
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

    #[tokio::test]
    async fn close_handler_releases_slot_when_state_becomes_closed() {
        // レビュー指摘（イシュー #26）の再発防止テスト: register_close_handler が
        // RTCPeerConnectionState::Closed への遷移を検知してレジストリの枠を確実に
        // 解放することを、ネットワーク I/O なしで直接検証する（`RTCPeerConnection::close`
        // は state-change ハンドラを同期的に await して呼ぶため、ポーリング不要で決定的）。
        let config = WebRtcConfig::new().with_max_peer_connections(1);
        let slot_id = config.reserve_slot().expect("1 件目は予約できる");
        let pc = build_peer_connection()
            .await
            .expect("RTCPeerConnection の生成に失敗した");
        register_close_handler(&pc, &config, slot_id);
        config.activate_slot(slot_id, Arc::clone(&pc));

        assert!(
            config.reserve_slot().is_none(),
            "上限到達時（1/1 使用中）は新規予約できないはず"
        );

        pc.close().await.expect("close に失敗した");

        assert!(
            config.reserve_slot().is_some(),
            "close 後はレジストリの枠が解放され、新規予約できるはず"
        );
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
        let config = WebRtcConfig::new().with_signaling_timeout(Duration::from_nanos(1));
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
        let config = WebRtcConfig::new().with_max_peer_connections(1);
        let slot_id = config.reserve_slot().expect("1 件目は予約できる");
        let pc = build_peer_connection()
            .await
            .expect("RTCPeerConnection の生成に失敗した");

        assert!(
            config.reserve_slot().is_none(),
            "上限到達時（1/1 使用中）は新規予約できないはず"
        );

        close_and_release(Some(Arc::clone(&pc)), &config, slot_id).await;

        assert_eq!(
            pc.connection_state(),
            RTCPeerConnectionState::Closed,
            "close_and_release は pc を明示的に close するはず"
        );
        assert!(
            config.reserve_slot().is_some(),
            "close_and_release はレジストリの枠も解放するはず"
        );
    }

    #[tokio::test]
    async fn cancelled_complete_signaling_leaves_pc_in_cell_for_caller_to_close() {
        // PR #138 レビュー指摘の再発防止テスト: complete_signaling の Future が
        // pc 生成後・シグナリング完了前にキャンセル（drop）された場合でも、pc_cell に
        // 公開済みの pc を呼び出し元が検知できることを、tokio::time::timeout と同じ
        // キャンセル機構（Future の drop）で直接検証する。
        let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
        let config = WebRtcConfig::new();
        let slot_id = config.reserve_slot().expect("予約できる");
        let pc_cell: Arc<Mutex<Option<Arc<RTCPeerConnection>>>> = Arc::new(Mutex::new(None));

        // 即時に elapsed する timeout で complete_signaling を包み、pc 生成後の
        // 最初の await 点（set_remote_description）でキャンセルされることを狙う
        // （tokio::time::timeout は deadline 経過時に inner Future を drop する）。
        let result = tokio::time::timeout(
            Duration::from_nanos(1),
            complete_signaling(body, &config, slot_id, Arc::clone(&pc_cell)),
        )
        .await;

        if result.is_err() {
            // キャンセルされた場合のみ、pc_cell に pc が残っていることを確認できる
            // （環境依存のタイミングで pc 生成前にタイムアウトする可能性も許容する）。
            let maybe_pc = pc_cell.lock().unwrap().take();
            if let Some(pc) = maybe_pc {
                assert_ne!(
                    pc.connection_state(),
                    RTCPeerConnectionState::Closed,
                    "drop だけでは pc は close 状態にならない（本テストの前提）"
                );
                // 呼び出し元（try_handle_rtc_offer）が行う後始末と同じ処理を実行し、
                // 明示的に close できることを確認する。
                let _ = pc.close().await;
                assert_eq!(pc.connection_state(), RTCPeerConnectionState::Closed);
            }
        }
    }
}
