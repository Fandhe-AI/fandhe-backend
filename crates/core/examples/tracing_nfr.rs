//! TASK-10.4（#59）NFR 計測専用サーバ（サンプリング適用後性能再検証）。
//!
//! `tracing` feature 有効時に `init_tracing`（非同期・バッファ済み I/O）+
//! `Server::tracing(config)`（`TracingMiddleware` 登録）を組んだ構成で、
//! `examples/minimal.rs`（TASK-10.4 でベースライン側にも追加した）と同一の
//! `GET /`・`GET /health` を提供する。`benches/tracing-nfr-bench.sh` が本 example と
//! `examples/minimal.rs`（`tracing` feature 無効のベースライン）へそれぞれ
//! `GET /health` へ負荷をかけ、TASK-10.1〜10.3（決定的サンプリング・イベント統合・
//! 高頻度パス除外）を適用した構成での RPS・p95 影響を実測するために使う。
//! production 配線（`Server::tracing` の呼び出し判断自体）には触れず、計測専用の
//! example として追加する（TASK-10.4 は test スコープ、production コード変更を
//! 含まない。`examples/graphql_nfr6.rs`＝TASK-5.2／#53、`examples/webrtc_nfr6.rs`＝
//! TASK-8.4／#29 と同型のパターン）。
//!
//! 計測シナリオは環境変数で切り替える（`benches/tracing-nfr-bench.sh` が指定する）:
//! - `SAMPLE_INTERVAL`: `TracingConfig::sample_interval`。既定 `100`
//!   （`TracingConfig::default()` と同値）。数値でない・`0` の場合は既定へ
//!   フォールバックする（不正な env 入力の安全側処理、`.claude/rules/security.md`）
//! - `EXCLUDE_HEALTH`: `1`（既定）で `/health` を `TracingConfig::exclude_path` に
//!   登録し TASK-10.3 の除外機構を適用する。`0` で除外なし（サンプリングのみ）の
//!   参考シナリオに切り替える。`1`/`0` 以外は既定（`1`）へフォールバックする
//!
//! 動作確認手順:
//! ```text
//! $ cargo run --release --example tracing_nfr -p backend-framework-core --features tracing
//! $ curl -v http://127.0.0.1:3006/            # 200 応答（無関係パス）
//! $ curl -v http://127.0.0.1:3006/health       # 200 応答（計測対象パス）
//! ```

use std::num::NonZeroU64;

use backend_framework_core::Server;
use bf_http::response::Response;
use bf_plugin_tracing::{TracingConfig, TracingOutput, init_tracing};
use bf_routes::Router;

/// `SAMPLE_INTERVAL` env を読み、不正値（非数値・`0`）は
/// `TracingConfig::default()` と同じ既定（`100`）へフォールバックする。
///
/// 計測シナリオ切替のためだけの最小パースであり、本番の設定入力経路ではない
/// （production 配線には env 由来の値を渡さない、実装計画 3 節）。
fn sample_interval_from_env() -> NonZeroU64 {
    const DEFAULT: u64 = 100;
    std::env::var("SAMPLE_INTERVAL")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(NonZeroU64::new)
        .unwrap_or_else(|| NonZeroU64::new(DEFAULT).expect("DEFAULT は非ゼロ定数"))
}

/// `EXCLUDE_HEALTH` env を読み、`/health` を除外パスに登録するか判定する。
/// `1` のみを真とし、それ以外（`0`・未設定・不正値）は既定の `true` を含め
/// 明示的に判定する（フェイルセーフではなく計測シナリオの明示切替のため、
/// 既定は「除外あり」＝TASK-10.3 適用後の受け入れ判定対象シナリオ）。
fn exclude_health_from_env() -> bool {
    !matches!(std::env::var("EXCLUDE_HEALTH").ok().as_deref(), Some("0"))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    // WorkerGuard はプロセス終了まで保持する契約（bf_plugin_tracing::init_tracing
    // の doc を参照）。ローカル変数として main のスコープ終了まで生かす。
    let _guard = init_tracing(TracingOutput::Stdout);

    let mut config = TracingConfig::new(sample_interval_from_env());
    if exclude_health_from_env() {
        config = config.exclude_path("/health");
    }

    let router = Router::new()
        .route("GET", "/", |_head, _body| {
            Response::new(200, b"backend-framework: tracing nfr example\n".to_vec())
        })
        .route("GET", "/health", |_head, _body| {
            Response::new(200, b"ok\n".to_vec())
        });

    let server = Server::new().handler(router).tracing(config);
    let bound = server.bind("127.0.0.1:3006").await?;
    println!("listening on http://{}", bound.local_addr()?);
    bound.run().await
}
