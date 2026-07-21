# examples/

Next.js の [`examples/`](https://github.com/vercel/next.js/tree/canary/examples)
（「1 機能 = 1 独立サンプル + README」方式）を参考にした、fandhe-backend の
トップレベルサンプル集です。各サンプルは他のサンプル・root workspace から
独立し、単体で `cargo run` できます（`templates/app/` と同じ standalone
workspace 構成、下記「構成の流儀」節を参照）。

命名は Next.js 流に `with-<feature>`（例: `with-cors`）とし、1 サンプルにつき
焦点を絞った 1 機能のみを見せます。複数 feature を組み合わせた実運用形の
雛形は本ディレクトリではなく [`templates/app/`](../templates/app/) を参照してください。

## 収録サンプル

| ディレクトリ | 見せる機能 | 起動方法 |
|-------------|-----------|---------|
| [`with-cors/`](./with-cors/) | CORS の 2 層配線（`Router::options_fallback` + `Server::cors`） | `cd examples/with-cors && cargo run` |
| [`with-graphql/`](./with-graphql/) | GraphQL の配線（`Server::graphql` へのスキーマ登録 + `POST /graphql` 最小クエリ実行） | `cd examples/with-graphql && cargo run` |

## サンプルコードの重複回避方針

fandhe-backend には目的の異なる 3 種類のサンプル置き場があり、内容を重複させません
（[`docs/guide/README.md`](../docs/guide/README.md) の「サンプルコードの原則」と同一方針）。

| 置き場 | 目的 | 読者 |
|--------|------|------|
| `crates/core/examples/*` | feature 単体の実装パターンを示す最小 example（`cargo run --example <name>`）。`docs/guide/feature-samples.md` から導線を張る「正」のサンプルソース | フレームワーク実装を読み解く開発者 |
| `docs/guide/feature-samples.md` | feature ごとの有効化方法・実行手順の一覧。コード全文は複製せず `crates/core/examples/*` への導線のみを提供 | 利用者（ガイド読者） |
| `examples/<with-feature>/`（本ディレクトリ） | 独立した `cargo run` 可能なプロジェクトとして、1 機能の配線を Next.js 流に切り出したサンプル。`crates/core/examples/*` の対応する example を土台にしつつ standalone crate として複製 | フレームワークを新規プロジェクトへ導入する利用者 |
| `templates/app/` | 複数 feature を組み合わせた実運用形の雛形（`cargo new` 相当の出発点） | フレームワークで新規プロジェクトを始める利用者 |

`examples/with-cors/` は `crates/core/examples/cors_demo.rs` を土台にしていますが、
standalone crate（独立 `cargo run`・独自 README・独自テスト）として複製している点が
`crates/core/examples/` との違いです。両者に差分が生じた場合は
`crates/core/examples/cors_demo.rs` 側を正とし、本ディレクトリ側を追随させてください。

## 構成の流儀（`templates/app/` と共通）

各サンプルは `templates/app/Cargo.toml` と同じ流儀で構成します。

- root workspace（`crates/*` glob）のメンバーにしない。各サンプル自身の
  `Cargo.toml` に `[workspace] members = ["."]` を書き、standalone workspace として切り離す
- `publish = false`（crates.io には公開しない）
- 依存は `version = "0.1.0"` + `path = "../../crates/..."` の併記。リポジトリ内では
  path 参照で常に最新実装を検証し、リポジトリ外へコピーして使う場合は `path` を
  外して crates.io 版参照に切り替える（各 `Cargo.toml` のコメントを参照）
- 見せたい機能に必要な Cargo feature のみ有効化する（pay-for-what-you-use、
  [`.claude/rules/pay-for-what-you-use.md`](../.claude/rules/pay-for-what-you-use.md)）
