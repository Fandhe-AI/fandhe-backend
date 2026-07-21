# fandhe-backend-template-app

`fandhe-backend` の feature 一式（`cors` / `compression` / `static` / `openapi`）を
組み合わせた ToDo API テンプレート。CRUD 本体は
`crates/core/examples/todo_async.rs` を土台にし、実運用アプリで複数 feature を
同時配線する際の雛形として `templates/app/` に独立させている
（root workspace から独立した standalone crate、`Cargo.toml` の doc を参照）。

## 何を見せるテンプレートか

- `route_async` / `route_param_async` による ToDo CRUD（`Arc<RwLock<...>>` 共有状態）
- CORS の 2 層配線（`Router::options_fallback` + `Server::cors`）
- gzip レスポンス圧縮（`Server::compression`）
- 静的ファイル配信（`Server::static_files`、`static/index.html` の素の HTML+JS UI）
- OpenAPI スキーマ配信（`Server::openapi_with`、手書き `openapi.json`）
- 404 fallback（`Router::fallback`、JSON エラーボディ）
- graceful shutdown（`BoundServer::run_until` + `Server::shutdown_grace_period`）

## 起動方法

```bash
cd templates/app
cargo run
```

既定で `127.0.0.1:3000` に bind する（`PORT` 環境変数で上書き可能）。
ブラウザで <http://127.0.0.1:3000/index.html> を開くと ToDo UI が表示される。

## エンドポイント一覧

| メソッド | パス | 説明 |
|---------|------|------|
| `GET`    | `/todos`      | ToDo 一覧取得 |
| `POST`   | `/todos`      | ToDo 作成（`{"title": "..."}`） |
| `GET`    | `/todos/{id}` | ToDo 単体取得 |
| `PATCH`  | `/todos/{id}` | ToDo 更新（`{"title"?: "...", "done"?: bool}`） |
| `DELETE` | `/todos/{id}` | ToDo 削除 |
| `GET`    | `/index.html` | ToDo UI（静的配信） |
| `GET`    | `/openapi.json` | OpenAPI 3.0 スキーマ |
| `OPTIONS`| 任意の登録パス | CORS プリフライト |

## 検証 curl 例

```bash
# 作成
curl -s -X POST http://127.0.0.1:3000/todos -d '{"title":"buy milk"}'

# 一覧
curl -s http://127.0.0.1:3000/todos

# 更新
curl -s -X PATCH http://127.0.0.1:3000/todos/1 -d '{"done":true}'

# 削除
curl -s -X DELETE http://127.0.0.1:3000/todos/1

# OpenAPI スキーマ
curl -s http://127.0.0.1:3000/openapi.json

# 未登録パス（JSON 404）
curl -si http://127.0.0.1:3000/nope

# CORS プリフライト（開発オリジン http://localhost:5173 のみ許可）
curl -si -X OPTIONS http://127.0.0.1:3000/todos \
  -H 'Origin: http://localhost:5173' \
  -H 'Access-Control-Request-Method: POST'
```

## 静的ファイル配信の mount について

`Server::static_files` の mount は `"/"` ではなく `"/index.html"`（配信対象
ファイルそのもののパス）にしている。`try_intercept`（静的配信を含むパス
インターセプト型プラグイン）は `Router` のディスパッチより先に評価される
ため、mount `"/"` はすべての `GET` パスに一致し、`GET /todos` 等の CRUD API
を静的配信が横取りしてしまう（`crates/plugin-static/src/lib.rs` の
`strip_mount` doc を参照）。本テンプレートは配信対象が `index.html` 1
ファイルのみのため、mount をファイルパスそのものにすることで CRUD API と
競合させずに済ませている。複数の静的アセットを配信する場合は
`crates/core/examples/static_demo.rs` のようにプレフィックス（例
`"/static"`）を mount にすること。

## 完了条件チェック

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
