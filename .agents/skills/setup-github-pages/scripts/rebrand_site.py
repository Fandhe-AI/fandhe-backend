#!/usr/bin/env python3
"""生成済み docs サイト（dist）のブランド表示を brand.toml の値へ置換する後処理。

# 役割・境界

docs-site-gen（fandhe-frontend の docs-site を呼ぶ wrapper）が出力した dist を入力とし、
上流にハードコードされた fandhe-frontend 固有の表示（ヘッダーのブランド・GitHub リンク・
バージョン badge・フッターのタグライン / crates.io リンク / 著作権・favicon・lang）を
対象リポジトリの値へ差し替える。`build-local.sh`（ローカルと CI の共通入口）から呼ばれる。
上流の `[site]` が `title` / `base_path` 以外のキーを受理するようになったら本スクリプトごと
不要になる（setup-github-pages スキルの references/maintenance.md「上流改修の追跡」）。

# 置換の方針（なぜ一括置換ではないか）

- 置換はページの `<header class="docs-header">` と `<footer class="docs-footer">` の領域内だけで
  行い、`<main>` 内のユーザー本文には触れない。
- 各置換ルールは期待出現数（chrome を持つ全 HTML で 1 件）を持ち、0 件・複数件ならその場で
  非 0 終了する。上流 rev を更新して DOM 構造が変わった場合に「黙って置換漏れ」になるのを防ぐ
  （fail-closed）。全ファイルの検証を終えるまでディスクへ書き込まない。
- フッターの LICENSE-MIT / LICENSE-APACHE リンク（fandhe-frontend の URL を含む）は
  MIT / Apache-2.0 の帰属表記として保持する。直前の「Licensed under」は対象サイトの
  ライセンス宣言と誤読されるため「Built with fandhe-frontend docs-site (…)」へ書き換える
  （リンク要素のバイト列は不変）。
- 置換後に、帰属箇所以外へ `fandhe-frontend` が残っていないことを検査する（`--verify-only`
  でも単独実行できる。CI とローカルが同じコードを通る）。

# セキュリティ

brand.toml の値は `html.escape` を通してから HTML へ入れる。リポジトリ URL は
`https://github.com/<owner>/<repo>` 形式のみ許可（`_common.load_brand` で検証）。
生成物の CSP（script-src 'self' 等）を保つため、インライン script / style は追加しない
（favicon は presentation 属性のみの SVG）。

終了コード: 0 成功 / 1 置換・検証の失敗 / 2 引数・入力の不正。
"""

from __future__ import annotations

import argparse
import html
import os
import re
import sys
from pathlib import Path
from typing import Callable

# `-I`（隔離モード）では起動スクリプトのディレクトリが sys.path に入らないため、自分で足す。append にして、
# 同じディレクトリに標準モジュール名のファイル（argparse.py 等）があっても標準ライブラリを先に解決させる。
sys.path.append(str(Path(__file__).resolve().parent))
from _common import RESIDUAL_RE, write_target_problem, UPSTREAM_BRAND, Brand, BrandError, load_brand  # noqa: E402

UPSTREAM_REPO_URL = "https://github.com/Fandhe-AI/fandhe-frontend"
ATTRIBUTION_TEXT = "Built with fandhe-frontend docs-site"

# 帰属表記として残すリンク（href が上流の LICENSE 2 件に完全一致する <a> 開始タグ）。
_LICENSE_ANCHOR_RE = re.compile(
    r'<a [^>]*href="' + re.escape(UPSTREAM_REPO_URL) + r'/blob/main/LICENSE-(?:MIT|APACHE)"[^>]*>'
)
_ARTICLE_RE = re.compile(r'<article class="docs-content">.*?</article>', re.S)
_HEADER_RE = re.compile(r'<header class="docs-header">.*?</header>', re.S)
_FOOTER_RE = re.compile(r'<footer class="docs-footer">.*?</footer>', re.S)

# 検索インデックスはユーザー本文由来のため、本文中の言及を残存検査の対象外にする。
_RESIDUAL_SKIP_PREFIXES = ("assets/search-index",)


def esc(value: str) -> str:
    return html.escape(value, quote=True)


def favicon_svg(brand: Brand) -> str:
    """ヘッダーのインライン SVG と assets/favicon.svg の共通実体。

    `<text>` は SVG ファイル単体・HTML インラインのどちらでも CSP に抵触しない
    （スクリプトもスタイルシートも使わない）。色は brand.toml で検証済みの #RRGGBB のみ。
    """
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" role="img" '
        f'aria-label="{esc(brand.brand)}">'
        f'<rect width="32" height="32" rx="7" fill="{brand.favicon_color}"></rect>'
        '<text x="16" y="23" text-anchor="middle" font-family="system-ui, sans-serif" '
        f'font-size="20" font-weight="700" fill="#ffffff">{esc(brand.favicon_letter)}</text></svg>'
    )


Repl = Callable[[re.Match[str]], str]


def _rules(brand: Brand) -> tuple[list, list]:
    """(header_rules, footer_rules)。要素は (名前, 正規表現, 置換関数)。期待出現数は常に 1。"""
    repo = brand.repository

    header = [
        (
            "ヘッダー: ブランドマーク SVG",
            re.compile(
                r'(<span class="docs-brand-mark" aria-hidden="true">)'
                r'<svg\b[^>]*aria-label="' + UPSTREAM_BRAND + r'"[^>]*>.*?</svg>(</span>)',
                re.S,
            ),
            lambda m: m.group(1) + favicon_svg(brand) + m.group(2),
        ),
        (
            "ヘッダー: ブランド名",
            re.compile(
                r'(<a href="[^"]*" class="docs-brand">.*?</span>)' + UPSTREAM_BRAND + r"(</a>)",
                re.S,
            ),
            lambda m: m.group(1) + esc(brand.brand) + m.group(2),
        ),
        (
            "ヘッダー: バージョン badge",
            re.compile(r'(<span class="docs-brand-version"><span [^>]*>)[^<]*(</span></span>)'),
            (lambda m: m.group(1) + esc(brand.version_badge) + m.group(2))
            if brand.version_badge
            else (lambda m: ""),
        ),
        (
            "ヘッダー: GitHub リンク",
            re.compile(
                r'(<span class="docs-github-link"><a [^>]*?href=")'
                + re.escape(UPSTREAM_REPO_URL)
                + r'(")'
            ),
            lambda m: m.group(1) + repo + m.group(2),
        ),
    ]
    footer = [
        (
            "フッター: タグライン",
            re.compile(r'(<p class="docs-footer-tagline">)[^<]*(</p>)'),
            (lambda m: m.group(1) + esc(brand.tagline) + m.group(2))
            if brand.tagline
            else (lambda m: ""),
        ),
        (
            "フッター: GitHub リンク",
            # href が上流リポジトリ URL に完全一致するものだけ（LICENSE へのリンクは /blob/... が続くため対象外）。
            re.compile(r'(<a [^>]*?href=")' + re.escape(UPSTREAM_REPO_URL) + r'(")'),
            lambda m: m.group(1) + repo + m.group(2),
        ),
        (
            "フッター: crates.io リンク（削除）",
            re.compile(
                r'<li><a [^>]*href="https://crates\.io/crates/' + UPSTREAM_BRAND + r'-core"[^>]*>crates\.io</a></li>'
            ),
            lambda m: "",
        ),
        (
            "フッター: 著作権表記",
            re.compile(r"(<p [^>]*>)© \d{4} Fandhe-AI / " + UPSTREAM_BRAND + r" contributors(</p>)"),
            lambda m: m.group(1) + esc(brand.copyright) + m.group(2),
        ),
        (
            "フッター: ライセンス行 → 帰属表記",
            re.compile(
                r"(<p [^>]*>)Licensed under "
                r'(<a [^>]*href="' + re.escape(UPSTREAM_REPO_URL) + r'/blob/main/LICENSE-MIT"[^>]*>MIT</a> OR '
                r'<a [^>]*href="' + re.escape(UPSTREAM_REPO_URL) + r'/blob/main/LICENSE-APACHE"[^>]*>Apache-2\.0</a>)'
                r"(</p>)"
            ),
            lambda m: m.group(1) + ATTRIBUTION_TEXT + " (" + m.group(2) + ")" + m.group(3),
        ),
    ]
    return header, footer


def _apply(region: str, rules: list, where: str, problems: list[str]) -> str:
    for name, pattern, repl in rules:
        found = len(pattern.findall(region))
        if found != 1:
            problems.append(f"{where}: 「{name}」の一致数が {found}（期待 1）")
            continue
        region = pattern.sub(repl, region, count=1)
    return region


def rebrand_html(text: str, brand: Brand, where: str, problems: list[str]) -> str:
    """chrome を持つ HTML を置換して返す。redirect ページ（chrome なし）はそのまま返す。"""
    if '<header class="docs-header">' not in text:
        # redirect.rs の生成物は chrome を持たず meta refresh のみ。それ以外の chrome なしページは
        # 上流の構造変更のサインなので失敗にする。
        if 'http-equiv="refresh"' in text:
            return text
        problems.append(f"{where}: docs-header も meta refresh も無い未知のページ構造")
        return text

    header_rules, footer_rules = _rules(brand)
    for label, region_re, rules in (
        ("header", _HEADER_RE, header_rules),
        ("footer", _FOOTER_RE, footer_rules),
    ):
        regions = region_re.findall(text)
        if len(regions) != 1:
            problems.append(f"{where}: <{label}> 領域が {len(regions)} 個（期待 1）")
            continue
        new_region = _apply(regions[0], rules, f"{where} {label}", problems)
        text = text.replace(regions[0], new_region, 1)

    lang_re = re.compile(r'<html lang="[^"]*">')
    if len(lang_re.findall(text)) != 1:
        problems.append(f"{where}: <html lang> の一致数が {len(lang_re.findall(text))}（期待 1）")
    else:
        text = lang_re.sub(lambda m: f'<html lang="{brand.lang}">', text, count=1)
    return text


def residual_hits(rel: str, text: str) -> int:
    """帰属表記・本文を除いた上流表示の残存件数。

    単純な部分一致ではなく `_common.RESIDUAL_RE`（上流名の独立した語・上流 URL・上流 crates.io URL）で
    数える。利用者のリポジトリ名が `fandhe-frontend-docs` でも、base_path や自サイトの GitHub URL を
    誤検出しない。"""
    if rel.startswith(_RESIDUAL_SKIP_PREFIXES):
        return 0
    if rel.endswith(".html"):
        text = _ARTICLE_RE.sub("", text)
    text = _LICENSE_ANCHOR_RE.sub("", text)
    text = text.replace(ATTRIBUTION_TEXT, "")
    return len(RESIDUAL_RE.findall(text))


def missing_attribution(text: str) -> bool:
    """chrome を持つページで帰属表記（文言 + LICENSE 2 リンク）が揃っていなければ True。"""
    if '<header class="docs-header">' not in text:
        return False
    return (
        ATTRIBUTION_TEXT not in text
        or len(_LICENSE_ANCHOR_RE.findall(text)) != 2
    )


def find_symlinks(dist: Path) -> list[str]:
    """dist 配下の symlink（ファイル・ディレクトリ両方）の相対パス一覧。

    生成器は symlink を出力しない。存在する場合は、読み書きがリンク先（dist の外）へ及ぶ経路になるため、
    辿らずに失敗として扱う。os.walk は followlinks=False（既定）で、symlink ディレクトリは
    dirnames に現れるが降下しない。
    """
    found: list[str] = []
    for cur, dirs, names in os.walk(dist, followlinks=False):
        for n in dirs + names:
            p = Path(cur) / n
            if p.is_symlink():
                found.append(p.relative_to(dist).as_posix())
    return sorted(found)


def _collect(dist: Path) -> dict[str, str]:
    files: dict[str, str] = {}
    for cur, dirs, names in os.walk(dist, followlinks=False):
        dirs.sort()
        for n in sorted(names):
            p = Path(cur) / n
            if p.is_symlink() or not p.is_file():
                continue
            try:
                files[p.relative_to(dist).as_posix()] = p.read_text(encoding="utf-8")
            except UnicodeDecodeError:
                continue  # 画像等のバイナリは対象外
    return files


def verify(files: dict[str, str]) -> list[str]:
    problems: list[str] = []
    for rel, text in files.items():
        n = residual_hits(rel, text)
        if n:
            problems.append(f"{rel}: 帰属表記以外に `{UPSTREAM_BRAND}` が {n} 件残っている")
        if rel.endswith(".html") and missing_attribution(text):
            problems.append(f"{rel}: 帰属表記（Built with … + LICENSE-MIT/APACHE リンク）が欠けている")
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--dist", required=True, type=Path, help="docs-site-gen の出力ディレクトリ")
    ap.add_argument("--brand", required=True, type=Path, help="brand.toml のパス")
    ap.add_argument("--verify-only", action="store_true", help="置換せず残存・帰属の検査のみ行う")
    args = ap.parse_args(argv)

    if not args.dist.is_dir():
        print(f"エラー: --dist がディレクトリではない: {args.dist}", file=sys.stderr)
        return 2
    links = find_symlinks(args.dist)
    if links:
        print("エラー: dist 内に symlink がある（リンク先へ読み書きが及ぶため中止）: "
              + ", ".join(links[:10]), file=sys.stderr)
        return 1
    files = _collect(args.dist)
    if not any(r.endswith(".html") for r in files):
        print("エラー: dist に HTML が 1 件も無い（生成失敗の可能性）", file=sys.stderr)
        return 1

    if args.verify_only:
        problems = verify(files)
        for p in problems:
            print(f"NG {p}", file=sys.stderr)
        if problems:
            return 1
        print(f"verify ok: {len(files)} ファイルを検査（残存 0・帰属表記あり）")
        return 0

    try:
        brand = load_brand(args.brand)
    except BrandError as e:
        print(f"エラー: {e}", file=sys.stderr)
        return 2

    problems: list[str] = []
    out: dict[str, str] = {}
    html_count = 0
    for rel, text in files.items():
        if rel.endswith(".html"):
            html_count += 1
            out[rel] = rebrand_html(text, brand, rel, problems)
        elif rel == "assets/favicon.svg":
            if text.count(f'aria-label="{UPSTREAM_BRAND}"') != 1:
                problems.append("assets/favicon.svg: 上流 favicon の識別子（aria-label）が 1 件でない")
            else:
                out[rel] = favicon_svg(brand)
        else:
            out[rel] = text
    if "assets/favicon.svg" not in files:
        problems.append("assets/favicon.svg が dist に無い")

    problems += verify(out)
    if problems:
        for p in problems:
            print(f"NG {p}", file=sys.stderr)
        print("置換を中止した（dist は変更していない）。上流のデザイン更新で HTML の構造が変わり、置換対象が合わなくなった"
              "可能性が高い。スキル側の修正が必要（保守者は setup-github-pages スキルの references/maintenance.md"
              "「FF_REV の更新手順」で置換対象を再確認する）。利用者は更新を取り消し、スキルの修正を待つ。", file=sys.stderr)
        return 1

    dist_real = Path(os.path.realpath(args.dist))
    changed = 0
    for rel, text in out.items():
        if text != files[rel]:
            target = args.dist / rel
            # 書き込み先は dist の実体配下の通常ファイルに限る（find_symlinks 後の競合に備えた最終確認）
            why = write_target_problem(dist_real, target)
            if why:
                print(f"エラー: {rel} の書き込み先が不適（{why}）。中止", file=sys.stderr)
                return 1
            target.write_text(text, encoding="utf-8")
            changed += 1
    print(f"rebrand ok: HTML {html_count} 件を検査、{changed} ファイルを更新（残存 0・帰属表記あり）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
