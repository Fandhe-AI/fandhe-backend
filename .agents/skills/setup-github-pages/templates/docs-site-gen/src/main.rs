//! docs-site-gen: fandhe-frontend の docs-site を「外部サイト向け」に呼び出す薄い wrapper。
//!
//! # 役割・境界
//!
//! 対象リポジトリの `tools/docs-site-gen/` に置かれ、`build-local.sh`（ローカル・CI 共通）から
//! 呼ばれる。`<root>/site/nav.toml` と Markdown を読み、静的サイトを `<out>` へ書き出す。
//! 生成後の差し替え（ブランド表示）は別工程の `rebrand_site.py` が担う。本 wrapper はビルドのみ。
//!
//! # なぜ stock の `docs-site` バイナリを使わないか
//!
//! stock のバイナリは `build_site()` 経由で fandhe-frontend 専用の page section registry
//! （`/api/` `/blocks/` `/themes/` 等 8 パスを必須とし固有リンクを注入する）を強制するため、
//! 外部サイトでは「必須パスが無い」として失敗する。ここでは空の registry を渡す
//! `build_site_with` を呼び、registry 由来のページ・リンクを一切生成しない。
//! 副作用としてトップページはヒーロー/カードグリッドを持たない通常の Docs レイアウトになる。
//!
//! # 契約
//!
//! - 終了コード 0: 生成成功（リンク検査を通過）。1: 生成失敗（リンク切れ・nav 不正等。
//!   上流の検査は fail-closed で、1 件でも壊れていれば `<out>` に何も書かない）。2: 引数不正。
//! - 引数は上流 `docs-site` と同じ `--root` / `--out` の 2 つのみ。未知の引数は黙って
//!   無視せず usage を出して終了コード 2 にする。
//! - 上流 API（`build_site_with` / `EMPTY_REGISTRY`）は FF_REV 固定下でのみ有効。FF_REV を
//!   更新したらコンパイルエラーで API 変更を検知できる（実行時に壊れるより望ましい）。

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use fandhe_frontend_docs_site::build::build_site_with;
use fandhe_frontend_docs_site::page_sections::EMPTY_REGISTRY;

const USAGE: &str = "usage: docs-site-gen --root <dir> --out <dir>\n\n  --root <dir>  repository root containing site/nav.toml\n  --out <dir>   output directory (created by the generator)";

/// `--root` / `--out` を取り出す。どちらかが欠ける・重複する・未知の引数がある場合は `Err`。
fn parse_args<I: Iterator<Item = String>>(mut args: I) -> Result<(PathBuf, PathBuf), String> {
    let mut root: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        let slot = match arg.as_str() {
            "--root" => &mut root,
            "--out" => &mut out,
            other => return Err(format!("unknown argument: {other}\n\n{USAGE}")),
        };
        let value = args
            .next()
            .ok_or_else(|| format!("{arg} requires a value\n\n{USAGE}"))?;
        if slot.is_some() {
            return Err(format!("{arg} specified more than once\n\n{USAGE}"));
        }
        *slot = Some(PathBuf::from(value));
    }
    match (root, out) {
        (Some(r), Some(o)) => Ok((r, o)),
        _ => Err(format!("--root and --out are both required\n\n{USAGE}")),
    }
}

fn main() -> ExitCode {
    let (root, out) = match parse_args(std::env::args().skip(1)) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    match build_site_with(Path::new(&root), Path::new(&out), &EMPTY_REGISTRY) {
        Ok(report) => {
            println!(
                "ok pages={} redirects={} assets={}",
                report.written.len(),
                report.redirects.len(),
                report.assets.len()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}
