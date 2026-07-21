//! `site/nav.toml`（実マニフェスト）と既存 docs 資産のドリフト検知テスト。
//!
//! Fandhe-AI/fandhe-frontend の `crates/docs-site/tests/site_nav.rs` からの
//! 移植。検証内容（パース成功・ページ登録の網羅・path/source の一意性・
//! source 実在・ブロックレベルのレンダリング健全性）は同じで、期待値を
//! fandhe-backend の `site/nav.toml`（トップ + `docs/guide/` 4 本 =
//! 全 5 ページ）へ合わせている。`docs/guide/` の編集・改名でナビ登録と
//! 実ファイルが乖離した場合に `cargo test` が fail-closed で検知する。

use std::path::{Path, PathBuf};

use fandhe_backend_docs_site::markdown::render_markdown;
use fandhe_backend_docs_site::nav::{Nav, parse_nav, validate_sources};
use fandhe_frontend_core::Node;

/// `CARGO_MANIFEST_DIR`（`crates/docs-site`）から repo_root を解決する。
/// テストフィクスチャがクレート内に閉じず repo_root 配下の実ファイルを
/// 参照するため、`crates/docs-site` の 2 階層上を repo_root とみなす
/// （`Cargo.toml` の `members = ["crates/*"]` 構成に対応）。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo_root should resolve from CARGO_MANIFEST_DIR")
}

fn load_nav() -> Nav {
    let path = repo_root().join("site/nav.toml");
    let input = std::fs::read_to_string(&path).expect("site/nav.toml should be readable");
    parse_nav(&input).expect("site/nav.toml should conform to the fail-closed TOML subset")
}

#[test]
fn site_nav_parses_successfully() {
    let nav = load_nav();
    assert_eq!(nav.site.title, "fandhe-backend");
    assert_eq!(nav.site.base_path, "/fandhe-backend");
}

#[test]
fn site_nav_registers_two_sections_with_expected_titles() {
    let nav = load_nav();
    let titles: Vec<&str> = nav.sections.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, vec!["Getting Started", "Guides"]);
}

/// 既存の利用者向けドキュメント（トップ + `docs/guide/` の 4 本 = 全 5 ページ）
/// がサイト生成対象として漏れなく登録されている。
#[test]
fn site_nav_registers_all_five_pages_with_expected_paths() {
    let nav = load_nav();
    let pages: Vec<(&str, &str)> = nav
        .sections
        .iter()
        .flat_map(|s| s.pages.iter())
        .map(|p| (p.source.as_str(), p.path.as_str()))
        .collect();

    let expected = vec![
        ("site/index.md", "/"),
        ("docs/guide/getting-started.md", "/getting-started/"),
        ("docs/guide/README.md", "/guides/"),
        ("docs/guide/feature-samples.md", "/guides/feature-samples/"),
        ("docs/guide/tutorial.md", "/guides/tutorial/"),
    ];
    assert_eq!(pages.len(), expected.len(), "unexpected pages: {pages:?}");
    for expected_pair in &expected {
        assert!(
            pages.contains(expected_pair),
            "nav.toml is missing expected page {expected_pair:?}"
        );
    }
}

#[test]
fn site_nav_has_no_duplicate_paths_or_sources() {
    let nav = load_nav();
    let mut seen_paths = std::collections::BTreeSet::new();
    let mut seen_sources = std::collections::BTreeSet::new();
    for section in &nav.sections {
        for page in &section.pages {
            assert!(
                seen_paths.insert(page.path.clone()),
                "duplicate page.path: {}",
                page.path
            );
            assert!(
                seen_sources.insert(page.source.clone()),
                "duplicate page.source: {}",
                page.source
            );
        }
    }
}

/// 登録された全 source が repo_root 配下に実在する（`validate_sources` の
/// パストラバーサル検証・存在検証を実マニフェストに対して通す）。
#[test]
fn site_nav_validate_sources_covers_all_pages() {
    let root = repo_root();
    let nav = load_nav();
    validate_sources(&nav, &root).expect("all page.source entries should exist under repo_root");
}

/// サブセット外構文の残存によるレンダリング崩れがない。
///
/// `render_markdown` はブロックレベル解釈を担うため、本テストはブロック構造の
/// 健全性（先頭が見出しであること・フェンスコードの閉じ忘れがテキストノードに
/// 漏れ出していないこと）のみを確認する。
#[test]
fn every_source_renders_without_fence_leakage_and_starts_with_a_heading() {
    let root = repo_root();
    let nav = load_nav();
    for section in &nav.sections {
        for page in &section.pages {
            let full_path = root.join(&page.source);
            let input = std::fs::read_to_string(&full_path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", page.source));
            let blocks = render_markdown(&input);
            assert!(
                !blocks.is_empty(),
                "{} rendered to an empty block list",
                page.source
            );
            assert!(
                is_heading(&blocks[0]),
                "{} does not start with a heading (H1 expected as the page title)",
                page.source
            );
            for block in &blocks {
                assert!(
                    !contains_unclosed_fence_marker(block),
                    "{} contains a stray ``` marker in rendered text, likely an unclosed fence",
                    page.source
                );
            }
        }
    }
}

fn is_heading(node: &Node) -> bool {
    matches!(
        node,
        Node::Element { tag, .. } if matches!(*tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
    )
}

/// テキストノードに ``` （フェンス開始/終了マーカー）がそのまま残っていないか
/// 再帰的に確認する。閉じ忘れたフェンスはブロックパーサが段落テキストとして
/// フォールバックするため、マーカー文字列がテキストノードに漏れ出る形で検知できる。
fn contains_unclosed_fence_marker(node: &Node) -> bool {
    match node {
        Node::Text(s) => s.contains("```"),
        Node::RawHtml(_) => false,
        Node::Element { children, .. } => children.iter().any(contains_unclosed_fence_marker),
    }
}
