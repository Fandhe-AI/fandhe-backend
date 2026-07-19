//! ドキュメント用スキーマ型。
//!
//! ここに定義する型は [`crate::docs`] の `#[utoipa::path]` 定義から
//! `request_body` / `responses` の `body` として参照され、
//! `utoipa::OpenApi` の `components.schemas` に登録される。
//!
//! # 実装本体との関係
//! 将来 `crates/routes` 側にエンドポイントの実装本体を追加する際、リクエスト/
//! レスポンス型はここに定義したものと同一のフィールド構成に保つ運用とする
//! （PoC-4 で確認した「ドキュメント専用関数＋薄いスキーマ型」方式、
//! docs/spec/03-poc/openapi-generation/README.md）。本クレートは実装本体に
//! 依存しないため、両者の一致は実装時のレビューで担保する（TASK-3.3、#32 の
//! 齟齬照合スコープ）。

use serde::{Deserialize, Serialize};

/// `POST /echo` のリクエスト/レスポンスボディ。
///
/// # Examples
/// ```
/// use fandhe_backend_plugin_openapi::EchoBody;
///
/// let body = EchoBody { message: "hi".to_string() };
/// let json = serde_json::to_string(&body).unwrap();
/// assert_eq!(json, r#"{"message":"hi"}"#);
/// ```
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EchoBody {
    /// エコー対象のメッセージ本文。
    pub message: String,
}

/// `GET /users/{id}` のレスポンスボディ。
///
/// # Examples
/// ```
/// use fandhe_backend_plugin_openapi::UserResponse;
///
/// let body = UserResponse { id: 42, name: "User 42".to_string() };
/// assert_eq!(body.id, 42);
/// ```
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UserResponse {
    /// リクエストされたユーザー ID。
    pub id: u64,
    /// 表示名。
    pub name: String,
}

/// `GET /search` のレスポンスボディ（クエリパラメータを受け取るエンドポイントの例）。
///
/// # Examples
/// ```
/// use fandhe_backend_plugin_openapi::SearchResponse;
///
/// let body = SearchResponse {
///     query: "rust".to_string(),
///     limit: 10,
///     results: vec!["rust-result-0".to_string()],
/// };
/// assert_eq!(body.limit, 10);
/// ```
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchResponse {
    /// 検索に使ったクエリ文字列（`q`）。
    pub query: String,
    /// 適用された最大件数（`limit`。省略時は既定値 `10`）。
    pub limit: u32,
    /// 検索結果。
    pub results: Vec<String>,
}

/// エラーレスポンス共通ボディ。
///
/// # Examples
/// ```
/// use fandhe_backend_plugin_openapi::ErrorBody;
///
/// let body = ErrorBody { error: "invalid id".to_string() };
/// assert_eq!(body.error, "invalid id");
/// ```
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ErrorBody {
    /// エラー内容を表す人間可読な短い文字列。
    pub error: String,
}
