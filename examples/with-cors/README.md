# fandhe-backend-example-with-cors

`fandhe-backend` の CORS プラグイン（`cors` feature）の 2 層配線だけを見せる
最小サンプルです。`crates/core/examples/cors_demo.rs` を土台に、独立して
`cargo run` できる standalone crate として切り出しています
（[`examples/README.md`](../README.md) 参照）。

## 何を見せるサンプルか

- `GET`/`POST /todos` の最小 ToDo API（`Arc<RwLock<Vec<Todo>>>` 共有状態）
- CORS の 2 層配線:
  1. `Router::options_fallback` へ `fandhe_backend_plugin_cors::preflight_response` を配線し、
     プリフライト（`OPTIONS` + `Origin` + `Access-Control-Request-Method`）を完結させる
  2. `Server::cors(CorsConfig)` を登録し、実リクエストへ `Access-Control-Allow-Origin` 等の
     ヘッダを付与する

CORS 以外の feature（compression / static / openapi 等）は焦点外のため有効化していません
（pay-for-what-you-use、複数 feature を組み合わせた実運用形の雛形は
[`templates/app/`](../../templates/app/) を参照してください）。

## 起動方法

```bash
cd examples/with-cors
cargo run
```

既定で `127.0.0.1:3000` に bind します（`PORT` 環境変数で上書き可能）。

## 検証 curl 例

```bash
# プリフライト（204 + Access-Control-Allow-* を確認）
curl -si -X OPTIONS http://127.0.0.1:3000/todos \
  -H 'Origin: https://app.example.com' \
  -H 'Access-Control-Request-Method: POST'

# 実リクエスト（許可オリジン、Access-Control-Allow-Origin 付与を確認）
curl -si http://127.0.0.1:3000/todos -H 'Origin: https://app.example.com'

# 実リクエスト（不許可オリジン、Access-Control-Allow-Origin なしを確認）
curl -si http://127.0.0.1:3000/todos -H 'Origin: https://evil.example'

# ToDo 作成
curl -s -X POST http://127.0.0.1:3000/todos \
  -H 'Origin: https://app.example.com' -d '{"title":"buy milk"}'
```

## 完了条件チェック

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
