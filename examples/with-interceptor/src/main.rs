//! コア拡張点 `Interceptor`（リダイレクト・レスポンス改変、イシュー #420）
//! だけを見せる最小サンプル。
//!
//! `crates/core/src/interceptor.rs` のモジュール doc が定義する 2 フックを、
//! それぞれ片方のみ実装する 2 つのトイ `Interceptor` として示す（「片方のみ
//! オーバーライドできる」流儀の実演、`Interceptor` の doc と同一原則）。
//! 独立して `cargo run` できる standalone crate として切り出している
//! （Next.js の `examples/` 方式、`examples/README.md` 参照）。
//!
//! 1. [`TrailingSlashRedirect`]（`intercept` のみ実装）: 末尾スラッシュ付き
//!    パスを 301 で正規化する
//! 2. [`SecurityHeaders`]（`map_response` のみ実装）: 全応答へ
//!    `X-Content-Type-Options` / `X-Frame-Options` を付与する
//!
//! # 評価順序（`crate::interceptor` モジュール doc、要点のみ）
//!
//! `Interceptor::intercept` は `RequestGate`（フェイルクローズ既定拒否）・
//! `UpgradeHandler` の**後**、`plugin::try_intercept`・`Handler` の**前**に
//! 評価される。よって `RequestGate` の拒否応答を Interceptor で迂回すること
//! はできない（A01 アクセス制御対策）。`map_response` は最終応答確定後・
//! `finalize_response`（CORS → 圧縮）**前**に逐次適用され、`Handler` の
//! フォールバック応答（404 等）にも及ぶ。Interceptor は feature ゲート不要の
//! 純コア機能（`Interceptor` モジュール doc の pay-for-what-you-use 節）。
//!
//! # 起動方法
//!
//! ```text
//! $ cd examples/with-interceptor
//! $ cargo run
//! ```
//!
//! 既定で `127.0.0.1:3000` に bind する（`PORT` 環境変数で上書き可能）。
//!
//! # 動作確認手順
//!
//! ```text
//! # 通常応答（200 + セキュリティヘッダを確認）
//! $ curl -si http://127.0.0.1:3000/hello
//!
//! # 末尾スラッシュの正規化（301 + Location: /hello、かつセキュリティヘッダも確認）
//! $ curl -si http://127.0.0.1:3000/hello/
//!
//! # クエリ付きの正規化（Location: /hello?q=1 を確認）
//! $ curl -si "http://127.0.0.1:3000/hello/?q=1"
//!
//! # 未登録パス（404 + セキュリティヘッダを確認、map_response が Handler の
//! # フォールバック応答にも及ぶことの実演）
//! $ curl -si http://127.0.0.1:3000/missing
//! ```

use fandhe_backend_core::{Interceptor, Server};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::Router;

/// 末尾スラッシュ付きパスを 301 で正規化するトイ実装（`intercept` のみ実装）。
///
/// `Interceptor::intercept` の doc test（`crates/core/src/interceptor.rs`）を
/// 土台に、クエリ文字列の保存を追加している。`Location` はリクエストパス・
/// クエリのみから構成し、外部由来のホスト・スキームを一切含めない
/// （オープンリダイレクト対策、`.claude/rules/security.md` A01/A03 観点）。
/// パス・クエリは `parse_request_head` 検証済みで CR/LF を含み得ないため、
/// ヘッダインジェクションも成立しない。
struct TrailingSlashRedirect;

impl Interceptor for TrailingSlashRedirect {
    fn name(&self) -> &'static str {
        "trailing-slash-redirect"
    }

    fn intercept(&self, head: &RequestHead, _body: &[u8]) -> Option<Response> {
        let path = head.path();
        // 長さ 1（ルート `/`）は正規化対象から除外する（除外しないと
        // `/` -> `/` への自己リダイレクトでループする）。
        if path.len() <= 1 || !path.ends_with('/') {
            return None;
        }
        let mut location = path.trim_end_matches('/').to_string();
        if let Some(query) = head.query() {
            location.push('?');
            location.push_str(query);
        }
        Response::redirect(301, location).ok()
    }
}

/// 全応答へ最小限のセキュリティヘッダを付与するトイ実装
/// （`map_response` のみ実装）。`Interceptor::intercept` 応答・`Handler` の
/// フォールバック応答（404 等）の両方に及ぶことを動作確認手順で示す。
///
/// ヘッダ名・値は静的な妥当値（tchar のみ・予約名でもない）のため
/// `with_header` の `Result` を `expect` してよい
/// （`examples/with-cors` の `cors_config` と同一流儀）。
struct SecurityHeaders;

impl Interceptor for SecurityHeaders {
    fn name(&self) -> &'static str {
        "security-headers"
    }

    fn map_response(&self, _head: &RequestHead, response: Response) -> Response {
        response
            .with_header("X-Content-Type-Options", "nosniff")
            .expect("静的な妥当ヘッダ値は必ず成功する")
            .with_header("X-Frame-Options", "DENY")
            .expect("静的な妥当ヘッダ値は必ず成功する")
    }
}

/// `GET /hello` のみを持つ最小 [`Router`]（`main` とテストの両方から共有する
/// ため関数として切り出す、`examples/with-cors/src/main.rs` と同一パターン）。
/// Interceptor は `Router` の責務外（`Server` 層の拡張点）のため、ここには
/// 一切配線しない（`main` 側で `Server::interceptor` を登録する）。
fn build_router() -> Router {
    Router::new().route("GET", "/hello", |_head, _body| {
        Response::new(
            200,
            b"fandhe-backend-example-with-interceptor: try /hello/ or /missing\n".to_vec(),
        )
    })
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> std::io::Result<()> {
    let server = Server::new()
        .handler(build_router())
        // 登録順に評価される（`Interceptor` モジュール doc）。
        // TrailingSlashRedirect（intercept）→ SecurityHeaders（map_response）の
        // 順で登録しているが、両者は別フックのため実行順には影響しない。
        .interceptor(TrailingSlashRedirect)
        .interceptor(SecurityHeaders);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("127.0.0.1:{port}");
    let bound = server.bind(&addr).await?;
    println!("fandhe-backend-example-with-interceptor listening on {addr}");
    bound
        .run_until(async {
            // 登録失敗を握りつぶすと future が即完了し bind 直後にサーバが
            // 終了してしまうため、シグナルハンドラを登録できない環境では
            // 起動継続せず明示的に panic させる（`examples/with-cors` と同方針）
            tokio::signal::ctrl_c()
                .await
                .expect("Ctrl-C シグナルハンドラの登録に失敗した");
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_core::handle_connection;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// `examples/with-graphql/src/main.rs` と同型のヘルパ。Interceptor は
    /// `Server` 層（`handle_connection`）で評価されるため、`Router::dispatch`
    /// 単体ではなく end-to-end に検証する。
    async fn roundtrip(server: &Server, request: &[u8]) -> String {
        let (mut client, server_stream) = tokio::io::duplex(8192);
        client.write_all(request).await.unwrap();
        client.shutdown().await.unwrap();

        handle_connection(server, server_stream).await;

        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        String::from_utf8(out).unwrap()
    }

    fn new_server() -> Server {
        Server::new()
            .handler(build_router())
            .interceptor(TrailingSlashRedirect)
            .interceptor(SecurityHeaders)
    }

    // PoC-9 教訓に従い、ステータスのみでなくヘッダ・body まで検証する
    // （`crates/core/tests/plugin_graphql_boundary.rs` と同一原則）。

    #[tokio::test]
    async fn normal_response_has_security_headers() {
        let server = new_server();
        let request = b"GET /hello HTTP/1.1\r\nConnection: close\r\n\r\n";

        let response = roundtrip(&server, request).await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("X-Content-Type-Options: nosniff\r\n"));
        assert!(response.contains("X-Frame-Options: DENY\r\n"));
        assert!(response.contains("try /hello/ or /missing"));
    }

    #[tokio::test]
    async fn trailing_slash_is_redirected_with_security_headers() {
        let server = new_server();
        let request = b"GET /hello/ HTTP/1.1\r\nConnection: close\r\n\r\n";

        let response = roundtrip(&server, request).await;

        assert!(response.starts_with("HTTP/1.1 301 Moved Permanently\r\n"));
        assert!(response.contains("Location: /hello\r\n"));
        // intercept 応答にも map_response が適用されることの検証
        // （`crate::interceptor` モジュール doc の評価順序どおり）。
        assert!(response.contains("X-Content-Type-Options: nosniff\r\n"));
    }

    #[tokio::test]
    async fn query_string_is_preserved_across_redirect() {
        let server = new_server();
        let request = b"GET /hello/?q=1 HTTP/1.1\r\nConnection: close\r\n\r\n";

        let response = roundtrip(&server, request).await;

        assert!(response.starts_with("HTTP/1.1 301 Moved Permanently\r\n"));
        assert!(response.contains("Location: /hello?q=1\r\n"));
    }

    #[tokio::test]
    async fn root_path_is_not_redirected() {
        let server = new_server();
        let request = b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n";

        let response = roundtrip(&server, request).await;

        // ルートは正規化対象外（自己リダイレクトのループ防止）。Router に
        // `GET /` は未登録のため 404（かつセキュリティヘッダあり）。
        assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(response.contains("X-Content-Type-Options: nosniff\r\n"));
    }

    #[tokio::test]
    async fn unmatched_path_returns_404_with_security_headers() {
        let server = new_server();
        let request = b"GET /missing HTTP/1.1\r\nConnection: close\r\n\r\n";

        let response = roundtrip(&server, request).await;

        // Handler のフォールバック応答（404）にも map_response が及ぶことの
        // 実演（`crate::interceptor` モジュール doc「評価順序」節）。
        assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(response.contains("X-Content-Type-Options: nosniff\r\n"));
        assert!(response.contains("X-Frame-Options: DENY\r\n"));
    }
}
