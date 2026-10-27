//! 認証器 [`JwtAuthenticator`]（署名検証結果のキャッシュ・未知 `kid` 通知、イシュー #816）。
//!
//! `JwtGate`（`RequestGate` 実装）が [`authenticate`](JwtAuthenticator::authenticate) を
//! 1 要求につき 1 回呼ぶ。`GateOutcome` はクレームを運べないため、受理した要求のクレームは
//! `JwtGate` が `RequestExtensions` 経由でハンドラへ渡す（`gate` モジュール、`GateVerified`）。
//! ハンドラは本型を呼んで検証し直さない。
//!
//! # キャッシュヒットを許す条件（fail-closed）
//!
//! 1. 検証時点の `Arc<KeySet>` が現行（`SharedKeySet::snapshot`）と `Arc::ptr_eq`
//!    （`SharedKeySet::set` は必ず新しい `Arc` を作るため、差し替え後は全件ミス扱い）
//! 2. ヒット時も `exp`・`nbf` を leeway 付きで毎回再判定する
//! 3. 成功だけをキャッシュする（不正トークンの大量投入でキャッシュを汚さない）
//!
//! # DPoP（`dpop` feature）
//!
//! `JwtAuthenticatorBuilder::dpop` で `dpop::DpopConfig` を設定した
//! 認証器だけが DPoP を検証する。設定しない認証器は従来どおり `Authorization: Bearer` だけを
//! 見る（`Authorization: DPoP` は資格情報なし扱い）。判定は、アクセストークンの検証（キャッシュ
//! を含む）→ `cnf` の解釈 → 方式とスキームによる振り分け → proof の検証 → `cnf.jkt` との
//! サムプリント比較 → `jti` の登録、の順（設計 9.8 節）。proof の署名はキャッシュせず毎回検証する。
//! DPoP を設定した場合、`authenticate` は `jti` の登録を伴うため
//! 1 要求につきゲートが 1 回だけ呼ぶ。`jti` の履歴はこの重複登録の検出だけに使い、ゲートを
//! 通った証明には使わない。
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

#[cfg(feature = "dpop")]
use crate::dpop::{DpopConfig, DpopError, DpopMode, ProofContext, ReplayStore, verify_proof};
use crate::jwks::{KeySet, SharedKeySet};
use crate::notify::{
    DEFAULT_UNKNOWN_KEY_MIN_INTERVAL, NotifyThrottle, UnknownKeyEvent, UnknownKeyNotifier,
    call_guarded,
};
#[cfg(feature = "dpop")]
use crate::policy::Algorithm;
use crate::policy::JwtPolicy;
use crate::verify::{Claims, NoExtraClaims, VerifyError, verify_inner};
use fandhe_backend_http::response::Response;

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
    match authorization(head)? {
        (scheme, token) if scheme.eq_ignore_ascii_case("bearer") => Some(token),
        _ => None,
    }
}

/// `Authorization` をちょうど 1 つ持つときに `(スキーム名, 空でない資格情報)` へ分ける。
/// 複数ヘッダ・スキームと資格情報の区切りなし・資格情報が空は `None`（拒否側へ倒す）。
fn authorization(head: &RequestHead) -> Option<(&str, &str)> {
    let mut auth = head
        .headers()
        .filter(|(n, _)| n.eq_ignore_ascii_case("authorization"));
    let (_, value) = auth.next()?;
    if auth.next().is_some() {
        return None;
    }
    // `find(' ')` は ASCII 1 バイトの位置のため、スライスは常に char 境界。
    let scheme_end = value.find(' ')?;
    let token = value[scheme_end..].trim_start_matches(' ');
    if token.is_empty() {
        None
    } else {
        Some((&value[..scheme_end], token))
    }
}

/// 提示された資格情報のスキーム（`dpop` を設定した認証器だけが `DPoP` を認める）。
#[cfg(feature = "dpop")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scheme {
    Bearer,
    Dpop,
}

#[cfg(feature = "dpop")]
fn credentials(head: &RequestHead) -> Option<(Scheme, &str)> {
    let (scheme, token) = authorization(head)?;
    if scheme.eq_ignore_ascii_case("bearer") {
        Some((Scheme::Bearer, token))
    } else if scheme.eq_ignore_ascii_case("dpop") {
        Some((Scheme::Dpop, token))
    } else {
        None
    }
}

/// 401 の固定本文（Bearer のみの経路と、アクセストークン側の拒否）。
const BODY_INVALID_TOKEN: &[u8] = br#"{"error":"invalid_token"}"#;
/// proof 側の拒否の固定本文。
#[cfg(feature = "dpop")]
const BODY_INVALID_DPOP_PROOF: &[u8] = br#"{"error":"invalid_dpop_proof"}"#;
/// トークンが無い場合（RFC 6750 3.1: 認証情報なしの要求には `error` を付けない）。
const CHALLENGE_BEARER_NO_TOKEN: &str = "Bearer";
/// トークンはあるが検証に失敗した場合。`error_description` は付けない。
const CHALLENGE_BEARER_INVALID: &str = "Bearer error=\"invalid_token\"";

/// 401 拒否応答。ヘッダ付与に失敗した場合もヘッダ無しの 401 に倒し、`Allow` にはしない。
fn unauthorized(body: &'static [u8], challenges: &[&str]) -> Response {
    let base = Response::new(401, body.to_vec()).with_content_type("application/json");
    let mut r = base.clone();
    for c in challenges {
        match r.with_header("WWW-Authenticate", *c) {
            Ok(next) => r = next,
            Err(_) => return base,
        }
    }
    r
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
    /// `cnf` を持つ `Claims` は大きいため Box 化する（`large_enum_variant`）。
    Hit(Box<Claims<E>>),
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
        Lookup::Hit(Box::new(c.clone()))
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

/// DPoP の設定と、それに紐づく状態（`jti` の保持器・チャレンジの `algs` 値）。
#[cfg(feature = "dpop")]
struct DpopState {
    config: DpopConfig,
    replay: ReplayStore,
    /// `WWW-Authenticate` の `algs` 値。構築時に 1 回だけ正準順（`ES256 RS256`）で作り、
    /// リクエスト由来の値は入れない。
    challenge_algs: Box<str>,
}

struct Inner<E> {
    keys: SharedKeySet,
    policy: JwtPolicy,
    cache: TokenCache<E>,
    notifier: Option<Box<dyn UnknownKeyNotifier>>,
    #[cfg(feature = "jwks-refresh")]
    refresh_trigger: Option<crate::refresh::RefreshTrigger>,
    #[cfg(feature = "dpop")]
    dpop: Option<DpopState>,
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
    #[cfg(feature = "jwks-refresh")]
    refresh_trigger: Option<crate::refresh::RefreshTrigger>,
    #[cfg(feature = "dpop")]
    dpop: Option<DpopConfig>,
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

    /// 未知 `kid` を契機に JWKS を取り直す合図ハンドルを登録する（枠は 1 つ。再度呼ぶと後勝ち）。
    ///
    /// 対象 feature: `jwks-refresh`。通知の間引き（`NotifyThrottle`）を通さず、
    /// 未知 `kid` のたびに [`RefreshTrigger`](crate::RefreshTrigger) へ合図する
    /// （まとめはタスク側の最小間隔が担う）。合図は判定確定後に同期・非ブロックで送られ、
    /// 未知 `kid` のトークンは取り直しを待たず従来どおりその場で拒否される。
    /// `kid` は合図の経路へ渡らない。
    ///
    /// ```
    /// # use fandhe_backend_plugin_jwt::*;
    /// # struct F;
    /// # impl JwksFetcher for F {
    /// #     fn fetch(&self) -> FetchFuture<'_> { Box::pin(async { Ok(JwksResponse::new(String::new(), None)) }) }
    /// # }
    /// # let policy = JwtPolicy::new(
    /// #     Issuer::new("https://issuer.example")?,
    /// #     Audiences::single("api")?,
    /// #     AllowedAlgorithms::only(Algorithm::Es256),
    /// #     TokenType::AccessToken,
    /// # );
    /// let keys = SharedKeySet::new(KeySet::empty());
    /// let cfg = RefreshConfig::new(std::time::Duration::from_secs(60)).unwrap();
    /// let (_task, handle) = JwksRefresher::new(keys.clone(), F, cfg);
    /// let _auth = JwtAuthenticator::<NoExtraClaims>::builder(keys, policy)
    ///     .jwks_refresh_trigger(handle.trigger())
    ///     .build();
    /// # Ok::<(), PolicyError>(())
    /// ```
    #[cfg(feature = "jwks-refresh")]
    #[must_use]
    pub fn jwks_refresh_trigger(mut self, t: crate::refresh::RefreshTrigger) -> Self {
        self.refresh_trigger = Some(t);
        self
    }

    /// DPoP の検証を有効にする（枠は 1 つ。再度呼ぶと後勝ち）。
    ///
    /// 対象 feature: `dpop`。設定した認証器は `Authorization: DPoP` と `DPoP` ヘッダを読み、
    /// `DpopConfig` の方式に従ってアクセストークンと proof を検査する（判定の流れは
    /// 本モジュールの doc）。設定しない認証器は従来どおり Bearer だけを見て、`cnf` を
    /// 持つトークンも bearer として受理する点に注意（DPoP を強制したい下流は必ず設定する）。
    ///
    /// ```
    /// # use fandhe_backend_plugin_jwt::*;
    /// use fandhe_backend_plugin_jwt::dpop::{DpopConfig, DpopMode, PublicOrigin};
    /// # let policy = JwtPolicy::new(
    /// #     Issuer::new("https://issuer.example")?,
    /// #     Audiences::single("api")?,
    /// #     AllowedAlgorithms::only(Algorithm::Es256),
    /// #     TokenType::AccessToken,
    /// # );
    /// let cfg = DpopConfig::new(
    ///     DpopMode::Required,
    ///     PublicOrigin::new("https://api.example").unwrap(),
    ///     AllowedAlgorithms::only(Algorithm::Es256),
    /// );
    /// let _auth = JwtAuthenticator::<NoExtraClaims>::builder(SharedKeySet::new(KeySet::empty()), policy)
    ///     .dpop(cfg)
    ///     .build();
    /// # Ok::<(), PolicyError>(())
    /// ```
    #[cfg(feature = "dpop")]
    #[must_use]
    pub fn dpop(mut self, cfg: DpopConfig) -> Self {
        self.dpop = Some(cfg);
        self
    }

    /// 認証器を作る。
    pub fn build(self) -> JwtAuthenticator<E>
    where
        E: DeserializeOwned + Clone + Send + Sync + 'static,
    {
        #[cfg(feature = "dpop")]
        let dpop = self.dpop.map(|config| {
            // 容量は `DpopConfig` が構築時に検証済みで、ここでの失敗は起きない。
            // 起きても panic させず既定の保持器に倒す。
            let window = config.iat_window();
            let replay = match config.replay_capacity() {
                Some(c) => ReplayStore::with_capacity(window, c)
                    .unwrap_or_else(|_| ReplayStore::new(window)),
                None => ReplayStore::new(window),
            };
            let challenge_algs = [Algorithm::Es256, Algorithm::Rs256]
                .into_iter()
                .filter(|a| config.algorithms().contains(*a))
                .map(Algorithm::as_jose_name)
                .collect::<Vec<_>>()
                .join(" ")
                .into_boxed_str();
            DpopState {
                config,
                replay,
                challenge_algs,
            }
        });
        JwtAuthenticator {
            inner: Arc::new(Inner {
                keys: self.keys,
                policy: self.policy,
                cache: TokenCache::new(),
                notifier: self.notifier,
                #[cfg(feature = "jwks-refresh")]
                refresh_trigger: self.refresh_trigger,
                #[cfg(feature = "dpop")]
                dpop,
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
            #[cfg(feature = "jwks-refresh")]
            refresh_trigger: None,
            #[cfg(feature = "dpop")]
            dpop: None,
            min_interval: DEFAULT_UNKNOWN_KEY_MIN_INTERVAL,
            _extra: PhantomData,
        }
    }

    /// `Authorization: Bearer` のトークンを検証してクレームを返す。
    ///
    /// DPoP を設定した認証器（`dpop` feature）では `Authorization: DPoP` も受け、proof の
    /// 検証と `jti` の登録まで行う。登録を伴うため、**1 要求につきゲートで 1 回だけ**呼ぶこと
    /// （同じ要求でもう一度呼ぶと `Replayed` で失敗する）。ハンドラは呼ばず、`JwtGate` が
    /// `RequestExtensions` に入れた検証結果を `JwtGate::verified` で読む。
    pub fn authenticate(&self, head: &RequestHead) -> Result<Claims<E>, VerifyError> {
        self.authenticate_at(head, unix_now())
    }

    /// [`authenticate`](Self::authenticate) の時刻注入版。
    pub fn authenticate_at(
        &self,
        head: &RequestHead,
        now_unix: u64,
    ) -> Result<Claims<E>, VerifyError> {
        #[cfg(feature = "dpop")]
        if let Some(d) = self.inner.dpop.as_ref() {
            return self.authenticate_dpop(d, head, now_unix);
        }
        let token = bearer_token(head).ok_or(VerifyError::MissingToken)?;
        self.verify_token_cached(token, now_unix)
    }

    /// アクセストークンの検証（署名検証結果のキャッシュを含む）。スキームの解釈は呼び出し側。
    fn verify_token_cached(&self, token: &str, now_unix: u64) -> Result<Claims<E>, VerifyError> {
        let inner = &*self.inner;
        let key = cache_key(token);
        let current = inner.keys.snapshot();
        let leeway = inner.policy.clock_skew().as_secs();

        match inner.cache.get(&key, &current, now_unix, leeway) {
            Lookup::Hit(c) => {
                inner.hits.fetch_add(1, Ordering::Relaxed);
                return Ok(*c);
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
        // 取り直しの合図は間引きを通さない（まとめはタスク側の最小間隔が担う）。`kid` は渡さない。
        #[cfg(feature = "jwks-refresh")]
        if unknown.is_some()
            && let Some(t) = inner.refresh_trigger.as_ref()
        {
            t.fire();
        }
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

    /// DPoP を設定した認証器の判定（設計 9.8 節の表）。
    #[cfg(feature = "dpop")]
    fn authenticate_dpop(
        &self,
        d: &DpopState,
        head: &RequestHead,
        now_unix: u64,
    ) -> Result<Claims<E>, VerifyError> {
        use crate::verify::Confirmation;

        let (scheme, token) = credentials(head).ok_or(VerifyError::MissingToken)?;
        let claims = self.verify_token_cached(token, now_unix)?;
        let jkt: Option<Box<str>> = match claims.confirmation() {
            Confirmation::Absent => None,
            Confirmation::Jkt(j) if is_thumbprint_shape(j) => Some(j.clone()),
            // `jkt` として使えない `cnf`（`x5t#S256` のみ・型違い・不正な形式）は結合を
            // 確認できないので拒否する。
            _ => return Err(VerifyError::Malformed),
        };
        let jkt = match (scheme, jkt, d.config.mode()) {
            (Scheme::Bearer, Some(_), _) => {
                return Err(DpopError::BoundTokenAsBearer.into());
            }
            (Scheme::Bearer, None, DpopMode::AllowUnboundBearer) => return Ok(claims),
            (Scheme::Bearer, None, DpopMode::Required) | (Scheme::Dpop, None, _) => {
                return Err(DpopError::UnboundToken.into());
            }
            (Scheme::Dpop, Some(j), _) => j,
        };

        let mut proofs = head
            .headers()
            .filter(|(n, _)| n.eq_ignore_ascii_case("dpop"))
            .map(|(_, v)| v);
        let proof = proofs.next().ok_or(DpopError::MissingProof)?;
        if proofs.next().is_some() {
            return Err(DpopError::MultipleProofs.into());
        }
        let htu = d.config.origin().expected_htu(head.path())?;
        let ctx = ProofContext::new(
            head.method(),
            &htu,
            token,
            d.config.algorithms(),
            d.config.iat_window(),
        );
        let verified = verify_proof(proof, &ctx, now_unix)?;
        // `jkt` は公開値で秘密ではないため、定数時間比較は不要。
        if verified.thumbprint().as_str() != &*jkt {
            return Err(DpopError::ThumbprintMismatch.into());
        }
        d.replay.register(&verified, now_unix)?;
        Ok(claims)
    }

    /// 検証失敗に対する 401 応答を作る（`JwtGate` と、同じ認証器を使う他のゲートが共有する）。
    ///
    /// 本文は固定 JSON、`WWW-Authenticate` は固定文字列と構築時に作った `algs` だけで組み立て、
    /// リクエスト由来の値・失敗理由の詳細は含めない。DPoP を設定していなければ従来どおり
    /// `Bearer` の 1 行。ヘッダの付与に失敗した場合もヘッダ無しの 401 に倒す。
    ///
    /// ```
    /// use fandhe_backend_plugin_jwt::*;
    /// # use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    /// # let policy = JwtPolicy::new(
    /// #     Issuer::new("https://issuer.example")?,
    /// #     Audiences::single("api")?,
    /// #     AllowedAlgorithms::only(Algorithm::Es256),
    /// #     TokenType::AccessToken,
    /// # );
    /// # let ParseOutcome::Complete { head, .. } = parse_request_head(b"GET / HTTP/1.1\r\n\r\n").unwrap() else { panic!() };
    /// let auth = JwtAuthenticator::<NoExtraClaims>::builder(SharedKeySet::new(KeySet::empty()), policy).build();
    /// let r = auth.unauthorized_response(&head, &VerifyError::MissingToken);
    /// assert!(String::from_utf8(r.serialize(true)).unwrap().contains("WWW-Authenticate: Bearer\r\n"));
    /// # Ok::<(), PolicyError>(())
    /// ```
    pub fn unauthorized_response(&self, head: &RequestHead, err: &VerifyError) -> Response {
        #[cfg(feature = "dpop")]
        if let Some(d) = self.inner.dpop.as_ref() {
            return dpop_unauthorized(d, head, err);
        }
        #[cfg(not(feature = "dpop"))]
        let _ = head;
        let challenge = match err {
            VerifyError::MissingToken => CHALLENGE_BEARER_NO_TOKEN,
            _ => CHALLENGE_BEARER_INVALID,
        };
        unauthorized(BODY_INVALID_TOKEN, &[challenge])
    }

    /// DPoP の方式。DPoP を設定していない認証器では `None`。
    ///
    /// 対象 feature: `dpop`。`from_jwt` などで認証器を受け取る側（hub 共通配線）が、
    /// DPoP 用のチャレンジを付けるかどうかを決めるために使う。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_jwt::dpop::{DpopConfig, DpopMode, PublicOrigin};
    /// use fandhe_backend_plugin_jwt::{
    ///     Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtAuthenticator, JwtPolicy, KeySet,
    ///     NoExtraClaims, SharedKeySet, TokenType,
    /// };
    ///
    /// let policy = || JwtPolicy::new(
    ///     Issuer::new("https://issuer.example").unwrap(),
    ///     Audiences::single("api").unwrap(),
    ///     AllowedAlgorithms::only(Algorithm::Es256),
    ///     TokenType::AccessToken,
    /// );
    /// let keys = || SharedKeySet::new(KeySet::empty());
    /// let plain = JwtAuthenticator::<NoExtraClaims>::builder(keys(), policy()).build();
    /// assert_eq!(plain.dpop_mode(), None);
    ///
    /// let cfg = DpopConfig::new(
    ///     DpopMode::Required,
    ///     PublicOrigin::new("https://api.example").unwrap(),
    ///     AllowedAlgorithms::only(Algorithm::Es256),
    /// );
    /// let bound = JwtAuthenticator::<NoExtraClaims>::builder(keys(), policy()).dpop(cfg).build();
    /// assert_eq!(bound.dpop_mode(), Some(DpopMode::Required));
    /// ```
    #[cfg(feature = "dpop")]
    pub fn dpop_mode(&self) -> Option<DpopMode> {
        self.inner.dpop.as_ref().map(|d| d.config.mode())
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

fn unix_now() -> u64 {
    // 時計異常時は u64::MAX を渡し、正の exp を常に期限切れ側へ倒す（フェイルクローズ）。
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(u64::MAX)
}

/// `cnf.jkt` の形式（SHA-256 の base64url、43 文字）。
#[cfg(feature = "dpop")]
fn is_thumbprint_shape(s: &str) -> bool {
    s.len() == 43
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// DPoP を設定した認証器の 401 応答（設計 9.9 節）。
#[cfg(feature = "dpop")]
fn dpop_unauthorized(d: &DpopState, head: &RequestHead, err: &VerifyError) -> Response {
    // (error コード, error を DPoP の行に付けるか)。資格情報なしは error を付けない。
    let (code, on_dpop): (Option<&str>, bool) = match err {
        VerifyError::MissingToken => (None, true),
        VerifyError::Dpop(e) if e.is_token_side() => (Some("invalid_token"), true),
        VerifyError::Dpop(_) => (Some("invalid_dpop_proof"), true),
        _ => {
            // 失敗した資格情報のスキームの行へ付ける（Bearer の行があるのは併用時だけ）。
            let bearer = matches!(credentials(head), Some((Scheme::Bearer, _)));
            (Some("invalid_token"), !(bearer && allows_bearer(d)))
        }
    };
    let dpop_line = match (code, on_dpop) {
        (Some(c), true) => format!("DPoP error=\"{c}\", algs=\"{}\"", d.challenge_algs),
        _ => format!("DPoP algs=\"{}\"", d.challenge_algs),
    };
    let bearer_line = match (code, on_dpop) {
        (Some(c), false) => format!("Bearer error=\"{c}\""),
        _ => "Bearer".to_owned(),
    };
    let body = match err {
        VerifyError::Dpop(e) if !e.is_token_side() => BODY_INVALID_DPOP_PROOF,
        _ => BODY_INVALID_TOKEN,
    };
    if allows_bearer(d) {
        unauthorized(body, &[&dpop_line, &bearer_line])
    } else {
        unauthorized(body, &[&dpop_line])
    }
}

#[cfg(feature = "dpop")]
fn allows_bearer(d: &DpopState) -> bool {
    d.config.mode() == DpopMode::AllowUnboundBearer
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

    #[cfg(feature = "dpop")]
    #[test]
    fn credentials_cases() {
        let h = |v: &str| head(&format!("GET / HTTP/1.1\r\nAuthorization: {v}\r\n\r\n"));
        let scheme = |v: &str| credentials(&h(v)).map(|(s, t)| (s == Scheme::Dpop, t.to_owned()));
        assert_eq!(scheme("DPoP abc"), Some((true, "abc".to_owned())));
        assert_eq!(scheme("dpop   abc"), Some((true, "abc".to_owned())));
        assert_eq!(scheme("Bearer abc"), Some((false, "abc".to_owned())));
        assert_eq!(scheme("DPoP "), None);
        assert_eq!(scheme("Basic abc"), None);
        let dup =
            head("GET / HTTP/1.1\r\nAuthorization: DPoP a\r\nAuthorization: Bearer b\r\n\r\n");
        assert!(credentials(&dup).is_none());
        assert!(credentials(&head("GET / HTTP/1.1\r\n\r\n")).is_none());
    }

    #[cfg(feature = "dpop")]
    #[test]
    fn thumbprint_shape() {
        assert!(is_thumbprint_shape(&"A".repeat(43)));
        assert!(!is_thumbprint_shape(&"A".repeat(42)));
        assert!(!is_thumbprint_shape(&format!("{}=", "A".repeat(42))));
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
