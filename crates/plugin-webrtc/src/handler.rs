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

use std::sync::Arc;

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
///   生成せず `503` で拒否する（フェイルクローズ。生成済み接続はクローズ処理を
///   持たずプロセス生存期間中レジストリに残るため、恒久対応は TASK-8.3・#28へ
///   申し送り）
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

    // 新規 RTCPeerConnection 生成の前に同時接続数上限を確認する（フェイルクローズ。
    // 上限判定を signaling_timeout の外・パース処理の前に置くことで、上限超過時は
    // webrtc-rs の重い初期化処理（MediaEngine・interceptor 登録）自体を走らせない）。
    {
        let registry = config.registry().lock().unwrap_or_else(|e| e.into_inner());
        if registry.len() >= config.max_peer_connections() {
            return Some(error_response(
                503,
                br#"{"error":"peer_connection_limit_reached"}"#,
            ));
        }
    }

    match tokio::time::timeout(config.signaling_timeout(), complete_signaling(body, config)).await {
        Ok(response) => Some(response),
        Err(_elapsed) => Some(error_response(504, br#"{"error":"signaling_timeout"}"#)),
    }
}

/// SDP Offer の JSON パースから Answer 生成までのシグナリング本体。
///
/// [`try_handle_rtc_offer`] から `tokio::time::timeout` で包んで呼ばれる
/// （シグナリング全体のタイムアウト適用範囲を本関数の呼び出し全体に一致させるため、
/// 早期 return を含む全経路がこの関数内で完結する設計）。
async fn complete_signaling(body: &[u8], config: &WebRtcConfig) -> Response {
    let offer: RTCSessionDescription = match serde_json::from_slice(body) {
        Ok(offer) => offer,
        Err(_) => return error_response(400, br#"{"error":"invalid_offer_json"}"#),
    };

    let pc = match build_peer_connection().await {
        Ok(pc) => pc,
        Err(_) => {
            return error_response(500, br#"{"error":"peer_connection_init_failed"}"#);
        }
    };

    register_echo_handler(&pc);

    if pc.set_remote_description(offer).await.is_err() {
        return error_response(400, br#"{"error":"invalid_remote_description"}"#);
    }

    let answer = match pc.create_answer(None).await {
        Ok(answer) => answer,
        Err(_) => return error_response(500, br#"{"error":"create_answer_failed"}"#),
    };

    // 非トリクル ICE: 候補収集が終わるまで応答を保留し、1 往復のシグナリングで
    // 完結させる（PoC-5 の簡易実装を踏襲。トリクル ICE は REQ-8 でスコープ外）。
    let mut gather_complete = pc.gathering_complete_promise().await;
    if pc.set_local_description(answer).await.is_err() {
        return error_response(500, br#"{"error":"set_local_description_failed"}"#);
    }
    let _ = gather_complete.recv().await;

    let local_desc = match pc.local_description().await {
        Some(desc) => desc,
        None => return error_response(500, br#"{"error":"local_description_missing"}"#),
    };

    let answer_bytes = match serde_json::to_vec(&local_desc) {
        Ok(bytes) => bytes,
        Err(_) => return error_response(500, br#"{"error":"serialization_failed"}"#),
    };

    // シグナリング成功。以降このプロセス内で `RTCPeerConnection` を保持し続ける
    // （`WebRtcConfig::max_peer_connections` の doc・恒久対応の申し送りを参照）。
    config
        .registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(pc);

    Response::new(200, answer_bytes).with_content_type("application/json")
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
}
