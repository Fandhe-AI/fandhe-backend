# チュートリアル

最小サーバから始め、拡張点（`Middleware`）の実装、feature 有効化までを段階的に学びます。
各段の裏取りは既存 example・doc test へのリンクで行います（コード全文の複製はしません。
[`README.md`](./README.md) の原則）。

## 1. 最小サーバ

まず [`getting-started.md`](./getting-started.md) の手順で `examples/minimal.rs` を
動かし、`Server` + `Router` の基本構成を確認してください。

`fandhe_backend_core::lib.rs` のクレート doc にも、`cargo test --doc` で検証される
クイックスタートの doc test があります。ソースを直接読みたい場合は
`crates/core/src/lib.rs` を参照してください。

```bash
cargo test --doc -p fandhe-backend-core
```

## 2. 拡張点を実装する: `Middleware`

コアは 3 種の拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）を公開します。
ここでは最も単純な `Middleware`（リクエスト数を数えるだけの実装）を例に、
拡張点の実装パターンを確認します。

`Middleware` trait の完全な実装例（doc test として `cargo test` で検証されます）は
`crates/core/src/extension.rs` の `Middleware` trait doc comment にあります。要点は次のとおりです。

```rust,ignore
use fandhe_backend_core::Middleware;
use fandhe_backend_http::request::RequestHead;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct CountingMiddleware {
    requests: AtomicUsize,
}

impl Middleware for CountingMiddleware {
    fn name(&self) -> &'static str {
        "counting-middleware"
    }

    fn on_request(&self, _head: &RequestHead) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    fn on_response(&self, _head: &RequestHead, _elapsed: Duration) {}
}
```

`Server::middleware` へ登録すると、コアのリクエストループから `on_request` /
`on_response` が呼ばれます。

```rust,ignore
let server = Server::new()
    .handler(router)
    .middleware(CountingMiddleware { requests: AtomicUsize::new(0) });
```

> **注意（並行性）**: `Middleware` 実装は同期ブロッキング I/O を行わないでください。
> 非同期チャネルへの送信・別タスクでの I/O 実行に留めます
> （[`.claude/rules/coding-rust.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/coding-rust.md)、実装パターンの
> 詳細根拠は [`AGENTS.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/AGENTS.md) を参照）。実際のプロダクション実装は
> `crates/plugin-tracing` の `TracingMiddleware`（非同期・バッファ済み I/O）を参照して
> ください。

## 3. feature を有効化する: websocket エコー

拡張点の実装パターンを踏まえ、実際のプラグイン（`websocket` feature）を有効化して
動かしてみます。`UpgradeHandler` 拡張点を通じて WebSocket ハンドシェイクへ委譲する
実装は `fandhe-backend-plugin-websocket` が提供します。

```bash
cargo run --release --example ws_echo -p fandhe-backend-core --features websocket
curl -v http://127.0.0.1:3007/health   # 200 応答
```

WebSocket クライアント（`wscat` 等）で `ws://127.0.0.1:3007/ws` に接続すると、
送信した内容がそのままエコーされます。

feature を無効化してビルドし直すと（`cargo build -p fandhe-backend-core`）、
`fandhe-backend-plugin-websocket` への依存が `cargo tree` から消えることを確認できます
（pay-for-what-you-use、[`feature-samples.md`](./feature-samples.md) の検証手順を参照）。

```bash
cargo tree -p fandhe-backend-core            # websocket 依存が出ない
cargo tree -p fandhe-backend-core --features websocket  # websocket 依存が出る
```

## 次のステップ

- 他 feature（graphql / webrtc 系 / tracing / openapi / hub-wiring）の最小サンプルは
  [`feature-samples.md`](./feature-samples.md) を参照してください
- プラグイン境界の設計判断（なぜこのパターンを採用したか）は
  [`docs/design/plugin-boundary.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/plugin-boundary.md) を参照してください
