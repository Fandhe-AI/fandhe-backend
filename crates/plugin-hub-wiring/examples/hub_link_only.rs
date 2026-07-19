//! TASK-9.5（#65）NFR-6「リンクコストのみ」計測専用サーバ（Cursor Bugbot 指摘対応、PR #163）。
//!
//! `examples/hub_service_demo.rs` は PoC-6 相当のマルチテナント `/items` 系ハンドラ
//! （JWKS 注入・RSA 鍵・シードストア・`Authenticator` 呼び出し）を持つため、
//! `benches/hub-nfr6-bench.sh` がその `GET /`（無関係パス）へ負荷をかけても
//! 計測値には `fandhe-backend-plugin-hub-wiring` のリンクコストに加えてアプリケーション層
//! （マルチルート登録・ハンドラクロージャの `Arc`/`Clone` キャプチャ量等）の
//! オーバーヘッドが混入し得る、という指摘（Bugbot review 4727552092、指摘1）を
//! 受けて追加した最小 example。
//!
//! `examples/minimal.rs`（ベースライン）と同一の `GET /`（200・同一 body）のみを
//! 持ち、`FANDHE_BACKEND_HUB_GATE=off` 未設定時のみ空 JWKS（`TenantGateConfig::from_jwks_json`
//! の doctest と同一の `{"keys":[]}`、鍵ローテーション・実トークンは計測対象外の
//! ため不要）で構成した `TenantGate` を登録する。`hub_service_demo.rs` の
//! `// --- wiring:begin --- 〜 // --- wiring:end ---` 区間と同型の配線のみを行い、
//! `/items` 系ハンドラ・シードストア・RSA 鍵は一切持たない
//! （`crates/core/examples/graphql_nfr6.rs`・`webrtc_nfr6.rs` と同型のパターン。
//! これらは対象 feature がコア機能のため `crates/core/examples/` に置くが、
//! hub-wiring はコア機能ではなく `server.gate(...)` 経由の汎用配線のため、
//! 本 example は `crates/plugin-hub-wiring/examples/` に置く）。
//!
//! `benches/hub-nfr6-bench.sh` の主計測（`FANDHE_BACKEND_HUB_GATE=off` によるリンクコスト
//! 分離計測）は本 example をベースラインと比較する。ゲート有効 + 有効トークン時の
//! opt-in コスト参考値（PASS/FAIL 判定には使わない）は、実データ・実トークンを
//! 要するため引き続き `hub_service_demo.rs` を手動計測で使う
//! （`docs/acceptance/req9-hub-wiring.md` 参照）。
//!
//! ```bash
//! cargo build --release -p fandhe-backend-plugin-hub-wiring --example hub_link_only
//! FANDHE_BACKEND_HUB_GATE=off ./target/release/examples/hub_link_only &
//! curl -v http://127.0.0.1:3101/   # 200 応答（無関係パス、minimal と同一 body）
//! ```

use std::env;

use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_hub_wiring::{TenantGate, TenantGateConfig};
use fandhe_backend_routes::Router;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    // `examples/minimal.rs` の `GET /` と完全に同一の応答形状にする
    // （ベースラインとの差分がリンクコストのみになるようにするための前提、
    // `hub_service_demo.rs` の同種コメントと同じ理由）。
    let router = Router::new().route("GET", "/", |_head, _body| {
        Response::new(200, b"backend-framework: minimal example\n".to_vec())
    });

    // --- wiring:begin ---
    // 実データ・実トークンを持たないため空 JWKS で構成する（鍵ローテーション・
    // 認証成否は本 example の計測範囲外。`TenantGateConfig::from_jwks_json` の
    // doctest と同一の `{"keys":[]}`）。
    let config = TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#)
        .expect("空 JWKS の構築は静的に妥当なので必ず成功する");
    let mut server = Server::new();
    if env::var("FANDHE_BACKEND_HUB_GATE").as_deref() != Ok("off") {
        server = server.gate(TenantGate::new(config));
    }
    // --- wiring:end ---

    let server = server.handler(router);

    // hub_service_demo（3100）と衝突しないポートを使う（同時起動の可能性は
    // 低いが、benches/hub-nfr6-bench.sh 以外からの手動起動も考慮する）。
    let bound = server.bind("127.0.0.1:3101").await?;
    println!("hub_link_only: listening on http://{}", bound.local_addr()?);
    bound.run().await
}
