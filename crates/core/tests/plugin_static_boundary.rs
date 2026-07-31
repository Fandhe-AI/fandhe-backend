//! `static` feature（イシュー #318）配線の統合テスト（feature 有効側)。
//!
//! `crates/core/src/plugin.rs` の非公開 `try_intercept` シームが
//! `Server::static_files(config)` で明示登録済みの場合のみ `config.mount()`
//! プレフィックス配下の `GET` リクエストへ
//! `fandhe_backend_plugin_static::try_handle_static` を委譲し、既定
//! `Handler` より先にインターセプトされることを `tokio::io::duplex` で
//! 駆動する `handle_connection` を通して検証する。`graphql`・`openapi` と
//! 同じ「設定登録型」パターンのため、**未登録時は feature が有効でもフォー
//! ルスルー（既定 `Handler` へ、未登録なら 404）する**ことも併せて確認する。
//!
//! パストラバーサル拒否のエンドツーエンド確認（受け入れ基準 1・4）も含む
//! （プラグイン内部のより網羅的な拒否ケースは
//! `crates/plugin-static/src/lib.rs` の `#[cfg(test)] mod tests` を参照）。
//!
//! feature 無効時の陰性対照は `plugin_static_boundary_disabled.rs` を参照。

#![cfg(feature = "static")]

use fandhe_backend_core::interceptor::Interceptor;
use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_static::StaticFilesConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `Handler::handle` が呼ばれたら panic するトイハンドラ。
///
/// `plugin::try_intercept` が `Some` を返した場合は既定 `Handler` を呼ばない
/// 契約（`crates/core/src/server.rs` の `handle_connection` を参照）の証跡に使う。
struct NotCalledHandler;
impl Handler for NotCalledHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        panic!("plugin::try_intercept が Some を返したのに既定 Handler が呼ばれた");
    }
}

/// 固定 200 応答を返すだけのトイハンドラ（フォールスルー確認用）。
struct FixedOkHandler;
impl Handler for FixedOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        Box::pin(std::future::ready(Response::new(200, b"ok".to_vec())))
    }
}

/// テスト専用の一意な一時ディレクトリ（std のみ、`Drop` で自動削除）。
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "fandhe-core-plugin-static-boundary-{label}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, rel: &str, contents: &[u8]) {
        std::fs::write(self.0.join(rel), contents).unwrap();
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
async fn registered_static_serves_file_and_bypasses_default_handler() {
    let dir = TempDir::new("serves");
    dir.write("app.js", b"console.log('hi')");
    let config = StaticFilesConfig::builder("/static", dir.path())
        .build()
        .unwrap();
    let server = Server::new().handler(NotCalledHandler).static_files(config);

    let request = b"GET /static/app.js HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);

    // PoC-9 教訓: ステータスのみでなく Content-Type・Content-Length・body
    // 全件を検証する（`crates/core/tests/plugin_openapi_boundary.rs` と同一原則）。
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("Content-Type: text/javascript; charset=utf-8\r\n"));
    assert!(response.contains("X-Content-Type-Options: nosniff\r\n"));
    assert!(response.contains("Content-Length: 17\r\n"));
    assert!(response.ends_with("console.log('hi')"));
}

#[tokio::test]
async fn registered_static_serves_directory_index_with_trailing_slash() {
    // 末尾スラッシュ付きディレクトリ URL（`/static/docs/`）が index.html を
    // 解決してコア配線経由で 200 を返すことを確認する（イシュー #418。
    // より網羅的な拒否・許可ケースは `crates/plugin-static/src/lib.rs` の
    // ユニットテストを参照）。
    let dir = TempDir::new("trailing-slash");
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::write(dir.path().join("docs/index.html"), b"<h1>docs</h1>").unwrap();
    let config = StaticFilesConfig::builder("/static", dir.path())
        .build()
        .unwrap();
    let server = Server::new().handler(NotCalledHandler).static_files(config);

    let request = b"GET /static/docs/ HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("<h1>docs</h1>"));
}

#[tokio::test]
async fn registered_static_traversal_attempt_returns_404() {
    let dir = TempDir::new("traversal");
    dir.write("index.html", b"hi");
    let config = StaticFilesConfig::builder("/static", dir.path())
        .build()
        .unwrap();
    let server = Server::new().handler(NotCalledHandler).static_files(config);

    let request = b"GET /static/../Cargo.toml HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}

#[tokio::test]
async fn unregistered_static_falls_through_to_default_handler() {
    // `static` feature は有効だが `Server::static_files` を呼んでいない構成。
    // `graphql`・`openapi` と同じ設定登録型パターンにより、未登録時は
    // 既定 `Handler`（未登録時 404）へフォールスルーする
    // （`crates/core/src/plugin.rs` の doc を参照）。
    let server = Server::new();

    let request = b"GET /static/app.js HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}

#[tokio::test]
async fn wrong_method_falls_through_to_default_handler() {
    let dir = TempDir::new("wrong-method");
    dir.write("app.js", b"x");
    let config = StaticFilesConfig::builder("/static", dir.path())
        .build()
        .unwrap();
    let server = Server::new().handler(FixedOkHandler).static_files(config);

    let request = b"POST /static/app.js HTTP/1.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);
    // POST はインターセプト対象外のため、既定 Handler（FixedOkHandler）が応答する。
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("ok"));
}

#[tokio::test]
async fn unrelated_path_falls_through_to_default_handler() {
    let dir = TempDir::new("unrelated");
    dir.write("app.js", b"x");
    let config = StaticFilesConfig::builder("/static", dir.path())
        .build()
        .unwrap();
    let server = Server::new().handler(FixedOkHandler).static_files(config);

    let response = roundtrip(
        &server,
        b"GET /api/todos HTTP/1.1\r\nConnection: close\r\n\r\n",
    )
    .await;
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("ok"));
}

#[tokio::test]
async fn config_built_via_core_reexport_serves_file() {
    // イシュー #421: `fandhe_backend_core::plugin_static::StaticFilesConfig`
    // （プラグインクレートへの直接依存を追加しない再エクスポート経路）
    // 経由で構築した設定でも、直接依存経路（上の各テスト）と同一の配線・
    // 応答になることを確認する。
    let dir = TempDir::new("reexport");
    dir.write("app.js", b"console.log('re-export')");
    let config =
        fandhe_backend_core::plugin_static::StaticFilesConfig::builder("/static", dir.path())
            .build()
            .unwrap();
    let server = Server::new().handler(NotCalledHandler).static_files(config);

    let request = b"GET /static/app.js HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("console.log('re-export')"));
}

/// `/static/` （末尾スラッシュ）を `/static`（末尾なし）へ 301 で正規化する
/// トイ `Interceptor`。イシュー #420 の想定ユースケース（末尾スラッシュ正規化）。
struct TrailingSlashRedirect;
impl Interceptor for TrailingSlashRedirect {
    fn name(&self) -> &'static str {
        "trailing-slash-redirect"
    }

    fn intercept(&self, head: &RequestHead, _body: &[u8]) -> Option<Response> {
        let path = head.path();
        if path.len() > 1 && path.ends_with('/') {
            Response::redirect(301, path.trim_end_matches('/').to_string()).ok()
        } else {
            None
        }
    }
}

/// `Interceptor::intercept`（イシュー #420）が `plugin::try_intercept`
/// （`static` feature）より先に評価され、static mount 配下でも先取りできる
/// ことを確認する（計画の「plugin_static_boundary.rs へ intercept が static
/// 配信を先取りするケースを追加」に対応）。
#[tokio::test]
async fn interceptor_intercept_takes_priority_over_static_mount() {
    let dir = TempDir::new("intercept-priority");
    dir.write("index.html", b"hi");
    let config = StaticFilesConfig::builder("/static", dir.path())
        .build()
        .unwrap();
    let server = Server::new()
        .interceptor(TrailingSlashRedirect)
        .handler(NotCalledHandler)
        .static_files(config);

    let request = b"GET /static/ HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);

    assert!(response.starts_with("HTTP/1.1 301"), "response: {response}");
    assert!(
        response.contains("Location: /static\r\n"),
        "response: {response}"
    );
}

/// static mount の一律 404 body を `Interceptor::map_response`（イシュー #420）
/// で差し替えられることを確認する（計画の「map_response が static の 404 body
/// を差し替えるケースを追加」に対応）。
struct Custom404Page;
impl Interceptor for Custom404Page {
    fn name(&self) -> &'static str {
        "custom-404-page"
    }

    fn map_response(&self, _head: &RequestHead, response: Response) -> Response {
        if response.status == 404 {
            Response::new(404, b"<html>custom static 404</html>".to_vec())
        } else {
            response
        }
    }
}

#[tokio::test]
async fn interceptor_map_response_rewrites_static_404_body() {
    let dir = TempDir::new("map-response-404");
    dir.write("index.html", b"hi");
    let config = StaticFilesConfig::builder("/static", dir.path())
        .build()
        .unwrap();
    let server = Server::new()
        .interceptor(Custom404Page)
        .handler(NotCalledHandler)
        .static_files(config);

    // 存在しないファイルを要求 → plugin-static が一律 404 を返す
    // （`crates/plugin-static` の doc「未検出・検証失敗・サイズ超過は一律 404」）。
    let request = b"GET /static/missing.js HTTP/1.1\r\nConnection: close\r\n\r\n";
    let response = roundtrip(&server, request).await;
    let response = String::from_utf8_lossy(&response);

    assert!(response.starts_with("HTTP/1.1 404"), "response: {response}");
    assert!(
        response.ends_with("<html>custom static 404</html>"),
        "response: {response}"
    );
}
