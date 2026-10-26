//! `jwks-refresh` の設定型・取得関数 trait の公開 API テスト。
#![cfg(feature = "jwks-refresh")]

use fandhe_backend_plugin_jwt::{
    DEFAULT_FETCH_TIMEOUT, DEFAULT_REFRESH_MIN_INTERVAL, FetchError, FetchFuture, JwksFetcher,
    JwksResponse, MAX_FETCH_TIMEOUT, MAX_REFRESH_MIN_INTERVAL, MAX_REFRESH_PERIOD, MAX_STALE_LIMIT,
    RefreshConfig, RefreshConfigError as E,
};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

const NS: Duration = Duration::from_nanos(1);

fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn defaults() {
    let c = RefreshConfig::new(s(60)).unwrap();
    assert_eq!(c.min_interval(), DEFAULT_REFRESH_MIN_INTERVAL);
    assert_eq!(c.fetch_timeout(), DEFAULT_FETCH_TIMEOUT);
    assert_eq!(c.max_stale(), None);
}

#[test]
fn period_bounds() {
    assert_eq!(
        RefreshConfig::new(Duration::ZERO).unwrap_err(),
        E::ZeroPeriod
    );
    assert!(RefreshConfig::new(MAX_REFRESH_PERIOD).is_ok());
    assert_eq!(
        RefreshConfig::new(MAX_REFRESH_PERIOD + NS).unwrap_err(),
        E::PeriodTooLarge
    );
}

#[test]
fn min_interval_bounds() {
    let b = || RefreshConfig::builder(MAX_REFRESH_PERIOD);
    assert_eq!(
        b().min_interval(Duration::ZERO).build().unwrap_err(),
        E::ZeroMinInterval
    );
    assert!(b().min_interval(MAX_REFRESH_MIN_INTERVAL).build().is_ok());
    assert_eq!(
        b().min_interval(MAX_REFRESH_MIN_INTERVAL + NS)
            .build()
            .unwrap_err(),
        E::MinIntervalTooLarge
    );
}

#[test]
fn fetch_timeout_bounds() {
    let b = || RefreshConfig::builder(MAX_REFRESH_PERIOD).min_interval(MAX_REFRESH_MIN_INTERVAL);
    assert_eq!(
        b().fetch_timeout(Duration::ZERO).build().unwrap_err(),
        E::ZeroFetchTimeout
    );
    assert!(b().fetch_timeout(MAX_FETCH_TIMEOUT).build().is_ok());
    assert_eq!(
        b().fetch_timeout(MAX_FETCH_TIMEOUT + NS)
            .build()
            .unwrap_err(),
        E::FetchTimeoutTooLarge
    );
}

#[test]
fn max_stale_bounds() {
    let b = || RefreshConfig::builder(MAX_REFRESH_PERIOD);
    assert_eq!(
        b().max_stale(Duration::ZERO).build().unwrap_err(),
        E::ZeroMaxStale
    );
    assert!(b().max_stale(MAX_STALE_LIMIT).build().is_ok());
    assert_eq!(
        b().max_stale(MAX_STALE_LIMIT + NS).build().unwrap_err(),
        E::MaxStaleTooLarge
    );
}

#[test]
fn relations() {
    let b = || RefreshConfig::builder(s(60));
    assert!(b().min_interval(s(60)).build().is_ok());
    assert_eq!(
        b().min_interval(s(61)).build().unwrap_err(),
        E::MinIntervalExceedsPeriod
    );
    assert!(b().fetch_timeout(s(10)).build().is_ok());
    assert_eq!(
        b().fetch_timeout(s(11)).build().unwrap_err(),
        E::FetchTimeoutExceedsMinInterval
    );
    // period + fetch_timeout = 65s
    assert!(b().max_stale(s(65)).build().is_ok());
    assert_eq!(
        b().max_stale(s(64)).build().unwrap_err(),
        E::MaxStaleTooShort
    );
}

#[test]
fn short_period_is_order_independent() {
    let a = RefreshConfig::builder(s(5))
        .min_interval(s(2))
        .fetch_timeout(s(1))
        .build();
    let b = RefreshConfig::builder(s(5))
        .fetch_timeout(s(1))
        .min_interval(s(2))
        .build();
    assert!(a.is_ok() && b.is_ok());
}

#[test]
fn display_is_fixed_and_nonempty() {
    for e in [
        E::ZeroPeriod,
        E::MaxStaleTooShort,
        E::FetchTimeoutExceedsMinInterval,
    ] {
        assert!(!e.to_string().is_empty());
    }
    for e in [
        FetchError::Transport,
        FetchError::Status,
        FetchError::TooLarge,
        FetchError::Other,
    ] {
        assert!(!e.to_string().is_empty());
    }
}

#[test]
fn response_debug_hides_body() {
    let r = JwksResponse::new("SECRET-BODY".into(), None);
    assert!(!format!("{r:?}").contains("SECRET"));
}

struct Fixed;
impl JwksFetcher for Fixed {
    fn fetch(&self) -> FetchFuture<'_> {
        Box::pin(async { Ok(JwksResponse::new(r#"{"keys":[]}"#.into(), Some(s(30)))) })
    }
}

#[test]
fn fetcher_is_dyn_compatible() {
    let f: Arc<dyn JwksFetcher> = Arc::new(Fixed);
    let mut fut = f.fetch();
    let mut cx = Context::from_waker(Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(Ok(r)) => assert_eq!(r.max_age(), Some(s(30))),
        _ => panic!("即時完了のはず"),
    }
}
