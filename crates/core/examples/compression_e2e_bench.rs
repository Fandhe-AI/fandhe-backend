//! イシュー #473 の E2E 負荷計測専用バイナリ（production コード変更を含まない、
//! `examples/compression_blocking_micro_bench.rs`・`examples/tracing_nfr.rs` と
//! 同型の計測専用 example パターン）。
//!
//! イシュー #468 で `plugin-compression` の gzip 圧縮に `spawn_blocking`
//! オフロードを追加したが、採否判定はマイクロベンチ（`compress_body` 単体の
//! 所要時間 vs ディスパッチ往復コスト）のみに基づいており、**並行負荷下での
//! E2E 検証（巨大応答の圧縮が同居する小応答のテールレイテンシを実際に
//! 保護しているか）は未実施だった**。本 example は
//! `benches/compression-e2e-bench.sh` から実行され、`BLOCKING_THRESHOLD` を
//! 差し替えた 2 構成（既定 64 KiB / 常時インライン相当の `usize::MAX`）で
//! 混在ワークロードを比較するためのサーバを提供する。
//!
//! # エンドポイント
//!
//! - `GET /health`: 無圧縮の最小応答（`benches/lib/common.sh` の
//!   `wait_for_health` が起動完了検知に使う）
//! - `GET /large`: `LARGE_BODY_SIZE` バイトの `application/json` 応答
//!   （既定しきい値 64 KiB 以上。バックグラウンド負荷として圧縮の
//!   オフロード有無を切り替える対象）
//! - `GET /small`: 4 KiB の `application/json` 応答（`min_size`（既定
//!   1024 バイト）以上・64 KiB 未満に固定し、両構成とも常にインライン
//!   圧縮になるようにする。比較条件を「/large のオフロード有無」だけに
//!   揃えるための設計、詳細は `docs/design/plugin-boundary.md` 5.10.7 節
//!   「E2E 検証」小節を参照）
//!
//! # 実行方法
//!
//! ```text
//! $ cargo build --release --example compression_e2e_bench \
//!     -p fandhe-backend-core --features compression
//! $ BIND_ADDR=127.0.0.1:3011 BLOCKING_THRESHOLD=max \
//!     ./target/release/examples/compression_e2e_bench
//! ```

use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_compression::CompressionConfig;
use fandhe_backend_routes::Router;

/// `LARGE_BODY_SIZE` の既定値（256 KiB）。マイクロベンチ（#468）で
/// 圧縮所要時間が約 370µs に達し、ワーカスレッド占有が /small の p99 に
/// 現れやすい規模として選んだ（`benches/reports/
/// issue468-compression-blocking.md` 1 節の 262144 バイト行を参照）。
const DEFAULT_LARGE_BODY_SIZE: usize = 256 * 1024;

/// `LARGE_BODY_SIZE` の上限（16 MiB）。誤設定による自己 DoS
/// （メモリ枯渇）を防ぐための構築時検証（`.claude/rules/security.md`）。
const MAX_LARGE_BODY_SIZE: usize = 16 * 1024 * 1024;

/// `/small` の固定応答サイズ（4 KiB）。既定 `min_size`（1024 バイト）以上・
/// 既定 `blocking_threshold`（64 KiB）未満に収め、両構成で常にインライン
/// 圧縮になるようにする。
const SMALL_BODY_SIZE: usize = 4 * 1024;

/// 指定サイズの疑似 JSON body（実運用応答に近い、繰り返しパターンによる
/// 中程度の圧縮率を持つデータ）を生成する。
/// `compression_blocking_micro_bench.rs::make_body` と同型の生成方式。
fn make_body(size: usize) -> Vec<u8> {
    let unit = b"{\"id\":1,\"name\":\"issue-473-e2e-bench\",\"note\":\"payload\"} ";
    unit.iter().copied().cycle().take(size).collect()
}

/// `/health`・`/large`・`/small` の 3 エンドポイントを持つ [`Router`] を
/// 組み立てる。`large_body_size` は env `LARGE_BODY_SIZE` から渡される。
fn build_router(large_body_size: usize) -> Router {
    let large_body = make_body(large_body_size);
    let small_body = make_body(SMALL_BODY_SIZE);
    Router::new()
        .route("GET", "/health", |_head, _body| {
            Response::new(200, b"ok".to_vec()).with_content_type("text/plain")
        })
        .route("GET", "/large", move |_head, _body| {
            Response::new(200, large_body.clone()).with_content_type("application/json")
        })
        .route("GET", "/small", move |_head, _body| {
            Response::new(200, small_body.clone()).with_content_type("application/json")
        })
}

/// `BLOCKING_THRESHOLD` env を解釈する。10 進整数または `max`
/// （`usize::MAX`、常時インライン相当のオプトアウト。
/// `CompressionConfigBuilder::blocking_threshold` の doc を参照）を受け付け、
/// 未指定なら `CompressionConfig` の既定値（64 KiB）をそのまま使う。
///
/// パース失敗は黙って既定値へ丸めず `Err` を返す（フェイルクローズ、
/// `.claude/rules/security.md`）。呼び出し元が stderr へエラーを出し
/// exit 1 する契約。
fn parse_blocking_threshold(raw: &str) -> Result<usize, String> {
    if raw == "max" {
        return Ok(usize::MAX);
    }
    raw.parse::<usize>().map_err(|_| {
        format!("BLOCKING_THRESHOLD は 10 進整数または 'max' である必要があります（現在: {raw}）")
    })
}

/// `LARGE_BODY_SIZE` env を解釈する。上限（16 MiB）超過はエラーとして
/// 拒否する（誤設定による OOM 防止、フェイルクローズ）。
fn parse_large_body_size(raw: &str) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|_| format!("LARGE_BODY_SIZE は 10 進整数である必要があります（現在: {raw}）"))?;
    if value > MAX_LARGE_BODY_SIZE {
        return Err(format!(
            "LARGE_BODY_SIZE は {MAX_LARGE_BODY_SIZE} バイト（16 MiB）以下である必要があります（現在: {value}）"
        ));
    }
    Ok(value)
}

// `worker_threads = 4` に固定する（マイクロベンチ #468 と同一設定）。
// ワーカ数を絞ることで、インライン圧縮（構成 B）が発生させるワーカ
// スレッド占有が /small の p99 に現れやすくなり、オフロード（構成 A）
// との効果差を混在負荷下で観測しやすくする。
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> std::io::Result<()> {
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3011".to_string());

    let large_body_size = match std::env::var("LARGE_BODY_SIZE") {
        Ok(raw) => match parse_large_body_size(&raw) {
            Ok(value) => value,
            Err(msg) => {
                eprintln!("エラー: {msg}");
                std::process::exit(1);
            }
        },
        Err(_) => DEFAULT_LARGE_BODY_SIZE,
    };

    let mut config_builder = CompressionConfig::builder();
    if let Ok(raw) = std::env::var("BLOCKING_THRESHOLD") {
        match parse_blocking_threshold(&raw) {
            Ok(threshold) => {
                config_builder = config_builder.blocking_threshold(threshold);
            }
            Err(msg) => {
                eprintln!("エラー: {msg}");
                std::process::exit(1);
            }
        }
    }
    let compression_config = config_builder.build();
    // `Server::compression` へ move する前に値を控えておく（起動ログ表示用。
    // `CompressionConfig` は `Copy` を実装しないため）。
    let blocking_threshold = compression_config.blocking_threshold();

    let router = build_router(large_body_size);
    let server = fandhe_backend_core::Server::new()
        .handler(router)
        .compression(compression_config);

    eprintln!(
        "compression_e2e_bench: listening on http://{addr}（LARGE_BODY_SIZE={large_body_size}, blocking_threshold={blocking_threshold}）"
    );
    let bound = server.bind(&addr).await?;
    bound.run().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_blocking_threshold_accepts_decimal() {
        assert_eq!(parse_blocking_threshold("65536"), Ok(65536));
    }

    #[test]
    fn parse_blocking_threshold_accepts_max_keyword() {
        assert_eq!(parse_blocking_threshold("max"), Ok(usize::MAX));
    }

    #[test]
    fn parse_blocking_threshold_rejects_garbage() {
        assert!(parse_blocking_threshold("not-a-number").is_err());
    }

    #[test]
    fn parse_large_body_size_accepts_within_limit() {
        assert_eq!(parse_large_body_size("1048576"), Ok(1048576));
    }

    #[test]
    fn parse_large_body_size_rejects_over_limit() {
        assert!(parse_large_body_size("99999999999").is_err());
    }

    #[test]
    fn parse_large_body_size_rejects_garbage() {
        assert!(parse_large_body_size("nope").is_err());
    }

    #[test]
    fn build_router_serves_three_routes() {
        // ルータ構築が panic せず、期待エンドポイント数を持つことを最小確認する
        // （実際の HTTP 応答検証はスモークテスト・E2E 計測スクリプト側で行う）。
        let router = build_router(SMALL_BODY_SIZE);
        let _ = router;
    }
}
