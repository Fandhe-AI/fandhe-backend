//! 未知 `kid` の通知口（設計 `docs/design/jwt-verify-plugin.md` 2.5 節、イシュー #816）。
//!
//! `RequestGate::check` は同期・I/O なしの契約のため、ゲート自身は JWKS を取り直せない。
//! 知らない `kid` のトークンはその場で 401 にし（フェイルクローズ）、鍵ローテーションの
//! 可能性があることだけを、利用側が登録した同期の通知口 [`UnknownKeyNotifier`] へ知らせる。
//! 取り直し（HTTP 取得・[`SharedKeySet::set`](crate::SharedKeySet::set)）は利用側の
//! 非同期タスクが担う。呼び出し元は `crate::auth::JwtAuthenticator`（判定確定後に呼ぶ）。

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// 未知 `kid` 通知の既定の最小間隔（10 秒）。
pub const DEFAULT_UNKNOWN_KEY_MIN_INTERVAL: Duration = Duration::from_secs(10);

/// 未知 `kid` の通知イベント。
///
/// 生トークンや鍵材料は持たない。`Debug` は `kid` を出さない。
#[non_exhaustive]
pub struct UnknownKeyEvent<'a> {
    kid: &'a str,
    suppressed: u64,
}

impl<'a> UnknownKeyEvent<'a> {
    pub(crate) fn new(kid: &'a str, suppressed: u64) -> Self {
        Self { kid, suppressed }
    }

    /// トークンヘッダの `kid`（長さは `MAX_KID_LEN` 以下）。
    ///
    /// **攻撃者が自由に決められる信頼できない値**。ログへ出す場合はエスケープし、
    /// 鍵取得先の URL やファイル名の組み立てに直接使わないこと。
    pub fn kid(&self) -> &'a str {
        self.kid
    }

    /// 前回の通知以降に、間引かれて通知されなかった件数。
    pub fn suppressed(&self) -> u64 {
        self.suppressed
    }
}

impl fmt::Debug for UnknownKeyEvent<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnknownKeyEvent")
            .field("suppressed", &self.suppressed)
            .finish_non_exhaustive()
    }
}

/// 未知 `kid` を受けたときに呼ばれる同期の通知口。
///
/// # 実装の契約
///
/// - **ブロックしない**: `RequestGate::check` と同じ呼び出しスレッド（Tokio ワーカー）で
///   実行される。ロックの長期保持・`sleep`・同期 I/O をしない。
/// - **I/O しない**: JWKS の取得はここでは行わず、`try_send` 等で利用側の非同期タスクへ
///   合図するだけにとどめる。
/// - **panic しない**: 呼び出し側は `catch_unwind` で包むが、`panic = "abort"` では効かない。
///
/// 通知は最小間隔で間引かれる（[`DEFAULT_UNKNOWN_KEY_MIN_INTERVAL`]）。
/// クロージャ `Fn(&UnknownKeyEvent<'_>) + Send + Sync + 'static` も実装として使える。
///
/// ```
/// use fandhe_backend_plugin_jwt::{UnknownKeyEvent, UnknownKeyNotifier};
/// fn assert_notifier(_: impl UnknownKeyNotifier) {}
/// assert_notifier(|e: &UnknownKeyEvent<'_>| { let _ = e.kid(); });
/// ```
pub trait UnknownKeyNotifier: Send + Sync {
    /// 未知 `kid` を通知する。
    fn notify(&self, event: &UnknownKeyEvent<'_>);
}

impl<F> UnknownKeyNotifier for F
where
    F: Fn(&UnknownKeyEvent<'_>) + Send + Sync + 'static,
{
    fn notify(&self, event: &UnknownKeyEvent<'_>) {
        self(event);
    }
}

/// 通知の間引き。秒単位の最小間隔で、勝った 1 スレッドだけが通知する。
///
/// 「最終通知時刻」ではなく「次に許可する時刻」を持つ。番兵値が時計異常時の
/// `now = u64::MAX` やテストの `now = 0` と衝突しないようにするため。
pub(crate) struct NotifyThrottle {
    next_allowed_unix: AtomicU64,
    suppressed: AtomicU64,
    interval_secs: u64,
}

impl NotifyThrottle {
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            next_allowed_unix: AtomicU64::new(0),
            suppressed: AtomicU64::new(0),
            interval_secs: interval.as_secs(),
        }
    }

    /// 通知してよければ、これまでに間引いた件数を返す。
    pub(crate) fn try_acquire(&self, now_unix: u64) -> Option<u64> {
        let next = self.next_allowed_unix.load(Ordering::Acquire);
        if next == u64::MAX && self.interval_secs > 0
            || now_unix < next
            || self
                .next_allowed_unix
                .compare_exchange(
                    next,
                    now_unix.saturating_add(self.interval_secs),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
        {
            self.suppressed.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Some(self.suppressed.swap(0, Ordering::Relaxed))
    }
}

/// 通知口を panic 境界つきで呼ぶ。panic は捨て、判定結果へ影響させない。
pub(crate) fn call_guarded(n: &dyn UnknownKeyNotifier, ev: &UnknownKeyEvent<'_>) {
    let _ = catch_unwind(AssertUnwindSafe(|| n.notify(ev)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_interval() {
        let t = NotifyThrottle::new(Duration::from_secs(10));
        assert_eq!(t.try_acquire(100), Some(0));
        assert_eq!(t.try_acquire(105), None);
        assert_eq!(t.try_acquire(109), None);
        assert_eq!(t.try_acquire(110), Some(2));
        assert_eq!(t.try_acquire(110), None);
    }

    #[test]
    fn throttle_boundaries() {
        let t = NotifyThrottle::new(Duration::from_secs(10));
        assert_eq!(t.try_acquire(0), Some(0));
        assert_eq!(t.try_acquire(5), None);
        let t = NotifyThrottle::new(Duration::from_secs(10));
        assert_eq!(t.try_acquire(u64::MAX), Some(0));
        assert_eq!(t.try_acquire(u64::MAX), None);
        let t = NotifyThrottle::new(Duration::ZERO);
        assert_eq!(t.try_acquire(7), Some(0));
        assert_eq!(t.try_acquire(7), Some(0));
    }

    #[test]
    fn debug_hides_kid() {
        let ev = UnknownKeyEvent::new("secret-kid", 3);
        assert!(!format!("{ev:?}").contains("secret-kid"));
    }
}
