//! `openapi` feature（TASK-2.1 / #256）配線の統合テスト（feature 有効側）。
//!
//! `crates/core/src/plugin.rs` の非公開 `try_intercept` シームが `Server::openapi()`
//! で明示登録済みの場合のみ `GET /openapi.json` へ
//! `fandhe_backend_plugin_openapi::OPENAPI_JSON`（コンパイル時埋め込みの静的
//! JSON）を `Content-Type: application/json` で、`GET /openapi.yaml`（#279）へ
//! `OPENAPI_YAML` を `Content-Type: application/yaml` で返し、既定 `Handler` より
//! 先にインターセプトされることを、`tokio::io::duplex` で駆動する
//! `handle_connection` を通して検証する。`webrtc-proxy`・`graphql` と同じ
//! 「設定登録型」パターンのため、**未登録時は feature が有効でもフォール
//! スルー（404）する**ことも併せて確認する（`Server::openapi` の doc・
//! `crates/plugin-openapi/src/embed.rs` の接続契約、
//! `crates/core/tests/plugin_graphql_boundary.rs` と同型のパターン）。
//!
//! feature 無効時の陰性対照は `plugin_openapi_boundary_disabled.rs` を参照。

#![cfg(feature = "openapi")]

use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_openapi::{OPENAPI_JSON, OPENAPI_YAML};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `Handler::handle` が呼ばれたら panic するトイハンドラ。
///
/// `plugin::try_intercept` が `Some` を返した場合は既定 `Handler` を呼ばない
/// 契約（`crates/core/src/server.rs` の `handle_connection` を参照）の証跡に使う。
struct NotCalledHandler;
impl Handler for NotCalledHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        panic!("plugin::try_intercept が Some を返したのに既定 Handler が呼ばれた");
    }
}

/// 固定 200 応答を返すだけのトイハンドラ（フォールスルー確認用）。
struct FixedOkHandler;
impl Handler for FixedOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        Response::new(200, b"ok".to_vec())
    }
}

async fn roundtrip(server: &Server, request: &[u8]) -> Vec<u8> {
    let (mut client, server_stream) = tokio::io::duplex(1 << 20);
    client.write_all(request).await.unwrap();
    client.shutdown().await.unwrap();

    handle_connection(server, server_stream).await;

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    out
}

#[tokio::test]
async fn registered_openapi_serves_embedded_json_and_bypasses_default_handler() {
    let server = Server::new().handler(NotCalledHandler).openapi();

    let request = b"GET /openapi.json HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);

    // ステータス・Content-Type・Content-Length・body の全件を検証する（PoC-9
    // 教訓: ステータスのみの検証は reason/Content-Type/body の劣化を見逃す。
    // AGENTS.md「アサーション網羅性」節が HTTP レスポンステストで少なくとも
    // Content-Type / Content-Length の検証を必須としているため、
    // `Response::new` が `body.len()` から算出する Content-Length も検証する）。
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("Content-Type: application/json\r\n"));
    assert!(response.contains(&format!("Content-Length: {}\r\n", OPENAPI_JSON.len())));
    assert!(response.ends_with(OPENAPI_JSON));
}

#[tokio::test]
async fn registered_openapi_serves_embedded_yaml_and_bypasses_default_handler() {
    // JSON 側と同じ opt-in トグル（`Server::openapi()`）を共有する（#279、
    // `Server::openapi` の doc を参照）。
    let server = Server::new().handler(NotCalledHandler).openapi();

    let request = b"GET /openapi.yaml HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("Content-Type: application/yaml\r\n"));
    assert!(response.contains(&format!("Content-Length: {}\r\n", OPENAPI_YAML.len())));
    assert!(response.ends_with(OPENAPI_YAML));
}

#[tokio::test]
async fn unregistered_openapi_yaml_falls_through_to_404() {
    // json 側と同じ設定登録型パターン: `Server::openapi` 未登録時は feature が
    // 有効でも既定 `Handler`（未登録時 404）へフォールスルーする。
    let server = Server::new();

    let request = b"GET /openapi.yaml HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}

#[tokio::test]
async fn wrong_method_openapi_yaml_falls_through_to_404() {
    let server = Server::new().openapi();

    let request = b"POST /openapi.yaml HTTP/1.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}

#[tokio::test]
async fn unregistered_openapi_falls_through_to_404() {
    // `openapi` feature は有効だが `Server::openapi` を呼んでいない構成。
    // `webrtc-proxy`・`graphql` と同じ設定登録型パターンにより、未登録時は
    // 既定 `Handler`（未登録時 404）へフォールスルーする
    // （`crates/core/src/plugin.rs` の doc を参照）。
    let server = Server::new();

    let request = b"GET /openapi.json HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}

#[tokio::test]
async fn wrong_method_falls_through_to_404() {
    // メソッド不一致（POST）はパスが一致していてもインターセプトしない
    // （`server.method == "GET"` の完全一致条件、`crates/core/src/plugin.rs`
    // の該当分岐を参照）。
    let server = Server::new().openapi();

    let request = b"POST /openapi.json HTTP/1.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}

#[tokio::test]
async fn unrelated_path_falls_through_to_default_handler() {
    let server = Server::new().handler(FixedOkHandler).openapi();

    let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("ok"));
}
