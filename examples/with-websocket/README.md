# fandhe-backend-example-with-websocket

`fandhe-backend` の WebSocket プラグイン（`websocket` feature）における
`WebSocketConfig::with_handler` によるユーザー定義メッセージハンドラ配線だけを
見せる最小サンプルです。独立して `cargo run` できる standalone crate として
切り出しています（[`examples/README.md`](../README.md) 参照）。

## 何を見せるサンプルか

- `WebSocketConfig::with_handler` によるユーザー定義 `WsMessageHandler`
  （`PingPongEchoHandler`）の登録:
  - Text `"ping"` → `"pong"`（固定の独自応答）
  - Text `"bye"` → サーバ起点の Close（`WsOutcome::Close`）
  - それ以外の Text/Binary → そのままエコー
- `GET /` の通常 HTTP ルートと WebSocket（既定パス `/ws`）が同一 `Server` に
  共存できること
- `WebSocketConfig` の DoS 安全側の既定値（`max_message_size` 1 MiB /
  `max_frame_size` 256 KiB / アイドルタイムアウト 60 秒）はそのまま維持し、
  `without_idle_timeout` 等の保護解除 API は使用していません

WebSocket 以外の feature（cors / compression / static / openapi 等）は焦点外の
ため有効化していません（pay-for-what-you-use）。

## 起動方法

```bash
cd examples/with-websocket
cargo run
```

既定で `127.0.0.1:3000` に bind します（`PORT` 環境変数で上書き可能）。

## 接続例

[websocat](https://github.com/vi/websocat) がある環境:

```bash
# 案内メッセージの確認
curl -s http://127.0.0.1:3000/

# WebSocket 接続（既定パス /ws）
websocat ws://127.0.0.1:3000/ws
ping     # -> pong
hello    # -> hello（エコー）
bye      # -> サーバから Close
```

websocat が使えない環境では、`cargo test` の E2E テスト
（`handshake_and_message_roundtrip_over_real_tcp`）が実 TCP 上でハンドシェイク
（101 応答）とメッセージ往復（ping→pong / hello→エコー / bye→Close）を
自動検証します。

## 完了条件チェック

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
