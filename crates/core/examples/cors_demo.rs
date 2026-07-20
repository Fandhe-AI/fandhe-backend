//! CORS プラグイン（`cors` feature）の 2 点配線サンプル（イシュー #305）。
//!
//! `fandhe_backend_plugin_cors` を実際に配線した最小 todo API を起動する。
//! `crates/plugin-cors/src/lib.rs` の crate doc が述べる 2 層構成をそのまま
//! 実装している:
//!
//! 1. `Router::options_fallback`（イシュー #304）へ
//!    `fandhe_backend_plugin_cors::preflight_response` を配線し、プリフライトを
//!    完結させる
//! 2. `Server::cors(config)` を登録し、実リクエストへ CORS ヘッダを付与する
//!    （`crate::plugin::finalize_response`、非公開シーム経由）
//!
//! # 動作確認手順
//!
//! ```text
//! $ cargo run --example cors_demo -p fandhe-backend-core --features cors
//!
//! # プリフライト（204 + Access-Control-Allow-* を確認）
//! $ curl -si -X OPTIONS localhost:3004/todos \
//!     -H 'Origin: https://app.example.com' \
//!     -H 'Access-Control-Request-Method: POST'
//!
//! # 実リクエスト（許可オリジン、Access-Control-Allow-Origin 付与を確認）
//! $ curl -si localhost:3004/todos -H 'Origin: https://app.example.com'
//!
//! # 実リクエスト（不許可オリジン、Access-Control-Allow-Origin なしを確認）
//! $ curl -si localhost:3004/todos -H 'Origin: https://evil.example'
//! ```

use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_cors::{CorsConfig, preflight_response};
use fandhe_backend_routes::Router;

/// `https://app.example.com` のみを許可する CORS 設定を組み立てる。
///
/// 実運用では起動時設定・環境変数等から許可オリジンを注入する想定
/// （本 example では固定値、`CorsConfig::builder` の doc を参照）。
fn cors_config() -> CorsConfig {
    CorsConfig::builder()
        .allow_origin("https://app.example.com")
        .allow_headers(["Content-Type"])
        .max_age(600)
        .build()
        .expect("固定の許可オリジン設定は allow_any_origin + credentials 併用を含まないため必ず成功する")
}

/// 最小 todo API（`GET`/`POST /todos`）に OPTIONS プリフライトの委譲先を
/// 配線した [`Router`] を組み立てる。
fn build_router(config: CorsConfig) -> Router {
    Router::new()
        .route("GET", "/todos", |_head, _body| {
            Response::new(200, b"[]".to_vec()).with_content_type("application/json")
        })
        .route("POST", "/todos", |_head, body| {
            Response::new(201, body.to_vec()).with_content_type("application/json")
        })
        // プリフライト側の配線（1/2）。`config` を move し、明示登録された
        // OPTIONS ルートが常に優先される契約（イシュー #304）はそのまま維持する。
        .options_fallback(move |head, allow, _body| preflight_response(head, allow, &config))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3004".to_string());
    let config = cors_config();
    let router = build_router(config.clone());
    // 実リクエスト側の配線（2/2）。`Server::cors` 未登録なら feature 有効でも
    // 完全フォールスルーする（`Server::cors` の doc を参照）。
    let server = fandhe_backend_core::Server::new()
        .handler(router)
        .cors(config);

    println!("cors_demo: listening on http://{addr}");
    let bound = server.bind(&addr).await?;
    bound.run().await
}
