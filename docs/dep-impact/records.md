# 依存インパクト記録台帳

`docs/dep-impact/README.md` の運用手順に従い、`crates/plugin-*` 追加・変更時の依存
インパクト計測結果を追記する。エントリは新しい順に追加する。

> 注記: 2026-07 の crate・import 一括改名（#202、`fandhe-backend` 体系への改名）以前の
> エントリは旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` /
> `bf-plugin-*` 等）表記のまま保持している。実測値本文は改変せず、履歴記録として残す
> （`docs/design/framework-naming.md` 7 節の推奨方針）。

## 2026-08-11 — `crates/http` に `stats_alloc`（dev-dependency）を追加（`tests/alloc_count.rs` の unsafe 除去、PR #602 レビュー指摘 P0 対応、イシュー #591）

`crates/http/tests/alloc_count.rs`（`parse_request_head` の 1 リクエストあたり
alloc 回数が N 非依存の定数であることを固定する常設テスト）が自前で実装していた
`unsafe impl GlobalAlloc`（workspace lint `unsafe_code = "warn"` を `#![allow(unsafe_code)]`
で緩めていた、`scripts/unsafe-baseline.json` の `http` を 0→8/0→1 へ増やす原因だった）を、
`GlobalAlloc` を実装済みの計測専用 crate `stats_alloc`（外部依存ゼロ・`unsafe` 実装は
crate 内部に閉じる）へ置き換えた。本クレート自体には `unsafe` を一切導入せず、
`scripts/unsafe-triage.sh --update-baseline` で `http` のベースラインを 0/0 へ戻した。

### 依存情報（pay-for-what-you-use）

`[dev-dependencies]`（テスト専用、release バイナリ・`cargo tree -e normal` には
現れない）に `stats_alloc = "0.1.10"` を追加した。

```
$ cargo tree -p fandhe-backend-http -e dev
fandhe-backend-http v0.3.0
[dev-dependencies]
├── stats_alloc v0.1.10
└── tokio v1.53.1
    ...
```

`stats_alloc` 自体の推移依存は 0 件。`-e normal` フィルタでの依存グラフ
（`memchr` + `tokio` の 2 件、下記エントリ参照）に変化はなく、通常依存
（release ビルド対象）への影響はない。

## 2026-08-11 — `crates/http` に `memchr` を追加（`find_subslice` のヘッド終端探索を memmem ベースへ変更、イシュー #586）

リクエストヘッド終端（`\r\n\r\n`）・ヘッダ行区切り（`\r\n`）探索を担う `find_subslice`
（素朴な `windows().position()` の線形走査）を `memchr::memmem::find`（SIMD 最適化された
Two-Way 法）へ置き換えた。大ヘッダ・バッファ分割着弾時（`read_request` が Incomplete
のたびに先頭から再走査する）の探索コストを削減する狙い。シグネチャ
（`pub(crate) fn find_subslice(&[u8], &[u8]) -> Option<usize>`）・空 needle 時 `None`
を返す契約は不変。

### 依存情報（pay-for-what-you-use）

`crates/http` の外部 crates.io 依存に `memchr = { version = "2.8", default-features =
false }` を追加した（既存の `tokio` のみの構成に 1 件追加。「tokio が唯一の必須実行時
依存」というクレート不変条件は崩れ、Cargo.toml のコメントを「tokio + memchr の 2 件」へ
更新済み）。`memchr` は既に workspace 依存ツリーに存在する v2.8.3
（axum-ref / plugin-graphql / plugin-webrtc / plugin-websocket 経由）へ統一解決され、
バージョン解決上の新規エントリは増えない。

```
$ cargo tree -p fandhe-backend-http -e normal
fandhe-backend-http v0.3.0
├── memchr v2.8.3
└── tokio v1.53.1
    ├── bytes v1.12.1
    └── pin-project-lite v0.2.17
```

`memchr` 自体の推移依存は 0 件（`default-features = false` により no_std・alloc 不要な
`memmem::find` のみを使用。`tokio` は既存の `fandhe-backend-http` の必須依存であり本
変更による増分ではない）。

### unsafe 件数

`cargo geiger`（`crates/core` 起点、2026-08-11 実測）:

```
Functions  Expressions  Impls  Traits  Methods  Dependency

0/0        0/0          0/0    0/0     0/0      ?  fandhe-backend-core 0.3.0
0/0        0/0          0/0    0/0     0/0      ?  ├── fandhe-backend-http 0.3.0
34/48      1992/2440    2/2    0/0     110/148  !  │   ├── memchr 2.8.3
25/30      2154/3011    103/119 3/3     103/139  !  │   └── tokio 1.53.1
...(tokio 系、変更前から存在する既存依存)...
```

`fandhe-backend-http` 本体の unsafe は 0 件のまま（`scripts/unsafe-triage.sh` の
workspace ベースラインは不変、"baseline から変化なし" で確認済み）。`memchr` は内部に
SIMD intrinsics 由来の unsafe（34/48 関数・1992/2440 式）を持つが、これは既存
workspace 依存ツリーに既に存在していた依存側 unsafe であり、本変更が新規に持ち込む
ものではない。`cargo audit` / `cargo deny check`（`scripts/dep-audit.sh`）は全 feature
構成で PASS（advisories ok, bans ok, licenses ok, sources ok）。ライセンスは
`Unlicense OR MIT`（`memchr` の SPDX OR 式）で `deny.toml` の既存 allowlist（MIT 含む）
が充足するため `deny.toml` 変更は不要だった。

### トレードオフ

- `crates/http` の「tokio が唯一の必須実行時依存」という不変条件が崩れる
  （Cargo.toml のコメントで明記済み）
- 最小コア構成（`fandhe-backend-core` default）の依存クレート数が +0（既に workspace
  ツリーに存在していたバージョンへ統一解決されるため、`cargo tree` の union 件数
  （`scripts/dep-impact.sh` の「依存クレート数」表）には現れない）

### 検証コマンド

```
cargo tree -p fandhe-backend-http -e normal
cargo geiger -p fandhe-backend-core
bash scripts/dep-audit.sh
bash scripts/unsafe-triage.sh
```

## 2026-08-11 — `crates/routes` に `rustc-hash` を追加（静的ルート lookup 借用キー化 + FxHash 化、イシュー #583）

静的ルート照合の既定ハッシャ（SipHash 1-3）を FxHash（`rustc-hash` 2 系）へ差し替え、
`routes` フィールドを `HashMap<(String, String), _>` から `FxHashMap<Box<str>, FxHashMap<Box<str>, _>>`
（path → method のネスト map）へ変更した。これにより静的ルート照合が `&str` の借用キー
2 段照合となり、リクエストごとの `String` 確保（旧実装は `(method.clone(), path.to_string())`
で 2 個）が発生しなくなる。副次効果として 405 応答の `Allow` 集約が「全登録静的ルート
キーの線形走査」から「対象パスの inner map 参照」（`O(登録 method 数)`）へ縮小した。

### 依存情報（pay-for-what-you-use）

`crates/routes` の外部 crates.io 依存に `rustc-hash = "2"` を追加した（既存の
`fandhe-backend-http` のみの構成に 1 件追加）。`rustc-hash` は既定構成（`std` feature
のみ）で推移依存ゼロの純 Rust 実装。

```
$ cargo tree -p fandhe-backend-routes -e normal
fandhe-backend-routes v0.3.0
├── fandhe-backend-http v0.3.0
│   └── tokio v1.53.1
│       ├── bytes v1.12.1
│       └── pin-project-lite v0.2.17
└── rustc-hash v2.1.3
```

`rustc-hash` 自体の推移依存は 0 件（`tokio` は既存の `fandhe-backend-http` 経由の依存
であり本変更による増分ではない）。

### unsafe 件数

`cargo geiger -p fandhe-backend-routes`（2026-08-11 実測）:

```
Functions  Expressions  Impls  Traits  Methods  Dependency

0/0        0/0          0/0    0/0     0/0      ?  fandhe-backend-routes 0.3.0
0/0        0/0          0/0    0/0     0/0      ?  ├── fandhe-backend-http 0.3.0
...(tokio 系、変更前から存在する既存依存)...
0/0        0/0          0/0    0/0     0/0      ?  └── rustc-hash 2.1.3
```

`fandhe-backend-routes` 本体・`rustc-hash` ともに unsafe 0 件（本変更による unsafe 増分
ゼロ）。`cargo audit` / `cargo deny check`（`scripts/dep-audit.sh`）は全 feature 構成で
PASS（advisories ok, bans ok, licenses ok, sources ok）。

### 検証コマンド

```
cargo tree -p fandhe-backend-routes -e normal
cargo geiger -p fandhe-backend-routes
bash scripts/dep-audit.sh
```

## 2026-07-21 — docs-site 基盤追加（GitHub Pages ドキュメントサイト生成ツール）

`crates/docs-site`（`fandhe-backend-docs-site`、publish=false）を新設し、GitHub Pages
ドキュメントサイト基盤を構築した。fandhe-frontend の docs-site を移植した SSG ツールで、
`site/` の nav.toml・docs/guide/ から静的サイトを生成する。

### 依存情報（pay-for-what-you-use）

`crates/docs-site` の外部 crates.io 依存は以下の 3 件のみ:

- `fandhe-frontend-core = "0.1.0"`
- `fandhe-frontend-app = "0.1.0"`
- `fandhe-frontend-server = "0.1.0"`

本クレートは `publish = false` で crates.io リリース対象外であり、`crates/core` の
`Cargo.toml` では依存しないため、本体サーババイナリ・本体 release ビルド依存ツリー、
および feature 有効/無効のいずれの構成にも一切含まれない（docs 生成ツール専用。
CI の `docs-site.yml` ワークフロー実行時のみビルド対象）。pay-for-what-you-use
原則に抵触しない。

```
$ cargo tree -p fandhe-backend-core | grep fandhe-frontend
（該当なし）
$ cargo tree -p fandhe-backend-docs-site | grep fandhe-frontend
├── fandhe-frontend-core v0.1.0
├── fandhe-frontend-app v0.1.0
└── fandhe-frontend-server v0.1.0
```

### 機能・特徴

- `site/nav.toml` + `docs/guide/**` から静的 HTML サイト生成
- base_path = `/fandhe-backend`（GitHub Pages 上の相対パス）
- 内蔵 linkcheck（fail-closed）：リンク切れ検出時は書き出さない

### unsafe 件数

`unsafe` は 0 件（crate 自体の unsafe ブロック・テキストベース走査ともに 0 件。
外部依存（fandhe-frontend-* 系）由来の unsafe は対象外）。

### CI ワークフロー

`.github/workflows/docs-site.yml` にて main への docs/guide・site・crates/docs-site
変更 push で自動ビルド → GitHub Pages デプロイを実行。Pages Source=Actions の事前有効化が必要
（ワークフロー・PR テンプレートのコメント記載参照）。

## 2026-07-21 — `crates/plugin-openapi` に利用者アプリ独自 OpenAPI スキーマ登録
API を追加（イシュー #320）

`Server::openapi_with(doc)` / `fandhe_backend_plugin_openapi::OpenApiDoc` を追加し、
`crates/plugin-openapi/Cargo.toml` の `serde_json` を `gen-cli` feature 限定の
optional 依存から通常依存へ変更した（`OpenApiDoc::from_json` の JSON 構文検証に
使う）。

### 依存の残留確認（pay-for-what-you-use）

`serde_json` は変更前から `utoipa`（本クレートの常時有効な依存）が推移的に
引き込んでいたため、通常依存化しても `cargo tree` 上の推移依存差はゼロ。

```
$ cargo tree -p fandhe-backend-plugin-openapi --no-default-features -e normal
fandhe-backend-plugin-openapi v0.1.0
├── serde v1.0.229 (...)
├── serde_json v1.0.151 (...)   # 変更前は utoipa の推移依存としてのみ出現していた版と同一
└── utoipa v5.5.0
    ├── ...
    └── serde_json v1.0.151 (*)  # 通常依存化前からここに存在（同一バージョン解決）
```

`crates/core` 側（`openapi` feature 有効時）の依存クレート数にも変化なし
（`cargo tree -p fandhe-backend-core --features openapi` の `serde_json` 出現数は
変更前後とも 2 箇所＝`utoipa` 経由と `fandhe-backend-plugin-openapi` 直接の union）。
`openapi` feature 無効時（既定構成）は `fandhe-backend-plugin-openapi` 自体が
`cargo tree` から消えるため、本変更は無効時の依存グラフに一切影響しない。

### 新規追加型・API

`OpenApiDoc` / `OpenApiDocError`（`crates/plugin-openapi/src/custom.rs`）は
`std` のみで実装（`serde_json::Value` の妥当性検証を除き外部依存なし）。
`Server::openapi_with` は `crates/core` 側に新規依存を追加しない（既存の
`fandhe-backend-plugin-openapi`（optional dep）が公開する型を受け取るのみ）。

### unsafe 件数

`unsafe` は 0 件（`custom.rs` 全体で `unsafe` ブロックなし。`unsafe-triage.sh` の
テキストベース走査でも 0 件を確認）。

## 2026-07-21 — `crates/plugin-static` 追加（イシュー #318）

静的ファイル配信プラグイン（`static` feature）を新設した。`graphql`・`openapi` と
同じパスインターセプト型（`try_intercept`、設定登録型）で配線し、外部 crates.io
依存はゼロ（`fandhe-backend-http` + `tokio`（`rt` feature、`spawn_blocking` 用）
のみ、`docs/design/plugin-boundary.md` 5.11 節）。

### 依存の残留確認（pay-for-what-you-use）

```
$ cargo tree -p fandhe-backend-core --no-default-features | grep -c plugin-static
0
$ cargo tree -p fandhe-backend-core --no-default-features --features static | grep plugin-static
├── fandhe-backend-plugin-static v0.1.0 (crates/plugin-static)
```

`static` feature 有効時に増える workspace 内依存は `fandhe-backend-plugin-static`
1 件のみ。`tokio` は `fandhe-backend-core` 自体が既に依存済み（`rt`/`net`/`io-util`/
`time`/`sync` feature）のため、本プラグインが要求する `rt` feature の追加による
新規外部依存の増分はゼロ。MIME 推定は crate 内蔵の静的テーブル
（`crates/plugin-static/src/mime.rs`）で行い、`mime_guess` 等の外部依存は追加しない。

### unsafe 件数

`unsafe` は 0 件（`crates/plugin-static/src/lib.rs`・`src/mime.rs` 全体で `unsafe`
ブロックなし。`cargo-geiger` 未導入のため厳密計測は未実施、`unsafe-triage.sh` の
テキストベース走査でも 0 件を確認）。

## 2026-07-20 — `crates/plugin-compression` 追加（イシュー #321）

レスポンス圧縮プラグイン（`compression` feature）を新設した。`crates/plugin-cors`
（#305）が確立した「レスポンス後処理型」シーム（`docs/design/plugin-boundary.md`
5.9 節）の第 2 インスタンスとして配線し、外部 crates.io 依存は `flate2`
（`default-features = false` + `rust_backend`、純 Rust の miniz_oxide 実装に固定
し C 実装＝zlib へのリンクを排除）のみ。

### 依存の残留確認（pay-for-what-you-use）

```
$ cargo tree -p fandhe-backend-core --no-default-features -e normal | grep -c -E "plugin-compression|flate2|miniz_oxide"
0
$ cargo tree -p fandhe-backend-core --no-default-features --features compression | grep -E "plugin-compression|flate2|miniz_oxide|crc32fast|adler2"
├── fandhe-backend-plugin-compression v0.1.0 (crates/plugin-compression)
│   └── flate2 v1.1.9
│       ├── crc32fast v1.5.0
│       └── miniz_oxide v0.8.9
│           ├── adler2 v2.0.1
├── flate2 v1.1.9 (*)
```

`compression` feature 有効時に増える workspace 内依存は
`fandhe-backend-plugin-compression` 1 件、外部 crates.io 依存は `flate2` と
その推移的依存（`crc32fast` / `miniz_oxide` / `adler2`）の計 4 件。無効時は
これらが `cargo tree -e normal`（release ビルドに含まれる通常依存のみ）に
一切現れないことを確認済み（`-e normal` を付けない素の `cargo tree` は
`fandhe-backend-http` の dev-dependencies 経由で `flate2` がテスト専用に解決
されるため一致率確認には `-e normal` が必要。テストコードは release
バイナリに含まれないため pay-for-what-you-use 違反ではない、
`scripts/pay-for-what-you-use-check.sh` の `cargo tree` 検証もこれと同じ
`-e normal` 相当のフィルタで実行し PASS を確認済み）。

### unsafe 件数

`crates/plugin-compression/src/lib.rs` 全体で `unsafe` ブロックは 0 件
（`cargo-geiger` 未導入のため厳密計測は未実施。無効構成の依存グラフに
プラグインクレート自体が現れないため geiger 計上対象にもならない、
`scripts/pay-for-what-you-use-check.sh` c 項の実行結果と整合）。

## 2026-07-20 — `crates/plugin-cors` 追加（イシュー #305）

CORS プラグイン（`cors` feature）を新設した。「レスポンス後処理型」という
新パターン（`docs/design/plugin-boundary.md` 5.9 節）で配線し、外部 crates.io
依存はゼロ（`fandhe-backend-http` のみに依存）。

### 依存の残留確認（pay-for-what-you-use）

```
$ cargo tree -p fandhe-backend-core --no-default-features | grep -c plugin-cors
0
$ cargo tree -p fandhe-backend-core --no-default-features --features cors | grep plugin-cors
├── fandhe-backend-plugin-cors v0.1.0 (crates/plugin-cors)
```

`cors` feature 有効時に増える workspace 内依存は `fandhe-backend-plugin-cors`
1 件のみ。外部 crates.io 依存の増分はゼロ（std のみで実装、
`crates/plugin-cors/Cargo.toml` の依存は `fandhe-backend-http` のみ）。

### unsafe 件数

`unsafe` は 0 件（`crates/plugin-cors/src/lib.rs` 全体で `unsafe` ブロックなし。
`cargo-geiger` 未導入のため厳密計測は未実施、`unsafe-triage.sh` のテキスト
ベース走査でも 0 件を確認）。

## 2026-07-20 — `GET /openapi.yaml` 配信・gen-openapi YAML 生成の追加（#279）

仕様（`docs/spec/04-requirements.md`）が明記する「GET /openapi.json（GET /openapi.yaml
も同等に提供）」との不一致を解消するため、`crates/plugin-openapi` の `gen-cli`
feature に `utoipa/yaml`（serde_norway 経由）を追加し、`openapi.yaml` の静的埋め込み
（`OPENAPI_YAML`）・`GET /openapi.yaml` 配信を実装した。

### 依存の残留確認（pay-for-what-you-use）

`utoipa/yaml`（serde_norway・unsafe-libyaml-norway）は `gen-cli` feature（開発用 CLI、
`required-features` によりサーバービルド対象外）に限定して有効化した。

```
$ cargo tree -p fandhe-backend-core --no-default-features | grep -i "yaml\|norway"
（該当なし）
$ cargo tree -p fandhe-backend-core --features openapi | grep -i "yaml\|norway"
（該当なし）
$ cargo tree -p fandhe-backend-plugin-openapi --features gen-cli | grep -i "yaml\|norway"
    ├── serde_norway v0.9.42
    │   └── unsafe-libyaml-norway v0.2.15
```

サーバー側（`openapi` feature 有効・`gen-cli` 無効の通常経路）の依存クレート数・
`fandhe-backend-core` の依存ツリーには変化なし。`gen-cli` feature（開発ツール専用、
CI の `openapi-two-stage` ジョブ・ローカル再生成時のみビルド対象）でのみ
serde_norway・unsafe-libyaml-norway が新規に加わる。

### unmaintained crate（serde_yaml）を避けた理由

仕様本文が挙げる「serde_yaml 等」は unmaintained（RUSTSEC 情報勧告あり）のため直接
依存に加えず、utoipa 公式の `yaml` feature が使う保守フォーク serde_norway を採用した
（`.claude/rules/security.md` A06 依存の脆弱性対策）。

### unsafe 件数

`gen-cli` feature（開発ツール限定）にのみ影響し、サーバー本体の実行時経路・
既存 unsafe 件数には影響しない（`cargo-geiger` 未導入のため厳密計測は未実施。
`unsafe-triage.sh` のテキストベース走査で本クレートの `.rs` ソース側 unsafe 件数は
変化なし（0 件）を確認）。

## 2026-07-19 — REQ-8 webrtc unsafe 増分の削減策評価とリスク受容判断確定（#242）

`docs/acceptance/req8-webrtc-attack-surface.md` の基準 B補足（依存側 unsafe 増分、WARN）に
ついて、悪化要因の特定記録の確定・削減策の評価・削減不能な残余リスクの受容判断の 3 点を
確定させた（親トラッキング #235 の Conditional Go 条件(2)「WebRTC 別プロセス切り出し・
攻撃表面評価」の「条件付き解消」を解消するための対応）。

**スコープの明確化**: 本エントリは基準 B補足（unsafe 増分、WARN）のみを対象とする。
同レポートに併存する基準 E（NFR-6 無関係パス性能影響、FAIL）は host contention（#178）に
起因する別課題であり、本エントリでは一切扱わない（再論しない）。

### 1. 悪化要因の特定記録の確定（既存記録の再掲・結論固定）

見かけの「約 2.2 倍→約 4.4 倍」の乖離要因は **#183 で既に特定済み**（本ファイル下記
「`webrtc` feature の unsafe 増分乖離（PoC-5 比 2.2 倍→実測 4.4 倍）の原因特定（#183）」
エントリ）であり、2026-07-19 の再実測（#220、`docs/acceptance/req8-webrtc-attack-surface.md`
「2026-07 再実行」節）でも Functions 69/170・304/592 が完全一致することを確認済みである。
本イシューはこの結論を **確定した最終結論として固定**する。

- **主因**: ベースライン圧縮効果。pay-for-what-you-use を徹底した `crates/core` は
  baseline（webrtc 無効時）が 69 と小さく、PoC-5 の PoC 用スケルトン（baseline
  110〜111）に比べて同程度の絶対増分でも比率が拡大する。
- **副次要因**: `cargo-geiger` の到達可能性（used）判定がバージョン・環境に依存し、
  同一ソース・同一 `Cargo.lock` でも再現性を完全には持たない（実測で 247→306 のずれを確認済み）。
- **バージョン差は要因から棄却済み**: `webrtc-rs` は PoC-5・#183・本エントリの全時点で
  一貫して 0.17.1（後述「2. 削減策の評価」でも最新版であることを確認）。
- **実害の有無**: 依存側 unsafe の絶対量（feature 有効時 total 592〜594）は両時点で
  ほぼ不変であり、新規の危険 unsafe パターン（`// SAFETY:` 欠落を伴う生ポインタ操作等）は
  確認されていない。**見かけの 4.4 倍は比率の誇張であり、実害の増加ではない**という
  結論を本エントリで最終確定する。

### 2. 削減策の評価（バージョン更新・feature 絞り込み）

#### 2.1 バージョン更新 — 不適用

`docs/design/webrtc-rs-version-strategy.md` の 2026-07-17 再確認結果（crates.io 上の
`webrtc` クレート最新版は引き続き v0.17.1、Sans-I/O 系は別クレート `rtc` へ分離され
`webrtc` 本体の v0.20 化は確認されず）を根拠とする。**本エントリ作成時点（2026-07-19）で
この記録から 2 日しか経過しておらず、かつ本セッションはネットワークアクセス権限を持たない
subagent 実行環境のため、crates.io への再照会は実施できなかった**（`curl` / `cargo search`
とも権限エラーで失敗、実行不能を確認済み。捏造しない・断定と推測の区別、
`.claude/rules/japanese-style.md`・`.claude/rules/security.md`）。したがって
「2026-07-17 時点で v0.17.1 が最新」という直近の確定記録を根拠として据え、バージョン更新は
以下の理由により**不適用**と判定する。

- 安定した後継版（`webrtc` v0.20 系）が存在せず、Sans-I/O 系の `rtc` クレートは
  webrtc-rs-version-strategy.md の移行トリガー（安定版リリース）が未成立のため採用対象外
- `webrtc-rs-version-strategy.md` は当面 v0.17.x（保守モード）継続を決定事項としており、
  unsafe 削減のみを目的とした先行バージョン更新はこの決定と整合しない
- 直近 2 日間で webrtc 系クレートに RUSTSEC advisory が新規発行された兆候はない
  （`bash scripts/dep-audit.sh` が CI schedule で監視を継続しており、基準 C は PASS 継続）

**フォローアップ**: crates.io の最新版確認は次回 `webrtc-rs-version-strategy.md` の
定期再確認（同ドキュメント記載の移行トリガー監視）またはネットワークアクセス権限を持つ
セッションでの再確認時に行う。本エントリはこの確認を「省略」したのではなく、「実行不能を
確認したうえで直近の確定記録に依拠した」ことを明記して残す。

#### 2.2 feature 絞り込み — 不適用（現状では安全な除去候補を特定できず）

`crates/plugin-webrtc` が実際に使う機能は SDP Offer/Answer・データチャネル確立
（`crates/plugin-webrtc/src/lib.rs` の `RTCPeerConnection` 経由 API）のみであり、
`webrtc` 0.17 系は crates.io 上で default feature を分割公開していない
（`webrtc` クレートは単一の default feature 構成で SDP/ICE/SCTP/DataChannel が
不可分に結合しており、部分的な機能除去による依存削減の余地が確認できなかった）。
除去候補の機械的な列挙（`cargo metadata` によるオプション feature 一覧確認）は
本エントリの調査範囲では実施しておらず、確度の高い安全な除去候補は特定できなかった
ため、**現時点では不適用**と判定する。

- 安全に除去できる default-on feature が具体的に特定できた場合は
  `crates/plugin-webrtc/Cargo.toml` の production 変更・テスト追加を伴うため、
  `.claude/rules/out-of-scope-tracking.md` に従い**別 Issue として切り出す**方針とし、
  本エントリ（ドキュメント確定タスク）では適用しない
- 現状の不適用判定は「調査不足による保留」であり「機能除去不可能と断定した」わけではない
  ことを明記する（断定と推測の区別）

#### 2.3 不適用の総合根拠（既存の攻撃表面最小化方針の再確認）

- in-process 版（`crates/plugin-webrtc`）は既に非推奨。MVP 推奨は別プロセス切り出し版
  （`crates/plugin-webrtc-proxy`）で、こちらは `webrtc-rs` に一切依存しない
  （`docs/acceptance/req8-webrtc-attack-surface.md` 基準 D補足）
- フレームワーク本体は `webrtc` feature が既定で無効であり、無効時は `cargo tree` に
  webrtc 系依存が 0 件（基準 A、pay-for-what-you-use を既に満たす）
- 依存側 unsafe の絶対量は #183 で確認済みのとおり不変・危険パターンなし

### 3. 削減不能な残余リスクの受容判断（提案・PR レビュー承認で確定）

上記のとおり、バージョン更新・feature 絞り込みのいずれも本時点では適用しないため、
`webrtc` feature を有効化した in-process WebRTC 利用時の依存側 unsafe（大きな絶対量、
feature 有効時 total 592〜594）は削減されずに残る。この残余リスクについて、以下を
**リスク受容案（提案）**として記録する。

- **リスクの所在**: 残余リスクは、`AGENTS.md`「WebRTC の攻撃表面と『使う/使わない』
  サービスの安全性方針」および `crates/plugin-webrtc/Cargo.toml` の「高攻撃表面の
  選択肢」コメントに既に明記されているとおり、**明示的に非推奨の in-process プラグイン
  （`webrtc` feature）を opt-in したサービスにのみ顕在化する**。既定構成・別プロセス
  切り出し版（`plugin-webrtc-proxy`）を利用するサービスは本リスクの影響を受けない
  （pay-for-what-you-use により依存グラフから完全除外）。
- **受容の根拠**（新しい正当化を創作せず、既存フレーミングに依拠）:
  1. `webrtc-rs` は攻撃表面の大きさを承知のうえで PoC-5・TASK-8.1〜8.3 を経て採用された
     既知のトレードオフであり、別プロセス切り出し設計（`crates/plugin-webrtc-proxy`）
     という緩和策が既に提供されている
  2. 依存側 unsafe の絶対量は変化しておらず、新規の危険パターンも確認されていない
     （#183・本エントリ 1 節）
  3. `cargo audit` / `cargo deny check`（基準 C）による既知脆弱性監視は CI schedule で
     継続しており、監視体制自体は本判断で弱めない
- **承認フローの扱い（自動運転モード）**: 本判断は自動運転モードでの実装のため
  ユーザー承認を待たずに記録するが、**最終承認は本タスクの PR レビュー（人間承認ゲート）
  で行う**。これは `webrtc-rs-version-strategy.md`（「最終承認は人間レビュー（本タスクの
  PR レビュー）で行う」）・`.claude/rules/feature-modification.md`（受け入れ基準充足は
  人間レビューゲート）の既存前例と同一原則に従う。

**基準 B補足の確定扱い**: 上記 1〜3 節を根拠に、`docs/acceptance/req8-webrtc-attack-surface.md`
の基準 B補足を「**受容 WARN（削減不能・残余リスク受容済み、PR レビュー承認をもって確定）**」
として記載を更新した。これにより親トラッキング #235 の Conditional Go 条件(2) の
「条件付き解消」を解消できる状態になる（最終確定は PR レビュー承認後）。

### 検証方法

- `bash scripts/tests/run-webrtc-accept-tests.sh`（cargo 非依存のオフラインセルフテスト、
  判定ロジック `scripts/accept/lib/nfr6-ratio.sh` には触れないため非退行）
- `docs/acceptance/req8-webrtc-attack-surface.md` の基準 B補足・判定サマリー表の更新箇所
- 次回バージョン再確認は `webrtc-rs-version-strategy.md` の移行トリガー監視、または
  ネットワークアクセス権限を持つセッションでの `cargo search` / crates.io 参照で実施

## 2026-07-18 — `webrtc` feature の unsafe 増分乖離（PoC-5 比 2.2 倍→実測 4.4 倍）の原因特定（#183）

TASK-8.4（#29、上記「`crates/plugin-webrtc` 攻撃表面評価・単独再評価」エントリ）が
「BLOCKED / フォローアップ」として残した、PoC-5（`docs/spec/03-poc/webrtc-plugin/README.md`、
2026-07-08 実測「約 2.2 倍」）と TASK-8.4 実測（Functions 69/170 → 304/592、約 **4.4 倍**）
の乖離原因を、本 worktree（`rustc`/`cargo` 1.96.0、`cargo-geiger` 0.13.0）での再実測で
特定した（`docs/acceptance/req8-webrtc-attack-surface.md` の該当項目もあわせて更新）。

### 受け入れ条件 1: バージョン・計測対象範囲の比較

**webrtc-rs バージョン**は PoC-5 実測時点・TASK-8.4 実測時点・本タスク実測時点のすべてで
`webrtc` および `webrtc-data` / `webrtc-ice` / `webrtc-mdns` / `webrtc-media` /
`webrtc-sctp` / `webrtc-srtp` / `webrtc-util` が**すべて 0.17.1 で一致**することを確認した
（ルート `Cargo.lock` と PoC-5 側 `Cargo.lock`（後述の方法で参照）の双方を `grep` で確認）。
バージョン変化は乖離要因から**棄却できる**。

**計測対象範囲**は当初から異なる: PoC-5 は PoC 用スケルトン `pluggable-core`
（`docs/spec/03-poc/webrtc-plugin/core/`、`plugin-webrtc` feature）、TASK-8.4 以降は
`backend-framework-core`（`crates/core/`、`webrtc` feature）。両者とも `webrtc-rs`
本体への依存は同一だが、常時依存する周辺クレート構成（後述）が異なる。

### 受け入れ条件 2: 乖離要因の分解・特定

`docs/spec` submodule はコミット対象外の read-only 参照のため、PoC-5 の
`docs/spec/03-poc/webrtc-plugin/core/` をスクラッチパッドへコピーし（submodule 自体は
変更しない、`git -C docs/spec status` で無変更を確認済み）、同一 `Cargo.lock` を保持した
まま `cargo-geiger 0.13.0`（TASK-8.4 と同一バージョン）で再計測した。

```
# (a) backend-framework-core（TASK-8.4 と同一計測、本タスクで再現）
$ cargo geiger --output-format Ascii \
    --manifest-path "$(pwd)/crates/core/Cargo.toml" --no-default-features 2>&1 \
    | grep -E '^[0-9]+/[0-9]+' | tail -1
69/170     3812/6377    119/165 4/4     143/297
$ cargo geiger --output-format Ascii \
    --manifest-path "$(pwd)/crates/core/Cargo.toml" --features webrtc 2>&1 \
    | grep -E '^[0-9]+/[0-9]+' | tail -1
304/592    24704/33613  610/713 83/87   902/1263

# (b) PoC-5 の pluggable-core を同一 Cargo.lock のまま現環境で再計測
#     （CARGO_NET_OFFLINE=true は cargo-geiger 内蔵 cargo が並列 worktree 実行下の
#     レジストリキャッシュ競合で `assertion failed: self.pending_ids.insert(id)` を
#     panic する事象を避けるため、`cargo fetch` 後にオフラインで実行）
$ cargo fetch --no-default-features && CARGO_NET_OFFLINE=true \
    cargo geiger --output-format Ascii --no-default-features 2>&1 \
    | grep -E '^[0-9]+/[0-9]+' | tail -1
110/225    6293/9573    96/161 4/4     245/447
$ cargo fetch --features plugin-webrtc && CARGO_NET_OFFLINE=true \
    cargo geiger --output-format Ascii --features plugin-webrtc 2>&1 \
    | grep -E '^[0-9]+/[0-9]+' | tail -1
306/594    24618/33077  610/668 83/87   898/1232
```

Functions 列（PoC-5・TASK-8.4 と同一指標）で比較すると:

| 計測対象 | baseline | webrtc 有効 | 比率 |
|---|---|---|---|
| PoC-5 原記録（2026-07-08、geiger バージョン未記録） | 111/225 | 247/594 | 約 2.2 倍 |
| PoC-5 を本タスクで同一 `Cargo.lock` のまま再実測（geiger 0.13.0） | 110/225 | 306/594 | 約 2.78 倍 |
| `backend-framework-core`（TASK-8.4、#29） | 69/170 | 304/592 | 約 4.4 倍 |

baseline（110 vs 111）・feature 有効時 total（594 vs 594、592 vs 594 はほぼ同値）は
高精度に再現し、**同一ソース・同一 `Cargo.lock` の計測手法自体は正しく再現できる**ことを
確認した。一方で feature 有効時の **used Functions**（247 → 306）は同一ソース・同一
`Cargo.lock` にもかかわらず 59 件（約 24%）増加した。乖離を 2 要因に分解する:

1. **baseline（分母）の計測対象差（主因）**: PoC-5 の `pluggable-core` は常時依存に
   `tokio`（`rt-multi-thread` 含む）・`serde`（derive）・`serde_json` を持ち baseline が
   110〜111 と大きい。`backend-framework-core` は pay-for-what-you-use 徹底により
   baseline が 69 と小さい。同程度の絶対増分（PoC: 306-110=196、core: 304-69=235）
   でも、**分母が小さいほど比率は拡大する**（110→306 は 2.78 倍、69→304 は 4.4 倍）。
   絶対増分自体も 196 vs 235 とやや core 側が大きく、これは `crates/plugin-webrtc`
   の実装コードが PoC スケルトンより `webrtc-rs` API を広く呼び出すことに起因すると
   考えられる（未確定、残余の不確実性として明記）。
2. **geiger バージョン差・到達可能性判定の非決定性（副次的要因）**: PoC-5 原記録は
   使用した `cargo-geiger` バージョンが記録されておらず、本タスクの `0.13.0` と異なる
   可能性が高い。同一ソース・同一ロックファイルで used が 247 → 306（+59）とずれた
   ことから、feature 有効時の到達可能性（used/reachable）判定は geiger の実装・
   バージョンに依存し、**ソース・依存が完全に同一でも数値が変動しうる**ことが実測で
   裏付けられた。これにより比率だけで「増分が悪化した」と単純評価するのは不適切
   （断定と推測の区別、`.claude/rules/japanese-style.md`）。

**結論**: 乖離の主因は **(1) 計測対象範囲差によるベースライン圧縮効果**（pay-for-what-
you-use が徹底された分だけ比率が誇張される）であり、副次的に **(2) cargo-geiger の
到達可能性判定がバージョン・環境に依存し完全な再現性を持たない**ことも実測で確認した。
**webrtc-rs バージョン変化は要因ではない**（受け入れ条件 1 で 0.17.1 同一と確認済み）。

### 受け入れ条件 3: 実害なしの確認

- 依存側 unsafe（`webrtc-rs` 由来）の**絶対量自体は両時点でほぼ不変**: feature 有効時
  total は 594（PoC-5）→ 592〜594（TASK-8.4・本タスク再実測）とほぼ同値であり、
  `webrtc-rs` 0.17.1 自体の unsafe コード量に変化はない
- `bash scripts/unsafe-triage.sh` で自コード（`crates/plugin-webrtc` 含む全クレート）の
  `unsafe` 0 件・baseline から変化なしを再確認した
- `bash scripts/dep-audit.sh`（全 feature 構成）で `cargo audit` 既知脆弱性 0 件・
  `cargo deny check` 違反 0 件を再確認した（詳細は
  `docs/acceptance/req8-webrtc-attack-surface.md` 参照）
- `webrtc` feature 有効時に新規で used となったクレート集合を baseline との差分で
  確認したところ、`icu_*` / `zerovec` / `idna` / `percent-encoding` 系（URL・IDNA
  正規化処理）など STUN/ICE の URL 解析で一般的に使われる既知の周辺クレートのみで
  あり、`// SAFETY:` 欠落を伴う生ポインタ操作等の新規の危険パターンは確認されなかった

### 実測環境・再現手順の注意

- `cargo-geiger` は内蔵する `cargo` ライブラリ（本環境では `cargo-0.86.0` 系）が、
  並列 worktree での同時ビルド実行下ではレジストリキャッシュへの同時アクセスで
  `panicked ... assertion failed: self.pending_ids.insert(id)` を発生させることがある
  （`cargo_clean::clean` 内の download 重複防止アサーション）。再現する場合は
  `cargo fetch` で依存取得を完了させたのち `CARGO_NET_OFFLINE=true` を付けて
  `cargo geiger` を実行すると回避できる（本件は cargo-geiger 側の並行実行時の
  既知でない不具合であり、本タスクのスコープでは回避策の記録に留める）
- PoC-5 側の再計測は `docs/spec`（submodule）をスクラッチパッドへコピーして実行した。
  submodule 自体への変更は一切行っていない（`git -C docs/spec status` で確認済み）

## 2026-07-18 — `crates/plugin-websocket` アイドルタイムアウト実装（#175）に伴う tokio feature 追加

WebSocket セッションのアイドルタイムアウト実装（Issue #175、`session::run_echo_session`
が `tokio::time::timeout` で受信待ちを監視）のため、`crates/plugin-websocket/Cargo.toml`
の lib 依存 `tokio` に `time` feature を追加した（従来は `io-util` のみ）。

### 新規クレート増加の有無

`tokio` の `time` feature は `tokio` crate 自体の内部モジュール切り替えであり、新規の
外部クレートを追加しない。加えてコア（`crates/core/Cargo.toml`）は `websocket`
feature 有無に関わらず既に `tokio` の `time` feature を有効化済み
（accept ループのタイムアウト等で使用）のため、`websocket` feature 有効時の
`backend-framework-core` の依存グラフに実質差分は生じない。

```
$ cargo tree -p backend-framework-core --features websocket -e normal --no-default-features \
    | grep -c 'bf-plugin-websocket\|tokio-tungstenite\|futures-util'
4
```

（本コマンドは websocket feature 経由で配線される依存の内訳確認のみで、本変更前後
での件数差分はない。`tokio` 自体は feature 追加のみで crate 数としては増減なし。）

### pay-for-what-you-use の継続確認

```
$ cargo tree -p backend-framework-core -e normal --no-default-features | grep -c 'tungstenite'
0
```

`websocket` feature 無効時は本クレート自体が依存グラフから除外される現状に変化なし。

計測コマンド: `cargo tree`（`scripts/dep-impact.sh` によるフル計測は本変更が軽微
（feature 追加のみ・新規クレート 0 件）のため今回は省略し、上記個別確認に留めた）。

## 2026-07-18 — `crates/plugin-tracing` 依存インパクト記録（#60、TASK-10.5）

TASK-10.1（#56）で `crates/plugin-tracing` を追加した際、依存インパクト実測記録は
本タスク（TASK-10.5）へ切り出されていた（#151 のコミットメッセージ「対象外: 依存
インパクト文書化(#60)」）。本エントリで PoC-10（`docs/spec/04-requirements.md` REQ-10。
サンプリングなし構成での実測: 依存 +26 クレート・バイナリサイズ +57.6%・RSS +301.4%）
との比較込みで実測記録する（本 worktree での実測、`rustc`/`cargo` 1.96.0、
`Cargo.lock` は本 PR ブランチのもの、計測日時 2026-07-18）。

### 依存クレート数（`tracing` feature 有効/無効の `cargo tree -p backend-framework-core` 差分）

```
$ cargo tree -p backend-framework-core -e normal --no-default-features \
    | grep -c -E 'bf-plugin-tracing|^tracing|tracing-subscriber|tracing-appender'
0
$ cargo tree -p backend-framework-core -e normal --no-default-features --features tracing \
    | grep -c -E 'bf-plugin-tracing|^tracing|tracing-subscriber|tracing-appender'
4
```

`tracing` feature 無効時（既定）は 0 件で完全除外を維持（pay-for-what-you-use、
`.claude/rules/pay-for-what-you-use.md`）。`grep -c` は行単位の出現数（`(*)` で
省略される再掲ノードは数えない）であり、実クレート数は各ノード配下の推移依存を
展開した実数で見る必要がある。そこで無効/有効それぞれの `cargo tree` 出力から
`name vX.Y.Z` 形式のユニークパッケージ集合を抽出し、union 差分（既存エントリと
同一手法、workspace メンバーは除外しない）を取ったところ、無効時 9 件・有効時
33 件で **+24 クレート**（`bf-plugin-tracing` 自身を含む。内訳: `tracing` /
`tracing-core` / `tracing-subscriber` / `tracing-appender` 本体 4 件 + 推移依存
`crossbeam-channel` / `crossbeam-utils` / `symlink` / `thiserror` / `thiserror-impl` /
`proc-macro2` / `quote` / `syn` / `unicode-ident` / `time` / `time-core` /
`deranged` / `num-conv` / `powerfmt` / `once_cell` / `sharded-slab` / `lazy_static` /
`thread_local` / `cfg-if` 19 件）。PoC-10 実測「+26 クレート」とはほぼ同オーダーで
整合する（差分 2 件は `Cargo.lock` のバージョン解決差によるノイズと考えられる）。

### feature 無効時の完全除外確認（pay-for-what-you-use）

上記の通り `cargo tree -p backend-framework-core -e normal --no-default-features` に
`bf-plugin-tracing` / `tracing` / `tracing-subscriber` / `tracing-appender` は
一切現れない（0 件）。`bash scripts/pay-for-what-you-use-check.sh` の既存チェック
（他プラグイン feature を対象）と同一方式であり、`tracing` feature 専用の機械検証は
`scripts/accept/tracing-accept.sh` の A チェック（既存、TASK-10.4）および本タスクで
追加した D/E チェック（後述）が担う。

### リリースバイナリサイズ（feature 有効/無効の実バイナリ比較）

TASK-10.4（#59）で追加済みの計測専用 example（`crates/core/examples/minimal.rs`＝
`tracing` 無効ベースライン、`crates/core/examples/tracing_nfr.rs`＝`tracing` 有効・
`Server::tracing` 登録済み）を用い、release バイナリサイズを直接比較した。

```
$ cargo build --release -p backend-framework-core --example minimal --no-default-features
$ cargo build --release -p backend-framework-core --example tracing_nfr --features tracing
$ stat -c '%s %n' target/release/examples/minimal target/release/examples/tracing_nfr
799144 target/release/examples/minimal
1059800 target/release/examples/tracing_nfr
```

| バイナリ | 構成 | サイズ (bytes) |
|---|---|---|
| `examples/minimal` | `--no-default-features`（tracing 無効） | 799,144 |
| `examples/tracing_nfr` | `--features tracing`（tracing 有効、`Server::tracing` 登録） | 1,059,800 |

増分: +260,656 bytes（約 **+32.6%**）。PoC-10 実測「+57.6%」より小さい。両 example は
同一の `GET /`・`GET /health` ハンドラのみを持つ最小構成であり、PoC-10 計測時の
実装（サンプリングなし・非同期 I/O 化前の初期実装）とはコード規模・最適化状態が
異なることがノイズ要因。`webrtc`（TASK-8.4 エントリ、約 11.08 倍で PoC-5 の
「約 10.4 倍」と整合）ほど厳密な再現ではないが、増分の桁（数十%オーダー）は
PoC-10 の劣化トレンドと矛盾しない。

### RSS（アイドル時・負荷時）

`benches/bench-rss.sh` の方式（負荷印加中に `ps -o rss=` を複数回サンプリングし
中央値を取る）に倣い、両バイナリを起動し `oha`（`oha 1.15.0`）で `GET /health` へ
5 秒間・同時接続 32 の負荷をかけて RSS を計測した（1 試行のみ。`benches/bench-rss.sh`
の複数試行中央値方式ほど厳密ではない点は既知の制約として明記する）。

```
$ ./target/release/examples/minimal &   # idle: 2,980 KB
$ ./target/release/examples/tracing_nfr &   # idle: 7,312 KB
$ oha -z 5s -c 32 --no-tui http://127.0.0.1:3000/health   # 負荷中 RSS サンプル: 3196/3240/3240/3244 KB（中央値 3,240 KB）
$ oha -z 5s -c 32 --no-tui http://127.0.0.1:3006/health   # 負荷中 RSS サンプル: 7496/7500/7540/7556 KB（中央値 7,520 KB）
```

| 状態 | `examples/minimal`（無効） | `examples/tracing_nfr`（有効） | 増分 |
|---|---|---|---|
| アイドル RSS | 2,980 KB | 7,312 KB | +145.4% |
| 負荷時 RSS（中央値） | 3,240 KB | 7,520 KB | +132.1% |

PoC-10 実測「RSS +301.4%」より小さい増分。`init_tracing` の non_blocking writer が
専用ワーカースレッド + バッファを常時保持するため tracing 有効時は絶対値として
数 MB 増える一方、TASK-10.1〜10.3 の緩和策（サンプリング・イベント統合・高頻度
パス除外）適用後は PoC-10 実測ほどの相対増分にはならないことが確認できた。

参考: 上記負荷計測中の RPS は `examples/minimal` 147,501 req/s・`examples/tracing_nfr`
143,116 req/s（比 97.0%、`scripts/accept/tracing-accept.sh` C チェックの正式計測
（`benches/tracing-nfr-bench.sh`、`GET /health` 除外適用シナリオ）とは別の簡易計測。
正式な RPS/p95 受け入れ判定は同スクリプト・`benches/reports/task-10.4-tracing-performance.md`
を参照）。

### unsafe 件数

`scripts/unsafe-triage.sh`（テキストベース走査、`scripts/unsafe-baseline.json` 参照）で
自コード（`crates/plugin-tracing`）の `unsafe` は 0 件（baseline から変化なし）。

依存側 `unsafe` は `cargo-geiger 0.13.0` により実測した（TASK-8.4 エントリと同じく
`crates/core/Cargo.toml` を絶対パスで `--manifest-path` に指定）:

```
$ cargo geiger --output-format Ascii \
    --manifest-path "$(pwd)/crates/core/Cargo.toml" --no-default-features 2>&1 | tail -1
69/170     3812/6377    119/165 4/4     143/297
$ cargo geiger --output-format Ascii \
    --manifest-path "$(pwd)/crates/core/Cargo.toml" --features tracing 2>&1 | tail -3
85/250     5968/15429   163/283 4/7     194/722
```

列は関数/式/終端子/型/クロージャの `unsafe 使用/全体` 件数（geiger 標準フォーマット）。
`tracing` feature 有効化により関数 69→85（+16）・式 3812→5968（+2156）・終端子
119→163（+44）・型 4→4（±0）・クロージャ 143→194（+51）と増加するが、いずれも
`bf-plugin-tracing` 自身の出力は全列 `0/0`（自コード unsafe 0 件、上記 unsafe-triage.sh
結果と一致）であり、増分は推移依存（`crossbeam-channel`・`time`・`tracing-subscriber`
系の `!` マーク付き crate）由来。

### workspace 全体の参考値（`scripts/dep-impact.sh`）

```
$ bash scripts/dep-impact.sh
```

| feature 構成 | 依存クレート数 |
|---|---|
| --no-default-features | 271 |
| default | 271 |
| --all-features | 271 |

TASK-8.1 エントリ以降と同じ既知の制約（`bf-plugin-tracing` が workspace メンバーの
ため `cargo metadata` ベースの本スクリプトは 3 構成で同値になり、`tracing` feature
単体の有効/無効差分を区別できない）が引き続き成立する。`tracing` feature 単体の
実インパクトは上記の `cargo tree -p backend-framework-core --features tracing` 差分
（0 → 33 件、union 展開後の実数）で判断すること。

### まとめ・受け入れ基準との対応

| TASK-10.5 成果物 | 対応箇所 |
|---|---|
| 依存インパクト実測記録 | 本エントリ（依存クレート数・バイナリサイズ・RSS） |
| feature 無効時の依存完全除外確認 | 本エントリ「feature 無効時の完全除外確認」節 + `scripts/accept/tracing-accept.sh` A/D/E チェック |
| 連携方式設計文書 | `docs/design/tracing-integration.md` |
| 受け入れテストスクリプト・実行結果 | `scripts/accept/tracing-accept.sh`（D/E 追加）+ `docs/reports/task-10-5-acceptance.md` |

## 2026-07-17 — `crates/plugin-webrtc` 攻撃表面評価・単独再評価（#29、TASK-8.4）

TASK-8.1（#26）エントリ（下記）が既に記録した「`scripts/dep-impact.sh` は `plugin-webrtc`
が workspace メンバーのため 3 構成で同値になる」既知の制約を踏まえ、TASK-8.4 では
`cargo tree -p backend-framework-core --features <feature>` の per-feature 差分で
`webrtc` feature 単体の実インパクトを再計測した（本 worktree での実測、`cargo` 環境:
`rustc`/`cargo` 1.9x 系、`Cargo.lock` は本 PR ブランチのもの）。

```
$ cargo tree -p backend-framework-core | grep -c webrtc
0
$ cargo tree -p backend-framework-core --features webrtc | grep -c webrtc
23
$ cargo tree -p backend-framework-core --features webrtc-proxy | grep -c webrtc
1
$ cargo tree -p backend-framework-core --all-features | grep -c webrtc
24
```

`webrtc` feature 無効時（既定）は 0 件で完全除外を維持。`webrtc` feature 単体で 23 件、
`webrtc-proxy`（`webrtc-rs` 非依存の軽量プロキシ）は 1 件（自クレート `bf-plugin-webrtc-proxy`
自身のみ）。TASK-8.1 エントリの実測（0 → 23）と一致する。

### リリースバイナリサイズ（feature 有効/無効の実バイナリ比較）

TASK-8.4 で追加した計測専用 example（`crates/core/examples/webrtc_nfr6.rs`、NFR-6
計測と兼用）を用い、`webrtc` feature 有効/無効の release バイナリサイズを直接比較した
（`crates/core` 自体はライブラリ crate のため、workspace 内で `Server::webrtc` を実際に
リンクする example バイナリで比較する）。

| バイナリ | 構成 | サイズ (bytes) |
|---|---|---|
| `examples/minimal` | `--no-default-features`（webrtc 無効） | 798,688 |
| `examples/webrtc_nfr6` | `--features webrtc`（webrtc 有効、`Server::webrtc` 登録） | 8,846,544 |

比率: 約 **11.08 倍**（PoC-5 実測「約 10.4 倍」・`crates/plugin-webrtc/src/lib.rs` の
doc 記載と同オーダーで整合。差はビルド環境・rustc バージョン・LTO 設定等によるノイズの
範囲内と考えられる）。

### unsafe 件数

`scripts/unsafe-triage.sh`（テキストベース走査、`scripts/unsafe-baseline.json` 参照）で
自コード（`crates/plugin-webrtc`）の `unsafe` は 0 件（baseline から変化なし）。

依存側 `unsafe`（`webrtc-rs` 由来）は本タスクで `cargo-geiger 0.13.0` により実測できた。
`cargo geiger -p <name>` は workspace の仮想 manifest 配下で誤ったエラー（virtual
manifest 扱い）を返すため、`crates/core/Cargo.toml` を**絶対パス**で `--manifest-path`
に直接指定する必要がある（TASK-8.1 エントリ・従来の `unsafe-triage.sh` 実行記録は
この呼び出し方の制約に気づかず「本環境で実行失敗」としていたが、正しい呼び出し方で
再計測できたため本エントリで訂正する）:

```
$ cargo geiger --output-format Ascii \
    --manifest-path "$(pwd)/crates/core/Cargo.toml" --no-default-features 2>&1 \
    | grep -E '^[0-9]+/[0-9]+' | tail -1
69/170     3812/6367    119/165 4/4     143/297

$ cargo geiger --output-format Ascii \
    --manifest-path "$(pwd)/crates/core/Cargo.toml" --features webrtc 2>&1 \
    | grep -E '^[0-9]+/[0-9]+' | tail -1
304/592    24694/33174  610/674 83/87   902/1236

$ cargo geiger --output-format Ascii \
    --manifest-path "$(pwd)/crates/core/Cargo.toml" --features webrtc-proxy 2>&1 \
    | grep -E '^[0-9]+/[0-9]+' | tail -1
69/170     3812/6367    119/165 4/4     143/297
```

列は `Functions used/total  Expressions used/total  Impls used/total  Traits
used/total  Methods used/total`（cargo-geiger の Ascii 出力形式、`docs/spec` 記載の
PoC-5 と同一指標）。`webrtc-proxy` feature は baseline と完全に同値（69/170）であり、
`webrtc-rs` 非依存の設計方針どおり unsafe 増分がゼロであることを裏付ける。`webrtc`
feature は Functions 列で 69 → 304（約 **4.4 倍**）、Expressions 列で 3812 → 24694
（約 6.5 倍）と、PoC-5（`docs/spec/03-poc/webrtc-plugin/`）が記録した「約 2.2 倍」より
大幅に大きい増分を示した。PoC-5 実測時点と本タスク実測時点で `webrtc-rs` の
バージョン・依存構成・計測対象範囲（クレート単体 vs `backend-framework-core` 全体の
推移的依存込み）が異なる可能性があり、両者の数値差の原因特定は本タスクのスコープ外
（out-of-scope-tracking 候補）。**本エントリの数値は本環境での実測値であり、PoC-5 の
数値を上書きするものではなく、両方を併記する**（捏造しない、
`.claude/rules/security.md` のフェイルクローズ原則）。

### cargo audit / cargo deny check

`bash scripts/dep-audit.sh`（全 feature 構成: `--no-default-features` / `default` /
`--all-features`）を実行し、既知脆弱性 0 件・`cargo deny check`（advisories/bans/
licenses/sources）違反 0 件を確認した（TASK-8.4、詳細ログは
`docs/acceptance/req8-webrtc-attack-surface.md` 参照）。

### NFR-6（無関係パスへの RPS・レイテンシ影響）

`benches/webrtc-nfr6-bench.sh` による empirical 計測結果は
`benches/reports/task-8.4-webrtc-nfr6.md` を参照。狭義の NFR-6 帯（100.3〜100.8%相当）
には収まらず（RPS 比おおむね 94〜95%）、バイナリサイズ増（約 11 倍）に起因すると考え
られる実測上の性能影響が確認された（詳細は AGENTS.md 「WebRTC の攻撃表面と『使う/
使わない』サービスの安全性方針」節）。

## 2026-07-17 — `crates/plugin-graphql` を実 GraphQL 実行へ実装（#38、TASK-5.1）

TASK-2.4（#21）のスタブ（`bf-http` のみ依存、外部依存 0 件）に `async-graphql = "7"`
（`default-features = false`）・`serde`（derive）・`serde_json` を追加し、実クエリ実行を
実装。事前見積もり（実装計画、PoC 実測）は「+95 クレート、バイナリ +1.51MB」だったが、
実測は以下のとおり（`default-features = false` により playground/graphiql 等の
開発 UI 系依存を含めていないため見積もりより少ない）。

`bf-plugin-graphql` 自身が引き込む新規クレート数（`bf-http`・自身を除く）:

```
$ cargo tree -p bf-plugin-graphql -e normal --prefix none | sed 's/ (\*)$//' | sort -u \
    | grep -v '^bf-http \|^bf-plugin-graphql ' | wc -l
76
```

pay-for-what-you-use の受け入れ条件（`backend-framework-core` が `graphql` feature
無効時に `async-graphql`/`bf-plugin-graphql` 系依存を一切解決しないこと）は
`bash scripts/pay-for-what-you-use-check.sh` の全項目（cargo tree 陰性/陽性・
cargo geiger・バイナリサイズ・全構成ビルド）で PASS を確認済み:

```
[PASS] b: cargo tree 検証（無効構成） — 全プラグインクレートが依存グラフから 0 件
[PASS] b: cargo tree 検証（有効構成 graphql） — bf-plugin-graphql のみが出現し他プラグインの混入なし
[PASS] c: cargo geiger 検証 — 無効構成の依存グラフにプラグインクレートは 0 件（unsafe 計上対象なし）
[PASS] d: バイナリサイズ計測 — 無効構成 798680 bytes <= 有効構成 9106696 bytes（差分 8308016 bytes、
    `--all-features` ビルドとの比較のため他プラグイン分を含む）
```

計測コマンド: `bash scripts/dep-impact.sh`（workspace 全体の参考値）

### 依存クレート数（workspace メンバー除外・重複バージョンは union で 1 件として計上）

| feature 構成 | 依存クレート数（直前エントリ差分） |
|---|---|
| --no-default-features | 265（+37） |
| default | 265（+37） |
| --all-features | 265（+37） |

`crates/plugin-*` を含む全 workspace メンバーをそれぞれ自身の既定 feature で計測する
方式であり、`crates/core` 単体の pay-for-what-you-use 遵守を測るものではない点は
既存エントリと同様（`bf-plugin-graphql` 自身の新規依存が上記 76 件、workspace 全体の
union 差分が +37 なのは `async-graphql`/`serde`/`serde_json`/`thiserror` 系の一部が
既に他プラグイン（`plugin-webrtc`・`plugin-openapi` 等）経由で workspace の依存
グラフに存在済みで union 上重複計上されないため）。

`crates/core` の dev-dependencies（テスト専用、`async-graphql`（`dynamic-schema`
feature））はリリースビルドに含まれないため pay-for-what-you-use の対象外
（`.claude/rules/pay-for-what-you-use.md`）。

### リリースバイナリサイズ

| bin | サイズ (bytes) |
|---|---|
| axum-ref | 1374024 |

`axum-ref` は `bf-plugin-graphql` に依存しないため、直前エントリとの差分は
ビルド環境ノイズ（既存エントリと同様の注記）。

### unsafe 件数

`cargo-geiger` は `dep-impact.sh` 標準実行（`cargo geiger --all-features`）では
webrtc 系の巨大依存グラフ解決に失敗したため未計測。pay-for-what-you-use-check.sh の
`c: cargo geiger 検証` で「`graphql` feature 無効構成に対象クレート 0 件（unsafe
計上対象なし）」は個別に確認済み（上記参照）。

## 2026-07-17 — `crates/plugin-webrtc` 追加（#26、TASK-8.1）

`bf-plugin-webrtc`（`webrtc = "0.17"`（0.17.1 系）・`serde_json`・`tokio`（`time`
のみ）依存、workspace 内クレートへの依存は `bf-http` のみ）を追加。PoC-5 の実測どおり
`webrtc` は依存 +189 クレート級のインパクトを持つ。

`scripts/dep-impact.sh` は `cargo metadata` がワークスペース全体の依存グラフを解決する
（`--features`/`--no-default-features` を渡してもワークスペースメンバー自体の
manifest 解決には影響しない）という既知の制約により、`bf-plugin-webrtc` を workspace
メンバーとして追加した時点で 3 構成すべてに `webrtc` 系依存が計上され、
「feature 無効時の依存数が変化しないこと」を本スクリプト単体では区別できない
（`bf-plugin-webrtc-proxy` 追加時も同様の制約を受けていた）。

pay-for-what-you-use の受け入れ条件（`backend-framework-core` が `webrtc` feature
無効時に `webrtc` 系依存を一切解決しないこと）は、より正確な
`cargo tree -p backend-framework-core` で機械検証した:

```
$ cargo tree -p backend-framework-core | grep -c webrtc
0
$ cargo tree -p backend-framework-core --features webrtc | grep -c webrtc
23
```

計測コマンド: `bash scripts/dep-impact.sh`（workspace 全体の参考値。上記 `cargo tree`
差分検証と併読すること）

### 依存クレート数（workspace メンバー除外・重複バージョンは union で 1 件として計上）

| feature 構成 | 依存クレート数（ベースライン差分） |
|---|---|
| --no-default-features | 228（+174） |
| default | 228（+174） |
| --all-features | 228（+174） |

上記の理由により 3 構成とも同一値（スクリプトの既知の制約。`webrtc` feature 単体の
実インパクトは `cargo tree -p backend-framework-core --features webrtc` の差分
（0 → 23 件、推移依存込みの重複除外前カウント）で判断すること）。

### リリースバイナリサイズ

| bin | サイズ (bytes) | ベースライン差分 |
|---|---|---|
| axum-ref | 1373104 | +14808（`bf-plugin-webrtc` に依存しないが、workspace 全体の
  `Cargo.lock` 更新に伴うビルド環境差によるノイズ） |

### unsafe 件数

`cargo geiger` の実行が本環境で失敗したため未計測（`scripts/unsafe-triage.sh` による
テキストベース走査では自コード（`crates/plugin-webrtc`）の `unsafe` は 0 件。
依存側 `unsafe` 増分（`webrtc-rs` 由来）は PoC-5 実測（約 2.2 倍）を参照し、恒久的な
計測は TASK-8.4（#29、攻撃表面評価）のスコープとする）。

## 2026-07-17 — `crates/plugin-websocket` 追加（#22、TASK-4.1）

`bf-plugin-websocket`（`tokio-tungstenite = "0.30"`（`default-features = false`,
`handshake` feature のみ）・`futures-util`（`sink`/`std` feature のみ）依存、
workspace 内クレートへの依存は `bf-http` のみ）を追加。`tokio-tungstenite` の
`handshake` feature は推移的に `tungstenite`（`sha1`/`data-encoding`/`http`/
`httparse`/`rand` 等）を引き込むため、workspace 全体の依存クレート数（本測定は
`crates/plugin-*` を含む全 workspace メンバーをそれぞれ自身の既定 feature で
計測する方式であり、`crates/core` 単体の pay-for-what-you-use 遵守を測るもの
ではない点に注意）が増加している。

`crates/core` 側の pay-for-what-you-use は個別に確認済み: `cargo tree -p
backend-framework-core`（既定構成）に `bf-plugin-websocket`・`tokio-tungstenite`
は一切現れず、`cargo tree -p backend-framework-core --features websocket` で
初めて出現する（`docs/design/plugin-boundary.md` 6 節）。TLS 系（`native-tls`・
`rustls`）・`connect`（クライアント接続関数群）feature は無効のままであり、
これらの重い依存は一切増えていない。

計測コマンド: `bash scripts/dep-impact.sh`

### 依存クレート数（workspace メンバー除外・重複バージョンは union で 1 件として計上）

| feature 構成 | 依存クレート数（直前エントリ差分） |
|---|---|
| --no-default-features | 73（+19） |
| default | 73（+19） |
| --all-features | 73（+19） |

feature を持つ workspace メンバーが `crates/core`（`webrtc-proxy`/`websocket`）
のみのため、`crates/plugin-websocket` 自身は常に自身の既定 feature（`handshake`
のみ、`default-features = false`）でツリーに現れ、3 構成とも同一値になる。
直前エントリ（54）比 +19 は `tokio-tungstenite`/`tungstenite` とその推移依存
（`sha1`・`digest`・`data-encoding`・`http`・`httparse`・`rand` 系等）。

### リリースバイナリサイズ

| bin | サイズ (bytes) | 直前エントリ差分 |
|---|---|---|
| axum-ref | 1356936 | 0（`axum-ref` は `bf-plugin-websocket` に依存しない） |

### unsafe 件数

`cargo-geiger` 実行が環境要因で失敗（`scripts/unsafe-triage.sh` の対象は
workspace 自身のコードのみで、`crates/plugin-websocket` は 0 件、
`scripts/unsafe-baseline.json` の既定値と一致。依存 crate 側の unsafe 増減は
本記録の対象外）。

## 2026-07-17 — `crates/plugin-openapi` 追加（#30、TASK-3.1）

`bf-plugin-openapi`（`utoipa = "5"`（default features）・`serde`（derive）依存、
workspace 内クレートへの依存なし）を追加。本クレートは他クレートから参照されない
独立プラグイン境界であり、`utoipa` 系依存は本クレート自身の構成にのみ現れる
（`.claude/rules/pay-for-what-you-use.md`）。`axum-ref` の bin サイズは workspace
全体のビルドグラフ変化の副作用でわずかに変動しているが、`bf-plugin-openapi` を
参照していないため実質的な影響はない。

計測コマンド: `bash scripts/dep-impact.sh`

### 依存クレート数（workspace メンバー除外・重複バージョンは union で 1 件として計上）

| feature 構成 | 依存クレート数（ベースライン差分） |
|---|---|
| --no-default-features | 54（+5） |
| default | 54（+5） |
| --all-features | 54（+5） |

feature を持つクレートが存在しないため 3 構成とも同一値。差分 5 件は
`utoipa` / `utoipa-gen` / `syn`（version 差分により union 上は別件計上され得る）等、
`bf-plugin-openapi` の直接・推移依存。

### リリースバイナリサイズ

| bin | サイズ (bytes) | ベースライン差分 |
|---|---|---|
| axum-ref | 1356936 | -1360（ビルド環境差によるノイズ。axum-ref は bf-plugin-openapi に依存しない） |

### unsafe 件数

`cargo-geiger` 未導入のため未計測（`scripts/unsafe-triage.sh` によるテキストベース
走査では `bf-plugin-openapi` の unsafe 件数は 0、`scripts/unsafe-baseline.json` の
既定値と一致）。

## 2026-07-16 — ベースライン（#17、TASK-15.2）

`crates/plugin-*` が未着手（TASK-2.1 以降）の現行 workspace（`crates/core` /
`crates/http` / `crates/axum-ref` のみ）で計測したベースライン。以降のプラグイン追加 PR は
このベースラインとの差分を確認する。

計測コマンド: `bash scripts/dep-impact.sh`

### 依存クレート数（workspace メンバー除外）

| feature 構成 | 依存クレート数 |
|---|---|
| --no-default-features | 49 |
| default | 49 |
| --all-features | 49 |

feature を持つクレートが存在しないため 3 構成とも同一値。

### リリースバイナリサイズ

| bin | サイズ (bytes) |
|---|---|
| axum-ref | 1358296 |

`crates/core` は現時点でライブラリのみ（実行可能 bin なし。TASK-1.4-2 / TASK-1.5 以降で
追加予定）。

### unsafe 件数

`cargo-geiger` 未導入のため未計測（導入後に追記する）。
