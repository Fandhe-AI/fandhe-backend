//! `GET /openapi.json` 実サービング（TASK-2.1、#18）向けの静的埋め込み実体（TASK-3.2、#31）。
//!
//! # 役割・責務境界
//! [`OPENAPI_JSON`] は `crates/plugin-openapi/openapi.json`（`gen-openapi` CLI が
//! [`crate::ApiDoc`] から生成し、リポジトリにコミットする成果物）をコンパイル時に
//! `include_str!` で文字列定数へ埋め込んだものである。サーバーのリクエスト処理経路では
//! 実行時に `ApiDoc::openapi()` を呼ばずこの定数を返すだけで済み、PoC-4 成功基準 3
//! （実行時コストゼロ）を満たす。
//!
//! # 接続契約（TASK-2.1 との関係）
//! `GET /openapi.json` を実サービングする HTTP ハンドラの配線は TASK-2.1（#256、
//! サーバ側 feature `openapi = ["dep:fandhe-backend-plugin-openapi"]`）で完了した。
//! 本クレートはハンドラを持たない。`crates/core/src/plugin.rs` の
//! `try_intercept` が `Server::openapi()` の明示登録後に限り、本定数を
//! `Content-Type: application/json` で返すだけの薄いハンドラとして配線している。
//! 実装との齟齬照合（宣言した path と `crates/routes` の実ルーティングの一致確認）は
//! TASK-3.3（#32）のスコープ。
//!
//! # 鮮度保証（fail-closed）
//! `openapi.json` は生成物のコミットであるため [`ApiDoc`] の変更後に再生成し忘れると
//! 陳腐化しうる（OWASP A08、`.claude/rules/security.md`）。次の 2 段のゲートで検知する。
//! 1. 本モジュールの `embedded_json_matches_current_api_doc` テスト（`cargo test` 常設）
//! 2. CI の 2 段階ビルド（`scripts/openapi-two-stage.sh` stage 1、`gen-openapi --check`）
//!
//! [`ApiDoc`]: crate::ApiDoc

/// コンパイル時に埋め込まれた OpenAPI 定義（JSON、pretty 形式・末尾改行付き）。
///
/// `crates/plugin-openapi/openapi.json` の内容そのもの。`gen-openapi` CLI
/// （`gen-cli` feature、TASK-3.2、#31）が生成した成果物を `include_str!` するのみで、
/// 本クレートの通常ビルド（`gen-cli` 無効）には CLI・`serde_json` 依存を一切含まない
/// （pay-for-what-you-use、`.claude/rules/pay-for-what-you-use.md`）。
///
/// # Examples
/// ```
/// use fandhe_backend_plugin_openapi::OPENAPI_JSON;
///
/// assert!(OPENAPI_JSON.starts_with('{'));
/// assert!(OPENAPI_JSON.contains("\"/health\""));
/// ```
pub const OPENAPI_JSON: &str = include_str!("../openapi.json");

#[cfg(test)]
mod tests {
    use super::OPENAPI_JSON;
    use crate::ApiDoc;
    use utoipa::OpenApi;

    /// 鮮度保証の一次ゲート: 埋め込み済み `OPENAPI_JSON` が `ApiDoc::openapi()` の
    /// 現在の定義から生成した内容と一致することを検証する。`ApiDoc`（docs.rs）を
    /// 変更したのに `gen-openapi` の再生成を忘れた場合、このテストが失敗して検知する
    /// （CI の `test` ジョブで常設実行、fail-closed）。
    #[test]
    fn embedded_json_matches_current_api_doc() {
        let mut expected = ApiDoc::openapi()
            .to_pretty_json()
            .expect("ApiDoc の JSON シリアライズに失敗した");
        if !expected.ends_with('\n') {
            expected.push('\n');
        }
        assert_eq!(
            OPENAPI_JSON, expected,
            "openapi.json が ApiDoc の最新定義と乖離している。`cargo run -p fandhe-backend-plugin-openapi \
             --features gen-cli --bin gen-openapi -- --update` で再生成すること"
        );
    }

    /// 埋め込み内容が JSON として再パース可能であることを確認する（構文検証の最小限。
    /// OpenAPI 3.x スキーマ準拠の検証は TASK-3.3、#32 のスコープ）。
    #[test]
    fn embedded_json_is_reparsable() {
        let parsed: serde_json::Value =
            serde_json::from_str(OPENAPI_JSON).expect("OPENAPI_JSON の再パースに失敗した");
        assert_eq!(parsed["paths"].as_object().unwrap().len(), 5);
    }
}
