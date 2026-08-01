//! docs サイトビルドエントリの E2E テスト（イシュー #470 受け入れ条件）。
//!
//! `tests/fixtures/site-ok/` / `tests/fixtures/site-broken-link/` の
//! ミニリポジトリ（`site/nav.toml` + Markdown + `site/assets/`）に対して
//! [`fandhe_backend_docs_site::build::build_site`] を直接呼ぶテストと、
//! `env!("CARGO_BIN_EXE_docs-site")` でバイナリ本体を起動して終了コード・
//! stderr を検証するテストの 2 系統からなる。
//!
//! フィクスチャは cargo プロジェクトではない単なるディレクトリのため、
//! 共有 `CARGO_TARGET_DIR`（`ci.md`）のキャッシュ誤命中問題は生じない
//! （バイナリ実行のみで `cargo build` を再度呼ばない）。
//!
//! 受け入れ条件 3 の実サイトビルド検証（`env!("CARGO_MANIFEST_DIR")/../.."`
//! をルートに実際の `site/nav.toml` でビルド）もここに含める。以後の
//! docs 編集によるリンク切れを `cargo test` が継続的に検出する
//! （ドッグフーディング保証）。

use std::path::{Path, PathBuf};
use std::process::Command;

use fandhe_backend_docs_site::build::{BuildError, build_site};

/// テスト専用の一時出力ディレクトリ。`crates/docs-site/src/nav.rs` の
/// `TempDir` と同方針（外部クレート `tempfile` を追加しない、REQ-3）。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "fandhe-backend-docs-site-e2e-{tag}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp dir for site_build.rs test");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

// ---- lib 経由（build_site を直接呼ぶ） ----

#[test]
fn build_site_generates_all_pages_and_assets_for_ok_fixture() {
    let out = TempDir::new("ok");
    let report =
        build_site(&fixture_root("site-ok"), &out.0).expect("site-ok fixture should build");

    assert_eq!(report.written.len(), 2);
    // `site/assets/site.css`（フィクスチャの静的アセット）+
    // `assets/site.js`（テーマトグル JS の生成物、イシュー #390）+
    // `assets/search-index.json`（検索インデックスの生成物、イシュー #396）
    // の 3 件。
    assert_eq!(report.assets.len(), 3);
    assert!(out.0.join("index.html").exists());
    assert!(out.0.join("guide/quickstart/index.html").exists());
    assert!(out.0.join("assets/site.css").exists());
    assert!(out.0.join("assets/site.js").exists());
    assert!(out.0.join("assets/search-index.json").exists());
}

#[test]
fn build_site_rewrites_md_links_to_site_paths_for_ok_fixture() {
    let out = TempDir::new("md-rewrite");
    build_site(&fixture_root("site-ok"), &out.0).expect("site-ok fixture should build");

    let index_html = std::fs::read_to_string(out.0.join("index.html")).unwrap();
    assert!(index_html.contains(r#"href="/fixture-base/guide/quickstart/""#));
    assert!(!index_html.contains(".md"));

    let quickstart_html =
        std::fs::read_to_string(out.0.join("guide/quickstart/index.html")).unwrap();
    assert!(quickstart_html.contains(r#"href="/fixture-base/""#));
    assert!(!quickstart_html.contains(".md"));
}

#[test]
fn build_site_fails_closed_and_writes_nothing_for_broken_link_fixture() {
    let temp = TempDir::new("broken");
    // `TempDir::new` 自体が一時ディレクトリを作成するため、`out_dir` には
    // その配下の未作成サブディレクトリを渡す（fail-closed で一切書き出さない
    // ことを「サブディレクトリが作成されないこと」で検証するため）。
    let out_dir = temp.0.join("dist");
    let err = build_site(&fixture_root("site-broken-link"), &out_dir)
        .expect_err("site-broken-link fixture should fail the build");

    match err {
        BuildError::LinkCheck(broken) => {
            assert_eq!(broken.len(), 1);
            assert!(broken[0].href.contains("missing.md"));
        }
        other => panic!("expected LinkCheck, got {other:?}"),
    }
    assert!(
        !out_dir.exists(),
        "out_dir must not exist on link-check failure"
    );
}

/// 受け入れ条件 3: `cargo run -p fandhe-backend-docs-site -- --out dist/` が
/// リポジトリ自身の `site/nav.toml` で成功し続けることをドッグフーディング
/// 保証として固定する。以後の docs 編集によるリンク切れも本テストが検出する。
#[test]
fn build_site_succeeds_for_the_real_repository_site() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repository root");
    let out = TempDir::new("real-site");

    let report = build_site(&repo_root, &out.0).expect("real site/nav.toml should build cleanly");
    assert!(!report.written.is_empty());
    assert!(!report.assets.is_empty());
    // `site/nav.toml` の総ページ数を機械固定する（イシュー #389 受け入れ条件
    // 2: 既存ページ全てがリンク切れなくビルドできること）。ページ数が
    // 変わった場合はこの値も追随する必要がある（イシュー #400/#406/#407 で
    // Examples・API Reference・Guides 索引ページが追加され 13 → 20 に増加、
    // イシュー #433 で with-interceptor サンプルページが追加され 20 → 21 に増加、
    // イシュー #460 で Interceptor 専用 API リファレンスページが追加され
    // 21 → 22 に増加）。
    assert_eq!(report.written.len(), 22);
    assert!(out.0.join("index.html").exists());

    // 3 カラム構造（イシュー #389 受け入れ条件 1）: トップページに
    // `docs-container` / `docs-brand` が出力されること。トップページ
    // （site/index.md）に h2/h3 が無ければ `docs-toc-aside` は出力されない
    // 契約のため、右カラムの有無自体はここでは固定しない。
    let index_html = std::fs::read_to_string(out.0.join("index.html")).unwrap();
    assert!(index_html.contains(r#"class="docs-container""#));
    assert!(index_html.contains(r#"class="docs-brand""#));
    assert!(index_html.contains(r#"class="docs-sidebar""#));
    assert!(index_html.contains(r#"class="docs-main""#));

    // ダークモードトグル・GitHub リンク（イシュー #390）の実出力検証。
    assert!(out.0.join("assets/site.js").exists());
    assert!(index_html.contains("docs-theme-toggle"));
    assert!(index_html.contains("docs-github-link"));
    assert!(index_html.contains(r#"src="/fandhe-backend/assets/site.js""#));
}

/// ヘッダーセクションメニュー + サイドバー現在セクション絞り込みの E2E 検証。
/// 実サイトのトップページ（Getting Started セクション所属）で、
/// (a) 全 4 セクションのトリガーがヘッダーに出力される、(b) 現在セクションの
/// トリガーに `aria-current="true"` が付く、(c) サイドバーには現在セクション
/// のページのみが載り他セクションのページが載らない、を固定する。
#[test]
fn build_site_output_has_header_section_menu_and_scoped_sidebar() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repository root");
    let out = TempDir::new("header-nav");

    build_site(&repo_root, &out.0).expect("real site/nav.toml should build cleanly");
    let index_html = std::fs::read_to_string(out.0.join("index.html")).unwrap();

    // (a) ヘッダーセクションメニューと 4 セクションのトリガー。
    assert!(index_html.contains(r#"class="docs-header-nav""#));
    assert_eq!(
        index_html.matches(r#"class="docs-header-trigger""#).count(),
        4
    );
    assert!(index_html.contains(r#"class="docs-header-dropdown""#));

    // (b) 現在セクション（Getting Started、index_path = "/"）のトリガーのみ
    // aria-current="true"。
    assert_eq!(index_html.matches(r#"aria-current="true""#).count(), 1);
    assert!(
        index_html
            .contains(r#"class="docs-header-trigger" href="/fandhe-backend/" aria-current="true""#)
    );

    // (c) サイドバー（nav.sidebar）は現在セクションのみ。ヘッダードロップ
    // ダウンにも全ページリンクが載るため、サイドバー部分だけを切り出して
    // 検証する。
    let sidebar_start = index_html
        .find(r#"<nav class="sidebar""#)
        .expect("sidebar nav must exist");
    let sidebar_end = index_html[sidebar_start..]
        .find("</nav>")
        .map(|i| sidebar_start + i)
        .expect("sidebar nav must close");
    let sidebar_html = &index_html[sidebar_start..sidebar_end];
    assert!(sidebar_html.contains(r#"href="/fandhe-backend/getting-started/""#));
    assert!(!sidebar_html.contains(r#"href="/fandhe-backend/guides/""#));
    assert!(!sidebar_html.contains(r#"href="/fandhe-backend/api/server-api/""#));
    assert!(sidebar_html.contains(">Getting Started<"));
    assert!(!sidebar_html.contains(">Guides<"));
}

/// イシュー #391: 実サイトビルド出力に SkipNav・`aria-current="page"` 一本化が
/// 反映され、`class="current"` が残っていないことを E2E で固定する。
#[test]
fn build_site_output_contains_skip_nav_and_aria_current_only() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repository root");
    let out = TempDir::new("a11y");

    build_site(&repo_root, &out.0).expect("real site/nav.toml should build cleanly");
    let index_html = std::fs::read_to_string(out.0.join("index.html")).unwrap();

    assert!(index_html.contains(r#"class="skip-nav""#));
    assert!(index_html.contains(r#"id="fandhe-skip-nav""#));
    assert!(index_html.contains(r#"aria-current="page""#));
    assert!(!index_html.contains(r#"class="current""#));

    let css = std::fs::read_to_string(out.0.join("assets/site.css")).unwrap();
    assert!(css.contains(".skip-nav"));
}

/// イシュー #396 受け入れ条件 1: 実サイトビルドで検索インデックスが生成され、
/// サイズ上限（[`fandhe_backend_docs_site::search::MAX_INDEX_BYTES`]）以内に
/// 収まることを固定する。
#[test]
fn build_site_generates_search_index_within_size_limit_for_the_real_repository_site() {
    use fandhe_backend_docs_site::search::MAX_INDEX_BYTES;

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repository root");
    let out = TempDir::new("search-index");

    let report = build_site(&repo_root, &out.0).expect("real site/nav.toml should build cleanly");
    assert!(
        report
            .assets
            .iter()
            .any(|p| p.ends_with("search-index.json"))
    );

    let index_path = out.0.join("assets/search-index.json");
    assert!(index_path.exists());
    let bytes = std::fs::metadata(&index_path).unwrap().len() as usize;
    assert!(
        bytes < MAX_INDEX_BYTES,
        "search index size {bytes} bytes should be below the {MAX_INDEX_BYTES} byte limit"
    );
}

/// イシュー #396: 実サイトの検索インデックスが `site/nav.toml` の全ページ
/// （22 ページ、`build_site_succeeds_for_the_real_repository_site` と同数）を
/// 含み、`base_path`（`/fandhe-backend`）を保持することを固定する。生の
/// `< > &` および `U+2028`/`U+2029` を含まないことも合わせて検証する
/// （[`fandhe_backend_docs_site::search::escape_json_string`] の多層防御
/// エスケープ契約の回帰テスト）。
#[test]
fn build_site_search_index_covers_all_pages_and_escapes_defense_in_depth_characters() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repository root");
    let out = TempDir::new("search-index-content");

    build_site(&repo_root, &out.0).expect("real site/nav.toml should build cleanly");
    let index_json = std::fs::read_to_string(out.0.join("assets/search-index.json")).unwrap();

    assert!(index_json.contains(r#""base_path":"/fandhe-backend""#));
    // `"href":` の出現回数でページ数を数える（JSON パーサを追加依存させない
    // ため文字列探索で代替する。受け入れ条件 4: 外部 JS/依存ライブラリを
    // 追加しない方針とは独立に、テスト側も外部 JSON クレートを増やさない）。
    assert_eq!(index_json.matches(r#""href":"#).count(), 22);

    for forbidden in ['<', '>', '&', '\u{2028}', '\u{2029}'] {
        assert!(
            !index_json.contains(forbidden),
            "search index must not contain raw {forbidden:?}"
        );
    }
}

/// イシュー #396: 検索インデックスの直列化は決定的であり、2 回ビルドしても
/// バイト列が同一になることを固定する（[`serialize_index`] のキー順固定
/// 契約。fandhe_backend_docs_site::search::serialize_index の doc 参照）。
#[test]
fn build_site_search_index_is_byte_identical_across_repeated_builds() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repository root");
    let out_a = TempDir::new("search-index-det-a");
    let out_b = TempDir::new("search-index-det-b");

    build_site(&repo_root, &out_a.0).expect("real site/nav.toml should build cleanly (a)");
    build_site(&repo_root, &out_b.0).expect("real site/nav.toml should build cleanly (b)");

    let json_a = std::fs::read(out_a.0.join("assets/search-index.json")).unwrap();
    let json_b = std::fs::read(out_b.0.join("assets/search-index.json")).unwrap();
    assert_eq!(json_a, json_b);
}

// ---- バイナリ経由（終了コード・stderr の契約） ----

fn docs_site_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_docs-site"))
}

#[test]
fn binary_exits_zero_and_reports_written_counts_for_ok_fixture() {
    let out = TempDir::new("bin-ok");
    let output = Command::new(docs_site_bin())
        .arg("--root")
        .arg(fixture_root("site-ok"))
        .arg("--out")
        .arg(&out.0)
        .output()
        .expect("spawn docs-site binary");

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out.0.join("index.html").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("wrote 2 page(s)"));
}

#[test]
fn binary_exits_nonzero_with_link_check_report_for_broken_fixture() {
    let temp = TempDir::new("bin-broken");
    let out_dir = temp.0.join("dist");
    let output = Command::new(docs_site_bin())
        .arg("--root")
        .arg(fixture_root("site-broken-link"))
        .arg("--out")
        .arg(&out_dir)
        .output()
        .expect("spawn docs-site binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("link check failed"));
    assert!(stderr.contains("missing.md"));
    assert!(!out_dir.exists());
}

#[test]
fn binary_exits_nonzero_when_out_argument_is_missing() {
    let output = Command::new(docs_site_bin())
        .arg("--root")
        .arg(fixture_root("site-ok"))
        .output()
        .expect("spawn docs-site binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--out"));
}

#[test]
fn binary_exits_nonzero_for_unknown_argument() {
    let out = TempDir::new("bin-unknown-arg");
    let output = Command::new(docs_site_bin())
        .arg("--out")
        .arg(&out.0)
        .arg("--bogus")
        .output()
        .expect("spawn docs-site binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown argument"));
}
