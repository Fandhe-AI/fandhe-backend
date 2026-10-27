//! DPoP を設定した `TenantGate` の統合テスト（イシュー #905）。
//!
//! `Server::gate(TenantGate)` + `handle_connection` の実 HTTP 経路で、DPoP 結合トークンと
//! proof の組の受理、`Bearer` への格下げの拒否、`org_id` の取り出しを確かめる。
//! 鍵はすべてテスト専用（アクセストークンは `tests/fixtures` の RSA、proof の鍵は実行時生成の
//! ES256 または rotated の RSA）。
#![cfg(feature = "dpop")]

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_hub_wiring::jwt::dpop::{
    DpopConfig, DpopError, DpopMode, PublicOrigin, jwk_thumbprint,
};
use fandhe_backend_plugin_hub_wiring::jwt::{
    Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtAuthenticator, JwtPolicy, SharedKeySet,
    TokenType, VerifyError,
};
use fandhe_backend_plugin_hub_wiring::{AuthError, Authenticator, HubClaims};
use fandhe_backend_plugin_hub_wiring::{TenantGate, TenantGateConfig};
use ring::digest::{SHA256, digest};
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
    RsaPublicKeyComponents,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ISS: &str = "https://issuer.example";
const AUD: &str = "hub-api";
const ORIGIN: &str = "https://api.example";
const PATH: &str = "/resource";
const KID: &str = "test-kid-1";
const ISSUER_PK8: &[u8] = include_bytes!("fixtures/test-rsa-2048.pk8");
const PROOF_RSA_PK8: &[u8] = include_bytes!("fixtures/test-rsa-2048-rotated.pk8");

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn issuer_pair() -> RsaKeyPair {
    RsaKeyPair::from_pkcs8(ISSUER_PK8).unwrap()
}

fn issuer_jwks() -> String {
    let c = RsaPublicKeyComponents::<Vec<u8>>::from(issuer_pair().public());
    format!(
        r#"{{"keys":[{{"kty":"RSA","kid":"{KID}","n":"{}","e":"{}","use":"sig","alg":"RS256"}}]}}"#,
        B.encode(&c.n),
        B.encode(&c.e)
    )
}

/// アクセストークン（RS256）。`org` が `None` なら `org_id` なし、`cnf` は生 JSON の断片。
fn token(org: Option<&str>, cnf: &str) -> String {
    let org = org.map_or(String::new(), |o| format!(r#","org_id":"{o}""#));
    let msg = format!(
        "{}.{}",
        B.encode(format!(r#"{{"alg":"RS256","typ":"at+jwt","kid":"{KID}"}}"#)),
        B.encode(format!(
            r#"{{"iss":"{ISS}","aud":"{AUD}","exp":{},"sub":"user-1"{org}{cnf}}}"#,
            now() + 300
        ))
    );
    let pair = issuer_pair();
    let mut sig = vec![0u8; pair.public().modulus_len()];
    pair.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        msg.as_bytes(),
        &mut sig,
    )
    .unwrap();
    format!("{msg}.{}", B.encode(sig))
}

/// proof 側の鍵（ES256 または RS256）。
enum Key {
    Ec(EcdsaKeyPair),
    Rsa(RsaKeyPair),
}

impl Key {
    fn ec() -> Self {
        let rng = SystemRandom::new();
        let pk8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        Self::Ec(
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pk8.as_ref(), &rng).unwrap(),
        )
    }
    fn rsa() -> Self {
        Self::Rsa(RsaKeyPair::from_pkcs8(PROOF_RSA_PK8).unwrap())
    }
    fn alg(&self) -> &'static str {
        match self {
            Self::Ec(_) => "ES256",
            Self::Rsa(_) => "RS256",
        }
    }
    fn jwk(&self) -> String {
        match self {
            Self::Ec(kp) => {
                let pt = kp.public_key().as_ref();
                format!(
                    r#"{{"kty":"EC","crv":"P-256","x":"{}","y":"{}"}}"#,
                    B.encode(&pt[1..33]),
                    B.encode(&pt[33..65])
                )
            }
            Self::Rsa(kp) => {
                let c = RsaPublicKeyComponents::<Vec<u8>>::from(kp.public());
                format!(
                    r#"{{"kty":"RSA","n":"{}","e":"{}"}}"#,
                    B.encode(&c.n),
                    B.encode(&c.e)
                )
            }
        }
    }
    fn cnf(&self) -> String {
        let jkt = jwk_thumbprint(&self.jwk()).unwrap();
        format!(r#","cnf":{{"jkt":"{}"}}"#, jkt.as_str())
    }
    fn sign(&self, msg: &str) -> Vec<u8> {
        match self {
            Self::Ec(kp) => kp
                .sign(&SystemRandom::new(), msg.as_bytes())
                .unwrap()
                .as_ref()
                .to_vec(),
            Self::Rsa(kp) => {
                let mut sig = vec![0u8; kp.public().modulus_len()];
                kp.sign(
                    &RSA_PKCS1_SHA256,
                    &SystemRandom::new(),
                    msg.as_bytes(),
                    &mut sig,
                )
                .unwrap();
                sig
            }
        }
    }
    /// `token` に結び付いた、`GET {ORIGIN}{PATH}` 向けの proof。
    fn proof(&self, token: &str, jti: &str) -> String {
        let header = format!(
            r#"{{"typ":"dpop+jwt","alg":"{}","jwk":{}}}"#,
            self.alg(),
            self.jwk()
        );
        let ath = B.encode(digest(&SHA256, token.as_bytes()).as_ref());
        let payload = format!(
            r#"{{"jti":"{jti}","htm":"GET","htu":"{ORIGIN}{PATH}","iat":{},"ath":"{ath}"}}"#,
            now()
        );
        let msg = format!("{}.{}", B.encode(header), B.encode(payload));
        format!("{msg}.{}", B.encode(self.sign(&msg)))
    }
}

/// ゲートが載せた検証結果から `org_id` を取り出して本文に返す。通っていなければ `ERR:`。
struct OrgHandler;

impl Handler for OrgHandler {
    fn handle(&self, head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        let body = match TenantGate::verified(head) {
            Ok(t) => format!("org={}", t.org_id()),
            Err(e) => format!("ERR:{e}"),
        };
        Box::pin(std::future::ready(
            Response::new(200, body.into_bytes()).with_content_type("text/plain"),
        ))
    }
}

fn policy() -> JwtPolicy {
    JwtPolicy::new(
        Issuer::new(ISS).unwrap(),
        Audiences::single(AUD).unwrap(),
        AllowedAlgorithms::only(Algorithm::Rs256),
        TokenType::AccessToken,
    )
}

fn dpop_config(mode: DpopMode) -> DpopConfig {
    DpopConfig::new(
        mode,
        PublicOrigin::new(ORIGIN).unwrap(),
        AllowedAlgorithms::new([Algorithm::Rs256, Algorithm::Es256]).unwrap(),
    )
}

/// DPoP を設定した認証器。ハンドラ側の再検証の確認用に clone を取り出せる形で返す。
fn authenticator(mode: DpopMode) -> Authenticator {
    let jwt = JwtAuthenticator::<HubClaims>::builder(
        SharedKeySet::from_json(&issuer_jwks()).unwrap(),
        policy(),
    )
    .dpop(dpop_config(mode))
    .build();
    Authenticator::from_jwt(jwt)
}

fn server(authenticator: Authenticator) -> Server {
    Server::new()
        .gate(TenantGate::new(TenantGateConfig::from_authenticator(
            authenticator,
        )))
        .handler(OrgHandler)
}

async fn roundtrip(server: &Server, request: &str) -> String {
    let (mut client, server_stream) = tokio::io::duplex(64 * 1024);
    client.write_all(request.as_bytes()).await.unwrap();
    client.shutdown().await.unwrap();
    handle_connection(server, server_stream).await;
    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

fn req(auth: Option<&str>, dpop: &[&str]) -> String {
    let a = auth.map_or(String::new(), |v| format!("Authorization: {v}\r\n"));
    let d: String = dpop.iter().map(|p| format!("DPoP: {p}\r\n")).collect();
    format!("GET {PATH} HTTP/1.1\r\nHost: x\r\n{a}{d}Connection: close\r\n\r\n")
}

fn www_authenticate(r: &str) -> Vec<&str> {
    let (head, _) = r.split_once("\r\n\r\n").unwrap();
    head.split("\r\n")
        .skip(1)
        .filter_map(|l| l.split_once(": "))
        .filter(|(n, _)| n.eq_ignore_ascii_case("www-authenticate"))
        .map(|(_, v)| v)
        .collect()
}

fn body(r: &str) -> &str {
    r.split_once("\r\n\r\n").unwrap().1
}

/// ステータス行・`Content-Type`・`Content-Length`・本文をすべて検証する
/// （AGENTS.md「アサーション網羅性」）。
fn assert_wire(r: &str, status: u16, content_type: &str, expected_body: &str) {
    let (head, body) = r.split_once("\r\n\r\n").expect("ヘッダ終端がない");
    assert!(head.starts_with(&format!("HTTP/1.1 {status} ")), "{r}");
    assert!(
        head.contains(&format!("Content-Type: {content_type}\r\n")),
        "{r}"
    );
    assert!(
        head.contains(&format!("Content-Length: {}\r\n", expected_body.len())),
        "{r}"
    );
    assert_eq!(body, expected_body, "{r}");
}

const ALGS: &str = "algs=\"ES256 RS256\"";

fn assert_401(r: &str, challenges: &[String], error: &str) {
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert_eq!(www_authenticate(r), challenges, "{r}");
    assert_eq!(body(r), format!(r#"{{"error":"{error}"}}"#), "{r}");
    // ハンドラへは到達していない。
    assert!(!r.contains("org="), "{r}");
}

// ---- 受理（R2） ----

#[tokio::test]
async fn bound_token_with_valid_proof_is_admitted_and_org_id_is_readable() {
    for mode in [DpopMode::Required, DpopMode::AllowUnboundBearer] {
        for key in [Key::ec(), Key::rsa()] {
            let s = server(authenticator(mode));
            let t = token(Some(" org-1 "), &key.cnf());
            let r = roundtrip(
                &s,
                &req(Some(&format!("DPoP {t}")), &[&key.proof(&t, "j1")]),
            )
            .await;
            assert_wire(&r, 200, "text/plain", "org=org-1");
            assert!(www_authenticate(&r).is_empty(), "{r}");
        }
    }
}

// ---- Bearer への格下げ（R3） ----

#[tokio::test]
async fn bound_token_presented_as_bearer_is_rejected() {
    let key = Key::ec();
    let t = token(Some("org-1"), &key.cnf());
    let bearer = format!("Bearer {t}");

    let s = server(authenticator(DpopMode::Required));
    let r = roundtrip(&s, &req(Some(&bearer), &[])).await;
    assert_401(
        &r,
        &[format!("DPoP error=\"invalid_token\", {ALGS}")],
        "invalid_token",
    );

    // Bearer を併用する方式でも、結合トークンは Bearer では受け付けない。
    let s = server(authenticator(DpopMode::AllowUnboundBearer));
    let r = roundtrip(&s, &req(Some(&bearer), &[])).await;
    assert_wire(&r, 401, "application/json", r#"{"error":"invalid_token"}"#);
    assert_eq!(www_authenticate(&r).len(), 2, "{r}");
    assert!(!r.contains("org="), "{r}");
}

// ---- proof の欠陥 ----

#[tokio::test]
async fn missing_duplicate_or_mismatched_proof_is_rejected() {
    let key = Key::ec();
    let t = token(Some("org-1"), &key.cnf());
    let auth = format!("DPoP {t}");
    let expected = [format!("DPoP error=\"invalid_dpop_proof\", {ALGS}")];

    let s = server(authenticator(DpopMode::Required));
    // proof なし
    let r = roundtrip(&s, &req(Some(&auth), &[])).await;
    assert_401(&r, &expected, "invalid_dpop_proof");
    // proof が 2 つ
    let r = roundtrip(
        &s,
        &req(Some(&auth), &[&key.proof(&t, "a"), &key.proof(&t, "b")]),
    )
    .await;
    assert_401(&r, &expected, "invalid_dpop_proof");
    // cnf.jkt と proof の鍵が一致しない
    let other = Key::ec();
    let r = roundtrip(&s, &req(Some(&auth), &[&other.proof(&t, "c")])).await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert_eq!(www_authenticate(&r).len(), 1, "{r}");
    assert!(!r.contains("org="), "{r}");
}

// ---- 順序: 403 は DPoP 検査の後 ----

#[tokio::test]
async fn missing_org_id_after_valid_dpop_is_403_without_challenge() {
    let key = Key::ec();
    for org in [None, Some("   ")] {
        let s = server(authenticator(DpopMode::Required));
        let t = token(org, &key.cnf());
        let r = roundtrip(
            &s,
            &req(Some(&format!("DPoP {t}")), &[&key.proof(&t, "j-403")]),
        )
        .await;
        assert_wire(
            &r,
            403,
            "application/json",
            r#"{"error":"tenant_scope_required"}"#,
        );
        assert!(www_authenticate(&r).is_empty(), "{r}");
    }
    // DPoP の検査に落ちる要求は、org_id がなくても 401（403 にはならない）。
    let s = server(authenticator(DpopMode::Required));
    let t = token(None, &key.cnf());
    let r = roundtrip(&s, &req(Some(&format!("DPoP {t}")), &[])).await;
    assert_wire(
        &r,
        401,
        "application/json",
        r#"{"error":"invalid_dpop_proof"}"#,
    );
}

// ---- jti の再使用 ----

#[tokio::test]
async fn reused_proof_is_rejected_on_a_second_connection() {
    let key = Key::ec();
    let s = server(authenticator(DpopMode::Required));
    let t = token(Some("org-1"), &key.cnf());
    let request = req(Some(&format!("DPoP {t}")), &[&key.proof(&t, "same")]);
    let first = roundtrip(&s, &request).await;
    assert_wire(&first, 200, "text/plain", "org=org-1");
    let second = roundtrip(&s, &request).await;
    assert_wire(
        &second,
        401,
        "application/json",
        r#"{"error":"invalid_dpop_proof"}"#,
    );
}

// ---- 資格情報なし ----

#[tokio::test]
async fn no_credentials_challenge_lists_schemes_per_mode() {
    let s = server(authenticator(DpopMode::Required));
    let r = roundtrip(&s, &req(None, &[])).await;
    assert_401(&r, &[format!("DPoP {ALGS}")], "invalid_token");

    let s = server(authenticator(DpopMode::AllowUnboundBearer));
    let r = roundtrip(&s, &req(None, &[])).await;
    assert_401(
        &r,
        &[format!("DPoP {ALGS}"), "Bearer".to_owned()],
        "invalid_token",
    );
}

// ---- 互換 ----

#[tokio::test]
async fn unbound_bearer_is_accepted_when_bearer_is_allowed() {
    let s = server(authenticator(DpopMode::AllowUnboundBearer));
    let t = token(Some("org-9"), "");
    let r = roundtrip(&s, &req(Some(&format!("Bearer {t}")), &[])).await;
    assert_wire(&r, 200, "text/plain", "org=org-9");
}

#[tokio::test]
async fn feature_enabled_without_dpop_config_keeps_legacy_wire() {
    let key = Key::ec();
    let config = TenantGateConfig::from_jwks_json(&issuer_jwks(), policy()).unwrap();
    let s = Server::new()
        .gate(TenantGate::new(config))
        .handler(OrgHandler);
    // 結合トークンも Bearer として受理される（DPoP を設定していない下流の従来動作）。
    let t = token(Some("org-1"), &key.cnf());
    let r = roundtrip(&s, &req(Some(&format!("Bearer {t}")), &[])).await;
    assert_wire(&r, 200, "text/plain", "org=org-1");
    // 拒否の 401 に WWW-Authenticate は付かない。
    let r = roundtrip(&s, &req(None, &[])).await;
    assert_wire(&r, 401, "application/json", r#"{"error":"invalid_token"}"#);
    assert!(www_authenticate(&r).is_empty(), "{r}");
}

// ---- 1 要求 1 回の契約 ----

#[test]
fn authenticating_the_same_request_twice_fails_with_replayed() {
    let key = Key::ec();
    let a = authenticator(DpopMode::Required);
    let t = token(Some("org-1"), &key.cnf());
    let raw = req(Some(&format!("DPoP {t}")), &[&key.proof(&t, "once")]);
    let ParseOutcome::Complete { head, .. } = parse_request_head(raw.as_bytes()).unwrap() else {
        unreachable!()
    };
    assert!(a.authenticate(&head).is_ok());
    // ハンドラが再度 `authenticate` を呼ぶと jti の再使用になる。
    // `TenantGate::verified` を使う理由。
    assert_eq!(
        a.authenticate(&head).err(),
        Some(AuthError::Jwt(VerifyError::Dpop(DpopError::Replayed)))
    );
}
