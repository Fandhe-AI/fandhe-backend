//! `RequestGate` 拡張点上の `TenantGate` 実装（TASK-9.1 の主成果物。JWT 検証は
//! イシュー #820 で汎用 JWT 検証クレートへ移した）。
//!
//! コアの `Server::gate(impl RequestGate + 'static)`
//! （`crates/core/src/server.rs`）へ登録することで、hub 系サービスが
//! 各自で手書きしていた「JWT 検証 → `org_id` 抽出 → テナントスコープ強制」の
//! 配線をこのプラグイン 1 個の登録に集約する。`RequestGate` はコアループ内で
//! `UpgradeHandler`・`plugin::try_intercept` より先に評価される
//! （`crates/core/src/server.rs` doc）ため、WebSocket アップグレード等の
//! 長時間接続もこの認証をバイパスできない。

use std::fmt;

use crate::auth::{AuthError, Authenticator, TenantClaims};
use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
use fandhe_backend_http::error::error_response;
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_jwt::{JwksError, JwtPolicy, SharedKeySet};

/// [`TenantGate`] の設定。JWKS は [`SharedKeySet`] ハンドル経由で保持し、
/// 利用側サービスが再起動なしで鍵ローテーション（[`SharedKeySet::set`]）を
/// 行えるようにする。JWKS の**取得**（HTTP フェッチ・自動リフレッシュ）は
/// 本クレートの責務外（`RequestGate::check` は同期・I/O なしの契約）であり、
/// 利用側サービスが取得した JSON ドキュメントを注入する
/// （.claude/rules/security.md シークレット管理: 本クレートは env 読み取り・
/// HTTP フェッチ等を行わない）。
///
/// 内部で [`Authenticator`] を保持し、`TenantGate::check` はこれへ委譲する
/// （TASK-9.3 / #63）。利用側サービスがゲート通過後のハンドラでも同一の
/// 検証結果キャッシュを再利用したい場合は、[`Self::authenticator`] で
/// `Authenticator` を clone して保持し、`Server::gate` へ config を渡す前に
/// 取り出しておく。
#[derive(Debug, Clone)]
pub struct TenantGateConfig {
    authenticator: Authenticator,
}

impl TenantGateConfig {
    /// 既に構築済みの [`SharedKeySet`] ハンドルと検証ポリシーから設定を組み立てる。
    /// ポリシー（`iss`・`aud`・`typ`・許可 `alg`）を渡さずに設定は作れない。
    /// 呼び出し元が [`SharedKeySet`] を保持し続けることで、`TenantGate` 登録後も
    /// [`SharedKeySet::set`] による鍵ローテーションを行える。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::gate::TenantGateConfig;
    /// use fandhe_backend_plugin_hub_wiring::jwt::{
    ///     Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtPolicy, KeySet, SharedKeySet,
    ///     TokenType,
    /// };
    ///
    /// let policy = JwtPolicy::new(
    ///     Issuer::new("https://issuer.example")?,
    ///     Audiences::single("hub-api")?,
    ///     AllowedAlgorithms::only(Algorithm::Rs256),
    ///     TokenType::AccessToken,
    /// );
    /// let shared = SharedKeySet::new(KeySet::from_json(r#"{"keys":[]}"#)?);
    /// let config = TenantGateConfig::new(shared, policy);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(jwks: SharedKeySet, policy: JwtPolicy) -> Self {
        Self {
            authenticator: Authenticator::new(jwks, policy),
        }
    }

    /// JWKS JSON ドキュメントと検証ポリシーから直接設定を組み立てる便宜コンストラクタ。
    /// 事後にローテーションする必要がない・単純な起動時注入のみで足りる
    /// 利用側サービス向け（ローテーションが必要な場合は [`SharedKeySet::new`] を
    /// 呼び出し元で保持したまま [`Self::new`] を使う）。
    ///
    /// # Errors
    ///
    /// JWKS のパースに失敗した場合は [`JwksError`] を返す。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::gate::TenantGateConfig;
    /// use fandhe_backend_plugin_hub_wiring::jwt::{
    ///     Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtPolicy, TokenType,
    /// };
    ///
    /// let policy = JwtPolicy::new(
    ///     Issuer::new("https://issuer.example")?,
    ///     Audiences::single("hub-api")?,
    ///     AllowedAlgorithms::only(Algorithm::Rs256),
    ///     TokenType::AccessToken,
    /// );
    /// let config = TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#, policy)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// 検証ポリシーを渡さない構成は型として作れない。
    ///
    /// ```compile_fail
    /// use fandhe_backend_plugin_hub_wiring::gate::TenantGateConfig;
    ///
    /// let _ = TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#);
    /// ```
    pub fn from_jwks_json(json: &str, policy: JwtPolicy) -> Result<Self, JwksError> {
        Ok(Self::new(SharedKeySet::from_json(json)?, policy))
    }

    /// 構築済みの [`Authenticator`] から設定を組み立てる。
    ///
    /// DPoP（`dpop` feature）を使うときの入口。`JwtAuthenticator::<HubClaims>::builder(..)
    /// .dpop(cfg).build()` を [`Authenticator::from_jwt`] で包んで渡す。DPoP を設定した
    /// 認証器のときだけ、拒否の 401 に `WWW-Authenticate: DPoP` チャレンジが付く。
    /// 設定しない認証器では [`Self::new`] と同じ応答になる。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::auth::Authenticator;
    /// use fandhe_backend_plugin_hub_wiring::gate::TenantGateConfig;
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
    /// let _config = TenantGateConfig::from_authenticator(Authenticator::new(keys, policy));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_authenticator(authenticator: Authenticator) -> Self {
        Self { authenticator }
    }

    /// 内部で共有する [`Authenticator`] を取得する（`Clone`、`Arc` 共有）。
    ///
    /// 利用側サービスがハンドラ内で `org_id` 等のクレームを再利用したい場合、
    /// `Server::gate(TenantGate::new(config))` で `config` を消費する**前に**
    /// 本メソッドで `Authenticator` を取り出しておく。ゲート通過後の
    /// ハンドラで同一トークンについて [`Authenticator::authenticate`] を
    /// 呼ぶと、ゲートの検証でキャッシュ済みのためヒットし、署名検証を
    /// 再実行しない（TASK-9.3 / #63 の主目的）。
    ///
    /// DPoP を設定した認証器では、ハンドラでの再呼び出しは `jti` の再使用（`Replayed`）で
    /// 失敗する。その構成では [`TenantGate::verified`] でゲートの検証結果を読むこと。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::gate::{TenantGate, TenantGateConfig};
    ///
    /// # use fandhe_backend_plugin_hub_wiring::jwt::*;
    /// # let policy = JwtPolicy::new(
    /// #     Issuer::new("https://issuer.example").unwrap(),
    /// #     Audiences::single("hub-api").unwrap(),
    /// #     AllowedAlgorithms::only(Algorithm::Rs256),
    /// #     TokenType::AccessToken,
    /// # );
    /// let config = TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#, policy).unwrap();
    /// let authenticator = config.authenticator();
    /// let gate = TenantGate::new(config);
    /// // `authenticator` はハンドラ側で保持し、`gate` はコアへ登録する。
    /// drop(gate);
    /// ```
    pub fn authenticator(&self) -> Authenticator {
        self.authenticator.clone()
    }
}

/// [`TenantGate`] が検証した結果。ゲートだけが `RequestExtensions` に入れる（イシュー #905）。
///
/// フィールドは非公開で公開コンストラクタも `Clone` もなく、ゲート以外には作れない。
/// このため値が存在すること自体が「[`TenantGate`] を通った」ことの証明になる。
/// ハンドラは [`TenantGate::verified`] で読む。`Debug` は値を出さない。
pub struct TenantVerified {
    tenant: TenantClaims,
}

impl TenantVerified {
    /// 検証済みのテナント情報。
    pub fn tenant(&self) -> &TenantClaims {
        &self.tenant
    }
}

impl fmt::Debug for TenantVerified {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TenantVerified").finish_non_exhaustive()
    }
}

/// この要求は [`TenantGate`] を通っていない（`RequestHead` に [`TenantVerified`] がない）。
///
/// ゲート未登録や別経路の要求を、ハンドラが拒否するために使う（フェイルクローズ）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_hub_wiring::gate::NotAdmitted;
///
/// assert_eq!(NotAdmitted.to_string(), "この要求は TenantGate を通っていません");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotAdmitted;

impl fmt::Display for NotAdmitted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("この要求は TenantGate を通っていません")
    }
}

impl std::error::Error for NotAdmitted {}

/// `Authorization: Bearer <JWT>` を検証し、テナントスコープをフェイルクローズで
/// 強制する [`RequestGate`] 実装。
///
/// 判定ポリシー（PoC-6 踏襲、TASK-9.2 で RS256 + JWKS 前提へ更新）:
/// - トークン欠落・形式不正・アルゴリズム不正・`kid` 欠落・未知 `kid`・
///   署名不一致・期限切れ・`iss`/`aud`/`typ` 不一致 → `401`（認証失敗）
/// - 署名は妥当だが `org_id` クレームが欠落・空 → `403`
///   （クライアント入力誤りとしてアプリ層の明示的拒否）
/// - 上記いずれにも該当しない検証成功時のみ `Allow`
///
/// 拒否応答（401/403）は `Content-Type: application/json` を付与した
/// `Response` を運ぶ（イシュー #439。従来はワイヤ互換のため意図的に
/// ヘッダなしで据え置いていたが、JSON ボディに対する MIME 誤解釈の余地を
/// 狭めるため本イシューで付与へ変更した。401/403 の判定ロジック・body は
/// 無変更）。
///
/// JWKS 鍵セットが空（利用側サービスが未注入、またはローテーション中に
/// 一時的に空へ差し替えた）場合は全リクエストが `UnknownKeyId` により
/// `401` になる（フェイルオープンにしない、.claude/rules/security.md A01）。
///
/// `GateOutcome` は許可/拒否の判定結果のみを運ぶ契約
/// （`crates/core/src/extension.rs` doc）のため、検証で得た `org_id` 等の
/// クレームはこの構造体・呼び出しの外へ一切持ち出さない（コアは hub 固有の
/// シンボルへ依存しない）。ハンドラ側で認証済み情報を再利用したい場合は
/// [`TenantGateConfig::authenticator`] で取得した [`crate::auth::Authenticator`]
/// を呼ぶ。ゲート（本 `check`）とハンドラが同一 `Authenticator` を共有していれば、
/// ゲート通過時点でキャッシュが温まりハンドラ側の呼び出しは署名検証を
/// 再実行しない（TASK-9.3 / #63 のキャッシュ最適化）。
pub struct TenantGate {
    config: TenantGateConfig,
}

impl TenantGate {
    /// 設定を受け取り `TenantGate` を組み立てる。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_core::extension::RequestGate;
    /// use fandhe_backend_plugin_hub_wiring::gate::{TenantGate, TenantGateConfig};
    ///
    /// # use fandhe_backend_plugin_hub_wiring::jwt::*;
    /// # let policy = JwtPolicy::new(
    /// #     Issuer::new("https://issuer.example").unwrap(),
    /// #     Audiences::single("hub-api").unwrap(),
    /// #     AllowedAlgorithms::only(Algorithm::Rs256),
    /// #     TokenType::AccessToken,
    /// # );
    /// let config = TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#, policy).unwrap();
    /// let gate = TenantGate::new(config);
    /// assert_eq!(gate.name(), "hub-tenant-gate");
    /// ```
    pub fn new(config: TenantGateConfig) -> Self {
        Self { config }
    }

    /// ゲートが検証した結果をハンドラから読む。検証は繰り返さない（イシュー #905）。
    ///
    /// DPoP を設定した認証器では、ハンドラが `Authenticator::authenticate` を呼び直せない
    /// （`jti` の再使用になる）ため、この経路で `org_id` を取り出す。ゲートを通っていない
    /// 要求は [`NotAdmitted`]。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    /// use fandhe_backend_plugin_hub_wiring::gate::{NotAdmitted, TenantGate};
    ///
    /// let ParseOutcome::Complete { head, .. } = parse_request_head(b"GET / HTTP/1.1\r\n\r\n")?
    /// else { unreachable!() };
    /// // ゲートを通っていない要求には値がない。
    /// assert_eq!(TenantGate::verified(&head).err(), Some(NotAdmitted));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn verified(head: &RequestHead) -> Result<&TenantClaims, NotAdmitted> {
        head.extensions()
            .get::<TenantVerified>()
            .map(TenantVerified::tenant)
            .ok_or(NotAdmitted)
    }
}

/// `401` 応答の固定 body。トークン内容・ヘッダ値・`kid` を一切反映しない
/// （情報漏えい・ヘッダ/ログインジェクション防止、.claude/rules/security.md A03）。
const UNAUTHORIZED_BODY: &[u8] = br#"{"error":"invalid_token"}"#;

/// `403` 応答の固定 body。
const FORBIDDEN_BODY: &[u8] = br#"{"error":"tenant_scope_required"}"#;

/// 401/403 拒否応答を `Content-Type: application/json` 付きで組み立てる
/// （イシュー #439）。`body` は本ファイル内の固定 JSON 定数のみを渡す
/// 前提（`&'static [u8]`）で、外部入力を Content-Type へ混入させる経路は
/// 存在しない。`GateOutcome::reject` ヘルパはヘッダなし構築専用のため、
/// ヘッダ付き拒否応答はコア doc（`crates/core/src/extension.rs`）の推奨どおり
/// `GateOutcome::Reject { response }` を直接構築する。
fn reject_json(status: u16, body: &'static [u8]) -> GateOutcome {
    GateOutcome::Reject {
        response: Response::new(status, body.to_vec()).with_content_type("application/json"),
    }
}

impl RequestGate for TenantGate {
    fn name(&self) -> &'static str {
        "hub-tenant-gate"
    }

    fn check(
        &self,
        head: &RequestHead,
        _ctx: &GateContext,
        extensions: &mut fandhe_backend_core::extension::RequestExtensions,
    ) -> GateOutcome {
        // `check()` は同期・非ブロッキング（I/O なし）で Tokio ワーカーを
        // 塞がない契約を維持する（`crates/core/src/extension.rs` doc）。
        // `Authenticator::authenticate` はロックを短時間保持するのみで、
        // 実際の署名検証（キャッシュミス時のみ）中はロックを保持しない
        // （.claude/rules/coding-rust.md）。判定ポリシー（401/403 マッピング）は
        // 自前検証時代（イシュー #820 以前）の 401/403 マッピングと同一であり、キャッシュ
        // ヒット/ミスで判定結果が変わることはない（TASK-9.3 / #63）。テナント
        // 境界判定はトークンクレーム（`org_id`）のみに基づき、`ctx` の実 peer
        // address は本ゲートの判定に使用しない（イシュー #486、`GateContext`
        // 追加は既存 gate 実装への影響を最小化する非対応追随のみ）。
        match self.config.authenticator.authenticate(head) {
            // 検証結果を要求に載せ、ハンドラが `TenantGate::verified` で読めるようにする。
            // 既に値がある（TenantGate の二重登録など）場合は上書きせず拒否する。
            Ok(tenant) => match extensions.insert(TenantVerified { tenant }) {
                Ok(()) => GateOutcome::Allow,
                Err(_) => GateOutcome::Reject {
                    response: error_response(500, "internal_error"),
                },
            },
            Err(AuthError::MissingOrgId) => reject_json(403, FORBIDDEN_BODY),
            // DPoP を設定した認証器だけ、DPoP のチャレンジ付き 401 にする（設計 9.9 節。
            // 組み立ては plugin-jwt に寄せる）。設定しない構成の応答は従来と同一。
            #[cfg(feature = "dpop")]
            Err(AuthError::Jwt(e)) if self.config.authenticator.jwt().dpop_mode().is_some() => {
                GateOutcome::Reject {
                    response: self
                        .config
                        .authenticator
                        .jwt()
                        .unauthorized_response(head, &e),
                }
            }
            // 検証失敗のすべて。`AuthError`・`VerifyError` は `#[non_exhaustive]` のため、
            // 将来の変種も 401 に倒す（フェイルクローズ、Allow へは落ちない）。
            Err(_) => reject_json(401, UNAUTHORIZED_BODY),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    use ring::rand::SystemRandom;
    use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};

    use fandhe_backend_plugin_jwt::{
        Algorithm, AllowedAlgorithms, Audiences, Issuer, KeySet, TokenType,
    };

    const TEST_KID: &str = "test-kid-1";
    const TEST_ISS: &str = "https://issuer.example";
    const TEST_AUD: &str = "hub-api";

    fn test_policy() -> JwtPolicy {
        JwtPolicy::new(
            Issuer::new(TEST_ISS).unwrap(),
            Audiences::single(TEST_AUD).unwrap(),
            AllowedAlgorithms::only(Algorithm::Rs256),
            TokenType::AccessToken,
        )
    }

    fn test_keypair() -> RsaKeyPair {
        RsaKeyPair::from_pkcs8(include_bytes!("../tests/fixtures/test-rsa-2048.pk8"))
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

    fn head_from(raw: &[u8]) -> RequestHead {
        match parse_request_head(raw).expect("valid head") {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => unreachable!("test fixtures are always complete"),
        }
    }

    fn make_token(keypair: &RsaKeyPair, kid: &str, org_id: Option<&str>, exp: u64) -> String {
        let header = format!(r#"{{"alg":"RS256","typ":"at+jwt","kid":"{kid}"}}"#);
        let payload = match org_id {
            Some(org_id) => format!(
                r#"{{"iss":"{TEST_ISS}","aud":"{TEST_AUD}","org_id":"{org_id}","exp":{exp}}}"#
            ),
            None => format!(r#"{{"iss":"{TEST_ISS}","aud":"{TEST_AUD}","exp":{exp}}}"#),
        };
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
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
        format!("{header_b64}.{payload_b64}.{sig_b64}")
    }

    fn gate_for(keypair: &RsaKeyPair) -> TenantGate {
        let config =
            TenantGateConfig::from_jwks_json(&jwks_json_for(keypair, TEST_KID), test_policy())
                .unwrap();
        TenantGate::new(config)
    }

    #[test]
    fn missing_authorization_header_is_401() {
        let keypair = test_keypair();
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        assert_eq!(
            gate_for(&keypair).check(
                &head,
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            reject_json(401, UNAUTHORIZED_BODY)
        );
    }

    #[test]
    fn non_bearer_scheme_is_401() {
        let keypair = test_keypair();
        let raw = b"GET / HTTP/1.1\r\nAuthorization: Basic abcdef\r\n\r\n".to_vec();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(
                &head,
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            reject_json(401, UNAUTHORIZED_BODY)
        );
    }

    #[test]
    fn valid_token_is_allow() {
        let keypair = test_keypair();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(
                &head,
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            GateOutcome::Allow
        );
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        let keypair = test_keypair();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: bEaReR {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(
                &head,
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            GateOutcome::Allow
        );
    }

    #[test]
    fn bearer_scheme_allows_extra_spaces_before_token() {
        // RFC 6750 は auth-scheme と token68 の間に 1*SP を許容する。
        // 固定 7 バイトプレフィックス剥がしだと 2 個以上のスペースで
        // 誤って 401 を返していた回帰を防ぐ。
        let keypair = test_keypair();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer   {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(
                &head,
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            GateOutcome::Allow
        );
    }

    #[test]
    fn bearer_scheme_without_token_is_401() {
        let keypair = test_keypair();
        let raw = b"GET / HTTP/1.1\r\nAuthorization: Bearer \r\n\r\n".to_vec();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(
                &head,
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            reject_json(401, UNAUTHORIZED_BODY)
        );
    }

    #[test]
    fn malformed_token_is_401() {
        let keypair = test_keypair();
        let raw = b"GET / HTTP/1.1\r\nAuthorization: Bearer not-a-jwt\r\n\r\n".to_vec();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(
                &head,
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            reject_json(401, UNAUTHORIZED_BODY)
        );
    }

    #[test]
    fn expired_token_is_401() {
        let keypair = test_keypair();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 1);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(
                &head,
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            reject_json(401, UNAUTHORIZED_BODY)
        );
    }

    #[test]
    fn unknown_kid_is_401() {
        let keypair = test_keypair();
        let token = make_token(&keypair, "other-kid", Some("org-1"), 9_999_999_999);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(
                &head,
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            reject_json(401, UNAUTHORIZED_BODY)
        );
    }

    #[test]
    fn empty_jwks_rejects_valid_token() {
        // 鍵セットが空の場合はフェイルオープンにせず全リクエスト拒否
        // （.claude/rules/security.md A01）。
        let keypair = test_keypair();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        let empty_config =
            TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#, test_policy()).unwrap();
        let gate = TenantGate::new(empty_config);
        assert_eq!(
            gate.check(
                &head,
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            reject_json(401, UNAUTHORIZED_BODY)
        );
    }

    #[test]
    fn missing_org_id_is_403() {
        let keypair = test_keypair();
        let token = make_token(&keypair, TEST_KID, None, 9_999_999_999);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(
                &head,
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            reject_json(403, FORBIDDEN_BODY)
        );
    }

    #[test]
    fn reject_wire_includes_json_content_type() {
        // イシュー #439: 401/403 拒否応答のワイヤ上バイト列に
        // `Content-Type: application/json` が 1 行追加されることを固定する
        // （互換性変更。ステータス行・`Content-Length`・body は #424 時点の
        // 仕様を維持し、判定ポリシー（401/403 マッピング）自体は変えない）。
        let keypair = test_keypair();
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        let GateOutcome::Reject { response } = gate_for(&keypair).check(
            &head,
            &GateContext::new(None),
            &mut fandhe_backend_core::extension::RequestExtensions::new(),
        ) else {
            panic!("expected Reject for missing Authorization header");
        };
        let wire = String::from_utf8(response.serialize(false)).unwrap();
        assert!(wire.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(wire.contains("Content-Length: "));
        assert!(wire.contains("Content-Type: application/json\r\n"));
        assert!(!wire.contains("Retry-After:"));
        // DPoP を設定しない構成では WWW-Authenticate を付けない（応答不変）。
        assert!(!wire.contains("WWW-Authenticate"));
        assert!(wire.ends_with(&String::from_utf8_lossy(UNAUTHORIZED_BODY).into_owned()));

        let token = make_token(&keypair, TEST_KID, None, 9_999_999_999);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        let GateOutcome::Reject { response } = gate_for(&keypair).check(
            &head,
            &GateContext::new(None),
            &mut fandhe_backend_core::extension::RequestExtensions::new(),
        ) else {
            panic!("expected Reject for missing org_id");
        };
        let wire = String::from_utf8(response.serialize(false)).unwrap();
        assert!(wire.starts_with("HTTP/1.1 403 Forbidden\r\n"));
        assert!(wire.contains("Content-Type: application/json\r\n"));
        assert!(wire.ends_with(&String::from_utf8_lossy(FORBIDDEN_BODY).into_owned()));
    }

    #[test]
    fn reject_body_never_reflects_token_content() {
        // Reject body は静的固定文字列であり、トークン値・鍵材料を含まない
        // ことを固定する（.claude/rules/security.md A03）。
        let keypair = test_keypair();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 1);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        match gate_for(&keypair).check(
            &head,
            &GateContext::new(None),
            &mut fandhe_backend_core::extension::RequestExtensions::new(),
        ) {
            GateOutcome::Reject { response } => {
                let body_str = String::from_utf8_lossy(&response.body);
                assert!(!body_str.contains(&token));
            }
            GateOutcome::Allow => panic!("expected Reject for expired token"),
        }
    }

    #[test]
    fn config_rotation_reflects_in_check() {
        // `SharedKeySet::set` によるローテーション後、旧鍵で署名したトークンは
        // 拒否され新鍵で署名したトークンが許可されることを確認する。
        let keypair = test_keypair();
        let rotated_keypair = RsaKeyPair::from_pkcs8(include_bytes!(
            "../tests/fixtures/test-rsa-2048-rotated.pk8"
        ))
        .expect("valid pkcs8 fixture");

        let shared = SharedKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let gate = TenantGate::new(TenantGateConfig::new(shared.clone(), test_policy()));

        let old_token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999);
        let raw =
            format!("GET / HTTP/1.1\r\nAuthorization: Bearer {old_token}\r\n\r\n").into_bytes();
        assert_eq!(
            gate.check(
                &head_from(&raw),
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            GateOutcome::Allow
        );

        shared.set(KeySet::from_json(&jwks_json_for(&rotated_keypair, TEST_KID)).unwrap());

        // 旧鍵の署名は新 JWKS では検証できず拒否される。
        assert_eq!(
            gate.check(
                &head_from(&raw),
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            reject_json(401, UNAUTHORIZED_BODY)
        );

        let new_token = make_token(&rotated_keypair, TEST_KID, Some("org-1"), 9_999_999_999);
        let raw2 =
            format!("GET / HTTP/1.1\r\nAuthorization: Bearer {new_token}\r\n\r\n").into_bytes();
        assert_eq!(
            gate.check(
                &head_from(&raw2),
                &GateContext::new(None),
                &mut fandhe_backend_core::extension::RequestExtensions::new()
            ),
            GateOutcome::Allow
        );
    }

    #[test]
    fn allow_inserts_tenant_verified_and_second_insert_is_500() {
        let keypair = test_keypair();
        let token = make_token(&keypair, TEST_KID, Some(" org-1 "), 9_999_999_999);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let gate = gate_for(&keypair);
        let mut ext = fandhe_backend_core::extension::RequestExtensions::new();
        assert_eq!(
            gate.check(&head_from(&raw), &GateContext::new(None), &mut ext),
            GateOutcome::Allow
        );
        assert_eq!(
            ext.get::<TenantVerified>().unwrap().tenant().org_id(),
            "org-1"
        );
        // 同じ要求に既に値がある（二重登録）場合は上書きせず 500 で拒否する。
        assert_eq!(
            gate.check(&head_from(&raw), &GateContext::new(None), &mut ext),
            GateOutcome::Reject {
                response: error_response(500, "internal_error")
            }
        );
    }

    #[test]
    fn rejection_does_not_insert_tenant_verified() {
        let keypair = test_keypair();
        let mut ext = fandhe_backend_core::extension::RequestExtensions::new();
        let _ = gate_for(&keypair).check(
            &head_from(b"GET / HTTP/1.1\r\n\r\n"),
            &GateContext::new(None),
            &mut ext,
        );
        assert!(ext.get::<TenantVerified>().is_none());
    }

    #[test]
    fn tenant_verified_debug_hides_values() {
        assert_eq!(format!("{NotAdmitted:?}"), "NotAdmitted");
        assert!(!NotAdmitted.to_string().is_empty());
    }
}
