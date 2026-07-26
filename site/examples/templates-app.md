# templates/app

`fandhe-backend` の feature 一式（`cors` / `compression` / `static` / `openapi`）を
組み合わせた ToDo API テンプレートです。実運用アプリで複数 feature を同時配線
する際の雛形として `templates/app/` に独立しています（root workspace から独立
した standalone crate）。

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
ブラウザで `http://127.0.0.1:3000/index.html` を開くと ToDo UI が表示される。

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

# OpenAPI スキーマ
curl -s http://127.0.0.1:3000/openapi.json
```

## GitHub 上の実体

コード全文・詳細な README は
[`templates/app/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/templates/app)
を参照してください。

[サンプル集に戻る](../examples.md)
