//! docs サイト内部リンクの書き換え・突合検証（イシュー #470）。
//!
//! # 呼び出し文脈
//!
//! [`build::build_site`](crate::build::build_site) から、各ページの Markdown
//! レンダリング結果を [`layout::docs_page`](crate::layout::docs_page) へ渡す
//! 前段（[`rewrite_md_links`]）と、全ページの文書 `Node` を組み立てた後・
//! `fandhe_frontend_server::ssg::generate_pages()` で書き出す前段
//! （[`check_links`]）の 2 箇所で呼ばれる。
//!
//! # 2 つの責務
//!
//! 1. [`rewrite_md_links`]: `docs/` 配下の Markdown 原稿は GitHub 上でもそのまま
//!    閲覧できるよう `[text](./other.md)` のようなソースファイル相対の `.md`
//!    リンクを使う。サイト出力では `.md` は存在しないため、`nav.toml` の
//!    `page.source` → `page.path` マッピングを使って `<a href>` を
//!    サイト内パスへ書き換える。マッピングに存在しない `.md` リンクは
//!    [`BrokenLink`] として収集する（ビルドを失敗させる契約は呼び出し元
//!    [`build::build_site`](crate::build::build_site) が担う）。
//! 2. [`check_links`]: 全ページの文書 `Node`（サイドバー・前後ナビ・TOC・
//!    本文を含む最終形）を対象に、内部リンク（絶対パス・相対パス・
//!    `#fragment`）が実在するページ・見出しアンカーを指しているかを検証する。
//!    外部リンク（http/https・protocol-relative）は対象外（ネットワーク到達性
//!    検証はスコープ外、実装計画参照）。
//!
//! # セキュリティ上の位置づけ
//!
//! 本モジュールはリンク文字列を読み取り・比較するのみで、ファイルシステム
//! アクセスは行わない。`.md` リンクの字句解決（[`resolve_segments`]）はサイト
//! パス生成のためのものであり、[`nav::validate_sources`](crate::nav::validate_sources)
//! が別途行うファイル存在確認・パストラバーサル対策（絶対パス禁止・`..`
//! セグメント拒否）を代替・迂回しない多層防御の一部として位置づける。
//! 生成する href はすべて [`fandhe_frontend_core::a`] 等のノード木 API 経由で
//! 属性値として設定され、`render()` 時に既定エスケープ（REQ-1）を通る。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use fandhe_frontend_core::{Node, find_attr_values};

use crate::nav::Nav;

/// 内部リンクの検証で見つかった 1 件の不整合（ビルド失敗時にまとめて報告する）。
///
/// `Display` はページパス・href・理由のみを含み、絶対パス・環境変数等の
/// 機微情報は含めない（`security.md` の機微情報露出防止方針。`NavError` と
/// 同方針）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokenLink {
    /// 壊れたリンクを含むページの `page.path`。
    pub page_path: String,
    /// 壊れている href（書き換え前・書き換え後いずれの表記かは呼び出し元の
    /// 文脈に依存する）。
    pub href: String,
    /// 壊れている理由（人間可読な短い説明）。
    pub reason: String,
}

impl fmt::Display for BrokenLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "page {:?}: broken link `{}` ({})",
            self.page_path, self.href, self.reason
        )
    }
}

/// `nav.sections` から `page.source` → `page.path` の対応表を構築する。
///
/// [`rewrite_md_links`] が `.md` リンクの解決先を引くために使う。
/// `nav::parse_nav` が `page.path` の一意性を既に保証しているため、
/// `source` の重複（複数ページが同じ Markdown ファイルを指す）だけが
/// 起こりうるが、その場合も後勝ちで問題ない（同じソースは同じ内容であり
/// リンク先として不整合を生まない）。
pub fn source_to_path_map(nav: &Nav) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for section in &nav.sections {
        for page in &section.pages {
            map.insert(page.source.clone(), page.path.clone());
        }
    }
    map
}

/// href を `(パス部分, フラグメント)` へ分割する。フラグメントが無ければ
/// `None`。
fn split_fragment(href: &str) -> (&str, Option<&str>) {
    match href.split_once('#') {
        Some((path, frag)) => (path, Some(frag)),
        None => (href, None),
    }
}

/// href が URI スキームを持つか（`scheme:` 形式で `:` の前に `/` を含まない）
/// を判定する。スキームを持つ値（`http:`/`https:`/`mailto:` 等）・
/// protocol-relative（`//host/...`）は本モジュールの書き換え・突合対象外
/// （外部リンクとして扱う）。
fn is_absolute_url(s: &str) -> bool {
    if s.starts_with("//") {
        return true;
    }
    match s.find(':') {
        Some(idx) => !s[..idx].contains('/'),
        None => false,
    }
}

/// `href` が `.md` リンク書き換えの対象かを判定し、対象なら
/// `(パス部分, フラグメント)` を返す。
///
/// 対象条件（実装計画より）: スキームなし・`#` 始まりでない・パス部分の
/// 末尾が `.md`。
fn md_link_target(href: &str) -> Option<(&str, Option<&str>)> {
    if href.is_empty() || href.starts_with('#') || is_absolute_url(href) {
        return None;
    }
    let (path, frag) = split_fragment(href);
    if path.ends_with(".md") {
        Some((path, frag))
    } else {
        None
    }
}

/// `base_segments`（ディレクトリを表すセグメント列）へ `relative`
/// （`/` 区切りの相対パス。`.`/`..` セグメントを許容）を適用し、解決後の
/// セグメント列を返す。`..` がルートより上に出ようとした場合は `None`
/// （エスケープ拒否。[`rewrite_md_links`]・[`check_links`] の双方が
/// 同一ロジックを共有する）。
fn resolve_segments(base_segments: &[String], relative: &str) -> Option<Vec<String>> {
    let mut segments = base_segments.to_vec();
    for seg in relative.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other.to_string()),
        }
    }
    Some(segments)
}

/// リポジトリ相対ソースパス（例: `"docs/guides/quickstart.md"`）の親
/// ディレクトリをセグメント列で返す。
fn source_dir_segments(source: &str) -> Vec<String> {
    let mut segments: Vec<String> = source.split('/').map(str::to_string).collect();
    segments.pop(); // ファイル名自身を除去する。
    segments
}

/// 本文 `Node` 中の `.md` リンクを nav の source → path 対応表で解決し、
/// サイト内 href（`base_path + page.path + #fragment`）へ書き換える。
///
/// 対応表に存在しない `.md` リンクは [`BrokenLink`] として `broken` へ
/// 追記し、href は書き換えずにそのまま残す（ビルド全体を失敗させる契約は
/// 呼び出し元が `broken` の非空を見て判断する）。木を再帰的に再構築する
/// （[`layout::inject_heading_anchors`](crate::layout) と同型のパターン）。
pub fn rewrite_md_links(
    node: Node,
    source: &str,
    nav: &Nav,
    page_path: &str,
    source_to_path: &BTreeMap<String, String>,
    broken: &mut Vec<BrokenLink>,
) -> Node {
    match node {
        Node::Element {
            tag,
            attrs,
            children,
        } => {
            let new_children: Vec<Node> = children
                .into_iter()
                .map(|c| rewrite_md_links(c, source, nav, page_path, source_to_path, broken))
                .collect();
            let new_attrs = if tag == "a" {
                rewrite_href_attrs(attrs, source, nav, page_path, source_to_path, broken)
            } else {
                attrs
            };
            Node::Element {
                tag,
                attrs: new_attrs,
                children: new_children,
            }
        }
        other => other,
    }
}

/// [`rewrite_md_links`] の `<a>` 属性列に対する処理本体。
fn rewrite_href_attrs(
    attrs: Vec<(String, String)>,
    source: &str,
    nav: &Nav,
    page_path: &str,
    source_to_path: &BTreeMap<String, String>,
    broken: &mut Vec<BrokenLink>,
) -> Vec<(String, String)> {
    attrs
        .into_iter()
        .map(|(name, value)| {
            if name != "href" {
                return (name, value);
            }
            let Some((path_part, frag)) = md_link_target(&value) else {
                return (name, value);
            };
            let base_segments = source_dir_segments(source);
            let Some(resolved_segments) = resolve_segments(&base_segments, path_part) else {
                broken.push(BrokenLink {
                    page_path: page_path.to_string(),
                    href: value.clone(),
                    reason: "relative .md link escapes the repository root".to_string(),
                });
                return (name, value);
            };
            let resolved_source = resolved_segments.join("/");
            let Some(target_path) = source_to_path.get(&resolved_source) else {
                broken.push(BrokenLink {
                    page_path: page_path.to_string(),
                    href: value.clone(),
                    reason: format!("no nav.toml page declares source `{resolved_source}`"),
                });
                return (name, value);
            };
            let new_href = match frag {
                Some(f) => format!("{}{}#{}", nav.site.base_path, target_path, f),
                None => format!("{}{}", nav.site.base_path, target_path),
            };
            (name, new_href)
        })
        .collect()
}

/// `path` を突合用のキーへ正規化する。末尾 `/` の有無を同一視するため、
/// ルート（`"/"`）以外は末尾 `/` を除去する（`generate_pages` の
/// パス正規化規則と同じ同一視。実装計画参照）。
fn normalize_target_key(path: &str) -> String {
    if path == "/" {
        "/".to_string()
    } else {
        path.trim_end_matches('/').to_string()
    }
}

/// 既知のリンクターゲット 1 件。id 集合はフラグメント付きリンク
/// （`/api/foo/#heading`）の検証に使う。
struct KnownTarget {
    ids: BTreeSet<String>,
}

/// 全ページ + 全アセットから成る既知ターゲット表を構築する。
fn build_known_targets(
    pages: &[(String, Node)],
    base_path: &str,
    asset_hrefs: &[String],
) -> BTreeMap<String, KnownTarget> {
    let mut targets = BTreeMap::new();
    for (page_path, node) in pages {
        let absolute = format!("{base_path}{page_path}");
        let ids: BTreeSet<String> = find_attr_values(node, "id").into_iter().collect();
        targets.insert(normalize_target_key(&absolute), KnownTarget { ids });
    }
    for asset in asset_hrefs {
        targets.insert(
            normalize_target_key(asset),
            KnownTarget {
                ids: BTreeSet::new(),
            },
        );
    }
    targets
}

/// 全ページの文書 `Node`（サイドバー・前後ナビ・TOC・本文を含む最終形）を
/// 対象に内部リンクを突合検証する。
///
/// 分類:
/// - スキームあり（`http:`/`https:`）・protocol-relative（`//...`）→ 対象外
///   （外部リンク、ネットワーク到達性検証はスコープ外）
/// - `#fragment` のみ → 同一ページの id 集合と突合
/// - 絶対パス（`/...`）・相対パス → [`resolve_segments`] で解決した上で
///   既知ターゲット表と突合（末尾 `/` の有無は同一視する）。`#fragment` が
///   付く場合は解決先ページの id 集合とも突合する
///
/// 壊れたリンクをすべて収集して返す（1 件目で打ち切らない。是正効率のため）。
pub fn check_links(
    pages: &[(String, Node)],
    base_path: &str,
    asset_hrefs: &[String],
) -> Vec<BrokenLink> {
    let known = build_known_targets(pages, base_path, asset_hrefs);
    let mut broken = Vec::new();

    for (page_path, node) in pages {
        let own_absolute = format!("{base_path}{page_path}");
        let own_key = normalize_target_key(&own_absolute);
        let own_ids: BTreeSet<String> = known
            .get(&own_key)
            .map(|t| t.ids.clone())
            .unwrap_or_default();
        // href 解決の基準ディレクトリ。ページ href は必ず `/` 終わりの
        // ディレクトリ形式（nav::validate_page_path が保証）。
        let own_dir_segments: Vec<String> = own_absolute
            .trim_start_matches('/')
            .trim_end_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();

        for href in find_attr_values(node, "href") {
            if href.is_empty() || is_absolute_url(&href) {
                continue;
            }
            let (path_part, frag) = split_fragment(&href);

            if path_part.ends_with(".md") {
                // 未解決の `.md` リンクは `rewrite_md_links` が既に
                // `BrokenLink` として報告済み（`build::build_site` が両者を
                // 連結する）。ここで再度検証すると同一リンクが二重に報告
                // されるため、`.md` で終わる href はここでは扱わない
                // （書き換え済みリンクは `.md` を含まない絶対パスへ変換
                // されているため、本分岐には到達しない）。
                continue;
            }

            if path_part.is_empty() {
                // フラグメントのみのリンク（同一ページ内アンカー）。
                match frag {
                    Some(f) if own_ids.contains(f) => {}
                    Some(f) => broken.push(BrokenLink {
                        page_path: page_path.clone(),
                        href: href.clone(),
                        reason: format!("no element with id `{f}` on this page"),
                    }),
                    None => broken.push(BrokenLink {
                        page_path: page_path.clone(),
                        href: href.clone(),
                        reason: "empty href".to_string(),
                    }),
                }
                continue;
            }

            let resolved_segments = if let Some(rest) = path_part.strip_prefix('/') {
                resolve_segments(&[], rest)
            } else {
                resolve_segments(&own_dir_segments, path_part)
            };

            let Some(segments) = resolved_segments else {
                broken.push(BrokenLink {
                    page_path: page_path.clone(),
                    href: href.clone(),
                    reason: "relative link escapes the site root".to_string(),
                });
                continue;
            };

            let target_key = if segments.is_empty() {
                "/".to_string()
            } else {
                format!("/{}", segments.join("/"))
            };

            match known.get(&target_key) {
                None => broken.push(BrokenLink {
                    page_path: page_path.clone(),
                    href: href.clone(),
                    reason: "target does not match any generated page or asset".to_string(),
                }),
                Some(target) => {
                    if let Some(f) = frag
                        && !target.ids.contains(f)
                    {
                        broken.push(BrokenLink {
                            page_path: page_path.clone(),
                            href: href.clone(),
                            reason: format!("no element with id `{f}` on target page"),
                        });
                    }
                }
            }
        }
    }

    broken
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::parse_nav;
    use fandhe_frontend_core::{a, div, el, text};

    const NAV_TOML: &str = r#"
[site]
title = "Docs"
base_path = "/fandhe-frontend"

[[section]]
title = "Guide"

[[section.page]]
title = "Intro"
source = "docs/guide/intro.md"
path = "/guide/intro/"

[[section.page]]
title = "Next"
source = "docs/guide/next.md"
path = "/guide/next/"
"#;

    #[test]
    fn rewrite_md_links_resolves_relative_md_link_to_site_path() {
        let nav = parse_nav(NAV_TOML).unwrap();
        let map = source_to_path_map(&nav);
        let mut broken = Vec::new();
        let body = div(
            vec![],
            vec![a(vec![("href", "./next.md")], vec![text("Next")])],
        );
        let rewritten = rewrite_md_links(
            body,
            "docs/guide/intro.md",
            &nav,
            "/guide/intro/",
            &map,
            &mut broken,
        );
        assert!(broken.is_empty());
        let html = fandhe_frontend_core::render(&rewritten);
        assert!(html.contains(r#"href="/fandhe-frontend/guide/next/""#));
    }

    #[test]
    fn rewrite_md_links_preserves_fragment() {
        let nav = parse_nav(NAV_TOML).unwrap();
        let map = source_to_path_map(&nav);
        let mut broken = Vec::new();
        let body = a(vec![("href", "./next.md#section")], vec![text("Next")]);
        let rewritten = rewrite_md_links(
            body,
            "docs/guide/intro.md",
            &nav,
            "/guide/intro/",
            &map,
            &mut broken,
        );
        assert!(broken.is_empty());
        let html = fandhe_frontend_core::render(&rewritten);
        assert!(html.contains(r#"href="/fandhe-frontend/guide/next/#section""#));
    }

    #[test]
    fn rewrite_md_links_reports_unresolvable_md_link() {
        let nav = parse_nav(NAV_TOML).unwrap();
        let map = source_to_path_map(&nav);
        let mut broken = Vec::new();
        let body = a(vec![("href", "./missing.md")], vec![text("Missing")]);
        let _ = rewrite_md_links(
            body,
            "docs/guide/intro.md",
            &nav,
            "/guide/intro/",
            &map,
            &mut broken,
        );
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].page_path, "/guide/intro/");
        assert!(broken[0].reason.contains("missing.md"));
    }

    #[test]
    fn rewrite_md_links_rejects_escape_above_root() {
        let nav = parse_nav(NAV_TOML).unwrap();
        let map = source_to_path_map(&nav);
        let mut broken = Vec::new();
        let body = a(vec![("href", "../../../etc/passwd.md")], vec![text("x")]);
        let _ = rewrite_md_links(
            body,
            "docs/guide/intro.md",
            &nav,
            "/guide/intro/",
            &map,
            &mut broken,
        );
        assert_eq!(broken.len(), 1);
        assert!(broken[0].reason.contains("escapes"));
    }

    #[test]
    fn rewrite_md_links_leaves_non_md_and_absolute_links_untouched() {
        let nav = parse_nav(NAV_TOML).unwrap();
        let map = source_to_path_map(&nav);
        let mut broken = Vec::new();
        let body = div(
            vec![],
            vec![
                a(vec![("href", "https://example.com")], vec![text("ext")]),
                a(vec![("href", "/guide/next/")], vec![text("abs")]),
                a(vec![("href", "#frag")], vec![text("frag")]),
            ],
        );
        let rewritten = rewrite_md_links(
            body,
            "docs/guide/intro.md",
            &nav,
            "/guide/intro/",
            &map,
            &mut broken,
        );
        assert!(broken.is_empty());
        let html = fandhe_frontend_core::render(&rewritten);
        assert!(html.contains(r#"href="https://example.com""#));
        assert!(html.contains(r#"href="/guide/next/""#));
        assert!(html.contains("href=\"#frag\""));
    }

    #[test]
    fn check_links_accepts_valid_relative_and_absolute_and_fragment_links() {
        // ページ href は `/` 終わりのディレクトリ形式のため、相対リンクは
        // 「自分自身のディレクトリ配下」を指す（ブラウザの相対解決規則どおり。
        // 実装計画: site/index.md の `getting-started/quickstart/` が
        // ルートページ `/` からの相対リンクとして機能する根拠と同じ）。
        let page1 = (
            "/guide/intro/".to_string(),
            el(
                "html",
                vec![],
                vec![
                    el("h2", vec![("id", "section")], vec![text("Section")]),
                    a(vec![("href", "next/")], vec![text("relative")]),
                    a(
                        vec![("href", "/fandhe-frontend/guide/intro/#section")],
                        vec![text("self")],
                    ),
                    a(
                        vec![("href", "/fandhe-frontend/assets/site.css")],
                        vec![text("css")],
                    ),
                ],
            ),
        );
        let page2 = (
            "/guide/intro/next/".to_string(),
            el("html", vec![], vec![text("next page")]),
        );
        let pages = vec![page1, page2];
        let broken = check_links(
            &pages,
            "/fandhe-frontend",
            &["/fandhe-frontend/assets/site.css".to_string()],
        );
        assert!(broken.is_empty(), "unexpected broken links: {broken:?}");
    }

    #[test]
    fn check_links_detects_missing_target_and_missing_fragment() {
        let page1 = (
            "/guide/intro/".to_string(),
            el(
                "html",
                vec![],
                vec![
                    a(
                        vec![("href", "/guide/does-not-exist/")],
                        vec![text("missing")],
                    ),
                    a(vec![("href", "#missing-id")], vec![text("missing frag")]),
                ],
            ),
        );
        let pages = vec![page1];
        let broken = check_links(&pages, "/fandhe-frontend", &[]);
        assert_eq!(broken.len(), 2);
    }

    #[test]
    fn check_links_ignores_external_links() {
        let page1 = (
            "/guide/intro/".to_string(),
            el(
                "html",
                vec![],
                vec![
                    a(vec![("href", "https://example.com/x")], vec![text("ext")]),
                    a(vec![("href", "//example.com/x")], vec![text("protorel")]),
                ],
            ),
        );
        let broken = check_links(&[page1], "/fandhe-frontend", &[]);
        assert!(broken.is_empty());
    }
}
