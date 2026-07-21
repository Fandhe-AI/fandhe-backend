//! `site/nav.toml`（docs サイトのナビゲーション構成マニフェスト）のパース、
//! およびサイドバー・前後ページナビの [`Node`] 生成を担うモジュール。
//!
//! # 呼び出し文脈
//!
//! 後続イシュー #470 の `main.rs` から [`parse_nav`] → [`validate_sources`]
//! の順で呼ばれ、得られた [`Nav`] を #469（`layout.rs`）が [`sidebar`] /
//! [`prev_next_nav`] 経由でページレイアウトへ埋め込む。最終的な HTML は
//! `fandhe_frontend_server::ssg::generate_pages`（PR #477）へ渡される
//! `(path, Node)` の一部として書き出される。
//!
//! # 対応する TOML サブセット
//!
//! `nav.toml` は以下の構文のみを許可するサブセットとして扱う（それ以外は
//! すべて `NavError::Parse` で明示的に失敗する。fail-closed。未対応構文を
//! 黙って無視することはしない）。
//!
//! - `#` から始まる行コメント、および文字列値の終端後に続く `# ...`
//! - `[site]` テーブル（`title` / `base_path` の 2 キー）
//! - `[[section]]` array-of-tables（`title` キー）
//! - `[[section.page]]` array-of-tables（直前の `[[section]]` に属する。
//!   `title` / `source` / `path` の 3 キー）
//! - `key = "value"`（ダブルクォート文字列のみ。エスケープは `\"` `\\`
//!   `\n` `\t` の 4 種類のみ対応）
//!
//! 整数・真偽値・inline table・複数行文字列・配列などは非対応であり、
//! 出現した場合はエラーにする。
//!
//! # `crates/cli/src/toml.rs` を流用しない理由
//!
//! `fandhe-frontend-cli` の `structure.toml` 用パーサ（`crates/cli/src/toml.rs`）
//! は (a) `[[a]]` 形式の array-of-tables を明示的に拒否しており本モジュールが
//! 必要とする `[[section]]` / `[[section.page]]` を扱えない、(b) `cli` は
//! bin クレートで `lib` ターゲットを持たずクレート間で参照できない、(c) 仮に
//! ライブラリ化しても `docs-site` から `cli` への依存は `structure.toml` の
//! クレート責務境界（`docs-site` は `core`/`app`/`server` のみを
//! `depends_on` として宣言）に反する — の 3 点から、コード共有はせず
//! 同じ設計方針（fail-closed・行番号付きエラー・入力サイズ上限・
//! `unwrap()`/`expect()`/`panic!` 不使用）を踏襲した専用の最小パーサを
//! 本モジュールに自前実装する（イシュー #468 実装計画より）。

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use fandhe_frontend_core::{Node, el, text};

/// `nav.toml` 入力の上限サイズ（`crates/cli/src/toml.rs` の DoS 抑止方針と
/// 同値。再帰を使わない行単位パースのためネスト深度問題は生じないが、
/// 巨大入力そのものによる処理時間膨張は別途抑止する）。
const MAX_INPUT_BYTES: usize = 1024 * 1024;

/// `nav.toml` 全体をパースした結果のモデル。フィールドはすべて検証済み
/// （必須キー充足・`page.path` / `site.base_path` 形式・`page.path` 重複なし）。
/// `page.source` の実ファイル存在は [`validate_sources`] が別途担う
/// （パーサ本体を FS 非依存に保ち単体テストしやすくするため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nav {
    /// サイト全体設定。
    pub site: Site,
    /// 宣言順を保持したセクション列。
    pub sections: Vec<Section>,
}

/// `[site]` テーブル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// サイトタイトル。
    pub title: String,
    /// GitHub Pages プロジェクトサイト等でルート以外にホストする場合の
    /// ベースパス。`""` または `/` 始まり・`/` 終わりでない文字列。
    pub base_path: String,
}

/// `[[section]]` 1 件分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// サイドバーの見出しとして表示するセクションタイトル。
    pub title: String,
    /// 宣言順を保持したページ列（1 件以上、空セクションはパース時点でエラー）。
    pub pages: Vec<Page>,
}

/// `[[section.page]]` 1 件分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// サイドバー・前後ナビのリンクテキスト。
    pub title: String,
    /// Markdown ソースファイルの `repo_root` からの相対パス
    /// （[`validate_sources`] が実在確認する）。
    pub source: String,
    /// 出力 URL パス。`/` 始まり・`/` 終わり必須。
    pub path: String,
}

/// [`parse_nav`] / [`validate_sources`] の失敗理由。
///
/// `Display` 実装は行番号と理由のみを含み、入力全文・絶対パス・環境変数は
/// 含めない（`security.md` の機微情報露出防止方針。`crates/cli/src/toml.rs`
/// の `TomlError` と同方針）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavError {
    /// 入力サイズが `MAX_INPUT_BYTES` を超えた。
    TooLarge,
    /// 構文エラー（未知のテーブル・未知のキー・非対応の値型・重複キー等）。
    Parse {
        /// 1 始まりの行番号。ファイル全体に関するエラーは `0`。
        line: usize,
        /// エラー理由（入力値の断片は含めても入力全文は含めない）。
        message: String,
    },
    /// 複数セクションにまたがり `page.path` が重複している。
    DuplicatePath(String),
    /// `page.source` が `repo_root` 配下のファイルとして実在しない。
    MissingSource(String),
    /// `page.source` が相対パスの安全条件（絶対パス禁止・`..` 禁止・
    /// `\` 禁止）を満たさない。
    UnsafeSource(String),
    /// `page.path` が `/` 始まり・`/` 終わり、またはセグメントの
    /// ホワイトリスト（英数字・`-`・`_`）を満たさない。
    UnsafePagePath(String),
    /// 必須キーが欠落している。
    MissingKey {
        /// 欠落箇所（`"site"` / `"section"` / `"section.page"`）。
        context: String,
        /// 欠落したキー名。
        key: String,
    },
    /// セクションにページが 1 件も宣言されていない。
    EmptySection(String),
}

impl fmt::Display for NavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NavError::TooLarge => {
                write!(f, "nav.toml exceeds the {MAX_INPUT_BYTES} byte size limit")
            }
            NavError::Parse { line, message } => write!(f, "nav.toml:{line}: {message}"),
            NavError::DuplicatePath(path) => write!(f, "duplicate page.path `{path}`"),
            NavError::MissingSource(source) => {
                write!(f, "page.source `{source}` does not exist under repo_root")
            }
            NavError::UnsafeSource(source) => {
                write!(f, "page.source `{source}` is not a safe relative path")
            }
            NavError::UnsafePagePath(path) => write!(
                f,
                "page.path `{path}` must start and end with `/` with segments limited to alphanumerics, `-`, `_`"
            ),
            NavError::MissingKey { context, key } => {
                write!(f, "missing required key `{key}` in [{context}]")
            }
            NavError::EmptySection(title) => write!(f, "section `{title}` has no pages"),
        }
    }
}

impl std::error::Error for NavError {}

/// パース中に組み立て途上のセクション。必須キーの充足は全行走査後に
/// まとめて検証する（欠落順序に依存しない一貫したエラーにするため）。
struct SectionBuilder {
    title: Option<String>,
    pages: Vec<PageBuilder>,
}

struct PageBuilder {
    title: Option<String>,
    source: Option<String>,
    path: Option<String>,
}

/// 現在どのテーブルの直下を走査しているかを表す。`[[section.page]]` は
/// 直前に開始された `[[section]]`（`sections` の末尾）に属する。
enum Ctx {
    None,
    Site,
    Section(usize),
    Page(usize, usize),
}

fn parse_err(line: usize, message: impl Into<String>) -> NavError {
    NavError::Parse {
        line,
        message: message.into(),
    }
}

/// テーブルヘッダ・値の後続部分が「空、または `#` 始まりのコメント」で
/// あることを検証する。それ以外の残存文字列はサブセット外構文として拒否する。
fn check_trailing(rest: &str, line: usize) -> Result<(), NavError> {
    let rest = rest.trim_start();
    if rest.is_empty() || rest.starts_with('#') {
        Ok(())
    } else {
        Err(parse_err(
            line,
            format!("unexpected trailing content `{rest}`"),
        ))
    }
}

/// `value_part`（`=` の右側、先頭空白は trim 済み）からダブルクォート
/// 文字列 1 個を読み取る。エスケープは `\"` `\\` `\n` `\t` のみ対応。
/// 戻り値は `(パース済み文字列, 閉じクォート以降の残り文字列)`。
fn parse_quoted_string(value_part: &str, line: usize) -> Result<(String, &str), NavError> {
    let mut chars = value_part.char_indices();
    match chars.next() {
        Some((_, '"')) => {}
        _ => {
            return Err(parse_err(
                line,
                "expected a double-quoted string value (this parser accepts no other TOML value type)",
            ));
        }
    }

    let mut out = String::new();
    loop {
        match chars.next() {
            None => return Err(parse_err(line, "unterminated string literal")),
            Some((idx, '"')) => {
                let remainder = &value_part[idx + '"'.len_utf8()..];
                return Ok((out, remainder));
            }
            Some((_, '\\')) => match chars.next() {
                Some((_, '"')) => out.push('"'),
                Some((_, '\\')) => out.push('\\'),
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, other)) => {
                    return Err(parse_err(
                        line,
                        format!("unsupported escape sequence `\\{other}`"),
                    ));
                }
                None => return Err(parse_err(line, "unterminated escape sequence")),
            },
            Some((_, c)) => out.push(c),
        }
    }
}

fn set_once(
    slot: &mut Option<String>,
    value: String,
    line: usize,
    name: &str,
) -> Result<(), NavError> {
    if slot.is_some() {
        return Err(parse_err(line, format!("duplicate key `{name}`")));
    }
    *slot = Some(value);
    Ok(())
}

/// `id` が出力パス片として安全（英数字・`-`・`_` のみ、非空）かを検証する。
/// `fandhe_frontend_server::ssg` の `is_safe_path_segment` と同一の
/// ホワイトリストを、`generate_pages()` へ渡す前段で早期適用する
/// （多層防御。二重検証の意図はここに明記する）。
fn is_safe_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn validate_base_path(base_path: &str) -> Result<(), NavError> {
    if base_path.is_empty() {
        return Ok(());
    }
    if base_path.starts_with('/') && !base_path.ends_with('/') {
        Ok(())
    } else {
        Err(parse_err(
            0,
            format!(
                "site.base_path `{base_path}` must be \"\" or start with `/` and not end with `/`"
            ),
        ))
    }
}

fn validate_page_path(path: &str) -> Result<(), NavError> {
    if !path.starts_with('/') || !path.ends_with('/') {
        return Err(NavError::UnsafePagePath(path.to_string()));
    }
    if path.len() == 1 {
        // "/"（サイトトップ）はセグメントなしで許可する。
        //
        // 単一文字 "/" は開始・終了の '/' が同一バイトを指すため、下の
        // `path[1..path.len() - 1]` スライス（1..0）は範囲が逆転してパニック
        // する（イシュー #473 実装時に検出）。長さ 1 の場合はスライス計算に
        // 入る前に早期リターンする。
        return Ok(());
    }
    let inner = &path[1..path.len() - 1];
    if inner.is_empty() {
        // "//" のような縮退ケース。セグメントなしとして許可する
        // （現状 nav.toml では使用しないが、ホワイトリスト方式の
        // 対称性のため拒否しない）。
        return Ok(());
    }
    if inner.split('/').all(is_safe_path_segment) {
        Ok(())
    } else {
        Err(NavError::UnsafePagePath(path.to_string()))
    }
}

/// `source` が相対パスの安全条件（絶対パス禁止・`..` セグメント禁止・
/// `\` 禁止）を満たすかを構文レベルで検証する（パストラバーサル対策の
/// 早期検出。実ファイル存在確認は [`validate_sources`] が別途行う）。
fn validate_source_shape(source: &str) -> Result<(), NavError> {
    let looks_safe = !source.is_empty()
        && !source.starts_with('/')
        && !source.contains('\\')
        && source.split('/').all(|segment| segment != "..");
    if looks_safe {
        Ok(())
    } else {
        Err(NavError::UnsafeSource(source.to_string()))
    }
}

/// `nav.toml` の内容（文字列）をパースし、スキーマ・`page.path` /
/// `site.base_path` の形式・`page.path` の重複検証までを行う純関数。
/// ファイルシステムには一切アクセスしない（`page.source` の実在確認は
/// [`validate_sources`] を別途呼ぶこと）。
///
/// # Errors
///
/// 対応外の TOML 構文・必須キー欠落・空セクション・`page.path` 重複・
/// `page.path` / `site.base_path` の形式違反・`page.source` の構文上の
/// 危険性（絶対パス・`..`・`\`）のいずれかがあれば [`NavError`] を返す。
pub fn parse_nav(input: &str) -> Result<Nav, NavError> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(NavError::TooLarge);
    }

    let mut ctx = Ctx::None;
    let mut site_title: Option<String> = None;
    let mut site_base_path: Option<String> = None;
    let mut sections: Vec<SectionBuilder> = Vec::new();

    for (line_no0, raw_line) in input.lines().enumerate() {
        let line = line_no0 + 1;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("[[") {
            let end = rest
                .find("]]")
                .ok_or_else(|| parse_err(line, "expected closing `]]`"))?;
            let header = rest[..end].trim();
            check_trailing(&rest[end + 2..], line)?;
            match header {
                "section" => {
                    sections.push(SectionBuilder {
                        title: None,
                        pages: Vec::new(),
                    });
                    ctx = Ctx::Section(sections.len() - 1);
                }
                "section.page" => {
                    let sidx = sections.len().checked_sub(1).ok_or_else(|| {
                        parse_err(line, "[[section.page]] appeared before any [[section]]")
                    })?;
                    sections[sidx].pages.push(PageBuilder {
                        title: None,
                        source: None,
                        path: None,
                    });
                    let pidx = sections[sidx].pages.len() - 1;
                    ctx = Ctx::Page(sidx, pidx);
                }
                other => return Err(parse_err(line, format!("unknown table `[[{other}]]`"))),
            }
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix('[') {
            let end = rest
                .find(']')
                .ok_or_else(|| parse_err(line, "expected closing `]`"))?;
            let header = rest[..end].trim();
            check_trailing(&rest[end + 1..], line)?;
            match header {
                "site" => ctx = Ctx::Site,
                other => return Err(parse_err(line, format!("unknown table `[{other}]`"))),
            }
            continue;
        }

        let eq = trimmed
            .find('=')
            .ok_or_else(|| parse_err(line, "expected `key = \"value\"`"))?;
        let key = trimmed[..eq].trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(parse_err(line, format!("invalid key `{key}`")));
        }
        let value_part = trimmed[eq + 1..].trim_start();
        let (value, remainder) = parse_quoted_string(value_part, line)?;
        check_trailing(remainder, line)?;

        match ctx {
            Ctx::None => return Err(parse_err(line, "key-value pair outside of any table")),
            Ctx::Site => match key {
                "title" => set_once(&mut site_title, value, line, "site.title")?,
                "base_path" => set_once(&mut site_base_path, value, line, "site.base_path")?,
                other => return Err(parse_err(line, format!("unknown key `{other}` in [site]"))),
            },
            Ctx::Section(sidx) => match key {
                "title" => set_once(&mut sections[sidx].title, value, line, "section.title")?,
                other => {
                    return Err(parse_err(
                        line,
                        format!("unknown key `{other}` in [[section]]"),
                    ));
                }
            },
            Ctx::Page(sidx, pidx) => {
                let page = &mut sections[sidx].pages[pidx];
                match key {
                    "title" => set_once(&mut page.title, value, line, "page.title")?,
                    "source" => set_once(&mut page.source, value, line, "page.source")?,
                    "path" => set_once(&mut page.path, value, line, "page.path")?,
                    other => {
                        return Err(parse_err(
                            line,
                            format!("unknown key `{other}` in [[section.page]]"),
                        ));
                    }
                }
            }
        }
    }

    let site = Site {
        title: site_title.ok_or_else(|| NavError::MissingKey {
            context: "site".to_string(),
            key: "title".to_string(),
        })?,
        base_path: site_base_path.ok_or_else(|| NavError::MissingKey {
            context: "site".to_string(),
            key: "base_path".to_string(),
        })?,
    };
    validate_base_path(&site.base_path)?;

    if sections.is_empty() {
        return Err(parse_err(
            0,
            "nav.toml must declare at least one [[section]]",
        ));
    }

    let mut seen_paths: BTreeSet<String> = BTreeSet::new();
    let mut out_sections = Vec::with_capacity(sections.len());
    for section in sections {
        let title = section.title.ok_or_else(|| NavError::MissingKey {
            context: "section".to_string(),
            key: "title".to_string(),
        })?;
        if section.pages.is_empty() {
            return Err(NavError::EmptySection(title));
        }
        let mut out_pages = Vec::with_capacity(section.pages.len());
        for page in section.pages {
            let title = page.title.ok_or_else(|| NavError::MissingKey {
                context: "section.page".to_string(),
                key: "title".to_string(),
            })?;
            let source = page.source.ok_or_else(|| NavError::MissingKey {
                context: "section.page".to_string(),
                key: "source".to_string(),
            })?;
            let path = page.path.ok_or_else(|| NavError::MissingKey {
                context: "section.page".to_string(),
                key: "path".to_string(),
            })?;
            validate_page_path(&path)?;
            validate_source_shape(&source)?;
            if !seen_paths.insert(path.clone()) {
                return Err(NavError::DuplicatePath(path));
            }
            out_pages.push(Page {
                title,
                source,
                path,
            });
        }
        out_sections.push(Section {
            title,
            pages: out_pages,
        });
    }

    Ok(Nav {
        site,
        sections: out_sections,
    })
}

/// 各 `page.source` が `repo_root` 配下の実ファイルとして存在することを
/// 検証する。[`parse_nav`] から FS アクセスを分離し、単体テストを
/// ファイルシステムに依存させないための独立関数（イシュー #468 実装計画）。
///
/// # Errors
///
/// いずれかの `page.source` が `repo_root` 配下のファイルとして存在しない
/// 場合、最初に見つかった不在ファイルについて `NavError::MissingSource` を返す。
pub fn validate_sources(nav: &Nav, repo_root: &Path) -> Result<(), NavError> {
    for section in &nav.sections {
        for page in &section.pages {
            let full_path = repo_root.join(&page.source);
            if !full_path.is_file() {
                return Err(NavError::MissingSource(page.source.clone()));
            }
        }
    }
    Ok(())
}

/// `nav.site.base_path` + `page.path` を単純連結した href を返す。
/// 両者とも [`parse_nav`] で形式検証済み（`base_path` は `/` 終わりでない、
/// `path` は `/` 始まり）のため、二重 `/` は発生しない。
fn href(nav: &Nav, path: &str) -> String {
    format!("{}{}", nav.site.base_path, path)
}

/// サイドバー [`Node`] を生成する。セクション・ページとも宣言順で列挙し、
/// `current_path` に一致するページの `<a>` にのみ `aria-current="page"` と
/// `class="current"` を付与する。`current_path` が `nav` 中のどの
/// `page.path` にも一致しない場合はハイライトなしで全ページを列挙する
/// （サイトトップ等、nav セクション外のページが正当に存在しうるため
/// エラーにはしない契約）。
///
/// タイトル・href はすべて [`el`] / [`text`] 経由で組み立てられ、
/// `render()` 時に既定エスケープ（REQ-1）を必ず経由する。HTML 文字列の
/// 直接組み立て・`raw_html()` は使用しない。
pub fn sidebar(nav: &Nav, current_path: &str) -> Node {
    let mut section_nodes: Vec<Node> = Vec::new();
    for section in &nav.sections {
        let mut items: Vec<Node> = Vec::new();
        for page in &section.pages {
            let link_href = href(nav, &page.path);
            let is_current = page.path == current_path;
            let mut attrs: Vec<(&str, &str)> = vec![("href", &link_href)];
            if is_current {
                attrs.push(("aria-current", "page"));
                attrs.push(("class", "current"));
            }
            let link = el("a", attrs, vec![text(page.title.clone())]);
            items.push(el("li", vec![], vec![link]));
        }
        section_nodes.push(el("h2", vec![], vec![text(section.title.clone())]));
        section_nodes.push(el("ul", vec![], items));
    }
    el(
        "nav",
        vec![("class", "sidebar"), ("aria-label", "Documentation")],
        section_nodes,
    )
}

/// 全セクションを文書順（宣言順）に平坦化したページ列における、
/// `current_path` の前後ページを返す。`current_path` が見つからない場合は
/// `(None, None)`。先頭ページは `(None, Some(next))`、末尾ページは
/// `(Some(prev), None)` になる。
pub fn prev_next<'a>(nav: &'a Nav, current_path: &str) -> (Option<&'a Page>, Option<&'a Page>) {
    let flat: Vec<&Page> = nav.sections.iter().flat_map(|s| s.pages.iter()).collect();
    let Some(idx) = flat.iter().position(|p| p.path == current_path) else {
        return (None, None);
    };
    let prev = if idx > 0 { Some(flat[idx - 1]) } else { None };
    let next = flat.get(idx + 1).copied();
    (prev, next)
}

/// 前後ページリンクの [`Node`]（`<nav class="prev-next">` 配下に
/// 存在する側のみの `<a class="prev">` / `<a class="next">`）を生成する。
pub fn prev_next_nav(nav: &Nav, current_path: &str) -> Node {
    let (prev, next) = prev_next(nav, current_path);
    let mut children: Vec<Node> = Vec::new();
    if let Some(page) = prev {
        let link_href = href(nav, &page.path);
        children.push(el(
            "a",
            vec![("class", "prev"), ("href", &link_href)],
            vec![text(page.title.clone())],
        ));
    }
    if let Some(page) = next {
        let link_href = href(nav, &page.path);
        children.push(el(
            "a",
            vec![("class", "next"), ("href", &link_href)],
            vec![text(page.title.clone())],
        ));
    }
    el("nav", vec![("class", "prev-next")], children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_frontend_core::render;

    /// テスト専用の一時ディレクトリ。`Drop` でベストエフォート削除する。
    /// 外部クレート（`tempfile` 等）を追加せず `std::env::temp_dir()` +
    /// プロセス固有サフィックスで代用する（REQ-3: 外部依存ゼロを維持する。
    /// `crates/server/tests/support/temp_dir.rs` と同方針）。
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "fandhe-backend-docs-site-nav-test-{tag}-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create temp dir for nav.rs test");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SAMPLE: &str = r#"
[site]
title = "fandhe-frontend docs"
base_path = "/fandhe-frontend"

[[section]]
title = "Guide"

[[section.page]]
title = "Introduction"
source = "docs/guide/intro.md"
path = "/guide/intro/"

[[section.page]]
title = "Getting Started"
source = "docs/guide/getting-started.md"
path = "/guide/getting-started/"

[[section]]
title = "Reference"

[[section.page]]
title = "API"
source = "docs/reference/api.md"
path = "/reference/api/"
"#;

    // ---- 正常系（受け入れ条件 1） ----

    #[test]
    fn parses_site_sections_and_pages_in_declaration_order() {
        let nav = parse_nav(SAMPLE).expect("valid nav.toml should parse");
        assert_eq!(nav.site.title, "fandhe-frontend docs");
        assert_eq!(nav.site.base_path, "/fandhe-frontend");
        assert_eq!(nav.sections.len(), 2);
        assert_eq!(nav.sections[0].title, "Guide");
        assert_eq!(nav.sections[0].pages.len(), 2);
        assert_eq!(nav.sections[0].pages[0].title, "Introduction");
        assert_eq!(nav.sections[0].pages[0].source, "docs/guide/intro.md");
        assert_eq!(nav.sections[0].pages[0].path, "/guide/intro/");
        assert_eq!(nav.sections[0].pages[1].title, "Getting Started");
        assert_eq!(nav.sections[1].title, "Reference");
        assert_eq!(nav.sections[1].pages.len(), 1);
        assert_eq!(nav.sections[1].pages[0].path, "/reference/api/");
    }

    #[test]
    fn supports_full_line_and_trailing_comments() {
        let input = r#"
# full line comment
[site]
title = "Docs" # trailing comment
base_path = ""

[[section]] # comment after header
title = "Guide"

[[section.page]]
title = "Intro"
source = "intro.md"
path = "/intro/"
"#;
        let nav = parse_nav(input).expect("comments should be tolerated");
        assert_eq!(nav.site.title, "Docs");
        assert_eq!(nav.site.base_path, "");
    }

    #[test]
    fn supports_basic_string_escapes() {
        let input = r#"
[site]
title = "Line1\nLine2 \"quoted\" \\backslash\\"
base_path = ""

[[section]]
title = "S"

[[section.page]]
title = "P"
source = "p.md"
path = "/p/"
"#;
        let nav = parse_nav(input).expect("escapes should be supported");
        assert_eq!(nav.site.title, "Line1\nLine2 \"quoted\" \\backslash\\");
    }

    // ---- 異常系（受け入れ条件 3） ----

    #[test]
    fn rejects_duplicate_path_across_sections() {
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "A"

[[section.page]]
title = "P1"
source = "p1.md"
path = "/dup/"

[[section]]
title = "B"

[[section.page]]
title = "P2"
source = "p2.md"
path = "/dup/"
"#;
        match parse_nav(input) {
            Err(NavError::DuplicatePath(path)) => assert_eq!(path, "/dup/"),
            other => panic!("expected DuplicatePath, got {other:?}"),
        }
    }

    #[test]
    fn validate_sources_reports_missing_source_file() {
        let temp = TempDir::new("missing-source");
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "A"

[[section.page]]
title = "P1"
source = "does-not-exist.md"
path = "/p1/"
"#;
        let nav = parse_nav(input).expect("structurally valid nav.toml should parse");
        match validate_sources(&nav, &temp.0) {
            Err(NavError::MissingSource(source)) => assert_eq!(source, "does-not-exist.md"),
            other => panic!("expected MissingSource, got {other:?}"),
        }
    }

    #[test]
    fn validate_sources_accepts_existing_files() {
        let temp = TempDir::new("existing-source");
        std::fs::write(temp.0.join("p1.md"), b"# hello").expect("write fixture source file");
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "A"

[[section.page]]
title = "P1"
source = "p1.md"
path = "/p1/"
"#;
        let nav = parse_nav(input).expect("valid nav.toml should parse");
        assert!(validate_sources(&nav, &temp.0).is_ok());
    }

    #[test]
    fn rejects_parent_traversal_in_source() {
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "A"

[[section.page]]
title = "P1"
source = "../secret.md"
path = "/p1/"
"#;
        match parse_nav(input) {
            Err(NavError::UnsafeSource(source)) => assert_eq!(source, "../secret.md"),
            other => panic!("expected UnsafeSource, got {other:?}"),
        }
    }

    #[test]
    fn rejects_absolute_path_source() {
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "A"

[[section.page]]
title = "P1"
source = "/etc/passwd"
path = "/p1/"
"#;
        assert!(matches!(parse_nav(input), Err(NavError::UnsafeSource(_))));
    }

    /// イシュー #473 実装時に検出した回帰テスト。`path = "/"`
    /// （サイトトップ）は `validate_page_path` 内のスライス計算
    /// （`path[1..path.len() - 1]`）が `1..0` の逆転範囲になりパニックして
    /// いた。長さ 1 の早期リターンで解消したことを確認する。
    #[test]
    fn accepts_site_root_page_path() {
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "A"

[[section.page]]
title = "Top"
source = "index.md"
path = "/"
"#;
        let nav = parse_nav(input).expect("path = \"/\" should be accepted as the site root");
        assert_eq!(nav.sections[0].pages[0].path, "/");
    }

    #[test]
    fn rejects_page_path_without_leading_slash() {
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "A"

[[section.page]]
title = "P1"
source = "p1.md"
path = "p1/"
"#;
        assert!(matches!(parse_nav(input), Err(NavError::UnsafePagePath(_))));
    }

    #[test]
    fn rejects_page_path_without_trailing_slash() {
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "A"

[[section.page]]
title = "P1"
source = "p1.md"
path = "/p1"
"#;
        assert!(matches!(parse_nav(input), Err(NavError::UnsafePagePath(_))));
    }

    #[test]
    fn rejects_page_path_with_unsafe_segment_characters() {
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "A"

[[section.page]]
title = "P1"
source = "p1.md"
path = "/../p1/"
"#;
        assert!(matches!(parse_nav(input), Err(NavError::UnsafePagePath(_))));
    }

    #[test]
    fn rejects_missing_required_site_key() {
        let input = r#"
[site]
title = "Docs"

[[section]]
title = "A"

[[section.page]]
title = "P1"
source = "p1.md"
path = "/p1/"
"#;
        match parse_nav(input) {
            Err(NavError::MissingKey { context, key }) => {
                assert_eq!(context, "site");
                assert_eq!(key, "base_path");
            }
            other => panic!("expected MissingKey, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_section() {
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "Empty"
"#;
        match parse_nav(input) {
            Err(NavError::EmptySection(title)) => assert_eq!(title, "Empty"),
            other => panic!("expected EmptySection, got {other:?}"),
        }
    }

    #[test]
    fn rejects_section_page_before_any_section() {
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section.page]]
title = "Orphan"
source = "orphan.md"
path = "/orphan/"
"#;
        assert!(matches!(parse_nav(input), Err(NavError::Parse { .. })));
    }

    #[test]
    fn rejects_unsupported_value_types() {
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "A"

[[section.page]]
title = "P1"
source = "p1.md"
path = "/p1/"
weight = 1
"#;
        assert!(matches!(parse_nav(input), Err(NavError::Parse { .. })));
    }

    #[test]
    fn rejects_unterminated_string() {
        let input = "[site]\ntitle = \"unterminated\nbase_path = \"\"\n";
        assert!(matches!(parse_nav(input), Err(NavError::Parse { .. })));
    }

    #[test]
    fn rejects_input_larger_than_size_limit() {
        let mut input = String::from("[site]\ntitle = \"");
        input.push_str(&"a".repeat(MAX_INPUT_BYTES + 1));
        input.push_str("\"\nbase_path = \"\"\n");
        assert!(matches!(parse_nav(&input), Err(NavError::TooLarge)));
    }

    #[test]
    fn rejects_invalid_base_path() {
        let input = r#"
[site]
title = "Docs"
base_path = "no-leading-slash"

[[section]]
title = "A"

[[section.page]]
title = "P1"
source = "p1.md"
path = "/p1/"
"#;
        assert!(matches!(parse_nav(input), Err(NavError::Parse { .. })));
    }

    // ---- サイドバー（受け入れ条件 2） ----

    #[test]
    fn sidebar_lists_all_pages_in_document_order_with_current_highlighted() {
        let nav = parse_nav(SAMPLE).unwrap();
        let html = render(&sidebar(&nav, "/guide/getting-started/"));
        // 文書順: Introduction, Getting Started, API
        let intro_idx = html.find("Introduction").unwrap();
        let getting_started_idx = html.find("Getting Started").unwrap();
        let api_idx = html.find("API").unwrap();
        assert!(intro_idx < getting_started_idx);
        assert!(getting_started_idx < api_idx);

        assert!(html.contains(r#"href="/fandhe-frontend/guide/getting-started/""#));
        // 現在ページのみ aria-current="page" を持つ。
        assert_eq!(html.matches(r#"aria-current="page""#).count(), 1);
        assert!(html.contains(r#"class="current""#));
    }

    #[test]
    fn sidebar_has_no_highlight_when_current_path_absent() {
        let nav = parse_nav(SAMPLE).unwrap();
        let html = render(&sidebar(&nav, "/not-in-nav/"));
        assert!(!html.contains("aria-current"));
        assert!(!html.contains(r#"class="current""#));
    }

    #[test]
    fn sidebar_escapes_title_and_attribute_content() {
        let input = r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "<script>alert(1)</script>"

[[section.page]]
title = "Quote\"Title"
source = "p1.md"
path = "/p1/"
"#;
        let nav = parse_nav(input).unwrap();
        let html = render(&sidebar(&nav, "/p1/"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("Quote&quot;Title"));
    }

    // ---- 前後ナビ（受け入れ条件 2） ----

    #[test]
    fn prev_next_at_first_page_has_no_prev() {
        let nav = parse_nav(SAMPLE).unwrap();
        let (prev, next) = prev_next(&nav, "/guide/intro/");
        assert!(prev.is_none());
        assert_eq!(next.unwrap().path, "/guide/getting-started/");
    }

    #[test]
    fn prev_next_at_last_page_has_no_next() {
        let nav = parse_nav(SAMPLE).unwrap();
        let (prev, next) = prev_next(&nav, "/reference/api/");
        assert_eq!(prev.unwrap().path, "/guide/getting-started/");
        assert!(next.is_none());
    }

    #[test]
    fn prev_next_crosses_section_boundary() {
        let nav = parse_nav(SAMPLE).unwrap();
        let (prev, next) = prev_next(&nav, "/guide/getting-started/");
        assert_eq!(prev.unwrap().path, "/guide/intro/");
        assert_eq!(next.unwrap().path, "/reference/api/");
    }

    #[test]
    fn prev_next_absent_current_path_returns_none_none() {
        let nav = parse_nav(SAMPLE).unwrap();
        let (prev, next) = prev_next(&nav, "/not-in-nav/");
        assert!(prev.is_none());
        assert!(next.is_none());
    }

    #[test]
    fn prev_next_nav_renders_only_present_sides() {
        let nav = parse_nav(SAMPLE).unwrap();
        let html_first = render(&prev_next_nav(&nav, "/guide/intro/"));
        assert!(!html_first.contains(r#"class="prev""#));
        assert!(html_first.contains(r#"class="next""#));

        let html_last = render(&prev_next_nav(&nav, "/reference/api/"));
        assert!(html_last.contains(r#"class="prev""#));
        assert!(!html_last.contains(r#"class="next""#));
    }
}
