//! 越境アクセス監査ログ標準整合（TASK-9.6 / #89）の統合テスト。
//!
//! `tests/tenant_gate.rs` と同型の `tokio::io::duplex` + `handle_connection`
//! パターンでコアループを実駆動し、`Server::gate(TenantGate)` による 401/403
//! フェイルクローズを通過した後段で、テナントスコープ付きインメモリストアが
//! `TenantLookupOutcome`（`src/audit.rs`）を用いて「正当な 404」と
//! 「越境 404」を判定し、外部応答（HTTP 404）は完全同一のまま監査ログ
//! （`MemoryAuditSink`）のみで区別できることを検証する
//! （受け入れ条件 1・2、docs/design/outbox-consent-integration.md 9 節）。

use backend_framework_core::{Handler, Server, handle_connection};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bf_http::request::RequestHead;
use bf_http::response::Response;
use bf_plugin_hub_wiring::audit::{AuditContext, AuditSink, MemoryAuditSink, TenantLookupOutcome};
use bf_plugin_hub_wiring::jwks::SharedJwks;
use bf_plugin_hub_wiring::jwt::verify_token;
use bf_plugin_hub_wiring::{TenantGate, TenantGateConfig};
use ring::rand::SystemRandom;
use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TEST_KID: &str = "test-kid-1";

fn test_keypair() -> RsaKeyPair {
    RsaKeyPair::from_pkcs8(include_bytes!("fixtures/test-rsa-2048.pk8")).expect("valid pkcs8")
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

fn make_token(keypair: &RsaKeyPair, kid: &str, org_id: &str, exp: u64) -> String {
    let header = format!(r#"{{"alg":"RS256","typ":"JWT","kid":"{kid}"}}"#);
    let payload = format!(r#"{{"org_id":"{org_id}","exp":{exp}}}"#);
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

/// テナントスコープ付きの単一リソース（`/widgets/1`、所有 `org-a`）を模した
/// インメモリストア。データ層（RLS 相当）が越境行を 0 行として遮断する挙動を
/// `TenantLookupOutcome` で表現する（`docs/design/outbox-consent-integration.md`
/// 6 節の 2 層設計と同型）。
struct TenantScopedStore;

impl TenantScopedStore {
    /// `org-a` が所有する唯一のリソース `/widgets/1` を、呼び出し元
    /// `caller_org_id` の視点で解決する。
    fn lookup(&self, path: &str, caller_org_id: &str) -> TenantLookupOutcome<&'static str> {
        const OWNER_ORG_ID: &str = "org-a";
        const RESOURCE_PATH: &str = "/widgets/1";

        if path != RESOURCE_PATH {
            return TenantLookupOutcome::NotFound;
        }
        if caller_org_id == OWNER_ORG_ID {
            TenantLookupOutcome::Found("widget-payload")
        } else {
            TenantLookupOutcome::CrossTenantAttempt
        }
    }
}

/// `RequestGate`（`TenantGate`、401/403）を通過したリクエストのみが到達する
/// ハンドラ。ここで `org_id` を再抽出し（`GateOutcome` は許可/拒否のみを運ぶ
/// 契約のため、`crate::gate::TenantGate` の doc 参照）、`TenantScopedStore` へ
/// 委ねてテナントスコープ判定・監査記録・404/200 応答の組み立てを行う。
struct TenantAwareHandler {
    jwks: SharedJwks,
    store: TenantScopedStore,
    sink: Arc<MemoryAuditSink>,
}

const NOT_FOUND_BODY: &[u8] = b"not found";

impl Handler for TenantAwareHandler {
    fn handle(&self, head: &RequestHead, _body: &[u8]) -> Response {
        // `TenantGate` を通過済み（`Authorization: Bearer <valid JWT>`）である
        // 前提だが、`org_id` はゲート層から渡されないため本ハンドラで再検証する。
        let token = head
            .header("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .expect("gate already validated the bearer token");
        let keys = self.jwks.snapshot();
        let claims = verify_token(token, &keys, 0).expect("gate already validated the token");

        let ctx = AuditContext::new(
            claims.org_id.clone(),
            head.method.clone(),
            head.target.as_str(),
            "tenant-aware-handler",
        );
        let path = head
            .target
            .split_once('?')
            .map_or(head.target.as_str(), |(path, _query)| path);

        match self.store.lookup(path, &claims.org_id).resolve(
            self.sink.as_ref() as &dyn AuditSink,
            &ctx,
            0,
        ) {
            Some(payload) => Response::new(200, payload.as_bytes().to_vec()),
            None => Response::new(404, NOT_FOUND_BODY.to_vec()),
        }
    }
}

async fn roundtrip(server: &Server, request: &[u8]) -> String {
    let (mut client, server_stream) = tokio::io::duplex(32 * 1024);
    client.write_all(request).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

fn server_with(keypair: &RsaKeyPair, sink: Arc<MemoryAuditSink>) -> Server {
    let shared = SharedJwks::from_json(&jwks_json_for(keypair, TEST_KID)).unwrap();
    let handler = TenantAwareHandler {
        jwks: shared.clone(),
        store: TenantScopedStore,
        sink,
    };
    Server::new()
        .gate(TenantGate::new(TenantGateConfig::new(shared)))
        .handler(handler)
}

fn request_for(token: &str, path: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    )
}

#[tokio::test]
async fn cross_tenant_attempt_yields_404_and_is_recorded_in_audit_log() {
    let keypair = test_keypair();
    let sink = Arc::new(MemoryAuditSink::new());
    let server = server_with(&keypair, sink.clone());

    // org-b の valid JWT で org-a 所有の `/widgets/1` へアクセス（越境試行）。
    let token = make_token(&keypair, TEST_KID, "org-b", 9_999_999_999);
    let response = roundtrip(&server, request_for(&token, "/widgets/1").as_bytes()).await;

    assert!(response.starts_with("HTTP/1.1 404"), "response: {response}");
    assert_eq!(
        sink.len(),
        1,
        "越境試行は cross_tenant_attempt として 1 件記録されるはず"
    );
    let events = sink.events();
    assert!(events[0].to_json().contains("cross_tenant_attempt"));
}

#[tokio::test]
async fn legitimate_not_found_yields_byte_identical_404_and_records_nothing() {
    let keypair = test_keypair();
    let sink = Arc::new(MemoryAuditSink::new());
    let server = server_with(&keypair, sink.clone());

    // org-a 自身の valid JWT で存在しないリソースへアクセス（正当な 404）。
    let token = make_token(&keypair, TEST_KID, "org-a", 9_999_999_999);
    let response = roundtrip(&server, request_for(&token, "/widgets/999").as_bytes()).await;

    assert!(response.starts_with("HTTP/1.1 404"), "response: {response}");
    assert_eq!(sink.len(), 0, "正当な 404 は監査記録されないはず");
}

#[tokio::test]
async fn cross_tenant_404_and_legitimate_404_are_byte_identical_responses() {
    // 受け入れ条件 1 の直接検証: 「越境 404」と「正当な 404」の外部応答が
    // バイト同一であり、監査ログの記録有無のみが両者を区別することを示す。
    let keypair = test_keypair();

    let cross_tenant_sink = Arc::new(MemoryAuditSink::new());
    let cross_tenant_server = server_with(&keypair, cross_tenant_sink.clone());
    let cross_tenant_token = make_token(&keypair, TEST_KID, "org-b", 9_999_999_999);
    let cross_tenant_response = roundtrip(
        &cross_tenant_server,
        request_for(&cross_tenant_token, "/widgets/1").as_bytes(),
    )
    .await;

    let legitimate_sink = Arc::new(MemoryAuditSink::new());
    let legitimate_server = server_with(&keypair, legitimate_sink.clone());
    let legitimate_token = make_token(&keypair, TEST_KID, "org-a", 9_999_999_999);
    let legitimate_response = roundtrip(
        &legitimate_server,
        request_for(&legitimate_token, "/widgets/999").as_bytes(),
    )
    .await;

    assert_eq!(
        cross_tenant_response, legitimate_response,
        "外部応答は越境 404 と正当な 404 で完全同一のはず"
    );
    assert_eq!(cross_tenant_sink.len(), 1);
    assert_eq!(legitimate_sink.len(), 0);
}

#[tokio::test]
async fn audit_event_json_never_contains_token_or_bearer_content() {
    // 受け入れ条件 2: 監査ログに機密（トークン・Authorization ヘッダ値）を
    // 含めないことを E2E レイヤでも固定する（.claude/rules/security.md）。
    let keypair = test_keypair();
    let sink = Arc::new(MemoryAuditSink::new());
    let server = server_with(&keypair, sink.clone());

    let token = make_token(&keypair, TEST_KID, "org-b", 9_999_999_999);
    let _ = roundtrip(&server, request_for(&token, "/widgets/1").as_bytes()).await;

    assert_eq!(sink.len(), 1);
    let json = sink.events()[0].to_json();
    assert!(!json.contains(&token));
    assert!(!json.contains("Bearer"));
    assert!(!json.contains("Authorization"));
}

#[tokio::test]
async fn gate_still_rejects_before_handler_for_missing_token() {
    // TenantGate（401）は本タスクで変更していないことの回帰確認。
    let keypair = test_keypair();
    let sink = Arc::new(MemoryAuditSink::new());
    let server = server_with(&keypair, sink.clone());

    let request = b"GET /widgets/1 HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;

    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
    assert_eq!(
        sink.len(),
        0,
        "認証失敗は cross_tenant_attempt ではないため記録されない"
    );
}
