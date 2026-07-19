//! `crates/core` の性能受け入れ計測用サーバ（TASK-1.6-3 / #168）。
//!
//! # このバイナリの役割
//!
//! 親イシュー #15（TASK-1.6 最小コア受け入れテスト）の性能系受け入れ 2 項目
//! （axum-ref との実測比較・NFR-1 起動時間確認）は、`crates/core` 側に
//! `crates/axum-ref`（`crates/axum-ref/src/main.rs`）と機能等価な計測対象
//! サーババイナリが存在しないため未達だった（`benches/bench-accept.sh` が
//! `CORE_BIN` 未検出で BLOCKED 終了）。本 example がその計測対象を提供する。
//!
//! axum-ref と同じ 4 エンドポイント（`GET /health` / `GET /hello/{name}` /
//! `GET /users/{id}` / `POST /echo`）を、`fandhe_backend_core::Handler`
//! （`crates/core/src/server.rs`）を直接実装する形で提供する。`fandhe_backend_routes::Router`
//! は (method, target) の完全一致ディスパッチのみでパスパラメータ
//! （`{name}` / `{id}`）を扱えない（TASK-1.5 / #14 時点の既知の制約）ため、
//! パスパラメータ対応の Router 拡張を待たずに計測できるよう、本 example では
//! プレフィックスマッチによる手書きディスパッチを行う（`fandhe_backend_routes::Router` への
//! パスパラメータ対応は本イシューのスコープ外。out-of-scope-tracking 対象として
//! PR 側に記録する）。
//!
//! # ハーネスとの契約
//!
//! `benches/lib/common.sh` は計測対象バイナリを `BIND_ADDR=host:port` 環境変数で
//! 起動し、`GET /health` が 200 を返した時点で起動完了と判定する（axum-ref と
//! 同一契約、`crates/axum-ref/src/main.rs` の doc を参照）。既定バインドは
//! ループバック `127.0.0.1:3002`（axum-ref の既定 `127.0.0.1:3001` と衝突回避。
//! 計測専用バイナリのため外部公開しない、`.claude/rules/security.md`）。
//!
//! # pay-for-what-you-use（`.claude/rules/pay-for-what-you-use.md`）
//!
//! JSON 処理（`/users/{id}` 応答・`/echo` の JSON パース）には serde/serde_json
//! を使う（手書き JSON パーサは正確性・安全性リスクがあり不採用）。Cargo の
//! example は `[dev-dependencies]` のみを参照するため、`crates/core/Cargo.toml`
//! の `[dev-dependencies]` に追加した serde/serde_json は本体（lib）の依存
//! グラフ・下流クレートに一切波及しない（`cargo tree -p fandhe-backend-core
//! -e normal` に現れないことで検証可能）。
//!
//! # axum-ref との既知の機能差
//!
//! axum の `Path` エクストラクタはパーセントデコードを行うが、本 example は
//! 行わない。計測対象 URL（`/hello/world`・`/users/42`・`POST /echo`）および
//! テスト入力の範囲では機能差が現れないため、計測の公平性を損なわない。
//!
//! # 動作確認手順
//!
//! ```text
//! $ cargo run --example core-bench -p fandhe-backend-core
//! $ curl -v http://127.0.0.1:3002/health
//! $ curl -v http://127.0.0.1:3002/hello/world
//! $ curl -v http://127.0.0.1:3002/users/42
//! $ curl -v http://127.0.0.1:3002/users/abc     # 400
//! $ curl -v -X POST http://127.0.0.1:3002/echo -d '{"message":"hi"}'
//! $ curl -v -X POST http://127.0.0.1:3002/health # 405
//! $ curl -v http://127.0.0.1:3002/missing        # 404
//! ```

use fandhe_backend_core::{Handler, Server};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use serde::{Deserialize, Serialize};

/// `POST /echo` のリクエスト/レスポンス body。`crates/axum-ref/src/main.rs` の
/// `EchoBody` と同一スキーマを維持し、比較対象間でレスポンス形式差を計測ノイズ
/// として持ち込まないようにする。
#[derive(Debug, Serialize, Deserialize)]
struct EchoBody {
    message: String,
}

/// `GET /users/{id}` の正常応答 body（axum-ref `UserResponse` と同一スキーマ）。
#[derive(Debug, Serialize)]
struct UserResponse {
    id: u64,
    name: String,
}

/// 異常系（400 Bad Request）の共通エラー body（axum-ref `ErrorBody` と同一スキーマ）。
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

/// axum-ref と機能等価な 4 エンドポイントをプレフィックスマッチで振り分ける
/// `Handler`（`fandhe_backend_core::server::Handler` 拡張点の実装。
/// モジュール冒頭の doc「このバイナリの役割」を参照）。
struct BenchHandler;

impl BenchHandler {
    /// `prefix` を除いた残りが「空でない・`/` を含まない」単一セグメントで
    /// あることを検証して返す（`/hello/{name}` `/users/{id}` 共通の抽出処理）。
    /// axum の `Path` と異なりパーセントデコードは行わない（モジュール冒頭の
    /// doc「axum-ref との既知の機能差」を参照）。
    fn single_segment_after<'a>(target: &'a str, prefix: &str) -> Option<&'a str> {
        let rest = target.strip_prefix(prefix)?;
        if rest.is_empty() || rest.contains('/') {
            None
        } else {
            Some(rest)
        }
    }

    fn health() -> Response {
        Response::new(200, b"OK".to_vec())
    }

    fn hello(name: &str) -> Response {
        Response::new(200, format!("Hello, {name}!").into_bytes())
    }

    fn users(id_str: &str) -> Response {
        match id_str.parse::<u64>() {
            Ok(id) => {
                let payload = UserResponse {
                    id,
                    name: format!("User {id}"),
                };
                // SAFETY 不要（`unsafe` なし）: serde_json::to_vec は構造体
                // シリアライズのみで失敗しうるのは writer エラー時のみ
                // （`Vec<u8>` writer は失敗しない）。`unwrap_or_else` で
                // ライブラリ境界を越えて panic させない（.claude/rules/coding-rust.md）。
                let body = serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec());
                Response::new(200, body).with_content_type("application/json")
            }
            Err(_) => {
                let payload = ErrorBody {
                    error: "invalid id".to_string(),
                };
                let body = serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec());
                Response::new(400, body).with_content_type("application/json")
            }
        }
    }

    fn echo(body: &[u8]) -> Response {
        match serde_json::from_slice::<EchoBody>(body) {
            Ok(payload) => {
                // 受信 body を生転写せず serde_json 経由で再シリアライズする
                // （レスポンス分割・生エコーによるインジェクション回避、
                // axum-ref `echo` と同一方針。.claude/rules/security.md）。
                let out = serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec());
                Response::new(200, out).with_content_type("application/json")
            }
            Err(_) => {
                let payload = ErrorBody {
                    error: "invalid json body".to_string(),
                };
                let out = serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec());
                Response::new(400, out).with_content_type("application/json")
            }
        }
    }
}

impl Handler for BenchHandler {
    fn handle(&self, head: &RequestHead, body: &[u8]) -> Response {
        let method = head.method.as_str();
        let target = head.target.as_str();

        if target == "/health" {
            return if method == "GET" {
                Self::health()
            } else {
                Response::empty(405)
            };
        }

        if let Some(name) = Self::single_segment_after(target, "/hello/") {
            return if method == "GET" {
                Self::hello(name)
            } else {
                Response::empty(405)
            };
        }

        if let Some(id_str) = Self::single_segment_after(target, "/users/") {
            return if method == "GET" {
                Self::users(id_str)
            } else {
                Response::empty(405)
            };
        }

        if target == "/echo" {
            return if method == "POST" {
                Self::echo(body)
            } else {
                Response::empty(405)
            };
        }

        Response::empty(404)
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // axum-ref（マルチスレッドランタイム、`#[tokio::main]`）と同条件に揃え、
    // ランタイム構成差を計測ノイズにしない（`docs/spec/03-poc` の教訓、
    // `crates/core/Cargo.toml` の `ws_nfr6` example doc も同種の注意を参照）。
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3002".to_string());
    let server = Server::new().handler(BenchHandler);
    let bound = server.bind(&addr).await?;
    println!("core-bench listening on {}", bound.local_addr()?);
    bound.run().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    /// テスト専用: 生の HTTP リクエストヘッドをパースして `RequestHead` を得る。
    /// `fandhe_backend_http::request::parse_request_head`（sans-IO パーサ）に委譲するだけの
    /// 薄いヘルパーで、`BenchHandler::handle` をソケットなしで直接検証できる
    /// ようにする（TASK-1.6-3 / #168 の等価性テストの共通基盤）。
    fn head_of(raw: &str) -> RequestHead {
        match parse_request_head(raw.as_bytes()).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("incomplete request head in test fixture"),
        }
    }

    fn status_of(response: &Response) -> u16 {
        response.status
    }

    fn body_str(response: &Response) -> String {
        String::from_utf8(response.body.clone()).unwrap()
    }

    #[test]
    fn health_returns_200_ok() {
        let head = head_of("GET /health HTTP/1.1\r\n\r\n");
        let response = BenchHandler.handle(&head, b"");
        assert_eq!(status_of(&response), 200);
        assert_eq!(body_str(&response), "OK");
    }

    #[test]
    fn health_wrong_method_returns_405() {
        let head = head_of("POST /health HTTP/1.1\r\n\r\n");
        let response = BenchHandler.handle(&head, b"");
        assert_eq!(status_of(&response), 405);
    }

    #[test]
    fn hello_returns_greeting() {
        let head = head_of("GET /hello/world HTTP/1.1\r\n\r\n");
        let response = BenchHandler.handle(&head, b"");
        assert_eq!(status_of(&response), 200);
        assert_eq!(body_str(&response), "Hello, world!");
    }

    #[test]
    fn users_valid_id_returns_200_json() {
        let head = head_of("GET /users/42 HTTP/1.1\r\n\r\n");
        let response = BenchHandler.handle(&head, b"");
        assert_eq!(status_of(&response), 200);
        let parsed: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(parsed["id"], 42);
        assert_eq!(parsed["name"], "User 42");
    }

    #[test]
    fn users_invalid_id_returns_400_json() {
        let head = head_of("GET /users/abc HTTP/1.1\r\n\r\n");
        let response = BenchHandler.handle(&head, b"");
        assert_eq!(status_of(&response), 400);
        let parsed: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(parsed["error"], "invalid id");
    }

    #[test]
    fn echo_valid_json_roundtrips() {
        let head = head_of("POST /echo HTTP/1.1\r\n\r\n");
        let response = BenchHandler.handle(&head, br#"{"message":"hi"}"#);
        assert_eq!(status_of(&response), 200);
        let parsed: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(parsed["message"], "hi");
    }

    #[test]
    fn echo_invalid_json_returns_400() {
        let head = head_of("POST /echo HTTP/1.1\r\n\r\n");
        let response = BenchHandler.handle(&head, b"not json");
        assert_eq!(status_of(&response), 400);
        let parsed: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(parsed["error"], "invalid json body");
    }

    #[test]
    fn unknown_path_returns_404() {
        let head = head_of("GET /nope HTTP/1.1\r\n\r\n");
        let response = BenchHandler.handle(&head, b"");
        assert_eq!(status_of(&response), 404);
    }
}
