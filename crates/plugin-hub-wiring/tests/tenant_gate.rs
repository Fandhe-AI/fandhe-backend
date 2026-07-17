//! `TenantGate` の統合テスト（TASK-9.1 / #61）。
//!
//! `crates/core/tests/plugin_graphql_boundary.rs` と同型の
//! `tokio::io::duplex` + `handle_connection` パターンでコアループを実駆動し、
//! `Server::gate(TenantGate::new(..))` 登録後に JWT 検証 → `org_id` 抽出 →
//! フェイルクローズが `RequestGate` 拡張点上で実際に機能することを、
//! ユニットテスト（`crate::gate` 内 `#[cfg(test)]`）より高いレイヤで検証する。

use backend_framework_core::{Handler, Server, handle_connection};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bf_http::request::RequestHead;
use bf_http::response::Response;
use bf_plugin_hub_wiring::{TenantGate, TenantGateConfig};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TEST_SECRET: &[u8] = b"test-only-dummy-secret-do-not-use-in-prod";

/// `RequestGate` が `Allow` を返した場合にのみ到達する固定 200 応答ハンドラ。
/// `check()` を通過したリクエストのみがここへ到達することの証跡に使う。
struct FixedOkHandler;
impl Handler for FixedOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        Response::new(200, b"ok".to_vec())
    }
}

fn make_token(org_id: Option<&str>, exp: u64, alg: &str, secret: &[u8]) -> String {
    let header = format!(r#"{{"alg":"{alg}","typ":"JWT"}}"#);
    let payload = match org_id {
        Some(org_id) => format!(r#"{{"org_id":"{org_id}","exp":{exp}}}"#),
        None => format!(r#"{{"exp":{exp}}}"#),
    };
    let header_b64 = URL_SAFE_NO_PAD.encode(header);
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
    let signing_input = format!("{header_b64}.{payload_b64}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("any length key");
    mac.update(signing_input.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{header_b64}.{payload_b64}.{sig}")
}

async fn roundtrip(server: &Server, request: &[u8]) -> String {
    // duplex バッファは `oversized_token_is_rejected_before_handler`（トークン長
    // 上限超過ケース、`jwt::MAX_TOKEN_LEN` = 8192 バイト超）のリクエストが単一
    // バッファに収まるサイズを確保する。`client.write_all` は
    // `handle_connection`（読み手）の起動前に呼ぶため、バッファが小さいと
    // `write_all` がバッファ空き待ちで永久にブロックする（読み手不在の自己
    // デッドロック）。`crates/http::request::MAX_HEADER_BYTES`（16KiB）+
    // 予備を確保する。
    let (mut client, server_stream) = tokio::io::duplex(32 * 1024);
    client.write_all(request).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

fn server() -> Server {
    Server::new()
        .gate(TenantGate::new(TenantGateConfig::new(TEST_SECRET.to_vec())))
        .handler(FixedOkHandler)
}

#[tokio::test]
async fn valid_token_reaches_handler() {
    let token = make_token(Some("org-1"), 9_999_999_999, "HS256", TEST_SECRET);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server(), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");
    assert!(response.ends_with("ok"), "response: {response}");
}

#[tokio::test]
async fn missing_authorization_is_rejected_before_handler() {
    let request = b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server(), request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn expired_token_is_rejected_before_handler() {
    let token = make_token(Some("org-1"), 1, "HS256", TEST_SECRET);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server(), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn alg_none_is_rejected_before_handler() {
    let token = make_token(Some("org-1"), 9_999_999_999, "none", TEST_SECRET);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server(), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn tampered_signature_is_rejected_before_handler() {
    let token = make_token(Some("org-1"), 9_999_999_999, "HS256", TEST_SECRET);
    let mut parts: Vec<&str> = token.split('.').collect();
    let mut sig = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
    sig[0] ^= 0xFF;
    let tampered_sig = URL_SAFE_NO_PAD.encode(sig);
    parts[2] = &tampered_sig;
    let tampered = parts.join(".");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {tampered}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server(), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn missing_org_id_is_rejected_with_403_before_handler() {
    let token = make_token(None, 9_999_999_999, "HS256", TEST_SECRET);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server(), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 403"), "response: {response}");
}

#[tokio::test]
async fn blank_org_id_is_rejected_with_403_before_handler() {
    let token = make_token(Some("   "), 9_999_999_999, "HS256", TEST_SECRET);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server(), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 403"), "response: {response}");
}

#[tokio::test]
async fn malformed_bearer_scheme_is_rejected_before_handler() {
    let request =
        b"GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Basic xyz\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server(), request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn oversized_token_is_rejected_before_handler() {
    let huge = "a".repeat(9000);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {huge}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server(), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}
