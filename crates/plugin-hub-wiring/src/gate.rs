//! `RequestGate` 拡張点上の `TenantGate` 実装（TASK-9.1 の主成果物）。
//!
//! コアの `Server::gate(impl RequestGate + 'static)`
//! （`crates/core/src/server.rs`）へ登録することで、hub 系サービスが
//! 各自で手書きしていた「JWT 検証 → `org_id` 抽出 → テナントスコープ強制」の
//! 配線をこのプラグイン 1 個の登録に集約する。`RequestGate` はコアループ内で
//! `UpgradeHandler`・`plugin::try_intercept` より先に評価される
//! （`crates/core/src/server.rs` doc）ため、WebSocket アップグレード等の
//! 長時間接続もこの認証をバイパスできない。

use crate::jwt::{TokenError, verify_token};
use backend_framework_core::extension::{GateOutcome, RequestGate};
use bf_http::request::RequestHead;
use std::time::{SystemTime, UNIX_EPOCH};

/// [`TenantGate`] の設定。秘密鍵はバイト列として利用側サービスが注入する
/// （本クレートは env 読み取り等を行わない、.claude/rules/security.md
/// シークレット管理）。
pub struct TenantGateConfig {
    secret: Vec<u8>,
}

impl TenantGateConfig {
    /// HS256 検証に使う共有秘密鍵から設定を組み立てる。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_hub_wiring::gate::TenantGateConfig;
    ///
    /// let config = TenantGateConfig::new(b"shared-secret".to_vec());
    /// // Debug 出力は秘密鍵の値を含まない（下記 Debug 実装を参照）。
    /// assert!(format!("{config:?}").contains("REDACTED"));
    /// ```
    pub fn new(secret: Vec<u8>) -> Self {
        Self { secret }
    }
}

// 秘密鍵をログ・パニックメッセージへ流出させないため `Debug` を手動実装する
// （.claude/rules/security.md シークレット管理、A02）。`#[derive(Debug)]` は
// フィールド値をそのまま出力してしまうため使わない。
impl std::fmt::Debug for TenantGateConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantGateConfig")
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

/// `Authorization: Bearer <JWT>` を検証し、テナントスコープをフェイルクローズで
/// 強制する [`RequestGate`] 実装。
///
/// 判定ポリシー（PoC-6 踏襲）:
/// - トークン欠落・形式不正・アルゴリズム不正・署名不一致・期限切れ →
///   `401`（認証失敗）
/// - 署名は妥当だが `org_id` クレームが欠落・空 → `403`
///   （クライアント入力誤りとしてアプリ層の明示的拒否）
/// - 上記いずれにも該当しない検証成功時のみ `Allow`
///
/// `GateOutcome` は許可/拒否の判定結果のみを運ぶ契約
/// （`crates/core/src/extension.rs` doc）のため、検証で得た `org_id` 等の
/// クレームはこの構造体・呼び出しの外へ一切持ち出さない（コアは hub 固有の
/// シンボルへ依存しない）。ハンドラ側で認証済み情報を再利用したい場合は
/// [`crate::jwt::verify_token`] を直接呼ぶ（TASK-9.3 のキャッシュ最適化対象）。
pub struct TenantGate {
    config: TenantGateConfig,
}

impl TenantGate {
    /// 設定を受け取り `TenantGate` を組み立てる。
    ///
    /// # Examples
    ///
    /// ```
    /// use backend_framework_core::extension::RequestGate;
    /// use bf_plugin_hub_wiring::gate::{TenantGate, TenantGateConfig};
    ///
    /// let gate = TenantGate::new(TenantGateConfig::new(b"secret".to_vec()));
    /// assert_eq!(gate.name(), "hub-tenant-gate");
    /// ```
    pub fn new(config: TenantGateConfig) -> Self {
        Self { config }
    }
}

/// `401` 応答の固定 body。トークン内容・ヘッダ値を一切反映しない
/// （情報漏えい・ヘッダ/ログインジェクション防止、.claude/rules/security.md A03）。
const UNAUTHORIZED_BODY: &[u8] = br#"{"error":"invalid_token"}"#;

/// `403` 応答の固定 body。
const FORBIDDEN_BODY: &[u8] = br#"{"error":"tenant_scope_required"}"#;

/// RFC 6750 の `Bearer` スキーム名（大文字小文字を区別しない）。
const BEARER_SCHEME: &str = "bearer";

impl RequestGate for TenantGate {
    fn name(&self) -> &'static str {
        "hub-tenant-gate"
    }

    fn check(&self, head: &RequestHead) -> GateOutcome {
        let Some(authorization) = head.header("authorization") else {
            return GateOutcome::Reject {
                status: 401,
                body: UNAUTHORIZED_BODY.to_vec(),
            };
        };

        // RFC 6750 (`credentials = auth-scheme 1*SP token68`):
        // スキーム名 `Bearer` は大文字小文字を区別せず、スキーム名とトークンの
        // 間には 1 個以上の SP が許容される。固定長プレフィックス一致だと
        // 「Bearer」の後にスペースが 2 個以上並ぶ正当なヘッダを誤って
        // 拒否してしまう（先頭スペースが token68 側に混入し検証失敗する）ため、
        // スキーム名部分とスペース列を明示的に分離して剥がす。
        let Some(scheme_end) = authorization.find(' ') else {
            return GateOutcome::Reject {
                status: 401,
                body: UNAUTHORIZED_BODY.to_vec(),
            };
        };
        // `find(' ')` は ASCII 空白（1 バイト固定）の位置を返す。ASCII バイトは
        // UTF-8 マルチバイト列の継続バイトとして現れ得ないため、この位置での
        // スライスは常に char 境界であり安全。
        let scheme = &authorization[..scheme_end];
        if !scheme.eq_ignore_ascii_case(BEARER_SCHEME) {
            return GateOutcome::Reject {
                status: 401,
                body: UNAUTHORIZED_BODY.to_vec(),
            };
        }
        let token = authorization[scheme_end..].trim_start_matches(' ');
        if token.is_empty() {
            return GateOutcome::Reject {
                status: 401,
                body: UNAUTHORIZED_BODY.to_vec(),
            };
        }

        // `check()` は同期・非ブロッキング（I/O なし）で Tokio ワーカーを
        // 塞がない契約を維持する（`crates/core/src/extension.rs` doc）。
        // 現在時刻の取得のみで、検証ロジック本体（`verify_token`）は
        // 時刻注入可能な純粋関数としてテスト容易性を確保している。
        //
        // フェイルクローズ: `SystemTime::now()` が UNIX epoch 秒への変換に
        // 失敗した場合（クロック異常）に `0` を渡すと `exp <= now_unix` の
        // 期限切れ判定が常に false になり、あらゆる `exp` を「期限内」として
        // 誤許可してしまう（.claude/rules/security.md フェイルクローズ原則）。
        // `u64::MAX` を渡すことで、正の `exp` を持つトークンは無条件に
        // `TokenError::Expired`（401）として拒否される側へ倒す。
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(u64::MAX);

        match verify_token(token, &self.config.secret, now_unix) {
            Ok(_claims) => GateOutcome::Allow,
            Err(TokenError::MissingOrgId) => GateOutcome::Reject {
                status: 403,
                body: FORBIDDEN_BODY.to_vec(),
            },
            Err(
                TokenError::MissingToken
                | TokenError::Malformed
                | TokenError::InvalidAlgorithm
                | TokenError::InvalidSignature
                | TokenError::Expired,
            ) => GateOutcome::Reject {
                status: 401,
                body: UNAUTHORIZED_BODY.to_vec(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use bf_http::request::{ParseOutcome, parse_request_head};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    const TEST_SECRET: &[u8] = b"test-only-dummy-secret-do-not-use-in-prod";

    fn head_from(raw: &[u8]) -> RequestHead {
        match parse_request_head(raw).expect("valid head") {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => unreachable!("test fixtures are always complete"),
        }
    }

    fn make_token(org_id: Option<&str>, exp: u64, alg: &str, secret: &[u8]) -> String {
        let header = format!(r#"{{"alg":"{alg}","typ":"JWT"}}"#);
        let payload = match org_id {
            Some(org_id) => format!(r#"{{"org_id":"{org_id}","exp":{exp}}}"#),
            None => format!(r#"{{"exp":{exp}}}"#),
        };
        let header_b64 = URL_SAFE_NO_PAD.encode(header);
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{header_b64}.{payload_b64}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("any length key");
        mac.update(signing_input.as_bytes());
        let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{header_b64}.{payload_b64}.{sig}")
    }

    fn gate() -> TenantGate {
        TenantGate::new(TenantGateConfig::new(TEST_SECRET.to_vec()))
    }

    #[test]
    fn missing_authorization_header_is_401() {
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        assert_eq!(
            gate().check(&head),
            GateOutcome::Reject {
                status: 401,
                body: UNAUTHORIZED_BODY.to_vec()
            }
        );
    }

    #[test]
    fn non_bearer_scheme_is_401() {
        let raw = b"GET / HTTP/1.1\r\nAuthorization: Basic abcdef\r\n\r\n".to_vec();
        let head = head_from(&raw);
        assert_eq!(
            gate().check(&head),
            GateOutcome::Reject {
                status: 401,
                body: UNAUTHORIZED_BODY.to_vec()
            }
        );
    }

    #[test]
    fn valid_token_is_allow() {
        let token = make_token(Some("org-1"), 9_999_999_999, "HS256", TEST_SECRET);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        assert_eq!(gate().check(&head), GateOutcome::Allow);
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        let token = make_token(Some("org-1"), 9_999_999_999, "HS256", TEST_SECRET);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: bEaReR {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        assert_eq!(gate().check(&head), GateOutcome::Allow);
    }

    #[test]
    fn bearer_scheme_allows_extra_spaces_before_token() {
        // RFC 6750 は auth-scheme と token68 の間に 1*SP を許容する。
        // 固定 7 バイトプレフィックス剥がしだと 2 個以上のスペースで
        // 誤って 401 を返していた回帰を防ぐ。
        let token = make_token(Some("org-1"), 9_999_999_999, "HS256", TEST_SECRET);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer   {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        assert_eq!(gate().check(&head), GateOutcome::Allow);
    }

    #[test]
    fn bearer_scheme_without_token_is_401() {
        let raw = b"GET / HTTP/1.1\r\nAuthorization: Bearer \r\n\r\n".to_vec();
        let head = head_from(&raw);
        assert_eq!(
            gate().check(&head),
            GateOutcome::Reject {
                status: 401,
                body: UNAUTHORIZED_BODY.to_vec()
            }
        );
    }

    #[test]
    fn malformed_token_is_401() {
        let raw = b"GET / HTTP/1.1\r\nAuthorization: Bearer not-a-jwt\r\n\r\n".to_vec();
        let head = head_from(&raw);
        assert_eq!(
            gate().check(&head),
            GateOutcome::Reject {
                status: 401,
                body: UNAUTHORIZED_BODY.to_vec()
            }
        );
    }

    #[test]
    fn expired_token_is_401() {
        let token = make_token(Some("org-1"), 1, "HS256", TEST_SECRET);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        assert_eq!(
            gate().check(&head),
            GateOutcome::Reject {
                status: 401,
                body: UNAUTHORIZED_BODY.to_vec()
            }
        );
    }

    #[test]
    fn missing_org_id_is_403() {
        let token = make_token(None, 9_999_999_999, "HS256", TEST_SECRET);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        assert_eq!(
            gate().check(&head),
            GateOutcome::Reject {
                status: 403,
                body: FORBIDDEN_BODY.to_vec()
            }
        );
    }

    #[test]
    fn reject_body_never_reflects_token_content() {
        // Reject body は静的固定文字列であり、トークン値・秘密鍵を含まない
        // ことを固定する（.claude/rules/security.md A03）。
        let token = make_token(Some("org-1"), 1, "HS256", TEST_SECRET);
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        let head = head_from(&raw);
        match gate().check(&head) {
            GateOutcome::Reject { body, .. } => {
                let body_str = String::from_utf8_lossy(&body);
                assert!(!body_str.contains(&token));
                assert!(!body_str.contains(std::str::from_utf8(TEST_SECRET).unwrap()));
            }
            GateOutcome::Allow => panic!("expected Reject for expired token"),
        }
    }

    #[test]
    fn config_debug_redacts_secret() {
        let config = TenantGateConfig::new(TEST_SECRET.to_vec());
        let debug = format!("{config:?}");
        assert!(!debug.contains("test-only-dummy-secret"));
        assert!(debug.contains("REDACTED"));
    }
}
