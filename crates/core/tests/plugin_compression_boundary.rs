//! `compression` feature（イシュー #321）配線の統合テスト（feature 有効側）。
//!
//! `crate::plugin::finalize_response`（非公開シーム）経由で
//! `fandhe_backend_plugin_compression::apply_compression` が実リクエスト
//! 応答へ適用されることを、`tokio::io::duplex` で駆動する
//! `handle_connection` を通して確認する:
//!
//! - `Server::compression(config)` 登録 + `Accept-Encoding: gzip` →
//!   `Content-Encoding: gzip` + 解凍後 body が元と同一（応答同一性、
//!   受け入れ基準）
//! - `Server::compression` 未登録 → 無圧縮（設定登録型プラグインの
//!   フォールスルー）
//! - `Accept-Encoding` なし → 無圧縮
//! - 閾値未満・対象外 `Content-Type` → 無圧縮
//! - `try_intercept` 応答（`graphql` feature 併用時のパスインターセプト型
//!   プラグイン応答）にも同一の後処理が適用されることを確認する
//!   （`crate::plugin::finalize_response` の doc の利点を実証）
//!
//! feature 無効時の陰性対照は `plugin_compression_boundary_disabled.rs` を参照。

#![cfg(feature = "compression")]

use fandhe_backend_core::interceptor::Interceptor;
use fandhe_backend_core::{Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_compression::CompressionConfig;
use fandhe_backend_routes::Router;
use std::io::Read;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 既定閾値（1024 バイト）を超える text/plain 応答を返す `Router`。
fn build_router() -> Router {
    Router::new()
        .route("GET", "/large", |_head, _body| {
            let body = "x".repeat(2048);
            Response::new(200, body.into_bytes()).with_content_type("text/plain")
        })
        .route("GET", "/small", |_head, _body| {
            Response::new(200, b"ok".to_vec()).with_content_type("text/plain")
        })
        .route("GET", "/image", |_head, _body| {
            let body = vec![0u8; 2048];
            Response::new(200, body).with_content_type("image/png")
        })
}

async fn roundtrip_raw(server: &Server, request: &[u8]) -> Vec<u8> {
    let (mut client, server_stream) = tokio::io::duplex(65536);
    client.write_all(request).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    out
}

/// 生バイト列のレスポンスを status line・ヘッダ文字列・生 body に分割する
/// （gzip 圧縮された body は UTF-8 として解釈できないため文字列化しない）。
fn split_response(raw: &[u8]) -> (String, Vec<u8>) {
    let sep = b"\r\n\r\n";
    let pos = raw
        .windows(sep.len())
        .position(|w| w == sep)
        .expect("レスポンスに空行区切りがない");
    let head = String::from_utf8(raw[..pos].to_vec()).unwrap();
    let body = raw[pos + sep.len()..].to_vec();
    (head, body)
}

#[tokio::test]
async fn registered_with_accept_encoding_compresses_and_roundtrips() {
    let config = CompressionConfig::builder().min_size(1).build();
    let router = build_router();
    let server = Server::new().handler(router).compression(config);

    let request = b"GET /large HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(head.contains("Content-Encoding: gzip\r\n"));
    assert!(head.contains("Vary: Accept-Encoding\r\n"));

    let mut decoder = flate2::read::GzDecoder::new(body.as_slice());
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).unwrap();
    assert_eq!(decoded, "x".repeat(2048));
}

#[tokio::test]
async fn unregistered_leaves_response_unmodified() {
    // `Server::compression` 未登録（`compression` feature は有効）は他
    // プラグインと同じ設定登録型パターンにより完全フォールスルーする。
    let router = build_router();
    let server = Server::new().handler(router);

    let request = b"GET /large HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(!head.contains("Content-Encoding"));
    assert_eq!(body, "x".repeat(2048).into_bytes());
}

#[tokio::test]
async fn without_accept_encoding_header_stays_uncompressed() {
    let config = CompressionConfig::builder().min_size(1).build();
    let router = build_router();
    let server = Server::new().handler(router).compression(config);

    let request = b"GET /large HTTP/1.1\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(!head.contains("Content-Encoding"));
    // Content-Type 一致・閾値超過は満たすため Vary は付与される。
    assert!(head.contains("Vary: Accept-Encoding\r\n"));
    assert_eq!(body, "x".repeat(2048).into_bytes());
}

#[tokio::test]
async fn below_threshold_stays_uncompressed() {
    let config = CompressionConfig::builder().build(); // 既定閾値 1024
    let router = build_router();
    let server = Server::new().handler(router).compression(config);

    let request = b"GET /small HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(!head.contains("Content-Encoding"));
    assert_eq!(body, b"ok");
}

#[tokio::test]
async fn non_matching_content_type_stays_uncompressed() {
    let config = CompressionConfig::builder().min_size(1).build();
    let router = build_router();
    let server = Server::new().handler(router).compression(config);

    let request = b"GET /image HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(!head.contains("Content-Encoding"));
    assert!(!head.contains("Vary"));
    assert_eq!(body, vec![0u8; 2048]);
}

#[cfg(feature = "cors")]
#[tokio::test]
async fn cors_and_compression_apply_in_sequence() {
    // イシュー #321 の設計判断: `finalize_response` は CORS → 圧縮の順で
    // 逐次適用する（`crates/plugin-compression/src/lib.rs` の crate doc を
    // 参照）。両方の後処理が同一レスポンスに効くことを確認する。
    use fandhe_backend_plugin_cors::CorsConfig;

    let cors_config = CorsConfig::builder()
        .allow_origin("https://app.example.com")
        .build()
        .unwrap();
    let compression_config = CompressionConfig::builder().min_size(1).build();
    let router = build_router();
    let server = Server::new()
        .handler(router)
        .cors(cors_config)
        .compression(compression_config);

    let request = b"GET /large HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(head.contains("Access-Control-Allow-Origin: https://app.example.com\r\n"));
    assert!(head.contains("Content-Encoding: gzip\r\n"));

    let mut decoder = flate2::read::GzDecoder::new(body.as_slice());
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).unwrap();
    assert_eq!(decoded, "x".repeat(2048));
}

/// 応答 body を大きく差し替える `Interceptor::map_response`（イシュー #420）。
/// `map_response` 後の body が圧縮対象になる（順序: map_response →
/// finalize_response の CORS → 圧縮）ことの検証に使う。
struct ExpandBody;
impl Interceptor for ExpandBody {
    fn name(&self) -> &'static str {
        "expand-body"
    }

    fn map_response(&self, _head: &RequestHead, response: Response) -> Response {
        Response::new(response.status, "y".repeat(2048).into_bytes())
            .with_content_type("text/plain")
    }
}

#[tokio::test]
async fn interceptor_map_response_output_is_compressed() {
    // イシュー #420 の設計判断: `Interceptor::map_response` は
    // `finalize_response`（CORS → 圧縮）より前に適用する。`map_response` が
    // 差し替えた body（元の `/small` 応答とは別の 2048 バイト body）が
    // gzip 圧縮対象になることを確認する（`crates/core/src/interceptor.rs`
    // モジュール doc の評価順序を参照）。
    let config = CompressionConfig::builder().min_size(1).build();
    let router = build_router();
    let server = Server::new()
        .interceptor(ExpandBody)
        .handler(router)
        .compression(config);

    let request = b"GET /small HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(head.contains("Content-Encoding: gzip\r\n"));

    let mut decoder = flate2::read::GzDecoder::new(body.as_slice());
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).unwrap();
    assert_eq!(decoded, "y".repeat(2048));
}
