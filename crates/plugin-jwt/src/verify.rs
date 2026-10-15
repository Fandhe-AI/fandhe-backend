//! JWT 検証関数 [`verify`]（RS256・ES256 の署名検証とクレーム検証、イシュー #815）。
//!
//! 後続の `RequestGate`（#816）が同期で呼ぶ純関数で、I/O・ロックを持たない。時刻は
//! 呼び出し側が注入する。ポリシーは [`JwtPolicy`]（#813）、鍵は [`KeySet`]（#814）から
//! 受け取り、本モジュールは両者をつなぐだけで自前の暗号演算は持たない（署名検証は ring）。
//!
//! # 検証順序（契約）
//!
//! 1. 長さ（[`MAX_TOKEN_LEN`]）と 3 区画構造
//! 2. ヘッダ解析 → `crit` 拒否 → `alg`（ポリシーの許可リスト）→ `kid` → 鍵の特定
//! 3. 署名検証（候補鍵を JWKS 文書順に試し、最初に成功した鍵で受理）
//! 4. **署名検証が済むまで payload は解析しない**（未検証の値で分岐しない）
//! 5. `typ` → payload 解析 → `iss` → `aud` → `exp` → `nbf` → 追加クレーム
//!
//! ヘッダの `alg` は信用せず、ポリシーの許可リストと鍵に固定された方式の双方に一致した
//! ときだけ受理する（RFC 8725 3.1・3.2）。`jku`・`x5u`・`jwk`・`x5c` は読まず無視し、
//! 鍵はトークン外の [`KeySet`] からのみ選ぶ。どのような入力でもパニックせず `Err` を返す。

use std::borrow::Cow;
use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED, RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents, UnparsedPublicKey,
};
use serde::de::{self, DeserializeOwned, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::jwks::{Jwk, KeyMaterial, KeySet, MAX_KID_LEN};
use crate::policy::{Algorithm, JwtPolicy};

/// 受理するトークン全体の最大バイト長（超過は [`VerifyError::TooLong`]）。
pub const MAX_TOKEN_LEN: usize = 8192;

/// トークン側 `aud` 配列の最大要素数（超過は [`VerifyError::Malformed`]）。
pub const MAX_TOKEN_AUDIENCES: usize = 16;

/// 検証失敗の理由。文字列を持たず、トークン・`kid`・クレーム値を含まない。
///
/// ```
/// use fandhe_backend_plugin_jwt::VerifyError;
/// assert_eq!(VerifyError::Expired.to_string(), "トークンの有効期限が切れています");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VerifyError {
    /// トークンが空。
    MissingToken,
    /// トークンが [`MAX_TOKEN_LEN`] を超えた。
    TooLong,
    /// 構造不正（区画数・base64・JSON・型・上限超過など）。
    Malformed,
    /// `alg` が許可リスト外（`none`・HS 系・未知の名前を含む）。
    AlgorithmNotAllowed,
    /// ヘッダに `kid` がない、または空。
    MissingKeyId,
    /// `kid` に一致する鍵がない（鍵集合が空の場合を含む）。
    UnknownKeyId,
    /// `kid` の鍵はあるが、ヘッダの `alg` と方式が一致する鍵がない。
    KeyAlgorithmMismatch,
    /// 署名検証に失敗した。
    InvalidSignature,
    /// ヘッダに `crit` がある（理解できる拡張はないため拒否）。
    CriticalHeader,
    /// `typ` の欠落、またはポリシーの種別と不一致。
    InvalidType,
    /// `iss` がポリシーと一致しない。
    IssuerMismatch,
    /// `aud` がポリシーのどの値とも一致しない。
    AudienceMismatch,
    /// 有効期限切れ（`exp`）。
    Expired,
    /// まだ有効でない（`nbf`）。
    NotYetValid,
    /// 必須クレーム（`iss`・`aud`・`exp`）の欠落。
    MissingClaim,
    /// 追加クレーム型 `E` への読み込みに失敗した。
    InvalidExtraClaims,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::MissingToken => "トークンがありません",
            Self::TooLong => "トークンが長すぎます",
            Self::Malformed => "トークンの形式が不正です",
            Self::AlgorithmNotAllowed => "許可されていない署名アルゴリズムです",
            Self::MissingKeyId => "ヘッダに kid がありません",
            Self::UnknownKeyId => "kid に一致する鍵がありません",
            Self::KeyAlgorithmMismatch => "鍵の署名方式がヘッダの alg と一致しません",
            Self::InvalidSignature => "署名が不正です",
            Self::CriticalHeader => "crit ヘッダは受け付けません",
            Self::InvalidType => "トークン種別（typ）が不正です",
            Self::IssuerMismatch => "発行者（iss）が一致しません",
            Self::AudienceMismatch => "受信者（aud）が一致しません",
            Self::Expired => "トークンの有効期限が切れています",
            Self::NotYetValid => "トークンはまだ有効ではありません",
            Self::MissingClaim => "必須クレームがありません",
            Self::InvalidExtraClaims => "追加クレームの形式が不正です",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for VerifyError {}

/// 追加クレームを使わないときの既定型。任意の値（`null` 以外の型も）を読み捨てる。
///
/// `()` は JSON の `null` しか受理しないため、代わりに本型を使う。
///
/// ```
/// use fandhe_backend_plugin_jwt::NoExtraClaims;
/// let _: NoExtraClaims = serde_json::from_str(r#"{"any":[1,2,3]}"#).unwrap();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NoExtraClaims;

impl<'de> Deserialize<'de> for NoExtraClaims {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        IgnoredAny::deserialize(d)?;
        Ok(Self)
    }
}

/// 検証済みのクレーム。`sub` などの識別子を含むためログへ出さないこと。
///
/// 生の payload は保持しない。追加クレームは型 `E` として [`Claims::extra`] で読む。
///
/// ```
/// use fandhe_backend_plugin_jwt::{Claims, NoExtraClaims};
/// fn show(c: &Claims<NoExtraClaims>) -> (&str, u64) { (c.iss(), c.exp()) }
/// # let _ = show;
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Claims<E = NoExtraClaims> {
    iss: String,
    sub: Option<String>,
    aud: Vec<String>,
    exp: u64,
    nbf: Option<u64>,
    iat: Option<u64>,
    jti: Option<String>,
    client_id: Option<String>,
    scope: Option<String>,
    extra: E,
}

/// `sub`・`jti`・`client_id`・`scope`・`extra` の値は出さず、有無だけを示す（ログ漏洩防止）。
impl<E> fmt::Debug for Claims<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Claims")
            .field("iss", &self.iss)
            .field("aud_count", &self.aud.len())
            .field("exp", &self.exp)
            .field("nbf", &self.nbf)
            .field("iat", &self.iat)
            .field("has_sub", &self.sub.is_some())
            .field("has_jti", &self.jti.is_some())
            .field("has_client_id", &self.client_id.is_some())
            .field("has_scope", &self.scope.is_some())
            .finish_non_exhaustive()
    }
}

impl<E> Claims<E> {
    /// 発行者（検証済みでポリシーの `iss` と一致）。
    pub fn iss(&self) -> &str {
        &self.iss
    }
    /// 主体（`sub`）。
    pub fn sub(&self) -> Option<&str> {
        self.sub.as_deref()
    }
    /// 受信者（文字列 1 個でも配列に正規化）。
    pub fn aud(&self) -> &[String] {
        &self.aud
    }
    /// 有効期限（UNIX 秒）。
    pub fn exp(&self) -> u64 {
        self.exp
    }
    /// 有効開始時刻（UNIX 秒）。
    pub fn nbf(&self) -> Option<u64> {
        self.nbf
    }
    /// 発行時刻（UNIX 秒、判定には使わない）。
    pub fn iat(&self) -> Option<u64> {
        self.iat
    }
    /// トークン ID。
    pub fn jti(&self) -> Option<&str> {
        self.jti.as_deref()
    }
    /// クライアント ID（RFC 9068）。
    pub fn client_id(&self) -> Option<&str> {
        self.client_id.as_deref()
    }
    /// スコープ（空白区切りの生文字列）。
    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }
    /// 追加クレーム。
    pub fn extra(&self) -> &E {
        &self.extra
    }
    /// 追加クレームを取り出して消費する。
    pub fn into_extra(self) -> E {
        self.extra
    }
}

/// ヘッダ。`jku`・`x5u`・`jwk`・`x5c` は意図的に読まない。重複キーは serde がエラーにする。
#[derive(Deserialize)]
struct RawHeader<'a> {
    #[serde(borrow)]
    alg: Cow<'a, str>,
    #[serde(borrow, default)]
    kid: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    typ: Option<Cow<'a, str>>,
    #[serde(default, deserialize_with = "present")]
    crit: bool,
}

/// キーが存在すれば値の型を問わず `true`（`null` でも存在扱い）。
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(d)?;
    Ok(true)
}

/// `aud` は文字列 1 個または文字列配列。
struct AudClaim(Vec<String>);

impl<'de> Deserialize<'de> for AudClaim {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = AudClaim;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("string or array of strings")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<AudClaim, E> {
                Ok(AudClaim(vec![v.to_owned()]))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<AudClaim, A::Error> {
                let mut out = Vec::new();
                while let Some(s) = seq.next_element::<String>()? {
                    if out.len() >= MAX_TOKEN_AUDIENCES {
                        return Err(de::Error::custom("too many audiences"));
                    }
                    out.push(s);
                }
                Ok(AudClaim(out))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Deserialize)]
struct RawClaims {
    iss: Option<String>,
    sub: Option<String>,
    aud: Option<AudClaim>,
    exp: Option<u64>,
    nbf: Option<u64>,
    iat: Option<u64>,
    jti: Option<String>,
    client_id: Option<String>,
    scope: Option<String>,
}

fn b64(s: &str) -> Result<Vec<u8>, VerifyError> {
    URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| VerifyError::Malformed)
}

/// 1 鍵での署名検証。方式と鍵成分が食い違う組は（鍵集合の不変条件上起きないが）失敗扱い。
fn signature_ok(alg: Algorithm, jwk: &Jwk, msg: &[u8], sig: &[u8]) -> bool {
    match (alg, jwk.material()) {
        (Algorithm::Rs256, KeyMaterial::Rsa { n, e }) => RsaPublicKeyComponents {
            n: &n[..],
            e: &e[..],
        }
        .verify(&RSA_PKCS1_2048_8192_SHA256, msg, sig)
        .is_ok(),
        (Algorithm::Es256, KeyMaterial::EcP256 { point }) => {
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &point[..])
                .verify(msg, sig)
                .is_ok()
        }
        _ => false,
    }
}

/// トークンを検証してクレームを返す。順序と拒否理由はモジュール doc の契約に従う。
///
/// `now_unix` は呼び出し側が注入する現在時刻（UNIX 秒）。時刻クレームは非負整数のみ
/// 受理し、小数・負数・型違いは [`VerifyError::Malformed`]。`E` を使わない場合は
/// `verify::<NoExtraClaims>(..)` と書く。
///
/// ```
/// use fandhe_backend_plugin_jwt::*;
/// let policy = JwtPolicy::new(
///     Issuer::new("https://issuer.example")?,
///     Audiences::single("api")?,
///     AllowedAlgorithms::only(Algorithm::Es256),
///     TokenType::AccessToken,
/// );
/// let r = verify::<NoExtraClaims>("", &KeySet::empty(), &policy, 0);
/// assert_eq!(r.unwrap_err(), VerifyError::MissingToken);
/// # Ok::<(), PolicyError>(())
/// ```
///
/// 署名付きトークンの受理例（ES256）。
///
/// ```
/// use base64::Engine;
/// use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
/// use fandhe_backend_plugin_jwt::*;
/// use ring::rand::SystemRandom;
/// use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
///
/// let rng = SystemRandom::new();
/// let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
/// let kp = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
///     .unwrap();
/// let pt = kp.public_key().as_ref();
/// let jwks = format!(
///     r#"{{"keys":[{{"kty":"EC","kid":"k1","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}]}}"#,
///     B.encode(&pt[1..33]),
///     B.encode(&pt[33..65])
/// );
/// let keys = KeySet::from_json(&jwks).unwrap();
///
/// let head = B.encode(r#"{"alg":"ES256","kid":"k1","typ":"at+jwt"}"#);
/// let body = B.encode(r#"{"iss":"https://issuer.example","aud":"api","exp":2000,"sub":"u1"}"#);
/// let msg = format!("{head}.{body}");
/// let sig = kp.sign(&rng, msg.as_bytes()).unwrap();
/// let token = format!("{msg}.{}", B.encode(sig.as_ref()));
///
/// let policy = JwtPolicy::new(
///     Issuer::new("https://issuer.example").unwrap(),
///     Audiences::single("api").unwrap(),
///     AllowedAlgorithms::only(Algorithm::Es256),
///     TokenType::AccessToken,
/// );
/// let claims = verify::<NoExtraClaims>(&token, &keys, &policy, 1000).unwrap();
/// assert_eq!(claims.iss(), "https://issuer.example");
/// assert_eq!(claims.sub(), Some("u1"));
/// assert_eq!(verify::<NoExtraClaims>(&token, &keys, &policy, 2000).unwrap_err(),
///     VerifyError::Expired);
/// ```
pub fn verify<E: DeserializeOwned>(
    token: &str,
    keys: &KeySet,
    policy: &JwtPolicy,
    now_unix: u64,
) -> Result<Claims<E>, VerifyError> {
    if token.is_empty() {
        return Err(VerifyError::MissingToken);
    }
    if token.len() > MAX_TOKEN_LEN {
        return Err(VerifyError::TooLong);
    }
    let mut parts = token.split('.');
    let (Some(h), Some(p), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(VerifyError::Malformed);
    };
    if h.is_empty() || p.is_empty() || s.is_empty() {
        return Err(VerifyError::Malformed);
    }

    let header_bytes = b64(h)?;
    let header: RawHeader<'_> =
        serde_json::from_slice(&header_bytes).map_err(|_| VerifyError::Malformed)?;
    if header.crit {
        return Err(VerifyError::CriticalHeader);
    }
    let alg = Algorithm::from_jose_name(&header.alg)
        .filter(|a| policy.algorithms().contains(*a))
        .ok_or(VerifyError::AlgorithmNotAllowed)?;
    let kid = match header.kid.as_deref() {
        None | Some("") => return Err(VerifyError::MissingKeyId),
        Some(k) if k.len() > MAX_KID_LEN => return Err(VerifyError::Malformed),
        Some(k) => k,
    };
    let mut any_kid = false;
    let candidates: Vec<&Jwk> = keys
        .keys_for(kid)
        .inspect(|_| any_kid = true)
        .filter(|k| k.algorithm() == alg)
        .collect();
    if candidates.is_empty() {
        return Err(if any_kid {
            VerifyError::KeyAlgorithmMismatch
        } else {
            VerifyError::UnknownKeyId
        });
    }

    let sig = b64(s)?;
    // 署名対象は元のトークンの `header.payload` 部分をそのまま使う（再エンコードしない）。
    let msg = &token[..h.len() + 1 + p.len()];
    if !candidates
        .iter()
        .any(|k| signature_ok(alg, k, msg.as_bytes(), &sig))
    {
        return Err(VerifyError::InvalidSignature);
    }

    // ここから先は署名検証済みの payload のみを扱う。
    match header.typ.as_deref() {
        Some(t) if policy.token_type().matches_header(t) => {}
        _ => return Err(VerifyError::InvalidType),
    }
    let payload = b64(p)?;
    let raw: RawClaims = serde_json::from_slice(&payload).map_err(|_| VerifyError::Malformed)?;

    let iss = raw.iss.ok_or(VerifyError::MissingClaim)?;
    if iss != policy.issuer().as_str() {
        return Err(VerifyError::IssuerMismatch);
    }
    let aud = raw.aud.ok_or(VerifyError::MissingClaim)?.0;
    if !aud.iter().any(|a| policy.audiences().contains(a)) {
        return Err(VerifyError::AudienceMismatch);
    }
    let exp = raw.exp.ok_or(VerifyError::MissingClaim)?;
    let leeway = policy.clock_skew().as_secs();
    if now_unix.saturating_sub(leeway) >= exp {
        return Err(VerifyError::Expired);
    }
    if let Some(nbf) = raw.nbf
        && now_unix.saturating_add(leeway) < nbf
    {
        return Err(VerifyError::NotYetValid);
    }
    let extra: E = serde_json::from_slice(&payload).map_err(|_| VerifyError::InvalidExtraClaims)?;

    Ok(Claims {
        iss,
        sub: raw.sub,
        aud,
        exp,
        nbf: raw.nbf,
        iat: raw.iat,
        jti: raw.jti,
        client_id: raw.client_id,
        scope: raw.scope,
        extra,
    })
}
