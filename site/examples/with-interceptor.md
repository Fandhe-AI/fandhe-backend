# with-interceptor

`fandhe-backend` のコア拡張点 `Interceptor`（リダイレクト・レスポンス改変）
だけを見せる最小サンプルです。独立して `cargo run` できる standalone crate
として `examples/with-interceptor/` に切り出されています。

## 何を見せるサンプルか

- `TrailingSlashRedirect`（`Interceptor::intercept` のみ実装）: 末尾スラッシュ
  付きパスを 301 で `Location` へリダイレクトする（`?query` があれば保存する）
- `SecurityHeaders`（`Interceptor::map_response` のみ実装）: 全応答へ
  `X-Content-Type-Options: nosniff` / `X-Frame-Options: DENY` を付与する
  （`intercept` 応答・`Handler` のフォールバック応答（404 等）の両方に及ぶ）
- `Interceptor` が **feature ゲート不要の純コア拡張点**であること
  （外部依存ゼロ、pay-for-what-you-use）

`Interceptor` 以外の feature（cors / compression / static / openapi 等）は焦点外
のため有効化していません（複数 feature を組み合わせた実運用形の雛形は
[templates/app](./templates-app.md) を参照してください）。

## 起動方法

```bash
cd examples/with-interceptor
cargo run
```

既定で `127.0.0.1:3000` に bind します（`PORT` 環境変数で上書き可能）。

## 検証 curl 例

```bash
# 通常応答（200 + セキュリティヘッダを確認）
curl -si http://127.0.0.1:3000/hello

# 末尾スラッシュの正規化（301 + Location: /hello を確認）
curl -si http://127.0.0.1:3000/hello/
```

## GitHub 上の実体

コード全文・詳細な README は
[`examples/with-interceptor/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/examples/with-interceptor)
を参照してください。

[サンプル集に戻る](../examples.md)
