//! `JwtGate` の統合テスト（`Server::gate` 登録 + `handle_connection` の実 HTTP 経路、#816）。
//! `crates/plugin-hub-wiring/tests/tenant_gate.rs` と同型の `tokio::io::duplex` 駆動。

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_jwt::*;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ISS: &str = "https://issuer.example";

struct Ec(EcdsaKeyPair);

impl Ec {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let pk8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        Self(
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pk8.as_ref(), &rng).unwrap(),
        )
    }
    fn keyset(&self, kid: &str) -> KeySet {
        let pt = self.0.public_key().as_ref();
        KeySet::from_json(&format!(
            r#"{{"keys":[{{"kty":"EC","kid":"{kid}","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}]}}"#,
            B.encode(&pt[1..33]),
            B.encode(&pt[33..65])
        ))
        .unwrap()
    }
    fn token(&self, kid: &str, exp: u64) -> String {
        let msg = format!(
            "{}.{}",
            B.encode(format!(r#"{{"alg":"ES256","kid":"{kid}","typ":"at+jwt"}}"#)),
            B.encode(format!(
                r#"{{"iss":"{ISS}","aud":"api","exp":{exp},"sub":"user-1"}}"#
            ))
        );
        let sig = self.0.sign(&SystemRandom::new(), msg.as_bytes()).unwrap();
        format!("{msg}.{}", B.encode(sig.as_ref()))
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn policy() -> JwtPolicy {
    JwtPolicy::new(
        Issuer::new(ISS).unwrap(),
        Audiences::single("api").unwrap(),
        AllowedAlgorithms::only(Algorithm::Es256),
        TokenType::AccessToken,
    )
}

/// ゲート通過後にだけ到達する。共有した認証器から `sub` を取り出して本文に返す。
struct WhoAmI(JwtAuthenticator);

impl Handler for WhoAmI {
    fn handle(&self, head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        let sub = self
            .0
            .authenticate(head)
            .ok()
            .and_then(|c| c.sub().map(str::to_owned))
            .unwrap_or_default();
        Box::pin(std::future::ready(Response::new(
            200,
            format!("sub={sub}").into_bytes(),
        )))
    }
}

async fn roundtrip(server: &Server, request: &str) -> String {
    let (mut client, server_stream) = tokio::io::duplex(32 * 1024);
    client.write_all(request.as_bytes()).await.unwrap();
    client.shutdown().await.unwrap();
    handle_connection(server, server_stream).await;
    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

fn req(auth: Option<&str>) -> String {
    let a = auth.map_or(String::new(), |v| format!("Authorization: {v}\r\n"));
    format!("GET / HTTP/1.1\r\nHost: x\r\n{a}Connection: close\r\n\r\n")
}

type Log = Arc<Mutex<Vec<String>>>;

fn setup(ec: &Ec) -> (Server, JwtAuthenticator, Log) {
    let log: Log = Arc::default();
    let l = Arc::clone(&log);
    let auth = JwtAuthenticator::builder(SharedKeySet::new(ec.keyset("k")), policy())
        .unknown_key_notifier(move |e: &UnknownKeyEvent<'_>| {
            l.lock().unwrap().push(e.kid().to_owned());
        })
        .unknown_key_min_interval(Duration::ZERO)
        .build();
    let gate = JwtGate::new(auth);
    let handler_auth = gate.authenticator();
    let server = Server::new()
        .gate(gate)
        .handler(WhoAmI(handler_auth.clone()));
    (server, handler_auth, log)
}

#[tokio::test]
async fn valid_token_reaches_handler_and_claims_are_shared() {
    let ec = Ec::new();
    let (server, auth, _) = setup(&ec);
    let t = ec.token("k", now() + 300);
    let r = roundtrip(&server, &req(Some(&format!("Bearer {t}")))).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(r.ends_with("sub=user-1"), "{r}");
    // ゲートで温まったキャッシュをハンドラが再利用する。
    assert!(auth.cache_hits() >= 1);
}

#[tokio::test]
async fn missing_token_is_401_with_bare_challenge() {
    let ec = Ec::new();
    let (server, _, _) = setup(&ec);
    let r = roundtrip(&server, &req(None)).await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert!(r.contains("WWW-Authenticate: Bearer\r\n"), "{r}");
    assert!(!r.contains("error="), "{r}");
    assert!(r.ends_with(r#"{"error":"invalid_token"}"#), "{r}");
    assert!(!r.contains("sub="), "{r}");
}

#[tokio::test]
async fn bad_signature_and_expired_are_401_invalid_token() {
    let ec = Ec::new();
    let other = Ec::new();
    let (server, _, _) = setup(&ec);
    for t in [other.token("k", now() + 300), ec.token("k", now() - 3600)] {
        let r = roundtrip(&server, &req(Some(&format!("Bearer {t}")))).await;
        assert!(r.starts_with("HTTP/1.1 401"), "{r}");
        assert!(
            r.contains("WWW-Authenticate: Bearer error=\"invalid_token\"\r\n"),
            "{r}"
        );
        assert!(!r.contains("sub="), "{r}");
    }
}

#[tokio::test]
async fn unknown_kid_is_rejected_and_notified() {
    let ec = Ec::new();
    let (server, _, log) = setup(&ec);
    let t = ec.token("rotated", now() + 300);
    let r = roundtrip(&server, &req(Some(&format!("Bearer {t}")))).await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert_eq!(*log.lock().unwrap(), vec!["rotated".to_owned()]);
}

#[tokio::test]
async fn non_bearer_scheme_is_401() {
    let ec = Ec::new();
    let (server, _, _) = setup(&ec);
    let r = roundtrip(&server, &req(Some("Basic dXNlcjpwYXNz"))).await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert!(r.contains("WWW-Authenticate: Bearer\r\n"), "{r}");
}
