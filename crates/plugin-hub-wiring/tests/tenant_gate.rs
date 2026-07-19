//! `TenantGate` の統合テスト（TASK-9.1 / #61、TASK-9.2 / #62 で RS256 + JWKS 化）。
//!
//! `crates/core/tests/plugin_graphql_boundary.rs` と同型の
//! `tokio::io::duplex` + `handle_connection` パターンでコアループを実駆動し、
//! `Server::gate(TenantGate::new(..))` 登録後に RS256 + JWKS による JWT 検証 →
//! `org_id` 抽出 → フェイルクローズが `RequestGate` 拡張点上で実際に機能する
//! ことを、ユニットテスト（`crate::gate` 内 `#[cfg(test)]`）より高いレイヤで
//! 検証する。鍵は `tests/fixtures/`（テスト専用、本番使用禁止）。

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_hub_wiring::jwks::{JwksKeySet, SharedJwks};
use fandhe_backend_plugin_hub_wiring::{Authenticator, TenantGate, TenantGateConfig};
use ring::rand::SystemRandom;
use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TEST_KID: &str = "test-kid-1";
const ROTATED_KID: &str = "test-kid-2";

fn test_keypair() -> RsaKeyPair {
    RsaKeyPair::from_pkcs8(include_bytes!("fixtures/test-rsa-2048.pk8")).expect("valid pkcs8")
}

fn rotated_keypair() -> RsaKeyPair {
    RsaKeyPair::from_pkcs8(include_bytes!("fixtures/test-rsa-2048-rotated.pk8"))
        .expect("valid pkcs8")
}

fn jwks_json_for(keypair: &RsaKeyPair, kid: &str) -> String {
    let components: RsaPublicKeyComponents<Vec<u8>> =
        RsaPublicKeyComponents::from(keypair.public_key());
    let n_b64 = URL_SAFE_NO_PAD.encode(&components.n);
    let e_b64 = URL_SAFE_NO_PAD.encode(&components.e);
    format!(
        r#"{{"keys":[{{"kty":"RSA","kid":"{kid}","n":"{n_b64}","e":"{e_b64}","use":"sig","alg":"RS256"}}]}}"#
    )
}

fn make_token(
    keypair: &RsaKeyPair,
    kid: &str,
    org_id: Option<&str>,
    exp: u64,
    alg: &str,
) -> String {
    let header = format!(r#"{{"alg":"{alg}","typ":"JWT","kid":"{kid}"}}"#);
    let payload = match org_id {
        Some(org_id) => format!(r#"{{"org_id":"{org_id}","exp":{exp}}}"#),
        None => format!(r#"{{"exp":{exp}}}"#),
    };
    let header_b64 = URL_SAFE_NO_PAD.encode(header);
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
    let signing_input = format!("{header_b64}.{payload_b64}");
    let rng = SystemRandom::new();
    let mut sig = vec![0u8; keypair.public().modulus_len()];
    keypair
        .sign(
            &signature::RSA_PKCS1_SHA256,
            &rng,
            signing_input.as_bytes(),
            &mut sig,
        )
        .expect("signing succeeds");
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
    format!("{header_b64}.{payload_b64}.{sig_b64}")
}

/// `RequestGate` が `Allow` を返した場合にのみ到達する固定 200 応答ハンドラ。
/// `check()` を通過したリクエストのみがここへ到達することの証跡に使う。
struct FixedOkHandler;
impl Handler for FixedOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        Response::new(200, b"ok".to_vec())
    }
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

fn server_with(keypair: &RsaKeyPair) -> Server {
    let config = TenantGateConfig::from_jwks_json(&jwks_json_for(keypair, TEST_KID)).unwrap();
    Server::new()
        .gate(TenantGate::new(config))
        .handler(FixedOkHandler)
}

#[tokio::test]
async fn valid_token_reaches_handler() {
    let keypair = test_keypair();
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server_with(&keypair), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");
    assert!(response.ends_with("ok"), "response: {response}");
}

#[tokio::test]
async fn missing_authorization_is_rejected_before_handler() {
    let keypair = test_keypair();
    let request = b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server_with(&keypair), request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn expired_token_is_rejected_before_handler() {
    let keypair = test_keypair();
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 1, "RS256");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server_with(&keypair), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn alg_none_is_rejected_before_handler() {
    let keypair = test_keypair();
    let header_b64 = URL_SAFE_NO_PAD.encode(format!(r#"{{"alg":"none","kid":"{TEST_KID}"}}"#));
    let payload_b64 = URL_SAFE_NO_PAD.encode(r#"{"org_id":"org-1","exp":9999999999}"#);
    let token = format!("{header_b64}.{payload_b64}.");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server_with(&keypair), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn alg_hs256_downgrade_is_rejected_before_handler() {
    // アルゴリズム混同（HS256 ダウングレード）攻撃の遮断を E2E レイヤでも固定する
    // （.claude/rules/security.md A05）。
    let keypair = test_keypair();
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "HS256");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server_with(&keypair), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn unknown_kid_is_rejected_before_handler() {
    let keypair = test_keypair();
    let token = make_token(
        &keypair,
        "unregistered-kid",
        Some("org-1"),
        9_999_999_999,
        "RS256",
    );
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server_with(&keypair), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn empty_jwks_rejects_all_requests() {
    // JWKS 未注入・空鍵セットはフェイルオープンにせず全リクエスト拒否する
    // （.claude/rules/security.md A01）。
    let keypair = test_keypair();
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let config = TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#).unwrap();
    let server = Server::new()
        .gate(TenantGate::new(config))
        .handler(FixedOkHandler);
    let response = roundtrip(&server, request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn tampered_signature_is_rejected_before_handler() {
    let keypair = test_keypair();
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");
    let mut parts: Vec<&str> = token.split('.').collect();
    let mut sig = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
    sig[0] ^= 0xFF;
    let tampered_sig = URL_SAFE_NO_PAD.encode(sig);
    parts[2] = &tampered_sig;
    let tampered = parts.join(".");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {tampered}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server_with(&keypair), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn missing_org_id_is_rejected_with_403_before_handler() {
    let keypair = test_keypair();
    let token = make_token(&keypair, TEST_KID, None, 9_999_999_999, "RS256");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server_with(&keypair), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 403"), "response: {response}");
}

#[tokio::test]
async fn blank_org_id_is_rejected_with_403_before_handler() {
    let keypair = test_keypair();
    let token = make_token(&keypair, TEST_KID, Some("   "), 9_999_999_999, "RS256");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server_with(&keypair), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 403"), "response: {response}");
}

#[tokio::test]
async fn malformed_bearer_scheme_is_rejected_before_handler() {
    let keypair = test_keypair();
    let request =
        b"GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Basic xyz\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server_with(&keypair), request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn oversized_token_is_rejected_before_handler() {
    let keypair = test_keypair();
    let huge = "a".repeat(9000);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {huge}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server_with(&keypair), request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
}

#[tokio::test]
async fn key_rotation_via_shared_jwks_is_reflected_without_restart() {
    // `SharedJwks::set()` によるローテーション: 旧鍵で署名したトークンは
    // ローテーション後に拒否され、新鍵で署名したトークンが新たに許可される
    // ことを、サーバ再構築なし（同一 `Server`）で確認する
    // （計画 2.2 節: 再起動なしの鍵ローテーションが受け入れ条件）。
    let old_keypair = test_keypair();
    let new_keypair = rotated_keypair();

    let shared = SharedJwks::from_json(&jwks_json_for(&old_keypair, TEST_KID)).unwrap();
    let server = Server::new()
        .gate(TenantGate::new(TenantGateConfig::new(shared.clone())))
        .handler(FixedOkHandler);

    let old_token = make_token(
        &old_keypair,
        TEST_KID,
        Some("org-1"),
        9_999_999_999,
        "RS256",
    );
    let old_request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {old_token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server, old_request.as_bytes()).await;
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "旧鍵ローテーション前は許可されるはず: {response}"
    );

    // 新しい鍵セットへ差し替える（`kid` を変えて両立可能性も確認する）。
    shared.set(JwksKeySet::from_json(&jwks_json_for(&new_keypair, ROTATED_KID)).unwrap());

    let response_after_rotation = roundtrip(&server, old_request.as_bytes()).await;
    assert!(
        response_after_rotation.starts_with("HTTP/1.1 401"),
        "ローテーション後は旧鍵の kid が JWKS 内に存在せず拒否されるはず: {response_after_rotation}"
    );

    let new_token = make_token(
        &new_keypair,
        ROTATED_KID,
        Some("org-1"),
        9_999_999_999,
        "RS256",
    );
    let new_request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {new_token}\r\nConnection: close\r\n\r\n"
    );
    let response_new = roundtrip(&server, new_request.as_bytes()).await;
    assert!(
        response_new.starts_with("HTTP/1.1 200"),
        "ローテーション後は新鍵のトークンが許可されるはず: {response_new}"
    );
}

/// `TenantGateConfig::authenticator()` で取り出した `Authenticator` をハンドラが
/// 保持し、ゲート通過後に `org_id` を再取得するハンドラ（TASK-9.3 / #63 の
/// 主目的: ゲートとハンドラが同一キャッシュを共有し、ハンドラ側の呼び出しは
/// 署名検証を再実行しない = キャッシュヒットになることを固定する）。
struct CachedAuthHandler {
    authenticator: Authenticator,
}

impl Handler for CachedAuthHandler {
    fn handle(&self, head: &RequestHead, _body: &[u8]) -> Response {
        // `check()`（RequestGate）が既に検証を済ませ `Allow` を返した後にのみ
        // ハンドラへ到達するため、ここでの `authenticate` は必ず成功する
        // （フェイルクローズ経路を通過済みのリクエストのみが到達する契約）。
        let claims = self
            .authenticator
            .authenticate(head)
            .expect("gate already allowed this request");
        Response::new(200, claims.org_id.into_bytes())
    }
}

#[tokio::test]
async fn handler_reuses_gate_verification_via_shared_authenticator() {
    let keypair = test_keypair();
    let config = TenantGateConfig::from_jwks_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
    // `config` を `TenantGate::new` へ渡す（消費する）前に `Authenticator` を
    // 取り出しておく（計画 2.3 節の利用手順）。
    let authenticator = config.authenticator();
    let server = Server::new()
        .gate(TenantGate::new(config))
        .handler(CachedAuthHandler {
            authenticator: authenticator.clone(),
        });

    let token = make_token(&keypair, TEST_KID, Some("org-42"), 9_999_999_999, "RS256");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let response = roundtrip(&server, request.as_bytes()).await;

    assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");
    assert!(response.ends_with("org-42"), "response: {response}");
    // ゲート（1 ミス: 実署名検証）→ ハンドラ（1 ヒット: キャッシュ再利用）の
    // 順で呼ばれたことを直接検証する（重複解消の直接証跡）。
    assert_eq!(authenticator.cache_misses(), 1);
    assert_eq!(authenticator.cache_hits(), 1);
}
