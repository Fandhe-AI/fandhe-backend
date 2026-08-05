//! `RequestGate` 拡張点上の `TenantGate` 実装（TASK-9.1 の主成果物、TASK-9.2 で
//! RS256 + JWKS へ差し替え）。
//!
//! コアの `Server::gate(impl RequestGate + 'static)`
//! （`crates/core/src/server.rs`）へ登録することで、hub 系サービスが
//! 各自で手書きしていた「JWT 検証 → `org_id` 抽出 → テナントスコープ強制」の
//! 配線をこのプラグイン 1 個の登録に集約する。`RequestGate` はコアループ内で
//! `UpgradeHandler`・`plugin::try_intercept` より先に評価される
//! （`crates/core/src/server.rs` doc）ため、WebSocket アップグレード等の
//! 長時間接続もこの認証をバイパスできない。

use crate::auth::Authenticator;
#[cfg(test)]
use crate::jwks::JwksKeySet;
use crate::jwks::{JwksError, SharedJwks};
use crate::jwt::TokenError;
use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;

/// [`TenantGate`] の設定。JWKS は [`SharedJwks`] ハンドル経由で保持し、
/// 利用側サービスが再起動なしで鍵ローテーション（[`SharedJwks::set`]）を
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
    /// 既に構築済みの [`SharedJwks`] ハンドルから設定を組み立てる。
    /// 呼び出し元が [`SharedJwks`] を保持し続けることで、`TenantGate` 登録後も
    /// [`SharedJwks::set`] による鍵ローテーションを行える。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::gate::TenantGateConfig;
    /// use fandhe_backend_plugin_hub_wiring::jwks::{JwksKeySet, SharedJwks};
    ///
    /// let shared = SharedJwks::new(JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap());
    /// let config = TenantGateConfig::new(shared);
    /// ```
    pub fn new(jwks: SharedJwks) -> Self {
        Self {
            authenticator: Authenticator::new(jwks),
        }
    }

    /// JWKS JSON ドキュメントから直接設定を組み立てる便宜コンストラクタ。
    /// 事後にローテーションする必要がない・単純な起動時注入のみで足りる
    /// 利用側サービス向け（ローテーションが必要な場合は [`SharedJwks::new`] を
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
    ///
    /// let config = TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#).unwrap();
    /// ```
    pub fn from_jwks_json(json: &str) -> Result<Self, JwksError> {
        Ok(Self::new(SharedJwks::from_json(json)?))
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
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::gate::{TenantGate, TenantGateConfig};
    ///
    /// let config = TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#).unwrap();
    /// let authenticator = config.authenticator();
    /// let gate = TenantGate::new(config);
    /// // `authenticator` はハンドラ側で保持し、`gate` はコアへ登録する。
    /// drop(gate);
    /// ```
    pub fn authenticator(&self) -> Authenticator {
        self.authenticator.clone()
    }
}

/// `Authorization: Bearer <JWT>` を検証し、テナントスコープをフェイルクローズで
/// 強制する [`RequestGate`] 実装。
///
/// 判定ポリシー（PoC-6 踏襲、TASK-9.2 で RS256 + JWKS 前提へ更新）:
/// - トークン欠落・形式不正・アルゴリズム不正・`kid` 欠落・未知 `kid`・
///   署名不一致・期限切れ → `401`（認証失敗）
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
    /// let config = TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#).unwrap();
    /// let gate = TenantGate::new(config);
    /// assert_eq!(gate.name(), "hub-tenant-gate");
    /// ```
    pub fn new(config: TenantGateConfig) -> Self {
        Self { config }
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

    fn check(&self, head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
        // `check()` は同期・非ブロッキング（I/O なし）で Tokio ワーカーを
        // 塞がない契約を維持する（`crates/core/src/extension.rs` doc）。
        // `Authenticator::authenticate` はロックを短時間保持するのみで、
        // 実際の署名検証（キャッシュミス時のみ）中はロックを保持しない
        // （.claude/rules/coding-rust.md）。判定ポリシー（401/403 マッピング）は
        // 従来の `verify_token` 直接呼び出しと完全に同一であり、キャッシュ
        // ヒット/ミスで判定結果が変わることはない（TASK-9.3 / #63）。テナント
        // 境界判定はトークンクレーム（`org_id`）のみに基づき、`ctx` の実 peer
        // address は本ゲートの判定に使用しない（イシュー #486、`GateContext`
        // 追加は既存 gate 実装への影響を最小化する非対応追随のみ）。
        match self.config.authenticator.authenticate(head) {
            Ok(_claims) => GateOutcome::Allow,
            Err(TokenError::MissingOrgId) => reject_json(403, FORBIDDEN_BODY),
            Err(
                TokenError::MissingToken
                | TokenError::Malformed
                | TokenError::InvalidAlgorithm
                | TokenError::MissingKeyId
                | TokenError::UnknownKeyId
                | TokenError::InvalidSignature
                | TokenError::Expired,
            ) => reject_json(401, UNAUTHORIZED_BODY),
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

    const TEST_KID: &str = "test-kid-1";

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
        let header = format!(r#"{{"alg":"RS256","typ":"JWT","kid":"{kid}"}}"#);
        let payload = match org_id {
            Some(org_id) => format!(r#"{{"org_id":"{org_id}","exp":{exp}}}"#),
            None => format!(r#"{{"exp":{exp}}}"#),
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
        let config = TenantGateConfig::from_jwks_json(&jwks_json_for(keypair, TEST_KID)).unwrap();
        TenantGate::new(config)
    }

    #[test]
    fn missing_authorization_header_is_401() {
        let keypair = test_keypair();
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        assert_eq!(
            gate_for(&keypair).check(&head, &GateContext::new(None)),
            reject_json(401, UNAUTHORIZED_BODY)
        );
    }

    #[test]
    fn non_bearer_scheme_is_401() {
        let keypair = test_keypair();
        let raw = b"GET / HTTP/1.1\r\nAuthorization: Basic abcdef\r\n\r\n".to_vec();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(&head, &GateContext::new(None)),
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
            gate_for(&keypair).check(&head, &GateContext::new(None)),
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
            gate_for(&keypair).check(&head, &GateContext::new(None)),
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
            gate_for(&keypair).check(&head, &GateContext::new(None)),
            GateOutcome::Allow
        );
    }

    #[test]
    fn bearer_scheme_without_token_is_401() {
        let keypair = test_keypair();
        let raw = b"GET / HTTP/1.1\r\nAuthorization: Bearer \r\n\r\n".to_vec();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(&head, &GateContext::new(None)),
            reject_json(401, UNAUTHORIZED_BODY)
        );
    }

    #[test]
    fn malformed_token_is_401() {
        let keypair = test_keypair();
        let raw = b"GET / HTTP/1.1\r\nAuthorization: Bearer not-a-jwt\r\n\r\n".to_vec();
        let head = head_from(&raw);
        assert_eq!(
            gate_for(&keypair).check(&head, &GateContext::new(None)),
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
            gate_for(&keypair).check(&head, &GateContext::new(None)),
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
            gate_for(&keypair).check(&head, &GateContext::new(None)),
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
        let empty_config = TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#).unwrap();
        let gate = TenantGate::new(empty_config);
        assert_eq!(
            gate.check(&head, &GateContext::new(None)),
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
            gate_for(&keypair).check(&head, &GateContext::new(None)),
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
        let GateOutcome::Reject { response } =
            gate_for(&keypair).check(&head, &GateContext::new(None))
        else {
            panic!("expected Reject for missing Authorization header");
        };
        let wire = String::from_utf8(response.serialize(false)).unwrap();
        assert!(wire.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(wire.contains("Content-Length: "));
        assert!(wire.contains("Content-Type: application/json\r\n"));
        assert!(!wire.contains("Retry-After:"));
        assert!(wire.ends_with(&String::from_utf8_lossy(UNAUTHORIZED_BODY).into_owned()));

        let token = make_token(&keypair, TEST_KID, None, 9_999_999_999);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        let GateOutcome::Reject { response } =
            gate_for(&keypair).check(&head, &GateContext::new(None))
        else {
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
        match gate_for(&keypair).check(&head, &GateContext::new(None)) {
            GateOutcome::Reject { response } => {
                let body_str = String::from_utf8_lossy(&response.body);
                assert!(!body_str.contains(&token));
            }
            GateOutcome::Allow => panic!("expected Reject for expired token"),
        }
    }

    #[test]
    fn config_rotation_reflects_in_check() {
        // `SharedJwks::set` によるローテーション後、旧鍵で署名したトークンは
        // 拒否され新鍵で署名したトークンが許可されることを確認する。
        let keypair = test_keypair();
        let rotated_keypair = RsaKeyPair::from_pkcs8(include_bytes!(
            "../tests/fixtures/test-rsa-2048-rotated.pk8"
        ))
        .expect("valid pkcs8 fixture");

        let shared =
            crate::jwks::SharedJwks::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let gate = TenantGate::new(TenantGateConfig::new(shared.clone()));

        let old_token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999);
        let raw =
            format!("GET / HTTP/1.1\r\nAuthorization: Bearer {old_token}\r\n\r\n").into_bytes();
        assert_eq!(
            gate.check(&head_from(&raw), &GateContext::new(None)),
            GateOutcome::Allow
        );

        shared.set(JwksKeySet::from_json(&jwks_json_for(&rotated_keypair, TEST_KID)).unwrap());

        // 旧鍵の署名は新 JWKS では検証できず拒否される。
        assert_eq!(
            gate.check(&head_from(&raw), &GateContext::new(None)),
            reject_json(401, UNAUTHORIZED_BODY)
        );

        let new_token = make_token(&rotated_keypair, TEST_KID, Some("org-1"), 9_999_999_999);
        let raw2 =
            format!("GET / HTTP/1.1\r\nAuthorization: Bearer {new_token}\r\n\r\n").into_bytes();
        assert_eq!(
            gate.check(&head_from(&raw2), &GateContext::new(None)),
            GateOutcome::Allow
        );
    }
}
