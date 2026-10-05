#!/usr/bin/env python3
"""docs サイトの生成前検証（nav.toml・brand.toml・site/assets・Markdown の事前チェック）。

# 役割・境界

`build-local.sh`（ローカルと CI 共通）の最初の工程として呼ばれ、生成器（fandhe-frontend
docs-site）に渡す前に「生成器は通すが公開物として壊れる / 他サイトのショーケースが混入する」
入力を止める。生成器自身の検査（リンク検査・nav スキーマ検査）と重複させず、生成器が
検知できない次の 5 点を担う。

1. 予約パス: nav の path が `/themes/` `/primitives/` `/blocks/` `/wireframes/` で始まると、
   registry を空にしていても fandhe-frontend のショーケースが混入する（実測）。全面禁止。
2. base_path と公開 URL の整合: GitHub Pages のプロジェクトサイトは `/<repo>/` 配下で配信される。
   nav.toml の base_path が brand.toml の repository から導出した値と一致しないと、全アセットと
   内部リンクが 404 になる。
3. 予約アセット名: `site/assets/` に生成物と同名のファイルがあると生成器がビルドエラーにする。
   エラー文が分かりにくいため事前に具体名で報告する。
4. 未置換プレースホルダー（`__SGP_*__`）の残存。
5. nav の title に上流名 `fandhe-frontend` が含まれる（rebrand 後の残存検査と衝突）。

警告のみ（終了コードに影響しない）: Markdown の画像記法（上流は画像非対応）、base_path を
含まない絶対パスリンク、THIRD-PARTY-LICENSES の欠落。

終了コード: 0 問題なし / 1 エラーあり / 2 入力不正（ファイル欠落・構文違反）。
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

# `-I`（隔離モード）では起動スクリプトのディレクトリが sys.path に入らないため、自分で足す。append にして、
# 同じディレクトリに標準モジュール名のファイル（argparse.py 等）があっても標準ライブラリを先に解決させる。
sys.path.append(str(Path(__file__).resolve().parent))
from _common import (  # noqa: E402
    PLACEHOLDER_RE, UPSTREAM_BRAND, BrandError, SubsetError, has_upstream_word, load_brand, parse_nav,
    read_bounded_text, resolves_inside, sanitize,
)

# 上流のショーケース生成パス。nav の path がここから始まると部品ページ等が混入する。
RESERVED_PATH_PREFIXES = ("/themes/", "/primitives/", "/blocks/", "/wireframes/")

# build.rs の RESERVED_ASSET_NAMES と同一（FF_REV 更新時に再確認する。SKILL.md 参照）。
RESERVED_ASSET_NAMES = {
    "site.css", "site-primitives.css", "skip-nav.css", "pre-styled-ui.css",
    "primitives-showcase.css", "admonition.css", "site.js", "theme-init.js",
    "favicon.svg", "search-index.json", "image-demo.svg", "blocks.css",
    "wireframes.css", "blocks-demo-product.svg", "blocks-demo-avatar.svg",
    "blocks-demo-logo.svg", "blocks-demo-screenshot.svg", "blocks-demo-background.svg",
}
RESERVED_ASSET_DIRS = {"search-index"}

# 入力（nav.toml・Markdown・brand.toml）は信頼しない。`[^\]]*` のような上限なしの繰り返しは、細工した入力で
# 最悪 O(n²) になるため長さを制限する。ファイル自体も読み取りサイズに上限を付ける。
_IMAGE_RE = re.compile(r"!\[[^\]\n]{0,300}\]\([^)\n]{0,500}\)")
_ABS_LINK_RE = re.compile(r"(?<!!)\[[^\]\n]{0,300}\]\((/[^)\s]{0,500})\)")
_INLINE_CODE_RE = re.compile(r"`[^`\n]{0,300}`")
NAV_MAX_BYTES = 1024 * 1024
BRAND_MAX_BYTES = 64 * 1024
MD_MAX_BYTES = 1024 * 1024
WORKFLOW_MAX_BYTES = 256 * 1024


def _strip_fences(text: str) -> str:
    """フェンスコードブロックを除く（行単位の単一走査。未閉じのフェンスは末尾まで除く）。"""
    out, fenced = [], False
    for line in text.split("\n"):
        if line.startswith("```"):
            fenced = not fenced
            continue
        if not fenced:
            out.append(line)
    return "\n".join(out)


def _read_in_root(root_real: Path, path: Path, cap: int) -> str:
    """root 内に解決されるファイルだけを、上限付きで読む。root 外（`.git` 配下を含む）へ解決されるものは読まない。"""
    rel = path.relative_to(root_real) if path.is_relative_to(root_real) else path
    if path.is_symlink():
        # root 内を指す symlink（`site/nav.toml -> ../.env` など）も辿らない。リンク先の内容の断片が
        # パースエラーとして出力に出るのを防ぐ。
        raise ValueError(f"{rel} がシンボリックリンクのため読まない（通常ファイルにする）")
    if not resolves_inside(root_real, path):
        raise ValueError(f"{rel} が対象リポジトリの外（または .git 配下）へ解決される。読まない")
    try:
        return read_bounded_text(path, cap)
    except (OSError, ValueError, UnicodeDecodeError) as e:
        raise ValueError(f"{rel} を読めない: {e if isinstance(e, ValueError) and not isinstance(e, UnicodeDecodeError) else type(e).__name__}") from e


def check(root: Path, brand_path: Path) -> tuple[list[str], list[str]]:
    errors: list[str] = []
    warnings: list[str] = []

    nav_path = root / "site" / "nav.toml"
    try:
        tables = parse_nav(_read_in_root(root, nav_path, NAV_MAX_BYTES))
    except (SubsetError, ValueError) as e:
        raise ValueError(f"site/nav.toml を読めない: {e}") from e
    if brand_path.is_symlink():
        raise ValueError("brand.toml がシンボリックリンクのため読まない（通常ファイルにする）")
    if not resolves_inside(root, brand_path):
        raise ValueError("brand.toml が対象リポジトリの外（または .git 配下）へ解決される。読まない")
    try:
        brand = load_brand(brand_path)
    except BrandError as e:
        raise ValueError(str(e)) from e

    # 1. base_path の整合
    site = [t for t in tables if t.header == "site"]
    base = site[0].values.get("base_path", "") if site else ""
    if base != brand.base_path:
        errors.append(
            f"site/nav.toml の base_path が `{base}`。repository（{brand.repository}）から導出した"
            f"公開パスは `{brand.base_path}`（User/Org サイトは空、プロジェクトサイトは /<repo>）"
        )

    # 2. 予約パス（page.path / section.index_path / menu.index_path）
    sources: list[str] = []
    for t in tables:
        for key in ("path", "index_path"):
            p = t.values.get(key)
            if p is not None and p.startswith(RESERVED_PATH_PREFIXES):
                errors.append(
                    f"site/nav.toml line {t.line}: {key} `{p}` は予約パス"
                    "（/themes/ /primitives/ /blocks/ /wireframes/ は上流のショーケースと衝突する）"
                )
        s = t.values.get("source")
        if s is not None:
            sources.append(s)
        title = t.values.get("title")
        if title is not None and has_upstream_word(title):
            # title はヘッダー・サイドバー・フッター・<title> に出る。rebrand 後の残存検査
            # （帰属表記以外に上流名が残らないこと）と衝突し、ビルド最終段で原因不明に失敗するため先に拒否する。
            errors.append(
                f"site/nav.toml line {t.line}: title `{title}` に上流名 `{UPSTREAM_BRAND}` を独立した語として含められない"
                "（生成後の残存検査と区別できない。`fandhe-frontend-docs` のような別の語の一部は可）"
            )

    # 3. 予約アセット名
    assets = root / "site" / "assets"
    if resolves_inside(root, assets) and assets.is_dir():   # 実体の検証が先（is_dir は親 symlink を辿って外を見る）
        for child in sorted(assets.iterdir()):
            if child.is_dir() and child.name in RESERVED_ASSET_DIRS:
                errors.append(f"site/assets/{child.name}/ は予約ディレクトリ（生成物と衝突）")
            elif child.name in RESERVED_ASSET_NAMES:
                errors.append(f"site/assets/{child.name} は予約アセット名（生成物と衝突しビルドエラーになる）")

    # 4. プレースホルダー残存（nav・brand・nav が参照する Markdown・workflow）
    scan = [nav_path, brand_path, root / ".github" / "workflows" / "pages.yml"]
    scan += [root / s for s in sources if not Path(s).is_absolute() and ".." not in Path(s).parts]
    for f in scan:
        # 実体の検証が先。root 外へ解決されるものは（存在の有無にかかわらず）読まずにエラーにする
        if resolves_inside(root, f) and not os.path.lexists(f):
            continue
        try:
            body = _read_in_root(root, f, WORKFLOW_MAX_BYTES if f.suffix == ".yml" else MD_MAX_BYTES)
        except ValueError as e:
            errors.append(str(e))   # root 外へ解決される・大きすぎる・読めない: 読まずにエラー
            continue
        found = sorted(set(PLACEHOLDER_RE.findall(body)))
        if found:
            errors.append(f"{f.relative_to(root)} に未置換のプレースホルダー: {', '.join(found)}")

    # 警告: Markdown（コードフェンス内は除外）
    for s in sources:
        f = root / s
        if Path(s).is_absolute() or ".." in Path(s).parts or not resolves_inside(root, f) or not os.path.lexists(f):
            continue  # 生成器が拒否・報告する（root 外へ解決されるものは、上のプレースホルダー検査でエラーにしている）
        try:
            body = _INLINE_CODE_RE.sub("", _strip_fences(_read_in_root(root, f, MD_MAX_BYTES)))
        except ValueError:
            continue  # root 外へ解決される・大きすぎる・読めない: 上のプレースホルダー検査で既にエラーにしている
        if _IMAGE_RE.search(body):
            warnings.append(f"{s}: 画像記法 ![](…) は上流が非対応（`!` + リンクとして描画される）")
        for link in _ABS_LINK_RE.findall(body):
            if base and not (link == base or link.startswith(base + "/")):
                warnings.append(f"{s}: 絶対パスリンク `{link}` が base_path `{base}` を含まない（リンク検査で失敗する）")

    tpl = root / "THIRD-PARTY-LICENSES"
    if not (resolves_inside(root, tpl) and tpl.is_file()):
        warnings.append("THIRD-PARTY-LICENSES が無い（build-local.sh --write-third-party で生成する）")
    return errors, warnings


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", type=Path, default=Path("."), help="対象リポジトリのルート")
    ap.add_argument("--brand", type=Path, default=None,
                    help="brand.toml のパス（既定 <root>/tools/docs-site-gen/brand.toml）")
    args = ap.parse_args(argv)
    root = args.root.resolve()
    brand_path = args.brand or root / "tools" / "docs-site-gen" / "brand.toml"
    try:
        errors, warnings = check(root, brand_path)
    except ValueError as e:
        print(f"エラー: {sanitize(e, 500)}", file=sys.stderr)
        return 2
    for w in warnings:
        print(f"警告 {sanitize(w, 500)}", file=sys.stderr)
    for e in errors:
        print(f"NG {sanitize(e, 500)}", file=sys.stderr)
    if errors:
        return 1
    print("check_site ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
