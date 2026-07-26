# with-graphql

`fandhe-backend` の GraphQL プラグイン（`graphql` feature）の配線だけを見せる
最小サンプルです。独立して `cargo run` できる standalone crate として
`examples/with-graphql/` に切り出されています。

## 何を見せるサンプルか

- `Server::graphql(GraphQlConfig)` へのスキーマ登録
- 登録済みスキーマに対する `POST /graphql` の最小クエリ実行（`hello` / `echo` の
  2 フィールド、`echo` は variables 実演用の引数付き）
- 未登録時のフォールスルー（GraphQL はパスインターセプト型プラグインであり
  `Router` の責務外であることの実演）
- クエリ深さ・複雑度制限（`Schema::limit_depth` / `Schema::limit_complexity`）を
  スキーマ登録側で明示設定する DoS 対策の実演

GraphQL 以外の feature（cors / compression / static / openapi 等）は焦点外のため
有効化していません（pay-for-what-you-use。複数 feature を組み合わせた実運用形の
雛形は [templates/app](./templates-app.md) を参照してください）。

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
```

## セキュリティ上の注意

- `Schema::limit_depth` / `Schema::limit_complexity` はスキーマ登録者（利用者
  アプリ）の責務です。自身のスキーマ構成に応じて上限を見直してください。
- introspection は `async-graphql` の既定で有効なままです。非開発環境で公開する
  場合は `Schema::disable_introspection` の追加を検討してください。

## GitHub 上の実体

コード全文・詳細な README は
[`examples/with-graphql/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/examples/with-graphql)
を参照してください。

[サンプル集に戻る](../examples.md)
