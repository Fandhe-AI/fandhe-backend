# fandhe-backend-example-with-interceptor

`fandhe-backend` のコア拡張点 `Interceptor`（リダイレクト・レスポンス改変、
イシュー #420）だけを見せる最小サンプルです。独立して `cargo run` できる
standalone crate として切り出しています（[`examples/README.md`](../README.md)
参照）。

## 何を見せるサンプルか

- `TrailingSlashRedirect`（`Interceptor::intercept` のみ実装）:
  末尾スラッシュ付きパスを 301 で `Location` へリダイレクトする
  （`?query` があれば保存する）
- `SecurityHeaders`（`Interceptor::map_response` のみ実装）:
  全応答へ `X-Content-Type-Options: nosniff` / `X-Frame-Options: DENY` を
  付与する（`intercept` 応答・`Handler` のフォールバック応答（404 等）の
  両方に及ぶ）
- `Interceptor` は **feature ゲート不要の純コア拡張点**であること
  （`Cargo.toml` の `fandhe-backend-core` 依存に `features` を一切
  指定していない点に注目。pay-for-what-you-use）

`Interceptor` 以外の feature（cors / compression / static / openapi 等）は
焦点外のため有効化していません。

## 起動方法

```bash
cd examples/with-interceptor
cargo run
```

既定で `127.0.0.1:3000` に bind します（`PORT` 環境変数で上書き可能）。

## 動作確認手順

```bash
# 通常応答（200 + セキュリティヘッダを確認）
curl -si http://127.0.0.1:3000/hello

# 末尾スラッシュの正規化（301 + Location: /hello、セキュリティヘッダも付与される）
curl -si http://127.0.0.1:3000/hello/

# クエリ付きの正規化（Location: /hello?q=1 を確認）
curl -si "http://127.0.0.1:3000/hello/?q=1"

# 未登録パス（404 + セキュリティヘッダを確認。map_response が
# Handler のフォールバック応答にも及ぶことの実演）
curl -si http://127.0.0.1:3000/missing
```

## 評価順序（要点）

`Interceptor::intercept` は `RequestGate`（フェイルクローズ既定拒否）・
`UpgradeHandler` の**後**、`Handler` の**前**に評価されます。
`RequestGate` の拒否応答を `Interceptor` で迂回することはできません。
`map_response` は最終応答確定後・`finalize_response`（CORS → 圧縮）**前**に
登録順で逐次適用されます。詳細は `crates/core/src/interceptor.rs` の
モジュール doc を参照してください。

## 完了条件チェック

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
