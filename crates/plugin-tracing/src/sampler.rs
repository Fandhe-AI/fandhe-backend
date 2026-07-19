//! 決定的カウンタ方式のサンプリング判定（TASK-10.1）。
//!
//! [`crate::layer::TracingLayer`]（コアの `Middleware::on_response` から呼ばれる）が
//! リクエストごとに [`Sampler::should_sample`] を呼び、記録可否を判定する。
//! 乱数を使わず `AtomicU64` の連番 `n` に対し `n % interval == 0` で判定するため、
//! テストで採択件数を厳密に検証できる（AI ファースト保守性、
//! `.claude/rules/coding-rust.md`）。

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

/// 一定間隔でのみ記録可否を許可するサンプラー。
///
/// `interval` に 1 を指定すると全件記録、`N`（`N > 1`）を指定すると `N` 件に
/// 1 件のみ記録する。内部カウンタは `AtomicU64::fetch_add` による単調増加のみで
/// 更新するため、`Middleware::on_response`（同期 API、`.claude/rules/coding-rust.md`）
/// から呼んでもロック取得やブロッキング I/O を伴わない（AGENTS.md「規約:
/// ミドルウェア非同期 I/O 必須化」の「非ブロッキング操作に留める」実装パターンに
/// 適合する）。
#[derive(Debug)]
pub struct Sampler {
    interval: NonZeroU64,
    counter: AtomicU64,
}

impl Sampler {
    /// `interval` 件に 1 件の割合で記録を許可するサンプラーを作る。
    ///
    /// `NonZeroU64` を要求することでゼロ除算（`n % 0`）を型レベルで排除する。
    #[must_use]
    pub const fn new(interval: NonZeroU64) -> Self {
        Self {
            interval,
            counter: AtomicU64::new(0),
        }
    }

    /// このリクエストを記録すべきかを判定する。
    ///
    /// 呼び出しごとに内部カウンタを 1 つ進め、`interval` 件に 1 件だけ `true` を
    /// 返す。複数スレッドから並行に呼んでも `fetch_add` の原子性により採択総数は
    /// 呼び出し総数 `N` に対して厳密に `N / interval`（切り捨て）件となる
    /// （`tests` モジュールの並行アクセステストで検証）。
    ///
    /// # Examples
    ///
    /// 全件記録（`interval = 1`）:
    ///
    /// ```
    /// use fandhe_backend_plugin_tracing::Sampler;
    /// use std::num::NonZeroU64;
    ///
    /// let sampler = Sampler::new(NonZeroU64::new(1).unwrap());
    /// assert!(sampler.should_sample());
    /// assert!(sampler.should_sample());
    /// assert!(sampler.should_sample());
    /// ```
    ///
    /// 3 件に 1 件のみ記録（`interval = 3`）:
    ///
    /// ```
    /// use fandhe_backend_plugin_tracing::Sampler;
    /// use std::num::NonZeroU64;
    ///
    /// let sampler = Sampler::new(NonZeroU64::new(3).unwrap());
    /// let results: Vec<bool> = (0..6).map(|_| sampler.should_sample()).collect();
    /// assert_eq!(results, vec![true, false, false, true, false, false]);
    /// ```
    pub fn should_sample(&self) -> bool {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        n.is_multiple_of(self.interval.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn interval_one_samples_every_call() {
        let sampler = Sampler::new(NonZeroU64::new(1).unwrap());
        for _ in 0..50 {
            assert!(sampler.should_sample());
        }
    }

    #[test]
    fn interval_hundred_samples_one_in_hundred() {
        let sampler = Sampler::new(NonZeroU64::new(100).unwrap());
        let sampled = (0..1000).filter(|_| sampler.should_sample()).count();
        assert_eq!(sampled, 10);
    }

    /// 複数スレッドから並行に `should_sample` を呼んでも、採択総数が
    /// `呼び出し総数 / interval` に厳密に一致することを検証する（決定的カウンタ
    /// 方式の並行安全性の根拠）。
    #[test]
    fn concurrent_access_preserves_exact_ratio() {
        let sampler = Arc::new(Sampler::new(NonZeroU64::new(10).unwrap()));
        let threads_count = 8;
        let calls_per_thread = 1000;

        let handles: Vec<_> = (0..threads_count)
            .map(|_| {
                let sampler = Arc::clone(&sampler);
                thread::spawn(move || {
                    (0..calls_per_thread)
                        .filter(|_| sampler.should_sample())
                        .count()
                })
            })
            .collect();

        let total_sampled: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
        let total_calls = threads_count * calls_per_thread;
        assert_eq!(total_sampled, total_calls / 10);
    }
}
