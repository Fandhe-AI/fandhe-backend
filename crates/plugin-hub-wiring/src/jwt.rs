//! HS256（HMAC-SHA256）JWT 検証（フェイルクローズ）。
//!
//! [`crate::gate::TenantGate`] から呼ばれる検証本体。`RequestGate` は
//! `GateOutcome`（許可/拒否のみ）しかコアへ返せない契約
//! （`crates/core/src/extension.rs` doc）のため、抽出したクレーム
//! （[`Claims`]）はここから [`crate::gate`] モジュール内に閉じ、コアへは
//! 一切渡らない。
//!
//! # スパイクである旨（本番流用禁止）
//!
//! HS256 は共有秘密鍵方式であり、複数サービス間で秘密鍵を安全に配布・
//! ローテーションする運用コストが RS256 + JWKS より高い。本実装は
//! `docs/spec/03-poc/hub-wiring-middleware` PoC-6 の再現スパイクであり、
//! TASK-9.2（RS256 + JWKS への差し替え）が完了するまでの暫定実装として
//! `TenantGate` から利用される。本番の複数サービス構成では TASK-9.2 完了後の
//! 実装へ切り替えること。

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// トークン全体（ヘッダ + `.` + ペイロード + `.` + 署名）の長さ上限（バイト）。
///
/// 検証冒頭でこの上限を強制し、巨大トークンによる base64 デコード・JSON
/// パースコストの浪費（DoS）を遮断する。コア側の `RequestHead` サイズ上限
/// （HTTP ヘッダ全体の上限）とは独立した二重防御であり、本クレート単体でも
/// 安全側に倒れることを保証する。
const MAX_TOKEN_LEN: usize = 8192;

/// 検証対象として受理する唯一のアルゴリズム。
///
/// `alg` がこれ以外（`none`・`RS256` 等）の場合は即座に
/// [`TokenError::InvalidAlgorithm`] とし、アルゴリズム混同攻撃・
/// `alg=none` 攻撃を遮断する（.claude/rules/security.md A05）。
const EXPECTED_ALG: &str = "HS256";

/// JWT 検証を通過したトークンから抽出する最小限のクレーム集合。
///
/// `TenantGate` のテナントスコープ判定に必要な `org_id` と、有効期限
/// 判定に使った `exp` のみを保持する。他のクレームは無視する（本モジュールの
/// 責務は認証・テナント識別のみであり、認可ロジックはハンドラ側の責務）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    /// テナント識別子。空文字列・空白のみは無効として扱う
    /// （[`verify_token`] が [`TokenError::MissingOrgId`] を返す）。
    pub org_id: String,
    /// UNIX epoch 秒での有効期限。
    pub exp: u64,
}

/// JWT 検証の失敗理由。
///
/// フェイルクローズ契約（.claude/rules/security.md A01）: あらゆる異常系は
/// 必ずこの列挙のいずれかへ収束し、`Claims` を返す経路はすべての検証を
/// 通過した場合のみ存在する（エラーで `Ok` に落ちる分岐を作らない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// トークン文字列が空、または長さ上限（8192 バイト、モジュール内定数
    /// `MAX_TOKEN_LEN`）を超過した。
    MissingToken,
    /// `header.payload.signature` の 3 分割に失敗、base64url デコード失敗、
    /// またはヘッダ/ペイロード JSON の構造不正。
    Malformed,
    /// ヘッダの `alg` が `HS256`（モジュール内定数 `EXPECTED_ALG`）と一致しない。
    InvalidAlgorithm,
    /// 署名検証（HMAC-SHA256、定数時間比較）に失敗した。
    InvalidSignature,
    /// `exp` が現在時刻以下（期限切れ）。
    Expired,
    /// `org_id` クレームが欠落、または空文字列・空白のみ。
    MissingOrgId,
}

/// JWT ヘッダの最小構造（`alg` のみを検証対象とする）。
#[derive(Deserialize)]
struct Header<'a> {
    alg: &'a str,
}

/// JWT ペイロードの最小構造。未知フィールドは無視する
/// （`serde` の既定動作: 未知フィールドはエラーにしない）。
#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    org_id: Option<String>,
    exp: u64,
}

/// HS256 JWT を検証し、成功時のみ [`Claims`] を返す。
///
/// 検証順序（未認証入力の解釈を最小化する契約、.claude/rules/security.md A01）:
/// 1. トークン長・構造（3 分割）の検証
/// 2. ヘッダ JSON パース・`alg` の完全一致検証
/// 3. **署名検証**（`exp`・`org_id` などクレーム内容の解釈より先に行う）
/// 4. ペイロード JSON パース・`exp` の期限検証・`org_id` の存在検証
///
/// `now_unix` を呼び出し側から注入することで、本関数は時刻依存の副作用を
/// 持たずテスト容易性を確保する（[`crate::gate::TenantGate`] の
/// `RequestGate::check` 実装が `SystemTime::now()` から取得した値を渡す）。
///
/// # Examples
///
/// ```
/// use bf_plugin_hub_wiring::jwt::{verify_token, TokenError};
///
/// // 検証に失敗するトークン（空文字列）は必ず `Err` を返す（フェイルクローズ）。
/// let result = verify_token("", b"secret", 0);
/// assert_eq!(result, Err(TokenError::MissingToken));
/// ```
pub fn verify_token(token: &str, secret: &[u8], now_unix: u64) -> Result<Claims, TokenError> {
    if token.is_empty() || token.len() > MAX_TOKEN_LEN {
        return Err(TokenError::MissingToken);
    }

    let mut parts = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(sig_b64)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return Err(TokenError::Malformed);
    };
    if parts.next().is_some() {
        // 4 分割以上は JWT として不正な形式。
        return Err(TokenError::Malformed);
    }

    let header_json = URL_SAFE_NO_PAD
        .decode(header_b64)
        .map_err(|_| TokenError::Malformed)?;
    let header: Header = serde_json::from_slice(&header_json).map_err(|_| TokenError::Malformed)?;
    if header.alg != EXPECTED_ALG {
        return Err(TokenError::InvalidAlgorithm);
    }

    let signature = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| TokenError::Malformed)?;

    // 署名対象は `header_b64.payload_b64`（元の base64url 表現そのもの、
    // 再エンコードしない）。JWT 標準の署名入力定義に従う。
    let signing_input_len = header_b64.len() + 1 + payload_b64.len();
    let mut signing_input = String::with_capacity(signing_input_len);
    signing_input.push_str(header_b64);
    signing_input.push('.');
    signing_input.push_str(payload_b64);

    let mut mac = HmacSha256::new_from_slice(secret).map_err(|_| TokenError::InvalidSignature)?;
    mac.update(signing_input.as_bytes());
    // `verify_slice` は定数時間比較（RustCrypto `hmac` 内蔵）であり、
    // タイミング攻撃を防ぐための自前実装を必要としない
    // （.claude/rules/security.md A02）。
    mac.verify_slice(&signature)
        .map_err(|_| TokenError::InvalidSignature)?;

    // 署名検証を通過するまでペイロードの中身を解釈しない
    // （未認証入力の解釈最小化）。
    let payload_json = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| TokenError::Malformed)?;
    let payload: Payload =
        serde_json::from_slice(&payload_json).map_err(|_| TokenError::Malformed)?;

    if payload.exp <= now_unix {
        return Err(TokenError::Expired);
    }

    let org_id = payload
        .org_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or(TokenError::MissingOrgId)?;

    Ok(Claims {
        org_id,
        exp: payload.exp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト専用のダミー秘密鍵。実運用値をコード・コミットに含めない
    /// （.claude/rules/security.md シークレット混入防止）。
    const TEST_SECRET: &[u8] = b"test-only-dummy-secret-do-not-use-in-prod";

    /// テストヘルパー: 指定したクレームで HS256 署名済みトークンを組み立てる。
    fn make_token(org_id: Option<&str>, exp: u64, alg: &str, secret: &[u8]) -> String {
        let header = format!(r#"{{"alg":"{alg}","typ":"JWT"}}"#);
        let payload = match org_id {
            Some(org_id) => format!(r#"{{"org_id":"{org_id}","exp":{exp}}}"#),
            None => format!(r#"{{"exp":{exp}}}"#),
        };
        let header_b64 = URL_SAFE_NO_PAD.encode(header);
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{header_b64}.{payload_b64}");

        let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
        mac.update(signing_input.as_bytes());
        let sig = mac.finalize().into_bytes();
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig);

        format!("{header_b64}.{payload_b64}.{sig_b64}")
    }

    #[test]
    fn valid_token_returns_claims() {
        let token = make_token(Some("org-1"), 9_999_999_999, "HS256", TEST_SECRET);
        let claims = verify_token(&token, TEST_SECRET, 0).expect("valid token");
        assert_eq!(claims.org_id, "org-1");
        assert_eq!(claims.exp, 9_999_999_999);
    }

    #[test]
    fn empty_token_is_missing() {
        assert_eq!(
            verify_token("", TEST_SECRET, 0),
            Err(TokenError::MissingToken)
        );
    }

    #[test]
    fn oversized_token_is_missing() {
        let huge = "a".repeat(MAX_TOKEN_LEN + 1);
        assert_eq!(
            verify_token(&huge, TEST_SECRET, 0),
            Err(TokenError::MissingToken)
        );
    }

    #[test]
    fn wrong_part_count_is_malformed() {
        assert_eq!(
            verify_token("a.b", TEST_SECRET, 0),
            Err(TokenError::Malformed)
        );
        assert_eq!(
            verify_token("a.b.c.d", TEST_SECRET, 0),
            Err(TokenError::Malformed)
        );
    }

    #[test]
    fn invalid_base64_is_malformed() {
        assert_eq!(
            verify_token("!!!.!!!.!!!", TEST_SECRET, 0),
            Err(TokenError::Malformed)
        );
    }

    #[test]
    fn invalid_header_json_is_malformed() {
        let header_b64 = URL_SAFE_NO_PAD.encode("not json");
        let payload_b64 = URL_SAFE_NO_PAD.encode(r#"{"org_id":"o","exp":1}"#);
        let token = format!("{header_b64}.{payload_b64}.sig");
        assert_eq!(
            verify_token(&token, TEST_SECRET, 0),
            Err(TokenError::Malformed)
        );
    }

    #[test]
    fn alg_none_is_rejected() {
        let token = make_token(Some("org-1"), 9_999_999_999, "none", TEST_SECRET);
        assert_eq!(
            verify_token(&token, TEST_SECRET, 0),
            Err(TokenError::InvalidAlgorithm)
        );
    }

    #[test]
    fn alg_rs256_is_rejected() {
        let token = make_token(Some("org-1"), 9_999_999_999, "RS256", TEST_SECRET);
        assert_eq!(
            verify_token(&token, TEST_SECRET, 0),
            Err(TokenError::InvalidAlgorithm)
        );
    }

    #[test]
    fn signature_mismatch_is_rejected() {
        let token = make_token(Some("org-1"), 9_999_999_999, "HS256", TEST_SECRET);
        assert_eq!(
            verify_token(&token, b"wrong-secret", 0),
            Err(TokenError::InvalidSignature)
        );
    }

    #[test]
    fn tampered_signature_byte_is_rejected() {
        let token = make_token(Some("org-1"), 9_999_999_999, "HS256", TEST_SECRET);
        let mut parts: Vec<&str> = token.split('.').collect();
        let mut sig = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        sig[0] ^= 0xFF;
        let tampered_sig = URL_SAFE_NO_PAD.encode(sig);
        parts[2] = &tampered_sig;
        let tampered = parts.join(".");
        assert_eq!(
            verify_token(&tampered, TEST_SECRET, 0),
            Err(TokenError::InvalidSignature)
        );
    }

    #[test]
    fn expired_token_is_rejected() {
        let token = make_token(Some("org-1"), 100, "HS256", TEST_SECRET);
        assert_eq!(
            verify_token(&token, TEST_SECRET, 200),
            Err(TokenError::Expired)
        );
    }

    #[test]
    fn exp_equal_to_now_is_rejected() {
        // `exp <= now` を期限切れとする契約（境界値: 等しい場合も拒否）。
        let token = make_token(Some("org-1"), 100, "HS256", TEST_SECRET);
        assert_eq!(
            verify_token(&token, TEST_SECRET, 100),
            Err(TokenError::Expired)
        );
    }

    #[test]
    fn missing_org_id_is_rejected() {
        let token = make_token(None, 9_999_999_999, "HS256", TEST_SECRET);
        assert_eq!(
            verify_token(&token, TEST_SECRET, 0),
            Err(TokenError::MissingOrgId)
        );
    }

    #[test]
    fn blank_org_id_is_rejected() {
        let token = make_token(Some("   "), 9_999_999_999, "HS256", TEST_SECRET);
        assert_eq!(
            verify_token(&token, TEST_SECRET, 0),
            Err(TokenError::MissingOrgId)
        );
    }

    #[test]
    fn empty_org_id_is_rejected() {
        let token = make_token(Some(""), 9_999_999_999, "HS256", TEST_SECRET);
        assert_eq!(
            verify_token(&token, TEST_SECRET, 0),
            Err(TokenError::MissingOrgId)
        );
    }
}
