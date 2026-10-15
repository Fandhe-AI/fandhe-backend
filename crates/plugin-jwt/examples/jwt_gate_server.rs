//! イシュー #821: JWT ゲートの NFR-6 計測専用サーバ。
//!
//! `benches/jwt-nfr6-bench.sh` が、`crates/core/examples/minimal.rs`（ベースライン）と
//! 本 example（リンクのみ／ゲート有効）を同一条件で比較するために使う。`GET /`・`GET /health`
//! の応答は `minimal` と同一で、差分が JWT ゲートのコストだけになるようにしている
//! （`plugin-hub-wiring` の `hub_link_only` と同型）。
//!
//! 環境変数:
//! - `FANDHE_BACKEND_JWT_GATE=off`: ゲートを登録しない（リンクコストのみの構成）
//! - `FANDHE_BACKEND_JWT_BENCH_ALG=es256|rs256`（既定 `es256`）: 書き出すトークンの方式。
//!   未知の値は起動時にエラー終了する（fail-closed）
//! - `FANDHE_BACKEND_JWT_BENCH_TOKEN_FILE`: 指定時、起動時に作った有効トークンをこの
//!   パスへ書く（ゲートの有無によらず書く。全系統に同じ `Authorization` を付けるため）。
//!   トークンは stdout にもログにも出さない
//! - `FANDHE_BACKEND_JWT_BENCH_ADDR`（既定 `127.0.0.1:3103`）: 待受アドレス。3000 は
//!   minimal、3100・3101 は hub の examples、3102 は `bench-accept.sh` の既定で使われており、
//!   3103 はリポジトリ内で未使用のため既定にした
//!
//! 鍵は `tests/fixtures` のテスト専用鍵で、本番使用禁止。example のため本体の依存
//! ツリーへ影響しない（`.claude/rules/pay-for-what-you-use.md`）。
//!
//! ```bash
//! cargo build --release -p fandhe-backend-plugin-jwt --example jwt_gate_server
//! FANDHE_BACKEND_JWT_BENCH_TOKEN_FILE=/tmp/tok ./target/release/examples/jwt_gate_server &
//! curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer $(cat /tmp/tok)" http://127.0.0.1:3103/  # 200
//! curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:3103/                                              # 401
//! ```

use std::env;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_jwt::{
    Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtAuthenticator, JwtGate, JwtPolicy,
    NoExtraClaims, SharedKeySet, TokenType,
};
use fandhe_backend_routes::Router;
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
    RsaPublicKeyComponents,
};

const ISS: &str = "https://bench.example";
const AUD: &str = "bench-api";
const EXP: u64 = 9_999_999_999;

/// テスト・計測専用鍵（PKCS#8 DER）。本番使用禁止（`tests/fixtures/README.md`）。
const RSA_PK8: &[u8] = include_bytes!("../tests/fixtures/test-rsa-2048.pk8");
const EC_PK8: &[u8] = include_bytes!("../tests/fixtures/test-ec-p256.pk8");

fn fail(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, msg.to_string())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let alg = env::var("FANDHE_BACKEND_JWT_BENCH_ALG").unwrap_or_else(|_| "es256".into());
    if alg != "es256" && alg != "rs256" {
        return Err(fail("FANDHE_BACKEND_JWT_BENCH_ALG は es256 か rs256"));
    }

    let rsa = RsaKeyPair::from_pkcs8(RSA_PK8).map_err(|_| fail("invalid rsa fixture"))?;
    let ec = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        EC_PK8,
        &SystemRandom::new(),
    )
    .map_err(|_| fail("invalid ec fixture"))?;

    let rc = RsaPublicKeyComponents::<Vec<u8>>::from(rsa.public());
    let pt = ec.public_key().as_ref();
    let jwks = format!(
        r#"{{"keys":[{{"kty":"RSA","kid":"bench-rs","alg":"RS256","n":"{}","e":"{}"}},{{"kty":"EC","kid":"bench-es","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}]}}"#,
        B.encode(&rc.n),
        B.encode(&rc.e),
        B.encode(&pt[1..33]),
        B.encode(&pt[33..65])
    );

    // 有効トークン（遠い未来の exp）。全系統に同じヘッダを付けて負荷をかけるために使う。
    let payload = format!(r#"{{"iss":"{ISS}","aud":"{AUD}","exp":{EXP}}}"#);
    let token = if alg == "rs256" {
        let header = r#"{"alg":"RS256","typ":"at+jwt","kid":"bench-rs"}"#;
        let msg = format!("{}.{}", B.encode(header), B.encode(&payload));
        let mut sig = vec![0u8; rsa.public().modulus_len()];
        rsa.sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            msg.as_bytes(),
            &mut sig,
        )
        .map_err(|_| fail("rsa sign"))?;
        format!("{msg}.{}", B.encode(sig))
    } else {
        let header = r#"{"alg":"ES256","typ":"at+jwt","kid":"bench-es"}"#;
        let msg = format!("{}.{}", B.encode(header), B.encode(&payload));
        let sig = ec
            .sign(&SystemRandom::new(), msg.as_bytes())
            .map_err(|_| fail("ec sign"))?;
        format!("{msg}.{}", B.encode(sig.as_ref()))
    };
    if let Ok(path) = env::var("FANDHE_BACKEND_JWT_BENCH_TOKEN_FILE") {
        std::fs::write(path, &token)?;
    }

    // `examples/minimal.rs` と同一の応答形状（差分をゲートのコストだけにする前提）。
    let router = Router::new()
        .route("GET", "/", |_head, _body| {
            Response::new(200, b"fandhe-backend: minimal example\n".to_vec())
        })
        .route("GET", "/health", |_head, _body| {
            Response::new(200, b"ok\n".to_vec())
        });

    let mut server = Server::new();
    if env::var("FANDHE_BACKEND_JWT_GATE").as_deref() != Ok("off") {
        let policy = JwtPolicy::new(
            Issuer::new(ISS).map_err(|_| fail("issuer"))?,
            Audiences::single(AUD).map_err(|_| fail("audience"))?,
            AllowedAlgorithms::new([Algorithm::Rs256, Algorithm::Es256])
                .map_err(|_| fail("algorithms"))?,
            TokenType::AccessToken,
        );
        let keys = SharedKeySet::from_json(&jwks).map_err(|_| fail("jwks"))?;
        let auth = JwtAuthenticator::<NoExtraClaims>::builder(keys, policy).build();
        server = server.gate(JwtGate::new(auth));
    }
    let server = server.handler(router);

    let addr =
        env::var("FANDHE_BACKEND_JWT_BENCH_ADDR").unwrap_or_else(|_| "127.0.0.1:3103".into());
    let bound = server.bind(addr.as_str()).await?;
    println!(
        "jwt_gate_server: listening on http://{}",
        bound.local_addr()?
    );
    bound.run().await
}
