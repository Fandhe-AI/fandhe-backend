# ゲートの検証結果を WebSocket で使う

`RequestGate`（`TenantGate`・`JwtGate`・自作のゲート）が検証した値は、WebSocket の upgrade 要求にも
同じように付きます。ハンドラは次の 3 箇所から、読み取り専用で参照できます。

| 場所 | 読み方 |
|------|--------|
| ハンドシェイクのフック（101 の前） | `WsHandshakeContext::head().extensions()` |
| `WsMessageHandler::on_open` | `WsOpenContext::extensions()` |
| `on_message_with_ctx` / `on_close` | `WsConnContext::extensions()` |

ゲートが拒否した upgrade は 101 を返さず、フックにもハンドラにも届きません。ゲートを登録していない
サーバでは器は空で、クライアントが送ったヘッダが値になることはありません。続けて張った別の接続にも
値は現れません。

## 読み方

`TenantGate::verified` と `JwtGate::verified` は `&RequestHead` を取るので、WebSocket のコンテキストからは
呼べません。代わりに型で引きます。

- hub 共通配線: `ctx.extensions().get::<TenantVerified>()` から `.tenant().org_id()`
- `JwtGate`: `ctx.extensions().get::<GateVerified<E>>()` から `.claims()`。`E` はゲート登録時と同じ型で
  ないと `None` になります

`None` は拒否側に倒してください（fail-closed）。

> 以下のコードは hub-wiring・plugin-jwt の型を使うため、コンパイル検証の対象外の説明用コード
> （`rust,ignore`）です。API の呼び出し形は各 rustdoc の doc test が「正」です。

```rust,ignore
use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_hub_wiring::TenantVerified;
use fandhe_backend_plugin_websocket::handler::{WsMessageHandler, WsOpenContext};
use fandhe_backend_plugin_websocket::{WebSocketConfig, WsHandshakeContext};

struct MyHandler;

impl WsMessageHandler for MyHandler {
    fn name(&self) -> &'static str { "my-handler" }

    fn on_open(&self, ctx: WsOpenContext) {
        let Some(verified) = ctx.extensions().get::<TenantVerified>() else {
            // 値がなければ閉じる。close は async なので task で呼ぶ。
            let sender = ctx.sender().clone();
            tokio::spawn(async move { let _ = sender.close(1008, "unauthorized").await; });
            return;
        };
        let org_id = verified.tenant().org_id().to_string();
        // org_id を接続の状態として保持する、など。
        let _ = org_id;
    }

    // on_message ほかは省略
}

// 101 の前に拒否する書き方（推奨）。
let config = WebSocketConfig::default()
    .with_handshake_check(|ctx: &WsHandshakeContext<'_>| {
        if ctx.head().extensions().get::<TenantVerified>().is_none() {
            return Err(Response::empty(401));
        }
        Ok(())
    })
    .with_handler(MyHandler);

let server = Server::new().gate(tenant_gate).websocket(config);
```

## 既知の限界

- 値は WebSocket セッションが続く間ずっと保持されます。
- **JWT の `exp` はハンドシェイク時点で検証するだけで、セッション中には再検証しません。** 鍵の
  ローテーションや失効も、張ったあとのセッションには反映されません。
- **コアの `Server::max_connection_lifetime` は upgrade 後の委譲セッションには効きません。** HTTP の
  keep-alive ループだけが参照します。

### 緩和策

`on_open` で期限を読み、期限になったらサーバから閉じます。

- `JwtGate`: `GateVerified::claims().exp()`
- hub 共通配線: `TenantVerified::tenant().claims().exp()`

spawn した task で期限まで `sleep_until` し、`WsSender::close` を呼びます。あわせて
`WebSocketConfig::with_ping_interval` と `idle_timeout` を設定し、死んだ接続を残さないでください。

## 注意

- `Debug` には値が出ません（`RequestExtensions` は件数だけ、両コンテキストは内部情報を出しません）。
  クレームをログへ出さないでください。
- 設計の背景は
  [`gate-request-extensions.md` 14 節](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/gate-request-extensions.md)
  と
  [`ws-connection-context-and-close.md` 17 節](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/ws-connection-context-and-close.md)
  を参照してください。
- 実 TCP での動作は `crates/core/tests/ws_gate_extensions.rs` が確かめています。
