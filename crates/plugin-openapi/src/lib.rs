//! backend-framework の OpenAPI ドキュメント生成プラグイン（TASK-3.1、REQ-3【Must】）。
//!
//! # 役割・責務境界
//! 本クレートは実装本体（`crates/routes` 等）から独立した「OpenAPI ドキュメント
//! 定義」のみを持つ。`bf-routes::Router` は method + target 完全一致の関数ベース
//! ルーティングであり、axum のような属性マクロで飾れるハンドラ単位を持たない。
//! そこで実装本体とは疎結合な「ドキュメント専用の薄い関数」に
//! `#[utoipa::path(...)]` を付与し、[`ApiDoc`] に集約する（PoC-4 で検証・
//! 採用した統合方式、`docs/spec/03-poc/openapi-generation/README.md`）。
//!
//! # 接続契約（TASK-2.1 / TASK-3.2 との関係）
//! - 本クレートは独立クレート = プラグイン境界として切り出す
//!   （`crates/plugin-webrtc-proxy` と同一パターン）。core / http / routes /
//!   他プラグインのどこからも参照しない限り `utoipa` 系依存は本クレートの外に
//!   一切現れない（pay-for-what-you-use、`.claude/rules/pay-for-what-you-use.md`）。
//! - サーバ側 feature（`openapi = ["dep:bf-plugin-openapi"]` 相当）による配線は
//!   TASK-2.1（#18、並列進行中）に接続点を委ねる。本クレート単体では未接続。
//! - `GET /openapi.json` の静的埋め込み・生成 CLI（`gen-openapi`）は
//!   TASK-3.2（#31）のスコープ。本クレートはドキュメント定義（[`ApiDoc`]・
//!   スキーマ型）の提供のみを担う。
//!
//! # 実行時コスト
//! [`ApiDoc::openapi()`] はコンパイル時に構築されたメタデータから実行時に
//! ドキュメント構造体を組み立てるのみで、サーバーのリクエスト処理経路からは
//! 呼び出さない（PoC-4 成功基準 3: 実行時コストゼロ）。

mod docs;
mod schemas;

pub use docs::ApiDoc;
pub use schemas::{EchoBody, ErrorBody, SearchResponse, UserResponse};

// doc test 内で `utoipa::OpenApi` トレイトの `openapi()` を呼べるようにするため、
// 公開 API 利用側の便宜として re-export しておく（利用側で `utoipa` を直接
// 依存に加えなくても `ApiDoc::openapi()` を呼べるようにする意図）。
pub use utoipa::OpenApi;
