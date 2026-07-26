//! docs サイトのビルドパイプライン本体（イシュー #470）。
//!
//! # 呼び出し文脈
//!
//! バイナリの `main`（バイナリ本体、引数パース・終了コード変換のみを担う薄い
//! ラッパー）と `tests/site_build.rs`（E2E テスト）の双方から [`build_site`]
//! を直接呼ぶ。bin/lib 両方から同一のビルドロジックを共有するために本モジュール
//! を `lib.rs` 側に置く。
//!
//! # 処理順（fail-closed）
//!
//! 1. `<repo_root>/site/nav.toml` を [`nav::parse_nav`] → [`nav::validate_sources`]
//! 2. 各ページの Markdown を [`markdown::render_markdown`](crate::markdown::render_markdown)
//!    → [`linkcheck::rewrite_md_links`]
//!    （`.md` リンクをサイト内パスへ書き換え）→ [`layout::docs_page`] で文書化
//! 3. [`linkcheck::check_links`] で全ページの内部リンクを突合検証し、1 件でも
//!    壊れていれば **書き出しより前に** [`BuildError::LinkCheck`] で失敗させる
//!    （「一部だけ更新された dist/」を残さない。`ssg::generate_pages` 自体も
//!    同じ fail-closed 方針だが、linkcheck はそれより手前の層で同じ方針を守る）
//! 4. [`fandhe_frontend_server::ssg::generate_pages`] で `out_dir` へ書き出す
//! 5. `<repo_root>/site/assets/` 配下を `<out_dir>/assets/` へコピーする
//!    （通常ファイルのみ許可。シンボリックリンク・ディレクトリ以外の特殊
//!    エントリはエラーにする fail-closed。リポジトリ外ファイルの持ち出し
//!    防止のため走査対象を固定ディレクトリに限定する）

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use fandhe_frontend_core::{Node, div};
use fandhe_frontend_server::ssg::{self, SsgError};

use crate::layout;
use crate::linkcheck::{self, BrokenLink};
use crate::markdown::render_markdown;
use crate::nav::{self, NavError};
use crate::script;
use crate::search::{self, IndexTooLarge, SearchIndex, SearchPage, SearchSection};

/// [`build_site`] が成功時に返すビルド結果のサマリ。
#[derive(Debug, Clone)]
pub struct BuildReport {
    /// 書き出したページファイルの絶対パス一覧（`generate_pages` の戻り値）。
    pub written: Vec<PathBuf>,
    /// コピーしたアセットファイルの絶対パス一覧。
    pub assets: Vec<PathBuf>,
}

/// [`build_site`] の失敗理由。
///
/// `Display` はリポジトリ相対パス・行番号・href のみを含み、絶対パス・
/// 環境変数・スタックトレース等の機微情報は含めない
/// （`security.md` の機微情報露出防止方針。[`NavError`] と同方針）。
#[derive(Debug)]
pub enum BuildError {
    /// `site/nav.toml` の読込・パース・ソース存在検証のいずれかが失敗した。
    Nav(NavError),
    /// ページ Markdown の読込・アセットコピーで I/O エラーが発生した。
    Io {
        /// 対象パス（表示用。`repo_root` からの相対パスを優先して構成する）。
        path: PathBuf,
        /// 発生した I/O エラー。
        source: std::io::Error,
    },
    /// `fandhe_frontend_server::ssg::generate_pages` が失敗した。
    Ssg(SsgError),
    /// 内部リンクの突合検証（`.md` リンク解決を含む）で 1 件以上のリンク
    /// 切れが見つかった。書き出しは一切行われていない。
    LinkCheck(Vec<BrokenLink>),
    /// `site/assets/` 配下にシンボリックリンク・ディレクトリ以外の特殊
    /// エントリが存在し、通常ファイルのみ許可する方針に反した
    /// （リポジトリ外ファイルの持ち出し防止のための fail-closed 検証）。
    UnsupportedAssetEntry(PathBuf),
    /// [`script::SCRIPT_REL_PATH`]（`assets/site.js`）や
    /// [`search::INDEX_REL_PATH`]（`assets/search-index.json`）と同名の
    /// 静的アセットが `site/assets/` 配下に既に存在し、ビルド生成物との
    /// ファイル名衝突が起きる（イシュー #390・#396。生成物が静的アセットを
    /// 暗黙に上書きしない fail-closed 検証。書き出しより前に検出し、
    /// `out_dir` には一切書き出さない）。
    AssetNameCollision(PathBuf),
    /// 検索インデックス（[`search::serialize_index`]）の直列化結果が
    /// [`search::MAX_INDEX_BYTES`] を超過した（イシュー #396。
    /// [`search::validate_index_size`] の fail-closed 検証結果）。
    SearchIndexTooLarge(IndexTooLarge),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::Nav(e) => write!(f, "{e}"),
            BuildError::Io { path, source } => {
                write!(f, "I/O error at {path:?}: {source}")
            }
            BuildError::Ssg(e) => write!(f, "{e}"),
            BuildError::LinkCheck(broken) => {
                writeln!(f, "link check failed with {} broken link(s):", broken.len())?;
                for (i, b) in broken.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "  - {b}")?;
                }
                Ok(())
            }
            BuildError::UnsupportedAssetEntry(path) => {
                write!(
                    f,
                    "unsupported entry under site/assets/ (only regular files are allowed): {path:?}"
                )
            }
            BuildError::AssetNameCollision(path) => {
                write!(
                    f,
                    "static asset collides with a generated artifact: {path:?}"
                )
            }
            BuildError::SearchIndexTooLarge(IndexTooLarge { bytes, max }) => {
                write!(
                    f,
                    "search index size {bytes} bytes exceeds the {max} byte limit"
                )
            }
        }
    }
}

impl std::error::Error for BuildError {}

impl From<NavError> for BuildError {
    fn from(e: NavError) -> Self {
        BuildError::Nav(e)
    }
}

impl From<SsgError> for BuildError {
    fn from(e: SsgError) -> Self {
        BuildError::Ssg(e)
    }
}

/// `repo_root/site/nav.toml` を読み込み、全ページを組み立て、内部リンクを
/// 検証した上で `out_dir` へ書き出す。
///
/// # Errors
///
/// [`BuildError`] の各種別を参照。リンク切れが 1 件でもあれば
/// [`BuildError::LinkCheck`] を返し、`out_dir` には一切書き出さない。
pub fn build_site(repo_root: &Path, out_dir: &Path) -> Result<BuildReport, BuildError> {
    let nav_path = repo_root.join("site/nav.toml");
    let nav_input = fs::read_to_string(&nav_path).map_err(|source| BuildError::Io {
        path: PathBuf::from("site/nav.toml"),
        source,
    })?;
    let nav = nav::parse_nav(&nav_input)?;
    nav::validate_sources(&nav, repo_root)?;
    check_no_asset_name_collision(repo_root)?;

    let source_to_path = linkcheck::source_to_path_map(&nav);

    let mut pages: Vec<(String, Node)> = Vec::new();
    let mut broken: Vec<BrokenLink> = Vec::new();
    let mut search_pages: Vec<SearchPage> = Vec::new();

    for section in &nav.sections {
        for page in &section.pages {
            let source_path = repo_root.join(&page.source);
            let markdown_input =
                fs::read_to_string(&source_path).map_err(|source_err| BuildError::Io {
                    path: PathBuf::from(&page.source),
                    source: source_err,
                })?;

            let blocks = render_markdown(&markdown_input);
            let raw_body = div(vec![], blocks);
            let rewritten_body = linkcheck::rewrite_md_links(
                raw_body,
                &page.source,
                &nav,
                &page.path,
                &source_to_path,
                &mut broken,
            );

            // 検索インデックスの入力は本文（`rewritten_body`）のみに限定する
            // （prev/next ナビ・サイドバー・ヘッダーを含めない、イシュー #396
            // 計画 3 節 5 項）。`with_heading_anchors` は本文を再構築しながら
            // TOC を収集する副作用があるため、`layout::docs_page` へ渡す本文
            // とは別に clone を取ってから索引用に呼ぶ（`Node: Clone` により
            // 安価な参照カウントではなく木の複製だが、docs サイトの
            // ビルド時専用処理でありホットパスではないため許容する）。
            let (_, toc_entries) = layout::with_heading_anchors(rewritten_body.clone());
            let text = search::truncate_at_char_boundary(
                &search::extract_plain_text(&rewritten_body),
                search::MAX_PAGE_TEXT_BYTES,
            )
            .to_string();
            search_pages.push(SearchPage {
                href: layout::asset_href(&nav.site.base_path, &page.path),
                title: page.title.clone(),
                sections: toc_entries
                    .into_iter()
                    .map(|entry| SearchSection {
                        level: entry.level,
                        id: entry.id,
                        title: entry.title,
                    })
                    .collect(),
                text,
            });

            let body = div(
                vec![],
                vec![rewritten_body, nav::prev_next_nav(&nav, &page.path)],
            );

            let document = layout::docs_page(
                &page.title,
                &nav.site.title,
                &nav.site.base_path,
                nav::sidebar(&nav, &page.path),
                body,
            );

            pages.push((page.path.clone(), document));
        }
    }

    let asset_hrefs = collect_asset_hrefs(repo_root, &nav.site.base_path)?;

    let mut link_check_broken = linkcheck::check_links(&pages, &nav.site.base_path, &asset_hrefs);
    broken.append(&mut link_check_broken);

    if !broken.is_empty() {
        // fail-closed: 書き出しより前に打ち切る。`generate_pages` にも
        // 到達させない（「一部だけ更新された dist/」を残さないため）。
        return Err(BuildError::LinkCheck(broken));
    }

    let search_index = SearchIndex {
        base_path: nav.site.base_path.clone(),
        pages: search_pages,
    };
    let search_index_json = search::serialize_index(&search_index);
    search::validate_index_size(&search_index_json, search::MAX_INDEX_BYTES)
        .map_err(BuildError::SearchIndexTooLarge)?;

    let written = ssg::generate_pages(&pages, out_dir)?;
    let mut assets = copy_assets(repo_root, out_dir)?;
    assets.push(write_site_js(out_dir)?);
    assets.push(write_search_index(out_dir, &search_index_json)?);

    Ok(BuildReport { written, assets })
}

/// ビルドが生成物として書き出す `out_dir` 相対パス一覧（`site/assets/`
/// 配下の同名静的アセットとの衝突検証対象）。[`script::SCRIPT_REL_PATH`]
/// （`assets/site.js`、イシュー #390）と [`search::INDEX_REL_PATH`]
/// （`assets/search-index.json`、イシュー #396）の 2 件。新しい生成物
/// アセットを追加する際はここへ加えることで [`check_no_asset_name_collision`]
/// が自動的に対象へ含める。
const RESERVED_GENERATED_ASSETS: [&str; 2] = [script::SCRIPT_REL_PATH, search::INDEX_REL_PATH];

/// `site/assets/` 配下に [`RESERVED_GENERATED_ASSETS`] のいずれかと同名の
/// 静的アセットが存在しないことを検証する（fail-closed。生成物として
/// 書き出すファイルを静的アセットが暗黙に上書き・混同されるのを防ぐ）。
/// 書き出しより前（`copy_assets` 呼び出しより前）に呼び、衝突があれば
/// `out_dir` に一切書き出させない。
fn check_no_asset_name_collision(repo_root: &Path) -> Result<(), BuildError> {
    for reserved in RESERVED_GENERATED_ASSETS {
        let collision_path = repo_root.join("site").join(reserved);
        if collision_path.exists() {
            return Err(BuildError::AssetNameCollision(PathBuf::from(format!(
                "site/{reserved}"
            ))));
        }
    }
    Ok(())
}

/// [`script::SITE_JS`] を `out_dir/assets/site.js`（[`script::SCRIPT_REL_PATH`]）
/// へ書き出す。`copy_assets` が `out_dir/assets/` を作成済みであることを
/// 前提とする（呼び出し順は [`build_site`] が保証する）。
fn write_site_js(out_dir: &Path) -> Result<PathBuf, BuildError> {
    let dest = out_dir.join(script::SCRIPT_REL_PATH);
    fs::write(&dest, script::SITE_JS).map_err(|source| BuildError::Io {
        path: dest.clone(),
        source,
    })?;
    Ok(dest)
}

/// 直列化済み検索インデックス JSON を `out_dir/assets/search-index.json`
/// （[`search::INDEX_REL_PATH`]）へ書き出す。[`write_site_js`] と同じく
/// `copy_assets` が `out_dir/assets/` を作成済みであることを前提とする
/// （呼び出し順は [`build_site`] が保証する）。呼び出し時点で
/// [`search::validate_index_size`] のサイズ検証は完了済みの契約
/// （`build_site` の処理順）。
fn write_search_index(out_dir: &Path, json: &str) -> Result<PathBuf, BuildError> {
    let dest = out_dir.join(search::INDEX_REL_PATH);
    fs::write(&dest, json).map_err(|source| BuildError::Io {
        path: dest.clone(),
        source,
    })?;
    Ok(dest)
}

/// `site/assets/` 配下の通常ファイル一覧から、突合検証用の href
/// （`base_path + "/assets/" + ファイル名`）列を構築する。
///
/// ディレクトリ走査自体は行わず [`copy_assets`] と同じ列挙ロジックを
/// 再利用しないのは、linkcheck を書き出しより前に完了させる本モジュールの
/// 処理順（モジュール冒頭の設計）上、コピーの副作用（`out_dir` への書き込み）
/// より前に href 集合だけを先に必要とするため。
fn collect_asset_hrefs(repo_root: &Path, base_path: &str) -> Result<Vec<String>, BuildError> {
    let assets_dir = repo_root.join("site/assets");
    let mut hrefs = Vec::new();
    for entry in list_regular_files(&assets_dir)? {
        let file_name = entry
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        hrefs.push(format!("{base_path}/assets/{file_name}"));
    }
    Ok(hrefs)
}

/// `dir` 直下の通常ファイルのみを列挙する。シンボリックリンク・
/// サブディレクトリ・その他特殊エントリが見つかった場合は
/// [`BuildError::UnsupportedAssetEntry`] を返す（fail-closed。
/// `site/assets/` はリポジトリ管理下の固定ディレクトリであり、想定外の
/// エントリ種別を許容しないことでリポジトリ外ファイルの持ち出し・
/// 予期しないシンボリックリンク追従を防ぐ）。
fn list_regular_files(dir: &Path) -> Result<Vec<PathBuf>, BuildError> {
    let read_dir = fs::read_dir(dir).map_err(|source| BuildError::Io {
        path: PathBuf::from("site/assets"),
        source,
    })?;
    let mut files = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|source| BuildError::Io {
            path: PathBuf::from("site/assets"),
            source,
        })?;
        // `DirEntry::metadata` はシンボリックリンク自体の種別を返す
        // （`std::fs::metadata` のようにリンク先を追跡することはない。
        // リンク先を追跡してしまうと、リンク先がリポジトリ外の通常ファイルの
        // 場合に判定をすり抜ける）。
        let file_type = entry
            .metadata()
            .map_err(|source| BuildError::Io {
                path: entry.path(),
                source,
            })?
            .file_type();
        if file_type.is_file() {
            files.push(entry.path());
        } else {
            return Err(BuildError::UnsupportedAssetEntry(entry.path()));
        }
    }
    files.sort();
    Ok(files)
}

/// `repo_root/site/assets/` 配下の通常ファイルを `out_dir/assets/` へコピーする。
fn copy_assets(repo_root: &Path, out_dir: &Path) -> Result<Vec<PathBuf>, BuildError> {
    let assets_dir = repo_root.join("site/assets");
    let out_assets_dir = out_dir.join("assets");
    fs::create_dir_all(&out_assets_dir).map_err(|source| BuildError::Io {
        path: out_assets_dir.clone(),
        source,
    })?;

    let mut copied = Vec::new();
    for src in list_regular_files(&assets_dir)? {
        let file_name = src.file_name().unwrap_or_default();
        let dest = out_assets_dir.join(file_name);
        fs::copy(&src, &dest).map_err(|source| BuildError::Io {
            path: dest.clone(),
            source,
        })?;
        copied.push(dest);
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト専用の一時ディレクトリ。`nav.rs`/`ssg.rs` のテストヘルパーと
    /// 同方針（外部クレート `tempfile` を追加しない方針、移植元と同様）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "fandhe-backend-docs-site-build-test-{tag}-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir for build.rs test");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_fixture_site(root: &Path) {
        fs::create_dir_all(root.join("site/assets")).unwrap();
        fs::write(
            root.join("site/nav.toml"),
            r#"
[site]
title = "Docs"
base_path = ""

[[section]]
title = "Guide"

[[section.page]]
title = "Intro"
source = "site/intro.md"
path = "/"

[[section.page]]
title = "Next"
source = "site/next.md"
path = "/next/"
"#,
        )
        .unwrap();
        fs::write(root.join("site/intro.md"), "# Intro\n\n[Next](./next.md)\n").unwrap();
        fs::write(root.join("site/next.md"), "# Next\n\nBack to intro.\n").unwrap();
        fs::write(root.join("site/assets/site.css"), "body{}\n").unwrap();
    }

    #[test]
    fn build_site_writes_pages_and_assets_for_valid_fixture() {
        let temp = TempDir::new("ok");
        write_fixture_site(&temp.0);
        let out_dir = temp.0.join("dist");

        let report = build_site(&temp.0, &out_dir).expect("valid fixture should build");
        assert_eq!(report.written.len(), 2);
        // `site/assets/site.css`（フィクスチャの静的アセット）+
        // `assets/site.js`（テーマトグル JS の生成物、イシュー #390）+
        // `assets/search-index.json`（検索インデックスの生成物、イシュー #396）
        // の 3 件。
        assert_eq!(report.assets.len(), 3);
        assert!(out_dir.join("index.html").exists());
        assert!(out_dir.join("next/index.html").exists());
        assert!(out_dir.join("assets/site.css").exists());
        assert!(out_dir.join("assets/site.js").exists());
        assert!(out_dir.join("assets/search-index.json").exists());

        let index_html = fs::read_to_string(out_dir.join("index.html")).unwrap();
        assert!(index_html.contains(r#"href="/next/""#));
        assert!(!index_html.contains(".md"));

        let site_js = fs::read_to_string(out_dir.join("assets/site.js")).unwrap();
        assert!(site_js.contains(crate::script::THEME_STORAGE_KEY));

        let search_index = fs::read_to_string(out_dir.join("assets/search-index.json")).unwrap();
        // 索引の入力は本文（`rewritten_body`）のみに限定される契約（イシュー
        // #396 計画 3 節 5 項）。フィクスチャの本文（"Back to intro."）は
        // 含まれるが、prev/next ナビ由来の文字列（`nav::prev_next_nav` が
        // 生成するリンクラベル）は含まれない回帰テスト。
        assert!(search_index.contains("Back to intro."));
        assert!(search_index.contains(r#""href":"/next/""#));
    }

    #[test]
    fn build_site_rejects_static_asset_colliding_with_generated_site_js() {
        let temp = TempDir::new("asset-collision");
        write_fixture_site(&temp.0);
        fs::write(temp.0.join("site/assets/site.js"), "console.log(1);\n").unwrap();
        let out_dir = temp.0.join("dist");

        let err = build_site(&temp.0, &out_dir)
            .expect_err("a static asset named site.js should collide with the generated script");
        assert!(matches!(err, BuildError::AssetNameCollision(_)));
        assert!(!out_dir.exists());
    }

    #[test]
    fn build_site_rejects_static_asset_colliding_with_generated_search_index() {
        let temp = TempDir::new("search-index-collision");
        write_fixture_site(&temp.0);
        fs::write(temp.0.join("site/assets/search-index.json"), "{}\n").unwrap();
        let out_dir = temp.0.join("dist");

        let err = build_site(&temp.0, &out_dir).expect_err(
            "a static asset named search-index.json should collide with the generated index",
        );
        assert!(matches!(err, BuildError::AssetNameCollision(_)));
        assert!(!out_dir.exists());
    }

    #[test]
    fn build_site_fails_closed_on_broken_md_link_without_writing_output() {
        let temp = TempDir::new("broken-md-link");
        write_fixture_site(&temp.0);
        fs::write(
            temp.0.join("site/intro.md"),
            "# Intro\n\n[Missing](./missing.md)\n",
        )
        .unwrap();
        let out_dir = temp.0.join("dist");

        let err = build_site(&temp.0, &out_dir).expect_err("broken .md link should fail the build");
        match err {
            BuildError::LinkCheck(broken) => {
                assert_eq!(broken.len(), 1);
                assert!(broken[0].href.contains("missing.md"));
            }
            other => panic!("expected LinkCheck, got {other:?}"),
        }
        assert!(!out_dir.exists());
    }

    #[test]
    fn build_site_fails_closed_on_broken_absolute_link_without_writing_output() {
        let temp = TempDir::new("broken-abs-link");
        write_fixture_site(&temp.0);
        fs::write(
            temp.0.join("site/intro.md"),
            "# Intro\n\n[Ghost](/does-not-exist/)\n",
        )
        .unwrap();
        let out_dir = temp.0.join("dist");

        let err = build_site(&temp.0, &out_dir).expect_err("broken absolute link should fail");
        assert!(matches!(err, BuildError::LinkCheck(_)));
        assert!(!out_dir.exists());
    }

    #[test]
    fn build_site_reports_nav_error_for_missing_nav_toml() {
        let temp = TempDir::new("missing-nav");
        let out_dir = temp.0.join("dist");
        let err = build_site(&temp.0, &out_dir).expect_err("missing nav.toml should fail");
        assert!(matches!(err, BuildError::Io { .. }));
    }

    #[test]
    fn build_site_rejects_directory_entry_under_assets() {
        let temp = TempDir::new("bad-asset-entry");
        write_fixture_site(&temp.0);
        fs::create_dir_all(temp.0.join("site/assets/nested")).unwrap();
        let out_dir = temp.0.join("dist");
        let err = build_site(&temp.0, &out_dir).expect_err("directory under assets should fail");
        assert!(matches!(err, BuildError::UnsupportedAssetEntry(_)));
        assert!(!out_dir.exists());
    }
}
