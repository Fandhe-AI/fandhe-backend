//! `BoundServer::run_until` の利用例（graceful shutdown、イシュー #313）。
//!
//! `examples/minimal.rs`（REQ-1/NFR-1 性能ベンチのベースライン、
//! `docs/design/bench-scheduled-run.md` 等が参照する固定構成）はベースラインの
//! 挙動を変えないため既存の `run()` 呼び出しのまま維持し、本 example を
//! 新規追加して `run_until` の利用方法のみを示す。
//!
//! `BoundServer::run_until` はシャットダウンシグナル源をコアで扱わない
//! （`Server::shutdown_grace_period` の doc・`crates/core/Cargo.toml` の
//! `signal` feature コメントを参照）。本 example は `tokio::signal::ctrl_c`
//! （dev-dependencies 限定の `signal` feature、pay-for-what-you-use）を
//! shutdown Future として渡す構成例を示す。
//!
//! 動作確認手順:
//! ```text
//! $ cargo run --example graceful_shutdown -p fandhe-backend-core
//! $ curl -v http://127.0.0.1:3001/   # 200 応答
//! # Ctrl-C を送ると「シャットダウンシグナルを受信しました」と出力し、
//! # 新規接続の受理を止めた上で in-flight リクエストの完了を待って終了する。
//! ```

use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::Router;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let router = Router::new().route("GET", "/", |_head, _body| {
        Response::new(200, b"fandhe-backend: graceful_shutdown example\n".to_vec())
    });

    let server = Server::new()
        .handler(router)
        .shutdown_grace_period(std::time::Duration::from_secs(10));
    let bound = server.bind("127.0.0.1:3001").await?;
    println!("listening on http://{}", bound.local_addr()?);

    bound
        .run_until(async {
            // Ctrl-C 受信を shutdown Future として渡す。`ctrl_c()` が返す
            // `Result` は「シグナルハンドラの登録に失敗した」場合のみ `Err`
            // になる（OS 側の異常）。本 example では `expect` で落とすに留め、
            // 通常運用のフォールバックはコアの責務ではなく利用者側の設計
            // （本 example のスコープ外）とする。
            tokio::signal::ctrl_c()
                .await
                .expect("Ctrl-C シグナルハンドラの登録に失敗しました");
            println!("シャットダウンシグナルを受信しました。graceful shutdown を開始します。");
        })
        .await
}
