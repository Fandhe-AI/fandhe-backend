//! `tracing` feature（TASK-10.1 / #56）配線の統合テスト（feature 有効側）。
//!
//! `Server::tracing` で登録した `TracingMiddleware`（`crates/core/src/server.rs`）
//! が実際に `handle_connection` の `Middleware::on_response` フックから呼ばれ、
//! `bf_plugin_tracing::TracingLayer` のサンプリング判定に従って応答時 1 イベント
//! （TASK-10.2 / #57 で span+2 イベントから統合）が記録されることを、
//! `tokio::io::duplex` で駆動する `handle_connection` + テスト専用
//! `tracing_subscriber::Layer`（イベント件数カウンタ）で検証する。
//!
//! feature 無効時の陰性対照は `plugin_tracing_boundary_disabled.rs` を参照。

#![cfg(feature = "tracing")]

use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use backend_framework_core::{Handler, Server, handle_connection};
use bf_http::request::RequestHead;
use bf_http::response::Response;
use bf_plugin_tracing::TracingConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::subscriber::Subscriber;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// 固定 200 応答を返すだけのトイハンドラ。
struct FixedOkHandler;
impl Handler for FixedOkHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> Response {
        Response::new(200, b"ok".to_vec())
    }
}

/// イベント発生回数だけを数えるテスト用レイヤー
/// （`crates/plugin-tracing/src/layer.rs` のテストと同型のパターン）。
struct CountingLayer(Arc<AtomicUsize>);

impl<S: Subscriber> Layer<S> for CountingLayer {
    fn on_event(&self, _event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// `count` 件の `GET /` リクエストを 1 本の keep-alive 接続にパイプライン化した
/// リクエストバイト列を組み立てる（最後の 1 件だけ `Connection: close` を付け、
/// `handle_connection` のループを終端させる）。
fn pipelined_get_requests(count: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    for i in 0..count {
        let connection_header = if i + 1 == count {
            "Connection: close\r\n"
        } else {
            ""
        };
        buf.extend_from_slice(
            format!("GET / HTTP/1.1\r\nHost: example.com\r\n{connection_header}\r\n").as_bytes(),
        );
    }
    buf
}

#[tokio::test]
async fn sampled_requests_emit_exactly_interval_ratio_of_events() {
    // interval = 3、9 リクエスト送信 → 3 リクエストが採択され、1 採択あたり
    // 応答時 1 イベント（TASK-10.2 / #57 で統合、`crates/plugin-tracing/src/
    // layer.rs` の doc を参照）記録されるので合計 3 イベントを期待する。
    let config = TracingConfig::new(NonZeroU64::new(3).unwrap());
    let server = Server::new().handler(FixedOkHandler).tracing(config);

    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = Registry::default().with(CountingLayer(Arc::clone(&count)));

    let (mut client, server_stream) = tokio::io::duplex(65536);
    let request = pipelined_get_requests(9);
    client.write_all(&request).await.unwrap();
    client.shutdown().await.unwrap();

    // `tracing::subscriber::with_default` は同期クロージャの実行中のみ
    // サブスクライバを差し替える一方、`handle_connection` は async fn（await を
    // 挟む）である。そのため `set_default` が返すガードを await をまたいで保持し、
    // スレッドローカルのデフォルトを `handle_connection` 完了まで維持する
    // （`#[tokio::test]` 既定の `current_thread` フレーバでは実行スレッドが
    // 変わらないため、ガード保持中の await でも有効に伝播する）。
    let _guard = tracing::subscriber::set_default(subscriber);
    handle_connection(&server, server_stream).await;
    drop(_guard);

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    let response = String::from_utf8(out).unwrap();
    // 9 リクエストすべてに 200 応答が返っていること（記録の有無に関わらず
    // リクエスト処理自体は全件成功する契約）。
    assert_eq!(response.matches("HTTP/1.1 200 OK\r\n").count(), 9);

    assert_eq!(count.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn interval_one_samples_every_request() {
    let config = TracingConfig::new(NonZeroU64::new(1).unwrap());
    let server = Server::new().handler(FixedOkHandler).tracing(config);

    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = Registry::default().with(CountingLayer(Arc::clone(&count)));

    let (mut client, server_stream) = tokio::io::duplex(65536);
    let request = pipelined_get_requests(4);
    client.write_all(&request).await.unwrap();
    client.shutdown().await.unwrap();

    let _guard = tracing::subscriber::set_default(subscriber);
    handle_connection(&server, server_stream).await;
    drop(_guard);

    // 4 リクエスト全件採択 × 応答時 1 イベント = 4（TASK-10.2 / #57 で統合）。
    assert_eq!(count.load(Ordering::Relaxed), 4);
}

/// `/health` と `/` を交互にパイプライン化したリクエストバイト列を組み立てる。
/// 最後の 1 件だけ `Connection: close` を付け、`handle_connection` のループを
/// 終端させる（`pipelined_get_requests` の変種、TASK-10.3 / #58 統合テスト用）。
fn pipelined_health_and_root_requests(pairs: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let total = pairs * 2;
    let mut i = 0;
    for _ in 0..pairs {
        for target in ["/health", "/"] {
            i += 1;
            let connection_header = if i == total {
                "Connection: close\r\n"
            } else {
                ""
            };
            buf.extend_from_slice(
                format!("GET {target} HTTP/1.1\r\nHost: example.com\r\n{connection_header}\r\n")
                    .as_bytes(),
            );
        }
    }
    buf
}

/// TASK-10.3（#58）: `Server::tracing` に `exclude_path("/health")` を渡すと、
/// `handle_connection` 経由の実接続でも `/health` 分の記録が一切発生せず、
/// `/` 分のみが記録されることを検証する（`crates/plugin-tracing/src/layer.rs`
/// の unit test と同型の判定を、コアの `Middleware` 配線を通して確認する）。
#[tokio::test]
async fn excluded_path_emits_no_events_through_middleware_pipeline() {
    // interval = 1（全件採択）でも `/health` は除外により記録 0、`/` のみ記録される。
    let config = TracingConfig::new(NonZeroU64::new(1).unwrap()).exclude_path("/health");
    let server = Server::new().handler(FixedOkHandler).tracing(config);

    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = Registry::default().with(CountingLayer(Arc::clone(&count)));

    let (mut client, server_stream) = tokio::io::duplex(65536);
    // "/health" × 3、"/" × 3 を交互送信。
    let request = pipelined_health_and_root_requests(3);
    client.write_all(&request).await.unwrap();
    client.shutdown().await.unwrap();

    let _guard = tracing::subscriber::set_default(subscriber);
    handle_connection(&server, server_stream).await;
    drop(_guard);

    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    let response = String::from_utf8(out).unwrap();
    // 除外対象であってもリクエスト処理自体は成功する（6 リクエスト全件 200）。
    assert_eq!(response.matches("HTTP/1.1 200 OK\r\n").count(), 6);

    // "/" の 3 件のみ採択（1 採択あたり応答時 1 イベント、TASK-10.2 / #57 で
    // span+2 イベントから統合）= 3。"/health" は 0 件。
    assert_eq!(count.load(Ordering::Relaxed), 3);
}
