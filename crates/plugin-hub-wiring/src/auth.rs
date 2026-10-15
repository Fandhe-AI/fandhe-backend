//! hub 向け認証器 [`Authenticator`]（検証は `fandhe-backend-plugin-jwt` へ委譲、イシュー #820）。
//!
//! [`crate::gate::TenantGate`]（`RequestGate` 拡張点）は判定結果のみを `GateOutcome` として
//! コアへ返す契約のため、ゲート通過後にハンドラが `org_id` を必要とするときは本型を
//! 呼ぶ。署名・`iss`・`aud`・`typ`・`exp`・`nbf` の検証と、検証成功済みトークンの
//! キャッシュ（鍵ローテーションで自動無効化）は汎用 JWT 検証クレートの
//! [`JwtAuthenticator`] が担う。本モジュールが足すのは hub 固有の判定だけである。
//!
//! - 追加クレーム `org_id` の取り出し（[`HubClaims`]）
//! - `org_id` の欠落・空・空白のみを [`AuthError::MissingOrgId`]（ゲートでは 403）にする
//!
//! [`Authenticator`] を [`TenantGate`](crate::gate::TenantGate) とハンドラで共有すると、
//! ゲート通過時点でキャッシュが温まり、ハンドラ側の呼び出しは署名検証を再実行しない。
//!
//! # フェイルクローズ（.claude/rules/security.md A01）
//!
//! 検証の失敗と `org_id` 欠落はいずれも `Err` で、`Allow` に落ちる経路はない。
//! [`AuthError`] は文字列を持たず、トークン・`kid`・`org_id` を含まない。

use std::fmt;

use fandhe_backend_http::request::RequestHead;
use fandhe_backend_plugin_jwt::{Claims, JwtAuthenticator, JwtPolicy, SharedKeySet, VerifyError};
use serde::Deserialize;

pub use fandhe_backend_plugin_jwt::bearer_token;

/// hub が検証済みトークンの payload から読む追加クレーム。
///
/// `org_id` が文字列以外（数値など）の場合は検証側で
/// [`VerifyError::InvalidExtraClaims`] になる。欠落は `None`（[`AuthError::MissingOrgId`]）。
///
/// # Examples
///
/// ```
/// # use base64::Engine as _;
/// # use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
/// # use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// # use fandhe_backend_plugin_hub_wiring::auth::Authenticator;
/// # use fandhe_backend_plugin_hub_wiring::jwt::{
/// #     Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtPolicy, SharedKeySet, TokenType,
/// # };
/// # use ring::rand::SystemRandom;
/// # use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};
/// # let pair = RsaKeyPair::from_pkcs8(include_bytes!("../tests/fixtures/test-rsa-2048.pk8"))
/// #     .map_err(|_| "pkcs8")?;
/// # let c: RsaPublicKeyComponents<Vec<u8>> = RsaPublicKeyComponents::from(pair.public_key());
/// # let jwks = format!(
/// #     r#"{{"keys":[{{"kty":"RSA","kid":"k1","n":"{}","e":"{}","use":"sig","alg":"RS256"}}]}}"#,
/// #     B64.encode(&c.n), B64.encode(&c.e)
/// # );
/// # let h = B64.encode(r#"{"alg":"RS256","typ":"at+jwt","kid":"k1"}"#);
/// # let p = B64.encode(r#"{"iss":"https://issuer.example","aud":"hub-api","sub":"u1","org_id":" org-1 ","exp":9999999999}"#);
/// # let mut sig = vec![0u8; pair.public().modulus_len()];
/// # pair.sign(&signature::RSA_PKCS1_SHA256, &SystemRandom::new(), format!("{h}.{p}").as_bytes(), &mut sig)
/// #     .map_err(|_| "sign")?;
/// # let token = format!("{h}.{p}.{}", B64.encode(sig));
/// # let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n");
/// # let ParseOutcome::Complete { head, .. } = parse_request_head(raw.as_bytes())? else { unreachable!() };
/// # let policy = JwtPolicy::new(
/// #     Issuer::new("https://issuer.example")?,
/// #     Audiences::single("hub-api")?,
/// #     AllowedAlgorithms::only(Algorithm::Rs256),
/// #     TokenType::AccessToken,
/// # );
/// # let authenticator = Authenticator::new(SharedKeySet::from_json(&jwks)?, policy);
/// let tenant = authenticator.authenticate(&head)?;
/// // `HubClaims` は検証済みトークンの追加クレームで、`Claims<HubClaims>` の `extra()` に載る。
/// let extra = tenant.claims().extra();
/// // 値（org_id）は Debug に出ない。
/// assert_eq!(format!("{extra:?}"), "HubClaims { has_org_id: true }");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Deserialize)]
pub struct HubClaims {
    #[serde(default)]
    org_id: Option<String>,
}

/// `org_id` の値は出さない（ログ漏洩防止）。
impl fmt::Debug for HubClaims {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HubClaims")
            .field("has_org_id", &self.org_id.is_some())
            .finish()
    }
}

/// [`Authenticator::authenticate`] の失敗理由。文字列を持たない。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_hub_wiring::auth::AuthError;
///
/// assert_eq!(AuthError::MissingOrgId.to_string(), "org_id クレームがありません");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthError {
    /// JWT の検証に失敗した（ゲートでは 401）。
    Jwt(VerifyError),
    /// 署名は妥当だが `org_id` が欠落・空・空白のみ（ゲートでは 403）。
    MissingOrgId,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Jwt(e) => e.fmt(f),
            Self::MissingOrgId => f.write_str("org_id クレームがありません"),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<VerifyError> for AuthError {
    fn from(e: VerifyError) -> Self {
        Self::Jwt(e)
    }
}

/// 認証済みのテナント情報。`org_id` は trim 後に空でないことが保証される。
///
/// `sub` などを含む [`Claims`] を持つため、ログへ出さないこと（`Debug` は値を出さない）。
///
/// # Examples
///
/// ```
/// # use base64::Engine as _;
/// # use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
/// # use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// # use fandhe_backend_plugin_hub_wiring::auth::Authenticator;
/// # use fandhe_backend_plugin_hub_wiring::jwt::{
/// #     Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtPolicy, SharedKeySet, TokenType,
/// # };
/// # use ring::rand::SystemRandom;
/// # use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};
/// # let pair = RsaKeyPair::from_pkcs8(include_bytes!("../tests/fixtures/test-rsa-2048.pk8"))
/// #     .map_err(|_| "pkcs8")?;
/// # let c: RsaPublicKeyComponents<Vec<u8>> = RsaPublicKeyComponents::from(pair.public_key());
/// # let jwks = format!(
/// #     r#"{{"keys":[{{"kty":"RSA","kid":"k1","n":"{}","e":"{}","use":"sig","alg":"RS256"}}]}}"#,
/// #     B64.encode(&c.n), B64.encode(&c.e)
/// # );
/// # let h = B64.encode(r#"{"alg":"RS256","typ":"at+jwt","kid":"k1"}"#);
/// # let p = B64.encode(r#"{"iss":"https://issuer.example","aud":"hub-api","sub":"u1","org_id":" org-1 ","exp":9999999999}"#);
/// # let mut sig = vec![0u8; pair.public().modulus_len()];
/// # pair.sign(&signature::RSA_PKCS1_SHA256, &SystemRandom::new(), format!("{h}.{p}").as_bytes(), &mut sig)
/// #     .map_err(|_| "sign")?;
/// # let token = format!("{h}.{p}.{}", B64.encode(sig));
/// # let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n");
/// # let ParseOutcome::Complete { head, .. } = parse_request_head(raw.as_bytes())? else { unreachable!() };
/// # let policy = JwtPolicy::new(
/// #     Issuer::new("https://issuer.example")?,
/// #     Audiences::single("hub-api")?,
/// #     AllowedAlgorithms::only(Algorithm::Rs256),
/// #     TokenType::AccessToken,
/// # );
/// # let authenticator = Authenticator::new(SharedKeySet::from_json(&jwks)?, policy);
/// let tenant = authenticator.authenticate(&head)?;
/// assert_eq!(tenant.org_id(), "org-1");
/// assert_eq!(tenant.claims().sub(), Some("u1"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct TenantClaims {
    org_id: String,
    claims: Claims<HubClaims>,
}

impl TenantClaims {
    /// テナント ID（trim 済み・非空）。
    ///
    /// # Examples
    ///
    /// ```
    /// # use base64::Engine as _;
    /// # use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    /// # use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    /// # use fandhe_backend_plugin_hub_wiring::auth::Authenticator;
    /// # use fandhe_backend_plugin_hub_wiring::jwt::{
    /// #     Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtPolicy, SharedKeySet, TokenType,
    /// # };
    /// # use ring::rand::SystemRandom;
    /// # use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};
    /// # let pair = RsaKeyPair::from_pkcs8(include_bytes!("../tests/fixtures/test-rsa-2048.pk8"))
    /// #     .map_err(|_| "pkcs8")?;
    /// # let c: RsaPublicKeyComponents<Vec<u8>> = RsaPublicKeyComponents::from(pair.public_key());
    /// # let jwks = format!(
    /// #     r#"{{"keys":[{{"kty":"RSA","kid":"k1","n":"{}","e":"{}","use":"sig","alg":"RS256"}}]}}"#,
    /// #     B64.encode(&c.n), B64.encode(&c.e)
    /// # );
    /// # let h = B64.encode(r#"{"alg":"RS256","typ":"at+jwt","kid":"k1"}"#);
    /// # let p = B64.encode(r#"{"iss":"https://issuer.example","aud":"hub-api","sub":"u1","org_id":" org-1 ","exp":9999999999}"#);
    /// # let mut sig = vec![0u8; pair.public().modulus_len()];
    /// # pair.sign(&signature::RSA_PKCS1_SHA256, &SystemRandom::new(), format!("{h}.{p}").as_bytes(), &mut sig)
    /// #     .map_err(|_| "sign")?;
    /// # let token = format!("{h}.{p}.{}", B64.encode(sig));
    /// # let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n");
    /// # let ParseOutcome::Complete { head, .. } = parse_request_head(raw.as_bytes())? else { unreachable!() };
    /// # let policy = JwtPolicy::new(
    /// #     Issuer::new("https://issuer.example")?,
    /// #     Audiences::single("hub-api")?,
    /// #     AllowedAlgorithms::only(Algorithm::Rs256),
    /// #     TokenType::AccessToken,
    /// # );
    /// # let authenticator = Authenticator::new(SharedKeySet::from_json(&jwks)?, policy);
    /// let tenant = authenticator.authenticate(&head)?;
    /// // trim 済みの非空テナント ID。
    /// assert_eq!(tenant.org_id(), "org-1");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn org_id(&self) -> &str {
        &self.org_id
    }

    /// 検証済みの標準クレーム（`iss`・`sub`・`aud`・`exp` など）。
    ///
    /// # Examples
    ///
    /// ```
    /// # use base64::Engine as _;
    /// # use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    /// # use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    /// # use fandhe_backend_plugin_hub_wiring::auth::Authenticator;
    /// # use fandhe_backend_plugin_hub_wiring::jwt::{
    /// #     Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtPolicy, SharedKeySet, TokenType,
    /// # };
    /// # use ring::rand::SystemRandom;
    /// # use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};
    /// # let pair = RsaKeyPair::from_pkcs8(include_bytes!("../tests/fixtures/test-rsa-2048.pk8"))
    /// #     .map_err(|_| "pkcs8")?;
    /// # let c: RsaPublicKeyComponents<Vec<u8>> = RsaPublicKeyComponents::from(pair.public_key());
    /// # let jwks = format!(
    /// #     r#"{{"keys":[{{"kty":"RSA","kid":"k1","n":"{}","e":"{}","use":"sig","alg":"RS256"}}]}}"#,
    /// #     B64.encode(&c.n), B64.encode(&c.e)
    /// # );
    /// # let h = B64.encode(r#"{"alg":"RS256","typ":"at+jwt","kid":"k1"}"#);
    /// # let p = B64.encode(r#"{"iss":"https://issuer.example","aud":"hub-api","sub":"u1","org_id":" org-1 ","exp":9999999999}"#);
    /// # let mut sig = vec![0u8; pair.public().modulus_len()];
    /// # pair.sign(&signature::RSA_PKCS1_SHA256, &SystemRandom::new(), format!("{h}.{p}").as_bytes(), &mut sig)
    /// #     .map_err(|_| "sign")?;
    /// # let token = format!("{h}.{p}.{}", B64.encode(sig));
    /// # let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n");
    /// # let ParseOutcome::Complete { head, .. } = parse_request_head(raw.as_bytes())? else { unreachable!() };
    /// # let policy = JwtPolicy::new(
    /// #     Issuer::new("https://issuer.example")?,
    /// #     Audiences::single("hub-api")?,
    /// #     AllowedAlgorithms::only(Algorithm::Rs256),
    /// #     TokenType::AccessToken,
    /// # );
    /// # let authenticator = Authenticator::new(SharedKeySet::from_json(&jwks)?, policy);
    /// let tenant = authenticator.authenticate(&head)?;
    /// let claims = tenant.claims();
    /// assert_eq!(claims.iss(), "https://issuer.example");
    /// assert_eq!(claims.sub(), Some("u1"));
    /// assert_eq!(claims.exp(), 9_999_999_999);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn claims(&self) -> &Claims<HubClaims> {
        &self.claims
    }
}

impl fmt::Debug for TenantClaims {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TenantClaims").finish_non_exhaustive()
    }
}

/// `Authorization: Bearer` を検証し、テナント ID を取り出す認証器（`Clone`、`Arc` 共有）。
///
/// [`JwtAuthenticator<HubClaims>`] の薄いラッパー。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_hub_wiring::auth::Authenticator;
/// use fandhe_backend_plugin_hub_wiring::jwt::{
///     Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtPolicy, SharedKeySet, TokenType,
/// };
///
/// let policy = JwtPolicy::new(
///     Issuer::new("https://issuer.example")?,
///     Audiences::single("hub-api")?,
///     AllowedAlgorithms::only(Algorithm::Rs256),
///     TokenType::AccessToken,
/// );
/// let keys = SharedKeySet::from_json(r#"{"keys":[]}"#)?;
/// let authenticator = Authenticator::new(keys, policy);
/// assert_eq!(authenticator.cache_hits(), 0);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Authenticator {
    inner: JwtAuthenticator<HubClaims>,
}

impl Authenticator {
    /// 鍵集合と検証ポリシーから認証器を作る。ポリシーなしでは作れない。
    pub fn new(keys: SharedKeySet, policy: JwtPolicy) -> Self {
        Self {
            inner: JwtAuthenticator::<HubClaims>::builder(keys, policy).build(),
        }
    }

    /// 構築済みの [`JwtAuthenticator<HubClaims>`] から作る。未知 `kid` 通知
    /// （`UnknownKeyNotifier`）などを設定したいときに builder 経由で組み立てて渡す。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::auth::{Authenticator, HubClaims};
    /// use fandhe_backend_plugin_hub_wiring::jwt::{
    ///     Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtAuthenticator, JwtPolicy,
    ///     SharedKeySet, TokenType,
    /// };
    ///
    /// let policy = JwtPolicy::new(
    ///     Issuer::new("https://issuer.example")?,
    ///     Audiences::single("hub-api")?,
    ///     AllowedAlgorithms::only(Algorithm::Rs256),
    ///     TokenType::AccessToken,
    /// );
    /// let keys = SharedKeySet::from_json(r#"{"keys":[]}"#)?;
    /// let jwt = JwtAuthenticator::<HubClaims>::builder(keys, policy).build();
    /// let _authenticator = Authenticator::from_jwt(jwt);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_jwt(inner: JwtAuthenticator<HubClaims>) -> Self {
        Self { inner }
    }

    /// トークンを検証してテナント情報を返す。検証結果はキャッシュされる。
    ///
    /// # Errors
    ///
    /// 検証失敗は [`AuthError::Jwt`]、`org_id` の欠落・空・空白のみは
    /// [`AuthError::MissingOrgId`]。
    pub fn authenticate(&self, head: &RequestHead) -> Result<TenantClaims, AuthError> {
        let claims = self.inner.authenticate(head)?;
        // 署名検証後の値だけを信用する。空白のみは欠落と同じ扱いにする。
        let org_id = claims
            .extra()
            .org_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or(AuthError::MissingOrgId)?
            .to_owned();
        Ok(TenantClaims { org_id, claims })
    }

    /// キャッシュヒット件数（計測・テスト用）。
    pub fn cache_hits(&self) -> u64 {
        self.inner.cache_hits()
    }

    /// キャッシュミス件数（計測・テスト用）。
    pub fn cache_misses(&self) -> u64 {
        self.inner.cache_misses()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    use fandhe_backend_plugin_jwt::{
        Algorithm, AllowedAlgorithms, Audiences, Issuer, KeySet, TokenType,
    };
    use ring::rand::SystemRandom;
    use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};

    const TEST_KID: &str = "test-kid-1";
    const ISS: &str = "https://issuer.example";
    const AUD: &str = "hub-api";
    const FAR: u64 = 9_999_999_999;

    fn policy() -> JwtPolicy {
        JwtPolicy::new(
            Issuer::new(ISS).unwrap(),
            Audiences::single(AUD).unwrap(),
            AllowedAlgorithms::only(Algorithm::Rs256),
            TokenType::AccessToken,
        )
    }

    fn test_keypair() -> RsaKeyPair {
        RsaKeyPair::from_pkcs8(include_bytes!("../tests/fixtures/test-rsa-2048.pk8"))
            .expect("valid pkcs8 fixture")
    }

    fn rotated_keypair() -> RsaKeyPair {
        RsaKeyPair::from_pkcs8(include_bytes!(
            "../tests/fixtures/test-rsa-2048-rotated.pk8"
        ))
        .expect("valid pkcs8 fixture")
    }

    fn jwks_json_for(keypair: &RsaKeyPair, kid: &str) -> String {
        let components: RsaPublicKeyComponents<Vec<u8>> =
            RsaPublicKeyComponents::from(keypair.public_key());
        let n_b64 = URL_SAFE_NO_PAD.encode(&components.n);
        let e_b64 = URL_SAFE_NO_PAD.encode(&components.e);
        format!(
            r#"{{"keys":[{{"kty":"RSA","kid":"{kid}","n":"{n_b64}","e":"{e_b64}","use":"sig","alg":"RS256"}}]}}"#
        )
    }

    fn head_with_bearer(token: &str) -> RequestHead {
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        match parse_request_head(&raw).expect("valid head") {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => unreachable!("test fixtures are always complete"),
        }
    }

    /// `org_id_json` は JSON 値そのもの（`"org"`・`123`）。`None` は欠落。
    fn make_token(keypair: &RsaKeyPair, org_id_json: Option<&str>, exp: u64) -> String {
        let header = format!(r#"{{"alg":"RS256","typ":"at+jwt","kid":"{TEST_KID}"}}"#);
        let org = org_id_json
            .map(|v| format!(r#","org_id":{v}"#))
            .unwrap_or_default();
        let payload = format!(r#"{{"iss":"{ISS}","aud":"{AUD}","exp":{exp}{org}}}"#);
        let header_b64 = URL_SAFE_NO_PAD.encode(header);
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{header_b64}.{payload_b64}");
        let rng = SystemRandom::new();
        let mut sig = vec![0u8; keypair.public().modulus_len()];
        keypair
            .sign(
                &signature::RSA_PKCS1_SHA256,
                &rng,
                signing_input.as_bytes(),
                &mut sig,
            )
            .expect("signing succeeds");
        format!("{header_b64}.{payload_b64}.{}", URL_SAFE_NO_PAD.encode(sig))
    }

    fn authenticator_for(keypair: &RsaKeyPair) -> (Authenticator, SharedKeySet) {
        let shared = SharedKeySet::from_json(&jwks_json_for(keypair, TEST_KID)).unwrap();
        (Authenticator::new(shared.clone(), policy()), shared)
    }

    #[test]
    fn valid_token_returns_org_id_and_standard_claims() {
        let kp = test_keypair();
        let (a, _) = authenticator_for(&kp);
        let t = a
            .authenticate(&head_with_bearer(&make_token(&kp, Some(r#""org-1""#), FAR)))
            .unwrap();
        assert_eq!(t.org_id(), "org-1");
        assert_eq!(t.claims().iss(), ISS);
    }

    #[test]
    fn org_id_is_trimmed() {
        let kp = test_keypair();
        let (a, _) = authenticator_for(&kp);
        let t = a
            .authenticate(&head_with_bearer(&make_token(
                &kp,
                Some(r#""  org-1 ""#),
                FAR,
            )))
            .unwrap();
        assert_eq!(t.org_id(), "org-1");
    }

    #[test]
    fn missing_empty_or_blank_org_id_is_missing_org_id() {
        let kp = test_keypair();
        let (a, _) = authenticator_for(&kp);
        for org in [None, Some(r#""""#), Some(r#""   ""#), Some("null")] {
            let r = a.authenticate(&head_with_bearer(&make_token(&kp, org, FAR)));
            assert_eq!(r.unwrap_err(), AuthError::MissingOrgId, "org={org:?}");
        }
    }

    #[test]
    fn non_string_org_id_is_invalid_extra_claims() {
        let kp = test_keypair();
        let (a, _) = authenticator_for(&kp);
        let r = a.authenticate(&head_with_bearer(&make_token(&kp, Some("123"), FAR)));
        assert_eq!(
            r.unwrap_err(),
            AuthError::Jwt(VerifyError::InvalidExtraClaims)
        );
    }

    #[test]
    fn missing_authorization_header_is_missing_token() {
        let kp = test_keypair();
        let (a, _) = authenticator_for(&kp);
        let head = match parse_request_head(b"GET / HTTP/1.1\r\n\r\n").unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => unreachable!(),
        };
        assert_eq!(
            a.authenticate(&head).unwrap_err(),
            AuthError::Jwt(VerifyError::MissingToken)
        );
    }

    #[test]
    fn expired_token_is_rejected() {
        let kp = test_keypair();
        let (a, _) = authenticator_for(&kp);
        let r = a.authenticate(&head_with_bearer(&make_token(&kp, Some(r#""org-1""#), 1)));
        assert_eq!(r.unwrap_err(), AuthError::Jwt(VerifyError::Expired));
    }

    #[test]
    fn second_call_with_same_token_is_a_cache_hit() {
        let kp = test_keypair();
        let (a, _) = authenticator_for(&kp);
        let head = head_with_bearer(&make_token(&kp, Some(r#""org-1""#), FAR));
        a.authenticate(&head).unwrap();
        a.authenticate(&head).unwrap();
        assert_eq!(a.cache_misses(), 1);
        assert_eq!(a.cache_hits(), 1);
    }

    #[test]
    fn clone_shares_cache_between_gate_and_handler() {
        let kp = test_keypair();
        let (a, _) = authenticator_for(&kp);
        let handler_side = a.clone();
        let head = head_with_bearer(&make_token(&kp, Some(r#""org-1""#), FAR));
        a.authenticate(&head).unwrap();
        handler_side.authenticate(&head).unwrap();
        assert_eq!(handler_side.cache_hits(), 1);
    }

    #[test]
    fn rotation_to_different_key_rejects_old_token_without_stale_hit() {
        let kp = test_keypair();
        let rotated = rotated_keypair();
        let (a, shared) = authenticator_for(&kp);
        let head = head_with_bearer(&make_token(&kp, Some(r#""org-1""#), FAR));
        a.authenticate(&head).unwrap();

        shared.set(KeySet::from_json(&jwks_json_for(&rotated, TEST_KID)).unwrap());

        assert_eq!(
            a.authenticate(&head).unwrap_err(),
            AuthError::Jwt(VerifyError::InvalidSignature)
        );
    }

    #[test]
    fn auth_error_display_is_fixed_text() {
        assert_eq!(
            AuthError::Jwt(VerifyError::Expired).to_string(),
            VerifyError::Expired.to_string()
        );
    }
}
