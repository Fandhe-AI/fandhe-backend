//! DPoP を設定した `JwtGate` の統合テスト（`Server::gate` 登録 + `handle_connection` の実 HTTP 経路、#904）。
//! 鍵はすべてテスト専用（ES256 は実行時生成、RS256 は `tests/fixtures`）。
#![cfg(feature = "dpop")]

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_jwt::dpop::*;
use fandhe_backend_plugin_jwt::{
    Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtAuthenticator, JwtGate, JwtPolicy, KeySet,
    SharedKeySet, TokenType, VerifyError,
};
use ring::digest::{SHA256, digest};
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
    RsaPublicKeyComponents,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ISS: &str = "https://issuer.example";
const ORIGIN: &str = "https://api.example";
const PATH: &str = "/resource";
const RSA_PK8: &[u8] = include_bytes!("fixtures/test-rsa-2048.pk8");

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// アクセストークンの発行者側の鍵（ES256）。
struct IssuerKey(EcdsaKeyPair);

impl IssuerKey {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let pk8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        Self(
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pk8.as_ref(), &rng).unwrap(),
        )
    }
    fn keyset(&self) -> KeySet {
        let pt = self.0.public_key().as_ref();
        KeySet::from_json(&format!(
            r#"{{"keys":[{{"kty":"EC","kid":"k","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}]}}"#,
            B.encode(&pt[1..33]),
            B.encode(&pt[33..65])
        ))
        .unwrap()
    }
    /// `cnf` は追加クレームの生 JSON（例: `,"cnf":{"jkt":"…"}`）。
    fn token(&self, cnf: &str) -> String {
        let msg = format!(
            "{}.{}",
            B.encode(r#"{"alg":"ES256","kid":"k","typ":"at+jwt"}"#),
            B.encode(format!(
                r#"{{"iss":"{ISS}","aud":"api","exp":{},"sub":"user-1"{cnf}}}"#,
                now() + 300
            ))
        );
        let sig = self.0.sign(&SystemRandom::new(), msg.as_bytes()).unwrap();
        format!("{msg}.{}", B.encode(sig.as_ref()))
    }
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
        Self::Rsa(RsaKeyPair::from_pkcs8(RSA_PK8).unwrap())
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
    fn jkt(&self) -> String {
        jwk_thumbprint(&self.jwk()).unwrap().as_str().to_owned()
    }
    fn cnf(&self) -> String {
        format!(r#","cnf":{{"jkt":"{}"}}"#, self.jkt())
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
    /// `token` に結び付いた proof。`htu` と `htm` は呼び出し側が指定する。
    fn proof(&self, token: &str, jti: &str, htm: &str, htu: &str) -> String {
        let header = format!(
            r#"{{"typ":"dpop+jwt","alg":"{}","jwk":{}}}"#,
            self.alg(),
            self.jwk()
        );
        let ath = B.encode(digest(&SHA256, token.as_bytes()).as_ref());
        let payload = format!(
            r#"{{"jti":"{jti}","htm":"{htm}","htu":"{htu}","iat":{},"ath":"{ath}"}}"#,
            now()
        );
        let msg = format!("{}.{}", B.encode(header), B.encode(payload));
        format!("{msg}.{}", B.encode(self.sign(&msg)))
    }
    fn good_proof(&self, token: &str, jti: &str) -> String {
        self.proof(token, jti, "GET", &format!("{ORIGIN}{PATH}"))
    }
}

/// ゲート通過後にだけ到達する。ゲートと共有した認証器から `sub` を取り出して本文に返す。
struct WhoAmI(JwtAuthenticator);

impl Handler for WhoAmI {
    fn handle(&self, head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        let sub = match self.0.authenticate_after_gate(head) {
            Ok(c) => c.sub().map(str::to_owned).unwrap_or_default(),
            Err(e) => format!("ERR:{e}"),
        };
        Box::pin(std::future::ready(Response::new(
            200,
            format!("sub={sub}").into_bytes(),
        )))
    }
}

fn policy() -> JwtPolicy {
    JwtPolicy::new(
        Issuer::new(ISS).unwrap(),
        Audiences::single("api").unwrap(),
        AllowedAlgorithms::only(Algorithm::Es256),
        TokenType::AccessToken,
    )
}

fn auth(issuer: &IssuerKey, mode: DpopMode) -> JwtAuthenticator {
    let cfg = DpopConfig::new(
        mode,
        PublicOrigin::new(ORIGIN).unwrap(),
        // 挿入順に依存せず `ES256 RS256` の正準順で出ることも確かめる。
        AllowedAlgorithms::new([Algorithm::Rs256, Algorithm::Es256]).unwrap(),
    );
    JwtAuthenticator::builder(SharedKeySet::new(issuer.keyset()), policy())
        .dpop(cfg)
        .build()
}

fn server(auth: &JwtAuthenticator) -> Server {
    let gate = JwtGate::new(auth.clone());
    let handler_auth = gate.authenticator();
    Server::new().gate(gate).handler(WhoAmI(handler_auth))
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

fn req(target: &str, auth: Option<&str>, dpop: &[&str]) -> String {
    let a = auth.map_or(String::new(), |v| format!("Authorization: {v}\r\n"));
    let d: String = dpop.iter().map(|p| format!("DPoP: {p}\r\n")).collect();
    format!("GET {target} HTTP/1.1\r\nHost: x\r\n{a}{d}Connection: close\r\n\r\n")
}

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

/// 401・`WWW-Authenticate` の全行（順序込み）・固定本文を検証する。
fn assert_401(r: &str, challenges: &[&str], error: &str) {
    let (lines, body) = split(r);
    assert_eq!(lines[0], "HTTP/1.1 401 Unauthorized", "{r}");
    assert_eq!(header_values(&lines, "WWW-Authenticate"), challenges, "{r}");
    assert_eq!(
        header_values(&lines, "Content-Type"),
        vec!["application/json"],
        "{r}"
    );
    assert_eq!(body, format!(r#"{{"error":"{error}"}}"#), "{r}");
    assert!(!r.contains("sub="), "{r}");
}

const ALGS: &str = "algs=\"ES256 RS256\"";

fn dpop_err(error: &str) -> String {
    format!("DPoP error=\"{error}\", {ALGS}")
}

fn ok_sub(r: &str) {
    let (lines, body) = split(r);
    assert_eq!(lines[0], "HTTP/1.1 200 OK", "{r}");
    assert_eq!(body, "sub=user-1", "{r}");
}

// ---- 受理 ----

#[tokio::test]
async fn valid_dpop_request_is_admitted_for_es256_and_rs256() {
    let issuer = IssuerKey::new();
    for key in [Key::ec(), Key::rsa()] {
        let a = auth(&issuer, DpopMode::Required);
        let s = server(&a);
        let t = issuer.token(&key.cnf());
        let r = roundtrip(
            &s,
            &req(
                PATH,
                Some(&format!("DPoP {t}")),
                &[&key.good_proof(&t, "j1")],
            ),
        )
        .await;
        ok_sub(&r);
    }
}

#[tokio::test]
async fn query_string_does_not_affect_htu() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let s = server(&auth(&issuer, DpopMode::Required));
    let t = issuer.token(&key.cnf());
    let r = roundtrip(
        &s,
        &req(
            "/resource?x=1&y=2",
            Some(&format!("DPoP {t}")),
            &[&key.good_proof(&t, "j1")],
        ),
    )
    .await;
    ok_sub(&r);
}

// ---- AC1: 4 種の拒否 ----

#[tokio::test]
async fn bound_token_presented_as_bearer_is_rejected_in_both_modes() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let t = issuer.token(&key.cnf());
    for mode in [DpopMode::Required, DpopMode::AllowUnboundBearer] {
        let s = server(&auth(&issuer, mode));
        // proof を添えても、Bearer での提示そのものを拒否する（格下げの防止）。
        let r = roundtrip(
            &s,
            &req(
                PATH,
                Some(&format!("Bearer {t}")),
                &[&key.good_proof(&t, "j1")],
            ),
        )
        .await;
        let dp = dpop_err("invalid_token");
        match mode {
            DpopMode::Required => assert_401(&r, &[&dp], "invalid_token"),
            _ => assert_401(&r, &[&dp, "Bearer"], "invalid_token"),
        }
    }
}

#[tokio::test]
async fn missing_dpop_header_is_rejected() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let s = server(&auth(&issuer, DpopMode::Required));
    let t = issuer.token(&key.cnf());
    let r = roundtrip(&s, &req(PATH, Some(&format!("DPoP {t}")), &[])).await;
    assert_401(&r, &[&dpop_err("invalid_dpop_proof")], "invalid_dpop_proof");
}

#[tokio::test]
async fn duplicate_dpop_header_is_rejected() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let s = server(&auth(&issuer, DpopMode::Required));
    let t = issuer.token(&key.cnf());
    let p1 = key.good_proof(&t, "j1");
    let p2 = key.good_proof(&t, "j2");
    let r = roundtrip(&s, &req(PATH, Some(&format!("DPoP {t}")), &[&p1, &p2])).await;
    assert_401(&r, &[&dpop_err("invalid_dpop_proof")], "invalid_dpop_proof");
    // 拒否された要求の jti は登録されない（片方だけを送れば通る）。
    let r = roundtrip(&s, &req(PATH, Some(&format!("DPoP {t}")), &[&p1])).await;
    ok_sub(&r);
}

#[tokio::test]
async fn thumbprint_mismatch_is_rejected() {
    let issuer = IssuerKey::new();
    let (bound, other) = (Key::ec(), Key::ec());
    let s = server(&auth(&issuer, DpopMode::Required));
    let t = issuer.token(&bound.cnf());
    let r = roundtrip(
        &s,
        &req(
            PATH,
            Some(&format!("DPoP {t}")),
            &[&other.good_proof(&t, "j1")],
        ),
    )
    .await;
    assert_401(&r, &[&dpop_err("invalid_dpop_proof")], "invalid_dpop_proof");
}

// ---- AC2: cnf.jkt のないトークン ----

#[tokio::test]
async fn unbound_token_in_required_mode_is_rejected_for_both_schemes() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let s = server(&auth(&issuer, DpopMode::Required));
    let t = issuer.token("");
    let dp = dpop_err("invalid_token");
    let r = roundtrip(&s, &req(PATH, Some(&format!("Bearer {t}")), &[])).await;
    assert_401(&r, &[&dp], "invalid_token");
    let r = roundtrip(
        &s,
        &req(
            PATH,
            Some(&format!("DPoP {t}")),
            &[&key.good_proof(&t, "j1")],
        ),
    )
    .await;
    assert_401(&r, &[&dp], "invalid_token");
}

#[tokio::test]
async fn unbound_token_in_allow_mode_is_accepted_only_as_bearer() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let s = server(&auth(&issuer, DpopMode::AllowUnboundBearer));
    let t = issuer.token("");
    // Bearer は受理（余分な DPoP ヘッダ 2 本があっても無視する）。
    let r = roundtrip(&s, &req(PATH, Some(&format!("Bearer {t}")), &[])).await;
    ok_sub(&r);
    let r = roundtrip(&s, &req(PATH, Some(&format!("Bearer {t}")), &["x", "y"])).await;
    ok_sub(&r);
    // DPoP スキームは結合がないので拒否。
    let r = roundtrip(
        &s,
        &req(
            PATH,
            Some(&format!("DPoP {t}")),
            &[&key.good_proof(&t, "j1")],
        ),
    )
    .await;
    assert_401(&r, &[&dpop_err("invalid_token"), "Bearer"], "invalid_token");
}

#[tokio::test]
async fn unusable_cnf_is_rejected_as_invalid_token() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let s = server(&auth(&issuer, DpopMode::AllowUnboundBearer));
    for cnf in [
        r#","cnf":{"x5t#S256":"abc"}"#,
        r#","cnf":1"#,
        r#","cnf":null"#,
        r#","cnf":{"jkt":"short"}"#,
        r#","cnf":{"jkt":5}"#,
    ] {
        let t = issuer.token(cnf);
        let r = roundtrip(&s, &req(PATH, Some(&format!("Bearer {t}")), &[])).await;
        assert_401(
            &r,
            &[&format!("DPoP {ALGS}"), "Bearer error=\"invalid_token\""],
            "invalid_token",
        );
        let r = roundtrip(
            &s,
            &req(
                PATH,
                Some(&format!("DPoP {t}")),
                &[&key.good_proof(&t, "j")],
            ),
        )
        .await;
        assert_401(&r, &[&dpop_err("invalid_token"), "Bearer"], "invalid_token");
    }
}

// ---- AC3: 拒否の応答 ----

#[tokio::test]
async fn challenge_without_credentials() {
    let issuer = IssuerKey::new();
    let r = roundtrip(
        &server(&auth(&issuer, DpopMode::Required)),
        &req(PATH, None, &[]),
    )
    .await;
    assert_401(&r, &[&format!("DPoP {ALGS}")], "invalid_token");
    let r = roundtrip(
        &server(&auth(&issuer, DpopMode::AllowUnboundBearer)),
        &req(PATH, Some("Basic dXNlcjpwYXNz"), &[]),
    )
    .await;
    assert_401(&r, &[&format!("DPoP {ALGS}"), "Bearer"], "invalid_token");
}

#[tokio::test]
async fn invalid_access_token_puts_error_on_the_used_scheme_line() {
    let issuer = IssuerKey::new();
    let other = IssuerKey::new();
    let key = Key::ec();
    let forged = other.token(&key.cnf());
    // Required: DPoP の行だけ。
    let s = server(&auth(&issuer, DpopMode::Required));
    let r = roundtrip(
        &s,
        &req(
            PATH,
            Some(&format!("DPoP {forged}")),
            &[&key.good_proof(&forged, "j")],
        ),
    )
    .await;
    assert_401(&r, &[&dpop_err("invalid_token")], "invalid_token");
    // AllowUnboundBearer: 使われたスキームの行にだけ error を付ける。
    let s = server(&auth(&issuer, DpopMode::AllowUnboundBearer));
    let r = roundtrip(&s, &req(PATH, Some(&format!("Bearer {forged}")), &[])).await;
    assert_401(
        &r,
        &[&format!("DPoP {ALGS}"), "Bearer error=\"invalid_token\""],
        "invalid_token",
    );
    let r = roundtrip(
        &s,
        &req(
            PATH,
            Some(&format!("DPoP {forged}")),
            &[&key.good_proof(&forged, "j")],
        ),
    )
    .await;
    assert_401(&r, &[&dpop_err("invalid_token"), "Bearer"], "invalid_token");
}

#[tokio::test]
async fn proof_failures_are_invalid_dpop_proof() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let s = server(&auth(&issuer, DpopMode::Required));
    let t = issuer.token(&key.cnf());
    let a = format!("DPoP {t}");
    let want = dpop_err("invalid_dpop_proof");
    let cases = [
        // メソッド違い
        key.proof(&t, "j1", "POST", &format!("{ORIGIN}{PATH}")),
        // オリジン違い
        key.proof(&t, "j2", "GET", &format!("https://evil.example{PATH}")),
        // パス違い
        key.proof(&t, "j3", "GET", &format!("{ORIGIN}/other")),
        // 別のアクセストークンへの結び付き（ath 不一致）
        key.good_proof("another-token", "j4"),
        // proof でない文字列
        "not-a-proof".to_owned(),
    ];
    for p in cases {
        let r = roundtrip(&s, &req(PATH, Some(&a), &[&p])).await;
        assert_401(&r, &[&want], "invalid_dpop_proof");
    }
}

#[tokio::test]
async fn absolute_form_target_is_rejected() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let s = server(&auth(&issuer, DpopMode::Required));
    let t = issuer.token(&key.cnf());
    let r = roundtrip(
        &s,
        &req(
            "http://api.example/resource",
            Some(&format!("DPoP {t}")),
            &[&key.good_proof(&t, "j1")],
        ),
    )
    .await;
    assert_401(&r, &[&dpop_err("invalid_dpop_proof")], "invalid_dpop_proof");
}

// ---- 再使用・キャッシュ・ハンドラへの受け渡し ----

#[tokio::test]
async fn reused_proof_is_rejected_on_second_connection() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let s = server(&auth(&issuer, DpopMode::Required));
    let t = issuer.token(&key.cnf());
    let a = format!("DPoP {t}");
    let p = key.good_proof(&t, "j1");
    ok_sub(&roundtrip(&s, &req(PATH, Some(&a), &[&p])).await);
    let r = roundtrip(&s, &req(PATH, Some(&a), &[&p])).await;
    assert_401(&r, &[&dpop_err("invalid_dpop_proof")], "invalid_dpop_proof");
}

#[tokio::test]
async fn binding_is_checked_on_cache_hits() {
    let issuer = IssuerKey::new();
    let (key, other) = (Key::ec(), Key::ec());
    let a = auth(&issuer, DpopMode::Required);
    let s = server(&a);
    let t = issuer.token(&key.cnf());
    let hdr = format!("DPoP {t}");
    ok_sub(&roundtrip(&s, &req(PATH, Some(&hdr), &[&key.good_proof(&t, "j1")])).await);
    ok_sub(&roundtrip(&s, &req(PATH, Some(&hdr), &[&key.good_proof(&t, "j2")])).await);
    assert!(a.cache_hits() >= 1);
    // キャッシュにヒットしても、別の鍵の proof は拒否される。
    let hits = a.cache_hits();
    let r = roundtrip(&s, &req(PATH, Some(&hdr), &[&other.good_proof(&t, "j3")])).await;
    assert_401(&r, &[&dpop_err("invalid_dpop_proof")], "invalid_dpop_proof");
    assert!(a.cache_hits() > hits);
}

#[tokio::test]
async fn handler_without_gate_is_fail_closed() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let a = auth(&issuer, DpopMode::Required);
    // ゲートなし: jti が登録されないので、ハンドラ側の再検証は常に拒否になる。
    let s = Server::new().handler(WhoAmI(a.clone()));
    let t = issuer.token(&key.cnf());
    let r = roundtrip(
        &s,
        &req(
            PATH,
            Some(&format!("DPoP {t}")),
            &[&key.good_proof(&t, "j1")],
        ),
    )
    .await;
    let (_, body) = split(&r);
    assert_eq!(
        body, "sub=ERR:この要求は DPoP の検査を通っていません",
        "{r}"
    );
}

#[test]
fn authenticate_after_gate_requires_prior_registration() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let a = auth(&issuer, DpopMode::Required);
    let t = issuer.token(&key.cnf());
    let raw = format!(
        "GET {PATH} HTTP/1.1\r\nAuthorization: DPoP {t}\r\nDPoP: {}\r\n\r\n",
        key.good_proof(&t, "j1")
    );
    let ParseOutcome::Complete { head, .. } = parse_request_head(raw.as_bytes()).unwrap() else {
        panic!()
    };
    let n = now();
    assert_eq!(
        a.authenticate_after_gate_at(&head, n).unwrap_err(),
        VerifyError::Dpop(DpopError::NotAdmitted)
    );
    assert!(a.authenticate_at(&head, n).is_ok());
    assert!(a.authenticate_after_gate_at(&head, n).is_ok());
    // ゲートと同じ呼び出しを繰り返すと jti の再使用になる。
    assert_eq!(
        a.authenticate_at(&head, n).unwrap_err(),
        VerifyError::Dpop(DpopError::Replayed)
    );
}

// ---- DPoP を設定していない認証器は従来どおり ----

#[tokio::test]
async fn authenticator_without_dpop_accepts_bound_token_as_bearer() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let plain = JwtAuthenticator::builder(SharedKeySet::new(issuer.keyset()), policy()).build();
    let gate = JwtGate::new(plain);
    let handler_auth = gate.authenticator();
    let s = Server::new().gate(gate).handler(WhoAmI(handler_auth));
    let t = issuer.token(&key.cnf());
    ok_sub(&roundtrip(&s, &req(PATH, Some(&format!("Bearer {t}")), &[])).await);
    // `DPoP` スキームは資格情報なし扱いで、従来どおり Bearer のチャレンジを返す。
    let r = roundtrip(&s, &req(PATH, Some(&format!("DPoP {t}")), &[])).await;
    let (lines, body) = split(&r);
    assert_eq!(lines[0], "HTTP/1.1 401 Unauthorized", "{r}");
    assert_eq!(header_values(&lines, "WWW-Authenticate"), vec!["Bearer"]);
    assert_eq!(body, r#"{"error":"invalid_token"}"#);
}

#[test]
fn claims_expose_cnf_jkt() {
    let issuer = IssuerKey::new();
    let key = Key::ec();
    let policy = policy();
    let keys = issuer.keyset();
    let c = fandhe_backend_plugin_jwt::verify::<fandhe_backend_plugin_jwt::NoExtraClaims>(
        &issuer.token(&key.cnf()),
        &keys,
        &policy,
        now(),
    )
    .unwrap();
    assert_eq!(c.cnf_jkt(), Some(key.jkt().as_str()));
    assert!(format!("{c:?}").contains("has_cnf: true"));
    let c = fandhe_backend_plugin_jwt::verify::<fandhe_backend_plugin_jwt::NoExtraClaims>(
        &issuer.token(""),
        &keys,
        &policy,
        now(),
    )
    .unwrap();
    assert_eq!(c.cnf_jkt(), None);
}
