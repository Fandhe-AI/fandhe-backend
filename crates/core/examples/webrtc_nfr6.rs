//! TASK-8.4（#29）NFR-6 計測専用サーバ。
//!
//! `webrtc` feature 有効時に `Server::webrtc` へ [`WebRtcConfig`] を登録した構成で
//! `examples/minimal.rs` と同一の `GET /`（`bf_routes::Router`）を提供する。
//! `docs/acceptance/req8-webrtc-attack-surface.md`（NFR-6）が、本 example と
//! `examples/minimal.rs`（`webrtc` feature 無効のベースライン）へそれぞれ無関係パス
//! （`/`）へ負荷をかけ、RPS・p95 の比が誤差範囲（100.3〜100.8% 相当）に収まることを
//! 検証するために使う。production 配線（`Server::webrtc` の呼び出し判断自体）には
//! 触れず、計測専用の example として追加する（TASK-8.4 は test スコープ、
//! production コード変更を含まない）。
//!
//! `plugin::try_intercept` は `webrtc` feature 有効時、`POST /rtc/offer` 宛て
//! リクエストのみをパス完全一致で捕捉し、それ以外（本計測対象の `GET /`）は
//! 1 回のパス比較のみで `Handler::handle` へフォールスルーする
//! （`crates/core/src/plugin.rs` の doc を参照）。
//!
//! 動作確認手順:
//! ```text
//! $ cargo run --release --example webrtc_nfr6 -p backend-framework-core --features webrtc
//! $ curl -v http://127.0.0.1:3002/            # 200 応答（無関係パス）
//! $ curl -v -X POST http://127.0.0.1:3002/rtc/offer -d '...'  # WebRTC シグナリング
//! ```

use backend_framework_core::Server;
use bf_http::response::Response;
use bf_plugin_webrtc::WebRtcConfig;
use bf_routes::Router;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let router = Router::new().route("GET", "/", |_head, _body| {
        Response::new(200, b"backend-framework: webrtc nfr6 example\n".to_vec())
    });

    let server = Server::new().handler(router).webrtc(WebRtcConfig::new());
    let bound = server.bind("127.0.0.1:3002").await?;
    println!("listening on http://{}", bound.local_addr()?);
    bound.run().await
}
