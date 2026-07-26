# docs-site 刷新設計

親トラッキング #384（GitHub Pages ドキュメントサイト刷新）Phase 1 の先頭タスク
（イシュー #388）として、後続 11 イシュー（#389〜#399）が受け入れ基準の根拠として
参照する設計判断を確定する。fandhe-frontend で確立済みの docs-site 設計正典 3 本
（`docs-site-three-column-redesign.md` / `docs-site-search-design.md` /
`docs-site-api-reference-split.md`。いずれも `Fandhe-AI/fandhe-frontend` リポジトリの
`docs/design/` 配下）を、本リポジトリ（バックエンドフレームワーク、移植版 SSG、
`publish = false`）の文脈へ翻訳する。

## 1. 背景・目的

- `crates/docs-site` は fandhe-frontend の docs-site SSG（`fandhe-frontend-core` /
  `fandhe-frontend-app` / `fandhe-frontend-server` 0.1.0 のみに依存する静的サイト生成器）を
  移植したもので、現行は「Linear Developers 風 2 カラム」骨格に留まっている。
- fandhe-frontend 側では 3 カラムレイアウト・依存ゼロ全文検索・利用者向け API と内部
  設計記録の分離という 3 つの刷新を既に完了させており、その設計判断・DOM/class 契約・
  セキュリティ不変条件は fandhe-backend にそのまま輸入できる部分と、リポジトリの性質
  （バックエンドフレームワーク、ページ規模 13、`docs/design/` が既に内部設計記録置き場
  として確立済み）に応じて調整すべき部分がある。
- 本文書はその翻訳・調整の結果を確定し、#389〜#399 の各イシューが実装に着手できる
  粒度の設計判断を 1 箇所に集約する。

### 実測値（本文書作成時点、`docs/388-docs-site-redesign` ブランチ、`origin/main` 追随時点）

- nav 登録ページ数: **13**（Getting Started 2 / Guides 6 / API Reference 5、`site/nav.toml`）
- `base_path`: `/fandhe-backend`
- `site/assets/site.css`: **420 行**。現行 class 一覧: `.docs-header` / `.docs-container` /
  `.docs-sidebar` / `.docs-main` / `.docs-content` / `.docs-toc` /
  `.docs-toc-level-2` / `.docs-toc-level-3` / `.sidebar` / `.nav-section` / `.current` /
  `.prev-next` / `.prev` / `.next` / `.language-*`（コードブロック言語クラス）
- `crates/docs-site/src/layout.rs`: **357 行**。現行 DOM 骨格は 2 カラム
  （`header.docs-header` + `div.docs-container`（`aside.docs-sidebar` +
  `main.docs-main > article.docs-content`））。`with_heading_anchors` / `TocEntry` による
  `h2`/`h3` アンカー注入・TOC 収集は実装済みで、右カラム TOC への差し込み先として
  流用できる状態にある
- 既存テスト 5 本: `layout_render.rs`（DOM 構造）・`markdown_render.rs`（Markdown → HTML
  変換契約）・`site_build.rs`（`build_site()` の書き出し・fail-closed linkcheck）・
  `site_css_contract.rs`（layout.rs が要求する class と site.css の存在整合）・
  `site_nav.rs`（`nav.toml` パース・ページ数）
- `docs/guide/**` 内の issue/TASK 番号出現: `feature-samples.md`（1 箇所）・
  `graceful-shutdown.md`（2 箇所）・`streaming.md`（1 箇所）。`docs/api/**` は 0 箇所
  （現時点で公開範囲規約違反は検出されず、後続 #395 は主に将来混入の防止規約制定が主眼）
- `.github/workflows/docs-site.yml` の `paths` トリガー: `docs/guide/**` / `site/**` /
  `crates/docs-site/**` / 自己参照のみ。**nav 登録済みの `docs/api/**` が含まれていない
  ギャップを検出**（9 節・#398 への作業指示として記録）
- `examples/`（`with-cors` / `with-graphql` / `with-websocket`）・`templates/app/` は各々
  `README.md` を保持済み。docs サイト側の Examples セクションはこれらの流用・再構成を
  前提にできる

## 2. 制約・前提

- **外部依存を追加しない**: レンダラは crates.io の `fandhe-frontend-core` /
  `fandhe-frontend-app` / `fandhe-frontend-server` 0.1.0 のみを使う。検索・テーマ
  トグル等の対話的機能は素の JavaScript で実装し、Lunr.js / Fuse.js 等の検索ライブラリ・
  CSS フレームワーク・CDN 参照は一切採用しない（`crates/docs-site` は `publish = false`
  で本体バイナリ・依存ツリーに影響しないが、[[pay-for-what-you-use]] の思想を docs
  ビルドにも準用する）。
- **fail-closed 原則の維持**: `linkcheck.rs` のリンク切れ検出時ゼロ書き出し・`nav.rs` の
  TOML サブセット検証・`site_build.rs` / `site_css_contract.rs` の契約テストはいずれも
  刷新後も維持し、後退させない。
- `base_path = /fandhe-backend` は変更しない。
- **frontend との差分制約**: フロントエンドの `docs_page_with_assets` 相当は backend 版
  `layout.rs` として既に移植済みであり、frontend 側の後続改修は自動追随しない。本文書が
  backend 版レイアウト・検索・公開範囲規約の正典となる（frontend 文書は参照専用）。

## 3. 3 カラムレイアウト設計（→ #389）

### 3.1 DOM/class 契約

現行 2 カラム骨格を 3 カラムへ拡張する。`docs-*` プレフィックス命名を維持し、既存
class は破壊的変更を避けるため据え置いたまま新規 class を追加する。

```
header.docs-header
  div.docs-header-inner
    div.docs-brand          (サイトタイトルへのリンク)
    div.docs-header-actions (テーマトグル・GitHub リンク。4 節)
div.docs-container            (3 列 grid: サイドバー / 本文 / 右 TOC)
  aside.docs-sidebar
    nav.nav-section (既存、複数)
  main.docs-main
    article.docs-content      (既存、Markdown 変換結果)
    div.prev-next              (既存)
  aside.docs-toc-aside
    nav.docs-toc               (既存 TocEntry 出力をここへ移設)
```

新旧 class 対応表:

| 旧 | 新 | 備考 |
|---|---|---|
| `.docs-header`（プレーンヘッダー） | `.docs-header` + `.docs-header-inner` + `.docs-brand` + `.docs-header-actions` | 内部構造追加、class 名は維持 |
| `.docs-container`（2 列 grid） | `.docs-container`（3 列 grid） | grid-template-columns の列数変更のみ |
| `.docs-toc`（`docs-main` 内に同居） | `.docs-toc`（`aside.docs-toc-aside` 直下へ移設） | 収集ロジック（`with_heading_anchors`）は無変更、差し込み先のみ変更 |

### 3.2 breakpoint 設計

| 幅 | カラム数 | 挙動 |
|---|---|---|
| `< 768px` | 1 カラム | サイドバー・右 TOC は折りたたみ（チェックボックスハック or `:focus-within` による CSS のみのトグル、JS 依存なし） |
| `768px 〜 1199px` | 2 カラム | サイドバー + 本文。右 TOC は非表示 |
| `≥ 1200px` | 3 カラム | サイドバー + 本文 + 右 TOC |

いずれの幅でも横スクロールを発生させない（`max-width: 100%` の徹底、コードブロックの
`overflow-x: auto` は既存維持）。

### 3.3 右 TOC・簡素化判断

右カラム TOC は既存 `with_heading_anchors` / `TocEntry` をそのまま流用する（差し込み先
を `docs-main` 直下から `docs-toc-aside` へ移すのみで、収集ロジックの変更は不要）。

frontend のヘッダードロップダウン（複数プロダクト間のナビゲーション切替）は、
fandhe-backend の nav 登録ページ数が 13 と小規模であるため採用しない。ヘッダーには
セクションへの直リンク（Getting Started / Guides / Examples / API Reference の 4 入口）
のみを置く（安全側・最小差分の判断）。

## 4. ダークモード・ヘッダー actions 設計（→ #390）

- CSS カスタムプロパティ `--docs-*`（例: `--docs-bg` / `--docs-fg` / `--docs-border` /
  `--docs-accent`）でライト/ダーク 2 テーマのトークンを定義し、`:root` と
  `:root[data-theme="dark"]` / `@media (prefers-color-scheme: dark)` の両方に対応させる
  （フレームワーク本体で採用している artifact 相当のテーマ戦略に揃える）。
- テーマトグルは素の JS（`docs-site` が生成する最小インラインスクリプト）で
  `localStorage` キー `fandhe-backend-docs-theme` に `"light"` / `"dark"` を永続化する。
  JS 無効時は `prefers-color-scheme` の自動追随のみで、トグル UI 自体は
  `<noscript>` で非表示にする（フォールバック安全側）。
- FOUC 回避のため、`<head>` 内に `localStorage` 読み出し + `data-theme` 属性設定のみを
  行う最小インラインスクリプトを 1 つ許可する（JS 実装制約・埋め込み機構は 4.1 節を参照）。
- ヘッダー actions にはテーマトグルと GitHub リポジトリへの外部リンク
  （`https://github.com/Fandhe-AI/fandhe-backend`）を配置する。

### 4.1 JS ソースの埋め込み機構（policy 上の扱い）

`crates/docs-site/src/layout.rs` は現行、docs サイトが静的文書のみで JS ハイド
レーションを行わないことを理由に `raw_html()`（`fandhe_frontend_core` が提供する
唯一の非エスケープ出力経路）を使わない方針を明記している。`text()` はデフォルトで
`<` `>` `&` 等をエスケープするため、JS ソースをそのまま `<script>` タグの子要素として
出力するには使えない。

テーマトグル（本節）・検索 UI 遅延 fetch（8.4 節）の JS を導入するには、この
「`raw_html()` を使わない」方針を **`<script>` タグの内容に限定して緩和する**。
具体的には `el("script", vec![], vec![raw_html(SCRIPT_SOURCE)])` の形で、ビルド
バイナリに埋め込んだ固定のトラステッド文字列定数（利用者入力・Markdown 由来の
値を一切混入しない）のみを `raw_html()` に渡す。`fandhe-frontend-core` 側のテスト
規約（`ESCAPE-REVIEWED: raw_html オプトイン時の非エスケープ透過` 相当の理由コメント）
に倣い、この 2 箇所（テーマ初期化スクリプト・検索 UI スクリプト）以外に `raw_html()`
の利用を広げない不変条件を #390・#396 の実装レビューで確認する。既存 5 テストの
うち `layout_render.rs` にはこの限定利用（呼び出し箇所が 2 つのみであること）を
検証するケースを追加する。

## 5. アクセシビリティ設計（→ #391）

- **SkipNav**: `<body>` 内で最初にフォーカス可能な要素として
  `<a class="docs-skip-link" href="#docs-main-content">本文へスキップ</a>` を配置し、
  遷移先は `article.docs-content` 直前（`id="docs-main-content"`）とする
  （WCAG SC 2.4.1 Bypass Blocks 対応）。
- 現在ページの表示は `aria-current="page"` に一本化する（既存の `.current` class は
  スタイリング専用に残し、意味論は `aria-current` 属性が担う）。
- サイドバー `nav` 要素には不適切な `role` 上書きを行わない（ネイティブの `nav` /
  `ul` / `li` / `a` セマンティクスをそのまま使う）。

## 6. コンテンツ構成・情報設計（→ #392〜#394）

セクション構成を **Getting Started / Guides / Examples / API Reference** の 4 部構成へ
再編する。

- **Examples**: 索引ページ `site/examples.md`（4 サンプルの 1 行要約一覧）+
  `examples/with-cors` / `examples/with-graphql` / `examples/with-websocket` /
  `templates/app` の紹介ページ 4 本。原稿は各ディレクトリの既存 `README.md` を流用・
  再構成し、実コードへの導線は GitHub 上の該当ディレクトリへの外部リンクとする
  （サンプルコード自体を docs サイトへ複製しない。二重管理を避ける）。
- **Guides** / **API Reference**: 各セクション先頭に 1 行要約付きの索引ページを置く
  （既存 `docs/guide/README.md` はガイド索引として流用、API Reference には新規索引
  ページを追加）。
- トップページ（`site/index.md`）は 2 原則（pay-for-what-you-use / AI ファースト
  保守性）・feature プラグイン一覧・4 セクションへの入口案内で構成する。

## 7. 公開範囲規約（→ #395）

frontend の分離基準 M1〜M6 を backend 向けに翻訳する。「TASK-N.N」表記も M1（issue/PR
番号）相当の内部進行情報として扱う。

- **移設対象**（nav 登録ページから除外・`docs/design/` へ集約）: issue/PR 番号・
  Phase 表記・ロードマップ言及・スコープ外注記・実装経緯・TASK-N.N 表記を含む記述。
- **移設先の翻訳判断**: frontend は新規に `docs/internal/` を新設したが、fandhe-backend
  には内部設計記録置き場 `docs/design/` が CLAUDE.md・`docs/design/README.md` で既に
  確立済みのため、**新ディレクトリは作らず `docs/design/` を移設先とする**（安全側・
  既存構造の維持。本文書自身も `docs/design/` に置くことでこの規約と整合する）。
- **残す対象**（`docs/guide/*.md` / `docs/api/*.md` に留める）: 利用者が API を使うために
  必要な説明（シグネチャ・使用例・feature 前提条件）。issue/PR 番号を含めない。
- **検知方法**: `grep -nE '#[0-9]{2,4}|TASK-[0-9]'` を nav 登録ソース（`site/`・
  `docs/guide/`・`docs/api/`）に対して実行し、ヒットした記述を `docs/design/` へ移設する
  か、記述自体を汎化する。
- **リンク方向の非対称規則**: nav 登録ページから nav 未登録の `.md`
  （`docs/design/**` 等）への Markdown リンクは、`linkcheck.rs` が fail-closed で
  ビルドを拒否するため使用禁止とし、代わりにインラインコード表記
  （例: `` `docs/design/xxx.md` 参照 ``）によるポインタ 1 行のみ許可する。逆方向
  （`docs/design/` → nav 登録ページ）は通常の相対リンクを使ってよい。

## 8. 依存ゼロ全文検索設計（→ #396）

### 8.1 インデックススキーマ v1

`assets/search-index.json` として書き出す。キー順を固定した手書きシリアライザで
決定的出力を保証する（`serde_json` 等の依存追加はしない。既存 `crates/docs-site` の
Markdown/HTML 生成が手書き実装である方針と揃える）。

```json
{
  "version": 1,
  "base_path": "/fandhe-backend",
  "pages": [
    { "href": "/fandhe-backend/getting-started/", "title": "Getting Started",
      "sections": [{ "id": "...", "level": 2, "title": "..." }],
      "text": "（切り詰め済み本文プレーンテキスト）" }
  ]
}
```

- `href` は `base_path` 適用済みの絶対パス。
- `sections` は `TocEntry`（3 節）と 1:1 対応。
- `text` は本文から HTML タグを除去したプレーンテキスト。

### 8.2 サイズ上限（fail-closed）

- `MAX_PAGE_TEXT_BYTES = 4096`: 1 ページあたりの `text` を UTF-8 文字境界で決定的に
  切り詰める。
- `MAX_INDEX_BYTES = 1 MiB`: インデックス全体の上限。超過時は `build_site()` を非 0
  終了させる（linkcheck と同様の fail-closed）。
- 実測根拠: nav 登録 13 ページ × 4 KiB ≒ 52 KiB。1 MiB 上限に対し十分な余裕があり、
  当面のページ増加（数十ページ規模）でも上限に抵触しない。

### 8.3 セキュリティ・配線

- JSON エンコードは `"` `\` 制御文字の必須エスケープに加え、HTML への埋め込み事故を
  防ぐため `<` `>` `&` `U+2028` `U+2029` も追加エスケープする多層防御を行う。
- インデックスは独立ファイルとして `fetch` するのみで、HTML へインライン埋め込み
  しない（XSS 攻撃面の限定）。
- `build_site()` 内での配線順は、fallible なインデックス生成処理をアセット書き出し前に
  完了させる（linkcheck 同様、失敗時に部分書き込みを残さない）。現行の
  `crates/docs-site/src/build.rs`（`copy_assets`）は `site/assets/` 配下の通常ファイルを
  無条件・無予約名で `out_dir/assets/` へコピーするのみで、予約名リストに類する仕組みは
  存在しない。`search-index.json` は生成物であり `site/assets/` に事前配置するソース
  ファイルではないため、`copy_assets` とは別の書き出し経路（インデックス生成 →
  `out_dir/assets/search-index.json` への直接書き出し）を新設する。既存ファイル名との
  衝突を避けるため、`site/assets/search-index.json` というソースファイルを利用者が
  誤って配置した場合に検知して fail-closed するチェックを #396 で追加する。

### 8.4 検索 UI 契約

- DOM/class 契約: `input.docs-search-input` + `div.docs-search-results`（結果リストは
  `<ul>` + `<li><a>` で `textContent` のみを使い `innerHTML` を使わない）。
- マッチ・ランキングは単純な部分一致 + タイトル一致優先の決定的スコアリング
  （検索ライブラリを使わない制約と両立する最小実装）。
- インデックス `fetch` 失敗時・JS 無効時は検索 UI 自体を非表示にし、レイアウトを
  破壊しない（`<noscript>` で入力欄を隠す、fetch エラーはサイレントフォールバック）。

## 9. fail-closed 契約テスト・CI 追随（→ #397 / #398）

- `site_nav.rs`: nav 登録ページ数の期待値をハードコードし、ページ増減時に明示的な
  更新を強制する既存方式を維持する。
- `site_css_contract.rs`: 3 カラム化後の新規 class（`docs-header-inner` /
  `docs-brand` / `docs-header-actions` / `docs-toc-aside` / `docs-skip-link` /
  `docs-search-input` / `docs-search-results` 等）を契約対象に追加し、`layout.rs` が
  要求する class と `site.css` の実定義の乖離を検知する既存方式を維持する。
- `.github/workflows/docs-site.yml` の改修事項:
  - ビルド後存在検査（現行 `index.html` / `assets/site.css` の 2 点）に、刷新後の
    必須生成物 `assets/search-index.json` およびテーマ/検索用 JS アセットの存在検査を
    追加する。
  - **paths トリガーに `docs/api/**` を追加する**（現状ギャップ、1 節「実測値」参照）。
    `examples/**` の README を流用する場合は `paths` にも追加を検討する（実装時に
    実際の流用方式に応じて確定する）。
  - self-hosted / `timeout-minutes` / 最小 `permissions`（`contents: read`）は
    `.claude/rules/ci.md` に従い維持する。

## 10. セキュリティ不変条件（OWASP Top 10 観点）

- **A03 インジェクション / XSS**: 検索インデックスの JSON エンコードは `"` `\` の
  必須エスケープに加えて `<` `>` `&` `U+2028/2029` を追加エスケープする。インデックスは
  HTML に埋め込まず独立ファイル `fetch` のみとする。検索 UI の DOM 挿入は
  `textContent` 系のみで `innerHTML` を使わない。TOC タイトルへの生 HTML 非取り込み
  （既存 `with_heading_anchors` の防御）を維持する。
- **A05 セキュリティ設定の不備**: `docs-site.yml` は `contents: read` の最小権限・
  self-hosted・`timeout-minutes` を維持する。CDN・外部オリジンへの参照はゼロを維持し、
  サプライチェーン面の攻撃表面を増やさない。
- **A06 脆弱で古い構成要素**: 外部 JS/CSS ライブラリ・新規クレート依存の追加は行わない
  （`cargo tree` 差分なしを受け入れ条件として固定）。
- **A01 アクセス制御の不備 / 情報露出**: 7 節の公開範囲規約により issue/PR 番号・
  内部タスク記録を nav 登録ページから分離し、内部進行情報の公開サイトへの露出を防ぐ。
- **DoS 耐性**: 検索インデックスの二段上限（per-page 決定的切り詰め + 総量
  fail-closed）を維持し、無自覚な肥大化を CI で阻止する。

## 11. Phase 対応表

| イシュー | 対応節 | 概要 |
|---|---|---|
| #389 | 3 節 | 3 カラムレイアウト（DOM/class 契約・breakpoint） |
| #390 | 4 節 | ダークモード・ヘッダー actions |
| #391 | 5 節 | アクセシビリティ（SkipNav・aria-current） |
| #392 | 6 節 | コンテンツ構成（Getting Started/Guides 索引・トップページ） |
| #393 | 6 節 | Examples セクション新設 |
| #394 | 6 節 | API Reference 索引ページ |
| #395 | 7 節 | 公開範囲規約（`docs/design/` への集約・非対称リンク規則） |
| #396 | 8 節 | 依存ゼロ全文検索 |
| #397 | 9 節 | fail-closed 契約テストの作り替え |
| #398 | 9 節 | `docs-site.yml` CI 追随（paths トリガー欠落含む） |
| #399 | 全節 | 統合検証・Pages 実デプロイ確認 |

節番号は本文書内で安定させ、後続イシューからの参照（`docs-site-redesign.md §N`）を
壊さない。節を追加する場合は末尾に追記し、既存節番号を振り直さない。

## 12. 再評価トリガー

- nav 登録ページ数が現行 13 から大きく増加し（目安: 30 ページ超）、検索インデックス
  サイズが `MAX_INDEX_BYTES` に接近した場合は上限値の再検討を行う。
- ヘッダードロップダウン不採用の判断（3.3 節）は、将来 fandhe-frontend との複数
  プロダクト間ナビゲーション統合が要求された場合に再評価する。
- `docs/design/` を移設先とする判断（7 節）は、`docs/design/` 配下のページ数が
  nav 登録ページ数の設計記録として肥大化し可読性を損なう場合に、専用ディレクトリ
  分離を再検討する。

## 13. 関連文書

- fandhe-frontend 側の設計正典（参照専用、backend 側の後続改修を自動反映しない）:
  - [`docs-site-three-column-redesign.md`](https://github.com/Fandhe-AI/fandhe-frontend/blob/main/docs/design/docs-site-three-column-redesign.md)
  - [`docs-site-search-design.md`](https://github.com/Fandhe-AI/fandhe-frontend/blob/main/docs/design/docs-site-search-design.md)
  - [`docs-site-api-reference-split.md`](https://github.com/Fandhe-AI/fandhe-frontend/blob/main/docs/design/docs-site-api-reference-split.md)
- 本リポジトリ内: [`docs/design/README.md`](./README.md)（設計ドキュメント置き場の位置づけ）、
  親トラッキング issue #384、CLAUDE.md の Repository Structure（`crates/docs-site`
  節・`site/` 節）
