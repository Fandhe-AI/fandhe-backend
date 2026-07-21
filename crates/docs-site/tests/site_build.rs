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
    assert_eq!(report.assets.len(), 1);
    assert!(out.0.join("index.html").exists());
    assert!(out.0.join("guide/quickstart/index.html").exists());
    assert!(out.0.join("assets/site.css").exists());
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
    assert!(out.0.join("index.html").exists());
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
