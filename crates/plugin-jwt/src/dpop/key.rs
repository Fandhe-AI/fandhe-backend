//! DPoP proof のヘッダ `jwk` の解析・検査と、RFC 7638 のサムプリント計算（イシュー #901）。
//!
//! 役割: proof が自己申告する公開鍵を、署名検証に使える [`KeyMaterial`] へフェイルクローズで
//! 変換する。秘密鍵の要素・共通鍵・未対応の種別・`alg` との食い違いを拒否し、受け入れた鍵
//! だけから正規形のサムプリントを作る（`cnf.jkt` との結合は #904）。JWKS 経路
//! （`crate::jwks`）とは別の経路で、proof 用に「正規形でない整数表現も拒否する」より
//! 厳しい規則を使う（再符号化した値が入力と一致することを保証し、サムプリントの
//! 解釈差を排除する、設計 9.7 節）。

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest::{SHA256, digest};
use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::DpopError;
use crate::jwks::KeyMaterial;
use crate::policy::Algorithm;

const EC_COORD_LEN: usize = 32;
const RSA_N_MIN: usize = 256;
const RSA_N_MAX: usize = 1024;
const RSA_E_MAX: usize = 8;

/// JWK の RFC 7638 サムプリント（SHA-256 を base64url にした 43 文字）。
///
/// 秘密ではない（公開鍵のハッシュ）ため `Debug` にも出す。
///
/// ```
/// use fandhe_backend_plugin_jwt::dpop::jwk_thumbprint;
/// let t = jwk_thumbprint(r#"{"kty":"RSA","e":"AQAB","n":"0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw"}"#)?;
/// assert_eq!(t.as_str(), "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs");
/// # Ok::<(), fandhe_backend_plugin_jwt::dpop::DpopError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwkThumbprint(Box<str>);

impl JwkThumbprint {
    /// base64url（パディングなし）の文字列。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JwkThumbprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// キーが存在すれば値の型を問わず `true`（`null` でも存在扱い）。
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(d)?;
    Ok(true)
}

/// `jwk` の読み取り用構造。`kid`・`key_ops`・`x5c` などは読まない。重複キーは serde がエラーにする。
#[derive(Deserialize)]
struct JwkFields {
    #[serde(default)]
    kty: Option<String>,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default, rename = "use")]
    use_: Option<String>,
    #[serde(default)]
    crv: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
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

/// JSON オブジェクトだけを受ける `jwk`。配列からの復元（derive の既定動作）を許さない。
pub(crate) struct ProofJwk(JwkFields);

impl<'de> Deserialize<'de> for ProofJwk {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = ProofJwk;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JWK object")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<ProofJwk, A::Error> {
                JwkFields::deserialize(de::value::MapAccessDeserializer::new(map)).map(ProofJwk)
            }
        }
        d.deserialize_map(V)
    }
}

/// 検査済みの proof 鍵。署名検証用の鍵成分とサムプリントを持つ。
pub(crate) struct ProofKey {
    pub(crate) material: KeyMaterial,
    pub(crate) thumbprint: JwkThumbprint,
}

/// `jwk` を検査して鍵成分とサムプリントを作る。`header_alg` が `Some` のときは proof 検証で、
/// 鍵の種別と一致しなければ拒否する。`None`（サムプリント単体）のときは `jwk.alg` があれば
/// 種別と矛盾しないことだけを要求する。
pub(crate) fn build_proof_key(
    jwk: &ProofJwk,
    header_alg: Option<Algorithm>,
) -> Result<ProofKey, DpopError> {
    let j = &jwk.0;
    if j.d || j.p || j.q || j.dp || j.dq || j.qi || j.oth || j.k {
        return Err(DpopError::PrivateKeyMaterial);
    }
    let kty = j.kty.as_deref().ok_or(DpopError::UnsupportedKey)?;
    let key_alg = match kty {
        "RSA" => Algorithm::Rs256,
        "EC" => Algorithm::Es256,
        _ => return Err(DpopError::UnsupportedKey),
    };
    if header_alg.is_some_and(|a| a != key_alg) {
        return Err(DpopError::KeyAlgorithmMismatch);
    }
    if j.alg
        .as_deref()
        .is_some_and(|a| a != key_alg.as_jose_name())
    {
        return Err(DpopError::KeyAlgorithmMismatch);
    }
    if j.use_.as_deref().is_some_and(|u| u != "sig") {
        return Err(DpopError::UnsupportedKey);
    }
    match key_alg {
        Algorithm::Rs256 => {
            let n = canonical_int(j.n.as_deref(), RSA_N_MIN, RSA_N_MAX)?;
            let e = canonical_int(j.e.as_deref(), 1, RSA_E_MAX)?;
            let json = format!(
                r#"{{"e":"{}","kty":"RSA","n":"{}"}}"#,
                URL_SAFE_NO_PAD.encode(&e),
                URL_SAFE_NO_PAD.encode(&n)
            );
            Ok(ProofKey {
                thumbprint: thumbprint_of(&json),
                material: KeyMaterial::Rsa {
                    n: n.into_boxed_slice(),
                    e: e.into_boxed_slice(),
                },
            })
        }
        Algorithm::Es256 => {
            if j.crv.as_deref() != Some("P-256") {
                return Err(DpopError::UnsupportedKey);
            }
            let x = fixed_coord(j.x.as_deref())?;
            let y = fixed_coord(j.y.as_deref())?;
            let json = format!(
                r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#,
                URL_SAFE_NO_PAD.encode(x),
                URL_SAFE_NO_PAD.encode(y)
            );
            let mut point = [0u8; 65];
            point[0] = 0x04;
            point[1..33].copy_from_slice(&x);
            point[33..].copy_from_slice(&y);
            Ok(ProofKey {
                thumbprint: thumbprint_of(&json),
                material: KeyMaterial::EcP256 { point },
            })
        }
    }
}

fn thumbprint_of(canonical_json: &str) -> JwkThumbprint {
    let d = digest(&SHA256, canonical_json.as_bytes());
    JwkThumbprint(URL_SAFE_NO_PAD.encode(d.as_ref()).into_boxed_str())
}

/// base64url の符号化後の最大長。復号前に過大入力を弾く。
const fn encoded_cap(bytes: usize) -> usize {
    bytes * 4 / 3 + 4
}

/// 先頭 0 を持たない正規形の整数（big-endian）として復号する。
fn canonical_int(s: Option<&str>, min: usize, max: usize) -> Result<Vec<u8>, DpopError> {
    let s = s.ok_or(DpopError::InvalidKey)?;
    if s.len() > encoded_cap(max) {
        return Err(DpopError::InvalidKey);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| DpopError::InvalidKey)?;
    if bytes.len() < min || bytes.len() > max || bytes.first() == Some(&0) {
        return Err(DpopError::InvalidKey);
    }
    Ok(bytes)
}

/// EC 座標（32 バイト固定、RFC 7518 6.2.1.2）。
fn fixed_coord(s: Option<&str>) -> Result<[u8; EC_COORD_LEN], DpopError> {
    let s = s.ok_or(DpopError::InvalidKey)?;
    if s.len() > encoded_cap(EC_COORD_LEN) {
        return Err(DpopError::InvalidKey);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| DpopError::InvalidKey)?;
    bytes.try_into().map_err(|_| DpopError::InvalidKey)
}

/// 単独の JWK（JSON 文字列）から RFC 7638 のサムプリントを計算する。
///
/// proof のヘッダ `jwk` と同じ規則で鍵を検査する（RSA は 2048 bit 以上、EC は P-256 のみ、
/// 秘密鍵の要素・共通鍵・正規形でない整数は拒否）。`kid` などの余分なメンバは無視する。
/// 入力は [`MAX_PROOF_LEN`](super::MAX_PROOF_LEN) バイトまで。
///
/// ```
/// use fandhe_backend_plugin_jwt::dpop::{DpopError, jwk_thumbprint};
/// assert_eq!(jwk_thumbprint("[]").unwrap_err(), DpopError::Malformed);
/// assert_eq!(
///     jwk_thumbprint(r#"{"kty":"oct","k":"AA"}"#).unwrap_err(),
///     DpopError::PrivateKeyMaterial
/// );
/// ```
pub fn jwk_thumbprint(jwk_json: &str) -> Result<JwkThumbprint, DpopError> {
    if jwk_json.len() > super::MAX_PROOF_LEN {
        return Err(DpopError::TooLong);
    }
    let jwk: ProofJwk = serde_json::from_str(jwk_json).map_err(|_| DpopError::Malformed)?;
    build_proof_key(&jwk, None).map(|k| k.thumbprint)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC7638_N: &str = "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw";

    #[test]
    fn rfc7638_example_thumbprint() {
        let json = format!(
            r#"{{"kty":"RSA","kid":"2011-04-29","alg":"RS256","e":"AQAB","n":"{RFC7638_N}"}}"#
        );
        assert_eq!(
            jwk_thumbprint(&json).unwrap().as_str(),
            "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs"
        );
    }

    #[test]
    fn duplicate_member_and_non_object_are_rejected() {
        assert_eq!(
            jwk_thumbprint(r#"{"kty":"RSA","kty":"RSA"}"#).unwrap_err(),
            DpopError::Malformed
        );
        assert_eq!(jwk_thumbprint("\"x\"").unwrap_err(), DpopError::Malformed);
    }
}
