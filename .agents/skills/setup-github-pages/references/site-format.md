<!-- source: https://github.com/Fandhe-AI/fandhe-frontend/tree/cf5edb9b8f1bf2a63d51dcf50a8d2806e2d8f9f9/crates/docs-site/src (nav.rs, markdown.rs, highlight.rs, linkcheck.rs, redirect.rs, build.rs) -->
<!-- 最終確認日: 2026-10-04 -->
<!-- 取得状況: ✅ 取得済み（ソース精読 + 同 rev のビルド・生成での実測。redirects.toml は生成も実測済み） -->

# docs サイトのファイル書式（nav.toml / redirects.toml / Markdown）

生成器は fandhe-frontend の docs-site（`FF_REV` 固定）。ここに書く制約は**その rev の実装**に基づく。
`FF_REV` を更新したら本ファイルの各節を再確認する（references/maintenance.md「FF_REV の更新手順」）。

## nav.toml

`site/nav.toml` が全ページの一覧・順序・URL を決める。TOML の**厳密なサブセット**しか読めない。

| 使える | 使えない |
|--------|----------|
| `# コメント`、`[table]` / `[[array.table]]` ヘッダー、`key = "文字列"` | 整数・bool・配列・inline table・複数行文字列・シングルクォート |
| 文字列のエスケープは `\"` `\\` `\n` `\t` のみ | `\u` 等その他のエスケープ |
| 値の後ろの `# コメント` | — |

ファイルサイズ上限は 1 MiB。構文は `scripts/check_site.py` が同じ制約で事前検査する。

### テーブルとキー

| テーブル | キー | 制約 |
|----------|------|------|
| `[site]` | `title` | フッターのブランド名にのみ反映される |
| `[site]` | `base_path` | `""` か `/` 始まりで末尾 `/` なし。プロジェクトサイトは `/<リポジトリ名>` |
| `[[section]]` | `title`, `index_path` | `index_path` は配下の `page.path` と完全一致。ページが 0 件のセクションは不可 |
| `[[section.page]]` | `title`, `source`, `path` | `section` の直後に置く |
| `[[section.group]]` | `title` | 入れ子は 1 段のみ。直後に `[[section.group.page]]` を並べる |
| `[[section.group.page]]` | `title`, `source`, `path` | 直前の `[[section.group]]` に属する |
| `[[menu]]` | `title`, `index_path`, `source` | 複数セクションを束ねるヘッダーメニューと集約ページ。`index_path` はどの `page.path` とも衝突不可。`[[menu]]` の後に `[[section.*]]` を続けるには新しい `[[section]]` が必要 |
| `[[menu.item]]` | `section`, `description` | `section` は束ねる既存セクションの `index_path`、`description` は 1 行（空・改行不可）。メンバーは 1 件以上で、直前の `[[menu]]` または `[[menu.item]]` の直後にだけ置ける |

`title`（セクション・ページ・メニュー）に上流名 `fandhe-frontend` を独立した語として含められない（ヘッダー・サイドバー・フッターに出るため、rebrand 後の残存検査と区別できない。`fandhe-frontend-docs` のような別の語の一部は可。`check_site.py` が事前に拒否する）。

`path` は `/` 始まり・`/` 終わりで、セグメントは英数字・`-`・`_` のみ。サイト全体で一意。
`source` は相対パスで、`..`・絶対パス・`\` は禁止、ファイルが実在すること。リポジトリ内のどこにあってもよい
（`docs/guide/x.md` や `README.md` も指定できる。ただし `pages.yml` の利用者区間（`sgp:user-paths:begin` と `end` の間）に `      - "docs/**"` の形で監視パスも追加する。区間の外を編集すると、スキルの更新が競合する）。

### 予約パス（全面禁止）

`path` / `index_path` が次のいずれかで始まるページは作らない。registry を空にしても
fandhe-frontend のショーケース（部品ページ等）が混入することを実測済み。

`/themes/` `/primitives/` `/blocks/` `/wireframes/`

`check_site.py` が検出して失敗にする。

### 予約アセット名

`site/assets/` の直下に次の名前を置くと生成器がビルドエラーにする（生成物と衝突）。
`search-index/` ディレクトリも同様。静的ファイルは `site/assets/` へ置くと `assets/` にコピーされる。

```text
site.css  site-primitives.css  skip-nav.css  pre-styled-ui.css  primitives-showcase.css
admonition.css  site.js  theme-init.js  favicon.svg  search-index.json  image-demo.svg
blocks.css  wireframes.css  blocks-demo-product.svg  blocks-demo-avatar.svg
blocks-demo-logo.svg  blocks-demo-screenshot.svg  blocks-demo-background.svg
```

## redirects.toml（任意）

`site/redirects.toml` に旧 URL の案内ページを宣言する。同じ TOML サブセット。

```toml
[[redirect]]
from = "/old-usage/"
to = "/usage/"
```

`from` は既存の `page.path` と衝突不可、`to` は実在するページであること。生成物は
`meta refresh` + `rel=canonical` + `noindex` のみの最小ページ（ヘッダー・フッターを持たない）で、
`rebrand_site.py` はこれを redirect ページとして許容する。1 件の生成と rebrand 通過を実測済み。

## Markdown サブセット

| 使える | 備考 |
|--------|------|
| 見出し `#`〜 | `id` が自動付与される（ページ内リンク `#id` に使える） |
| 段落・リスト（入れ子可）・表・引用 | — |
| フェンスコード | シンタックスハイライトは `rust` / `toml` / `html` のみ。エイリアス（`rs` 等）は不可で、他言語は色なしの素のコード |
| インラインコード・`*強調*`・`**太字**` | `_強調_` は非対応 |
| リンク `[文字](url)` | スキームは http / https / 相対のみ。`mailto:` は不可 |
| admonition | 引用の 1 行目に単独で `> [!NOTE]` / `TIP` / `IMPORTANT` / `WARNING` / `CAUTION`（大文字） |

非対応（書くと意図と違う出力になる）:

- **画像** — `![alt](x.png)` は `!` とリンクとして描画される。図は表・コードブロック・リンクで代替する
- 生 HTML — タグはエスケープされ文字として表示される
- 自動リンク（`<https://…>`・裸の URL）・参照リンク（`[a][b]`）

## リンク検査（fail-closed）

生成時に内部リンクを全件検査し、**1 件でも壊れていれば何も書き出さず非 0 終了**する。

| 書き方 | 結果 |
|--------|------|
| `[x](./other.md)` | nav の `source` → `path` で公開 URL へ自動書き換え。nav に無い `.md` は失敗 |
| `[x](#anchor)` | 存在しない `#anchor` は失敗 |
| `[x](/mini-repo/usage/)` | 絶対パスは**`base_path` を含めて**書く。存在しないパスは失敗 |
| `[x](https://…)` | 外部 URL は検査しない（到達性は保証されない） |

## トップページ

registry を空にして呼ぶため、トップはヒーロー・カードグリッドを持たない通常の Docs レイアウトになる
（fandhe-frontend 本家のランディングは再現できない）。
