//! `cors` feature（イシュー #305）配線の統合テスト（feature 有効側）。
//!
//! 2 層構成（`crates/plugin-cors/src/lib.rs` の crate doc を参照）の両方を
//! `tokio::io::duplex` で駆動する `handle_connection` を通して検証する:
//!
//! 1. プリフライト: `fandhe_backend_routes::Router::options_fallback`
//!    （イシュー #304）へ `fandhe_backend_plugin_cors::preflight_response` を
//!    直接配線し、`OPTIONS` + `Origin` + `Access-Control-Request-Method` が
//!    204 + CORS ヘッダで応答されることを確認する
//! 2. 実リクエストへのヘッダ付与: `Server::cors` に登録した設定が
//!    `crate::plugin::finalize_response`（非公開シーム）経由で `Router` の
//!    通常応答（`impl Handler for Router`）へ CORS ヘッダを付与することを
//!    確認する
//!
//! 明示登録 OPTIONS ルートが `options_fallback` より優先される既存契約
//! （イシュー #304）の回帰がないことも併せて確認する。
//!
//! feature 無効時の陰性対照は `plugin_cors_boundary_disabled.rs` を参照。

#![cfg(feature = "cors")]

use fandhe_backend_core::{Server, handle_connection};
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_cors::{CorsConfig, preflight_response};
use fandhe_backend_routes::Router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `GET`/`POST /todos` を静的登録し、OPTIONS プリフライトを CORS プラグインへ
/// 委譲する `Router`（`crates/plugin-cors` の crate doc の 2 点配線例）。
fn build_router(config: CorsConfig) -> Router {
    Router::new()
        .route("GET", "/todos", |_head, _body| {
            Response::new(200, b"[]".to_vec())
        })
        .route("POST", "/todos", |_head, _body| Response::empty(201))
        .options_fallback(move |head, allow, _body| preflight_response(head, allow, &config))
}

async fn roundtrip(server: &Server, request: &[u8]) -> String {
    let (mut client, server_stream) = tokio::io::duplex(8192);
    client.write_all(request).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

fn allowed_origin_config() -> CorsConfig {
    CorsConfig::builder()
        .allow_origin("https://app.example.com")
        .build()
        .unwrap()
}

#[tokio::test]
async fn preflight_from_allowed_origin_returns_204_with_cors_headers() {
    let config = allowed_origin_config();
    let router = build_router(config.clone());
    let server = Server::new().handler(router).cors(config);

    let request = b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccess-Control-Request-Method: POST\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;

    // PoC-9 教訓: ステータスのみでなく reason phrase・ヘッダ・body 全件を検証する
    // （`crates/core/tests/plugin_graphql_boundary.rs` と同一原則）。
    assert!(response.starts_with("HTTP/1.1 204 No Content\r\n"));
    assert!(response.contains("Access-Control-Allow-Origin: https://app.example.com\r\n"));
    assert!(response.contains("Access-Control-Allow-Methods:"));
    assert!(response.contains("Vary: Origin\r\n"));
}

#[tokio::test]
async fn preflight_from_disallowed_origin_returns_403_without_cors_headers() {
    let config = allowed_origin_config();
    let router = build_router(config.clone());
    let server = Server::new().handler(router).cors(config);

    let request = b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://evil.example\r\nAccess-Control-Request-Method: POST\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;

    assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
    assert!(!response.contains("Access-Control-Allow-Origin"));
}

#[tokio::test]
async fn actual_request_from_allowed_origin_gets_cors_headers() {
    let config = allowed_origin_config();
    let router = build_router(config.clone());
    let server = Server::new().handler(router).cors(config);

    let request =
        b"GET /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("Access-Control-Allow-Origin: https://app.example.com\r\n"));
    assert!(response.contains("Vary: Origin\r\n"));
    assert!(response.ends_with("[]"));
}

#[tokio::test]
async fn actual_request_from_disallowed_origin_is_untouched() {
    let config = allowed_origin_config();
    let router = build_router(config.clone());
    let server = Server::new().handler(router).cors(config);

    let request =
        b"GET /todos HTTP/1.1\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;

    // 不許可オリジンは無改変（フェイルクローズ、ブラウザ側でブロックさせる設計）。
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(!response.contains("Access-Control-Allow-Origin"));
}

#[tokio::test]
async fn same_origin_request_without_origin_header_is_completely_unaffected() {
    // Origin ヘッダなし（同一オリジンリクエスト）は既存挙動の回帰なし。
    let config = allowed_origin_config();
    let router = build_router(config.clone());
    let server = Server::new().handler(router).cors(config);

    let request = b"GET /todos HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(!response.contains("Access-Control-Allow-Origin"));
    assert!(!response.contains("Vary"));
}

#[tokio::test]
async fn cors_unregistered_leaves_all_responses_unmodified() {
    // `Server::cors` 未登録（`cors` feature は有効）は他プラグインと同じ
    // 設定登録型パターンにより完全フォールスルーする
    // （`crate::plugin::finalize_response` の doc を参照）。
    let config = allowed_origin_config();
    let router = build_router(config);
    let server = Server::new().handler(router);

    let request =
        b"GET /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(!response.contains("Access-Control-Allow-Origin"));
}

#[tokio::test]
async fn explicit_options_route_still_wins_over_options_fallback() {
    // イシュー #304 の契約（明示登録 OPTIONS ルートが `options_fallback` より
    // 常に優先される）の回帰がないことを確認する。
    let config = allowed_origin_config();
    let router = build_router(config.clone()).route("OPTIONS", "/todos", |_head, _body| {
        Response::new(299, b"explicit".to_vec())
    });
    let server = Server::new().handler(router).cors(config);

    let request = b"OPTIONS /todos HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccess-Control-Request-Method: POST\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;

    assert!(response.starts_with("HTTP/1.1 299"));
    assert!(response.ends_with("explicit"));
}
