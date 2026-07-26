//! `fandhe-backend-docs-site` のライブラリ入口。
//!
//! fandhe-backend の公式 docs サイト（GitHub Pages）を生成する SSG ツール。
//! Fandhe-AI/fandhe-frontend の `crates/docs-site`（MIT OR Apache-2.0）からの
//! 移植であり、レンダラとして fandhe-frontend の公開クレート
//! （`fandhe-frontend-core` / `fandhe-frontend-app` / `fandhe-frontend-server`、
//! crates.io）を使う。`site/nav.toml` に登録された `docs/guide/` 配下の
//! Markdown をリンク検証（fail-closed）付きで静的 HTML へ変換する。
//! バイナリ本体（`src/main.rs`）に加えて、統合テスト（`tests/`）から
//! 各モジュールを直接検証できるように `[lib]` ターゲットを併設する。
//! crate 外部への公開・配布は行わない（`Cargo.toml` の `publish = false`）。
//!
//! 各モジュール内の doc comment に現れるイシュー番号（#465〜#488 等）・
//! PR 番号・`REQ-3`・`crates/cli` 等への言及は移植元リポジトリ
//! （fandhe-frontend）の文脈であり、本リポジトリのイシュー・要件を指す
//! ものではない。
//!
//! - [`layout`]: docs レイアウトコンポーネント
//! - [`markdown`]: Markdown ブロック構文 → Node 木レンダラ
//! - [`nav`]: `site/nav.toml` のパース・サイドバー / 前後ナビ生成
//! - [`linkcheck`]: `.md` リンクのサイト内パスへの書き換え・内部リンク突合検証
//! - [`script`]: ダークモードトグル・全文検索用の唯一の JS（イシュー #390・
//!   #396）。`layout` の `<head>`・ヘッダーアクション領域から参照される
//! - [`search`]: 依存ゼロ全文検索インデックスの生成（イシュー #396）。
//!   ページ本文からのプレーンテキスト抽出・決定的 JSON 直列化・サイズ上限
//!   検証を sans-I/O な純関数として提供する
//! - [`build`]: `nav.toml` 読込 → ページ組み立て → linkcheck →
//!   `generate_pages()` 書き出し → アセットコピー → [`script::SITE_JS`] /
//!   検索インデックス（[`search`]）書き出しの一連のビルドパイプライン本体。
//!   `main.rs`（バイナリ本体）は本モジュールの [`build::build_site`] を
//!   呼ぶ薄いラッパー。
//!
//! `fandhe-frontend-core` / `fandhe-frontend-app` / `fandhe-frontend-server`
//! のみに依存し、外部クレートは追加しない（`Cargo.toml` の依存方針コメント
//! 参照）。
//!
//! workspace 全体の依存方向規約（依存方向: server → routes → http::*、
//! `scripts/dep-direction-check.sh` が機械検証）との関係では、本クレートは
//! ドキュメント生成専用の開発ツールとしてフレームワーク本体のどのクレートにも
//! 依存せず・依存されず、上記の一方向グラフの外に独立して位置する。
//!
//! `#![forbid(unsafe_code)]` は workspace lint（`unsafe_code = "warn"` +
//! CI の `-D warnings`）より強い保証として本クレートでも維持する
//! （`.claude/rules/coding-rust.md` の一般規約）。

#![forbid(unsafe_code)]

pub mod build;
pub mod layout;
pub mod linkcheck;
pub mod markdown;
pub mod nav;
pub mod script;
pub mod search;
