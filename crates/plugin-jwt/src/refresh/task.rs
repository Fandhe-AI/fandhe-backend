//! JWKS 取り直しタスク本体: 定期の取得・失敗時の保持・`max_stale` のフェイルクローズ。
//!
//! 対象 feature: `jwks-refresh`（既定では無効。pay-for-what-you-use）。
//!
//! アプリが [`JwksRefresher::new`] で組み立てた [`RefreshTask`] を自分のランタイムで
//! `tokio::spawn` し、[`RefreshHandle`] を保持する。タスクは [`SharedKeySet`] を
//! [`SharedKeySet::set`] だけで更新し、ゲート側（`JwtAuthenticator`）はこのモジュールを
//! 知らない（共有は `SharedKeySet` 経由のみ）。取得の実体は [`JwksFetcher`]（アプリ実装）。
//! 設計は `docs/design/jwt-verify-plugin.md` 8 章（8.4・8.7〜8.10）。
//!
//! 本クレートは `tokio::spawn` も `tokio::select!` も使わない（`rt`・`macros` 不要）。
//! 複数 future の競合は `poll_fn` による固定優先順位の自前 race で行う。

use std::fmt;
use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::{Instant, Sleep, sleep_until, timeout};

use super::{FetchError, FetchFuture, JwksFetcher, JwksResponse, RefreshConfig};
use crate::{JwksError, KeySet, SharedKeySet, UnknownKeyEvent, UnknownKeyNotifier};

/// 取り直し 1 回の失敗の分類。本文・`kid`・URL を持たない（ログ安全）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_jwt::{FetchError, RefreshFailure};
/// assert!(!RefreshFailure::Fetch(FetchError::Transport).to_string().is_empty());
/// assert!(!RefreshFailure::Timeout.to_string().is_empty());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefreshFailure {
    /// 取得関数がエラーを返した。
    Fetch(FetchError),
    /// 取得が `fetch_timeout` 内に完了しなかった。
    Timeout,
    /// 取得できた本文が `KeySet::from_json` の検査を通らなかった。
    Parse(JwksError),
    /// 取得関数が panic した。
    Panicked,
}

impl fmt::Display for RefreshFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Fetch(_) => "JWKS の取得に失敗しました",
            Self::Timeout => "JWKS の取得が時間切れになりました",
            Self::Parse(_) => "取得した JWKS が検査を通りませんでした",
            Self::Panicked => "JWKS の取得関数が panic しました",
        };
        f.write_str(msg)
    }
}

/// 取り直しタスクが [`RefreshObserver`] へ通知する出来事。件数と種別だけを運ぶ。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_jwt::RefreshEvent;
/// let e = RefreshEvent::Updated { keys: 2, skipped: 0 };
/// assert_eq!(e, RefreshEvent::Updated { keys: 2, skipped: 0 });
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefreshEvent {
    /// 鍵集合を更新した。
    Updated {
        /// 採用した鍵の件数。
        keys: usize,
        /// 読み飛ばした鍵の件数。
        skipped: usize,
    },
    /// 取り直しに失敗した（現行の鍵集合は保持）。
    Failed(RefreshFailure),
    /// `max_stale` の超過で鍵集合を空にした。
    Cleared,
}

/// 取り直しの観測口。同期・非ブロックで呼ばれる。
///
/// 契約: ブロックしない・panic しない・I/O しない（`UnknownKeyNotifier` と同じ）。
/// panic してもタスクは止まらない（`panic = "abort"` の構成では保護が効かない）。
/// `Fn(&RefreshEvent)` のクロージャをそのまま渡せる。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_jwt::{RefreshEvent, RefreshObserver};
///
/// fn takes(_: impl RefreshObserver) {}
/// takes(|_e: &RefreshEvent| {});
/// ```
pub trait RefreshObserver: Send + Sync {
    /// 出来事を受け取る。
    fn on_event(&self, event: &RefreshEvent);
}

impl<F> RefreshObserver for F
where
    F: Fn(&RefreshEvent) + Send + Sync,
{
    fn on_event(&self, event: &RefreshEvent) {
        self(event);
    }
}

struct NoopObserver;

impl RefreshObserver for NoopObserver {
    fn on_event(&self, _event: &RefreshEvent) {}
}

/// タスクの状態スナップショット（[`RefreshHandle::status`]）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_jwt::{
///     FetchFuture, JwksFetcher, JwksRefresher, JwksResponse, KeySet, RefreshConfig, SharedKeySet,
/// };
/// use std::time::Duration;
///
/// struct F;
/// impl JwksFetcher for F {
///     fn fetch(&self) -> FetchFuture<'_> {
///         Box::pin(async { Ok(JwksResponse::new(String::new(), None)) })
///     }
/// }
///
/// let cfg = RefreshConfig::new(Duration::from_secs(60)).unwrap();
/// let (_task, handle) = JwksRefresher::new(SharedKeySet::new(KeySet::empty()), F, cfg);
/// let s = handle.status();
/// assert!(s.last_success().is_none());
/// assert_eq!(s.consecutive_failures(), 0);
/// assert!(!s.is_cleared());
/// assert!(!s.is_stopped());
/// ```
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct RefreshStatus {
    last_success: Option<std::time::Instant>,
    consecutive_failures: u32,
    cleared: bool,
    stopped: bool,
}

impl RefreshStatus {
    /// 最後に成功した取得の開始時刻（まだ成功していなければ `None`）。
    #[must_use]
    pub fn last_success(&self) -> Option<std::time::Instant> {
        self.last_success
    }

    /// 連続失敗の回数（成功で 0 に戻る。飽和加算）。
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// `max_stale` の超過で鍵集合を空にした状態か（次の成功で `false` に戻る）。
    #[must_use]
    pub fn is_cleared(&self) -> bool {
        self.cleared
    }

    /// 停止済みか。
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }
}

/// タスクとハンドルの共有状態。`wake` は後続の取得トリガー用の継ぎ目（停止の合図とは別）。
struct Shared {
    stopped: AtomicBool,
    stop: Notify,
    wake: Notify,
    status: Mutex<RefreshStatus>,
}

impl Shared {
    fn new() -> Self {
        Self {
            stopped: AtomicBool::new(false),
            stop: Notify::new(),
            wake: Notify::new(),
            status: Mutex::new(RefreshStatus::default()),
        }
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    fn update(&self, f: impl FnOnce(&mut RefreshStatus)) {
        let mut g = self.status.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g);
    }

    fn mark_stopped(&self) {
        self.stopped.store(true, Ordering::Release);
        self.update(|s| s.stopped = true);
    }
}

/// 取り直しタスクの組み立て口（インスタンスを持たない名前空間）。
///
/// 対象 feature: `jwks-refresh`。組み立てるだけでは何も動かない。返る [`RefreshTask`] を
/// アプリが自分のランタイムで `tokio::spawn` する。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_jwt::{
///     FetchFuture, JwksFetcher, JwksRefresher, JwksResponse, KeySet, RefreshConfig, SharedKeySet,
/// };
/// use std::time::Duration;
///
/// struct Fixed;
/// impl JwksFetcher for Fixed {
///     fn fetch(&self) -> FetchFuture<'_> {
///         Box::pin(async { Ok(JwksResponse::new(r#"{"keys":[]}"#.into(), None)) })
///     }
/// }
///
/// let cfg = RefreshConfig::new(Duration::from_secs(300)).unwrap();
/// let (task, handle) = JwksRefresher::new(SharedKeySet::new(KeySet::empty()), Fixed, cfg);
/// // 本番では `tokio::spawn(task)`。ここでは停止してから 1 回 poll して終わることだけ確かめる。
/// handle.shutdown();
/// let mut task = Box::pin(task);
/// let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
/// assert!(std::future::Future::poll(task.as_mut(), &mut cx).is_ready());
/// assert!(handle.status().is_stopped());
/// ```
#[derive(Debug)]
pub struct JwksRefresher {
    _private: (),
}

impl JwksRefresher {
    /// 観測口なしで組み立てる。
    #[must_use]
    // 設計 8.4 の公開形（タスクとハンドルの組を返す名前空間型）に合わせるため `new` の名を使う。
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        keys: SharedKeySet,
        fetcher: impl JwksFetcher,
        config: RefreshConfig,
    ) -> (RefreshTask, RefreshHandle) {
        Self::with_observer(keys, fetcher, config, NoopObserver)
    }

    /// 観測口つきで組み立てる。
    ///
    /// ここではタイマーも基準時刻も作らない（ランタイム外でも呼べる）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_jwt::{
    ///     FetchFuture, JwksFetcher, JwksRefresher, JwksResponse, KeySet, RefreshConfig,
    ///     RefreshEvent, SharedKeySet,
    /// };
    /// use std::time::Duration;
    ///
    /// struct Fixed;
    /// impl JwksFetcher for Fixed {
    ///     fn fetch(&self) -> FetchFuture<'_> {
    ///         Box::pin(async { Ok(JwksResponse::new(r#"{"keys":[]}"#.into(), None)) })
    ///     }
    /// }
    ///
    /// let cfg = RefreshConfig::new(Duration::from_secs(300)).unwrap();
    /// let (_task, _handle) = JwksRefresher::with_observer(
    ///     SharedKeySet::new(KeySet::empty()),
    ///     Fixed,
    ///     cfg,
    ///     |e: &RefreshEvent| {
    ///         let _ = e; // 件数・種別だけをメトリクスへ送る（同期・非ブロック）
    ///     },
    /// );
    /// ```
    #[must_use]
    pub fn with_observer(
        keys: SharedKeySet,
        fetcher: impl JwksFetcher,
        config: RefreshConfig,
        observer: impl RefreshObserver + 'static,
    ) -> (RefreshTask, RefreshHandle) {
        let shared = Arc::new(Shared::new());
        let fut = run(
            Arc::clone(&shared),
            keys,
            Box::new(fetcher),
            config,
            Box::new(observer),
        );
        (RefreshTask(Box::pin(fut)), RefreshHandle { shared })
    }
}

/// アプリが spawn する取り直しのタスク（`Future<Output = ()>`）。
///
/// 動かすには time ドライバ（`enable_time`）が必要。**spawn しない・drop すると定期の
/// 取得に加え `max_stale` によるフェイルクローズも働かない**（鍵集合は現行のまま残る）。
/// 停止は [`RefreshHandle::shutdown`] か、ハンドルの drop。
pub struct RefreshTask(Pin<Box<dyn Future<Output = ()> + Send + 'static>>);

impl fmt::Debug for RefreshTask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshTask")
    }
}

impl Future for RefreshTask {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.0.as_mut().poll(cx)
    }
}

/// タスクの停止・状態参照ハンドル。`Clone` できない（drop が停止を意味するため）。
///
/// drop すると [`shutdown`](Self::shutdown) と同じく停止する。
pub struct RefreshHandle {
    shared: Arc<Shared>,
}

impl fmt::Debug for RefreshHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshHandle")
    }
}

impl RefreshHandle {
    /// タスクを停止する（冪等）。取得中でも即座に取得の future を drop して終わる。
    ///
    /// # Examples
    ///
    /// ```
    /// # use fandhe_backend_plugin_jwt::{FetchFuture, JwksFetcher, JwksRefresher, JwksResponse, KeySet, RefreshConfig, SharedKeySet};
    /// # struct F;
    /// # impl JwksFetcher for F {
    /// #     fn fetch(&self) -> FetchFuture<'_> { Box::pin(async { Ok(JwksResponse::new(String::new(), None)) }) }
    /// # }
    /// # let cfg = RefreshConfig::new(std::time::Duration::from_secs(60)).unwrap();
    /// let (_task, handle) = JwksRefresher::new(SharedKeySet::new(KeySet::empty()), F, cfg);
    /// handle.shutdown();
    /// handle.shutdown();
    /// assert!(handle.status().is_stopped());
    /// ```
    pub fn shutdown(&self) {
        self.shared.mark_stopped();
        self.shared.stop.notify_one();
    }

    /// 未知 `kid` を契機とする取り直しの合図ハンドルを得る（`Clone` 可）。
    ///
    /// 得たハンドルはタスクの寿命に影響しない。
    #[must_use]
    pub fn trigger(&self) -> RefreshTrigger {
        RefreshTrigger {
            shared: Arc::clone(&self.shared),
        }
    }

    /// 現在の状態のスナップショット。
    #[must_use]
    pub fn status(&self) -> RefreshStatus {
        self.shared
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// 未知 `kid` を契機に取り直しタスクへ合図を送る軽量ハンドル。`Clone` できる。
///
/// 対象 feature: `jwks-refresh`。[`RefreshHandle::trigger`] で得る。合図は
/// `tokio::sync::Notify` の許可 1 つに畳まれ、ブロックも I/O もしない（ゲートの
/// 同期・I/O なし契約を守る）。最小間隔より短い間に届いた合図は、タスクが
/// 「直前の取得開始 + 最小間隔」の 1 回の取得にまとめる（設計 8.7）。
///
/// # 注意
///
/// - 通知に含まれる `kid` は信頼しない値であり、本型は読まない（取得先は構築時に固定）。
/// - 停止後（[`RefreshHandle::shutdown`] / drop 後）の合図は何もしない。
/// - 本型を保持してもタスクは生きない・止まらない（停止は [`RefreshHandle`] が担う）。
/// - [`UnknownKeyNotifier`] も実装するが、`unknown_key_notifier` で登録すると
///   `NotifyThrottle`（既定 10 秒・秒精度）の後ろに並び、取り直しの遅延が延びる。
///   認証器へは `JwtAuthenticatorBuilder::jwks_refresh_trigger` で登録すること。
///
/// # Examples
///
/// ```
/// # use fandhe_backend_plugin_jwt::{FetchFuture, JwksFetcher, JwksRefresher, JwksResponse, KeySet, RefreshConfig, SharedKeySet};
/// # struct F;
/// # impl JwksFetcher for F {
/// #     fn fetch(&self) -> FetchFuture<'_> { Box::pin(async { Ok(JwksResponse::new(String::new(), None)) }) }
/// # }
/// # let cfg = RefreshConfig::new(std::time::Duration::from_secs(60)).unwrap();
/// let (_task, handle) = JwksRefresher::new(SharedKeySet::new(KeySet::empty()), F, cfg);
/// let trigger = handle.trigger();
/// handle.shutdown();
/// // 停止後でも保持・複製できる。
/// let _ = trigger.clone();
/// ```
#[derive(Clone)]
pub struct RefreshTrigger {
    shared: Arc<Shared>,
}

impl fmt::Debug for RefreshTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshTrigger")
    }
}

impl RefreshTrigger {
    /// 合図を 1 つ送る。停止後は何もしない。ブロックせず panic もしない。
    pub(crate) fn fire(&self) {
        if !self.shared.is_stopped() {
            self.shared.wake.notify_one();
        }
    }
}

/// `kid` は読まず合図だけを送る。契約と登録先の注意は [`RefreshTrigger`] を参照。
impl UnknownKeyNotifier for RefreshTrigger {
    fn notify(&self, _event: &UnknownKeyEvent<'_>) {
        self.fire();
    }
}

impl Drop for RefreshHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

type Fetched = Result<Result<JwksResponse, FetchError>, ()>;

/// 取得 future の poll 中の panic を `Err(())` に変える（`unsafe` 不要: `Pin<Box>` は `Unpin`）。
/// 完了後は再 poll せず `Pending` を返す。
struct Guarded<'a> {
    inner: FetchFuture<'a>,
    done: bool,
}

impl Future for Guarded<'_> {
    type Output = Fetched;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Fetched> {
        if self.done {
            return Poll::Pending;
        }
        match catch_unwind(AssertUnwindSafe(|| self.inner.as_mut().poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(r)) => {
                self.done = true;
                Poll::Ready(Ok(r))
            }
            Err(_) => {
                self.done = true;
                Poll::Ready(Err(()))
            }
        }
    }
}

enum Wait {
    Stop,
    Stale,
    Due,
    Signal,
}

fn poll_sleep(s: &mut Option<Pin<Box<Sleep>>>, cx: &mut Context<'_>) -> bool {
    s.as_mut().is_some_and(|s| s.as_mut().poll(cx).is_ready())
}

/// 待機フェーズ。優先順位は 停止 > 鮮度超過 > 取得時刻 > 取得の合図。
async fn wait_phase(shared: &Shared, stale: Option<Instant>, due: Instant) -> Wait {
    let mut stop = pin!(shared.stop.notified());
    let mut wake = pin!(shared.wake.notified());
    let mut stale = stale.map(|d| Box::pin(sleep_until(d)));
    let mut due = Box::pin(sleep_until(due));
    poll_fn(|cx| {
        if shared.is_stopped() || stop.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Wait::Stop);
        }
        if poll_sleep(&mut stale, cx) {
            return Poll::Ready(Wait::Stale);
        }
        if due.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Wait::Due);
        }
        if wake.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Wait::Signal);
        }
        Poll::Pending
    })
    .await
}

fn notify(obs: &dyn RefreshObserver, ev: &RefreshEvent) {
    let _ = catch_unwind(AssertUnwindSafe(|| obs.on_event(ev)));
}

fn clear_keys(shared: &Shared, keys: &SharedKeySet, obs: &dyn RefreshObserver) {
    keys.set(KeySet::empty());
    shared.update(|s| s.cleared = true);
    notify(obs, &RefreshEvent::Cleared);
}

enum FetchEv {
    Stop,
    Done(Result<Fetched, tokio::time::error::Elapsed>),
    Stale,
}

/// 取得 1 回。停止なら `None`（取得 future は drop 済み）。鮮度超過は取得を捨てずに
/// 鍵集合を空にして待ち続ける（8.9: 取得結果を待たずに空にする）。
async fn fetch_once(
    shared: &Shared,
    keys: &SharedKeySet,
    fetcher: &dyn JwksFetcher,
    timeout_d: Duration,
    stale_deadline: &mut Option<Instant>,
    obs: &dyn RefreshObserver,
) -> Option<Result<JwksResponse, RefreshFailure>> {
    let Ok(fut) = catch_unwind(AssertUnwindSafe(|| fetcher.fetch())) else {
        return Some(Err(RefreshFailure::Panicked));
    };
    let mut fut = pin!(timeout(
        timeout_d,
        Guarded {
            inner: fut,
            done: false
        }
    ));
    let mut stale = stale_deadline.map(|d| Box::pin(sleep_until(d)));
    loop {
        let mut stop = pin!(shared.stop.notified());
        let ev = poll_fn(|cx| {
            if shared.is_stopped() || stop.as_mut().poll(cx).is_ready() {
                return Poll::Ready(FetchEv::Stop);
            }
            if let Poll::Ready(r) = fut.as_mut().poll(cx) {
                return Poll::Ready(FetchEv::Done(r));
            }
            if poll_sleep(&mut stale, cx) {
                return Poll::Ready(FetchEv::Stale);
            }
            Poll::Pending
        })
        .await;
        match ev {
            FetchEv::Stop => return None,
            FetchEv::Stale => {
                stale = None;
                *stale_deadline = None;
                clear_keys(shared, keys, obs);
            }
            FetchEv::Done(Err(_elapsed)) => return Some(Err(RefreshFailure::Timeout)),
            FetchEv::Done(Ok(Err(()))) => return Some(Err(RefreshFailure::Panicked)),
            FetchEv::Done(Ok(Ok(Err(e)))) => return Some(Err(RefreshFailure::Fetch(e))),
            FetchEv::Done(Ok(Ok(Ok(resp)))) => return Some(Ok(resp)),
        }
    }
}

async fn run(
    shared: Arc<Shared>,
    keys: SharedKeySet,
    fetcher: Box<dyn JwksFetcher>,
    cfg: RefreshConfig,
    obs: Box<dyn RefreshObserver>,
) {
    // 停止済みなら時刻もタイマーも作らずに終わる（ランタイム外の poll でも安全）。
    if shared.is_stopped() {
        shared.mark_stopped();
        return;
    }
    let floor = cfg.min_interval();
    let start = Instant::now();
    let mut next_due = start;
    let mut next_allowed = start;
    // 起動直後は「タスク開始」を最後の成功とみなして鮮度期限を数える（8.9）。
    let mut stale_deadline = cfg.max_stale().map(|s| start + s);

    loop {
        match wait_phase(&shared, stale_deadline, next_due.max(next_allowed)).await {
            Wait::Stop => break,
            Wait::Stale => {
                stale_deadline = None;
                clear_keys(&shared, &keys, obs.as_ref());
            }
            Wait::Signal => next_due = next_due.min(Instant::now()),
            Wait::Due => {
                let fetch_start = Instant::now();
                next_allowed = fetch_start + floor;
                let Some(outcome) = fetch_once(
                    &shared,
                    &keys,
                    fetcher.as_ref(),
                    cfg.fetch_timeout(),
                    &mut stale_deadline,
                    obs.as_ref(),
                )
                .await
                else {
                    break;
                };
                let parsed = outcome.and_then(|resp| {
                    KeySet::from_json(resp.body())
                        .map(|set| (set, resp.max_age()))
                        .map_err(RefreshFailure::Parse)
                });
                match parsed {
                    Ok((set, max_age)) => {
                        let ev = RefreshEvent::Updated {
                            keys: set.len(),
                            skipped: set.skipped(),
                        };
                        keys.set(set);
                        stale_deadline = cfg.max_stale().map(|s| fetch_start + s);
                        shared.update(|s| {
                            s.consecutive_failures = 0;
                            s.cleared = false;
                            s.last_success = Some(fetch_start.into_std());
                        });
                        // min を先に取るので max_age が過大でもオーバーフローしない。
                        let wait = cfg.period().min(max_age.unwrap_or(cfg.period())).max(floor);
                        next_due = fetch_start + wait;
                        notify(obs.as_ref(), &ev);
                    }
                    Err(f) => {
                        shared.update(|s| {
                            s.consecutive_failures = s.consecutive_failures.saturating_add(1);
                        });
                        next_due = fetch_start + floor;
                        notify(obs.as_ref(), &RefreshEvent::Failed(f));
                    }
                }
            }
        }
    }
    shared.mark_stopped();
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Panics;
    impl JwksFetcher for Panics {
        fn fetch(&self) -> FetchFuture<'_> {
            Box::pin(async { panic!("テスト用") })
        }
    }

    #[test]
    fn guarded_converts_panic() {
        let f = Panics;
        let mut g = Box::pin(Guarded {
            inner: f.fetch(),
            done: false,
        });
        let mut cx = Context::from_waker(std::task::Waker::noop());
        assert!(matches!(g.as_mut().poll(&mut cx), Poll::Ready(Err(()))));
        assert!(g.as_mut().poll(&mut cx).is_pending());
    }

    #[test]
    fn observer_panic_is_contained() {
        let o = |_: &RefreshEvent| panic!("テスト用");
        notify(&o, &RefreshEvent::Cleared);
    }

    #[tokio::test(start_paused = true)]
    async fn signals_are_aligned_to_min_interval() {
        use std::sync::atomic::AtomicUsize;
        struct Count(Arc<AtomicUsize>);
        impl JwksFetcher for Count {
            fn fetch(&self) -> FetchFuture<'_> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(JwksResponse::new(r#"{"keys":[]}"#.into(), None)) })
            }
        }
        let n = Arc::new(AtomicUsize::new(0));
        let cfg = RefreshConfig::new(Duration::from_secs(3600)).unwrap();
        let (task, handle) = JwksRefresher::new(
            SharedKeySet::new(KeySet::empty()),
            Count(Arc::clone(&n)),
            cfg,
        );
        let trigger = handle.trigger();
        let join = tokio::spawn(task);
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(n.load(Ordering::SeqCst), 1);
        // 下限（10 秒）内に合図を何度送っても、下限が来るまで取得しない。
        for _ in 0..5 {
            trigger.fire();
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(n.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(n.load(Ordering::SeqCst), 2);
        drop(handle);
        join.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn notifier_ignores_kid_and_stops_after_shutdown() {
        use std::sync::atomic::AtomicUsize;
        struct Count(Arc<AtomicUsize>);
        impl JwksFetcher for Count {
            fn fetch(&self) -> FetchFuture<'_> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(JwksResponse::new(r#"{"keys":[]}"#.into(), None)) })
            }
        }
        let n = Arc::new(AtomicUsize::new(0));
        let cfg = RefreshConfig::new(Duration::from_secs(3600)).unwrap();
        let (task, handle) = JwksRefresher::new(
            SharedKeySet::new(KeySet::empty()),
            Count(Arc::clone(&n)),
            cfg,
        );
        let trigger = handle.trigger();
        assert_eq!(format!("{trigger:?}"), "RefreshTrigger");
        let join = tokio::spawn(task);
        tokio::time::sleep(Duration::from_millis(1)).await;
        trigger.notify(&UnknownKeyEvent::new("attacker-kid", 0));
        tokio::time::sleep(Duration::from_secs(11)).await;
        assert_eq!(n.load(Ordering::SeqCst), 2);
        handle.shutdown();
        join.await.unwrap();
        for _ in 0..100 {
            trigger.notify(&UnknownKeyEvent::new("x", 0));
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }
}
