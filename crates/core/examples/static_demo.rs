//! 静的ファイル配信プラグイン（`static` feature）の配線サンプル（イシュー #318）。
//!
//! 一時ディレクトリへ最小 SPA ライクな `index.html` を書き込み、
//! `Server::static_files(config)` で `/static` プレフィックス配下を配信する。
//!
//! # 動作確認手順
//!
//! ```text
//! $ cargo run --example static_demo -p fandhe-backend-core --features static
//!
//! # index.html（拡張子なしの mount そのまま）
//! $ curl -si localhost:3005/static
//!
//! # 通常ファイル（Content-Type 推定 + X-Content-Type-Options: nosniff を確認）
//! $ curl -si localhost:3005/static/app.js
//!
//! # パストラバーサル試行（404 を確認）
//! $ curl -si --path-as-is localhost:3005/static/../Cargo.toml
//! ```

use fandhe_backend_plugin_static::StaticFilesConfig;

/// デモ用の配信対象ディレクトリを一時領域に組み立てる（`index.html` +
/// `app.js`）。実運用ではビルド済み SPA の出力先ディレクトリを指す想定
/// （`StaticFilesConfig::builder` の doc を参照）。
fn prepare_demo_root() -> std::io::Result<std::path::PathBuf> {
    let mut dir = std::env::temp_dir();
    dir.push(format!("fandhe-static-demo-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("index.html"),
        b"<!doctype html><html><body><h1>fandhe-backend static demo</h1></body></html>",
    )?;
    std::fs::write(
        dir.join("app.js"),
        b"console.log('fandhe-backend static demo');",
    )?;
    Ok(dir)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3005".to_string());
    let root = prepare_demo_root()?;
    let config = StaticFilesConfig::builder("/static", &root)
        .build()
        .expect("prepare_demo_root() が作成した一時ディレクトリは必ず存在するため構築に成功する");

    // `Server::static_files` 未登録なら feature 有効でも完全フォールスルー
    // する（`Server::static_files` の doc を参照）。既定 `Handler` は未登録
    // のため、対象外パスは 404 になる。
    let server = fandhe_backend_core::Server::new().static_files(config);

    println!(
        "static_demo: listening on http://{addr} (serving {} at /static)",
        root.display()
    );
    let bound = server.bind(&addr).await?;
    bound.run().await
}
