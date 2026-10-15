//! JWKS（RFC 7517）の鍵集合: RSA（RS256）と EC P-256（ES256）の公開鍵を解析・保持する。
//!
//! 役割: 後続の署名検証（#815）とキャッシュ付き `RequestGate`（#816）が参照する
//! 検証鍵の集合を、フェイルクローズで構築する。設計は
//! `docs/design/jwt-verify-plugin.md` 2.3 節。
//!
//! # 契約
//!
//! - 文書長 → JSON → 各エントリ → 件数の順に検査し、1 件でも不正なら文書全体を
//!   拒否する（一部だけ採用しない）。
//! - 採用する鍵は `kid` と `alg` が必須。種別（`kty`・`crv`）と `alg` が矛盾する鍵、
//!   `oct`・`none`・HS 系・秘密鍵成分を含む鍵は文書全体を拒否する。
//! - 矛盾しない未対応の鍵（`use: enc`、RS384 など、P-256 以外の EC、OKP）は読み飛ばし、
//!   件数を [`KeySet::skipped`] で参照できる。
//! - 同じ `kid` に最大 [`MAX_KEYS_PER_KID`] 件（前・現・次）の鍵を持てる。各鍵は
//!   方式（[`Algorithm`]）が固定される。
//! - EC の点が曲線上にあるか、RSA のモジュラスが十分な長さかは解析時には保証しない。
//!   署名検証時に ring が拒否する（#815、フェイルクローズ）。
//! - JWKS の取得（HTTP 等）は本クレートの責務外。`RequestGate` は同期・I/O なしの
//!   契約のため、利用側が取得した文書を [`SharedKeySet::set`] で差し替える。

use std::borrow::Cow;
use std::fmt;
use std::sync::{Arc, RwLock};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};

use crate::policy::Algorithm;

/// JWKS 文書の最大バイト長（解析前に判定する）。
pub const MAX_JWKS_DOC_LEN: usize = 64 * 1024;
/// 採用する鍵の総数の上限（読み飛ばした鍵は数えない）。
pub const MAX_KEYS: usize = 32;
/// 同じ `kid` に持てる鍵の上限（前・現・次）。
pub const MAX_KEYS_PER_KID: usize = 3;
/// JWK の `kid` の最大バイト長。
pub const MAX_KID_LEN: usize = 256;

const MAX_RSA_N_LEN: usize = 1024;
const MAX_RSA_E_LEN: usize = 8;
const EC_P256_COORD_LEN: usize = 32;

/// JWKS 解析のエラー。固定文言で、入力値（`kid`・鍵値）は含まない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum JwksError {
    /// 文書長が [`MAX_JWKS_DOC_LEN`] を超えた。
    DocumentTooLarge,
    /// JSON として、または JWKS の構造として不正。
    InvalidJson,
    /// 採用した鍵の総数が [`MAX_KEYS`] を超えた。
    TooManyKeys,
    /// 同じ `kid` の鍵数が [`MAX_KEYS_PER_KID`] を超えた。
    TooManyKeysForKeyId,
    /// 採用する鍵に `kid` がない、または空。
    MissingKeyId,
    /// `kid` が [`MAX_KID_LEN`] を超えた。
    KeyIdTooLong,
    /// 採用する鍵に `alg` がない。
    MissingAlgorithm,
    /// 鍵の種別と `alg` が矛盾する、または `alg` が未知。
    KeyAlgorithmMismatch,
    /// 未対応の曲線（ES256 に P-256 以外、または EC で `crv` 欠落）。
    UnsupportedCurve,
    /// `alg` が `none` または HS 系。
    ForbiddenAlgorithm,
    /// 共通鍵（`kty: oct`）。
    SymmetricKey,
    /// 秘密鍵成分を含む。
    PrivateKeyMaterial,
    /// 鍵成分（`n`・`e`・`x`・`y`）が欠けている。
    MissingKeyComponent,
    /// 鍵成分が base64url（パディングなし）として不正。
    InvalidKeyEncoding,
    /// 鍵成分が長さ上限を超えた。
    KeyTooLarge,
    /// EC 座標が 32 バイトではない。
    InvalidKeyLength,
    /// `kid`・`alg`・鍵値がすべて同じ鍵が重複している。
    DuplicateKey,
    /// `keys` が空でないのに採用できる鍵が 0 件。
    NoUsableKeys,
}

impl fmt::Display for JwksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::DocumentTooLarge => "JWKS 文書が長さ上限を超えています",
            Self::InvalidJson => "JWKS が JSON または構造として不正です",
            Self::TooManyKeys => "JWKS の鍵数が上限を超えています",
            Self::TooManyKeysForKeyId => "同一 kid の鍵数が上限を超えています",
            Self::MissingKeyId => "kid がない、または空の鍵があります",
            Self::KeyIdTooLong => "kid が長さ上限を超えています",
            Self::MissingAlgorithm => "alg のない鍵があります",
            Self::KeyAlgorithmMismatch => "鍵の種別と alg が矛盾しています",
            Self::UnsupportedCurve => "未対応の曲線、または crv がありません",
            Self::ForbiddenAlgorithm => "禁止された alg が指定されています",
            Self::SymmetricKey => "共通鍵は受け付けません",
            Self::PrivateKeyMaterial => "秘密鍵成分を含む鍵は受け付けません",
            Self::MissingKeyComponent => "鍵成分が欠けています",
            Self::InvalidKeyEncoding => "鍵成分のエンコーディングが不正です",
            Self::KeyTooLarge => "鍵成分が長さ上限を超えています",
            Self::InvalidKeyLength => "EC 座標の長さが不正です",
            Self::DuplicateKey => "同一の鍵が重複しています",
            Self::NoUsableKeys => "採用できる鍵がありません",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for JwksError {}

/// 鍵成分。署名検証（#815）が ring の入力へ組み立てる。
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum KeyMaterial {
    /// RSA の公開鍵（big-endian、先頭 0 除去済み）。
    Rsa { n: Box<[u8]>, e: Box<[u8]> },
    /// EC P-256 の非圧縮点 `0x04 || x || y`。
    EcP256 { point: [u8; 65] },
}

/// 採用した検証鍵 1 件。`kid` と方式が固定される。
#[derive(Clone, PartialEq, Eq)]
pub struct Jwk {
    kid: Box<str>,
    alg: Algorithm,
    material: KeyMaterial,
}

impl Jwk {
    /// この鍵の `kid`。
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// この鍵に固定された署名方式。
    pub fn algorithm(&self) -> Algorithm {
        self.alg
    }
}

impl fmt::Debug for Jwk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Jwk")
            .field("kid", &self.kid)
            .field("alg", &self.alg)
            .finish_non_exhaustive()
    }
}

/// 検証鍵の集合。文書の順に鍵を保持する。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_jwt::{Algorithm, KeySet};
///
/// let json = r#"{"keys":[{"kty":"EC","crv":"P-256","kid":"k1","alg":"ES256",
///   "x":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
///   "y":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}]}"#;
/// let set = KeySet::from_json(json).unwrap();
/// assert_eq!(set.len(), 1);
/// assert_eq!(set.keys_for("k1").next().unwrap().algorithm(), Algorithm::Es256);
/// ```
#[derive(Clone, PartialEq, Eq, Default)]
pub struct KeySet {
    keys: Box<[Jwk]>,
    skipped: usize,
}

impl fmt::Debug for KeySet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeySet")
            .field("keys", &self.keys)
            .field("skipped", &self.skipped)
            .finish()
    }
}

impl KeySet {
    /// 空の鍵集合。どのトークンも検証できず、全件拒否になる。
    ///
    /// ```
    /// assert!(fandhe_backend_plugin_jwt::KeySet::empty().is_empty());
    /// ```
    pub fn empty() -> Self {
        Self::default()
    }

    /// JWKS の JSON 文字列から鍵集合を構築する。
    ///
    /// `{"keys":[]}` のときだけ空集合を返す。`keys` が空でないのに採用できる鍵が
    /// 0 件なら [`JwksError::NoUsableKeys`]。
    ///
    /// ```
    /// use fandhe_backend_plugin_jwt::{JwksError, KeySet};
    ///
    /// assert!(KeySet::from_json(r#"{"keys":[]}"#).unwrap().is_empty());
    /// assert_eq!(KeySet::from_json("{").unwrap_err(), JwksError::InvalidJson);
    /// ```
    pub fn from_json(json: &str) -> Result<Self, JwksError> {
        if json.len() > MAX_JWKS_DOC_LEN {
            return Err(JwksError::DocumentTooLarge);
        }
        let raw: RawSet<'_> = serde_json::from_str(json).map_err(|_| JwksError::InvalidJson)?;
        let total = raw.keys.len();
        let mut keys: Vec<Jwk> = Vec::new();
        let mut skipped = 0usize;
        for entry in &raw.keys {
            match classify(entry)? {
                Some(jwk) => keys.push(jwk),
                None => skipped += 1,
            }
        }
        if keys.len() > MAX_KEYS {
            return Err(JwksError::TooManyKeys);
        }
        for (i, a) in keys.iter().enumerate() {
            let same_kid = keys.iter().filter(|b| b.kid == a.kid).count();
            if same_kid > MAX_KEYS_PER_KID {
                return Err(JwksError::TooManyKeysForKeyId);
            }
            if keys[..i].iter().any(|b| b == a) {
                return Err(JwksError::DuplicateKey);
            }
        }
        if total > 0 && keys.is_empty() {
            return Err(JwksError::NoUsableKeys);
        }
        Ok(Self {
            keys: keys.into_boxed_slice(),
            skipped,
        })
    }

    /// 採用した鍵の件数。
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// 採用した鍵が 0 件か。
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// 読み飛ばした鍵の件数（黙って捨てず、運用側が観測できるようにする）。
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// 指定 `kid` の鍵を文書の順で返す（鍵の回転中は最大 3 件）。
    ///
    /// ```
    /// assert_eq!(fandhe_backend_plugin_jwt::KeySet::empty().keys_for("x").count(), 0);
    /// ```
    pub fn keys_for<'a>(&'a self, kid: &'a str) -> impl Iterator<Item = &'a Jwk> + 'a {
        self.keys.iter().filter(move |k| &*k.kid == kid)
    }
}

/// 稼働中に差し替え可能な共有鍵集合。
///
/// [`snapshot`](Self::snapshot) は内側の `Arc` をそのまま返す。差し替えの有無は
/// `Arc::ptr_eq` で判定できる（#816 のキャッシュ無効化が依存する契約）。
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use fandhe_backend_plugin_jwt::{KeySet, SharedKeySet};
///
/// let shared = SharedKeySet::new(KeySet::empty());
/// let before = shared.snapshot();
/// shared.set(KeySet::empty());
/// assert!(!Arc::ptr_eq(&before, &shared.snapshot()));
/// ```
#[derive(Debug, Clone)]
pub struct SharedKeySet(Arc<RwLock<Arc<KeySet>>>);

impl SharedKeySet {
    /// 鍵集合から共有ハンドルを作る。
    pub fn new(set: KeySet) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(set))))
    }

    /// JWKS の JSON から共有ハンドルを作る。
    ///
    /// ```
    /// assert!(fandhe_backend_plugin_jwt::SharedKeySet::from_json(r#"{"keys":[]}"#).is_ok());
    /// ```
    pub fn from_json(json: &str) -> Result<Self, JwksError> {
        KeySet::from_json(json).map(Self::new)
    }

    /// 現在の鍵集合を取得する。ロックは短時間だけ保持する。
    pub fn snapshot(&self) -> Arc<KeySet> {
        // poison しても保持する値は常に整合している（差し替えは代入 1 回のみ）ため回復する。
        let guard = self.0.read().unwrap_or_else(|e| e.into_inner());
        Arc::clone(&guard)
    }

    /// 鍵集合を差し替える。以降の [`snapshot`](Self::snapshot) は新しい集合を返す。
    pub fn set(&self, set: KeySet) {
        let mut guard = self.0.write().unwrap_or_else(|e| e.into_inner());
        *guard = Arc::new(set);
    }
}

// ---- 解析 ----

#[derive(Deserialize)]
struct RawSet<'a> {
    #[serde(borrow)]
    keys: Vec<RawJwk<'a>>,
}

/// 値の型を問わず「キーが存在すること」だけを読み取る（`null` も存在扱い）。
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(d)?;
    Ok(true)
}

#[derive(Deserialize)]
struct RawJwk<'a> {
    #[serde(borrow, default)]
    kty: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    kid: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    alg: Option<Cow<'a, str>>,
    #[serde(borrow, default, rename = "use")]
    use_: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    crv: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    n: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    e: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    x: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    y: Option<Cow<'a, str>>,
    #[serde(default, deserialize_with = "present")]
    d: bool,
    #[serde(default, deserialize_with = "present")]
    p: bool,
    #[serde(default, deserialize_with = "present")]
    q: bool,
    #[serde(default, deserialize_with = "present")]
    dp: bool,
    #[serde(default, deserialize_with = "present")]
    dq: bool,
    #[serde(default, deserialize_with = "present")]
    qi: bool,
    #[serde(default, deserialize_with = "present")]
    oth: bool,
    #[serde(default, deserialize_with = "present")]
    k: bool,
}

/// 矛盾しない未対応の方式（採用せず読み飛ばす）。
const SKIPPABLE_RSA_ALGS: &[&str] = &[
    "RS384",
    "RS512",
    "PS256",
    "PS384",
    "PS512",
    "RSA1_5",
    "RSA-OAEP",
    "RSA-OAEP-256",
];
const SKIPPABLE_EC_ALGS: &[&str] = &[
    "ECDH-ES",
    "ECDH-ES+A128KW",
    "ECDH-ES+A192KW",
    "ECDH-ES+A256KW",
];

/// 1 エントリを判定する。`Ok(None)` は読み飛ばし、`Err` は文書全体の拒否。
fn classify(raw: &RawJwk<'_>) -> Result<Option<Jwk>, JwksError> {
    let kty = raw.kty.as_deref().ok_or(JwksError::InvalidJson)?;
    let alg = raw.alg.as_deref();
    let crv = raw.crv.as_deref();

    if kty == "oct" {
        return Err(JwksError::SymmetricKey);
    }
    if let Some(a) = alg
        && (a.eq_ignore_ascii_case("none") || matches!(a, "HS256" | "HS384" | "HS512"))
    {
        return Err(JwksError::ForbiddenAlgorithm);
    }
    if raw.d || raw.p || raw.q || raw.dp || raw.dq || raw.qi || raw.oth || raw.k {
        return Err(JwksError::PrivateKeyMaterial);
    }
    // 未対応方式を含む既知の alg について、kty・crv との矛盾を読み飛ばし判定より前に
    // 検査する（矛盾する鍵が 1 つでもあれば文書全体を拒否する fail-closed）。
    if let Some(a) = alg {
        let required_kty = if a.starts_with("RS") || a.starts_with("PS") || a.starts_with("RSA") {
            Some(&["RSA"][..])
        } else if a.starts_with("ECDH-ES") {
            // RFC 8037 は ECDH-ES に OKP（X25519/X448）の鍵も許可する。
            Some(&["EC", "OKP"][..])
        } else if a.starts_with("ES") {
            Some(&["EC"][..])
        } else {
            None
        };
        // 未対応 kty（OKP 等）も比較対象にする（読み飛ばしで矛盾を見逃さない）。
        if let Some(req) = required_kty
            && !req.contains(&kty)
        {
            return Err(JwksError::KeyAlgorithmMismatch);
        }
        let required_crv = match a {
            "ES256" => Some("P-256"),
            "ES384" => Some("P-384"),
            "ES512" => Some("P-521"),
            _ => None,
        };
        if let Some(rc) = required_crv
            && kty == "EC"
            && crv != Some(rc)
        {
            // ES256 は従来どおり曲線不対応、未対応方式は alg との矛盾として扱う。
            return Err(if a == "ES256" {
                JwksError::UnsupportedCurve
            } else {
                JwksError::KeyAlgorithmMismatch
            });
        }
    }
    if raw.use_.as_deref().is_some_and(|u| u != "sig") {
        return Ok(None);
    }
    let is_rsa = kty == "RSA";
    let is_ec = kty == "EC";
    if is_ec && crv.is_none() {
        return Err(JwksError::UnsupportedCurve);
    }
    if !(is_rsa || (is_ec && crv == Some("P-256"))) {
        return Ok(None);
    }

    let (algorithm, skippable) = if is_rsa {
        (Algorithm::Rs256, SKIPPABLE_RSA_ALGS)
    } else {
        (Algorithm::Es256, SKIPPABLE_EC_ALGS)
    };
    // 読み飛ばし対象の alg（暗号化用途など）は kid 検査より前に判定し、
    // kid を持たない非署名用鍵で文書全体を拒否しない。
    if let Some(a) = alg
        && a != algorithm.as_jose_name()
        && skippable.contains(&a)
    {
        return Ok(None);
    }

    let kid = raw.kid.as_deref().filter(|k| !k.is_empty());
    let kid = kid.ok_or(JwksError::MissingKeyId)?;
    if kid.len() > MAX_KID_LEN {
        return Err(JwksError::KeyIdTooLong);
    }
    let alg = alg.ok_or(JwksError::MissingAlgorithm)?;
    if alg != algorithm.as_jose_name() {
        return Err(JwksError::KeyAlgorithmMismatch);
    }

    let material = if is_rsa {
        let n = decode_rsa(raw.n.as_deref(), MAX_RSA_N_LEN)?;
        let e = decode_rsa(raw.e.as_deref(), MAX_RSA_E_LEN)?;
        KeyMaterial::Rsa { n, e }
    } else {
        let x = decode_coord(raw.x.as_deref())?;
        let y = decode_coord(raw.y.as_deref())?;
        let mut point = [0u8; 65];
        point[0] = 0x04;
        point[1..33].copy_from_slice(&x);
        point[33..].copy_from_slice(&y);
        KeyMaterial::EcP256 { point }
    };
    Ok(Some(Jwk {
        kid: kid.into(),
        alg: algorithm,
        material,
    }))
}

/// base64url（パディングなし）の符号化後の最大長。デコード前に過大入力を弾く。
const fn encoded_cap(bytes: usize) -> usize {
    bytes * 4 / 3 + 4
}

/// RSA の整数成分をデコードし、先頭の 0 を除く。
fn decode_rsa(s: Option<&str>, max: usize) -> Result<Box<[u8]>, JwksError> {
    let s = s.ok_or(JwksError::MissingKeyComponent)?;
    // 先頭 0 を含む冗長な表現を許容するため、上限に余裕を持たせて事前判定する。
    if s.len() > encoded_cap(max) + 8 {
        return Err(JwksError::KeyTooLarge);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| JwksError::InvalidKeyEncoding)?;
    let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
    let trimmed = &bytes[start..];
    if trimmed.is_empty() {
        return Err(JwksError::InvalidKeyEncoding);
    }
    if trimmed.len() > max {
        return Err(JwksError::KeyTooLarge);
    }
    Ok(trimmed.into())
}

/// EC の座標をデコードする。RFC 7518 6.2.1.2 により 32 バイト固定で、0 除去はしない。
fn decode_coord(s: Option<&str>) -> Result<[u8; EC_P256_COORD_LEN], JwksError> {
    let s = s.ok_or(JwksError::MissingKeyComponent)?;
    if s.len() > encoded_cap(EC_P256_COORD_LEN) {
        return Err(JwksError::InvalidKeyLength);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| JwksError::InvalidKeyEncoding)?;
    bytes.try_into().map_err(|_| JwksError::InvalidKeyLength)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn rsa(kid: &str, n: &[u8]) -> String {
        format!(
            r#"{{"kty":"RSA","kid":"{kid}","alg":"RS256","n":"{}","e":"AQAB"}}"#,
            b64(n)
        )
    }

    #[test]
    fn rsa_leading_zeros_are_stripped() {
        let json = format!(r#"{{"keys":[{}]}}"#, rsa("a", &[0, 0, 7, 8]));
        let set = KeySet::from_json(&json).unwrap();
        match &set.keys[0].material {
            KeyMaterial::Rsa { n, e } => {
                assert_eq!(&**n, &[7, 8]);
                assert_eq!(&**e, &[1, 0, 1]);
            }
            KeyMaterial::EcP256 { .. } => panic!("RSA のはず"),
        }
    }

    #[test]
    fn ec_point_is_uncompressed_and_keeps_leading_zero() {
        let mut x = [1u8; 32];
        x[0] = 0;
        let y = [2u8; 32];
        let json = format!(
            r#"{{"keys":[{{"kty":"EC","crv":"P-256","kid":"e","alg":"ES256","x":"{}","y":"{}"}}]}}"#,
            b64(&x),
            b64(&y)
        );
        let set = KeySet::from_json(&json).unwrap();
        match &set.keys[0].material {
            KeyMaterial::EcP256 { point } => {
                assert_eq!(point[0], 4);
                assert_eq!(&point[1..33], &x);
                assert_eq!(&point[33..], &y);
            }
            KeyMaterial::Rsa { .. } => panic!("EC のはず"),
        }
    }

    #[test]
    fn encryption_key_without_kid_is_skipped() {
        let json = format!(
            r#"{{"keys":[{{"kty":"RSA","alg":"RSA-OAEP","n":"{}","e":"AQAB"}},{}]}}"#,
            b64(&[7, 8]),
            rsa("a", &[7, 8])
        );
        let set = KeySet::from_json(&json).unwrap();
        assert_eq!((set.len(), set.skipped()), (1, 1));
    }

    #[test]
    fn signing_key_without_kid_is_still_rejected() {
        let json = format!(
            r#"{{"keys":[{{"kty":"RSA","alg":"RS256","n":"{}","e":"AQAB"}}]}}"#,
            b64(&[7, 8])
        );
        assert!(KeySet::from_json(&json).is_err());
    }

    #[test]
    fn unsupported_alg_contradicting_kty_rejects_whole_document() {
        let good = rsa("a", &[7, 8]);
        let ec384 = r#"{"kty":"EC","crv":"P-384","alg":"RS384"}"#;
        let json = format!(r#"{{"keys":[{good},{ec384}]}}"#);
        assert_eq!(
            KeySet::from_json(&json).unwrap_err(),
            JwksError::KeyAlgorithmMismatch
        );
        let rsa_es = format!(
            r#"{{"kty":"RSA","kid":"b","alg":"ES384","use":"enc","n":"{}","e":"AQAB"}}"#,
            b64(&[7, 8])
        );
        let json = format!(r#"{{"keys":[{good},{rsa_es}]}}"#);
        assert_eq!(
            KeySet::from_json(&json).unwrap_err(),
            JwksError::KeyAlgorithmMismatch
        );
    }

    #[test]
    fn unsupported_kty_contradicting_alg_rejects_whole_document() {
        let good = rsa("a", &[7, 8]);
        for alg in ["RS256", "PS256", "ES256", "ES384"] {
            let okp = format!(r#"{{"kty":"OKP","crv":"Ed25519","alg":"{alg}"}}"#);
            let json = format!(r#"{{"keys":[{good},{okp}]}}"#);
            assert_eq!(
                KeySet::from_json(&json).unwrap_err(),
                JwksError::KeyAlgorithmMismatch,
                "{alg}"
            );
        }
        let okp = r#"{"kty":"OKP","crv":"Ed25519","alg":"EdDSA"}"#;
        let set = KeySet::from_json(&format!(r#"{{"keys":[{good},{okp}]}}"#)).unwrap();
        assert_eq!((set.len(), set.skipped()), (1, 1));
    }

    #[test]
    fn ecdh_es_allows_okp_but_rejects_rsa() {
        let good = rsa("a", &[7, 8]);
        let okp = r#"{"kty":"OKP","crv":"X25519","use":"enc","alg":"ECDH-ES"}"#;
        let set = KeySet::from_json(&format!(r#"{{"keys":[{good},{okp}]}}"#)).unwrap();
        assert_eq!((set.len(), set.skipped()), (1, 1));
        let bad = format!(
            r#"{{"kty":"RSA","kid":"b","alg":"ECDH-ES","use":"enc","n":"{}","e":"AQAB"}}"#,
            b64(&[7, 8])
        );
        assert_eq!(
            KeySet::from_json(&format!(r#"{{"keys":[{good},{bad}]}}"#)).unwrap_err(),
            JwksError::KeyAlgorithmMismatch
        );
    }
}
