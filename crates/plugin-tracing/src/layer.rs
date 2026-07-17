//! サンプリング判定と span/event 記録本体（TASK-10.1）。
//!
//! [`TracingLayer`] はコア側の `Middleware` アダプタ（`crates/core/src/server.rs`
//! の `TracingMiddleware`、`tracing` feature 限定）から `on_response` フック内で
//! 呼ばれる。`Middleware::on_request` / `on_response` は同期 API のままであり
//! （`crates/core/src/extension.rs`）、trait を跨いでリクエスト毎の状態を運ぶ経路が
//! ないため、判定・記録は `on_response` の 1 点に集約する（`on_request` 側での
//! 独立判定は同一リクエストの記録が対にならないため採らない。計画書 3.1 節）。

use std::time::Duration;

use bf_http::request::RequestHead;

use crate::config::TracingConfig;
use crate::sampler::Sampler;

/// [`TracingConfig`] と [`Sampler`] を束ね、コアの `Middleware` アダプタへ委譲先を
/// 提供する。
///
/// `tracing` クレートのマクロ呼び出し自体は、`tracing-subscriber` に登録された
/// レイヤが `tracing-appender::non_blocking` の writer を使う限り非同期・
/// バッファ済みになる（AGENTS.md「規約: ミドルウェア非同期 I/O 必須化」）。本型
/// 自体は writer の選択に関与せず、[`crate::init::init_tracing`] が初期化した
/// グローバルサブスクライバに記録を委ねるだけであり、`Send + Sync` を満たす
/// （コアが `Box<dyn Middleware>` として複数接続タスク間で共有するための要件）。
#[derive(Debug)]
pub struct TracingLayer {
    sampler: Sampler,
}

impl TracingLayer {
    /// `config.sample_interval` に従うサンプラーを持つレイヤーを作る。
    #[must_use]
    pub fn new(config: &TracingConfig) -> Self {
        Self {
            sampler: Sampler::new(config.sample_interval),
        }
    }

    /// レスポンス送出後にコアの `Middleware::on_response` から呼ばれる記録エントリ
    /// ポイント。
    ///
    /// サンプリング対象外（[`Sampler::should_sample`] が `false`）の場合は何もせず
    /// 即座に返る（`tracing` マクロ呼び出し自体を避けることで、有効化コストを
    /// サンプリング間隔に応じて按分する。PoC-10 の「非同期 I/O 化だけでは不十分」
    /// という知見に対する最小限の対策）。
    ///
    /// 記録内容は method・path・elapsed_ms の 3 フィールドに限定する
    /// （`.claude/rules/security.md` のログインジェクション・PII 観点。ヘッダ値・
    /// ボディ・クエリ文字列は一切記録しない）。`RequestHead` の `Display` /
    /// `Debug` を経由せず、`tracing` の構造化フィールドとして値を直接渡すため、
    /// 制御文字混入によるログフォーマット崩壊のリスクがない。
    ///
    /// 記録は 1 つの span 内で受理・応答の 2 イベントとして残す（PoC-10 代表構成と
    /// 同粒度）。1 イベントへの統合は TASK-10.2（#57）のスコープ。
    pub fn record_response(&self, head: &RequestHead, elapsed: Duration) {
        if !self.sampler.should_sample() {
            return;
        }

        let span = tracing::info_span!(
            "http_request",
            method = %head.method,
            path = %head.target,
        );
        let _guard = span.enter();
        tracing::info!(parent: &span, "request accepted");
        tracing::info!(
            parent: &span,
            elapsed_ms = elapsed.as_secs_f64() * 1000.0,
            "response sent"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_http::request::{ParseOutcome, parse_request_head};
    use std::num::NonZeroU64;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing::subscriber::{self, Subscriber};
    use tracing_subscriber::Registry;
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

    fn sample_head() -> RequestHead {
        let buf = b"GET /health HTTP/1.1\r\nHost: example.com\r\n\r\n";
        match parse_request_head(buf).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    /// イベント発生回数だけを数えるテスト用レイヤー。
    struct CountingLayer(Arc<AtomicUsize>);

    impl<S: Subscriber> Layer<S> for CountingLayer {
        fn on_event(&self, _event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// サンプリング採択件数どおりに `tracing` イベントが記録されることを
    /// （実際のサブスクライバ経由で）検証する。`init_tracing` の非同期 writer は
    /// 使わず、テスト用の同期カウンタレイヤーで代替する（グローバル状態を汚さない
    /// ため `tracing::subscriber::with_default` でスコープを限定する）。
    #[test]
    fn record_response_emits_events_only_when_sampled() {
        let count = Arc::new(AtomicUsize::new(0));
        let subscriber = Registry::default().with(CountingLayer(Arc::clone(&count)));

        let config = TracingConfig::new(NonZeroU64::new(3).unwrap());
        let layer = TracingLayer::new(&config);
        let head = sample_head();

        subscriber::with_default(subscriber, || {
            for _ in 0..9 {
                layer.record_response(&head, Duration::from_millis(1));
            }
        });

        // interval = 3 → 9 回中 3 回採択、1 採択あたり 2 イベント（受理・応答）。
        assert_eq!(count.load(Ordering::Relaxed), 6);
    }
}
