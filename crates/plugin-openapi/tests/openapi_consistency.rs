//! TASK-3.3（#32、REQ-3【Must】）: OpenAPI 自動生成受け入れテストのうち、機械検証可能な範囲を担う。
//!
//! # 検証範囲（このファイルが担保すること）
//! 生成された [`fandhe_backend_plugin_openapi::OPENAPI_JSON`] の構造（path/method/パラメータ名・型・
//! 必須有無・レスポンススキーマ）が [`fandhe_backend_plugin_openapi::ApiDoc`]（`crates/plugin-openapi/src/docs.rs`
//! の `#[utoipa::path]` 宣言）と一致し続けることを、宣言側の変更を伴わない「実体側の
//! ピン留めテスト」として固定する。`docs.rs`・`schemas.rs` の doc comment がそれぞれ想定する
//! 5 エンドポイントの契約（パスパラメータ・クエリパラメータ・リクエスト/レスポンス body・
//! パラメータなしの 4 形態）を openapi.json 実体に対して網羅的にアサートする。
//!
//! # 検証対象外（BLOCKED、docs/acceptance/req3-openapi-generation.md を参照）
//! REQ-3 の受け入れ基準が要求する「生成定義とエンドポイント実装の齟齬 0 件」のうち、
//! **実装側（`crates/routes`）との突合**はこのファイルのスコープ外である。
//! `crates/routes::Router` は method + target の完全一致のみを扱い、本イシュー着手時点
//! （2026-07-17）で `GET /health` 以外の 4 エンドポイント（パスパラメータ・クエリパラメータを
//! 要する `/hello/{name}`・`/users/{id}`・`/search`、および `POST /echo`）の実サービングは
//! `crates/core::examples::minimal` にも `crates/routes` にも存在しない
//! （`docs.rs`・`schemas.rs` の doc comment が明記する既知ギャップ）。このため
//! 「実装との齟齬 0 件」を機械的に確認できるのは `GET /health` の 1 エンドポイントのみに限られ、
//! 残り 4 件は手動突合表（`docs/acceptance/req3-openapi-generation.md`）で BLOCKED として
//! 記録する。実サービング追加はコアのルーティング設計変更（パスパラメータ対応）を伴うため
//! 本テストタスクのスコープ外とし、フォローアップ Issue 化の要否を PR 側で扱う
//! （`.claude/rules/out-of-scope-tracking.md`）。
//!
//! # 呼び出し元・実行タイミング
//! `cargo test -p fandhe-backend-plugin-openapi`（CI `test` ジョブ、`--all-features` 構成に含まれる）
//! から実行する。`scripts/accept/openapi-accept.sh` が本テストの実行結果を受け入れ判定の
//! 一部として参照する。
//!
//! # `OPENAPI_YAML`（#279）との関係
//! `GET /openapi.yaml` 配信で埋め込まれる [`fandhe_backend_plugin_openapi::OPENAPI_YAML`] は
//! `OPENAPI_JSON` と同一 [`ApiDoc`] を単一のスキーマ源として `gen-openapi` CLI が生成する
//! ため、本ファイルが担う「path/method/パラメータ構造のピン留め」は JSON 側で検証すれば
//! YAML 側にも及ぶ。YAML 固有の検証（埋め込み定数がスキーマ源から乖離していないこと）は
//! `crates/plugin-openapi/src/embed.rs` の `embedded_yaml_matches_current_api_doc` が担う。

use fandhe_backend_plugin_openapi::{ApiDoc, OPENAPI_JSON};
use serde_json::Value;
use utoipa::OpenApi;

/// `OPENAPI_JSON` を JSON としてパースする。パース不能はこのファイルの全アサートの前提が
/// 崩れるためテストとして即座に失敗させる（フェイルクローズ）。
fn parsed() -> Value {
    serde_json::from_str(OPENAPI_JSON).expect("OPENAPI_JSON の JSON パースに失敗した")
}

/// 対象 path + method のオペレーションオブジェクトを取得するヘルパ。
fn operation<'a>(doc: &'a Value, path: &str, method: &str) -> &'a Value {
    doc["paths"][path][method]
        .as_object()
        .map(|_| &doc["paths"][path][method])
        .unwrap_or_else(|| panic!("{method} {path} が openapi.json に存在しない"))
}

/// `parameters` 配列から `name` 一致の 1 件を取得するヘルパ。
fn find_param<'a>(op: &'a Value, name: &str) -> &'a Value {
    op["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("parameters 配列が存在しない: {op}"))
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("パラメータ {name} が見つからない"))
}

/// OpenAPI バージョンが 3.x であることを確認する（受け入れ基準 1 の前提。
/// 構文妥当性そのもの（スキーマ準拠）は `openapi-spec-validator` に委ねる、
/// `scripts/accept/openapi-accept.sh` 参照）。
#[test]
fn openapi_version_is_3x() {
    let doc = parsed();
    let version = doc["openapi"].as_str().expect("openapi バージョン欠落");
    assert!(
        version.starts_with("3."),
        "openapi バージョンが 3.x でない: {version}"
    );
}

/// パラメータなし形態（`GET /health`）: パラメータを持たず、200 応答のみを宣言する。
#[test]
fn health_has_no_parameters() {
    let doc = parsed();
    let op = operation(&doc, "/health", "get");
    assert!(
        op.get("parameters").is_none() || op["parameters"].as_array().unwrap().is_empty(),
        "GET /health はパラメータなしの契約のはずだが宣言されている"
    );
    assert!(op["responses"]["200"].is_object());
}

/// パスパラメータ 1 件形態（`GET /hello/{name}`）: `name` が `path` 必須の string 型であることを確認する。
#[test]
fn hello_path_param_matches_declared_contract() {
    let doc = parsed();
    let op = operation(&doc, "/hello/{name}", "get");
    let name_param = find_param(op, "name");
    assert_eq!(name_param["in"], "path");
    assert_eq!(name_param["required"], true);
    assert_eq!(name_param["schema"]["type"], "string");
}

/// パスパラメータ + 400 応答形態（`GET /users/{id}`）: `id` が非負整数（`u64` 相当）であることを確認する。
#[test]
fn users_path_param_matches_declared_contract() {
    let doc = parsed();
    let op = operation(&doc, "/users/{id}", "get");
    let id_param = find_param(op, "id");
    assert_eq!(id_param["in"], "path");
    assert_eq!(id_param["required"], true);
    assert_eq!(id_param["schema"]["type"], "integer");
    // `u64`（docs.rs の宣言）は utoipa 既定で format: int64・minimum: 0 を生成する。
    assert_eq!(id_param["schema"]["format"], "int64");
    assert_eq!(id_param["schema"]["minimum"], 0);
    assert!(op["responses"]["400"].is_object());
}

/// クエリパラメータ 2 件形態（`GET /search`）: `q`（必須）・`limit`（任意）の型・必須有無を確認する。
#[test]
fn search_query_params_match_declared_contract() {
    let doc = parsed();
    let op = operation(&doc, "/search", "get");

    let q_param = find_param(op, "q");
    assert_eq!(q_param["in"], "query");
    assert_eq!(q_param["required"], true);
    assert_eq!(q_param["schema"]["type"], "string");

    let limit_param = find_param(op, "limit");
    assert_eq!(limit_param["in"], "query");
    assert_eq!(limit_param["required"], false);
    assert_eq!(limit_param["schema"]["type"], "integer");
    assert!(op["responses"]["400"].is_object());
}

/// リクエスト/レスポンス body 形態（`POST /echo`）: request/response とも `EchoBody` を参照する。
#[test]
fn echo_request_response_body_matches_declared_contract() {
    let doc = parsed();
    let op = operation(&doc, "/echo", "post");
    assert_eq!(
        op["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/EchoBody"
    );
    assert_eq!(
        op["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/EchoBody"
    );
    assert!(op["responses"]["400"].is_object());
}

/// スキーマ側（`schemas.rs`）のフィールド構成が openapi.json の `components.schemas` に
/// 正確に反映されていることを確認する（型不一致検出）。
#[test]
fn component_schema_field_types_match_rust_struct_fields() {
    let doc = parsed();
    let schemas = &doc["components"]["schemas"];

    let user = &schemas["UserResponse"]["properties"];
    assert_eq!(user["id"]["type"], "integer");
    assert_eq!(user["id"]["format"], "int64");
    assert_eq!(user["name"]["type"], "string");

    let search = &schemas["SearchResponse"]["properties"];
    assert_eq!(search["query"]["type"], "string");
    assert_eq!(search["limit"]["type"], "integer");
    assert_eq!(search["results"]["type"], "array");
    assert_eq!(search["results"]["items"]["type"], "string");
}

/// `ApiDoc::openapi()`（宣言側）を都度シリアライズし直しても `OPENAPI_JSON`（埋め込み実体）と
/// 一致すること。`embed.rs` の鮮度保証テストと同一目的だが、本ファイルは齟齬照合スイート
/// 単体で完結させるために独立して持つ（`cargo test -p fandhe-backend-plugin-openapi
/// openapi_consistency` のようなテスト名指定実行でも鮮度崩れを検知できるようにする）。
#[test]
fn embedded_json_still_matches_api_doc_declaration() {
    let mut expected = ApiDoc::openapi()
        .to_pretty_json()
        .expect("ApiDoc の JSON シリアライズに失敗した");
    if !expected.ends_with('\n') {
        expected.push('\n');
    }
    assert_eq!(OPENAPI_JSON, expected);
}
