//! TASK-9.5（#65 / REQ-9）: hub 共通配線の受け入れ統合テスト。
//!
//! `tests/tenant_gate.rs`（TenantGate 単体の E2E 確認）を土台に、PoC-6 相当の
//! 実データ入りマルチテナントハンドラ（`GET /items`・`GET /items/{id}`・
//! `POST /items`）を構えたダミー hub サービス構成で、次を検証する:
//!
//! - **越境クエリ 100% 遮断**: org-1 トークンで org-2 の item を列挙的に全件
//!   要求し、1 件も漏れず 404（データ層フェイルクローズ）になること
//! - **JWT 欠落・不正時のフェイルクローズ**: `tests/tenant_gate.rs` の各失敗
//!   ケースを、実ハンドラ（到達すればデータ層まで届く構成）でも再確認し、
//!   `RequestGate` が拒否した場合はハンドラへ**到達すらしない**ことを到達
//!   カウンタで直接証跡化する
//! - **鍵ローテーション**: `SharedJwks::set` によるローテーション後、旧鍵
//!   トークンが拒否され新鍵トークンが許可されること（再起動なし）
//! - **検証結果キャッシュの共有**: TASK-9.3（#63）の `Authenticator` 共有が
//!   実データハンドラでも機能し、ゲート（ミス）→ ハンドラ（ヒット）の順に
//!   なること
//!
//! 越境遮断・フェイルクローズの判定ロジック（マーカー区間 LOC 集計・
//! ハンドラ内手書き JWT シンボルの不在確認）は `scripts/accept/
//! hub-wiring-accept.sh` が `examples/hub_service_demo.rs` を対象に別途行う。
//! 本ファイルは「実際にランタイムで越境が防げているか」を保証する。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use backend_framework_core::{Server, handle_connection};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bf_http::request::RequestHead;
use bf_http::response::Response;
use bf_plugin_hub_wiring::jwks::{JwksKeySet, SharedJwks};
use bf_plugin_hub_wiring::{Authenticator, TenantGate, TenantGateConfig, TokenError};
use bf_routes::Router;
use ring::rand::SystemRandom;
use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TEST_KID: &str = "test-kid-1";
const ROTATED_KID: &str = "test-kid-2";

/// org-1 が保有する item ID（越境検証における「自テナント」側）。
const ORG1_IDS: [u64; 2] = [1, 2];
/// org-2 が保有する item ID（越境検証における「他テナント」側）。
const ORG2_IDS: [u64; 2] = [3, 4];

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

fn bearer_request(target: &str, method: &str, token: Option<&str>, body: &str) -> Vec<u8> {
    match token {
        Some(token) => format!(
            "{method} {target} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
        None => format!(
            "{method} {target} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    }
    .into_bytes()
}

/// PoC-6 相当のマルチテナントダミーデータ 1 件。
#[derive(Clone)]
struct Item {
    org_id: String,
    name: String,
}

type Store = Arc<RwLock<HashMap<u64, Item>>>;

/// org-1: id 1, 2 / org-2: id 3, 4 を投入する（越境検証の列挙対象を固定する）。
fn seed_store() -> Store {
    let mut map = HashMap::new();
    map.insert(
        1,
        Item {
            org_id: "org-1".to_string(),
            name: "alpha".to_string(),
        },
    );
    map.insert(
        2,
        Item {
            org_id: "org-1".to_string(),
            name: "beta".to_string(),
        },
    );
    map.insert(
        3,
        Item {
            org_id: "org-2".to_string(),
            name: "gamma".to_string(),
        },
    );
    map.insert(
        4,
        Item {
            org_id: "org-2".to_string(),
            name: "delta".to_string(),
        },
    );
    Arc::new(RwLock::new(map))
}

/// `TenantGate` が `Allow` を返した場合にのみ `Authenticator::authenticate` を
/// 呼ぶ（`examples/hub_service_demo.rs` と同一の再利用パターン）。JWT 検証・
/// クレーム抽出コードはここにも一切書かない。ステータスマッピングは
/// `TenantGate::check`（`src/gate.rs`）の判定ポリシーと一致させる
/// （`org_id` 欠落は `403`、それ以外は `401`。`examples/hub_service_demo.rs` と
/// 同一の修正、Cursor Bugbot 指摘対応、PR #163）。
fn require_org(authenticator: &Authenticator, head: &RequestHead) -> Result<String, Response> {
    authenticator
        .authenticate(head)
        .map(|claims| claims.org_id)
        .map_err(|err| match err {
            TokenError::MissingOrgId => {
                Response::new(403, br#"{"error":"tenant_scope_required"}"#.to_vec())
            }
            TokenError::MissingToken
            | TokenError::Malformed
            | TokenError::InvalidAlgorithm
            | TokenError::MissingKeyId
            | TokenError::UnknownKeyId
            | TokenError::InvalidSignature
            | TokenError::Expired => Response::new(401, br#"{"error":"invalid_token"}"#.to_vec()),
        })
}

/// `reached` はハンドラ本体へ実際に到達したリクエスト数を数える。`RequestGate`
/// が拒否した場合はコアループがハンドラを一切呼ばない契約
/// （`crates/core/src/server.rs` doc）のため、フェイルクローズ系テストでは
/// このカウンタが 0 のままであることが「ハンドラ未到達」の直接証跡になる。
fn build_router(store: Store, authenticator: Authenticator, reached: Arc<AtomicU64>) -> Router {
    let mut router = Router::new();

    router = router.route("GET", "/items", {
        let store = store.clone();
        let authenticator = authenticator.clone();
        let reached = reached.clone();
        move |head, _body| {
            reached.fetch_add(1, Ordering::SeqCst);
            let org_id = match require_org(&authenticator, head) {
                Ok(org_id) => org_id,
                Err(resp) => return resp,
            };
            let items = store.read().expect("store lock not poisoned");
            let mut ids: Vec<u64> = items
                .iter()
                .filter(|(_, item)| item.org_id == org_id)
                .map(|(id, _)| *id)
                .collect();
            ids.sort_unstable();
            let body: String = ids
                .into_iter()
                .map(|id| format!("{id}\t{}\n", items[&id].name))
                .collect();
            Response::new(200, body.into_bytes())
        }
    });

    for id in 1..=4u64 {
        let store = store.clone();
        let authenticator = authenticator.clone();
        let reached = reached.clone();
        router = router.route("GET", format!("/items/{id}"), move |head, _body| {
            reached.fetch_add(1, Ordering::SeqCst);
            let org_id = match require_org(&authenticator, head) {
                Ok(org_id) => org_id,
                Err(resp) => return resp,
            };
            let items = store.read().expect("store lock not poisoned");
            match items.get(&id) {
                // 他テナントの item も、未登録 ID と同一の 404 を返す（存在有無を
                // 漏らさないデータ層フェイルクローズ、越境クエリ 100% 遮断の対象）。
                Some(item) if item.org_id == org_id => {
                    Response::new(200, format!("{id}\t{}\n", item.name).into_bytes())
                }
                _ => Response::empty(404),
            }
        });
    }

    router = router.route("POST", "/items", {
        let reached = reached.clone();
        move |head, body| {
            reached.fetch_add(1, Ordering::SeqCst);
            let org_id = match require_org(&authenticator, head) {
                Ok(org_id) => org_id,
                Err(resp) => return resp,
            };
            let name = String::from_utf8_lossy(body).trim().to_string();
            let name = if name.is_empty() {
                "unnamed".to_string()
            } else {
                name
            };
            let mut items = store.write().expect("store lock not poisoned");
            let new_id = items.keys().max().copied().unwrap_or(0) + 1;
            items.insert(new_id, Item { org_id, name });
            Response::new(201, new_id.to_string().into_bytes())
        }
    });

    router
}

/// `TenantGate` + 実データハンドラを配線したダミー hub サービスを組み立てる。
/// `config` を `TenantGate::new` へ渡す前に `Authenticator` を取り出す手順は
/// `TenantGateConfig::authenticator` の doc・TASK-9.3（#63）の利用手順どおり。
fn hub_service(keypair: &RsaKeyPair) -> (Server, Arc<AtomicU64>, Authenticator) {
    let config = TenantGateConfig::from_jwks_json(&jwks_json_for(keypair, TEST_KID)).unwrap();
    let authenticator = config.authenticator();
    let reached = Arc::new(AtomicU64::new(0));
    let router = build_router(seed_store(), authenticator.clone(), reached.clone());
    let server = Server::new().gate(TenantGate::new(config)).handler(router);
    (server, reached, authenticator)
}

async fn roundtrip(server: &Server, request: &[u8]) -> String {
    // `tests/tenant_gate.rs` の `roundtrip` と同一のバッファサイズ根拠
    // （`oversized_token_is_rejected_before_handler` が単一バッファに収まる
    // ようにする）。
    let (mut client, server_stream) = tokio::io::duplex(32 * 1024);
    client.write_all(request).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

// --- 越境クエリ 100% 遮断（受け入れ基準の主目的） ---------------------------

#[tokio::test]
async fn cross_tenant_get_by_id_is_blocked_for_all_foreign_ids() {
    let keypair = test_keypair();
    let (server, reached, _authenticator) = hub_service(&keypair);
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");

    let mut blocked = 0usize;
    for id in ORG2_IDS {
        let target = format!("/items/{id}");
        let request = bearer_request(&target, "GET", Some(&token), "");
        let response = roundtrip(&server, &request).await;
        assert!(
            response.starts_with("HTTP/1.1 404"),
            "org-1 が org-2 の item {id} を越境取得できてしまった: {response}"
        );
        blocked += 1;
    }
    assert_eq!(
        blocked,
        ORG2_IDS.len(),
        "越境クエリは 100% 遮断が受け入れ基準（1 件でも漏れたら fail）"
    );
    assert_eq!(
        reached.load(Ordering::SeqCst),
        ORG2_IDS.len() as u64,
        "ゲートは許可（有効な org-1 トークン）だが、データ層で越境を拒否したことの証跡"
    );
}

#[tokio::test]
async fn own_tenant_get_by_id_succeeds_and_returns_only_own_data() {
    let keypair = test_keypair();
    let (server, _reached, _authenticator) = hub_service(&keypair);
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");

    for id in ORG1_IDS {
        let target = format!("/items/{id}");
        let request = bearer_request(&target, "GET", Some(&token), "");
        let response = roundtrip(&server, &request).await;
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "自テナントの item {id} は許可されるはず: {response}"
        );
    }
}

#[tokio::test]
async fn list_endpoint_returns_only_own_tenant_rows() {
    let keypair = test_keypair();
    let (server, _reached, _authenticator) = hub_service(&keypair);
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");

    let request = bearer_request("/items", "GET", Some(&token), "");
    let response = roundtrip(&server, &request).await;
    assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");
    assert!(
        response.contains("alpha") && response.contains("beta"),
        "org-1 分のみ含まれるべき: {response}"
    );
    assert!(
        !response.contains("gamma") && !response.contains("delta"),
        "org-2 の行が一覧に混入している（越境）: {response}"
    );
}

#[tokio::test]
async fn post_creates_item_scoped_to_caller_org_and_stays_tenant_isolated() {
    // `bf_routes::Router` は起動時登録の完全一致のみ（`crates/routes/src/lib.rs`
    // doc「実行時にルートを追加・削除する API は持たない」）のため、新規作成
    // item への単件アクセスは一覧（`GET /items`、org でフィルタする既存ロジック）
    // 経由で境界を確認する（単件ルートは既知 ID を起動時に列挙登録する設計、
    // `examples/hub_service_demo.rs` と同型）。
    let keypair = test_keypair();
    let (server, _reached, _authenticator) = hub_service(&keypair);
    let org1_token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");
    let org2_token = make_token(&keypair, TEST_KID, Some("org-2"), 9_999_999_999, "RS256");

    let create_request = bearer_request("/items", "POST", Some(&org1_token), "widget");
    let create_response = roundtrip(&server, &create_request).await;
    assert!(
        create_response.starts_with("HTTP/1.1 201"),
        "response: {create_response}"
    );

    let list_as_owner = bearer_request("/items", "GET", Some(&org1_token), "");
    let owner_response = roundtrip(&server, &list_as_owner).await;
    assert!(
        owner_response.contains("widget"),
        "作成した org-1 自身の一覧には新規 item が見えるはず: {owner_response}"
    );

    let list_as_other = bearer_request("/items", "GET", Some(&org2_token), "");
    let other_response = roundtrip(&server, &list_as_other).await;
    assert!(
        !other_response.contains("widget"),
        "新規作成データが org-2 の一覧に越境混入している: {other_response}"
    );
}

// --- フェイルクローズ（JWT 欠落・不正） -------------------------------------

#[tokio::test]
async fn missing_authorization_is_rejected_before_handler() {
    let keypair = test_keypair();
    let (server, reached, _authenticator) = hub_service(&keypair);
    let request = bearer_request("/items", "GET", None, "");
    let response = roundtrip(&server, &request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
    assert_eq!(
        reached.load(Ordering::SeqCst),
        0,
        "ゲート拒否時はハンドラへ到達しない"
    );
}

#[tokio::test]
async fn blank_bearer_token_is_rejected_before_handler() {
    let keypair = test_keypair();
    let (server, reached, _authenticator) = hub_service(&keypair);
    let request =
        b"GET /items HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer \r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn tampered_signature_is_rejected_before_handler() {
    let keypair = test_keypair();
    let (server, reached, _authenticator) = hub_service(&keypair);
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");
    let mut parts: Vec<&str> = token.split('.').collect();
    let mut sig = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
    sig[0] ^= 0xFF;
    let tampered_sig = URL_SAFE_NO_PAD.encode(sig);
    parts[2] = &tampered_sig;
    let tampered = parts.join(".");
    let request = bearer_request("/items", "GET", Some(&tampered), "");
    let response = roundtrip(&server, &request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn expired_token_is_rejected_before_handler() {
    let keypair = test_keypair();
    let (server, reached, _authenticator) = hub_service(&keypair);
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 1, "RS256");
    let request = bearer_request("/items", "GET", Some(&token), "");
    let response = roundtrip(&server, &request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn alg_none_is_rejected_before_handler() {
    let keypair = test_keypair();
    let (server, reached, _authenticator) = hub_service(&keypair);
    let header_b64 = URL_SAFE_NO_PAD.encode(format!(r#"{{"alg":"none","kid":"{TEST_KID}"}}"#));
    let payload_b64 = URL_SAFE_NO_PAD.encode(r#"{"org_id":"org-1","exp":9999999999}"#);
    let token = format!("{header_b64}.{payload_b64}.");
    let request = bearer_request("/items", "GET", Some(&token), "");
    let response = roundtrip(&server, &request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn alg_hs256_downgrade_is_rejected_before_handler() {
    // アルゴリズム混同（HS256 ダウングレード）攻撃の遮断（.claude/rules/security.md A05）。
    let keypair = test_keypair();
    let (server, reached, _authenticator) = hub_service(&keypair);
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "HS256");
    let request = bearer_request("/items", "GET", Some(&token), "");
    let response = roundtrip(&server, &request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unknown_kid_is_rejected_before_handler() {
    let keypair = test_keypair();
    let (server, reached, _authenticator) = hub_service(&keypair);
    let token = make_token(
        &keypair,
        "unregistered-kid",
        Some("org-1"),
        9_999_999_999,
        "RS256",
    );
    let request = bearer_request("/items", "GET", Some(&token), "");
    let response = roundtrip(&server, &request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn oversized_token_is_rejected_before_handler() {
    // `jwt::MAX_TOKEN_LEN`（8192 バイト）超のトークンはリソース枯渇対策として
    // 拒否する（.claude/rules/security.md A04）。
    let keypair = test_keypair();
    let (server, reached, _authenticator) = hub_service(&keypair);
    let huge = "a".repeat(9000);
    let request = bearer_request("/items", "GET", Some(&huge), "");
    let response = roundtrip(&server, &request).await;
    assert!(response.starts_with("HTTP/1.1 401"), "response: {response}");
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn missing_org_id_is_rejected_with_403_before_handler() {
    let keypair = test_keypair();
    let (server, reached, _authenticator) = hub_service(&keypair);
    let token = make_token(&keypair, TEST_KID, None, 9_999_999_999, "RS256");
    let request = bearer_request("/items", "GET", Some(&token), "");
    let response = roundtrip(&server, &request).await;
    assert!(response.starts_with("HTTP/1.1 403"), "response: {response}");
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn blank_org_id_is_rejected_with_403_before_handler() {
    let keypair = test_keypair();
    let (server, reached, _authenticator) = hub_service(&keypair);
    let token = make_token(&keypair, TEST_KID, Some("   "), 9_999_999_999, "RS256");
    let request = bearer_request("/items", "GET", Some(&token), "");
    let response = roundtrip(&server, &request).await;
    assert!(response.starts_with("HTTP/1.1 403"), "response: {response}");
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

// --- 鍵ローテーション（再起動なし） -----------------------------------------

#[tokio::test]
async fn key_rotation_via_shared_jwks_rejects_old_key_tokens_without_restart() {
    let old_keypair = test_keypair();
    let new_keypair = rotated_keypair();

    let shared = SharedJwks::from_json(&jwks_json_for(&old_keypair, TEST_KID)).unwrap();
    let config = TenantGateConfig::new(shared.clone());
    let authenticator = config.authenticator();
    let reached = Arc::new(AtomicU64::new(0));
    let router = build_router(seed_store(), authenticator, reached);
    let server = Server::new().gate(TenantGate::new(config)).handler(router);

    let old_token = make_token(
        &old_keypair,
        TEST_KID,
        Some("org-1"),
        9_999_999_999,
        "RS256",
    );
    let old_request = bearer_request("/items", "GET", Some(&old_token), "");
    let response = roundtrip(&server, &old_request).await;
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "ローテーション前は旧鍵トークンが許可されるはず: {response}"
    );

    shared.set(JwksKeySet::from_json(&jwks_json_for(&new_keypair, ROTATED_KID)).unwrap());

    let response_after_rotation = roundtrip(&server, &old_request).await;
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
    let new_request = bearer_request("/items", "GET", Some(&new_token), "");
    let response_new = roundtrip(&server, &new_request).await;
    assert!(
        response_new.starts_with("HTTP/1.1 200"),
        "ローテーション後は新鍵トークンが許可されるはず: {response_new}"
    );
}

// --- 検証結果キャッシュの共有（TASK-9.3 / #63 の実データハンドラでの再確認） ---

#[tokio::test]
async fn handler_reuses_gate_verification_via_shared_authenticator_with_real_data() {
    let keypair = test_keypair();
    let (server, _reached, authenticator) = hub_service(&keypair);
    let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");
    let request = bearer_request("/items/1", "GET", Some(&token), "");
    let response = roundtrip(&server, &request).await;
    assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");
    // ゲート（1 ミス: 実署名検証）→ ハンドラの `require_org`（1 ヒット:
    // キャッシュ再利用）の順で呼ばれたことを直接検証する。
    assert_eq!(authenticator.cache_misses(), 1);
    assert_eq!(authenticator.cache_hits(), 1);
}
