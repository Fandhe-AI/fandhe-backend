//! `jwks-refresh` の取り直しタスクの統合テスト（#895）。
//!
//! tokio の仮想時間（`start_paused`）で周期・`max-age`・下限・時間切れ・失敗時の保持・
//! `max_stale`・停止を決定的に検証する。鍵はテスト専用に実行時生成する ES256。
#![cfg(feature = "jwks-refresh")]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_plugin_jwt::*;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, sleep_until};

const ISS: &str = "https://issuer.example";
const NOW: u64 = 1_000_000;

struct Ec(EcdsaKeyPair);

impl Ec {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let pk8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        Self(
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pk8.as_ref(), &rng).unwrap(),
        )
    }
    fn jwks(&self, kid: &str) -> String {
        let pt = self.0.public_key().as_ref();
        format!(
            r#"{{"keys":[{{"kty":"EC","kid":"{kid}","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}]}}"#,
            B.encode(&pt[1..33]),
            B.encode(&pt[33..65])
        )
    }
    fn keyset(&self, kid: &str) -> KeySet {
        KeySet::from_json(&self.jwks(kid)).unwrap()
    }
    fn token(&self, kid: &str) -> String {
        let h = format!(r#"{{"alg":"ES256","kid":"{kid}","typ":"at+jwt"}}"#);
        let b = format!(r#"{{"iss":"{ISS}","aud":"api","exp":{}}}"#, NOW + 100);
        let msg = format!("{}.{}", B.encode(h), B.encode(b));
        let sig = self.0.sign(&SystemRandom::new(), msg.as_bytes()).unwrap();
        format!("{msg}.{}", B.encode(sig.as_ref()))
    }
}

fn head(token: &str) -> RequestHead {
    let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n");
    match parse_request_head(raw.as_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    }
}

fn authenticator(shared: &SharedKeySet) -> JwtAuthenticator {
    let policy = JwtPolicy::new(
        Issuer::new(ISS).unwrap(),
        Audiences::single("api").unwrap(),
        AllowedAlgorithms::only(Algorithm::Es256),
        TokenType::AccessToken,
    );
    JwtAuthenticator::builder(shared.clone(), policy).build()
}

fn accepts(a: &JwtAuthenticator, ec: &Ec, kid: &str) -> bool {
    a.authenticate_at(&head(&ec.token(kid)), NOW).is_ok()
}

#[derive(Clone)]
enum Step {
    Ok(String, Option<Duration>),
    Err(FetchError),
    Hang,
    Panic,
    Slow(Duration, String),
}

struct DropFlag(Arc<AtomicUsize>);
impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// 台本どおりに応える取得関数。台本が尽きたら最後の手順を繰り返す。
struct Scripted {
    steps: VecDeque<Step>,
    t0: Instant,
    starts: Arc<Mutex<Vec<Duration>>>,
    drops: Arc<AtomicUsize>,
}

impl JwksFetcher for Scripted {
    fn fetch(&self) -> FetchFuture<'_> {
        let i = {
            let mut s = self.starts.lock().unwrap();
            s.push(Instant::now() - self.t0);
            s.len() - 1
        };
        let step = self.steps[i.min(self.steps.len() - 1)].clone();
        let drops = Arc::clone(&self.drops);
        Box::pin(async move {
            match step {
                Step::Ok(b, m) => Ok(JwksResponse::new(b, m)),
                Step::Err(e) => Err(e),
                Step::Hang => {
                    let _g = DropFlag(drops);
                    std::future::pending().await
                }
                Step::Panic => panic!("テスト用の panic"),
                Step::Slow(d, b) => {
                    sleep(d).await;
                    Ok(JwksResponse::new(b, None))
                }
            }
        })
    }
}

struct Fx {
    t0: Instant,
    starts: Arc<Mutex<Vec<Duration>>>,
    drops: Arc<AtomicUsize>,
    events: Arc<Mutex<Vec<RefreshEvent>>>,
    handle: RefreshHandle,
    join: JoinHandle<()>,
}

impl Fx {
    fn start(shared: SharedKeySet, cfg: RefreshConfig, steps: Vec<Step>) -> Self {
        let events: Arc<Mutex<Vec<RefreshEvent>>> = Arc::default();
        let e = Arc::clone(&events);
        Self::start_with(shared, cfg, steps, events, move |ev: &RefreshEvent| {
            e.lock().unwrap().push(*ev);
        })
    }

    fn start_with(
        shared: SharedKeySet,
        cfg: RefreshConfig,
        steps: Vec<Step>,
        events: Arc<Mutex<Vec<RefreshEvent>>>,
        observer: impl RefreshObserver + 'static,
    ) -> Self {
        let t0 = Instant::now();
        let starts: Arc<Mutex<Vec<Duration>>> = Arc::default();
        let drops: Arc<AtomicUsize> = Arc::default();
        let fetcher = Scripted {
            steps: steps.into(),
            t0,
            starts: Arc::clone(&starts),
            drops: Arc::clone(&drops),
        };
        let (task, handle) = JwksRefresher::with_observer(shared.clone(), fetcher, cfg, observer);
        Self {
            t0,
            starts,
            drops,
            events,
            handle,
            join: tokio::spawn(task),
        }
    }

    /// 起動から `ms` ミリ秒後まで時間を進める（同時刻の競合を避けるため半端な値で呼ぶ）。
    async fn until(&self, ms: u64) {
        sleep_until(self.t0 + Duration::from_millis(ms)).await;
        tokio::task::yield_now().await;
    }

    fn starts(&self) -> Vec<u64> {
        self.starts
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect()
    }

    fn events(&self) -> Vec<RefreshEvent> {
        self.events.lock().unwrap().clone()
    }
}

fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn ok(ec: &Ec, kid: &str) -> Step {
    Step::Ok(ec.jwks(kid), None)
}

fn empty_ok(max_age: Option<Duration>) -> Step {
    Step::Ok(r#"{"keys":[]}"#.to_owned(), max_age)
}

fn cfg(period: u64) -> RefreshConfig {
    RefreshConfig::new(s(period)).unwrap()
}

// ---- AC1: 周期・max-age・下限 ----

#[tokio::test(start_paused = true)]
async fn fetches_on_period() {
    let fx = Fx::start(
        SharedKeySet::new(KeySet::empty()),
        cfg(60),
        vec![empty_ok(None)],
    );
    fx.until(125_500).await;
    assert_eq!(fx.starts(), [0, 60, 120]);
}

#[tokio::test(start_paused = true)]
async fn max_age_shorter_than_period_shortens_interval() {
    let fx = Fx::start(
        SharedKeySet::new(KeySet::empty()),
        cfg(60),
        vec![empty_ok(Some(s(30)))],
    );
    fx.until(65_500).await;
    assert_eq!(fx.starts(), [0, 30, 60]);
}

#[tokio::test(start_paused = true)]
async fn max_age_below_min_interval_is_clamped() {
    for max_age in [s(1), Duration::ZERO] {
        let fx = Fx::start(
            SharedKeySet::new(KeySet::empty()),
            cfg(60),
            vec![empty_ok(Some(max_age))],
        );
        fx.until(25_500).await;
        assert_eq!(fx.starts(), [0, 10, 20]);
    }
}

#[tokio::test(start_paused = true)]
async fn max_age_longer_than_period_uses_period() {
    let fx = Fx::start(
        SharedKeySet::new(KeySet::empty()),
        cfg(60),
        vec![empty_ok(Some(Duration::from_secs(u64::MAX / 4)))],
    );
    fx.until(65_500).await;
    assert_eq!(fx.starts(), [0, 60]);
}

// ---- AC2: 失敗 3 種で現行を保ち、下限の間隔で再試行 ----

#[tokio::test(start_paused = true)]
async fn fetch_error_keeps_keys_and_retries_at_min_interval() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let before = shared.snapshot();
    let fx = Fx::start(
        shared.clone(),
        cfg(60),
        vec![Step::Err(FetchError::Transport)],
    );
    fx.until(25_500).await;
    assert_eq!(fx.starts(), [0, 10, 20]);
    assert!(Arc::ptr_eq(&before, &shared.snapshot()));
    assert_eq!(fx.handle.status().consecutive_failures(), 3);
    assert_eq!(
        fx.events(),
        vec![RefreshEvent::Failed(RefreshFailure::Fetch(FetchError::Transport)); 3]
    );
}

#[tokio::test(start_paused = true)]
async fn timeout_keeps_keys_and_retries_at_min_interval() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let before = shared.snapshot();
    let fx = Fx::start(shared.clone(), cfg(60), vec![Step::Hang]);
    fx.until(25_500).await;
    // 時間切れ（5 秒）は開始時刻から数えるため、次の開始は 15 秒ではなく 10 秒。
    assert_eq!(fx.starts(), [0, 10, 20]);
    assert!(Arc::ptr_eq(&before, &shared.snapshot()));
    // 3 回目（20 秒開始）は 25 秒に時間切れ。取得 future は drop される。
    assert_eq!(fx.drops.load(Ordering::SeqCst), 3);
    assert_eq!(
        fx.events(),
        vec![RefreshEvent::Failed(RefreshFailure::Timeout); 3]
    );
}

#[tokio::test(start_paused = true)]
async fn parse_error_keeps_keys_and_retries_at_min_interval() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let before = shared.snapshot();
    let steps = vec![
        Step::Ok("{".to_owned(), None),
        // 空でないのに採用できる鍵が 0 件の JWKS（ゲートと同じ検査で弾かれる）。
        Step::Ok(
            r#"{"keys":[{"kty":"oct","kid":"x","k":"AA"}]}"#.to_owned(),
            None,
        ),
    ];
    let fx = Fx::start(shared.clone(), cfg(60), steps);
    fx.until(25_500).await;
    assert_eq!(fx.starts(), [0, 10, 20]);
    assert!(Arc::ptr_eq(&before, &shared.snapshot()));
    let ev = fx.events();
    assert_eq!(ev.len(), 3);
    assert!(
        ev.iter()
            .all(|e| matches!(e, RefreshEvent::Failed(RefreshFailure::Parse(_))))
    );
}

#[tokio::test(start_paused = true)]
async fn panic_in_fetcher_is_a_failure_and_task_survives() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k1"));
    let fx = Fx::start(shared.clone(), cfg(60), vec![Step::Panic, ok(&ec, "k2")]);
    fx.until(15_500).await;
    assert_eq!(fx.starts(), [0, 10]);
    assert_eq!(
        fx.events(),
        vec![
            RefreshEvent::Failed(RefreshFailure::Panicked),
            RefreshEvent::Updated {
                keys: 1,
                skipped: 0
            }
        ]
    );
    assert_eq!(shared.snapshot().keys_for("k2").count(), 1);
    // 失敗の後の成功で連続失敗は 0 に戻り、周期（60 秒）に戻る。
    assert_eq!(fx.handle.status().consecutive_failures(), 0);
    fx.until(75_500).await;
    assert_eq!(fx.starts(), [0, 10, 70]);
}

#[tokio::test(start_paused = true)]
async fn explicit_empty_keys_is_reflected_as_success() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let a = authenticator(&shared);
    assert!(accepts(&a, &ec, "k"));
    let fx = Fx::start(shared.clone(), cfg(60), vec![empty_ok(None)]);
    fx.until(1_500).await;
    assert!(!accepts(&a, &ec, "k"));
    assert_eq!(
        fx.events(),
        vec![RefreshEvent::Updated {
            keys: 0,
            skipped: 0
        }]
    );
}

// ---- AC3: max_stale ----

#[tokio::test(start_paused = true)]
async fn without_max_stale_keys_are_kept_forever() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let before = shared.snapshot();
    let a = authenticator(&shared);
    let fx = Fx::start(shared.clone(), cfg(60), vec![Step::Err(FetchError::Status)]);
    fx.until(2 * 24 * 3600 * 1000 + 500).await;
    assert!(Arc::ptr_eq(&before, &shared.snapshot()));
    assert!(accepts(&a, &ec, "k"));
    assert!(!fx.handle.status().is_cleared());
}

fn stale_cfg(max_stale: u64) -> RefreshConfig {
    RefreshConfig::builder(s(60))
        .max_stale(s(max_stale))
        .build()
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn max_stale_clears_keys_and_rejects_tokens() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(KeySet::empty());
    let a = authenticator(&shared);
    // 0 秒に成功、その後は 60 秒から 10 秒間隔で失敗が続き、180 秒に成功する。
    let mut steps = vec![ok(&ec, "k")];
    steps.extend(std::iter::repeat_n(Step::Err(FetchError::Transport), 12));
    steps.push(ok(&ec, "k"));
    let fx = Fx::start(shared.clone(), stale_cfg(100), steps);
    fx.until(99_500).await;
    assert!(accepts(&a, &ec, "k"));
    assert!(!fx.handle.status().is_cleared());
    fx.until(100_500).await;
    assert!(!accepts(&a, &ec, "k"));
    assert!(fx.handle.status().is_cleared());
    // 空にするのは 1 回だけ（失敗の再試行のたびに再発火しない）。
    fx.until(125_500).await;
    let cleared = fx
        .events()
        .iter()
        .filter(|e| **e == RefreshEvent::Cleared)
        .count();
    assert_eq!(cleared, 1);
    // 次の成功で鍵が戻る。
    fx.until(180_500).await;
    assert!(accepts(&a, &ec, "k"));
    assert!(!fx.handle.status().is_cleared());
}

#[tokio::test(start_paused = true)]
async fn max_stale_counts_from_task_start_when_first_fetch_fails() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(ec.keyset("k"));
    let a = authenticator(&shared);
    let fx = Fx::start(shared, stale_cfg(100), vec![Step::Err(FetchError::Other)]);
    fx.until(99_500).await;
    assert!(accepts(&a, &ec, "k"));
    fx.until(100_500).await;
    assert!(!accepts(&a, &ec, "k"));
}

#[tokio::test(start_paused = true)]
async fn max_stale_fires_during_in_flight_fetch() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(KeySet::empty());
    let a = authenticator(&shared);
    // 0 秒に成功（期限 95 秒）→ 60・70・80 秒に失敗 → 90 秒開始の取得が 98 秒に成功。
    let steps = vec![
        ok(&ec, "k"),
        Step::Err(FetchError::Transport),
        Step::Err(FetchError::Transport),
        Step::Err(FetchError::Transport),
        Step::Slow(Duration::from_secs(8), ec.jwks("k")),
    ];
    // 時間切れを 10 秒にして、8 秒かかる取得が時間切れにならないようにする。
    let cfg = RefreshConfig::builder(s(60))
        .fetch_timeout(s(10))
        .max_stale(s(95))
        .build()
        .unwrap();
    let fx = Fx::start(shared.clone(), cfg, steps);
    fx.until(94_500).await;
    assert!(accepts(&a, &ec, "k"));
    fx.until(96_000).await;
    assert!(!accepts(&a, &ec, "k"));
    fx.until(99_000).await;
    assert!(accepts(&a, &ec, "k"));
    let ev = fx.events();
    assert_eq!(ev[ev.len() - 2], RefreshEvent::Cleared);
    assert_eq!(
        ev[ev.len() - 1],
        RefreshEvent::Updated {
            keys: 1,
            skipped: 0
        }
    );
}

// ---- AC4: 停止 ----

#[tokio::test(start_paused = true)]
async fn dropping_handle_stops_fetching() {
    let fx = Fx::start(
        SharedKeySet::new(KeySet::empty()),
        cfg(60),
        vec![empty_ok(None)],
    );
    fx.until(500).await;
    let Fx {
        starts,
        join,
        handle,
        t0,
        ..
    } = fx;
    drop(handle);
    tokio::time::timeout(s(1), join).await.unwrap().unwrap();
    sleep_until(t0 + s(600)).await;
    assert_eq!(starts.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn shutdown_stops_fetching() {
    let fx = Fx::start(
        SharedKeySet::new(KeySet::empty()),
        cfg(60),
        vec![empty_ok(None)],
    );
    fx.until(500).await;
    fx.handle.shutdown();
    assert!(fx.handle.status().is_stopped());
    let Fx {
        starts, join, t0, ..
    } = fx;
    tokio::time::timeout(s(1), join).await.unwrap().unwrap();
    sleep_until(t0 + s(600)).await;
    assert_eq!(starts.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn stop_during_in_flight_fetch_drops_fetch_future() {
    let fx = Fx::start(
        SharedKeySet::new(KeySet::empty()),
        cfg(60),
        vec![Step::Hang],
    );
    fx.until(1_000).await;
    assert_eq!(fx.drops.load(Ordering::SeqCst), 0);
    fx.handle.shutdown();
    let Fx { drops, join, .. } = fx;
    tokio::time::timeout(s(1), join).await.unwrap().unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

// ---- 補助: 観測口の panic はタスクを止めない ----

#[tokio::test(start_paused = true)]
async fn observer_panic_does_not_stop_task() {
    let fx = Fx::start_with(
        SharedKeySet::new(KeySet::empty()),
        cfg(60),
        vec![empty_ok(None)],
        Arc::default(),
        |_: &RefreshEvent| panic!("テスト用の panic"),
    );
    fx.until(65_500).await;
    assert_eq!(fx.starts(), [0, 60]);
}

// ---- #896: 未知 kid を契機にした取り直しと、下限によるまとめ ----

/// 合図ハンドルを登録した認証器。`counter` は利用者の通知口の呼び出し回数。
fn triggered_authenticator(
    shared: &SharedKeySet,
    trigger: RefreshTrigger,
    counter: Arc<AtomicUsize>,
) -> JwtAuthenticator {
    let policy = JwtPolicy::new(
        Issuer::new(ISS).unwrap(),
        Audiences::single("api").unwrap(),
        AllowedAlgorithms::only(Algorithm::Es256),
        TokenType::AccessToken,
    );
    JwtAuthenticator::builder(shared.clone(), policy)
        .jwks_refresh_trigger(trigger)
        .unknown_key_notifier(move |_: &UnknownKeyEvent<'_>| {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .build()
}

#[tokio::test(start_paused = true)]
async fn unknown_kid_rejects_immediately_then_refetch_accepts_new_key() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(KeySet::empty());
    let fx = Fx::start(
        shared.clone(),
        cfg(3600),
        vec![ok(&ec, "k1"), ok(&ec, "k2")],
    );
    let auth = triggered_authenticator(&shared, fx.handle.trigger(), Arc::default());
    fx.until(500).await;
    assert_eq!(fx.starts(), [0]);
    // 取り直しを待たず、その場で拒否される（フェイルクローズ）。
    assert!(!accepts(&auth, &ec, "k2"));
    // 合図は最小間隔（10 秒）にそろえられて取得される。
    fx.until(15_500).await;
    assert_eq!(fx.starts(), [0, 10]);
    assert!(accepts(&auth, &ec, "k2"));
}

#[tokio::test(start_paused = true)]
async fn trigger_is_not_throttled_like_user_notifier() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(KeySet::empty());
    let fx = Fx::start(
        shared.clone(),
        cfg(3600),
        vec![ok(&ec, "k1"), ok(&ec, "k2")],
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let auth = triggered_authenticator(&shared, fx.handle.trigger(), Arc::clone(&calls));
    fx.until(500).await;
    // 同じ now_unix で 2 回。利用者の通知口は間引かれて 1 回、合図は毎回送られる。
    assert!(!accepts(&auth, &ec, "evil"));
    assert!(!accepts(&auth, &ec, "k2"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    fx.until(15_500).await;
    assert_eq!(fx.starts(), [0, 10]);
    assert!(accepts(&auth, &ec, "k2"));
}

#[tokio::test(start_paused = true)]
async fn many_signals_within_min_interval_collapse_into_one_fetch() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(KeySet::empty());
    let fx = Fx::start(shared.clone(), cfg(3600), vec![ok(&ec, "k1")]);
    let auth = triggered_authenticator(&shared, fx.handle.trigger(), Arc::default());
    fx.until(1_000).await;
    for _ in 0..1000 {
        assert!(!accepts(&auth, &ec, "evil"));
    }
    fx.until(9_500).await;
    assert_eq!(fx.starts(), [0]);
    fx.until(10_500).await;
    assert_eq!(fx.starts(), [0, 10]);
    fx.until(100_000).await;
    assert_eq!(fx.starts(), [0, 10]);
}

#[tokio::test(start_paused = true)]
async fn signals_during_in_flight_fetch_collapse_into_one_more_fetch() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(KeySet::empty());
    let fx = Fx::start(
        shared.clone(),
        cfg(3600),
        vec![ok(&ec, "k1"), Step::Slow(s(3), ec.jwks("k2"))],
    );
    let auth = triggered_authenticator(&shared, fx.handle.trigger(), Arc::default());
    fx.until(500).await;
    assert!(!accepts(&auth, &ec, "evil"));
    // 10 秒で始まった取得（3 秒かかる）の最中に合図を連打する。
    fx.until(11_500).await;
    assert_eq!(fx.starts(), [0, 10]);
    for _ in 0..1000 {
        assert!(!accepts(&auth, &ec, "evil"));
    }
    // 追加の取得は「取得開始 + 最小間隔」の 1 回だけ。
    fx.until(19_500).await;
    assert_eq!(fx.starts(), [0, 10]);
    fx.until(25_500).await;
    assert_eq!(fx.starts(), [0, 10, 20]);
    fx.until(100_000).await;
    assert_eq!(fx.starts(), [0, 10, 20]);
}

/// 通知はブロックしない。`Notify` は許可を 1 つしか持たずキューがないため、
/// 「満杯で待つ」状態がそもそも存在しない。タスク未 poll・停止後・終了後でも即座に戻る。
#[test]
fn signalling_never_blocks_without_runtime_or_task() {
    struct Never;
    impl JwksFetcher for Never {
        fn fetch(&self) -> FetchFuture<'_> {
            Box::pin(std::future::pending())
        }
    }
    let ec = Ec::new();
    let shared = SharedKeySet::new(KeySet::empty());
    let (task, handle) = JwksRefresher::new(shared.clone(), Never, cfg(3600));
    let auth = triggered_authenticator(&shared, handle.trigger(), Arc::default());
    let tok = head(&ec.token("evil"));
    for _ in 0..10_000 {
        assert!(auth.authenticate_at(&tok, NOW).is_err());
    }
    handle.shutdown();
    for _ in 0..10_000 {
        assert!(auth.authenticate_at(&tok, NOW).is_err());
    }
    assert!(handle.status().is_stopped());
    drop(task);
    for _ in 0..10_000 {
        assert!(auth.authenticate_at(&tok, NOW).is_err());
    }
}

#[tokio::test(start_paused = true)]
async fn signalling_after_task_finished_is_noop() {
    let ec = Ec::new();
    let shared = SharedKeySet::new(KeySet::empty());
    let fx = Fx::start(shared.clone(), cfg(3600), vec![ok(&ec, "k1")]);
    let auth = triggered_authenticator(&shared, fx.handle.trigger(), Arc::default());
    fx.until(500).await;
    fx.handle.shutdown();
    let Fx {
        starts,
        join,
        t0,
        handle,
        ..
    } = fx;
    tokio::time::timeout(s(1), join).await.unwrap().unwrap();
    for _ in 0..1000 {
        assert!(!accepts(&auth, &ec, "evil"));
    }
    sleep_until(t0 + s(100)).await;
    assert_eq!(starts.lock().unwrap().len(), 1);
    assert!(handle.status().is_stopped());
}
