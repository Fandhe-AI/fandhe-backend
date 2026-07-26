# サンプル集

fandhe-backend には目的の異なる 3 種類のサンプル置き場があります。
本ページでは、そのうち独立して `cargo run` できる 4 つのサンプル
（`examples/with-*` 3 種 + `templates/app`）への入口をまとめます。

## 3 種のサンプル置き場の使い分け

| 置き場 | 目的 | 読者 |
|--------|------|------|
| `crates/core/examples/*` | feature 単体の実装パターンを示す最小 example（`cargo run --example <name>`）。[feature 構成別サンプル](/fandhe-backend/guides/feature-samples/)から導線を張る「正」のサンプルソース | フレームワーク実装を読み解く開発者 |
| `examples/<with-feature>/` | 独立した `cargo run` 可能なプロジェクトとして、1 機能の配線を Next.js 流に切り出したサンプル | フレームワークを新規プロジェクトへ導入する利用者 |
| `templates/app/` | 複数 feature を組み合わせた実運用形の雛形（`cargo new` 相当の出発点） | フレームワークで新規プロジェクトを始める利用者 |

各サンプルは他のサンプル・root workspace から独立し、単体で `cargo run` できます
（root workspace 非メンバーの standalone workspace + `publish = false`）。

## 収録サンプル

- [with-cors](./examples/with-cors.md) — CORS の 2 層配線（`Router::options_fallback` +
  `Server::cors`）だけを見せる最小サンプル
- [with-graphql](./examples/with-graphql.md) — GraphQL の配線（`Server::graphql` への
  スキーマ登録 + `POST /graphql` 最小クエリ実行）だけを見せる最小サンプル
- [with-websocket](./examples/with-websocket.md) — ユーザー定義 WebSocket
  メッセージハンドラ（`WebSocketConfig::with_handler`）だけを見せる最小サンプル
- [templates/app](./examples/templates-app.md) — cors / compression / static /
  openapi を組み合わせた実運用形 ToDo API 雛形

GitHub 上の実体は
[`examples/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/examples) と
[`templates/app/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/templates/app)
を参照してください。
