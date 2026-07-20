//! 圧縮プラグイン（`compression` feature）の配線サンプル（イシュー #321）。
//!
//! `fandhe_backend_plugin_compression` を実際に配線した最小 API を起動する。
//! `Server::compression(config)` を登録すると、`crate::plugin::finalize_response`
//! （非公開シーム経由）が条件充足時に応答を gzip 圧縮する
//! （`crates/plugin-compression/src/lib.rs` の crate doc を参照）。
//!
//! # 動作確認手順
//!
//! ```text
//! $ cargo run --example compression_demo -p fandhe-backend-core --features compression
//!
//! # 閾値以上の text/plain・Accept-Encoding: gzip → Content-Encoding: gzip
//! $ curl -si localhost:3008/large -H 'Accept-Encoding: gzip' | head -20
//!
//! # Accept-Encoding なし → 無圧縮のまま
//! $ curl -si localhost:3008/large | head -20
//!
//! # 閾値未満の応答 → 無圧縮のまま（min_size 未満）
//! $ curl -si localhost:3008/small -H 'Accept-Encoding: gzip'
//! ```

use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_compression::CompressionConfig;
use fandhe_backend_routes::Router;

/// 既定閾値（1024 バイト）・既定 `Content-Type` リストのまま利用する
/// 圧縮設定を組み立てる。
fn compression_config() -> CompressionConfig {
    CompressionConfig::builder().build()
}

/// 圧縮対象になる大きめの `text/plain` 応答・対象外にする小さな応答を
/// 持つ最小 [`Router`] を組み立てる。
fn build_router() -> Router {
    Router::new()
        .route("GET", "/large", |_head, _body| {
            // 既定閾値 1024 バイトを超える text/plain（繰り返し文字列）。
            let body = "fandhe-backend のレスポンス圧縮デモ。".repeat(50);
            Response::new(200, body.into_bytes()).with_content_type("text/plain")
        })
        .route("GET", "/small", |_head, _body| {
            // 既定閾値未満のため圧縮対象から外れる。
            Response::new(200, b"ok".to_vec()).with_content_type("text/plain")
        })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3008".to_string());
    let router = build_router();
    // `Server::compression` 未登録なら feature 有効でも完全フォールスルー
    // する（`Server::compression` の doc を参照）。
    let server = fandhe_backend_core::Server::new()
        .handler(router)
        .compression(compression_config());

    println!("compression_demo: listening on http://{addr}");
    let bound = server.bind(&addr).await?;
    bound.run().await
}
