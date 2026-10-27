//! DPoP をゲートへ配線するための設定型 [`DpopMode`]・[`DpopConfig`]（`dpop` feature、イシュー #904）。
//!
//! 役割: `JwtAuthenticatorBuilder::dpop` に渡す設定を 1 つにまとめる。方式・公開オリジン・
//! proof の許可 `alg` は**構築時の必須引数**で、`Default` を持たない（既定値を置くと、
//! 設定漏れが「検証しない」「誤ったオリジンで検証する」へ静かに倒れるため）。窓と保持数の
//! 上限は任意で、検証は構築時に済ませる。設計は `docs/design/jwt-verify-plugin.md` 9.3・9.13 節。

use std::fmt;

use super::{DpopConfigError, IatWindow, MAX_REPLAY_CAPACITY, PublicOrigin};
use crate::policy::AllowedAlgorithms;

/// DPoP の適用方式。`Default` は実装しない（利用側に必ず選ばせる）。
///
/// ```
/// use fandhe_backend_plugin_jwt::dpop::DpopMode;
/// assert_ne!(DpopMode::Required, DpopMode::AllowUnboundBearer);
/// ```
///
/// 既定値はない。
///
/// ```compile_fail
/// use fandhe_backend_plugin_jwt::dpop::DpopMode;
/// let _ = DpopMode::default();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DpopMode {
    /// すべての要求で DPoP を必須にする。`cnf.jkt` のないトークンは `Bearer` でも `DPoP` でも拒否する。
    Required,
    /// `cnf.jkt` のないトークンに限り `Bearer` での提示を受理する（DPoP 結合のトークンを
    /// `Bearer` で提示した場合は拒否する）。移行期間用。`DPoP` スキームでの提示は、
    /// 結合のないトークンなら拒否する。
    AllowUnboundBearer,
}

/// DPoP の設定。方式・公開オリジン・proof の許可 `alg` は必須。
///
/// ```
/// use fandhe_backend_plugin_jwt::dpop::{DpopConfig, DpopMode, PublicOrigin};
/// use fandhe_backend_plugin_jwt::{Algorithm, AllowedAlgorithms};
///
/// let cfg = DpopConfig::new(
///     DpopMode::Required,
///     PublicOrigin::new("https://api.example")?,
///     AllowedAlgorithms::only(Algorithm::Es256),
/// )
/// .with_replay_capacity(1000)?;
/// assert_eq!(cfg.mode(), DpopMode::Required);
/// assert_eq!(cfg.replay_capacity(), Some(1000));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// 方式を省いた構築はコンパイルできない（既定値を持たない）。
///
/// ```compile_fail
/// use fandhe_backend_plugin_jwt::dpop::{DpopConfig, PublicOrigin};
/// use fandhe_backend_plugin_jwt::{Algorithm, AllowedAlgorithms};
///
/// let _ = DpopConfig::new(
///     PublicOrigin::new("https://api.example").unwrap(),
///     AllowedAlgorithms::only(Algorithm::Es256),
/// );
/// ```
#[derive(Clone)]
pub struct DpopConfig {
    mode: DpopMode,
    origin: PublicOrigin,
    algs: AllowedAlgorithms,
    window: IatWindow,
    replay_capacity: Option<usize>,
}

impl DpopConfig {
    /// 必須の 3 項目で構築する。`iat` の窓は既定（過去 60 秒・未来 5 秒）、`jti` の保持数は
    /// `ReplayStore` の既定。
    pub fn new(mode: DpopMode, origin: PublicOrigin, algs: AllowedAlgorithms) -> Self {
        Self {
            mode,
            origin,
            algs,
            window: IatWindow::default(),
            replay_capacity: None,
        }
    }

    /// proof の `iat` の許容窓を指定する。`jti` の保持期間も同じ窓から決まる。
    #[must_use]
    pub fn with_iat_window(mut self, window: IatWindow) -> Self {
        self.window = window;
        self
    }

    /// `jti` の保持数の上限を指定する。0 と [`MAX_REPLAY_CAPACITY`] 超は構築時に拒否する。
    pub fn with_replay_capacity(mut self, capacity: usize) -> Result<Self, DpopConfigError> {
        if capacity == 0 {
            return Err(DpopConfigError::ReplayCapacityZero);
        }
        if capacity > MAX_REPLAY_CAPACITY {
            return Err(DpopConfigError::ReplayCapacityTooLarge);
        }
        self.replay_capacity = Some(capacity);
        Ok(self)
    }

    /// 適用方式。
    pub fn mode(&self) -> DpopMode {
        self.mode
    }

    /// 公開オリジン。
    pub fn origin(&self) -> &PublicOrigin {
        &self.origin
    }

    /// proof に許す署名アルゴリズム（アクセストークン側のポリシーとは独立）。
    pub fn algorithms(&self) -> &AllowedAlgorithms {
        &self.algs
    }

    /// `iat` の許容窓。
    pub fn iat_window(&self) -> IatWindow {
        self.window
    }

    /// `jti` の保持数の上限（未指定なら `None` で `ReplayStore` の既定）。
    pub fn replay_capacity(&self) -> Option<usize> {
        self.replay_capacity
    }
}

impl fmt::Debug for DpopConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DpopConfig")
            .field("mode", &self.mode)
            .field("origin", &self.origin)
            .field("algs", &self.algs)
            .field("window", &self.window)
            .field("replay_capacity", &self.replay_capacity)
            .finish()
    }
}
