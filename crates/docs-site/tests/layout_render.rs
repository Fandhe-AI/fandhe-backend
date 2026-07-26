//! `fandhe-backend-docs-site::layout` の統合テスト（イシュー #469）。
//!
//! 受け入れ条件（完全文書組み立て・見出しアンカー抽出・アセットパス正規化）
//! と、XSS 回帰・決定性（REQ-6 のモード非依存性契約に倣う）を検証する。
//! `fandhe_frontend_server::ssg::generate_pages()` が `render()` 結果へ
//! `<!DOCTYPE html>` を前置する契約であるため、本テストは `layout::docs_page`
//! が返す `Node` に対する `render()` 出力のみを検証し DOCTYPE の有無は
//! 検証しない（DOCTYPE 前置は #470 でエントリ接続後に検証する）。

use fandhe_backend_docs_site::layout::{asset_href, docs_page, toc_nav, with_heading_anchors};
use fandhe_frontend_core::{h2, h3, li, p, render, text, ul};

fn sample_sidebar() -> fandhe_frontend_core::Node {
    ul(vec![], vec![li(vec![], vec![text("はじめに")])])
}

#[test]
fn docs_page_renders_a_single_complete_document() {
    let body = p(vec![], vec![text("本文です。")]);
    let node = docs_page("タイトル", "fandhe-backend", "", sample_sidebar(), body);
    let html = render(&node);

    assert!(html.starts_with("<html lang=\"ja\">"));
    assert!(html.contains("<head>"));
    assert!(html.contains("<title>タイトル</title>"));
    assert!(html.contains("はじめに"));
    assert!(html.contains("本文です。"));
    assert!(html.contains(r#"class="docs-sidebar""#));
    assert!(html.contains(r#"class="docs-content""#));
    assert!(html.contains(r#"href="/assets/site.css""#));
}

#[test]
fn docs_header_home_link_uses_given_site_title() {
    // ヘッダのホームリンク文言は引数 site_title（実運用では site/nav.toml の
    // `[site] title`）に従う。移植元サイト名のハードコード回帰
    // （PR #348 Bugbot 指摘）を防ぐ。
    let body = p(vec![], vec![text("本文です。")]);
    let node = docs_page("タイトル", "fandhe-backend", "", sample_sidebar(), body);
    let html = render(&node);

    assert!(html.contains(r#"<a href="/">fandhe-backend</a>"#));
    assert!(!html.contains(">fandhe-frontend<"));
}

#[test]
fn header_actions_contain_github_link_and_theme_toggle() {
    // イシュー #390: ヘッダー右側のアクション領域に GitHub リンク・
    // ダークモードトグルが出力されること。
    let body = p(vec![], vec![text("本文です。")]);
    let node = docs_page("タイトル", "fandhe-backend", "", sample_sidebar(), body);
    let html = render(&node);

    assert!(html.contains(r#"class="docs-header-actions""#));
    assert!(html.contains(r#"class="docs-github-link""#));
    assert!(html.contains(r#"href="https://github.com/Fandhe-AI/fandhe-backend""#));
    assert!(html.contains(r#"target="_blank""#));
    assert!(html.contains(r#"rel="noopener noreferrer""#));

    assert!(html.contains(r#"class="docs-theme-toggle""#));
    assert!(html.contains(r#"type="button""#));
    assert!(html.contains("hidden"));
    assert!(html.contains(r#"aria-label="Toggle color theme""#));
    assert!(html.contains(r#"aria-pressed="false""#));
}

#[test]
fn head_places_inline_theme_bootstrap_before_stylesheet_and_deferred_script_after() {
    // イシュー #390: FOUC 抑止インラインスニペットは stylesheet より前、
    // `assets/site.js`（defer）は stylesheet より後に出力される
    // （`layout::docs_page` モジュール doc・`script` モジュール doc 参照）。
    let body = p(vec![], vec![text("本文です。")]);
    let node = docs_page(
        "タイトル",
        "fandhe-backend",
        "/fandhe-backend",
        sample_sidebar(),
        body,
    );
    let html = render(&node);

    let bootstrap_pos = html
        .find("localStorage.getItem")
        .expect("inline theme bootstrap script should be present");
    let stylesheet_pos = html
        .find(r#"href="/fandhe-backend/assets/site.css""#)
        .expect("site.css stylesheet link should be present");
    let script_src_pos = html
        .find(r#"src="/fandhe-backend/assets/site.js""#)
        .expect("assets/site.js script tag should be present");

    assert!(
        bootstrap_pos < stylesheet_pos,
        "inline theme bootstrap must come before the stylesheet link"
    );
    assert!(
        stylesheet_pos < script_src_pos,
        "assets/site.js must come after the stylesheet link"
    );
    assert!(html.contains("defer"));
}

#[test]
fn heading_anchors_are_extracted_in_document_order_with_correct_levels() {
    let body = fandhe_frontend_core::div(
        vec![],
        vec![
            h2(vec![], vec![text("導入")]),
            p(vec![], vec![text("前置き")]),
            h3(vec![], vec![text("詳細")]),
        ],
    );
    let (annotated, entries) = with_heading_anchors(body);

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].level, 2);
    assert_eq!(entries[0].title, "導入");
    assert_eq!(entries[1].level, 3);
    assert_eq!(entries[1].title, "詳細");

    let html = render(&annotated);
    assert!(html.contains(&format!(r#"<h2 id="{}">導入</h2>"#, entries[0].id)));
    assert!(html.contains(&format!(r#"<h3 id="{}">詳細</h3>"#, entries[1].id)));
}

#[test]
fn duplicate_heading_titles_get_deterministic_unique_ids() {
    let body = fandhe_frontend_core::div(
        vec![],
        vec![
            h2(vec![], vec![text("概要")]),
            h2(vec![], vec![text("概要")]),
        ],
    );
    let (_, entries) = with_heading_anchors(body);

    assert_eq!(entries[0].id, "概要");
    assert_eq!(entries[1].id, "概要-2");
    assert_ne!(entries[0].id, entries[1].id);
}

#[test]
fn existing_heading_id_is_respected_and_not_overwritten() {
    let body = fandhe_frontend_core::div(
        vec![],
        vec![fandhe_frontend_core::el(
            "h2",
            vec![("id", "custom-anchor")],
            vec![text("見出し")],
        )],
    );
    let (annotated, entries) = with_heading_anchors(body);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "custom-anchor");
    let html = render(&annotated);
    assert!(html.contains(r#"<h2 id="custom-anchor">見出し</h2>"#));
}

#[test]
fn existing_id_colliding_with_autogenerated_slug_is_made_unique() {
    // 著者指定 id が既に自動生成スラグに確保済みの値と衝突するケース
    // （Cursor Bugbot 指摘 BUGBOT_BUG_ID: 6aa791a9-b7d6-4155-843e-3814b6b74504）。
    // 衝突を検出せず両見出しが同一 id を持つと、TOC・静的 `#...` リンクが
    // 最初の見出ししか指さなくなる。
    let body = fandhe_frontend_core::div(
        vec![],
        vec![
            h2(vec![], vec![text("概要")]),
            fandhe_frontend_core::el("h2", vec![("id", "概要")], vec![text("別の概要")]),
        ],
    );
    let (annotated, entries) = with_heading_anchors(body);

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].id, "概要");
    // 著者指定 id は尊重されつつ、衝突時のみ一意化される。
    assert_ne!(entries[1].id, "概要");
    assert!(entries[1].id.starts_with("概要-"));

    let html = render(&annotated);
    assert!(html.contains(&format!(r#"<h2 id="{}">概要</h2>"#, entries[0].id)));
    assert!(html.contains(&format!(r#"<h2 id="{}">別の概要</h2>"#, entries[1].id)));
    // 衝突後の id 属性は 1 つのみ出力されること（重複属性が残らないこと）。
    let second_open = html.rfind("<h2 id=").expect("second heading tag");
    assert_eq!(html[second_open..].matches(" id=").count(), 1);
}

#[test]
fn raw_html_children_are_not_concatenated_into_heading_title() {
    // docs-site クレートは raw_html() を使わない方針だが、混入時でも
    // TOC タイトルへ生 HTML 断片を取り込まない防御的実装を検証する。
    // raw_html() 呼び出しは clippy::disallowed_methods 対象のため、ここでは
    // 検証対象の `Node::RawHtml` バリアントを直接構築する（呼び出し経路の
    // レビューを要さない、列挙子の直接構築）。
    let body = fandhe_frontend_core::el(
        "h2",
        vec![],
        vec![
            text("見出し"),
            fandhe_frontend_core::Node::RawHtml("<b>強調</b>".to_string()),
        ],
    );
    let (_, entries) = with_heading_anchors(body);

    assert_eq!(entries[0].title, "見出し");
}

#[test]
fn no_headings_means_no_toc_nav_and_no_toc_section_in_document() {
    let body = p(vec![], vec![text("見出しのない本文")]);
    let (annotated, entries) = with_heading_anchors(body.clone());
    assert!(entries.is_empty());
    assert!(toc_nav(&entries).is_none());

    let node = docs_page("タイトル", "fandhe-backend", "", sample_sidebar(), body);
    let html = render(&node);
    assert!(!html.contains(r#"class="docs-toc""#));
    let _ = annotated;
}

#[test]
fn toc_nav_links_use_anchor_hrefs_matching_injected_ids() {
    let body = fandhe_frontend_core::div(vec![], vec![h2(vec![], vec![text("導入")])]);
    let node = docs_page("タイトル", "fandhe-backend", "", sample_sidebar(), body);
    let html = render(&node);

    assert!(html.contains(r#"class="docs-toc""#));
    // id 属性値と一致する #<id> アンカーが目次に出力されること。
    let id_marker = r#"<h2 id=""#;
    let start = html.find(id_marker).expect("h2 with injected id");
    let after = &html[start + id_marker.len()..];
    let end = after.find('"').expect("closing quote of id attr");
    let id = &after[..end];
    assert!(html.contains(&format!("href=\"#{id}\"")));
}

#[test]
fn toc_nav_items_carry_level_class_distinguishing_h2_and_h3() {
    // Bugbot 指摘 b0e41098: toc_nav が TocEntry::level を無視してフラットな
    // <li> を出すと h2/h3 の階層がマークアップから読み取れなくなる。
    // レベルクラス（docs-toc-level-2 / docs-toc-level-3）で区別できることを
    // 確認する回帰テスト。
    let body = fandhe_frontend_core::div(
        vec![],
        vec![
            h2(vec![], vec![text("導入")]),
            h3(vec![], vec![text("背景")]),
        ],
    );
    let (_, entries) = with_heading_anchors(body.clone());
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].level, 2);
    assert_eq!(entries[1].level, 3);

    let toc = toc_nav(&entries).expect("toc_nav must return Some for non-empty entries");
    let html = render(&toc);
    assert!(html.contains(r#"class="docs-toc-level-2""#));
    assert!(html.contains(r#"class="docs-toc-level-3""#));
}

#[test]
fn asset_href_normalizes_base_path_variants() {
    assert_eq!(asset_href("", "assets/site.css"), "/assets/site.css");
    assert_eq!(
        asset_href("/fandhe-frontend", "assets/site.css"),
        "/fandhe-frontend/assets/site.css"
    );
    assert_eq!(
        asset_href("/fandhe-frontend/", "assets/site.css"),
        "/fandhe-frontend/assets/site.css"
    );
    assert_eq!(asset_href("", ""), "/");
    assert_eq!(asset_href("/fandhe-frontend", ""), "/fandhe-frontend/");
}

#[test]
fn docs_page_output_is_deterministic_for_identical_input() {
    let make = || {
        let body = fandhe_frontend_core::div(
            vec![],
            vec![
                h2(vec![], vec![text("導入")]),
                p(vec![], vec![text("本文")]),
            ],
        );
        docs_page(
            "タイトル",
            "fandhe-backend",
            "/fandhe-backend",
            sample_sidebar(),
            body,
        )
    };
    assert_eq!(render(&make()), render(&make()));
}

#[test]
fn xss_payloads_in_title_headings_and_sidebar_are_escaped() {
    let payload = "<script>alert(1)</script>";
    let attr_payload = "\"><img src=x onerror=alert(1)>";

    let sidebar = ul(vec![], vec![li(vec![], vec![text(attr_payload)])]);
    let body = fandhe_frontend_core::div(vec![], vec![h2(vec![], vec![text(payload)])]);
    let node = docs_page(payload, payload, "", sidebar, body);
    let html = render(&node);

    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(!html.contains("<img src=x onerror=alert(1)>"));
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("&quot;&gt;&lt;img"));

    // 悪意ある見出しから生成した slug は英数字と `-` のみに正規化され、
    // 属性値エスケープを経由する（id 属性は render_into 側で常に
    // escape_html_into を通る。生成 slug 自体に `"` `<` `>` を含まない）。
    let (_, entries) = with_heading_anchors(fandhe_frontend_core::div(
        vec![],
        vec![h2(vec![], vec![text(payload)])],
    ));
    let id = &entries[0].id;
    assert!(id.chars().all(|c| c.is_alphanumeric() || c == '-'));
}
