//! TASK-4.3（#24）10,000 同時 WebSocket 接続負荷試験・RSS 再計測用サーバ。
//!
//! `benches/bench-ws-load.sh` が `crates/axum-ref`（`ws` feature 有効）と対で
//! 起動し、同一の負荷（`crates/ws-load-client`）を掛けて接続あたり RSS 増分を
//! 比較する計測専用 example。production 配線（`Server::websocket` を呼ぶか
//! 否かの判断自体）には触れず、`examples/tracing_nfr.rs`（TASK-10.4 / #59）と
//! 同型の「計測専用サーバ」パターンを踏襲する。
//!
//! `GET /health` は `benches/lib/common.sh` の `wait_for_health` が起動完了を
//! 検知するために使う（`examples/minimal.rs` と同一パス）。`GET /ws`
//! （既定パス、`bf_plugin_websocket::WebSocketConfig::default()`）が
//! エコーセッション（`crates/plugin-websocket/src/session.rs` の
//! `run_echo_session`）を提供する。
//!
//! 環境変数（既定値は 10,000 同時接続 + 監視用ヘルスチェック接続の余裕を
//! 見込んだ値）:
//! - `BIND_ADDR`: 待受アドレス（既定 `127.0.0.1:3007`）。ループバック限定を
//!   崩さず外部公開したい場合のみ呼び出し側の責任で明示指定する
//!   （`.claude/rules/security.md` の攻撃表面最小化）
//! - `MAX_CONNECTIONS`: 同時接続数上限（既定 10,100）。`DEFAULT_MAX_CONNECTIONS`
//!   （`crates/core/src/server.rs`、既定 10,000）は WS 10,000 本ちょうどで
//!   ヘルスチェック接続の余裕がないため、計測専用に明示引き上げる
//!   （production の既定値そのものは変更しない）
//!
//! 動作確認手順:
//! ```text
//! $ cargo run --release --example ws_echo -p backend-framework-core --features websocket
//! $ curl -v http://127.0.0.1:3007/health   # 200 応答
//! ```

use backend_framework_core::Server;
use bf_http::response::Response;
use bf_plugin_websocket::WebSocketConfig;
use bf_routes::Router;

/// `MAX_CONNECTIONS` env を読み、不正値（非数値・`0`）は既定 10,100
/// （10,000 WS 接続 + 監視用ヘルスチェック接続の余裕）へフォールバックする。
/// 計測シナリオ切替のためだけの最小パースであり、production の設定入力
/// 経路ではない（`examples/tracing_nfr.rs` の `sample_interval_from_env` と
/// 同一方針）。
fn max_connections_from_env() -> usize {
    const DEFAULT: usize = 10_100;
    std::env::var("MAX_CONNECTIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> std::io::Result<()> {
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3007".to_string());

    let router = Router::new().route("GET", "/health", |_head, _body| {
        Response::new(200, b"ok\n".to_vec())
    });

    let server = Server::new()
        .handler(router)
        .max_connections(max_connections_from_env())
        .websocket(WebSocketConfig::default());
    let bound = server.bind(&bind_addr).await?;
    println!("listening on http://{}", bound.local_addr()?);
    bound.run().await
}
