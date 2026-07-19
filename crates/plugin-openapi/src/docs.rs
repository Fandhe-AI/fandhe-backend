//! コンパイル時 OpenAPI 定義（TASK-3.1、REQ-3、PoC-4 の採用方式を踏襲）。
//!
//! # 統合方式
//! fandhe-backend のルーティング（`fandhe-backend-routes::Router`）は「method + target
//! 完全一致の関数ベース」であり、axum のような属性マクロで飾れるハンドラ単位を
//! 持たない。このため実装本体とは疎結合な「ドキュメント専用の薄い関数」に
//! `#[utoipa::path(...)]` を付与し、`#[derive(utoipa::OpenApi)]` で 1 つの
//! [`ApiDoc`] に集約する（案 1）。独自の宣言的マクロを自作する案（案 2）は
//! OpenAPI 3.x 仕様準拠の実装コストが大きく不採用とした。判断の詳細は
//! `docs/spec/03-poc/openapi-generation/README.md`（PoC-4）を参照。
//!
//! # 呼び出し元・実行タイミング
//! ここで定義する `*_doc` 関数はコンパイル時のメタデータ収集のみに使われ、
//! 実行時には呼ばれない（本体は空関数）。[`ApiDoc::openapi`] の呼び出しは
//! 開発用の生成 CLI（`gen-openapi`、`gen-cli` feature、TASK-3.2、#31）またはテストからのみ
//! 行い、サーバーのリクエスト処理経路には載せない（実行時コストゼロ、PoC-4 成功基準 3）。
//!
//! # 実装本体との契約
//! 対象 5 エンドポイントの実サービングは `crates/routes` 側の責務であり、本
//! クレートは関知しない。`fandhe-backend-routes::Router` は現時点でパスパラメータ・クエリ
//! 文字列分離を持たない完全一致マッチのため、本モジュールの `path`（例:
//! `/hello/{name}`）と実装側のルーティング定義が一致することは、実装時の
//! レビューで担保する運用とする（実装との齟齬照合は TASK-3.3、#32 のスコープ）。

use crate::schemas::{EchoBody, ErrorBody, SearchResponse, UserResponse};
use utoipa::OpenApi;

/// `GET /health`（ドキュメント専用の宣言。パラメータなし）。
#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "サーバーが正常に稼働している（本文は固定文字列 `OK`）", body = String)
    )
)]
#[allow(
    dead_code,
    reason = "utoipa::path のメタデータ収集専用。実行時には呼ばれない"
)]
fn health_doc() {}

/// `GET /hello/{name}`（ドキュメント専用の宣言。パスパラメータ 1 件）。
#[utoipa::path(
    get,
    path = "/hello/{name}",
    params(
        ("name" = String, Path, description = "挨拶対象の名前")
    ),
    responses(
        (status = 200, description = "挨拶メッセージ（例: `Hello, world!`）", body = String)
    )
)]
#[allow(
    dead_code,
    reason = "utoipa::path のメタデータ収集専用。実行時には呼ばれない"
)]
fn hello_doc() {}

/// `GET /users/{id}`（ドキュメント専用の宣言。パスパラメータ + 400 応答定義）。
#[utoipa::path(
    get,
    path = "/users/{id}",
    params(
        ("id" = u64, Path, description = "ユーザー ID（非負整数）")
    ),
    responses(
        (status = 200, description = "ユーザー情報", body = UserResponse),
        (status = 400, description = "id が非負整数としてパースできない", body = ErrorBody)
    )
)]
#[allow(
    dead_code,
    reason = "utoipa::path のメタデータ収集専用。実行時には呼ばれない"
)]
fn users_doc() {}

/// `POST /echo`（ドキュメント専用の宣言。リクエスト/レスポンス body）。
#[utoipa::path(
    post,
    path = "/echo",
    request_body = EchoBody,
    responses(
        (status = 200, description = "受け取ったメッセージをそのまま返す", body = EchoBody),
        (status = 400, description = "リクエストボディが JSON として不正", body = ErrorBody)
    )
)]
#[allow(
    dead_code,
    reason = "utoipa::path のメタデータ収集専用。実行時には呼ばれない"
)]
fn echo_doc() {}

/// `GET /search`（ドキュメント専用の宣言。クエリパラメータ 2 件）。
#[utoipa::path(
    get,
    path = "/search",
    params(
        ("q" = String, Query, description = "検索クエリ（必須）"),
        ("limit" = Option<u32>, Query, description = "最大件数（省略時 10）")
    ),
    responses(
        (status = 200, description = "検索結果", body = SearchResponse),
        (status = 400, description = "q 未指定、または limit が非負整数としてパースできない", body = ErrorBody)
    )
)]
#[allow(
    dead_code,
    reason = "utoipa::path のメタデータ収集専用。実行時には呼ばれない"
)]
fn search_doc() {}

/// fandhe-backend の全対象エンドポイントを束ねる OpenAPI 定義。
///
/// `ApiDoc::openapi()` で `utoipa::openapi::OpenApi` を構築し、
/// `to_pretty_json()` / `to_yaml()`（`yaml` feature 有効時）でシリアライズできる。
/// 生成 CLI（`gen-openapi`）・`GET /openapi.json` 向けの静的埋め込み
/// （[`crate::OPENAPI_JSON`]）は TASK-3.2（#31）で実装済み。
///
/// # Examples
/// ```
/// use fandhe_backend_plugin_openapi::ApiDoc;
/// use utoipa::OpenApi;
///
/// let doc = ApiDoc::openapi();
/// assert_eq!(doc.paths.paths.len(), 5);
/// ```
#[derive(OpenApi)]
#[openapi(
    info(
        title = "fandhe-backend OpenAPI",
        version = env!("CARGO_PKG_VERSION"),
        description = "TASK-3.1（REQ-3）: ドキュメント専用関数 + utoipa 統合による OpenAPI 自動生成の最小エンドポイント定義"
    ),
    paths(health_doc, hello_doc, users_doc, echo_doc, search_doc),
    components(schemas(EchoBody, UserResponse, SearchResponse, ErrorBody))
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_doc_has_five_paths() {
        // utoipa::OpenApi::openapi() はコンパイル時に構築されたメタデータから
        // 実行時にドキュメント構造体を組み立てるのみで、失敗し得ない（Result を返さない）。
        let doc = ApiDoc::openapi();
        assert_eq!(doc.paths.paths.len(), 5);
    }

    #[test]
    fn api_doc_registers_expected_path_keys() {
        let doc = ApiDoc::openapi();
        for expected in [
            "/health",
            "/hello/{name}",
            "/users/{id}",
            "/echo",
            "/search",
        ] {
            assert!(
                doc.paths.paths.contains_key(expected),
                "path {expected} が登録されていない"
            );
        }
    }

    #[test]
    fn api_doc_registers_four_component_schemas() {
        let doc = ApiDoc::openapi();
        let components = doc.components.expect("components が生成されていない");
        for expected in ["EchoBody", "UserResponse", "SearchResponse", "ErrorBody"] {
            assert!(
                components.schemas.contains_key(expected),
                "schema {expected} が登録されていない"
            );
        }
        assert_eq!(components.schemas.len(), 4);
    }

    #[test]
    fn health_path_has_get_operation_only() {
        let doc = ApiDoc::openapi();
        let item = &doc.paths.paths["/health"];
        assert!(item.get.is_some());
        assert!(item.post.is_none());
    }

    #[test]
    fn echo_path_has_post_operation_only() {
        let doc = ApiDoc::openapi();
        let item = &doc.paths.paths["/echo"];
        assert!(item.post.is_some());
        assert!(item.get.is_none());
    }

    #[test]
    fn users_path_declares_400_response() {
        let doc = ApiDoc::openapi();
        let op = doc.paths.paths["/users/{id}"]
            .get
            .as_ref()
            .expect("GET /users/{id} が定義されていない");
        assert!(op.responses.responses.contains_key("400"));
    }

    #[test]
    fn echo_path_declares_400_response() {
        let doc = ApiDoc::openapi();
        let op = doc.paths.paths["/echo"]
            .post
            .as_ref()
            .expect("POST /echo が定義されていない");
        assert!(op.responses.responses.contains_key("400"));
    }

    #[test]
    fn search_path_declares_400_response() {
        let doc = ApiDoc::openapi();
        let op = doc.paths.paths["/search"]
            .get
            .as_ref()
            .expect("GET /search が定義されていない");
        assert!(op.responses.responses.contains_key("400"));
    }

    #[test]
    fn api_doc_serializes_to_reparsable_json() {
        let doc = ApiDoc::openapi();
        let json = doc.to_pretty_json().expect("JSON シリアライズに失敗した");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("生成された JSON の再パースに失敗した");
        assert_eq!(parsed["paths"].as_object().unwrap().len(), 5);
    }
}
