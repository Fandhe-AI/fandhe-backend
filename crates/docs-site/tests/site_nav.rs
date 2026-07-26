//! `site/nav.toml`（実マニフェスト）と既存 docs 資産のドリフト検知テスト。
//!
//! Fandhe-AI/fandhe-frontend の `crates/docs-site/tests/site_nav.rs` からの
//! 移植。検証内容（パース成功・ページ登録の網羅・path/source の一意性・
//! source 実在・ブロックレベルのレンダリング健全性）は同じで、期待値を
//! fandhe-backend の `site/nav.toml`（トップ + Guides セクション索引 +
//! `docs/guide/` 7 本 + API Reference セクション索引 + `docs/api/` 5 本 +
//! `site/examples/` 5 本 = 全 20 ページ）へ合わせている。
//! `docs/guide/`・`docs/api/`・`site/examples/` の編集・改名でナビ登録と
//! 実ファイルが乖離した場合に `cargo test` が fail-closed で検知する。
//!
//! セクション順は `docs/design/docs-site-redesign.md` 6 節が定める
//! Getting Started / Guides / Examples / API Reference の 4 部構成に従う
//! （`docs/design/docs-site-redesign.md` 7 節の公開範囲規約に基づき、
//! nav 登録ソースからは issue 番号・内部タスク表記を分離済み。将来の
//! 再混入は本ファイル末尾の `site_nav_sources_contain_no_internal_records`
//! が fail-closed で検知する）。Guides / API Reference の要約付きセクション
//! 索引ページ（`site/guides.md` / `site/api.md`）も同規約に従い登録する。

use std::path::{Path, PathBuf};

use fandhe_backend_docs_site::markdown::render_markdown;
use fandhe_backend_docs_site::nav::{parse_nav, sidebar, validate_sources, Nav};
use fandhe_frontend_core::{render, Node};

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
fn site_nav_registers_four_sections_with_expected_titles() {
    let nav = load_nav();
    let titles: Vec<&str> = nav.sections.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(
        titles,
        vec!["Getting Started", "Guides", "Examples", "API Reference"]
    );
}

/// 既存の利用者向けドキュメント（トップ + Guides セクション索引 +
/// `docs/guide/` の 7 本 + API Reference セクション索引 + `docs/api/` の
/// 5 本 + `site/examples/` の 5 本 = 全 20 ページ）を
/// モジュールスコープの定数として保持する。`site/nav.toml` にページを
/// 追加・削除した場合はこのリストを実測値へ追随させる必要があり、
/// 更新を怠ると `site_nav_registers_all_pages_with_expected_paths` が
/// fail-closed で検知する。[`EXPECTED_PAGE_COUNT`] は本リストの長さを
/// 導出するのみで、別途値を保持しない（レビュー指摘: リストと定数の
/// 二重管理は「一方だけ更新して他方を放置する」という定数自身が作り出す
/// 失敗モードを生むため、導出値にして構造的に排除する）。
const EXPECTED_PAGES: &[(&str, &str)] = &[
    ("site/index.md", "/"),
    ("docs/guide/getting-started.md", "/getting-started/"),
    ("site/guides.md", "/guides/"),
    ("docs/guide/README.md", "/guides/reading/"),
    ("docs/guide/feature-samples.md", "/guides/feature-samples/"),
    ("docs/guide/tutorial.md", "/guides/tutorial/"),
    (
        "docs/guide/extension-points.md",
        "/guides/extension-points/",
    ),
    ("docs/guide/streaming.md", "/guides/streaming/"),
    (
        "docs/guide/graceful-shutdown.md",
        "/guides/graceful-shutdown/",
    ),
    ("site/api.md", "/api/"),
    ("docs/api/server-api.md", "/api/server-api/"),
    ("docs/api/extension-api.md", "/api/extension-api/"),
    ("docs/api/http-api.md", "/api/http-api/"),
    ("docs/api/router-api.md", "/api/router-api/"),
    ("docs/api/plugin-config-api.md", "/api/plugin-config-api/"),
    ("site/examples.md", "/examples/"),
    ("site/examples/with-cors.md", "/examples/with-cors/"),
    ("site/examples/with-graphql.md", "/examples/with-graphql/"),
    (
        "site/examples/with-websocket.md",
        "/examples/with-websocket/",
    ),
    ("site/examples/templates-app.md", "/examples/templates-app/"),
];

/// nav 登録ページ総数の契約値（イシュー #397）。[`EXPECTED_PAGES`] の長さを
/// そのまま導出するだけで独立した値を持たないため、[`EXPECTED_PAGES`] の
/// 更新漏れが本定数との不一致として現れることは構造上あり得ない
/// （「期待値で固定する」という受け入れ条件は
/// `site_nav_registers_expected_page_count` が引き続き担保する）。
const EXPECTED_PAGE_COUNT: usize = EXPECTED_PAGES.len();

/// 既存の利用者向けドキュメント（[`EXPECTED_PAGES`]、トップ + Guides
/// セクション索引 + `docs/guide/` の 7 本 + API Reference セクション索引 +
/// `docs/api/` の 5 本 + `site/examples/` の 5 本 = 全 20 ページ）が
/// サイト生成対象として漏れなく登録されている。
#[test]
fn site_nav_registers_all_pages_with_expected_paths() {
    let nav = load_nav();
    let pages: Vec<(&str, &str)> = nav
        .sections
        .iter()
        .flat_map(|s| s.pages.iter())
        .map(|p| (p.source.as_str(), p.path.as_str()))
        .collect();

    assert_eq!(
        pages.len(),
        EXPECTED_PAGES.len(),
        "unexpected pages: {pages:?}"
    );
    for expected_pair in EXPECTED_PAGES {
        assert!(
            pages.contains(expected_pair),
            "nav.toml is missing expected page {expected_pair:?}"
        );
    }
}

/// nav 登録ページ総数を独立の契約定数（[`EXPECTED_PAGE_COUNT`]）で明示固定する
/// （イシュー #397）。[`EXPECTED_PAGE_COUNT`] は [`EXPECTED_PAGES`] からの
/// 導出値であり二重管理ではないが、本テストは「ページを 1 本増減しただけで
/// 意図せず通過してしまわないか」を定数比較の形で読み手に明示するのが目的。
#[test]
fn site_nav_registers_expected_page_count() {
    let nav = load_nav();
    let page_count = nav.sections.iter().map(|s| s.pages.len()).sum::<usize>();
    assert_eq!(
        page_count, EXPECTED_PAGE_COUNT,
        "site/nav.toml のページ総数が契約値 EXPECTED_PAGE_COUNT と乖離しています。\
         ページを追加・削除した場合は EXPECTED_PAGES を実測値へ追随させてください \
         （EXPECTED_PAGE_COUNT は EXPECTED_PAGES から自動導出されるため個別更新は不要）。"
    );
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

/// イシュー #391: 実マニフェスト（`site/nav.toml`）から生成したサイドバーが
/// 現在ページ表現・意味論の両方でアクセシビリティ契約を満たすことを固定する。
#[test]
fn site_nav_sidebar_uses_aria_current_only_and_no_role_attribute() {
    let nav = load_nav();
    // 実在ページの 1 つを現在ページとして選び、aria-current 分岐を発生させる。
    let current_path = "/getting-started/";
    let html = render(&sidebar(&nav, current_path));

    // 現在ページは aria-current="page" のみで表現され、`class="current"`
    // を含まない（移植元イシュー #756 と同じく class 併用は廃止済み）。
    assert!(html.contains(r#"aria-current="page""#));
    assert!(!html.contains(r#"class="current""#));

    // サイドバー nav は role なしの headless 構造（ネイティブ要素の暗黙 role
    // をそのまま使う）。
    assert!(!html.contains("role="));
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

/// `docs/design/docs-site-redesign.md` 7 節「公開範囲規約」が定める内部進行情報
/// （issue/PR 番号・`TASK-N.N` 表記・`PoC-N` 表記）を nav 登録ページから機械的に
/// 検出する。公開サイトへの内部記録の再混入を CI（`cargo test`）レベルで
/// fail-closed に阻止するのが目的（イシュー #395）。
///
/// `regex` クレート等の外部依存は `pay-for-what-you-use` 方針
/// （`crates/docs-site` の依存は fandhe-frontend-* に限定）に反するため、
/// 検出は手書きの文字走査で行う。
///
/// 検出パターン:
/// - (a) `TASK-` の直後に ASCII 数字が続く（`TASK-11.3` 等）
/// - (b) `PoC-` の直後に ASCII 数字が続く（`PoC-3` 等）
/// - (c) `#` の直後に ASCII 数字が 2〜4 桁連続し、その直後の文字が英数字で
///   ない（`#279、` `（#313）` 等の issue 参照を捕捉しつつ、`#0075ca` のような
///   6 桁 hex カラーコード（数字4桁の直後が英字）や `#1a2b3c`
///   （数字が1桁で連続しない）は誤検知しない）
fn find_internal_record_marker(text: &str) -> Option<&'static str> {
    if contains_prefixed_digit(text, "TASK-") {
        return Some("TASK-N 表記");
    }
    if contains_prefixed_digit(text, "PoC-") {
        return Some("PoC-N 表記");
    }
    if contains_issue_hash_reference(text) {
        return Some("# 直後の issue 番号表記");
    }
    None
}

/// `prefix` の直後に ASCII 数字が来る箇所が存在するかを走査する
/// （`TASK-11.3`・`PoC-3` のような内部タスク表記の検出に使う）。
fn contains_prefixed_digit(text: &str, prefix: &str) -> bool {
    let bytes = text.as_bytes();
    let plen = prefix.len();
    let mut start = 0;
    while let Some(rel) = text[start..].find(prefix) {
        let idx = start + rel + plen;
        if bytes.get(idx).is_some_and(u8::is_ascii_digit) {
            return true;
        }
        start += rel + plen;
    }
    false
}

/// `#` の直後に ASCII 数字が 2〜4 桁連続し、その次の文字が英数字でない
/// 箇所を検出する（hex カラーコードとの誤検知を避けるための桁数・後続文字条件）。
fn contains_issue_hash_reference(text: &str) -> bool {
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'#' {
            continue;
        }
        let mut digits = 0usize;
        while bytes.get(i + 1 + digits).is_some_and(u8::is_ascii_digit) {
            digits += 1;
        }
        if (2..=4).contains(&digits) {
            let next = bytes.get(i + 1 + digits);
            let next_is_alnum = next.is_some_and(u8::is_ascii_alphanumeric);
            if !next_is_alnum {
                return true;
            }
        }
    }
    false
}

/// 内部記録の再混入を検知する fail-closed ガードテスト（イシュー #395）。
///
/// nav 登録済みの全 source（`site/`・`docs/guide/`・`docs/api/`）を読み、
/// issue 番号・`TASK-N`・`PoC-N` 表記が残っていれば検出ファイル名・行番号と
/// 分離規約の参照先を添えて失敗する。
#[test]
fn site_nav_sources_contain_no_internal_records() {
    let root = repo_root();
    let nav = load_nav();
    let mut violations = Vec::new();
    for section in &nav.sections {
        for page in &section.pages {
            let full_path = root.join(&page.source);
            let input = std::fs::read_to_string(&full_path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", page.source));
            for (line_no, line) in input.lines().enumerate() {
                if let Some(kind) = find_internal_record_marker(line) {
                    violations.push(format!(
                        "{}:{}: {kind} が検出されました（分離規約は \
                         `docs/design/docs-site-redesign.md` 7 節参照）: {line}",
                        page.source,
                        line_no + 1,
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "nav 登録ページに内部記録の残存が検出されました:\n{}",
        violations.join("\n")
    );
}
