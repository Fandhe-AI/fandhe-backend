//! JWT 検証ポリシー型（イシュー #813、設計は `docs/design/jwt-verify-plugin.md` 2.2 節）。
//!
//! 後続のトークン検証（#815）とゲート（#816）が参照する「何を受理するか」の宣言を担う。
//! 期待する `iss`・`aud`・`typ` と許可 `alg` は**必須**で、欠けた構成は
//! コンパイルエラー（位置引数の不足）か構築エラー（[`PolicyError`]）になる。
//! 構成要素はすべて構築時に検証済みで、フィールドは非公開のため構造体リテラルや
//! `Default` による迂回もできない。

use std::fmt;
use std::time::Duration;

/// 時刻ずれの許容幅（leeway）の上限。DoS・リプレイ窓の拡大を防ぐ（設計 5 章 A04/A05）。
pub const MAX_CLOCK_SKEW: Duration = Duration::from_secs(300);

/// `iss` および各 `aud` 値のバイト長上限。ポリシー側のメモリを有界に保つ。
pub const MAX_CLAIM_VALUE_LEN: usize = 1024;

/// ポリシーが受け付ける `aud` の個数上限。
pub const MAX_AUDIENCES: usize = 16;

/// ポリシー構築時のエラー。入力値を含めない（ログ経由の情報漏洩防止）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PolicyError {
    /// `iss` が空。
    EmptyIssuer,
    /// `aud` が 0 個、または空文字の要素を含む。
    EmptyAudience,
    /// `aud` の個数が [`MAX_AUDIENCES`] を超えた。
    TooManyAudiences,
    /// `iss` または `aud` の値が [`MAX_CLAIM_VALUE_LEN`] バイトを超えた。
    ClaimValueTooLong,
    /// 許可 `alg` が 0 個。
    NoAllowedAlgorithms,
    /// leeway が [`MAX_CLOCK_SKEW`] を超えた。
    ClockSkewTooLarge,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::EmptyIssuer => "issuer が空です",
            Self::EmptyAudience => "audience が空、または空文字を含みます",
            Self::TooManyAudiences => "audience の個数が上限を超えています",
            Self::ClaimValueTooLong => "クレーム値が長さ上限を超えています",
            Self::NoAllowedAlgorithms => "許可アルゴリズムが指定されていません",
            Self::ClockSkewTooLarge => "時刻ずれの許容幅が上限を超えています",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for PolicyError {}

fn check_claim_value(value: &str) -> Result<(), PolicyError> {
    if value.len() > MAX_CLAIM_VALUE_LEN {
        return Err(PolicyError::ClaimValueTooLong);
    }
    Ok(())
}

/// 期待する発行者（`iss`）。
///
/// トークンの `iss` とは完全一致（大文字小文字・前後空白も区別）で比較する契約。
/// 比較の実装はトークン検証（#815）が担う。
///
/// ```
/// use fandhe_backend_plugin_jwt::{Issuer, PolicyError};
/// let iss = Issuer::new("https://issuer.example")?;
/// assert_eq!(iss.as_str(), "https://issuer.example");
/// assert_eq!(Issuer::new(""), Err(PolicyError::EmptyIssuer));
/// # Ok::<(), PolicyError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issuer(Box<str>);

impl Issuer {
    /// 空文字と長さ上限超過を拒否して構築する。
    pub fn new(value: impl Into<String>) -> Result<Self, PolicyError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PolicyError::EmptyIssuer);
        }
        check_claim_value(&value)?;
        Ok(Self(value.into_boxed_str()))
    }

    /// 発行者の文字列表現。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 受理する `aud` の集合。
///
/// トークンの `aud` が文字列でも配列でも、どれか 1 つが集合に一致すれば受理する契約
/// （判定は #815）。
///
/// ```
/// use fandhe_backend_plugin_jwt::{Audiences, PolicyError};
/// let aud = Audiences::new(["api", "web", "api"])?;
/// assert!(aud.contains("api"));
/// assert_eq!(aud.iter().count(), 2);
/// assert_eq!(Audiences::new(Vec::<String>::new()), Err(PolicyError::EmptyAudience));
/// # Ok::<(), PolicyError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audiences(Box<[Box<str>]>);

impl Audiences {
    /// 複数の `aud` から構築する。0 個・空文字・上限超過は拒否し、重複は除去する。
    pub fn new<I, S>(values: I) -> Result<Self, PolicyError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut out: Vec<Box<str>> = Vec::new();
        for v in values {
            let v: String = v.into();
            if v.is_empty() {
                return Err(PolicyError::EmptyAudience);
            }
            check_claim_value(&v)?;
            if out.iter().any(|e| &**e == v.as_str()) {
                continue;
            }
            if out.len() >= MAX_AUDIENCES {
                return Err(PolicyError::TooManyAudiences);
            }
            out.push(v.into_boxed_str());
        }
        if out.is_empty() {
            return Err(PolicyError::EmptyAudience);
        }
        Ok(Self(out.into_boxed_slice()))
    }

    /// 1 個だけの `aud` から構築する。
    pub fn single(value: impl Into<String>) -> Result<Self, PolicyError> {
        Self::new([value.into()])
    }

    /// 受理する `aud` を列挙する。
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|s| &**s)
    }

    /// 完全一致で含まれるか。
    pub fn contains(&self, aud: &str) -> bool {
        self.iter().any(|a| a == aud)
    }
}

/// 許可される署名アルゴリズム。
///
/// `none`・HS 系の変種は**存在しない**ため型として表現できない
/// （alg 混同攻撃対策、RFC 8725 3.1）。
///
/// ```
/// use fandhe_backend_plugin_jwt::Algorithm;
/// assert_eq!(Algorithm::from_jose_name("RS256"), Some(Algorithm::Rs256));
/// assert_eq!(Algorithm::from_jose_name("none"), None);
/// assert_eq!(Algorithm::from_jose_name("HS256"), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Algorithm {
    /// RSASSA-PKCS1-v1_5 + SHA-256。
    Rs256,
    /// ECDSA P-256 + SHA-256。
    Es256,
}

impl Algorithm {
    /// JOSE ヘッダ上の名前。
    pub const fn as_jose_name(self) -> &'static str {
        match self {
            Self::Rs256 => "RS256",
            Self::Es256 => "ES256",
        }
    }

    /// ヘッダの `alg` を解釈する唯一の入口。大文字小文字を区別する完全一致
    /// （RFC 7515 4.1.1）で、許可対象外（`none`・HS 系等）は `None`。
    pub fn from_jose_name(name: &str) -> Option<Self> {
        match name {
            "RS256" => Some(Self::Rs256),
            "ES256" => Some(Self::Es256),
            _ => None,
        }
    }
}

/// 許可 `alg` の非空集合（1 ポリシー = 1 発行者に 1 つ）。
///
/// ```
/// use fandhe_backend_plugin_jwt::{Algorithm, AllowedAlgorithms, PolicyError};
/// let algs = AllowedAlgorithms::new([Algorithm::Rs256, Algorithm::Rs256])?;
/// assert!(algs.contains(Algorithm::Rs256));
/// assert!(!algs.contains(Algorithm::Es256));
/// assert_eq!(AllowedAlgorithms::new([]), Err(PolicyError::NoAllowedAlgorithms));
/// # Ok::<(), PolicyError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedAlgorithms(Box<[Algorithm]>);

impl AllowedAlgorithms {
    /// 空集合を拒否し、重複を除去して構築する。
    pub fn new(algs: impl IntoIterator<Item = Algorithm>) -> Result<Self, PolicyError> {
        let mut out: Vec<Algorithm> = Vec::new();
        for a in algs {
            if !out.contains(&a) {
                out.push(a);
            }
        }
        if out.is_empty() {
            return Err(PolicyError::NoAllowedAlgorithms);
        }
        Ok(Self(out.into_boxed_slice()))
    }

    /// 単一アルゴリズムのみ許可する（失敗しない）。
    pub fn only(alg: Algorithm) -> Self {
        Self(Box::new([alg]))
    }

    /// 許可されているか。
    pub fn contains(&self, alg: Algorithm) -> bool {
        self.0.contains(&alg)
    }

    /// 許可アルゴリズムを列挙する。
    pub fn iter(&self) -> impl Iterator<Item = Algorithm> + '_ {
        self.0.iter().copied()
    }
}

/// 期待するトークン種別（ヘッダ `typ`）。
///
/// ```
/// use fandhe_backend_plugin_jwt::TokenType;
/// assert!(TokenType::AccessToken.matches_header("at+jwt"));
/// assert!(TokenType::AccessToken.matches_header("application/at+jwt"));
/// assert!(!TokenType::AccessToken.matches_header("JWT"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenType {
    /// RFC 9068 の JWT アクセストークン（`at+jwt`）。
    AccessToken,
}

impl TokenType {
    /// ヘッダ `typ` が一致するか。`at+jwt` と `application/at+jwt` を ASCII の
    /// 大文字小文字を無視して受理する（RFC 9068 4 節）。`typ` 欠落の拒否は
    /// 検証関数（#815）が行う。
    pub fn matches_header(self, typ: &str) -> bool {
        match self {
            Self::AccessToken => {
                typ.eq_ignore_ascii_case("at+jwt") || typ.eq_ignore_ascii_case("application/at+jwt")
            }
        }
    }
}

/// JWT 検証ポリシー。構築後は不変。
///
/// `iss`・`aud`・許可 `alg`・`typ` はすべて必須の位置引数で、欠けるとコンパイルできない。
///
/// ```
/// use fandhe_backend_plugin_jwt::*;
/// use std::time::Duration;
/// let policy = JwtPolicy::new(
///     Issuer::new("https://issuer.example")?,
///     Audiences::single("api")?,
///     AllowedAlgorithms::only(Algorithm::Rs256),
///     TokenType::AccessToken,
/// )
/// .with_clock_skew(Duration::from_secs(30))?;
/// assert_eq!(policy.clock_skew(), Duration::from_secs(30));
/// # Ok::<(), PolicyError>(())
/// ```
///
/// `typ` を省いた呼び出しはコンパイルできない（直前の正常呼び出しが陽性対照）。
///
/// ```
/// # use fandhe_backend_plugin_jwt::*;
/// # fn f() -> Result<(), PolicyError> {
/// let _ = JwtPolicy::new(
///     Issuer::new("i")?, Audiences::single("a")?,
///     AllowedAlgorithms::only(Algorithm::Rs256), TokenType::AccessToken,
/// );
/// # Ok(()) }
/// ```
///
/// ```compile_fail
/// # use fandhe_backend_plugin_jwt::*;
/// # fn f() -> Result<(), PolicyError> {
/// let _ = JwtPolicy::new(
///     Issuer::new("i")?, Audiences::single("a")?,
///     AllowedAlgorithms::only(Algorithm::Rs256),
/// );
/// # Ok(()) }
/// ```
///
/// `iss` を省いた呼び出しもコンパイルできない。
///
/// ```compile_fail
/// # use fandhe_backend_plugin_jwt::*;
/// # fn f() -> Result<(), PolicyError> {
/// let _ = JwtPolicy::new(
///     Audiences::single("a")?,
///     AllowedAlgorithms::only(Algorithm::Rs256), TokenType::AccessToken,
/// );
/// # Ok(()) }
/// ```
///
/// 構造体リテラルでの構築（非公開フィールド）はできない。
///
/// ```compile_fail
/// # use fandhe_backend_plugin_jwt::*;
/// # fn f() -> Result<(), PolicyError> {
/// let _ = JwtPolicy {
///     issuer: Issuer::new("i")?, audiences: Audiences::single("a")?,
///     algorithms: AllowedAlgorithms::only(Algorithm::Rs256),
///     token_type: TokenType::AccessToken, clock_skew: std::time::Duration::ZERO,
/// };
/// # Ok(()) }
/// ```
///
/// `Default` は実装していない。
///
/// ```compile_fail
/// # use fandhe_backend_plugin_jwt::*;
/// let _ = JwtPolicy::default();
/// ```
///
/// `none`・HS 系は `Algorithm` に存在しない（陽性対照は `Algorithm::Rs256` の使用）。
///
/// ```
/// # use fandhe_backend_plugin_jwt::*;
/// let _ = Algorithm::Rs256;
/// ```
///
/// ```compile_fail
/// # use fandhe_backend_plugin_jwt::*;
/// let _ = Algorithm::Hs256;
/// ```
///
/// ```compile_fail
/// # use fandhe_backend_plugin_jwt::*;
/// let _ = Algorithm::None;
/// ```
///
/// # 時刻クレームの契約
///
/// `exp` は必須（欠落は拒否）。`exp` は `now - leeway < exp`、`nbf` は任意で、あれば
/// `now + leeway >= nbf` を要求する。`iat` は判定に使わない。実装は #815 が行う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtPolicy {
    issuer: Issuer,
    audiences: Audiences,
    algorithms: AllowedAlgorithms,
    token_type: TokenType,
    clock_skew: Duration,
}

impl JwtPolicy {
    /// 必須 4 項目からポリシーを構築する。leeway の既定は 0 秒。
    pub fn new(
        issuer: Issuer,
        audiences: Audiences,
        algorithms: AllowedAlgorithms,
        token_type: TokenType,
    ) -> Self {
        Self {
            issuer,
            audiences,
            algorithms,
            token_type,
            clock_skew: Duration::ZERO,
        }
    }

    /// 時刻ずれの許容幅を設定する。[`MAX_CLOCK_SKEW`] 超過は拒否する。
    ///
    /// ```
    /// use fandhe_backend_plugin_jwt::*;
    /// use std::time::Duration;
    /// let p = JwtPolicy::new(
    ///     Issuer::new("i")?, Audiences::single("a")?,
    ///     AllowedAlgorithms::only(Algorithm::Es256), TokenType::AccessToken,
    /// );
    /// assert_eq!(
    ///     p.with_clock_skew(MAX_CLOCK_SKEW + Duration::from_secs(1)),
    ///     Err(PolicyError::ClockSkewTooLarge)
    /// );
    /// # Ok::<(), PolicyError>(())
    /// ```
    pub fn with_clock_skew(mut self, skew: Duration) -> Result<Self, PolicyError> {
        if skew > MAX_CLOCK_SKEW {
            return Err(PolicyError::ClockSkewTooLarge);
        }
        self.clock_skew = skew;
        Ok(self)
    }

    /// 期待する発行者。
    pub fn issuer(&self) -> &Issuer {
        &self.issuer
    }

    /// 受理する `aud` 集合。
    pub fn audiences(&self) -> &Audiences {
        &self.audiences
    }

    /// 許可 `alg` 集合。
    pub fn algorithms(&self) -> &AllowedAlgorithms {
        &self.algorithms
    }

    /// 期待するトークン種別。
    pub fn token_type(&self) -> TokenType {
        self.token_type
    }

    /// 時刻ずれの許容幅。
    pub fn clock_skew(&self) -> Duration {
        self.clock_skew
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync + Clone>() {}

    #[test]
    fn policy_types_are_send_sync() {
        assert_send_sync::<JwtPolicy>();
        assert_send_sync::<PolicyError>();
    }

    #[test]
    fn issuer_bounds() {
        assert_eq!(Issuer::new(""), Err(PolicyError::EmptyIssuer));
        assert!(Issuer::new("a".repeat(MAX_CLAIM_VALUE_LEN)).is_ok());
        assert_eq!(
            Issuer::new("a".repeat(MAX_CLAIM_VALUE_LEN + 1)),
            Err(PolicyError::ClaimValueTooLong)
        );
    }

    #[test]
    fn audiences_bounds() {
        assert_eq!(Audiences::new(["a", ""]), Err(PolicyError::EmptyAudience));
        let ok: Vec<String> = (0..MAX_AUDIENCES).map(|i| format!("a{i}")).collect();
        assert!(Audiences::new(ok.clone()).is_ok());
        let mut over = ok;
        over.push("extra".into());
        assert_eq!(Audiences::new(over), Err(PolicyError::TooManyAudiences));
        assert_eq!(
            Audiences::single("a".repeat(MAX_CLAIM_VALUE_LEN + 1)),
            Err(PolicyError::ClaimValueTooLong)
        );
    }

    #[test]
    fn display_does_not_leak_input() {
        let err = Issuer::new("x".repeat(MAX_CLAIM_VALUE_LEN + 1)).unwrap_err();
        assert!(!err.to_string().contains("xxx"));
    }
}
