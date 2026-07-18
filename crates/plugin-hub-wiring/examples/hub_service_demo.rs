//! TASK-9.5（#65）: 受け入れ検証用のダミー hub サービス。
//!
//! PoC-6 手書き実装（3 エンドポイント・207 行の JWT 検証 + `org_id` 抽出 +
//! テナントスコープ強制コード）を模した、マルチテナントの `GET /items`・
//! `GET /items/{id}`・`POST /items` を持つ最小サーバ。JWT (RS256) 検証・
//! `org_id` 抽出・スコープ強制のコードはハンドラに一切書かず、
//! [`bf_plugin_hub_wiring`] が提供する [`TenantGate`]（`RequestGate` 拡張点）と
//! [`Authenticator`]（ゲートと共有するキャッシュ、TASK-9.3 / #63）のみで賄う
//! （越境クエリは全件データ層で 404、`scripts/accept/hub-wiring-accept.sh` の
//! 判定 A・B が本ファイルを対象に検証する）。
//!
//! # 配線マーカー（`scripts/accept/hub-wiring-accept.sh` 判定 B が LOC 集計）
//!
//! `// --- wiring:begin ---` 〜 `// --- wiring:end ---` の区間が、利用側サービス
//! （hub サービス）が新たに書く必要のある配線コードそのもの。PoC-6 の 207 行
//! （3 エンドポイント × 手書き JWT 検証・クレーム抽出・境界チェック）に対し、
//! ここでは JWKS 注入 + ゲート登録の数行のみで済む。
//!
//! # `BF_HUB_GATE=off`（NFR-6 「無関係パスへの影響」計測用、`benches/hub-nfr6-bench.sh`）
//!
//! 環境変数 `BF_HUB_GATE=off` を設定すると `TenantGate` を登録せずに起動する
//! （本クレート・依存自体はリンクされたまま。ビルド成果物としてのリンク
//! コストとゲート登録コストを分離計測するための切り替え）。`GET /items` 系は
//! この構成でもルート自体は登録済みのままだが、各ハンドラが `require_org`
//! （`Authenticator::authenticate` 委譲）で認証を強制するため、有効なトークン
//! なしに叩くと `401` になる（ゲート未登録＝認証なしで通る、という意味では
//! ない）。NFR-6 計測は認証を要さない `GET /`（無関係パス）のみを対象にする。
//!
//! ```bash
//! cargo run --release -p bf-plugin-hub-wiring --example hub_service_demo
//! # 別ターミナルで（起動時に表示される curl コマンドをそのまま使う）:
//! curl -i http://127.0.0.1:3100/items
//! ```

use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use backend_framework_core::Server;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bf_http::request::RequestHead;
use bf_http::response::Response;
use bf_plugin_hub_wiring::{Authenticator, TenantGate, TenantGateConfig};
use bf_routes::Router;
use ring::rand::SystemRandom;
use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};

/// デモ・受け入れ検証専用の RSA 2048bit 秘密鍵（PKCS#8 DER）。本番使用禁止
/// （`tests/fixtures/README.md`、`.claude/rules/security.md` シークレット混入防止）。
const DEMO_PKCS8: &[u8] = include_bytes!("../tests/fixtures/test-rsa-2048.pk8");
const DEMO_KID: &str = "hub-demo-kid-1";

/// マルチテナントのダミーデータ 1 件（TASK-9.5 の越境遮断検証対象）。
#[derive(Clone)]
struct Item {
    org_id: String,
    name: String,
}

/// `Arc<RwLock<..>>` で複数コネクションタスク間に共有するインメモリストア。
type Store = Arc<RwLock<HashMap<u64, Item>>>;

/// org-1 / org-2 それぞれにダミー item を投入する（越境クエリの列挙対象）。
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

/// `RsaKeyPair` の公開鍵から JWKS ドキュメント（単一鍵）を組み立てる
/// （`tests/tenant_gate.rs`・`examples/jwt_cache_bench.rs` と同型のヘルパー）。
fn jwks_json_for(keypair: &RsaKeyPair, kid: &str) -> String {
    let components: RsaPublicKeyComponents<Vec<u8>> =
        RsaPublicKeyComponents::from(keypair.public_key());
    let n_b64 = URL_SAFE_NO_PAD.encode(&components.n);
    let e_b64 = URL_SAFE_NO_PAD.encode(&components.e);
    format!(
        r#"{{"keys":[{{"kty":"RSA","kid":"{kid}","n":"{n_b64}","e":"{e_b64}","use":"sig","alg":"RS256"}}]}}"#
    )
}

/// 手動動作確認（`curl`）用に、起動時に 1 件だけ有効な RS256 トークンを発行する。
/// 受け入れテスト本体（`tests/hub_acceptance.rs`）は自前でトークンを組み立てて
/// おり本関数には依存しない。
fn demo_token(keypair: &RsaKeyPair, org_id: &str) -> String {
    let header = format!(r#"{{"alg":"RS256","typ":"JWT","kid":"{DEMO_KID}"}}"#);
    let payload = format!(r#"{{"org_id":"{org_id}","exp":9999999999}}"#);
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
        .expect("signing succeeds with demo fixture key");
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
    format!("{header_b64}.{payload_b64}.{sig_b64}")
}

/// ゲート通過済みリクエストから `org_id` を取り出す。JWT 検証・クレーム抽出は
/// 一切自前実装せず [`Authenticator::authenticate`]（本クレート提供）へ委譲する
/// （TASK-9.5 の削減対象そのもの）。`BF_HUB_GATE=off` 構成で `/items` 系へ
/// 直接到達した場合（ゲート未登録）はここで検証が走り、失敗時は `401` を
/// 返す（ハンドラ単体でもフェイルクローズを維持する防御的多層化）。
fn require_org(authenticator: &Authenticator, head: &RequestHead) -> Result<String, Response> {
    authenticator
        .authenticate(head)
        .map(|claims| claims.org_id)
        .map_err(|_err| Response::empty(401))
}

/// PoC-6 と同数（3 エンドポイント、`GET /items`・`GET /items/{id}`・`POST /items`）に
/// 加え、`GET /`（NFR-6「無関係パスへの影響」計測対象、`benches/hub-nfr6-bench.sh`）を
/// 持つ `Router` を組み立てる。`store`・`authenticator` は各ハンドラクロージャへ
/// `Arc`/`Clone` 共有する。
///
/// `GET /` は `crates/core/examples/minimal.rs` の `GET /`（無認証・200 固定応答）と
/// 完全に同一の応答形状にする。NFR-6 計測は「ベースラインと比較対象の双方が同じ
/// パスへの同じ応答（200・同等サイズの body）を返す」ことが前提であり、双方の
/// 応答が食い違う（例: 片方だけ未登録で 404）と無関係パスへの影響ではなく応答
/// パス自体の違いを測ってしまう（`crates/core/examples/graphql_nfr6.rs` /
/// `webrtc_nfr6.rs` も同様にベースラインと同一の `GET /` を維持する）。
fn build_router(store: Store, authenticator: Authenticator, next_id: Arc<AtomicU64>) -> Router {
    let mut router = Router::new();

    router = router.route("GET", "/", |_head, _body| {
        Response::new(
            200,
            b"backend-framework: hub_service_demo example\n".to_vec(),
        )
    });

    router = router.route("GET", "/items", {
        let store = store.clone();
        let authenticator = authenticator.clone();
        move |head, _body| {
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

    // 単件取得（`bf_routes::Router` は完全一致のみのため、越境遮断の全件検証
    // 対象になる既知 ID を個別ルートとして列挙登録する。`docs/design/
    // outbox-consent-integration.md` の「データ層フェイルクローズ 404」を踏襲し、
    // 存在するが他テナントの item も未登録 ID と同一の 404 を返す（情報漏洩防止）。
    for id in 1..=4u64 {
        let store = store.clone();
        let authenticator = authenticator.clone();
        router = router.route("GET", format!("/items/{id}"), move |head, _body| {
            let org_id = match require_org(&authenticator, head) {
                Ok(org_id) => org_id,
                Err(resp) => return resp,
            };
            let items = store.read().expect("store lock not poisoned");
            match items.get(&id) {
                Some(item) if item.org_id == org_id => {
                    Response::new(200, format!("{id}\t{}\n", item.name).into_bytes())
                }
                _ => Response::empty(404),
            }
        });
    }

    router = router.route("POST", "/items", move |head, body| {
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
        let new_id = next_id.fetch_add(1, Ordering::SeqCst);
        store
            .write()
            .expect("store lock not poisoned")
            .insert(new_id, Item { org_id, name });
        Response::new(201, new_id.to_string().into_bytes())
    });

    router
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let store = seed_store();
    let next_id = Arc::new(AtomicU64::new(5));
    let keypair = RsaKeyPair::from_pkcs8(DEMO_PKCS8).expect("valid demo pkcs8 fixture");
    let jwks_json = jwks_json_for(&keypair, DEMO_KID);

    // --- wiring:begin ---
    // hub サービス側が新規に書く配線コードはこの区間のみ（PoC-6 の 207 行相当を
    // 代替、`scripts/accept/hub-wiring-accept.sh` 判定 B が本区間の LOC を集計）。
    let config = TenantGateConfig::from_jwks_json(&jwks_json).expect("valid demo jwks");
    let authenticator = config.authenticator();
    let mut server = Server::new();
    if env::var("BF_HUB_GATE").as_deref() != Ok("off") {
        server = server.gate(TenantGate::new(config));
    }
    // --- wiring:end ---

    let router = build_router(store, authenticator, next_id);
    let server = server.handler(router);

    let token = demo_token(&keypair, "org-1");

    // bind 成功後にのみ readiness をログ出力する（`minimal` 例と同一方針）。
    // bind 前に出力すると、bind 失敗時にも "listening" 行が残る／レースする
    // クライアントが connection refused になり得る（Cursor Bugbot 指摘対応）。
    let bound = server.bind("127.0.0.1:3100").await?;
    println!("hub_service_demo: listening on http://127.0.0.1:3100");
    println!("try: curl -i -H 'Authorization: Bearer {token}' http://127.0.0.1:3100/items");

    bound.run().await
}
