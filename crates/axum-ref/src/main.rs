//! axum を性能・フットプリント・攻撃表面の継続比較用参照実装として運用するバイナリ。
//!
//! # このクレートの役割
//!
//! `docs/spec/05-tasks.md` TASK-1.2 の成果物。TASK-1.6（最小コア受け入れテスト）が
//! REQ-1 の axum 比基準（RPS・p95/p99・アイドル RSS・バイナリサイズ・起動時間）を
//! 検証する際の比較対象として、`crates/core` 側のフルスクラッチ実装が揃うまで
//! `benches/*.sh` から起動される。エンドポイント集合・レスポンス形式・エラー応答は
//! `docs/spec/03-poc/fullscratch-performance/axum-ref`（PoC-2）と等価に保ち、
//! 機能等価性を維持したまま axum バージョンのみ現行安定版（0.8 系）へ追随する
//! （PoC は axum 0.7・`/:name` パス構文、本クレートは axum 0.8・`/{name}` 構文）。
//!
//! # workspace 内での位置づけ
//!
//! `crates/core` とは完全に独立したバイナリクレートであり、依存はここで完結する
//! （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。
//! `cargo tree -p backend-framework-core` で axum/tokio 等が現れないことを
//! 検証可能にしておくことが本クレート追加の前提条件。
//!
//! workspace 全体の依存方向規約（依存方向: server → routes → http::*、
//! `docs/spec/04-requirements.md` REQ-1 / `docs/spec/05-tasks.md` TASK-11.1）との関係では、
//! 本クレートはこの依存グラフの**外側**にある独立比較専用バイナリであり、workspace 内
//! path 依存を一切持たない（持ってはならない）。依存方向の機械検証は
//! `scripts/dep-direction-check.sh`（TASK-1.5 / TASK-11.1）が担う。
//!
//! # セキュリティ考慮
//!
//! 計測専用バイナリのため既定バインドは `127.0.0.1:3001`（ループバック限定）とし、
//! 外部公開しない。環境変数 `BIND_ADDR`（`host:port` 形式）でホスト・ポートの
//! 両方を上書き可能。外部公開したい場合は呼び出し側の責任で明示的に指定すること
//! （計測専用バイナリという前提を壊さないよう、通常のベンチ実行では既定値のまま使う）。

use axum::{
    Router,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

#[cfg(feature = "ws")]
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};

/// `POST /echo` のリクエスト/レスポンス body。
///
/// PoC-2 と同一スキーマを維持し、フルスクラッチコアとの比較時に
/// レスポンス形式差を計測ノイズとして持ち込まないようにする。
#[derive(Debug, Serialize, Deserialize)]
struct EchoBody {
    message: String,
}

/// `GET /users/{id}` の正常応答 body。
#[derive(Debug, Serialize)]
struct UserResponse {
    id: u64,
    name: String,
}

/// 異常系（400 Bad Request）の共通エラー body。
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

/// ヘルスチェック用エンドポイント。
///
/// `benches/*.sh` がサーバ起動完了を検知するためにポーリングする対象でもある
/// （起動時間計測・ウォームアップ完了判定の両方で使用）。
async fn health() -> &'static str {
    "OK"
}

/// 挨拶文字列を返すだけの軽量エンドポイント（RPS 計測のベースライン用）。
async fn hello(Path(name): Path<String>) -> String {
    format!("Hello, {name}!")
}

/// パスパラメータの数値パースを経る JSON 応答エンドポイント。
///
/// `id` のパース失敗は 400 で拒否する（入力検証、.claude/rules/security.md）。
async fn get_user(Path(id_str): Path<String>) -> impl IntoResponse {
    match id_str.parse::<u64>() {
        Ok(id) => (
            StatusCode::OK,
            Json(UserResponse {
                id,
                name: format!("User {id}"),
            }),
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                error: "invalid id".to_string(),
            }),
        )
            .into_response(),
    }
}

/// JSON body の受信・再エコーエンドポイント（POST 経路・body 抽出の計測用）。
///
/// 不正 JSON は 400 で拒否する。受信した body はそのまま文字列連結せず
/// serde_json 経由で再シリアライズするのみで、生エコーによるインジェクションを避ける。
async fn echo(
    body: Result<Json<EchoBody>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    match body {
        Ok(Json(payload)) => (StatusCode::OK, Json(payload)).into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                error: "invalid json body".to_string(),
            }),
        )
            .into_response(),
    }
}

/// `GET /ws`（`ws` feature 限定、TASK-4.3 / #24）の WebSocket アップグレード入口。
///
/// `bf-plugin-websocket::session::run_echo_session`（`crates/plugin-websocket`）と
/// 等価な意味論（Text/Binary はそのままエコー、Close 受信でループを終える）に
/// 保つことで、`benches/bench-ws-load.sh` の fullscratch/axum 比較を機能差ではなく
/// 実装差（RSS・CPU）のみに帰属させる。Ping/Pong は axum の内部実装
/// （tokio-tungstenite 由来）が自動応答するため、ここでは明示処理しない
/// （`run_echo_session` の Ping/Pong 無視と同一の建て付け）。
#[cfg(feature = "ws")]
async fn ws_echo(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(ws_echo_session)
}

/// アップグレード成立後のエコーループ本体（`ws_echo` から呼ばれる）。
///
/// 呼び出し元は 1 接続につき 1 タスク（axum が upgrade 済みソケットごとに
/// spawn する）。I/O エラー・Close フレームでループを抜け、接続を閉じる。
#[cfg(feature = "ws")]
async fn ws_echo_session(mut socket: WebSocket) {
    use axum::extract::ws::Message;

    while let Some(Ok(message)) = socket.recv().await {
        match message {
            WsMessage::Text(_) | WsMessage::Binary(_) => {
                if socket.send(message).await.is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            // Ping/Pong は axum 内部（tokio-tungstenite）が自動応答するため無視する
            // （`run_echo_session` と同一方針）。
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }
}

/// axum ルーターを構築する。
///
/// 本体（`main`）とテスト（`tests::routes_*`）の両方から呼ばれる共通の組み立て口。
/// axum 0.8 のパス構文は `{name}` 形式（0.7 以前の `:name` から変更）。
/// `ws` feature 有効時のみ `/ws` route を追加する（既定ビルドは従来どおり）。
fn app() -> Router {
    let router = Router::new()
        .route("/health", get(health))
        .route("/hello/{name}", get(hello))
        .route("/users/{id}", get(get_user))
        .route("/echo", post(echo));
    #[cfg(feature = "ws")]
    let router = router.route("/ws", get(ws_echo));
    router
}

#[tokio::main]
async fn main() {
    // 既定でループバックのみにバインドし、計測専用バイナリを外部公開しない
    // （攻撃表面最小化、.claude/rules/security.md）。
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3001".to_string());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("axum-ref: failed to bind {addr}: {e}"));
    println!("axum-ref listening on {addr}");
    axum::serve(listener, app())
        .await
        .unwrap_or_else(|e| panic!("axum-ref: server error: {e}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn routes_health() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn routes_hello() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/hello/world")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn routes_users_valid_id() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/users/42")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn routes_users_invalid_id() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/users/abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn routes_echo_valid_json() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/echo")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"message":"hi"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn routes_echo_invalid_json() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/echo")
                    .header("content-type", "application/json")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // `ws` feature 有効時のみコンパイルする回帰テスト（feature-flow-check.sh の
    // 「src 変更にはテスト追加必須」要件を満たす。実接続は
    // `benches/bench-ws-load.sh` 側の統合的な確認に委ね、ここでは非 Upgrade
    // リクエストが 400 系で拒否されることのみを検証する軽量なユニットテストに
    // 留める）。
    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn routes_ws_without_upgrade_headers_is_rejected() {
        let response = app()
            .oneshot(Request::builder().uri("/ws").body(Body::empty()).unwrap())
            .await
            .unwrap();
        // axum の WebSocketUpgrade extractor は Upgrade ヘッダ欠落を 400 として拒否する。
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn routes_unknown_path() {
        let response = app()
            .oneshot(Request::builder().uri("/nope").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
