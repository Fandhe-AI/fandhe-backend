//! RFC 6455 4.2.1 ハンドシェイク検証・101/400/426 応答の組み立て。
//!
//! `crate::matches` / `crate::handle_upgrade` から呼ばれる純関数群。検証は
//! 許可リスト方式・フェイルクローズとし（`.claude/rules/security.md`）、応答
//! バイト列は固定テンプレート + 導出値（`Sec-WebSocket-Accept`）のみから組み
//! 立てる。外部入力（`Sec-WebSocket-Key` 等）を応答ヘッダへ一切エコーしない
//! ことで、レスポンス分割・ヘッダインジェクション経路を構造的に排除する。

use bf_http::request::RequestHead;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

use crate::config::WebSocketConfig;
use crate::error::WsError;

/// リクエストが `config` の指すアップグレード対象（パス + メソッド +
/// `Upgrade: websocket`）に該当するかを判定する。
///
/// コア側 `UpgradeHandler` アダプタ（`crates/core/src/server.rs`）の
/// `matches` 実装から呼ばれる（`UpgradeHandler::matches` は「委譲判定のみ」の
/// 契約であり、詳細なハンドシェイク検証は行わない。詳細検証は委譲確定後の
/// [`validate`] が担う）。
#[must_use]
pub fn matches(head: &RequestHead, config: &WebSocketConfig) -> bool {
    head.method == "GET"
        && head.target == config.path
        && head
            .header("upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// 検証済みハンドシェイク（`Sec-WebSocket-Accept` 導出済み）。
pub(crate) struct ValidatedHandshake {
    pub(crate) accept_key: String,
}

/// RFC 6455 4.2.1 の要件を検証し、`Sec-WebSocket-Accept` を導出する。
///
/// 検証項目（許可リスト方式・フェイルクローズ）:
/// - `GET` + `HTTP/1.1`
/// - `Upgrade: websocket`（大小無視）
/// - `Connection` ヘッダのカンマ区切りトークンに `upgrade` を含む（大小無視）
/// - `Sec-WebSocket-Version: 13`（違反時は [`WsError::UnsupportedVersion`]。
///   呼び出し元が `426 Upgrade Required` を返す）
/// - `Sec-WebSocket-Key` が存在し、base64 文字集合・24 文字であること
///
/// 上記いずれかに違反した場合は [`WsError::InvalidHandshake`] /
/// [`WsError::UnsupportedVersion`] を返し、呼び出し元が接続を閉じる。
pub(crate) fn validate(head: &RequestHead) -> Result<ValidatedHandshake, WsError> {
    if head.method != "GET" {
        return Err(WsError::InvalidHandshake("method must be GET"));
    }
    if head.version != bf_http::request::HttpVersion::Http11 {
        return Err(WsError::InvalidHandshake("version must be HTTP/1.1"));
    }
    if !head
        .header("upgrade")
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
    {
        return Err(WsError::InvalidHandshake("missing Upgrade: websocket"));
    }
    if !connection_contains_upgrade(head) {
        return Err(WsError::InvalidHandshake(
            "Connection header must contain 'upgrade'",
        ));
    }

    // バージョン不一致は 426（426 応答は Sec-WebSocket-Version: 13 を
    // 付与する契約、呼び出し元 crate::handle_upgrade を参照）で、他の
    // 検証違反（400）とは異なる応答種別のため個別に判定する。
    match head.header("sec-websocket-version") {
        Some("13") => {}
        _ => return Err(WsError::UnsupportedVersion),
    }

    let key = head
        .header("sec-websocket-key")
        .ok_or(WsError::InvalidHandshake("missing Sec-WebSocket-Key"))?;
    if !is_valid_base64_key(key) {
        return Err(WsError::InvalidHandshake("invalid Sec-WebSocket-Key"));
    }

    let accept_key = derive_accept_key(key.as_bytes());
    Ok(ValidatedHandshake { accept_key })
}

/// `Connection` ヘッダのカンマ区切りトークンに `upgrade`（大小無視）が
/// 含まれるかを判定する（例: `Connection: keep-alive, Upgrade`）。
///
/// `Connection` ヘッダは複数出現しうる（例: `keep-alive` と `Upgrade` が別々の
/// ヘッダ行に分かれる正当なハンドシェイクが存在する）ため、`RequestHead::header`
/// （最初の 1 件のみ返す）ではなく [`RequestHead::headers`] で全件を走査する。
/// `bf_http::connection::should_keep_alive` と同じ理由・同じ走査方針。
fn connection_contains_upgrade(head: &RequestHead) -> bool {
    head.headers()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
}

/// `Sec-WebSocket-Key` が RFC 6455 の想定形（base64 エンコードされた 16
/// バイト、24 文字）に妥当かを検証する。
///
/// 完全な base64 デコードは行わず、文字集合・長さのみを検証する軽量
/// チェックにとどめる（`derive_accept_key` は入力バイト列をそのまま SHA-1 に
/// 通すだけで、デコードの成否に依存しないため）。長さを 24 文字ちょうどに
/// 限定するのは、RFC 6455 4.1 が 16 バイトのランダム値を要求しており、
/// 通常のクライアント実装は必ずこの長さで送るため
/// （長さ検証によりリソース枯渇的な極端に長い値も併せて排除する）。
fn is_valid_base64_key(key: &str) -> bool {
    key.len() == 24
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

/// `101 Switching Protocols` 応答をシリアライズする。
///
/// 固定テンプレート + `accept_key`（`derive_accept_key` の base64 出力）のみ
/// から組み立てる。`accept_key` は SHA-1 + base64 の出力であり base64 文字集合
/// （`[A-Za-z0-9+/=]`）以外の文字を含み得ないため、CRLF・ヘッダインジェクション
/// の混入余地はない。
#[must_use]
pub(crate) fn serialize_101(accept_key: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept_key}\r\n\
         \r\n"
    )
    .into_bytes()
}

/// `400 Bad Request` 応答をシリアライズする（ハンドシェイク検証違反）。
#[must_use]
pub(crate) fn serialize_400() -> Vec<u8> {
    b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".to_vec()
}

/// `426 Upgrade Required` 応答をシリアライズする（`Sec-WebSocket-Version`
/// 不一致）。RFC 6455 4.4 に従い `Sec-WebSocket-Version: 13` を明示する。
#[must_use]
pub(crate) fn serialize_426() -> Vec<u8> {
    b"HTTP/1.1 426 Upgrade Required\r\nSec-WebSocket-Version: 13\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_http::request::{ParseOutcome, parse_request_head};

    fn head_from(buf: &[u8]) -> RequestHead {
        match parse_request_head(buf).expect("parse should succeed") {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("expected Complete"),
        }
    }

    fn valid_handshake_head() -> RequestHead {
        head_from(
            b"GET /ws HTTP/1.1\r\n\
              Host: example.com\r\n\
              Upgrade: websocket\r\n\
              Connection: Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Sec-WebSocket-Version: 13\r\n\
              \r\n",
        )
    }

    #[test]
    fn matches_ws_path_and_upgrade_header() {
        let config = WebSocketConfig::default();
        assert!(matches(&valid_handshake_head(), &config));
    }

    #[test]
    fn matches_rejects_other_path() {
        let config = WebSocketConfig::default().with_path("/ws");
        let head = head_from(b"GET /other HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        assert!(!matches(&head, &config));
    }

    #[test]
    fn matches_rejects_missing_upgrade_header() {
        let config = WebSocketConfig::default();
        let head = head_from(b"GET /ws HTTP/1.1\r\n\r\n");
        assert!(!matches(&head, &config));
    }

    /// RFC 6455 4.2.2 の既知ベクタで `Sec-WebSocket-Accept` 導出を固定する。
    #[test]
    fn validate_derives_known_accept_key_vector() {
        let handshake = validate(&valid_handshake_head()).expect("valid handshake");
        assert_eq!(handshake.accept_key, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn validate_accepts_connection_header_with_multiple_tokens() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\n\
              Upgrade: websocket\r\n\
              Connection: keep-alive, Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Sec-WebSocket-Version: 13\r\n\
              \r\n",
        );
        assert!(validate(&head).is_ok());
    }

    #[test]
    fn validate_accepts_upgrade_token_split_across_multiple_connection_headers() {
        // `Connection` ヘッダが複数行に分かれ、`upgrade` トークンが最初の行に
        // 含まれない正当なハンドシェイク（`RequestHead::header` は最初の 1 件
        // しか返さないため、全件走査していないと誤って 400 を返す回帰）。
        let head = head_from(
            b"GET /ws HTTP/1.1\r\n\
              Upgrade: websocket\r\n\
              Connection: keep-alive\r\n\
              Connection: Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Sec-WebSocket-Version: 13\r\n\
              \r\n",
        );
        assert!(validate(&head).is_ok());
    }

    #[test]
    fn validate_rejects_missing_connection_upgrade_token() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\n\
              Upgrade: websocket\r\n\
              Connection: keep-alive\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Sec-WebSocket-Version: 13\r\n\
              \r\n",
        );
        assert!(matches!(validate(&head), Err(WsError::InvalidHandshake(_))));
    }

    #[test]
    fn validate_rejects_missing_key() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\r\n",
        );
        assert!(matches!(validate(&head), Err(WsError::InvalidHandshake(_))));
    }

    #[test]
    fn validate_rejects_malformed_key_length() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: short\r\nSec-WebSocket-Version: 13\r\n\r\n",
        );
        assert!(matches!(validate(&head), Err(WsError::InvalidHandshake(_))));
    }

    #[test]
    fn validate_rejects_unsupported_version() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 8\r\n\r\n",
        );
        assert!(matches!(validate(&head), Err(WsError::UnsupportedVersion)));
    }

    #[test]
    fn validate_rejects_missing_upgrade_header() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        );
        assert!(matches!(validate(&head), Err(WsError::InvalidHandshake(_))));
    }

    #[test]
    fn serialize_101_embeds_accept_key_without_injection_risk() {
        let bytes = serialize_101("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
        assert!(text.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));
        assert!(text.ends_with("\r\n\r\n"));
    }

    #[test]
    fn serialize_400_is_well_formed() {
        let text = String::from_utf8(serialize_400()).unwrap();
        assert!(text.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    }

    #[test]
    fn serialize_426_includes_supported_version_header() {
        let text = String::from_utf8(serialize_426()).unwrap();
        assert!(text.starts_with("HTTP/1.1 426 Upgrade Required\r\n"));
        assert!(text.contains("Sec-WebSocket-Version: 13\r\n"));
    }
}
