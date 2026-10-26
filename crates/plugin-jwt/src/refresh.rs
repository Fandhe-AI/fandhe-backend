//! JWKS 取り直しの土台: 取得関数の trait [`JwksFetcher`] と設定型 [`RefreshConfig`]。
//!
//! 対象 feature: `jwks-refresh`（既定では無効。pay-for-what-you-use）。
//!
//! ゲートの外で [`SharedKeySet`](crate::SharedKeySet) を最新に保つ非同期タスク
//! （後続 issue で実装）が、本モジュールの trait を介してアプリから JWKS 本文を
//! 受け取り、設定型の値で周期・最小間隔・時間切れを決める。本クレートは HTTP
//! クライアントも TLS も持たない（取得はアプリ側の責務。除外事項 #8・#11）。
//! 設計は `docs/design/jwt-verify-plugin.md` 8 章を参照。

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// 取り直しの最小間隔の既定値（10 秒）。
///
/// `DEFAULT_UNKNOWN_KEY_MIN_INTERVAL` と同じ値だが別の定数で、こちらは
/// `Duration` 精度（秒精度の通知間引きとは別物）。
pub const DEFAULT_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(10);
/// 取得 1 回の時間切れの既定値（5 秒）。
pub const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// 周期の上限（24 時間）。
pub const MAX_REFRESH_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);
/// 最小間隔の上限（1 時間）。
pub const MAX_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// 取得の時間切れの上限（60 秒）。
pub const MAX_FETCH_TIMEOUT: Duration = Duration::from_secs(60);
/// 鮮度期限 `max_stale` の上限（30 日）。
pub const MAX_STALE_LIMIT: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// [`JwksFetcher::fetch`] が返す future の型。
///
/// `async fn` in trait は dyn 互換でないため boxed future を返す
/// （`futures`・`async-trait` に依存しない。`routes::HandlerFuture` と同じ方針）。
pub type FetchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<JwksResponse, FetchError>> + Send + 'a>>;

/// アプリが実装する JWKS 取得関数。取り直しタスクから呼ばれる。
///
/// # 実装者への契約
///
/// 1. 取得先の URL は構築時に固定し、`kid`・リクエスト由来の値から組み立てない（SSRF）。
/// 2. HTTPS を使い、証明書の検証を無効にしない。
/// 3. リダイレクトは追わないか、同じオリジンに限る。
/// 4. 本文を読む量に上限を設ける（目安 64 KiB）。
/// 5. 取得関数自身にも時間切れを設ける。
/// 6. 認証情報・本文をログに出さない。
/// 7. 返す future は任意の `await` 点で drop されうる（時間切れ・停止のとき）。
///    drop されても整合が壊れないよう、副作用を `await` をまたいで持ち越さないこと。
///
/// 実装は panic しないこと。`panic = "abort"` の構成では panic の保護が効かない
/// （`UnknownKeyNotifier` と同じ注意）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_jwt::{FetchFuture, JwksFetcher, JwksResponse};
/// use std::future::Future;
/// use std::task::{Context, Poll, Waker};
///
/// struct Fixed;
/// impl JwksFetcher for Fixed {
///     fn fetch(&self) -> FetchFuture<'_> {
///         Box::pin(async { Ok(JwksResponse::new(r#"{"keys":[]}"#.to_string(), None)) })
///     }
/// }
///
/// let f: Box<dyn JwksFetcher> = Box::new(Fixed);
/// let mut fut = f.fetch();
/// let mut cx = Context::from_waker(Waker::noop());
/// match fut.as_mut().poll(&mut cx) {
///     Poll::Ready(Ok(r)) => assert_eq!(r.body(), r#"{"keys":[]}"#),
///     _ => panic!("即時に完了するはず"),
/// }
/// ```
pub trait JwksFetcher: Send + Sync + 'static {
    /// JWKS を 1 回取得する。
    fn fetch(&self) -> FetchFuture<'_>;
}

/// 取得結果。本文の UTF-8 検査と `Cache-Control` の解析は取得関数の責務。
///
/// `max_age` は `no-store`・`no-cache`・`max-age` なしのとき `None`。過大な値でも、
/// 利用側が周期（24 時間以下）で `min` を取るためオーバーフローしない。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_jwt::JwksResponse;
/// use std::time::Duration;
///
/// let r = JwksResponse::new("SECRET".to_string(), Some(Duration::from_secs(60)));
/// assert_eq!(r.body(), "SECRET");
/// assert_eq!(r.max_age(), Some(Duration::from_secs(60)));
/// assert!(!format!("{r:?}").contains("SECRET"));
/// ```
#[derive(Clone)]
pub struct JwksResponse {
    body: String,
    max_age: Option<Duration>,
}

impl JwksResponse {
    /// 本文と `max_age` から組み立てる。
    #[must_use]
    pub fn new(body: String, max_age: Option<Duration>) -> Self {
        Self { body, max_age }
    }

    /// JWKS 本文。
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// 本文を取り出す。
    #[must_use]
    pub fn into_body(self) -> String {
        self.body
    }

    /// 応答が示すキャッシュ寿命。
    #[must_use]
    pub fn max_age(&self) -> Option<Duration> {
        self.max_age
    }
}

impl fmt::Debug for JwksResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwksResponse")
            .field("body_len", &self.body.len())
            .field("max_age", &self.max_age)
            .finish()
    }
}

/// 取得失敗の分類。文字列も攻撃者入力も持たない（ログ安全）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_jwt::FetchError;
/// assert!(!FetchError::Transport.to_string().is_empty());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FetchError {
    /// 通信・接続の失敗。
    Transport,
    /// 想定外のステータス。
    Status,
    /// 本文が上限を超えた。
    TooLarge,
    /// その他。
    Other,
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Transport => "JWKS の取得で通信に失敗しました",
            Self::Status => "JWKS の取得で想定外のステータスを受け取りました",
            Self::TooLarge => "JWKS の本文が上限を超えました",
            Self::Other => "JWKS の取得に失敗しました",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for FetchError {}

/// 取り直しの設定。周期は必須で、既定値も `Default` も持たない。
///
/// 値の検査はすべて [`RefreshConfigBuilder::build`] で行う（セッターの呼び出し順に
/// 結果が依存しないようにするため）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_jwt::{RefreshConfig, RefreshConfigError};
/// use std::time::Duration;
///
/// let c = RefreshConfig::new(Duration::from_secs(3600)).unwrap();
/// assert_eq!(c.period(), Duration::from_secs(3600));
/// assert_eq!(c.max_stale(), None);
/// assert_eq!(
///     RefreshConfig::new(Duration::ZERO).unwrap_err(),
///     RefreshConfigError::ZeroPeriod
/// );
/// ```
///
/// 周期を渡さずには組み立てられない。
///
/// ```compile_fail
/// let _ = fandhe_backend_plugin_jwt::RefreshConfig::builder();
/// ```
#[derive(Debug, Clone)]
pub struct RefreshConfig {
    period: Duration,
    min_interval: Duration,
    fetch_timeout: Duration,
    max_stale: Option<Duration>,
}

impl RefreshConfig {
    /// 周期を必須引数にした builder。
    #[must_use]
    pub fn builder(period: Duration) -> RefreshConfigBuilder {
        RefreshConfigBuilder {
            period,
            min_interval: DEFAULT_REFRESH_MIN_INTERVAL,
            fetch_timeout: DEFAULT_FETCH_TIMEOUT,
            max_stale: None,
        }
    }

    /// `builder(period).build()` の短縮形。
    ///
    /// # Errors
    ///
    /// 値が範囲外のとき [`RefreshConfigError`]。
    pub fn new(period: Duration) -> Result<Self, RefreshConfigError> {
        Self::builder(period).build()
    }

    /// 定期取り直しの周期。
    #[must_use]
    pub fn period(&self) -> Duration {
        self.period
    }

    /// 取り直しの最小間隔。
    #[must_use]
    pub fn min_interval(&self) -> Duration {
        self.min_interval
    }

    /// 取得 1 回の時間切れ。
    #[must_use]
    pub fn fetch_timeout(&self) -> Duration {
        self.fetch_timeout
    }

    /// 鮮度期限（opt-in。既定は `None`）。
    #[must_use]
    pub fn max_stale(&self) -> Option<Duration> {
        self.max_stale
    }
}

/// [`RefreshConfig`] の builder。セッターは失敗せず、検査は [`build`](Self::build)。
#[derive(Debug, Clone)]
pub struct RefreshConfigBuilder {
    period: Duration,
    min_interval: Duration,
    fetch_timeout: Duration,
    max_stale: Option<Duration>,
}

impl RefreshConfigBuilder {
    /// 取り直しの最小間隔を設定する。
    #[must_use]
    pub fn min_interval(mut self, v: Duration) -> Self {
        self.min_interval = v;
        self
    }

    /// 取得 1 回の時間切れを設定する。
    #[must_use]
    pub fn fetch_timeout(mut self, v: Duration) -> Self {
        self.fetch_timeout = v;
        self
    }

    /// 鮮度期限を設定する（opt-in）。
    #[must_use]
    pub fn max_stale(mut self, v: Duration) -> Self {
        self.max_stale = Some(v);
        self
    }

    /// 値を検査して設定を確定する。範囲の検査（period、min_interval、
    /// fetch_timeout、max_stale の順）の後に関係の検査を行う（フェイルクローズ）。
    ///
    /// # Errors
    ///
    /// 0・上限超過・値同士の関係違反のとき [`RefreshConfigError`]。
    pub fn build(self) -> Result<RefreshConfig, RefreshConfigError> {
        use RefreshConfigError as E;
        range(
            self.period,
            MAX_REFRESH_PERIOD,
            E::ZeroPeriod,
            E::PeriodTooLarge,
        )?;
        range(
            self.min_interval,
            MAX_REFRESH_MIN_INTERVAL,
            E::ZeroMinInterval,
            E::MinIntervalTooLarge,
        )?;
        range(
            self.fetch_timeout,
            MAX_FETCH_TIMEOUT,
            E::ZeroFetchTimeout,
            E::FetchTimeoutTooLarge,
        )?;
        if let Some(s) = self.max_stale {
            range(s, MAX_STALE_LIMIT, E::ZeroMaxStale, E::MaxStaleTooLarge)?;
        }
        if self.min_interval > self.period {
            return Err(E::MinIntervalExceedsPeriod);
        }
        if self.fetch_timeout > self.min_interval {
            return Err(E::FetchTimeoutExceedsMinInterval);
        }
        if let Some(s) = self.max_stale {
            let need = self
                .period
                .checked_add(self.fetch_timeout)
                .ok_or(E::MaxStaleTooShort)?;
            if s < need {
                return Err(E::MaxStaleTooShort);
            }
        }
        Ok(RefreshConfig {
            period: self.period,
            min_interval: self.min_interval,
            fetch_timeout: self.fetch_timeout,
            max_stale: self.max_stale,
        })
    }
}

fn range(
    v: Duration,
    max: Duration,
    zero: RefreshConfigError,
    large: RefreshConfigError,
) -> Result<(), RefreshConfigError> {
    if v.is_zero() {
        Err(zero)
    } else if v > max {
        Err(large)
    } else {
        Ok(())
    }
}

/// [`RefreshConfigBuilder::build`] の失敗理由。固定文言のみ（ログ安全）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefreshConfigError {
    /// 周期が 0。
    ZeroPeriod,
    /// 周期が上限超過。
    PeriodTooLarge,
    /// 最小間隔が 0。
    ZeroMinInterval,
    /// 最小間隔が上限超過。
    MinIntervalTooLarge,
    /// 時間切れが 0。
    ZeroFetchTimeout,
    /// 時間切れが上限超過。
    FetchTimeoutTooLarge,
    /// 鮮度期限が 0。
    ZeroMaxStale,
    /// 鮮度期限が上限超過。
    MaxStaleTooLarge,
    /// 最小間隔が周期より大きい。
    MinIntervalExceedsPeriod,
    /// 時間切れが最小間隔より大きい。
    FetchTimeoutExceedsMinInterval,
    /// 鮮度期限が `period + fetch_timeout` 未満。
    MaxStaleTooShort,
}

impl fmt::Display for RefreshConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::ZeroPeriod => "取り直しの周期が 0 です",
            Self::PeriodTooLarge => "取り直しの周期が上限を超えています",
            Self::ZeroMinInterval => "取り直しの最小間隔が 0 です",
            Self::MinIntervalTooLarge => "取り直しの最小間隔が上限を超えています",
            Self::ZeroFetchTimeout => "取得の時間切れが 0 です",
            Self::FetchTimeoutTooLarge => "取得の時間切れが上限を超えています",
            Self::ZeroMaxStale => "鮮度期限が 0 です",
            Self::MaxStaleTooLarge => "鮮度期限が上限を超えています",
            Self::MinIntervalExceedsPeriod => "最小間隔が周期を超えています",
            Self::FetchTimeoutExceedsMinInterval => "取得の時間切れが最小間隔を超えています",
            Self::MaxStaleTooShort => "鮮度期限が周期と取得の時間切れの和に満たません",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for RefreshConfigError {}
