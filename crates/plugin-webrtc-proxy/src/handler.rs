//! パスインターセプト型のリクエスト/レスポンス完結型ハンドラ（[`try_handle_rtc_offer`]）。
//!
//! `crates/plugin-webrtc`（TASK-8.1）・PoC-5（`docs/spec/03-poc/webrtc-plugin/`）と
//! 同型のパターンを踏襲する: 対象パス以外は即 `None` を返し、無関係なリクエストへの
//! 性能影響をゼロにする。コアの接続受理ループ（TASK-1.4-2 / #70）・feature 配線
//! （TASK-2.1 / #18）から呼ばれる想定だが、本タスク（#74）時点ではどちらも未配線
//! のため、本モジュールはハンドラ関数単体として自己完結させる
//! （crate ルート `lib.rs` の「コアループへの配線について」参照）。

use fandhe_backend_http::request::RequestHead;

use crate::client::forward_offer;
use crate::config::ProxyConfig;
use crate::error::ProxyError;

/// 本プラグインがパスインターセプトの対象とするリクエストパス。
pub const OFFER_PATH: &str = "/rtc/offer";

/// [`try_handle_rtc_offer`] が返す完結済み HTTP レスポンス。
///
/// ソケットへの実書き込みは呼び出し元（コア接続ループ側）の責務とし、本構造体は
/// ステータス・`Content-Type`・body のみを保持する軽量な中間表現とする。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP ステータスコード。
    pub status: u16,
    /// ステータスの reason phrase（例: `"OK"`）。
    pub reason: &'static str,
    /// `Content-Type` ヘッダ値。
    pub content_type: &'static str,
    /// レスポンス body。
    pub body: Vec<u8>,
}

impl Response {
    /// 上流内部情報を含まない定型のエラー応答を作る。
    ///
    /// body には reason phrase のみを載せ、上流アドレス・生エラーメッセージ等
    /// の内部情報を漏らさない（.claude/rules/security.md のエラー情報漏えい対策）。
    fn error(status: u16, reason: &'static str) -> Self {
        Response {
            status,
            reason,
            content_type: "text/plain; charset=utf-8",
            body: reason.as_bytes().to_vec(),
        }
    }
}

/// `POST /rtc/offer` をパスインターセプトし、SDP Offer を上流 WebRTC サービスへ
/// 中継して SDP Answer を返す。
///
/// - メソッド・パスが対象外なら `None` を返す（呼び出し元は次のハンドラへフォール
///   スルーする契約。TASK-8.1 と同型）
/// - `Content-Length` が欠如・`body` の実長と不一致・`config.max_offer_bytes()`
///   超過のいずれかであれば `400`/`413` で拒否する（フェイルクローズ）
/// - 上流中継は [`forward_offer`] に委譲し、[`ProxyError`] の種別に応じて
///   `502`（Bad Gateway）/`504`（Gateway Timeout）へ丸める。上流の内部情報
///   （アドレス・生エラーメッセージ・SDP 本文）はクライアント応答・ログに含めない
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
/// use fandhe_backend_plugin_webrtc_proxy::{ProxyConfig, try_handle_rtc_offer};
///
/// let buf = b"GET /health HTTP/1.1\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// let config = ProxyConfig::new("127.0.0.1:9000");
///
/// // 対象外パスなので None（無関係パスへの性能影響ゼロ）。
/// let runtime = tokio::runtime::Runtime::new().unwrap();
/// let result = runtime.block_on(try_handle_rtc_offer(&head, b"", &config));
/// assert!(result.is_none());
/// ```
pub async fn try_handle_rtc_offer(
    head: &RequestHead,
    body: &[u8],
    config: &ProxyConfig,
) -> Option<Response> {
    if head.method != "POST" || head.target != OFFER_PATH {
        return None;
    }

    let declared_len = match head
        .header("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(len) => len,
        None => return Some(Response::error(400, "Bad Request")),
    };
    if declared_len != body.len() {
        return Some(Response::error(400, "Bad Request"));
    }
    if body.len() > config.max_offer_bytes() {
        return Some(Response::error(413, "Payload Too Large"));
    }

    match forward_offer(config, body).await {
        Ok(answer) => Some(Response {
            status: 200,
            reason: "OK",
            content_type: "application/sdp",
            body: answer,
        }),
        Err(ProxyError::UpstreamTimeout) => Some(Response::error(504, "Gateway Timeout")),
        Err(_) => Some(Response::error(502, "Bad Gateway")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    fn head_from(buf: &[u8]) -> RequestHead {
        match parse_request_head(buf).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("expected Complete"),
        }
    }

    #[tokio::test]
    async fn path_mismatch_returns_none() {
        let head = head_from(b"POST /other HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
        let config = ProxyConfig::new("127.0.0.1:9000");
        assert!(try_handle_rtc_offer(&head, b"", &config).await.is_none());
    }

    #[tokio::test]
    async fn method_mismatch_returns_none() {
        let head = head_from(b"GET /rtc/offer HTTP/1.1\r\n\r\n");
        let config = ProxyConfig::new("127.0.0.1:9000");
        assert!(try_handle_rtc_offer(&head, b"", &config).await.is_none());
    }

    #[tokio::test]
    async fn missing_content_length_is_rejected() {
        let head = head_from(b"POST /rtc/offer HTTP/1.1\r\n\r\n");
        let config = ProxyConfig::new("127.0.0.1:9000");
        let response = try_handle_rtc_offer(&head, b"", &config).await.unwrap();
        // ステータス・reason・Content-Type・body の全件を検証する（PoC-9 教訓:
        // ステータスコードのみの検証はクライアントが実際に受け取る内容の
        // 一部しか見ておらず、reason/Content-Type/body の劣化を見逃す）。
        assert_eq!(response.status, 400);
        assert_eq!(response.reason, "Bad Request");
        assert_eq!(response.content_type, "text/plain; charset=utf-8");
        assert_eq!(response.body, b"Bad Request");
    }

    #[tokio::test]
    async fn content_length_mismatch_is_rejected() {
        let head = head_from(b"POST /rtc/offer HTTP/1.1\r\nContent-Length: 10\r\n\r\n");
        let config = ProxyConfig::new("127.0.0.1:9000");
        let response = try_handle_rtc_offer(&head, b"abc", &config).await.unwrap();
        assert_eq!(response.status, 400);
        assert_eq!(response.reason, "Bad Request");
        assert_eq!(response.content_type, "text/plain; charset=utf-8");
        assert_eq!(response.body, b"Bad Request");
    }

    #[tokio::test]
    async fn non_numeric_content_length_is_rejected() {
        // `Content-Length` が数値としてパース不能な場合も 400 で拒否する境界。
        let head = head_from(b"POST /rtc/offer HTTP/1.1\r\nContent-Length: abc\r\n\r\n");
        let config = ProxyConfig::new("127.0.0.1:9000");
        let response = try_handle_rtc_offer(&head, b"x", &config).await.unwrap();
        assert_eq!(response.status, 400);
    }

    #[tokio::test]
    async fn oversized_offer_is_rejected() {
        let body = vec![b'a'; 16];
        let head = head_from(b"POST /rtc/offer HTTP/1.1\r\nContent-Length: 16\r\n\r\n");
        let config = ProxyConfig::new("127.0.0.1:9000").with_max_offer_bytes(8);
        let response = try_handle_rtc_offer(&head, &body, &config).await.unwrap();
        assert_eq!(response.status, 413);
        assert_eq!(response.reason, "Payload Too Large");
        assert_eq!(response.content_type, "text/plain; charset=utf-8");
        assert_eq!(response.body, b"Payload Too Large");
    }

    #[tokio::test]
    async fn offer_at_exact_max_bytes_is_forwarded() {
        // 上限ちょうど（8 バイト）は拒否されず、実際に上流へ転送を試みる境界。
        // 上流未起動のため 502/504 のいずれかになるが、413（拒否）にはならない
        // ことでサイズ判定の境界（`>` であり `>=` でない）を固定する。
        let body = vec![b'a'; 8];
        let head = head_from(b"POST /rtc/offer HTTP/1.1\r\nContent-Length: 8\r\n\r\n");
        let config = ProxyConfig::new("127.0.0.1:9000")
            .with_max_offer_bytes(8)
            .with_connect_timeout(std::time::Duration::from_millis(200));
        let response = try_handle_rtc_offer(&head, &body, &config).await.unwrap();
        assert_ne!(response.status, 413);
    }

    #[tokio::test]
    async fn upstream_connect_failure_maps_to_502_or_504() {
        // 127.0.0.1:1 は通常未リッスンのため接続失敗（環境依存でタイムアウトも許容）。
        let head = head_from(b"POST /rtc/offer HTTP/1.1\r\nContent-Length: 3\r\n\r\n");
        let config = ProxyConfig::new("127.0.0.1:1")
            .with_connect_timeout(std::time::Duration::from_millis(200));
        let response = try_handle_rtc_offer(&head, b"abc", &config).await.unwrap();
        assert!(response.status == 502 || response.status == 504);
    }
}
