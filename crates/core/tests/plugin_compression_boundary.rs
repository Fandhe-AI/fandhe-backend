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
//! - ストリーミング応答（`Handler::handle_streaming`）は既定
//!   （`compress_streaming` 未設定 = `false`）では圧縮対象外であることを
//!   確認する（設計判断の回帰防止）
//! - `compress_streaming(true)`（イシュー #461、opt-in）を明示登録した
//!   場合のみ、`crate::plugin::prepare_streaming_compression` 経由で
//!   chunked ストリーミング応答がチャンク単位に gzip 圧縮され、
//!   dechunk + gunzip で元の送出データ全体を復元できることを確認する
//!
//! feature 無効時の陰性対照は `plugin_compression_boundary_disabled.rs` を参照。

#![cfg(feature = "compression")]

use fandhe_backend_core::interceptor::Interceptor;
use fandhe_backend_core::streaming::StreamingResponse;
use fandhe_backend_core::{Handler, Server, handle_connection};
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
    // イシュー #461 レビュー指摘の回帰確認: CORS が先に `Vary: Origin` を
    // 確定していても、圧縮側の `Vary: Accept-Encoding` が欠落しないこと
    // （両方の Vary トークンが別ヘッダ行として共存する）。
    assert!(head.contains("Vary: Origin\r\n"), "head: {head}");
    assert!(head.contains("Vary: Accept-Encoding\r\n"), "head: {head}");

    let mut decoder = flate2::read::GzDecoder::new(body.as_slice());
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).unwrap();
    assert_eq!(decoded, "x".repeat(2048));
}

#[tokio::test]
async fn config_built_via_core_reexport_compresses_and_roundtrips() {
    // イシュー #421: `fandhe_backend_core::plugin_compression::CompressionConfig`
    // （プラグインクレートへの直接依存を追加しない再エクスポート経路）
    // 経由で構築した設定でも、直接依存経路（上のテスト）と同一の配線・
    // 応答になることを確認する。
    let config = fandhe_backend_core::plugin_compression::CompressionConfig::builder()
        .min_size(1)
        .build();
    let router = build_router();
    let server = Server::new().handler(router).compression(config);

    let request = b"GET /large HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
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

// --- ストリーミング応答の圧縮（既定 OFF、イシュー #461 で opt-in 追加）---

/// `handle_streaming` で閾値超過サイズのチャンクを返すトイハンドラ
/// （`min_size(1)` の圧縮設定でも常に圧縮対象サイズになる、
/// `crates/core/tests/interceptor.rs` の `StreamingOkHandler` と同型）。
struct StreamingOkHandler;
impl Handler for StreamingOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        Box::pin(std::future::ready(Response::empty(599)))
    }

    fn handle_streaming(&self, _head: &RequestHead, _body: &[u8]) -> Option<StreamingResponse> {
        let (response, writer) = StreamingResponse::channel(200, Some("text/plain"), 4);
        tokio::spawn(async move {
            let _ = writer.send("x".repeat(2048).into_bytes()).await;
            let _ = writer.finish().await;
        });
        Some(response)
    }
}

/// 複数チャンクへ分割して送出するトイハンドラ（圧縮のマルチチャンク
/// roundtrip 検証用、イシュー #461）。
struct StreamingMultiChunkHandler;
impl Handler for StreamingMultiChunkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        Box::pin(std::future::ready(Response::empty(599)))
    }

    fn handle_streaming(&self, _head: &RequestHead, _body: &[u8]) -> Option<StreamingResponse> {
        let (response, writer) = StreamingResponse::channel(200, Some("text/event-stream"), 4);
        tokio::spawn(async move {
            for chunk in ["event: a\ndata: ".repeat(50), "event: b\ndata: ".repeat(50)] {
                let _ = writer.send(chunk.into_bytes()).await;
            }
            let _ = writer.finish().await;
        });
        Some(response)
    }
}

/// dechunk のみを行うヘルパ（gunzip はテスト側で別途行う）。chunked
/// framing（サイズ行 + データ + `\r\n` の繰り返し、`0\r\n\r\n` で終端）を
/// パースして連結済み body を返す。
fn dechunk(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = body;
    loop {
        let line_end = rest
            .windows(2)
            .position(|w| w == b"\r\n")
            .expect("チャンクサイズ行の終端がない");
        let size_line = std::str::from_utf8(&rest[..line_end]).unwrap();
        let size = usize::from_str_radix(size_line.trim(), 16).unwrap();
        rest = &rest[line_end + 2..];
        if size == 0 {
            break;
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..]; // データ + 末尾 \r\n を読み飛ばす。
    }
    out
}

/// [`dechunk`] の寛容版。打ち切り（`RecvOutcome::Aborted`）で終端チャンクが
/// 送出されなかった body を対象に、パースできた完全なチャンクだけを連結して
/// 返す（不完全な末尾に遭遇したら panic せず打ち切る）。
fn dechunk_lenient(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(line_end) = rest.windows(2).position(|w| w == b"\r\n") {
        let Ok(size_line) = std::str::from_utf8(&rest[..line_end]) else {
            break;
        };
        let Ok(size) = usize::from_str_radix(size_line.trim(), 16) else {
            break;
        };
        rest = &rest[line_end + 2..];
        if size == 0 {
            break;
        }
        if rest.len() < size + 2 {
            // 末尾チャンクが途中で切れている（打ち切りにより write が
            // 発生しなかった、または部分的にしか届いていない）。
            out.extend_from_slice(&rest[..rest.len().min(size)]);
            break;
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
    out
}

#[tokio::test]
async fn streaming_response_not_compressed_by_default() {
    // `compress_streaming` 未設定（既定 `false`）は `Server::compression`
    // 登録済みでも圧縮を適用しない（`begin_streaming_compression` の設計
    // 判断 1「opt-in」を参照）。後方互換回帰の防止。
    let config = CompressionConfig::builder().min_size(1).build();
    let server = Server::new()
        .handler(StreamingOkHandler)
        .compression(config);

    let request = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "head: {head}");
    assert!(!head.contains("Content-Encoding"), "head: {head}");
    assert!(
        head.contains("Transfer-Encoding: chunked\r\n"),
        "head: {head}"
    );

    // chunked framing: サイズ行（16 進 800 = 2048）+ 生チャンク + 終端。
    let expected_chunk_size = format!("{:x}", "x".repeat(2048).len());
    let body_text = String::from_utf8_lossy(&body);
    assert!(
        body_text.starts_with(&format!("{expected_chunk_size}\r\n")),
        "body: {body_text}"
    );
    assert!(body_text.contains(&"x".repeat(2048)), "body: {body_text}");
    assert!(body_text.ends_with("0\r\n\r\n"), "body: {body_text}");
}

#[tokio::test]
async fn streaming_response_unregistered_compression_stays_identity() {
    // `compression` feature は有効だが `Server::compression` を一切
    // 登録していない構成（既存利用者の大多数が該当）。`prepare_streaming_
    // compression` は `server.compression_config()` が `None` のためフォール
    // スルーし、ストリーミング応答が無改変で届くことを確認する
    // （`streaming_response_not_compressed_by_default` は「登録済みだが
    // compress_streaming が既定 OFF」のケースであり、本テストとは異なる
    // フォールスルー経路を検証する）。
    let server = Server::new().handler(StreamingOkHandler);

    let request = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "head: {head}");
    assert!(!head.contains("Content-Encoding"), "head: {head}");
    assert!(!head.contains("Vary"), "head: {head}");
    let raw_body = dechunk(&body);
    assert_eq!(raw_body, "x".repeat(2048).into_bytes());
}

#[tokio::test]
async fn streaming_response_compress_streaming_enabled_roundtrips_multi_chunk() {
    // イシュー #461: `compress_streaming(true)` を明示登録すると、chunked
    // ストリーミング応答が gzip 圧縮される。dechunk 後の生バイト列を
    // gunzip すると producer が送出した全チャンクの連結と一致することを
    // 確認する（応答同一性、body 全体はバッファリングしない設計）。
    let config = CompressionConfig::builder()
        .min_size(1)
        .compress_streaming(true)
        .build();
    let server = Server::new()
        .handler(StreamingMultiChunkHandler)
        .compression(config);

    let request = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "head: {head}");
    assert!(head.contains("Content-Encoding: gzip\r\n"), "head: {head}");
    assert!(head.contains("Vary: Accept-Encoding\r\n"), "head: {head}");
    assert!(
        head.contains("Transfer-Encoding: chunked\r\n"),
        "head: {head}"
    );

    let compressed = dechunk(&body);
    let mut decoder = flate2::read::GzDecoder::new(compressed.as_slice());
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).unwrap();
    let expected = "event: a\ndata: ".repeat(50) + &"event: b\ndata: ".repeat(50);
    assert_eq!(decoded, expected);
}

#[cfg(feature = "cors")]
#[tokio::test]
async fn streaming_response_cors_and_compress_streaming_both_apply_vary_tokens() {
    // イシュー #461 レビュー指摘の回帰確認（ストリーミング経路）:
    // `Server::cors` + `Server::compression(compress_streaming(true))` を
    // 併用し、許可 Origin 付きリクエストがストリーミング応答に来た場合でも
    // `Vary: Origin`（CORS、`finalize_streaming_head`）と
    // `Vary: Accept-Encoding`（圧縮、`prepare_streaming_compression`）の
    // 両方が別ヘッダ行として付与されること。修正前は圧縮側が
    // `response.header("vary").is_none()` で早期スキップし、
    // `Accept-Encoding` が欠落していた。
    use fandhe_backend_plugin_cors::CorsConfig;

    let cors_config = CorsConfig::builder()
        .allow_origin("https://app.example.com")
        .build()
        .unwrap();
    let compression_config = CompressionConfig::builder()
        .min_size(1)
        .compress_streaming(true)
        .build();
    let server = Server::new()
        .handler(StreamingMultiChunkHandler)
        .cors(cors_config)
        .compression(compression_config);

    let request = b"GET / HTTP/1.1\r\nOrigin: https://app.example.com\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "head: {head}");
    assert!(
        head.contains("Access-Control-Allow-Origin: https://app.example.com\r\n"),
        "head: {head}"
    );
    assert!(head.contains("Content-Encoding: gzip\r\n"), "head: {head}");
    assert!(head.contains("Vary: Origin\r\n"), "head: {head}");
    assert!(head.contains("Vary: Accept-Encoding\r\n"), "head: {head}");

    let compressed = dechunk(&body);
    let mut decoder = flate2::read::GzDecoder::new(compressed.as_slice());
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).unwrap();
    let expected = "event: a\ndata: ".repeat(50) + &"event: b\ndata: ".repeat(50);
    assert_eq!(decoded, expected);
}

#[tokio::test]
async fn streaming_response_compress_streaming_enabled_skips_without_accept_encoding() {
    // `compress_streaming(true)` でも Accept-Encoding が gzip を受理しない
    // 場合は identity のまま（`begin_streaming_compression` 条件 (e)）。
    let config = CompressionConfig::builder()
        .min_size(1)
        .compress_streaming(true)
        .build();
    let server = Server::new()
        .handler(StreamingOkHandler)
        .compression(config);

    let request = b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "head: {head}");
    assert!(!head.contains("Content-Encoding"), "head: {head}");
    // Content-Type 一致は満たすため Vary は付与される。
    assert!(head.contains("Vary: Accept-Encoding\r\n"), "head: {head}");

    let raw_body = dechunk(&body);
    assert_eq!(raw_body, "x".repeat(2048).into_bytes());
}

#[tokio::test]
async fn streaming_response_compress_streaming_enabled_skips_non_matching_content_type() {
    let config = CompressionConfig::builder()
        .min_size(1)
        .compress_streaming(true)
        .build();

    struct StreamingImageHandler;
    impl Handler for StreamingImageHandler {
        fn handle(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> fandhe_backend_routes::HandlerFuture {
            Box::pin(std::future::ready(Response::empty(599)))
        }

        fn handle_streaming(&self, _head: &RequestHead, _body: &[u8]) -> Option<StreamingResponse> {
            let (response, writer) = StreamingResponse::channel(200, Some("image/png"), 4);
            tokio::spawn(async move {
                let _ = writer.send(vec![0u8; 2048]).await;
                let _ = writer.finish().await;
            });
            Some(response)
        }
    }

    let server = Server::new()
        .handler(StreamingImageHandler)
        .compression(config);

    let request = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(!head.contains("Content-Encoding"), "head: {head}");
    assert!(!head.contains("Vary"), "head: {head}");
    let raw_body = dechunk(&body);
    assert_eq!(raw_body, vec![0u8; 2048]);
}

#[tokio::test]
async fn streaming_response_compress_streaming_enabled_aborted_closes_without_trailer() {
    // producer が `finish` を呼ばずに drop（打ち切り）した場合、gzip
    // trailer・chunked 終端チャンクのどちらも送出せず接続がクローズされる
    // ことを確認する（応答完全性契約の維持、`crate::streaming` モジュール
    // doc・`crates/plugin-compression` crate doc「エンコーダ失敗時は
    // 接続クローズ」節と同根の fail-closed 原則）。
    struct AbortingHandler;
    impl Handler for AbortingHandler {
        fn handle(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> fandhe_backend_routes::HandlerFuture {
            Box::pin(std::future::ready(Response::empty(599)))
        }

        fn handle_streaming(&self, _head: &RequestHead, _body: &[u8]) -> Option<StreamingResponse> {
            let (response, writer) = StreamingResponse::channel(200, Some("text/plain"), 4);
            tokio::spawn(async move {
                let _ = writer.send("partial-data".repeat(100).into_bytes()).await;
                // `finish` を呼ばず drop する（打ち切り）。
            });
            Some(response)
        }
    }

    let config = CompressionConfig::builder()
        .min_size(1)
        .compress_streaming(true)
        .build();
    let server = Server::new().handler(AbortingHandler).compression(config);

    let request = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "head: {head}");
    assert!(head.contains("Content-Encoding: gzip\r\n"), "head: {head}");
    // 終端チャンク（`0\r\n\r\n`）が送出されていないこと。
    assert!(!body.ends_with(b"0\r\n\r\n"), "body: {body:?}");

    // 圧縮確定時の打ち切りは、単に終端チャンクが欠けるだけでなく gzip
    // trailer（CRC32・展開後長）も欠けるため、届いたバイト列は「不完全な
    // gzip ストリーム」でなければならない。sync flush 済みチャンクを
    // `GzDecoder::read_to_end` に通すと、trailer 欠如により必ずエラーで
    // 終わることを確認する（trailer が送出されていた場合はこのアサーションが
    // 失敗し、打ち切り検出の回帰を検知できる）。
    let compressed = dechunk_lenient(&body);
    assert!(!compressed.is_empty(), "圧縮済みデータが届いていない");
    let mut decoder = flate2::read::GzDecoder::new(compressed.as_slice());
    let mut decoded = String::new();
    let result = decoder.read_to_string(&mut decoded);
    assert!(
        result.is_err(),
        "trailer 欠如の不完全な gzip ストリームのはずが正常終了した: {decoded:?}"
    );
    // trailer 到達前にエラーになるまでにデコードできたバイト列は、
    // producer が送出した平文の先頭部分と一致するはず。
    assert!(
        "partial-data".repeat(100).starts_with(&decoded),
        "decoded: {decoded:?}"
    );
}

#[tokio::test]
async fn streaming_response_identity_aborted_closes_without_terminator() {
    // 上のテストの陰性対照（圧縮未確定 = identity 経路）。打ち切り時に
    // 終端チャンクが送出されないことは既存挙動（イシュー #319）のままで
    // あり、`StreamingBodyEncoder` の追加（イシュー #461）が identity 経路の
    // 打ち切り挙動を変えていないことを確認する。
    struct AbortingHandler;
    impl Handler for AbortingHandler {
        fn handle(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> fandhe_backend_routes::HandlerFuture {
            Box::pin(std::future::ready(Response::empty(599)))
        }

        fn handle_streaming(&self, _head: &RequestHead, _body: &[u8]) -> Option<StreamingResponse> {
            let (response, writer) = StreamingResponse::channel(200, Some("text/plain"), 4);
            tokio::spawn(async move {
                let _ = writer.send("partial-data".repeat(100).into_bytes()).await;
                // `finish` を呼ばず drop する（打ち切り）。
            });
            Some(response)
        }
    }

    // `compress_streaming` 未設定（既定 OFF）で identity 経路を通す。
    let server = Server::new().handler(AbortingHandler);

    let request = b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "head: {head}");
    assert!(!head.contains("Content-Encoding"), "head: {head}");
    assert!(!body.ends_with(b"0\r\n\r\n"), "body: {body:?}");
    // identity 経路では届いたバイト列がそのまま平文の先頭部分と一致する
    // （変換されていないことの確認）。
    let raw_body = dechunk_lenient(&body);
    assert!(
        "partial-data"
            .repeat(100)
            .starts_with(std::str::from_utf8(&raw_body).unwrap_or_default()),
        "body: {raw_body:?}"
    );
}

#[tokio::test]
async fn streaming_response_compress_streaming_enabled_map_response_204_skips_body() {
    // `Interceptor::map_response` でステータスを 204 へ書き換えた場合、
    // `begin_streaming_compression` は bodyless 判定（条件 (b)）で圧縮を
    // 適用せず、body 送出ループにも入らない（`is_bodyless_status` との
    // 整合、レスポンス分割対策の既存パターンを踏襲）。
    struct ForceNoContent;
    impl Interceptor for ForceNoContent {
        fn name(&self) -> &'static str {
            "force-no-content"
        }

        fn map_response(&self, _head: &RequestHead, _response: Response) -> Response {
            Response::empty(204).with_content_type("text/plain")
        }
    }

    let config = CompressionConfig::builder()
        .min_size(1)
        .compress_streaming(true)
        .build();
    let server = Server::new()
        .interceptor(ForceNoContent)
        .handler(StreamingOkHandler)
        .compression(config);

    let request = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    assert!(
        head.starts_with("HTTP/1.1 204 No Content\r\n"),
        "head: {head}"
    );
    assert!(!head.contains("Content-Encoding"), "head: {head}");
    assert!(!head.contains("Transfer-Encoding"), "head: {head}");
    // bodyless 判定は Vary 付与判定（条件 (b) の前）より先に early return
    // するため、`apply_compression` の 204 と同じく Vary も付与されない
    // （`begin_streaming_compression` の条件判定順を参照）。
    assert!(!head.contains("Vary"), "head: {head}");
    assert!(body.is_empty(), "body: {body:?}");
}

#[tokio::test]
async fn streaming_response_compress_streaming_enabled_http10_stays_uncompressed() {
    // イシュー #461 の設計判断 4: HTTP/1.0（EOF 終端・フレーミングなし）は
    // ストリーミング圧縮の対象外とし、常に identity のまま送出する
    // （`crates/plugin-compression` crate doc の「HTTP/1.1 chunked 経路
    // のみ対象」節を参照）。
    let config = CompressionConfig::builder()
        .min_size(1)
        .compress_streaming(true)
        .build();
    let server = Server::new()
        .handler(StreamingOkHandler)
        .compression(config);

    let request = b"GET / HTTP/1.0\r\nAccept-Encoding: gzip\r\n\r\n";
    let raw = roundtrip_raw(&server, request).await;
    let (head, body) = split_response(&raw);

    // ステータス行は HTTP バージョンに関わらず "HTTP/1.1" 固定
    // （`Response::serialize_streaming_head_http10` の doc を参照）。
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "head: {head}");
    assert!(!head.contains("Content-Encoding"), "head: {head}");
    assert!(!head.contains("Transfer-Encoding"), "head: {head}");
    assert_eq!(body, "x".repeat(2048).into_bytes());
}
