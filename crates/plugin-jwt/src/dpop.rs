//! DPoP proof（RFC 9449）の解析と検査（`dpop` feature、イシュー #901）。
//!
//! 役割: `fandhe-auth` が発行する DPoP 結合の資格情報を下流で受けるための、状態を持たない
//! sans-IO の部品。[`verify_proof`] が proof（DPoP JWT）の構造・`typ`・`alg`・`jwk`・署名・
//! `jti`・`htm`・`htu`・`iat`・`ath` を検査し、鍵のサムプリント（[`JwkThumbprint`]）を返す。
//! I/O もロックも持たず、時刻は呼び出し側が注入する。設計は
//! `docs/design/jwt-verify-plugin.md` 9 章。
//!
//! 含まないもの（後続のイシュー）: `jti` の保持と再使用の検知（#903）、`cnf.jkt` との比較・
//! `Authorization: DPoP` の解析・ゲート配線（#904）。
//!
//! # 検査の順序（契約）
//!
//! 1. 長さ（[`MAX_PROOF_LEN`]）と 3 区画構造
//! 2. ヘッダ解析 → `crit` 拒否 → `typ`（`dpop+jwt`）→ `alg`（許可リスト）→ `jwk` の存在
//! 3. `jwk` の検査（秘密鍵の要素・種別と `alg` の食い違い・サイズ）
//! 4. 署名検証（鍵は `jwk` のみ。`jku`・`x5u`・`x5c`・`kid` は読まない）
//! 5. **署名検証が済むまで payload は解析しない**
//! 6. `jti` → `htm` → `htu` → `iat` → `ath`
//!
//! どのような入力でもパニックせず `Err` を返す。エラーは文字列を持たず、proof・`jti`・
//! `jwk` の値を含まない。
//!
//! # Examples
//!
//! ```
//! use fandhe_backend_plugin_jwt::dpop::{
//!     DpopError, IatWindow, ProofContext, PublicOrigin, verify_proof,
//! };
//! use fandhe_backend_plugin_jwt::{Algorithm, AllowedAlgorithms};
//!
//! let origin = PublicOrigin::new("https://api.example")?;
//! let htu = origin.expected_htu("/resource")?;
//! let algs = AllowedAlgorithms::only(Algorithm::Es256);
//! let ctx = ProofContext::new("GET", &htu, "access-token", &algs, IatWindow::default());
//! assert_eq!(verify_proof("", &ctx, 1000).unwrap_err(), DpopError::MissingProof);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod key;
mod origin;

use std::fmt;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest::{SHA256, digest};
use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};

use crate::policy::{Algorithm, AllowedAlgorithms};
use crate::verify::{MAX_TOKEN_LEN, signature_ok};

pub use key::{JwkThumbprint, jwk_thumbprint};
pub use origin::{ExpectedHtu, MAX_HTU_LEN, MAX_ORIGIN_LEN, OriginError, PublicOrigin};

use key::{ProofJwk, build_proof_key};

/// 受理する proof 全体の最大バイト長（[`MAX_TOKEN_LEN`] と同じ）。
pub const MAX_PROOF_LEN: usize = MAX_TOKEN_LEN;
/// `jti` の最大バイト長。
pub const MAX_JTI_LEN: usize = 256;

/// `iat` の過去側の既定幅。
pub const DEFAULT_IAT_PAST: Duration = Duration::from_secs(60);
/// `iat` の過去側の上限。
pub const MAX_IAT_PAST: Duration = Duration::from_secs(300);
/// `iat` の未来側（時計ずれ）の既定幅。
pub const DEFAULT_IAT_FUTURE: Duration = Duration::from_secs(5);
/// `iat` の未来側の上限。
pub const MAX_IAT_FUTURE: Duration = Duration::from_secs(60);

/// DPoP 設定値の検証エラー。固定文言。
///
/// ```
/// use std::time::Duration;
/// use fandhe_backend_plugin_jwt::dpop::{DpopConfigError, IatWindow};
/// assert_eq!(
///     IatWindow::new(Duration::ZERO, Duration::ZERO).unwrap_err(),
///     DpopConfigError::IatPastZero
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DpopConfigError {
    /// `iat` の過去側の幅が 0（正規の proof も受けられなくなる）。
    IatPastZero,
    /// `iat` の過去側の幅が [`MAX_IAT_PAST`] を超えた。
    IatPastTooLarge,
    /// `iat` の未来側の幅が [`MAX_IAT_FUTURE`] を超えた。
    IatFutureTooLarge,
}

impl fmt::Display for DpopConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::IatPastZero => "iat の過去側の許容幅は 0 にできません",
            Self::IatPastTooLarge => "iat の過去側の許容幅が上限を超えています",
            Self::IatFutureTooLarge => "iat の未来側の許容幅が上限を超えています",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for DpopConfigError {}

/// proof の `iat` の許容窓（`now - past <= iat <= now + future`、秒単位）。
///
/// 既定は過去 60 秒・未来 5 秒。上限は過去 [`MAX_IAT_PAST`]（300 秒）・未来
/// [`MAX_IAT_FUTURE`]（60 秒）で、過去側の 0 は拒否する。
///
/// ```
/// use std::time::Duration;
/// use fandhe_backend_plugin_jwt::dpop::{DpopConfigError, IatWindow};
///
/// let w = IatWindow::default();
/// assert_eq!((w.past().as_secs(), w.future().as_secs()), (60, 5));
/// assert!(IatWindow::new(Duration::from_secs(30), Duration::ZERO).is_ok());
/// assert_eq!(
///     IatWindow::new(Duration::from_secs(301), Duration::ZERO).unwrap_err(),
///     DpopConfigError::IatPastTooLarge
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IatWindow {
    past: Duration,
    future: Duration,
}

impl IatWindow {
    /// 窓を構築する。上限超過と過去側 0 を拒否する。
    pub fn new(past: Duration, future: Duration) -> Result<Self, DpopConfigError> {
        if past.is_zero() {
            return Err(DpopConfigError::IatPastZero);
        }
        if past > MAX_IAT_PAST {
            return Err(DpopConfigError::IatPastTooLarge);
        }
        if future > MAX_IAT_FUTURE {
            return Err(DpopConfigError::IatFutureTooLarge);
        }
        Ok(Self { past, future })
    }

    /// 過去側の許容幅。
    pub fn past(&self) -> Duration {
        self.past
    }

    /// 未来側の許容幅。
    pub fn future(&self) -> Duration {
        self.future
    }
}

impl Default for IatWindow {
    fn default() -> Self {
        Self {
            past: DEFAULT_IAT_PAST,
            future: DEFAULT_IAT_FUTURE,
        }
    }
}

/// proof 検証の失敗理由。文字列を持たず、proof・`jti`・`jwk` の値を含まない。
///
/// ```
/// use fandhe_backend_plugin_jwt::dpop::DpopError;
/// assert_eq!(DpopError::InvalidSignature.to_string(), "DPoP proof の署名が不正です");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DpopError {
    /// proof が空。
    MissingProof,
    /// proof が [`MAX_PROOF_LEN`] を超えた。
    TooLong,
    /// 構造不正（区画数・base64・JSON・型・重複キーなど）。
    Malformed,
    /// `typ` の欠落、または `dpop+jwt` ではない。
    InvalidType,
    /// `alg` が許可リスト外（`none`・HS 系・未知の名前を含む）。
    AlgorithmNotAllowed,
    /// ヘッダに `crit` がある。
    CriticalHeader,
    /// ヘッダに `jwk` がない。
    MissingJwk,
    /// `jwk` に秘密鍵の要素（共通鍵を含む）がある。
    PrivateKeyMaterial,
    /// `jwk` の種別・曲線・`use` が未対応。
    UnsupportedKey,
    /// `jwk` の種別とヘッダの `alg`（または `jwk.alg`）が食い違う。
    KeyAlgorithmMismatch,
    /// `jwk` の鍵成分が不正（欠落・長さ・正規形でない表現）。
    InvalidKey,
    /// 署名検証に失敗した。
    InvalidSignature,
    /// 必須クレーム（`jti`・`htm`・`htu`・`iat`・`ath`）の欠落。
    MissingClaim,
    /// `jti` が空、または [`MAX_JTI_LEN`] を超えた。
    InvalidJti,
    /// `htm` がリクエストのメソッドと一致しない。
    MethodMismatch,
    /// `htu` が期待 URI と一致しない（解析できない場合を含む）。
    UriMismatch,
    /// `ath` がアクセストークンのハッシュと一致しない。
    AccessTokenHashMismatch,
    /// `iat` が許容窓の外。
    IssuedAtOutOfWindow,
    /// リクエストターゲットから期待 `htu` を作れない。
    InvalidRequestTarget,
}

impl fmt::Display for DpopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::MissingProof => "DPoP proof がありません",
            Self::TooLong => "DPoP proof が長すぎます",
            Self::Malformed => "DPoP proof の形式が不正です",
            Self::InvalidType => "DPoP proof の typ が不正です",
            Self::AlgorithmNotAllowed => "許可されていない署名アルゴリズムです",
            Self::CriticalHeader => "crit ヘッダは受け付けません",
            Self::MissingJwk => "DPoP proof のヘッダに jwk がありません",
            Self::PrivateKeyMaterial => "秘密鍵の要素を含む jwk は受け付けません",
            Self::UnsupportedKey => "未対応の種別の jwk です",
            Self::KeyAlgorithmMismatch => "jwk の種別と alg が矛盾しています",
            Self::InvalidKey => "jwk の鍵成分が不正です",
            Self::InvalidSignature => "DPoP proof の署名が不正です",
            Self::MissingClaim => "DPoP proof の必須クレームがありません",
            Self::InvalidJti => "DPoP proof の jti が不正です",
            Self::MethodMismatch => "DPoP proof の htm がリクエストのメソッドと一致しません",
            Self::UriMismatch => "DPoP proof の htu が期待 URI と一致しません",
            Self::AccessTokenHashMismatch => "DPoP proof の ath がアクセストークンと一致しません",
            Self::IssuedAtOutOfWindow => "DPoP proof の iat が許容窓の外です",
            Self::InvalidRequestTarget => "リクエストターゲットから期待 URI を作れません",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for DpopError {}

/// 1 リクエストぶんの検証文脈。フィールドは非公開で、[`ProofContext::new`] で作る。
///
/// ```
/// use fandhe_backend_plugin_jwt::dpop::{IatWindow, ProofContext, PublicOrigin};
/// use fandhe_backend_plugin_jwt::{Algorithm, AllowedAlgorithms};
///
/// let htu = PublicOrigin::new("https://api.example")?.expected_htu("/")?;
/// let algs = AllowedAlgorithms::only(Algorithm::Es256);
/// let _ctx = ProofContext::new("POST", &htu, "token", &algs, IatWindow::default());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy)]
pub struct ProofContext<'a> {
    method: &'a str,
    htu: &'a ExpectedHtu,
    access_token: &'a str,
    algs: &'a AllowedAlgorithms,
    window: IatWindow,
}

impl<'a> ProofContext<'a> {
    /// `method` はリクエストのメソッド（`htm` と大文字小文字を区別して完全一致を比べる）、
    /// `access_token` は `Authorization: DPoP` で受け取った生のアクセストークン（`ath` の
    /// 計算元。`Debug` には出さない）。
    pub fn new(
        method: &'a str,
        htu: &'a ExpectedHtu,
        access_token: &'a str,
        algs: &'a AllowedAlgorithms,
        window: IatWindow,
    ) -> Self {
        Self {
            method,
            htu,
            access_token,
            algs,
            window,
        }
    }
}

/// アクセストークンはログへ出さない。
impl fmt::Debug for ProofContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofContext")
            .field("method", &self.method)
            .field("htu", &self.htu)
            .field("window", &self.window)
            .finish_non_exhaustive()
    }
}

/// 検証済みの proof。`jti` は秘密ではないがログへ出さないよう `Debug` から除く。
///
/// ```
/// # use fandhe_backend_plugin_jwt::dpop::VerifiedProof;
/// fn show(p: &VerifiedProof) -> (&str, &str, u64) { (p.thumbprint().as_str(), p.jti(), p.iat()) }
/// # let _ = show;
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedProof {
    thumbprint: JwkThumbprint,
    jti: Box<str>,
    iat: u64,
}

impl VerifiedProof {
    /// proof の `jwk` の RFC 7638 サムプリント。
    pub fn thumbprint(&self) -> &JwkThumbprint {
        &self.thumbprint
    }

    /// proof の `jti`（再使用の検知は #903 が担う）。
    pub fn jti(&self) -> &str {
        &self.jti
    }

    /// proof の `iat`（UNIX 秒）。
    pub fn iat(&self) -> u64 {
        self.iat
    }
}

impl fmt::Debug for VerifiedProof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedProof")
            .field("thumbprint", &self.thumbprint)
            .field("iat", &self.iat)
            .finish_non_exhaustive()
    }
}

/// キーが存在すれば値の型を問わず `true`（`null` でも存在扱い）。
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(d)?;
    Ok(true)
}

/// ヘッダ。`jku`・`x5u`・`x5c`・`kid` は意図的に読まない。重複キーは serde がエラーにする。
#[derive(Deserialize)]
struct RawProofHeader {
    #[serde(default)]
    alg: Option<String>,
    #[serde(default)]
    typ: Option<String>,
    #[serde(default, deserialize_with = "present")]
    crit: bool,
    #[serde(default)]
    jwk: Option<ProofJwk>,
}

#[derive(Deserialize)]
struct RawProofClaims {
    jti: Option<String>,
    htm: Option<String>,
    htu: Option<String>,
    iat: Option<u64>,
    ath: Option<String>,
}

fn b64(s: &str) -> Result<Vec<u8>, DpopError> {
    URL_SAFE_NO_PAD.decode(s).map_err(|_| DpopError::Malformed)
}

/// JSON のトップレベルがオブジェクトであることを要求する（配列からの復元を拒否）。
fn require_json_object(bytes: &[u8]) -> Result<(), DpopError> {
    if bytes.trim_ascii_start().first() == Some(&b'{') {
        Ok(())
    } else {
        Err(DpopError::Malformed)
    }
}

fn typ_is_dpop(typ: &str) -> bool {
    let t = typ.strip_prefix("application/").unwrap_or(typ);
    t.eq_ignore_ascii_case("dpop+jwt")
}

/// DPoP proof を検証し、鍵のサムプリントを返す。順序と拒否理由はモジュール doc の契約に従う。
///
/// `now_unix` は呼び出し側が注入する現在時刻（UNIX 秒）。時刻クレーム `iat` は非負整数のみ
/// 受理し、小数・負数・型違いは [`DpopError::Malformed`]。検証鍵は proof の `jwk` のみで、
/// 外部から鍵を取得しない。
///
/// ```
/// use base64::Engine;
/// use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
/// use fandhe_backend_plugin_jwt::dpop::{IatWindow, ProofContext, PublicOrigin, verify_proof};
/// use fandhe_backend_plugin_jwt::{Algorithm, AllowedAlgorithms};
/// use ring::digest::{SHA256, digest};
/// use ring::rand::SystemRandom;
/// use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
///
/// let rng = SystemRandom::new();
/// let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
/// let kp = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
///     .unwrap();
/// let pt = kp.public_key().as_ref();
/// let header = format!(
///     r#"{{"typ":"dpop+jwt","alg":"ES256","jwk":{{"kty":"EC","crv":"P-256","x":"{}","y":"{}"}}}}"#,
///     B.encode(&pt[1..33]),
///     B.encode(&pt[33..65])
/// );
/// let ath = B.encode(digest(&SHA256, b"access-token").as_ref());
/// let payload = format!(
///     r#"{{"jti":"j1","htm":"GET","htu":"https://api.example/resource?x=1","iat":1000,"ath":"{ath}"}}"#
/// );
/// let msg = format!("{}.{}", B.encode(header), B.encode(payload));
/// let sig = kp.sign(&rng, msg.as_bytes()).unwrap();
/// let proof = format!("{msg}.{}", B.encode(sig.as_ref()));
///
/// let htu = PublicOrigin::new("https://api.example")?.expected_htu("/resource")?;
/// let algs = AllowedAlgorithms::only(Algorithm::Es256);
/// let ctx = ProofContext::new("GET", &htu, "access-token", &algs, IatWindow::default());
/// let verified = verify_proof(&proof, &ctx, 1010)?;
/// assert_eq!(verified.jti(), "j1");
/// assert_eq!(verified.thumbprint().as_str().len(), 43);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn verify_proof(
    proof: &str,
    ctx: &ProofContext<'_>,
    now_unix: u64,
) -> Result<VerifiedProof, DpopError> {
    if proof.is_empty() {
        return Err(DpopError::MissingProof);
    }
    if proof.len() > MAX_PROOF_LEN {
        return Err(DpopError::TooLong);
    }
    let mut parts = proof.split('.');
    let (Some(h), Some(p), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(DpopError::Malformed);
    };
    if h.is_empty() || p.is_empty() || s.is_empty() {
        return Err(DpopError::Malformed);
    }

    let header_bytes = b64(h)?;
    require_json_object(&header_bytes)?;
    let header: RawProofHeader =
        serde_json::from_slice(&header_bytes).map_err(|_| DpopError::Malformed)?;
    if header.crit {
        return Err(DpopError::CriticalHeader);
    }
    if !header.typ.as_deref().is_some_and(typ_is_dpop) {
        return Err(DpopError::InvalidType);
    }
    let alg = header
        .alg
        .as_deref()
        .and_then(Algorithm::from_jose_name)
        .filter(|a| ctx.algs.contains(*a))
        .ok_or(DpopError::AlgorithmNotAllowed)?;
    let jwk = header.jwk.as_ref().ok_or(DpopError::MissingJwk)?;
    let proof_key = build_proof_key(jwk, Some(alg))?;

    let sig = b64(s)?;
    // 署名対象は元の文字列の `header.payload` 部分をそのまま使う（再エンコードしない）。
    let msg = &proof[..h.len() + 1 + p.len()];
    if !signature_ok(alg, &proof_key.material, msg.as_bytes(), &sig) {
        return Err(DpopError::InvalidSignature);
    }

    // ここから先は署名検証済みの payload のみを扱う。
    let payload_bytes = b64(p)?;
    require_json_object(&payload_bytes)?;
    let claims: RawProofClaims =
        serde_json::from_slice(&payload_bytes).map_err(|_| DpopError::Malformed)?;
    let (Some(jti), Some(htm), Some(htu), Some(iat), Some(ath)) =
        (claims.jti, claims.htm, claims.htu, claims.iat, claims.ath)
    else {
        return Err(DpopError::MissingClaim);
    };
    if jti.is_empty() || jti.len() > MAX_JTI_LEN {
        return Err(DpopError::InvalidJti);
    }
    if htm != ctx.method {
        return Err(DpopError::MethodMismatch);
    }
    match origin::normalize_proof_htu(&htu) {
        Some(n) if n == ctx.htu.as_str() => {}
        _ => return Err(DpopError::UriMismatch),
    }
    let lo = now_unix.saturating_sub(ctx.window.past.as_secs());
    let hi = now_unix.saturating_add(ctx.window.future.as_secs());
    if iat < lo || iat > hi {
        return Err(DpopError::IssuedAtOutOfWindow);
    }
    // `ath` はアクセストークンの公開ハッシュで秘密ではないため、定数時間比較は不要。
    let expected_ath =
        URL_SAFE_NO_PAD.encode(digest(&SHA256, ctx.access_token.as_bytes()).as_ref());
    if ath != expected_ath {
        return Err(DpopError::AccessTokenHashMismatch);
    }

    Ok(VerifiedProof {
        thumbprint: proof_key.thumbprint,
        jti: jti.into_boxed_str(),
        iat,
    })
}
