//! actix-web を横並び性能比較用の参照実装として運用するバイナリ。
//!
//! # このクレートの役割
//!
//! `benches/bench-compare.sh` の計測対象の 1 つ。`crates/axum-ref`（axum、受け入れ判定の
//! baseline）と機能等価な 4 エンドポイント（`GET /health` / `GET /hello/{name}` /
//! `GET /users/{id}` / `POST /echo`）を actix-web で提供し、フレームワーク間の
//! RPS・レイテンシ・RSS・バイナリサイズを同一ハーネスで比較できるようにする。
//! レスポンス body スキーマ（`EchoBody` / `UserResponse` / `ErrorBody`）・
//! エラー応答（400 / 404）は axum-ref と同一に保つ。
//!
//! 受け入れ判定（`benches/bench-accept.sh`、REQ-1 / NFR-1 / NFR-2）の baseline は
//! axum-ref のままであり、本クレートは判定に関与しない（情報提供目的の比較専用）。
//!
//! # ハーネスとの契約
//!
//! `benches/lib/common.sh` は計測対象を `BIND_ADDR=host:port` 環境変数で起動し、
//! `GET /health` が 200 を返した時点で起動完了と判定する（axum-ref と同一契約）。
//! 既定バインドはループバック `127.0.0.1:3003`（axum-ref 3001・core-bench 3002 と
//! 衝突回避。計測専用バイナリのため外部公開しない、`.claude/rules/security.md`）。
//!
//! # axum-ref との既知の実行モデル差
//!
//! actix-web は worker スレッドごとに単一スレッドのランタイム（actix-rt）を持ち、
//! 接続を worker へ分配する（axum / fandhe-backend の tokio マルチスレッドランタイム
//! とは異なる）。worker 数は既定（論理コア数）のまま変更しない。これはフレームワーク
//! 固有の設計であり、比較では「フレームワーク既定構成どうし」の差として扱う。

use actix_web::{App, HttpResponse, HttpServer, Responder, web};
use serde::{Deserialize, Serialize};

/// `POST /echo` のリクエスト/レスポンス body（axum-ref と同一スキーマ）。
#[derive(Debug, Serialize, Deserialize)]
struct EchoBody {
    message: String,
}

/// `GET /users/{id}` の正常応答 body（axum-ref と同一スキーマ）。
#[derive(Debug, Serialize)]
struct UserResponse {
    id: u64,
    name: String,
}

/// 異常系（400 Bad Request）の共通エラー body（axum-ref と同一スキーマ）。
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

/// ヘルスチェック用エンドポイント。ハーネスの起動完了検知にも使われる。
async fn health() -> &'static str {
    "OK"
}

/// 挨拶文字列を返すだけの軽量エンドポイント（RPS 計測のベースライン用）。
async fn hello(name: web::Path<String>) -> String {
    format!("Hello, {name}!")
}

/// パスパラメータの数値パースを経る JSON 応答エンドポイント。
///
/// `id` のパース失敗は 400 で拒否する（入力検証、axum-ref と同一の意味論）。
async fn get_user(id_str: web::Path<String>) -> impl Responder {
    match id_str.parse::<u64>() {
        Ok(id) => HttpResponse::Ok().json(UserResponse {
            id,
            name: format!("User {id}"),
        }),
        Err(_) => HttpResponse::BadRequest().json(ErrorBody {
            error: "invalid id".to_string(),
        }),
    }
}

/// JSON body の受信・再エコーエンドポイント（POST 経路・body 抽出の計測用）。
///
/// 不正 JSON は 400 で拒否する。actix-web の既定は extractor エラーを 400 として
/// 返すが、body スキーマを axum-ref と揃えるため `Result` で受けて明示的に整形する。
async fn echo(body: Result<web::Json<EchoBody>, actix_web::Error>) -> impl Responder {
    match body {
        Ok(payload) => HttpResponse::Ok().json(payload.into_inner()),
        Err(_) => HttpResponse::BadRequest().json(ErrorBody {
            error: "invalid json body".to_string(),
        }),
    }
}

/// ルート定義。本体（`main`）とテストの両方から呼ばれる共通の組み立て口。
fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health))
        .route("/hello/{name}", web::get().to(hello))
        .route("/users/{id}", web::get().to(get_user))
        .route("/echo", web::post().to(echo));
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // 既定でループバックのみにバインドし、計測専用バイナリを外部公開しない。
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3003".to_string());
    let server = HttpServer::new(|| App::new().configure(configure)).bind(&addr)?;
    println!("actix-ref listening on {addr}");
    server.run().await
}

#[cfg(test)]
mod tests {
    //! axum-ref と同じ 4 エンドポイントの機能等価性を、AGENTS.md「アサーション網羅性」に
    //! 従いステータス・`Content-Type`・ボディの 3 点で検証する（比較条件の担保）。
    use super::*;
    use actix_web::dev::ServiceResponse;
    use actix_web::http::StatusCode;
    use actix_web::http::header::CONTENT_TYPE;
    use actix_web::test;

    /// レスポンスのステータス・`Content-Type`・ボディを一括検証する。
    /// `expected_content_type` が `None` のときは `Content-Type` ヘッダが無いことを検証する。
    async fn assert_response(
        resp: ServiceResponse,
        status: StatusCode,
        expected_content_type: Option<&str>,
        body: &str,
    ) {
        assert_eq!(resp.status(), status);
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .map(|v| v.to_str().expect("content-type is ascii").to_string());
        assert_eq!(content_type.as_deref(), expected_content_type);
        let actual = test::read_body(resp).await;
        assert_eq!(std::str::from_utf8(&actual).expect("utf-8 body"), body);
    }

    #[actix_web::test]
    async fn routes_health() {
        let app = test::init_service(App::new().configure(configure)).await;
        let resp =
            test::call_service(&app, test::TestRequest::get().uri("/health").to_request()).await;
        assert_response(
            resp,
            StatusCode::OK,
            Some("text/plain; charset=utf-8"),
            "OK",
        )
        .await;
    }

    #[actix_web::test]
    async fn routes_hello() {
        let app = test::init_service(App::new().configure(configure)).await;
        let resp = test::call_service(
            &app,
            test::TestRequest::get().uri("/hello/world").to_request(),
        )
        .await;
        assert_response(
            resp,
            StatusCode::OK,
            Some("text/plain; charset=utf-8"),
            "Hello, world!",
        )
        .await;
    }

    #[actix_web::test]
    async fn routes_users_valid_id() {
        let app = test::init_service(App::new().configure(configure)).await;
        let resp =
            test::call_service(&app, test::TestRequest::get().uri("/users/42").to_request()).await;
        assert_response(
            resp,
            StatusCode::OK,
            Some("application/json"),
            r#"{"id":42,"name":"User 42"}"#,
        )
        .await;
    }

    #[actix_web::test]
    async fn routes_users_invalid_id() {
        let app = test::init_service(App::new().configure(configure)).await;
        let resp = test::call_service(
            &app,
            test::TestRequest::get().uri("/users/abc").to_request(),
        )
        .await;
        assert_response(
            resp,
            StatusCode::BAD_REQUEST,
            Some("application/json"),
            r#"{"error":"invalid id"}"#,
        )
        .await;
    }

    #[actix_web::test]
    async fn routes_echo_valid_json() {
        let app = test::init_service(App::new().configure(configure)).await;
        let resp = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/echo")
                .insert_header(("content-type", "application/json"))
                .set_payload(r#"{"message":"hi"}"#)
                .to_request(),
        )
        .await;
        assert_response(
            resp,
            StatusCode::OK,
            Some("application/json"),
            r#"{"message":"hi"}"#,
        )
        .await;
    }

    #[actix_web::test]
    async fn routes_echo_invalid_json() {
        let app = test::init_service(App::new().configure(configure)).await;
        let resp = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/echo")
                .insert_header(("content-type", "application/json"))
                .set_payload("not json")
                .to_request(),
        )
        .await;
        assert_response(
            resp,
            StatusCode::BAD_REQUEST,
            Some("application/json"),
            r#"{"error":"invalid json body"}"#,
        )
        .await;
    }

    /// 未定義パスは 404・空ボディ（actix-web 既定）。axum-ref と同じく `Content-Type` なし。
    #[actix_web::test]
    async fn routes_unknown_path() {
        let app = test::init_service(App::new().configure(configure)).await;
        let resp =
            test::call_service(&app, test::TestRequest::get().uri("/nope").to_request()).await;
        assert_response(resp, StatusCode::NOT_FOUND, None, "").await;
    }
}
