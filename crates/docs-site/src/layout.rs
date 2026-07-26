//! docs サイトの Linear Developers 風 2 カラムページ骨格。
//!
//! タイトル・サイドバー・本文の各 [`Node`] から、DOCTYPE を除いた完全な
//! HTML 文書 `Node`（`<html>` 要素）を組み立てる。生成した `Node` は
//! `fandhe_frontend_server::ssg::generate_pages()`（`crates/server/src/ssg.rs`）が
//! `<!DOCTYPE html>` を前置して書き出す契約であり、本モジュールは
//! DOCTYPE を出力しない（後続イシュー #470 がビルドエントリで接続する）。
//!
//! `fandhe_frontend_app::page_shell` との差分: `page_shell` は
//! `/static/style.css` と `hydrate.js` をハードコードした `String` を返す
//! CSR/SSR 向けの実装であり docs には流用できないため、本モジュールは
//! `base_path` を考慮したアセット参照（[`asset_href`]）を持つ `Node` 返却の
//! 別実装として新規に用意する。docs サイトは CSR/SSR 用の JS ハイドレーション
//! （`hydrate.js`）は行わないが、イシュー #390 でダークモードトグル専用の
//! 素の JS（[`crate::script`]）のみを `<head>`（FOUC 抑止インラインスニペット）
//! と全 `<link rel="stylesheet">` の後（`assets/site.js`、`defer`）に埋め込む。

use std::collections::HashSet;

use fandhe_frontend_core::{
    Node, a, article, aside, button, div, el, header, li, main_tag, nav, text, ul,
};

use crate::script;

/// docs サイトが公開する GitHub リポジトリへの外部リンク先（イシュー #390）。
/// ヘッダー右側のアクション領域（`div.docs-header-actions`）に固定表示する。
const REPOSITORY_URL: &str = "https://github.com/Fandhe-AI/fandhe-backend";

/// SkipNav（本文へのスキップリンク）の遷移先フラグメント id
/// （イシュー #391）。moved-in-from `fandhe_frontend_headless_ui::skip_nav`
/// の `DEFAULT_ID` と同値を採用し、移植元との命名整合を保つ。`docs_page` が
/// リンク側 `href="#..."` とターゲット側 `id="..."` の両方でこの定数を
/// 参照するため、値の変更は 1 箇所で完結する。
const SKIP_NAV_ID: &str = "fandhe-skip-nav";

/// ページ内目次（TOC）の 1 エントリ。
///
/// [`with_heading_anchors`] が本文 `Node` を走査して収集する。`level` は
/// 見出しタグに対応する（`h2` → 2 / `h3` → 3）。`id` はアンカー先の
/// `id` 属性値（新規注入 or 既存採用）、`title` は見出しの表示テキスト。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    /// 見出しレベル（`h2` → 2 / `h3` → 3）。
    pub level: u8,
    /// アンカー先 `id` 属性値。
    pub id: String,
    /// 見出しの表示テキスト（既定エスケープ前のプレーン文字列）。
    pub title: String,
}

/// 本文 `Node` を走査して `h2`/`h3` 見出しを検出し、`id` 属性を注入した
/// 本文と、文書出現順の [`TocEntry`] 列を返す。
///
/// 既存の `id` 属性を持つ見出しはそれを尊重してそのまま採用し、注入しない
/// （静的アンカーへのリンク互換性を壊さないため）。`id` が無い見出しには
/// 見出しテキストから決定的に生成した slug を注入する。同一 slug が複数
/// 生成される場合は `-2` `-3` … を付与して一意化する（同一入力に対して
/// 常に同一出力を返す決定性を保証する。REQ-6 のモード非依存性契約に倣う）。
///
/// 見出しテキストは配下の [`Node::Text`] を出現順に連結して得る
/// （[`Node::RawHtml`] は連結対象に含めない。docs-site クレートは
/// `raw_html()` を使わない方針のため通常は出現しないが、混入した場合でも
/// TOC タイトルに生 HTML 断片を取り込まない防御的実装）。
pub fn with_heading_anchors(body: Node) -> (Node, Vec<TocEntry>) {
    let mut entries = Vec::new();
    let mut used_ids = HashSet::new();
    let annotated = inject_heading_anchors(body, &mut entries, &mut used_ids);
    (annotated, entries)
}

/// [`with_heading_anchors`] の内部再帰実装。木を再構築しながら `h2`/`h3` を
/// 検出する。
fn inject_heading_anchors(
    node: Node,
    entries: &mut Vec<TocEntry>,
    used_ids: &mut HashSet<String>,
) -> Node {
    match node {
        Node::Element {
            tag,
            attrs,
            children,
        } => {
            let level = heading_level(tag);
            let new_children: Vec<Node> = children
                .into_iter()
                .map(|c| inject_heading_anchors(c, entries, used_ids))
                .collect();

            let Some(level) = level else {
                return Node::Element {
                    tag,
                    attrs,
                    children: new_children,
                };
            };

            let title = extract_text(&new_children);
            let existing_id = attrs
                .iter()
                .find(|(name, _)| name == "id")
                .map(|(_, value)| value.clone());

            let mut new_attrs = attrs;
            let id = match existing_id {
                Some(id) => {
                    // 著者指定 id が自動生成スラグ（または別の著者指定 id）と衝突する
                    // 場合、`used_ids.insert` は false を返す。ここで戻り値を無視すると
                    // 両見出しが同一 id を持ち TOC・静的 `#...` リンクが最初の見出ししか
                    // 指さなくなる（「既存 id を尊重する」契約は壊さず、衝突時のみ
                    // `unique_slug` で一意な variant を採番する）。
                    if used_ids.insert(id.clone()) {
                        id
                    } else {
                        let generated = unique_slug(&id, used_ids);
                        if let Some(entry) = new_attrs.iter_mut().find(|(name, _)| name == "id") {
                            entry.1 = generated.clone();
                        }
                        generated
                    }
                }
                None => {
                    let generated = unique_slug(&slugify(&title), used_ids);
                    new_attrs.push(("id".to_string(), generated.clone()));
                    generated
                }
            };

            entries.push(TocEntry { level, id, title });
            Node::Element {
                tag,
                attrs: new_attrs,
                children: new_children,
            }
        }
        other => other,
    }
}

/// 見出しタグ名からレベル（`h2` → 2 / `h3` → 3）を判定する。対象外のタグは
/// `None`。
fn heading_level(tag: &str) -> Option<u8> {
    match tag {
        "h2" => Some(2),
        "h3" => Some(3),
        _ => None,
    }
}

/// ノード列配下の [`Node::Text`] を出現順に連結する。[`Node::RawHtml`] は
/// 連結対象に含めない（見出しテキストに生 HTML 断片を混入させないため）。
fn extract_text(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        extract_text_into(node, &mut out);
    }
    out
}

/// [`extract_text`] の内部再帰実装。
fn extract_text_into(node: &Node, out: &mut String) {
    match node {
        Node::Text(s) => out.push_str(s),
        Node::Element { children, .. } => {
            for child in children {
                extract_text_into(child, out);
            }
        }
        Node::RawHtml(_) => {}
    }
}

/// 見出しテキストから id 用の slug を生成する。小文字化した上で英数字
/// （Unicode 含む。日本語見出しを許容するため）以外の連続を単一 `-` に
/// 置換し、先頭・末尾の `-` を除去する。結果が空文字列になる場合（記号の
/// みの見出し等）は `"section"` にフォールバックする。
fn slugify(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut slug = String::with_capacity(lower.len());
    let mut last_was_dash = false;
    for c in lower.chars() {
        if c.is_alphanumeric() {
            slug.push(c);
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "section".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `base` を `used_ids` に対して一意化する。既に使われていれば `-2` `-3` …
/// を付与し、決定的に一意な id を返す（採番結果を `used_ids` へ登録する）。
fn unique_slug(base: &str, used_ids: &mut HashSet<String>) -> String {
    if used_ids.insert(base.to_string()) {
        return base.to_string();
    }
    let mut suffix = 2u32;
    loop {
        let candidate = format!("{base}-{suffix}");
        if used_ids.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

/// [`TocEntry`] 列からページ内目次の `nav` `Node` を生成する。空なら
/// `None`（目次を出さない。見出しの無いページで空の `nav` を出力しない
/// ため）。
///
/// 各項目には `entry.level` に応じたレベルクラス
/// （`docs-toc-level-2` / `docs-toc-level-3`）を付与し、`h2`/`h3` の階層を
/// CSS 側のインデント表現で区別できるようにする（Bugbot 指摘 b0e41098:
/// 従来はフラットな `<li>` 列で `level` を一切参照しておらず、見出し階層が
/// マークアップ上で表現できなかった）。
pub fn toc_nav(entries: &[TocEntry]) -> Option<Node> {
    if entries.is_empty() {
        return None;
    }
    let items = entries
        .iter()
        .map(|entry| {
            let href = format!("#{}", entry.id);
            let level_class = format!("docs-toc-level-{}", entry.level);
            li(
                vec![("class", &level_class)],
                vec![a(vec![("href", &href)], vec![text(entry.title.clone())])],
            )
        })
        .collect();
    Some(nav(vec![("class", "docs-toc")], vec![ul(vec![], items)]))
}

/// `base_path` を考慮したアセット参照パスを生成する（受け入れ条件 3 の
/// 単一実装点。`docs_page` 内のアセットリンク・サイトルートリンクは必ず
/// 本関数を経由し、パス結合ロジックを重複させない）。
///
/// `base_path` の末尾スラッシュ・空文字列は正規化する。`relative` が
/// 空文字列の場合はサイトルート（`base_path` 直下）を指すパスを返す。
///
/// # Examples
///
/// ```
/// use fandhe_backend_docs_site::layout::asset_href;
///
/// assert_eq!(asset_href("", "assets/site.css"), "/assets/site.css");
/// assert_eq!(
///     asset_href("/fandhe-frontend", "assets/site.css"),
///     "/fandhe-frontend/assets/site.css"
/// );
/// assert_eq!(
///     asset_href("/fandhe-frontend/", "assets/site.css"),
///     "/fandhe-frontend/assets/site.css"
/// );
/// ```
pub fn asset_href(base_path: &str, relative: &str) -> String {
    let trimmed_base = base_path.trim_end_matches('/');
    let trimmed_relative = relative.trim_start_matches('/');

    if trimmed_relative.is_empty() {
        if trimmed_base.is_empty() {
            "/".to_string()
        } else {
            format!("{trimmed_base}/")
        }
    } else if trimmed_base.is_empty() {
        format!("/{trimmed_relative}")
    } else {
        format!("{trimmed_base}/{trimmed_relative}")
    }
}

/// タイトル・`base_path`・サイドバー・本文から完全な HTML 文書 `Node`
/// （`<html>` 要素）を組み立てる。
///
/// 内部で [`with_heading_anchors`] と [`toc_nav`] を適用し、本文中の
/// `h2`/`h3` にアンカーを注入した上でページ内目次を生成する。`title`（ページ
/// タイトル）と `site_title`（全ページ共通ヘッダのホームリンク文言。
/// `site/nav.toml` の `[site] title` を渡す契約）は [`text`] 経由で、
/// `sidebar`/`body` はそのまま `Node` 木として埋め込むため
/// テキストスロットはすべて既定エスケープ済みで出力される（`raw_html()`・
/// HTML 文字列の直接組み立ては一切行わない）。
///
/// `<!DOCTYPE html>` の前置は呼び出し側
/// （`fandhe_frontend_server::ssg::generate_pages()`）の契約であり、本関数は
/// 文書 `Node` を返すのみで DOCTYPE 文字列を出力しない。
pub fn docs_page(
    title: &str,
    site_title: &str,
    base_path: &str,
    sidebar: Node,
    body: Node,
) -> Node {
    let (annotated_body, toc_entries) = with_heading_anchors(body);
    let toc = toc_nav(&toc_entries);

    let mut head_children = vec![
        el("meta", vec![("charset", "utf-8")], vec![]),
        el(
            "meta",
            vec![
                ("name", "viewport"),
                ("content", "width=device-width, initial-scale=1"),
            ],
            vec![],
        ),
        el("title", vec![], vec![text(title.to_string())]),
    ];
    // FOUC 抑止のインラインスニペット（イシュー #390）。全
    // `<link rel="stylesheet">` より前に同期実行させ、保存済みテーマが
    // あれば CSS 適用前に `data-theme` を確定させる。
    // `script::inline_theme_bootstrap` が `None`（エスケープ安全性検証に
    // 落ちた）場合は `<script>` 自体を出力しない fail-closed
    // （`crate::script` モジュール doc 参照）。
    if let Some(bootstrap) = script::inline_theme_bootstrap() {
        head_children.push(el("script", vec![], vec![text(bootstrap)]));
    }
    head_children.push(el(
        "style",
        vec![],
        vec![text("@view-transition { navigation: auto; }")],
    ));
    head_children.push(el(
        "link",
        vec![
            ("rel", "stylesheet"),
            ("href", &asset_href(base_path, "assets/site.css")),
        ],
        vec![],
    ));
    // 全 `<link rel="stylesheet">` の後に `assets/site.js`（イシュー #390）を
    // `defer` で読み込む。`src` はスクリプト本文を含まないため
    // `is_url_attr`/`is_safe_url`（`fandhe_frontend_core`）の既存検証を通る
    // 通常のアセット参照（[`asset_href`] 経由の単一実装点）。
    head_children.push(el(
        "script",
        vec![
            ("src", &asset_href(base_path, script::SCRIPT_REL_PATH)),
            ("defer", ""),
        ],
        vec![],
    ));
    let head = el("head", vec![], head_children);

    // 「on this page」目次は本文の前（`main` 内の先頭）に置く。読者が本文を
    // 読み始める前に目次へ気付けるようにするための並び順であり、
    // `site/assets/site.css` はこの `.docs-content` 前という位置関係を
    // 前提にスタイルしていない（`.docs-toc` 単体で完結する見た目にしている
    // ため、並び順を変えてもレイアウトは崩れない）。
    let mut main_children = Vec::new();
    if let Some(toc_node) = toc {
        main_children.push(toc_node);
    }
    // SkipNav のスキップ先ターゲット（イシュー #391）。`article` 直前に置き、
    // `.skip-nav` リンクからの遷移でキーボード利用者がヘッダ・サイドバーを
    // 経由せず本文直前へ到達できるようにする（WCAG 2.1 SC 2.4.1 Bypass
    // Blocks）。`tabindex="-1"` によりプログラム的フォーカスのみを許可し、
    // 通常の Tab 順序には加えない（移植元 `pre_styled_ui::skip_nav` と同契約）。
    let skip_nav_target_id = format!("#{SKIP_NAV_ID}");
    main_children.push(div(vec![("id", SKIP_NAV_ID), ("tabindex", "-1")], vec![]));
    main_children.push(article(
        vec![("class", "docs-content")],
        vec![annotated_body],
    ));

    let root_href = asset_href(base_path, "");
    // ヘッダー右側のアクション群（GitHub リンク・テーマトグル、イシュー
    // #390）。`target="_blank"` + `rel="noopener noreferrer"`（OWASP A05:
    // tabnabbing 対策。開いた先から `window.opener` を操作される経路と
    // Referer 漏えいを防ぐ）。テーマトグルは既定 `hidden`（JS 無効時・
    // `site.js` の読み込み失敗時は `site/assets/site.css` の
    // `.docs-theme-toggle[hidden]` が非表示を担保し、`prefers-color-scheme`
    // 追従へ退避する）。可視化・イベント配線は `crate::script::SITE_JS` の
    // みが行う（`crate::script` モジュール doc 手順 5 参照）。
    let header_actions = div(
        vec![("class", "docs-header-actions")],
        vec![
            a(
                vec![
                    ("href", REPOSITORY_URL),
                    ("class", "docs-github-link"),
                    ("target", "_blank"),
                    ("rel", "noopener noreferrer"),
                ],
                vec![text("GitHub")],
            ),
            button(
                vec![
                    ("type", "button"),
                    ("class", "docs-theme-toggle"),
                    ("hidden", ""),
                    ("aria-label", "Toggle color theme"),
                    ("aria-pressed", "false"),
                ],
                vec![text("Theme")],
            ),
        ],
    );
    let header_node = header(
        vec![("class", "docs-header")],
        vec![
            a(
                vec![("href", &root_href)],
                vec![text(site_title.to_string())],
            ),
            header_actions,
        ],
    );

    // SkipNav リンク（イシュー #391）。`<body>` 先頭に置き、既定は
    // `site/assets/site.css` の `.skip-nav` 規則で視覚上は隠しつつ Tab
    // 順序には含める（clip 手法）。キーボードフォーカス時のみ
    // `:focus-visible` で視覚復元し、ヘッダ・サイドバーを Tab で毎回
    // 通過させずに本文へ到達できるようにする。`href` は本モジュール内で
    // 組み立てた固定フラグメント（`#fandhe-skip-nav`）のみを指し、外部入力
    // 由来の URL・スキームを受理する経路は持たない。
    let skip_nav_link = a(
        vec![("class", "skip-nav"), ("href", &skip_nav_target_id)],
        vec![text("Skip to content".to_string())],
    );

    let body_node = el(
        "body",
        vec![],
        vec![
            skip_nav_link,
            header_node,
            div(
                vec![("class", "docs-container")],
                vec![
                    aside(vec![("class", "docs-sidebar")], vec![sidebar]),
                    main_tag(vec![("class", "docs-main")], main_children),
                ],
            ),
        ],
    );

    el("html", vec![("lang", "ja")], vec![head, body_node])
}
