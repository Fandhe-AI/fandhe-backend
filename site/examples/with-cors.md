# with-cors

`fandhe-backend` の CORS プラグイン（`cors` feature）の 2 層配線だけを見せる
最小サンプルです。独立して `cargo run` できる standalone crate として
`examples/with-cors/` に切り出されています。

## 何を見せるサンプルか

- `GET`/`POST /todos` の最小 ToDo API（`Arc<RwLock<Vec<Todo>>>` 共有状態）
- CORS の 2 層配線:
  1. `Router::options_fallback` へ `fandhe_backend_core::plugin_cors::preflight_response` を
     配線し、プリフライト（`OPTIONS` + `Origin` + `Access-Control-Request-Method`）を
     完結させる
  2. `Server::cors(CorsConfig)` を登録し、実リクエストへ
     `Access-Control-Allow-Origin` 等のヘッダを付与する

CORS 以外の feature（compression / static / openapi 等）は焦点外のため有効化して
いません（pay-for-what-you-use。複数 feature を組み合わせた実運用形の雛形は
[templates/app](./templates-app.md) を参照してください）。

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
```

## GitHub 上の実体

コード全文・詳細な README は
[`examples/with-cors/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/examples/with-cors)
を参照してください。

[サンプル集に戻る](../examples.md)
