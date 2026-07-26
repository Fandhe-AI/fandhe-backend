//! `crates/docs-site/src/layout.rs` / `nav.rs` が生成する HTML の `class`
//! 属性値と `site/assets/site.css` のクラス名契約が乖離していないことを
//! 検証する回帰テスト（イシュー #488）。
//!
//! # 背景
//!
//! `layout.rs` は独自に `docs-*` プレフィックス（`skip-nav` 含む）の class を
//! 組み立て、`nav.rs` は `sidebar` / `prev-next` / `prev` / `next` を
//! 独自に組み立てる並列実装であり、両者が実際に一致しているかは
//! コンパイラでは検証されない（CSS 文字列は Rust の型システム外）。過去に
//! `site.css` 側だけが `site-*` プレフィックスの想定クラス名で書かれ、
//! `layout.rs` の実出力（`docs-*` プレフィックス）と食い違ったまま放置され、
//! 本番 docs サイトで CSS がほぼ効かない不具合が発生した。本テストは
//! `layout.rs` / `nav.rs` の実出力に含まれる全 `class` 属性値（空白区切りの
//! 各トークン）が `site/assets/site.css` 内にセレクタとして出現することを
//! 機械的に検証し、再発を fail-closed で検知する。
//!
//! Markdown レンダラ（`markdown.rs`）が動的に生成する `language-<lang>`
//! クラス（コードブロックの言語トークン依存で無数の値を取りうる）は本テスト
//! のスコープ外とする（`.docs-content pre code` の要素セレクタでスタイルが
//! 適用されるため、契約ドリフトの対象にならない）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use fandhe_backend_docs_site::layout::docs_page;
use fandhe_backend_docs_site::nav::{Nav, parse_nav, prev_next_nav, sidebar};
use fandhe_frontend_core::{Node, li, p, render, text, ul};

/// `CARGO_MANIFEST_DIR`（`crates/docs-site`）から repo_root を解決する
/// （`site_nav.rs` と同じ規約。`members = ["crates/*"]` 構成に対応）。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo_root should resolve from CARGO_MANIFEST_DIR")
}

fn site_css() -> String {
    let path = repo_root().join("site/assets/site.css");
    std::fs::read_to_string(&path).expect("site/assets/site.css should be readable")
}

/// TOC（`h2`/`h3` 見出し）・サイドバー 2 セクション・前後ページ双方が揃う
/// ように仕立てたフィクスチャ `Nav`。`docs-toc-level-2` /
/// `docs-toc-level-3` と `prev-next` の `prev`/`next` 両方を同時に
/// 発生させるための最小構成。
fn fixture_nav() -> Nav {
    let toml = r#"
[site]
title = "Fixture"
base_path = ""

[[section]]
title = "Getting Started"

[[section.page]]
title = "Intro"
source = "site/index.md"
path = "/"

[[section.page]]
title = "Quickstart"
source = "site/index.md"
path = "/quickstart/"

[[section]]
title = "Guides"

[[section.page]]
title = "Advanced"
source = "site/index.md"
path = "/advanced/"
"#;
    parse_nav(toml).expect("fixture nav.toml should parse")
}

fn fixture_body() -> Node {
    fandhe_frontend_core::div(
        vec![],
        vec![
            fandhe_frontend_core::h2(vec![], vec![text("導入")]),
            p(vec![], vec![text("本文です。")]),
            fandhe_frontend_core::h3(vec![], vec![text("詳細")]),
            p(vec![], vec![text("詳細本文です。")]),
        ],
    )
}

/// 見出し（`h2`/`h3`）を含まない本文フィクスチャ。`docs_page` の TOC 省略
/// 分岐（`.docs-container.docs-no-toc`、イシュー #389 Bugbot 指摘）を
/// 発生させるために [`fixture_body`] と分けて用意する。
fn fixture_body_without_headings() -> Node {
    fandhe_frontend_core::div(
        vec![],
        vec![p(vec![], vec![text("見出しのない本文です。")])],
    )
}

fn fixture_sidebar() -> Node {
    // `docs_page` 単独呼び出しテストでは `nav::sidebar()` の実出力を使わず
    // 最小の `ul`/`li` を渡す既存 `layout_render.rs` の流儀に合わせつつ、
    // 本テストでは `nav::sidebar()` 自体の class も別途検証する
    // （`sidebar_html_class_tokens_are_covered_by_site_css` 参照）。
    ul(vec![], vec![li(vec![], vec![text("はじめに")])])
}

/// html 文字列中の全 `class="..."` 属性値を、空白区切りトークンへ展開して
/// 収集する。
fn extract_class_tokens(html: &str) -> HashSet<String> {
    let mut tokens = HashSet::new();
    let mut rest = html;
    while let Some(start) = rest.find(r#"class=""#) {
        let after = &rest[start + r#"class=""#.len()..];
        let Some(end) = after.find('"') else { break };
        let value = &after[..end];
        for token in value.split_whitespace() {
            tokens.insert(token.to_string());
        }
        rest = &after[end + 1..];
    }
    tokens
}

/// `/* ... */` コメントを取り除く。`site.css` 冒頭のクラス名契約コメントは
/// 実セレクタと同じ `.docs-header` のような記法で説明文を書いているため、
/// コメントを除去せずに [`extract_css_class_selectors`] を適用すると
/// 「コメントで名前に言及されているだけ」で実セレクタが存在するかのように
/// 誤判定してしまう（本テストが検知すべき乖離をすり抜けてしまう）。
/// ネストしないブロックコメントのみを前提とする単純な走査で十分
/// （`site.css` は CSS の仕様どおりネストしないブロックコメントしか使わない）。
fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("*/") {
            Some(end) => rest = &after[end + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// CSS テキストから `.identifier` 形式のクラスセレクタトークンをすべて
/// 収集する。コメントは事前に [`strip_css_comments`] で除去してから走査する
/// （契約コメント中の記法をセレクタと誤認しないため）。数値（`0.5rem` 等）の
/// 小数点は次の文字が識別子開始文字（英字 / `_`）でないため誤検出しない。
fn extract_css_class_selectors(css: &str) -> HashSet<String> {
    let css = strip_css_comments(css);
    let mut tokens = HashSet::new();
    let chars: Vec<char> = css.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '.' {
            let next = chars.get(i + 1).copied();
            if matches!(next, Some(c) if c.is_ascii_alphabetic() || c == '_') {
                let mut j = i + 1;
                let mut token = String::new();
                while j < chars.len()
                    && (chars[j].is_ascii_alphanumeric() || chars[j] == '-' || chars[j] == '_')
                {
                    token.push(chars[j]);
                    j += 1;
                }
                tokens.insert(token);
                i = j;
                continue;
            }
        }
        i += 1;
    }
    tokens
}

/// 生成 HTML 中の全 class トークンが `site.css` 側にセレクタとして
/// 存在することを検証する（fail-closed。1 つでも欠けていれば即失敗）。
fn assert_all_classes_covered(html: &str, css_tokens: &HashSet<String>, context: &str) {
    let html_tokens = extract_class_tokens(html);
    assert!(
        !html_tokens.is_empty(),
        "{context}: フィクスチャ HTML から class トークンが 1 件も抽出できなかった（テスト自体の不備の可能性）"
    );
    let missing: Vec<&String> = html_tokens
        .iter()
        .filter(|t| !css_tokens.contains(t.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "{context}: 以下の class が site/assets/site.css にセレクタとして存在しない: {missing:?}\n\
         layout.rs / nav.rs の実出力と site.css のクラス名契約が乖離している。"
    );
}

#[test]
fn docs_page_html_class_tokens_are_covered_by_site_css() {
    let css_tokens = extract_css_class_selectors(&site_css());
    let node = docs_page(
        "タイトル",
        "fandhe-backend",
        "",
        fixture_sidebar(),
        fixture_body(),
    );
    let html = render(&node);
    assert_all_classes_covered(&html, &css_tokens, "docs_page");
}

#[test]
fn sidebar_html_class_tokens_are_covered_by_site_css() {
    let css_tokens = extract_css_class_selectors(&site_css());
    let nav = fixture_nav();
    // 現在ページを 2 番目のページに一致させ、`aria-current` 付与の分岐を
    // 実際に発生させる（イシュー #391: `class="current"` は廃止済み）。
    let node = sidebar(&nav, "/quickstart/");
    let html = render(&node);
    assert_all_classes_covered(&html, &css_tokens, "nav::sidebar");
}

#[test]
fn prev_next_nav_html_class_tokens_are_covered_by_site_css() {
    let css_tokens = extract_css_class_selectors(&site_css());
    let nav = fixture_nav();
    // 中間ページを指定し、prev/next 両方の `<a>` を同時に発生させる。
    let node = prev_next_nav(&nav, "/quickstart/");
    let html = render(&node);
    assert_all_classes_covered(&html, &css_tokens, "nav::prev_next_nav");
}

/// 生成 HTML と `site.css` の class 契約インベントリ（イシュー #397）。
///
/// 上記 3 テスト（`docs_page_html_class_tokens_are_covered_by_site_css` 等）は
/// いずれも「生成 HTML の class ⊆ site.css のセレクタ」という片方向検証に
/// 留まる。この方向だけでは、実装変更でフィクスチャが特定の class を
/// 出力しなくなった場合（分岐条件の変更・要素の削除等）に該当 assert が
/// 単に対象を失って静かに通過してしまい、「実は誰も使っていない孤立
/// class が site.css に取り残される」ドリフトを検知できない抜け穴がある。
/// 本テストは契約対象 class を [`EXPECTED_CLASSES`] として明示列挙し、
/// (a) 生成 HTML に契約外 class が混入していないか、(b) 契約 class が
/// 生成 HTML から欠落していないか（抜け穴を閉じる本体）、(c) 契約 class が
/// `site.css` にセレクタとして定義されているか、(d) `site.css` に契約リスト
/// 外の孤立 class が残っていないか、の 4 方向を fail-closed に検証する。
const EXPECTED_CLASSES: &[&str] = &[
    "docs-brand",
    "docs-container",
    "docs-content",
    "docs-github-link",
    "docs-header",
    "docs-header-actions",
    "docs-main",
    "docs-no-toc",
    "docs-search",
    "docs-search-input",
    "docs-search-label",
    "docs-search-results",
    "docs-sidebar",
    "docs-sidebar-toggle",
    "docs-sidebar-toggle-label",
    "docs-theme-toggle",
    "docs-toc",
    "docs-toc-aside",
    "docs-toc-level-2",
    "docs-toc-level-3",
    "next",
    "prev",
    "prev-next",
    "sidebar",
    "skip-nav",
];

#[test]
fn generated_html_class_inventory_matches_expected_contract_and_site_css() {
    let css_tokens = extract_css_class_selectors(&site_css());
    let expected: HashSet<String> = EXPECTED_CLASSES.iter().map(|s| s.to_string()).collect();

    // `docs_page`・`sidebar`・`prev_next_nav` の 3 フィクスチャ HTML を合算し、
    // 契約対象 class が実際に出力される全経路をまとめて走査する
    // （TOC 2 階層・prev/next 両方向・ヘッダー右側要素を同時に発生させる
    // フィクスチャ選定は上記個別テストの流儀を踏襲）。
    let nav = fixture_nav();
    let mut html = String::new();
    html.push_str(&render(&docs_page(
        "タイトル",
        "fandhe-backend",
        "",
        fixture_sidebar(),
        fixture_body(),
    )));
    html.push_str(&render(&docs_page(
        "見出しなし",
        "fandhe-backend",
        "",
        fixture_sidebar(),
        fixture_body_without_headings(),
    )));
    html.push_str(&render(&sidebar(&nav, "/quickstart/")));
    html.push_str(&render(&prev_next_nav(&nav, "/quickstart/")));
    let html_tokens = extract_class_tokens(&html);

    let unexpected_in_html: Vec<&String> = html_tokens
        .iter()
        .filter(|t| !expected.contains(t.as_str()))
        .collect();
    assert!(
        unexpected_in_html.is_empty(),
        "契約リスト EXPECTED_CLASSES に無い class が生成 HTML に出現しました: \
         {unexpected_in_html:?}\n新規 class は EXPECTED_CLASSES へ追加した上で、\
         site/assets/site.css にも同名セレクタを定義してください \
         （EXPECTED_CLASSES へ追加するだけでは missing_from_css で失敗します）。"
    );

    let missing_from_html: Vec<&&str> = EXPECTED_CLASSES
        .iter()
        .filter(|c| !html_tokens.contains(**c))
        .collect();
    assert!(
        missing_from_html.is_empty(),
        "契約 class が生成 HTML から欠落しています: {missing_from_html:?}\n\
         フィクスチャが対象 class を出力しなくなった（実装変更で分岐・要素が\
         削除された）可能性があります。EXPECTED_CLASSES を実装に追随させるか、\
         フィクスチャを対象 class が出力される構成へ戻してください。"
    );

    let missing_from_css: Vec<&&str> = EXPECTED_CLASSES
        .iter()
        .filter(|c| !css_tokens.contains(**c))
        .collect();
    assert!(
        missing_from_css.is_empty(),
        "契約 class が site/assets/site.css にセレクタとして定義されていません: \
         {missing_from_css:?}"
    );

    let orphaned_in_css: Vec<&String> = css_tokens
        .iter()
        .filter(|t| !expected.contains(t.as_str()))
        .collect();
    assert!(
        orphaned_in_css.is_empty(),
        "site/assets/site.css に契約リスト EXPECTED_CLASSES 外の class セレクタが\
         残存しています: {orphaned_in_css:?}\n\
         生成 HTML のどの経路からも出力されない孤立 class の可能性があります。\
         実際に使われている経路があれば、当該 class が出力されるようフィクスチャを\
         変更した上で EXPECTED_CLASSES へ追加してください（EXPECTED_CLASSES へ\
         追加するだけでは missing_from_html で失敗します）。使われていなければ\
         site.css から削除してください。"
    );
}

#[test]
fn extract_css_class_selectors_ignores_decimal_numbers() {
    let css = "margin: 0.5rem; .docs-toc { color: red; }";
    let tokens = extract_css_class_selectors(css);
    assert!(tokens.contains("docs-toc"));
    assert!(!tokens.contains("5rem"));
}

/// 3 カラムレイアウトの breakpoint 契約（イシュー #389 受け入れ条件 3）。
/// `≥1200px` で 3 カラム化・`<768px` で単列化するメディアクエリと、右カラム
/// （`.docs-toc-aside`）の表示切り替えが `site/assets/site.css` に存在する
/// ことを文字列レベルで検証する（fail-closed。実ブラウザ検証は CI 環境で
/// 不可なため、この契約テストと CSS 実装レビューの 2 点で担保する）。
#[test]
fn site_css_has_three_column_responsive_breakpoints() {
    let css = site_css();

    assert!(
        css.contains("min-width: 1200px"),
        "3 カラム化の breakpoint（min-width: 1200px）が見つからない"
    );
    assert!(
        css.contains("max-width: 767px"),
        "単列化の breakpoint（max-width: 767px）が見つからない"
    );
    assert!(
        css.contains(".docs-toc-aside") && css.contains("display: none"),
        ".docs-toc-aside の非表示切り替えが見つからない"
    );
}
