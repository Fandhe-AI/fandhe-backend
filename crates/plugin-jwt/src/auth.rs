//! 認証器 [`JwtAuthenticator`]（署名検証結果のキャッシュ・未知 `kid` 通知、イシュー #816）。
//!
//! `JwtGate`（`RequestGate` 実装）とアプリのハンドラが同一インスタンスを共有する。
//! `GateOutcome` はクレームを運べない契約のため、ハンドラは本型の
//! [`authenticate`](JwtAuthenticator::authenticate) を呼んでクレームを得る。ゲート通過時に
//! キャッシュが温まっているので、ハンドラ側の呼び出しは署名検証を再実行しない。
//!
//! # キャッシュヒットを許す条件（fail-closed）
//!
//! 1. 検証時点の `Arc<KeySet>` が現行（`SharedKeySet::snapshot`）と `Arc::ptr_eq`
//!    （`SharedKeySet::set` は必ず新しい `Arc` を作るため、差し替え後は全件ミス扱い）
//! 2. ヒット時も `exp`・`nbf` を leeway 付きで毎回再判定する
//! 3. 成功だけをキャッシュする（不正トークンの大量投入でキャッシュを汚さない）
//!
//! キャッシュキーはトークンの SHA-256 で、生トークンは保持しない。容量は
//! [`CACHE_CAPACITY`]。満杯時は期限切れを先に掃き、なお満杯なら FIFO で追い出す。
//! ロックは短時間だけ保持し、署名検証中は保持しない。

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fandhe_backend_http::request::RequestHead;
use ring::digest::{SHA256, digest};
use serde::de::DeserializeOwned;

use crate::jwks::{KeySet, SharedKeySet};
use crate::notify::{
    DEFAULT_UNKNOWN_KEY_MIN_INTERVAL, NotifyThrottle, UnknownKeyEvent, UnknownKeyNotifier,
    call_guarded,
};
use crate::policy::JwtPolicy;
use crate::verify::{Claims, NoExtraClaims, VerifyError, verify_inner};

/// 署名検証結果キャッシュの最大件数。
pub const CACHE_CAPACITY: usize = 1024;

/// `Authorization: Bearer <token>` からトークンを取り出す（RFC 6750）。
///
/// スキーム名は大文字小文字を区別せず、スキームとトークンの間の 1 個以上の SP を許す。
/// トークンが空、スキームが `Bearer` 以外、または `Authorization` ヘッダが複数ある場合は
/// `None`（どちらを採るか曖昧な状態を作らず拒否側へ倒す）。
///
/// ```
/// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// use fandhe_backend_plugin_jwt::bearer_token;
///
/// let raw = b"GET / HTTP/1.1\r\nAuthorization: bearer  abc.def.ghi\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(raw).unwrap() else { panic!() };
/// assert_eq!(bearer_token(&head), Some("abc.def.ghi"));
/// ```
pub fn bearer_token(head: &RequestHead) -> Option<&str> {
    let mut auth = head
        .headers()
        .filter(|(n, _)| n.eq_ignore_ascii_case("authorization"));
    let (_, value) = auth.next()?;
    if auth.next().is_some() {
        return None;
    }
    // `find(' ')` は ASCII 1 バイトの位置のため、スライスは常に char 境界。
    let scheme_end = value.find(' ')?;
    if !value[..scheme_end].eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = value[scheme_end..].trim_start_matches(' ');
    if token.is_empty() { None } else { Some(token) }
}

type CacheKey = [u8; 32];

fn cache_key(token: &str) -> CacheKey {
    let d = digest(&SHA256, token.as_bytes());
    let mut k = [0u8; 32];
    k.copy_from_slice(d.as_ref());
    k
}

struct Entry<E> {
    claims: Claims<E>,
    verified_against: Arc<KeySet>,
}

struct CacheState<E> {
    map: HashMap<CacheKey, Entry<E>>,
    /// 挿入順（FIFO 追い出し用）。`map` と要素数を常に一致させる。
    order: VecDeque<CacheKey>,
}

struct TokenCache<E> {
    state: RwLock<CacheState<E>>,
}

/// ヒット時の再判定結果。
enum Lookup<E> {
    Hit(Claims<E>),
    Invalid(VerifyError),
    Miss,
}

impl<E: Clone> TokenCache<E> {
    fn new() -> Self {
        Self {
            state: RwLock::new(CacheState {
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    /// 鍵集合が現行と同一で、時刻の再判定に通ったときだけ `Hit`。
    /// 判定順は `verify` と同じ（`exp`、次に `nbf`）。
    fn get(&self, key: &CacheKey, current: &Arc<KeySet>, now: u64, leeway: u64) -> Lookup<E> {
        let g = self.state.read().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = g.map.get(key) else {
            return Lookup::Miss;
        };
        if !Arc::ptr_eq(&entry.verified_against, current) {
            return Lookup::Miss;
        }
        let c = &entry.claims;
        if now.saturating_sub(leeway) >= c.exp() {
            return Lookup::Invalid(VerifyError::Expired);
        }
        if let Some(nbf) = c.nbf()
            && now.saturating_add(leeway) < nbf
        {
            return Lookup::Invalid(VerifyError::NotYetValid);
        }
        Lookup::Hit(c.clone())
    }

    fn remove(&self, key: &CacheKey) {
        let mut g = self.state.write().unwrap_or_else(|e| e.into_inner());
        if g.map.remove(key).is_some() {
            g.order.retain(|k| k != key);
        }
    }

    fn insert(&self, key: CacheKey, claims: Claims<E>, keys: Arc<KeySet>, now: u64, leeway: u64) {
        let mut g = self.state.write().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = g.map.get_mut(&key) {
            e.claims = claims;
            e.verified_against = keys;
            return;
        }
        if g.map.len() >= CACHE_CAPACITY {
            let CacheState { map, order } = &mut *g;
            map.retain(|_, e| now.saturating_sub(leeway) < e.claims.exp());
            order.retain(|k| map.contains_key(k));
            while g.map.len() >= CACHE_CAPACITY {
                let Some(oldest) = g.order.pop_front() else {
                    break;
                };
                g.map.remove(&oldest);
            }
        }
        g.order.push_back(key);
        g.map.insert(
            key,
            Entry {
                claims,
                verified_against: keys,
            },
        );
    }
}

struct Inner<E> {
    keys: SharedKeySet,
    policy: JwtPolicy,
    cache: TokenCache<E>,
    notifier: Option<Box<dyn UnknownKeyNotifier>>,
    throttle: NotifyThrottle,
    hits: AtomicU64,
    misses: AtomicU64,
}

/// 署名検証結果をキャッシュする認証器。`Clone` は `Arc` 共有。
///
/// ```
/// use fandhe_backend_plugin_jwt::*;
/// let policy = JwtPolicy::new(
///     Issuer::new("https://issuer.example")?,
///     Audiences::single("api")?,
///     AllowedAlgorithms::only(Algorithm::Es256),
///     TokenType::AccessToken,
/// );
/// let auth = JwtAuthenticator::<NoExtraClaims>::builder(SharedKeySet::new(KeySet::empty()), policy)
///     .unknown_key_notifier(|e: &UnknownKeyEvent<'_>| { let _ = e.kid(); })
///     .build();
/// assert_eq!(auth.cache_hits(), 0);
/// # Ok::<(), PolicyError>(())
/// ```
pub struct JwtAuthenticator<E = NoExtraClaims> {
    inner: Arc<Inner<E>>,
}

impl<E> Clone for JwtAuthenticator<E> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// 統計値だけを出す（クレーム・キー・トークンは出さない）。
impl<E> fmt::Debug for JwtAuthenticator<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtAuthenticator")
            .field("cache_hits", &self.inner.hits.load(Ordering::Relaxed))
            .field("cache_misses", &self.inner.misses.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// [`JwtAuthenticator`] の構築器。
pub struct JwtAuthenticatorBuilder<E> {
    keys: SharedKeySet,
    policy: JwtPolicy,
    notifier: Option<Box<dyn UnknownKeyNotifier>>,
    min_interval: Duration,
    _extra: PhantomData<fn() -> E>,
}

impl<E> JwtAuthenticatorBuilder<E> {
    /// 未知 `kid` の通知口を登録する（契約は [`UnknownKeyNotifier`]）。
    pub fn unknown_key_notifier(mut self, n: impl UnknownKeyNotifier + 'static) -> Self {
        self.notifier = Some(Box::new(n));
        self
    }

    /// 通知の最小間隔（秒精度、既定 [`DEFAULT_UNKNOWN_KEY_MIN_INTERVAL`]）。
    /// 1 秒未満は 0 秒として扱う。0 にすると未知 `kid` のたびに通知される。
    pub fn unknown_key_min_interval(mut self, d: Duration) -> Self {
        self.min_interval = d;
        self
    }

    /// 認証器を作る。
    pub fn build(self) -> JwtAuthenticator<E>
    where
        E: DeserializeOwned + Clone + Send + Sync + 'static,
    {
        JwtAuthenticator {
            inner: Arc::new(Inner {
                keys: self.keys,
                policy: self.policy,
                cache: TokenCache::new(),
                notifier: self.notifier,
                throttle: NotifyThrottle::new(self.min_interval),
                hits: AtomicU64::new(0),
                misses: AtomicU64::new(0),
            }),
        }
    }
}

impl<E: DeserializeOwned + Clone + Send + Sync + 'static> JwtAuthenticator<E> {
    /// 構築器を作る。ポリシーは構築後に変更できない（キャッシュは 1 ポリシーに束縛される）。
    pub fn builder(keys: SharedKeySet, policy: JwtPolicy) -> JwtAuthenticatorBuilder<E> {
        JwtAuthenticatorBuilder {
            keys,
            policy,
            notifier: None,
            min_interval: DEFAULT_UNKNOWN_KEY_MIN_INTERVAL,
            _extra: PhantomData,
        }
    }

    /// `Authorization: Bearer` のトークンを検証してクレームを返す。
    pub fn authenticate(&self, head: &RequestHead) -> Result<Claims<E>, VerifyError> {
        // 時計異常時は u64::MAX を渡し、正の exp を常に期限切れ側へ倒す（フェイルクローズ）。
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(u64::MAX);
        self.authenticate_at(head, now)
    }

    /// [`authenticate`](Self::authenticate) の時刻注入版。
    pub fn authenticate_at(
        &self,
        head: &RequestHead,
        now_unix: u64,
    ) -> Result<Claims<E>, VerifyError> {
        let inner = &*self.inner;
        let token = bearer_token(head).ok_or(VerifyError::MissingToken)?;
        let key = cache_key(token);
        let current = inner.keys.snapshot();
        let leeway = inner.policy.clock_skew().as_secs();

        match inner.cache.get(&key, &current, now_unix, leeway) {
            Lookup::Hit(c) => {
                inner.hits.fetch_add(1, Ordering::Relaxed);
                return Ok(c);
            }
            Lookup::Invalid(e) => {
                inner.cache.remove(&key);
                return Err(e);
            }
            Lookup::Miss => {}
        }
        inner.cache.remove(&key);
        inner.misses.fetch_add(1, Ordering::Relaxed);

        let mut unknown: Option<String> = None;
        let result = verify_inner::<E, _>(token, &current, &inner.policy, now_unix, |kid| {
            unknown = Some(kid.to_owned());
        });
        // 判定が確定した後に通知する。通知口の遅延・panic は判定結果を変えない。
        if let (Some(kid), Some(n)) = (unknown, inner.notifier.as_deref())
            && let Some(suppressed) = inner.throttle.try_acquire(now_unix)
        {
            call_guarded(n, &UnknownKeyEvent::new(&kid, suppressed));
        }
        let claims = result?;
        inner
            .cache
            .insert(key, claims.clone(), current, now_unix, leeway);
        Ok(claims)
    }

    /// キャッシュヒット件数（計測・テスト用）。
    pub fn cache_hits(&self) -> u64 {
        self.inner.hits.load(Ordering::Relaxed)
    }

    /// キャッシュミス件数（計測・テスト用）。
    pub fn cache_misses(&self) -> u64 {
        self.inner.misses.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    fn head(raw: &str) -> RequestHead {
        match parse_request_head(raw.as_bytes()).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => unreachable!(),
        }
    }

    #[test]
    fn bearer_cases() {
        let h = |v: &str| head(&format!("GET / HTTP/1.1\r\nAuthorization: {v}\r\n\r\n"));
        assert_eq!(bearer_token(&h("Bearer abc")), Some("abc"));
        assert_eq!(bearer_token(&h("BEARER   abc")), Some("abc"));
        assert_eq!(bearer_token(&h("Bearer ")), None);
        assert_eq!(bearer_token(&h("Bearer")), None);
        assert_eq!(bearer_token(&h("Basic abc")), None);
        assert_eq!(bearer_token(&head("GET / HTTP/1.1\r\n\r\n")), None);
        let dup =
            head("GET / HTTP/1.1\r\nAuthorization: Bearer a\r\nAuthorization: Bearer b\r\n\r\n");
        assert_eq!(bearer_token(&dup), None);
    }

    #[test]
    fn empty_cache_misses_and_is_consistent() {
        let cache = TokenCache::<NoExtraClaims>::new();
        let keys = Arc::new(KeySet::empty());
        assert!(matches!(cache.get(&[0; 32], &keys, 0, 0), Lookup::Miss));
        let g = cache.state.read().unwrap();
        assert_eq!(g.map.len(), g.order.len());
    }
}
