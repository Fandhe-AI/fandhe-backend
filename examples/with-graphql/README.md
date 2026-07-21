# fandhe-backend-example-with-graphql

`fandhe-backend` の GraphQL プラグイン（`graphql` feature）の配線だけを見せる
最小サンプルです。`crates/core/examples/graphql_nfr6.rs` を土台に、独立して
`cargo run` できる standalone crate として切り出しています
（[`examples/README.md`](../README.md) 参照）。

## 何を見せるサンプルか

- `Server::graphql(GraphQlConfig)` へのスキーマ登録
- 登録済みスキーマに対する `POST /graphql` の最小クエリ実行（`hello` / `echo` の
  2 フィールド、`echo` は variables 実演用の引数付き）
- 未登録時のフォールスルー（GraphQL はパスインターセプト型プラグインであり
  `Router` の責務外。`Router` には無関係な `GET /` のみを配線し、GraphQL 側の
  登録有無に関わらず動作することを示す）
- クエリ深さ・複雑度制限（`Schema::limit_depth` / `Schema::limit_complexity`）を
  スキーマ登録側で明示設定する DoS 対策の実演（`GraphQlConfig::new` の doc・
  `.claude/rules/security.md` 参照。本クレートは既定値を提供しないため、
  利用者アプリ側で必ず設定すること）

GraphQL 以外の feature（cors / compression / static / openapi 等）は焦点外のため
有効化していません（pay-for-what-you-use、複数 feature を組み合わせた実運用形の
雛形は [`templates/app/`](../../templates/app/) を参照してください）。

## 起動方法

```bash
cd examples/with-graphql
cargo run
```

既定で `127.0.0.1:3000` に bind します（`PORT` 環境変数で上書き可能）。

## 検証 curl 例

```bash
# クエリ実行（{"data":{"hello":"world"}} を確認）
curl -s -X POST http://127.0.0.1:3000/graphql -d '{"query":"{ hello }"}'

# variables 付きクエリ実行（{"data":{"echo":"hi"}} を確認）
curl -s -X POST http://127.0.0.1:3000/graphql \
  -d '{"query":"query($v: String!) { echo(value: $v) }","variables":{"v":"hi"}}'

# 不正 body（400 を確認）
curl -si -X POST http://127.0.0.1:3000/graphql -d 'not json'

# 無関係パス（Router の応答を確認、GraphQL インターセプトが波及しないこと）
curl -s http://127.0.0.1:3000/
```

## セキュリティ上の注意

- `Schema::limit_depth` / `Schema::limit_complexity` はスキーマ登録者（利用者
  アプリ）の責務です。本サンプルでは実演のため `limit_depth(8)` /
  `limit_complexity(64)` を設定していますが、自身のスキーマ構成に応じて
  上限を見直してください。
- introspection は `async-graphql` の既定で有効なままにしています。開発サンプル
  用途のためですが、非開発環境で公開する場合は `Schema::disable_introspection`
  の追加を検討してください。

## 完了条件チェック

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
