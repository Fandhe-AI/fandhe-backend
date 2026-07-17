//! サンプリング判定と event 記録本体（TASK-10.1・TASK-10.2 / #57）。
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
    /// `bf_http::request::RequestHead::target` は HTTP リクエストラインの
    /// request-target をそのまま保持しており、クエリ文字列（`?` 以降）を含みうる
    /// （例: `/login?token=SECRET`）。クエリ文字列にはトークン・API キー等の機密情報
    /// が乗ることが多いため、`path` フィールドとして記録する前に `?` 以降を必ず
    /// 除去する（レビュー指摘対応。「クエリ文字列は一切記録しない」という本 doc
    /// comment・`crate` doc の契約を実コードで担保する）。
    ///
    /// 記録は応答時の 1 イベントに統合する（TASK-10.2 / #57）。
    ///
    /// PoC-10 代表構成と同粒度だった旧実装（span 1 つ + 受理・応答の 2 イベント）
    /// は、採択 1 件あたり subscriber コールバックが 4 回（`on_new_span` +
    /// enter/exit + イベント 2 件）発生していた。span 自体を廃止し単一イベントへ
    /// 落とすことで 1 コールバックに削減し、TASK-10.4（性能再検証）の前提となる
    /// 記録コスト削減を図る。旧 span に載せていた method・path はイベントの
    /// 構造化フィールドへ移すため、統合による情報欠落はない（"request accepted"
    /// イベントは固有フィールドを持たず、統合による情報損失は実質ゼロ）。
    pub fn record_response(&self, head: &RequestHead, elapsed: Duration) {
        if !self.sampler.should_sample() {
            return;
        }

        // クエリ文字列（機密情報を含みうる）を除いた path 部分のみを記録する。
        let path = head
            .target
            .split_once('?')
            .map_or(head.target.as_str(), |(path, _query)| path);

        tracing::info!(
            method = %head.method,
            path = %path,
            elapsed_ms = elapsed.as_secs_f64() * 1000.0,
            "request completed"
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

        // interval = 3 → 9 回中 3 回採択、1 採択あたりイベントちょうど 1 件
        // （TASK-10.2 / #57 で応答時 1 イベントへ統合、span は生成しない）。
        assert_eq!(count.load(Ordering::Relaxed), 3);
    }

    /// `record_response` が発行するイベントの `path` フィールドの文字列表現を
    /// 収集するテスト用レイヤー（TASK-10.2 / #57 で span 廃止に伴い、
    /// `on_new_span` ベースから `on_event` + `Visit` ベースへ書き換え）。
    struct PathCapturingLayer(Arc<std::sync::Mutex<Vec<String>>>);

    struct PathVisitor<'a>(&'a mut Vec<String>);

    impl tracing::field::Visit for PathVisitor<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "path" {
                self.0.push(format!("{value:?}"));
            }
        }
    }

    impl<S: Subscriber> Layer<S> for PathCapturingLayer {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let mut paths = self.0.lock().unwrap();
            event.record(&mut PathVisitor(&mut paths));
        }
    }

    /// span 生成回数だけを数えるテスト用レイヤー（`on_new_span` フック）。
    /// TASK-10.2 / #57 で `record_response` から `info_span!` を除去した契約を
    /// 検証する（span が 1 件も生成されないことの回帰テスト）。
    struct SpanCountingLayer(Arc<AtomicUsize>);

    impl<S: Subscriber> Layer<S> for SpanCountingLayer {
        fn on_new_span(
            &self,
            _attrs: &tracing::span::Attributes<'_>,
            _id: &tracing::span::Id,
            _ctx: Context<'_, S>,
        ) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 採択 1 件あたり span は生成されずイベントちょうど 1 件のみ記録されること
    /// を検証する（TASK-10.2 / #57 の受け入れ条件: 応答時 1 イベントへの統合）。
    #[test]
    fn record_response_emits_no_span_and_exactly_one_event_per_sample() {
        let event_count = Arc::new(AtomicUsize::new(0));
        let span_count = Arc::new(AtomicUsize::new(0));
        let subscriber = Registry::default()
            .with(CountingLayer(Arc::clone(&event_count)))
            .with(SpanCountingLayer(Arc::clone(&span_count)));

        let config = TracingConfig::new(NonZeroU64::new(1).unwrap());
        let layer = TracingLayer::new(&config);
        let head = sample_head();

        subscriber::with_default(subscriber, || {
            layer.record_response(&head, Duration::from_millis(1));
        });

        assert_eq!(event_count.load(Ordering::Relaxed), 1);
        assert_eq!(span_count.load(Ordering::Relaxed), 0);
    }

    /// レビュー指摘（High）対応の回帰テスト: クエリ文字列（機密情報を含みうる）が
    /// `path` フィールドに記録されないことを検証する。`?` 以降が確実に除去され、
    /// `lib.rs` / 本ファイルの「クエリ文字列は一切記録しない」契約が実コードで
    /// 担保されていることを保証する。
    #[test]
    fn record_response_strips_query_string_from_path() {
        let paths = Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(PathCapturingLayer(Arc::clone(&paths)));

        let config = TracingConfig::new(NonZeroU64::new(1).unwrap());
        let layer = TracingLayer::new(&config);

        let buf = b"GET /login?token=SECRET123&user=alice HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let head = match parse_request_head(buf).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            other => panic!("unexpected outcome: {other:?}"),
        };
        assert_eq!(head.target, "/login?token=SECRET123&user=alice");

        subscriber::with_default(subscriber, || {
            layer.record_response(&head, Duration::from_millis(1));
        });

        let recorded = paths.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0], "/login");
        assert!(!recorded[0].contains("token"));
        assert!(!recorded[0].contains("SECRET123"));
    }
}
