//! Rocket を横並び性能比較用の参照実装として運用するバイナリ。
//!
//! # このクレートの役割
//!
//! `benches/bench-compare.sh` の計測対象の 1 つ。`crates/axum-ref`（axum、受け入れ判定の
//! baseline）と機能等価な 4 エンドポイント（`GET /health` / `GET /hello/{name}` /
//! `GET /users/{id}` / `POST /echo`）を Rocket で提供し、フレームワーク間の
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
//! 既定バインドはループバック `127.0.0.1:3004`（axum-ref 3001・core-bench 3002・
//! actix-ref 3003 と衝突回避。計測専用バイナリのため外部公開しない、
//! `.claude/rules/security.md`）。
//!
//! # axum-ref との既知の構成差
//!
//! - Rocket は既定で全リクエストをログ出力する。axum-ref / actix-ref / core-bench は
//!   リクエストログを持たないため、公平性のためログレベルを `Critical` に下げる
//!   （stdout への逐次書き込みが RPS を支配してしまうのを避ける）
//! - Rocket の `Config::release_default()` を基点にし、worker 数は既定（論理コア数）の
//!   まま変更しない
//! - 404 応答は Rocket 既定の HTML ページ（axum-ref は空 body）。計測対象 URL に
//!   404 経路は含まれないため比較には影響しない

#[macro_use]
extern crate rocket;

use rocket::config::LogLevel;
use rocket::http::Status;
use rocket::response::status::Custom;
use rocket::serde::json::{self, Json};
use rocket::{Build, Config, Rocket};
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
#[get("/health")]
fn health() -> &'static str {
    "OK"
}

/// 挨拶文字列を返すだけの軽量エンドポイント（RPS 計測のベースライン用）。
#[get("/hello/<name>")]
fn hello(name: &str) -> String {
    format!("Hello, {name}!")
}

/// パスパラメータの数値パースを経る JSON 応答エンドポイント。
///
/// `id` のパース失敗は 400 で拒否する（入力検証、axum-ref と同一の意味論）。
/// Rocket の `u64` パラメータガードは失敗時に 404 へ転送するため、`&str` で受けて
/// 手動パースし 400 を返す。
#[get("/users/<id_str>")]
fn get_user(id_str: &str) -> Result<Json<UserResponse>, Custom<Json<ErrorBody>>> {
    match id_str.parse::<u64>() {
        Ok(id) => Ok(Json(UserResponse {
            id,
            name: format!("User {id}"),
        })),
        Err(_) => Err(Custom(
            Status::BadRequest,
            Json(ErrorBody {
                error: "invalid id".to_string(),
            }),
        )),
    }
}

/// JSON body の受信・再エコーエンドポイント（POST 経路・body 抽出の計測用）。
///
/// 不正 JSON は 400 で拒否する。Rocket の `Json` データガードは失敗時に既定で
/// 400 / 422 を返すが、body スキーマを axum-ref と揃えるため `Result` で受けて
/// 明示的に整形する。
#[post("/echo", data = "<body>")]
fn echo(
    body: Result<Json<EchoBody>, json::Error<'_>>,
) -> Result<Json<EchoBody>, Custom<Json<ErrorBody>>> {
    match body {
        Ok(payload) => Ok(payload),
        Err(_) => Err(Custom(
            Status::BadRequest,
            Json(ErrorBody {
                error: "invalid json body".to_string(),
            }),
        )),
    }
}

/// Rocket インスタンスを構築する。本体（`main`）とテストの両方から呼ばれる共通の組み立て口。
///
/// `addr` は `host:port` 形式。パース失敗は計測用バイナリのため即座に panic で停止する
/// （ライブラリ境界ではないため許容。誤った宛先で黙って起動しない）。
fn build(addr: &str) -> Rocket<Build> {
    let (host, port) = addr
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("rocket-ref: BIND_ADDR must be host:port (got {addr})"));
    let config = Config {
        address: host
            .parse()
            .unwrap_or_else(|e| panic!("rocket-ref: invalid host {host}: {e}")),
        port: port
            .parse()
            .unwrap_or_else(|e| panic!("rocket-ref: invalid port {port}: {e}")),
        log_level: LogLevel::Critical,
        ..Config::release_default()
    };
    rocket::custom(config).mount("/", routes![health, hello, get_user, echo])
}

#[launch]
fn rocket() -> Rocket<Build> {
    // 既定でループバックのみにバインドし、計測専用バイナリを外部公開しない。
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3004".to_string());
    println!("rocket-ref listening on {addr}");
    build(&addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rocket::http::ContentType;
    use rocket::local::blocking::Client;

    fn client() -> Client {
        Client::tracked(build("127.0.0.1:0")).expect("rocket-ref: test client")
    }

    #[test]
    fn routes_health() {
        let c = client();
        let resp = c.get("/health").dispatch();
        assert_eq!(resp.status(), Status::Ok);
    }

    #[test]
    fn routes_users_valid_and_invalid_id() {
        let c = client();
        assert_eq!(c.get("/users/42").dispatch().status(), Status::Ok);
        assert_eq!(c.get("/users/abc").dispatch().status(), Status::BadRequest);
    }

    #[test]
    fn routes_echo_valid_and_invalid_json() {
        let c = client();
        let ok = c
            .post("/echo")
            .header(ContentType::JSON)
            .body(r#"{"message":"hi"}"#)
            .dispatch();
        assert_eq!(ok.status(), Status::Ok);
        let bad = c
            .post("/echo")
            .header(ContentType::JSON)
            .body("not json")
            .dispatch();
        assert_eq!(bad.status(), Status::BadRequest);
    }

    #[test]
    fn routes_unknown_path() {
        let c = client();
        assert_eq!(c.get("/nope").dispatch().status(), Status::NotFound);
    }
}
