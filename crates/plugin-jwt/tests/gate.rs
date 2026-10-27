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

/// ゲートが `RequestExtensions` に入れた検証結果から `sub` を取り出して本文に返す。
/// ゲートを通っていない要求は `NotAdmitted` を本文に出す（フェイルクローズの確認用）。
struct WhoAmI;

impl Handler for WhoAmI {
    fn handle(&self, head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        let body = match JwtGate::<NoExtraClaims>::verified(head) {
            Ok(c) => format!("sub={}", c.sub().unwrap_or_default()),
            Err(e) => format!("ERR:{e}"),
        };
        Box::pin(std::future::ready(
            Response::new(200, body.into_bytes()).with_content_type("text/plain"),
        ))
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

/// レスポンス全体（ステータス行・ヘッダ・本文）を検証する。ヘッダは名前の一致で
/// 1 本ずつ取り出し、`WWW-Authenticate` の欠落・重複・値の変更を検出する。
fn split(r: &str) -> (Vec<&str>, &str) {
    let (head, body) = r.split_once("\r\n\r\n").expect("head/body separator");
    (head.split("\r\n").collect(), body)
}

fn header_values<'a>(lines: &[&'a str], name: &str) -> Vec<&'a str> {
    lines[1..]
        .iter()
        .filter_map(|l| l.split_once(": "))
        .filter(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
        .collect()
}

fn assert_401(r: &str, challenge: &str) {
    let (lines, body) = split(r);
    assert_eq!(lines[0], "HTTP/1.1 401 Unauthorized", "{r}");
    assert_eq!(
        header_values(&lines, "WWW-Authenticate"),
        vec![challenge],
        "{r}"
    );
    assert_eq!(
        header_values(&lines, "Content-Type"),
        vec!["application/json"],
        "{r}"
    );
    assert_eq!(body, r#"{"error":"invalid_token"}"#, "{r}");
    assert_eq!(
        header_values(&lines, "Content-Length"),
        vec![body.len().to_string()],
        "{r}"
    );
}

type Log = Arc<Mutex<Vec<String>>>;

fn setup(ec: &Ec) -> (Server, Log) {
    let log: Log = Arc::default();
    let l = Arc::clone(&log);
    let auth =
        JwtAuthenticator::<NoExtraClaims>::builder(SharedKeySet::new(ec.keyset("k")), policy())
            .unknown_key_notifier(move |e: &UnknownKeyEvent<'_>| {
                l.lock().unwrap().push(e.kid().to_owned());
            })
            .unknown_key_min_interval(Duration::ZERO)
            .build();
    let server = Server::new().gate(JwtGate::new(auth)).handler(WhoAmI);
    (server, log)
}

#[tokio::test]
async fn valid_token_reaches_handler_and_claims_are_shared() {
    let ec = Ec::new();
    let (server, _) = setup(&ec);
    let t = ec.token("k", now() + 300);
    let r = roundtrip(&server, &req(Some(&format!("Bearer {t}")))).await;
    let (lines, body) = split(&r);
    assert_eq!(lines[0], "HTTP/1.1 200 OK", "{r}");
    assert!(header_values(&lines, "WWW-Authenticate").is_empty(), "{r}");
    assert_eq!(header_values(&lines, "Content-Length"), vec!["10"], "{r}");
    assert_eq!(
        header_values(&lines, "Content-Type"),
        vec!["text/plain"],
        "{r}"
    );
    assert_eq!(body, "sub=user-1", "{r}");
}

#[tokio::test]
async fn missing_token_is_401_with_bare_challenge() {
    let ec = Ec::new();
    let (server, _) = setup(&ec);
    let r = roundtrip(&server, &req(None)).await;
    assert_401(&r, "Bearer");
    assert!(!r.contains("sub="), "{r}");
}

#[tokio::test]
async fn bad_signature_and_expired_are_401_invalid_token() {
    let ec = Ec::new();
    let other = Ec::new();
    let (server, _) = setup(&ec);
    for t in [other.token("k", now() + 300), ec.token("k", now() - 3600)] {
        let r = roundtrip(&server, &req(Some(&format!("Bearer {t}")))).await;
        assert_401(&r, "Bearer error=\"invalid_token\"");
        assert!(!r.contains("sub="), "{r}");
    }
}

#[tokio::test]
async fn unknown_kid_is_rejected_and_notified() {
    let ec = Ec::new();
    let (server, log) = setup(&ec);
    let t = ec.token("rotated", now() + 300);
    let r = roundtrip(&server, &req(Some(&format!("Bearer {t}")))).await;
    assert_401(&r, "Bearer error=\"invalid_token\"");
    assert!(!r.contains("rotated"), "{r}");
    assert_eq!(*log.lock().unwrap(), vec!["rotated".to_owned()]);
}

#[tokio::test]
async fn non_bearer_scheme_is_401() {
    let ec = Ec::new();
    let (server, _) = setup(&ec);
    let r = roundtrip(&server, &req(Some("Basic dXNlcjpwYXNz"))).await;
    assert_401(&r, "Bearer");
}

/// `dpop` feature の有無によらず、DPoP を設定していない認証器は `Authorization: DPoP` を
/// 資格情報なしとして扱い、従来どおり Bearer のチャレンジを返す（#904）。
#[tokio::test]
async fn dpop_scheme_is_not_a_credential_without_dpop_config() {
    let ec = Ec::new();
    let (server, _) = setup(&ec);
    let t = ec.token("k", now() + 300);
    let r = roundtrip(&server, &req(Some(&format!("DPoP {t}")))).await;
    assert_401(&r, "Bearer");
}

/// ゲートを通らない経路（ゲート未登録）のハンドラは、トークンが有効でもクレームを読めない。
#[tokio::test]
async fn handler_without_gate_is_not_admitted() {
    let ec = Ec::new();
    let server = Server::new().handler(WhoAmI);
    let t = ec.token("k", now() + 300);
    let r = roundtrip(&server, &req(Some(&format!("Bearer {t}")))).await;
    let (lines, body) = split(&r);
    assert_eq!(lines[0], "HTTP/1.1 200 OK", "{r}");
    assert_eq!(
        header_values(&lines, "Content-Type"),
        vec!["text/plain"],
        "{r}"
    );
    assert_eq!(
        header_values(&lines, "Content-Length"),
        vec![body.len().to_string()],
        "{r}"
    );
    assert_eq!(body, format!("ERR:{NotAdmitted}"), "{r}");
}

/// 同じ追加クレーム型のゲートを二重に登録すると、2 つ目が既存の値を上書きせず 500 で拒否する。
#[tokio::test]
async fn duplicate_gate_of_same_type_fails_closed() {
    let ec = Ec::new();
    let mk = || {
        JwtGate::new(
            JwtAuthenticator::<NoExtraClaims>::builder(SharedKeySet::new(ec.keyset("k")), policy())
                .build(),
        )
    };
    let server = Server::new().gate(mk()).gate(mk()).handler(WhoAmI);
    let t = ec.token("k", now() + 300);
    let r = roundtrip(&server, &req(Some(&format!("Bearer {t}")))).await;
    let (lines, body) = split(&r);
    assert_eq!(lines[0], "HTTP/1.1 500 Internal Server Error", "{r}");
    assert_eq!(
        header_values(&lines, "Content-Type"),
        vec!["application/json"],
        "{r}"
    );
    assert_eq!(body, r#"{"error":"internal_error"}"#, "{r}");
    assert_eq!(
        header_values(&lines, "Content-Length"),
        vec![body.len().to_string()],
        "{r}"
    );
    assert!(!r.contains("sub="), "{r}");
}
