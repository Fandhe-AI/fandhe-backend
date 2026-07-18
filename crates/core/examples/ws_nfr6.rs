//! TASK-4.4（#25）NFR-6 計測専用サーバ。
//!
//! `websocket` feature 有効時に `Server::websocket` へ [`WebSocketConfig`] を登録した
//! 構成で `examples/minimal.rs` と同一の `GET /health`（`bf_routes::Router`）を提供する。
//! `docs/acceptance/req4-websocket.md`（REQ-4 受け入れ基準: 無関係パスへの RPS・
//! レイテンシ影響が誤差範囲内）が、本 example と `examples/minimal.rs`（`websocket`
//! feature 無効のベースライン）へそれぞれ無関係パス（`/health`）へ負荷をかけ、RPS・
//! p95 の比が誤差範囲に収まることを検証するために使う。production 配線
//! （`Server::websocket` の呼び出し判断自体）には触れず、計測専用の example として
//! 追加する（TASK-4.4 は test スコープ、production コード変更を含まない。
//! `examples/graphql_nfr6.rs`＝TASK-5.2／#53・`examples/webrtc_nfr6.rs`＝TASK-8.4／#29
//! と同型のパターン）。
//!
//! `examples/ws_echo.rs`（TASK-4.3 / #24、`#[tokio::main(flavor = "multi_thread")]`）を
//! NFR-6 比較にそのまま流用しない理由: NFR-6 比較対象のベースライン
//! `examples/minimal.rs` は `current_thread` ランタイムで動く。ランタイムのスレッド
//! 数が揃っていないと、計測される RPS 差が「`websocket` feature の実処理コスト」
//! ではなく「シングルスレッド vs マルチスレッド」というランタイム構成の違いに
//! 支配されてしまう（`ws_echo` を暫定的に NFR-6 比較へ流用した際の実測で、無関係
//! パスの RPS 比が baseline 比 約190% という説明のつかない値になり判明した。
//! `benches/reports/task-4.4-ws-latency.md` 参照）。本 example は `graphql_nfr6.rs`・
//! `webrtc_nfr6.rs`・`tracing_nfr.rs` と同じく `current_thread` に固定し、
//! ベースラインとランタイム構成を揃える。
//!
//! `plugin::try_intercept`（`UpgradeHandler` 拡張点、WebSocket は Upgrade 型プラグイン
//! 境界パターンの第 1 号）は `GET /ws`（既定パス）宛てリクエストのみを捕捉し、
//! それ以外（本計測対象の `GET /health`）は `Sec-WebSocket-*` ヘッダの有無を確認する
//! だけの軽量な判定でフォールスルーする（`crates/plugin-websocket/src/lib.rs` の doc
//! を参照）。
//!
//! 動作確認手順:
//! ```text
//! $ cargo run --release --example ws_nfr6 -p backend-framework-core --features websocket
//! $ curl -v http://127.0.0.1:3009/health   # 200 応答（無関係パス）
//! ```

use backend_framework_core::Server;
use bf_http::response::Response;
use bf_plugin_websocket::WebSocketConfig;
use bf_routes::Router;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let router = Router::new().route("GET", "/health", |_head, _body| {
        Response::new(200, b"ok\n".to_vec())
    });

    let server = Server::new()
        .handler(router)
        .websocket(WebSocketConfig::default());
    let bound = server.bind("127.0.0.1:3009").await?;
    println!("listening on http://{}", bound.local_addr()?);
    bound.run().await
}
