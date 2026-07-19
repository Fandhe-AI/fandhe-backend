//! RS256（RSASSA-PKCS1-v1_5 using SHA-256）JWT 検証（フェイルクローズ）。
//!
//! [`crate::gate::TenantGate`] から呼ばれる検証本体。`RequestGate` は
//! `GateOutcome`（許可/拒否のみ）しかコアへ返せない契約
//! （`crates/core/src/extension.rs` doc）のため、抽出したクレーム
//! （[`Claims`]）はここから [`crate::gate`] モジュール内に閉じ、コアへは
//! 一切渡らない。
//!
//! # RS256 + JWKS（TASK-9.2 / #62）
//!
//! TASK-9.1（#61）の HS256（HMAC 共有秘密鍵）スパイクを本番相当構成へ
//! 差し替えた実装。署名検証は非対称鍵（RSA 公開鍵）に基づき、鍵は
//! [`crate::jwks`] が提供する [`crate::jwks::JwksKeySet`] から
//! ヘッダの `kid`（Key ID）で選択する。秘密鍵材料はこのプラグインに
//! 一切存在しない（JWKS は公開鍵のみを扱う）。署名検証本体は
//! `ring::signature::RsaPublicKeyComponents::verify` に委ね、自前で
//! RSA 演算を実装しない（.claude/rules/security.md A02）。

use crate::jwks::JwksKeySet;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::{self, RsaPublicKeyComponents};
use serde::Deserialize;

/// トークン全体（ヘッダ + `.` + ペイロード + `.` + 署名）の長さ上限（バイト）。
///
/// 検証冒頭でこの上限を強制し、巨大トークンによる base64 デコード・JSON
/// パースコストの浪費（DoS）を遮断する。コア側の `RequestHead` サイズ上限
/// （HTTP ヘッダ全体の上限）とは独立した二重防御であり、本クレート単体でも
/// 安全側に倒れることを保証する。
const MAX_TOKEN_LEN: usize = 8192;

/// 検証対象として受理する唯一のアルゴリズム。
///
/// `alg` がこれ以外（`none`・`HS256` 等）の場合は即座に
/// [`TokenError::InvalidAlgorithm`] とし、アルゴリズム混同攻撃・
/// `alg=none` 攻撃・対称鍵/非対称鍵混同攻撃を遮断する
/// （.claude/rules/security.md A05）。
const EXPECTED_ALG: &str = "RS256";

/// RS256 検証パラメータ（PKCS#1 v1.5 パディング、鍵長 2048〜8192 bit、SHA-256）。
///
/// 2048 bit 未満の弱鍵は ring がこのパラメータで検証時に自動的に拒否する
/// （`ring::rsa::verification` の `min_bits` チェック）ため、本クレート側で
/// 鍵長の別途チェックを実装する必要はない。
static RSA_VERIFY_PARAMS: &signature::RsaParameters = &signature::RSA_PKCS1_2048_8192_SHA256;

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
    /// ヘッダの `alg` が `RS256`（モジュール内定数 `EXPECTED_ALG`）と一致しない。
    InvalidAlgorithm,
    /// ヘッダに `kid`（Key ID）が存在しない。
    MissingKeyId,
    /// ヘッダの `kid` に一致する鍵が注入済み JWKS 内に存在しない。
    /// 鍵セットが空（JWKS 未注入・パース失敗未対応）の場合も本エラーになる。
    UnknownKeyId,
    /// 署名検証（RS256、ring 内蔵の RSA PKCS#1 v1.5 検証）に失敗した。
    /// 弱鍵（2048 bit 未満）による署名もここに含まれる（ring が拒否するため）。
    InvalidSignature,
    /// `exp` が現在時刻以下（期限切れ）。
    Expired,
    /// `org_id` クレームが欠落、または空文字列・空白のみ。
    MissingOrgId,
}

/// JWT ヘッダの最小構造（`alg`・`kid` のみを検証対象とする）。
#[derive(Deserialize)]
struct Header<'a> {
    alg: &'a str,
    kid: Option<&'a str>,
}

/// JWT ペイロードの最小構造。未知フィールドは無視する
/// （`serde` の既定動作: 未知フィールドはエラーにしない）。
#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    org_id: Option<String>,
    exp: u64,
}

/// RS256 JWT を検証し、成功時のみ [`Claims`] を返す。
///
/// 検証順序（未認証入力の解釈を最小化する契約、.claude/rules/security.md A01）:
/// 1. トークン長・構造（3 分割）の検証
/// 2. ヘッダ JSON パース・`alg` の完全一致検証・`kid` の抽出
/// 3. `kid` による JWKS 内の鍵選択（未知 `kid` は [`TokenError::UnknownKeyId`]）
/// 4. **署名検証**（`exp`・`org_id` などクレーム内容の解釈より先に行う）
/// 5. ペイロード JSON パース・`exp` の期限検証・`org_id` の存在検証
///
/// `now_unix` を呼び出し側から注入することで、本関数は時刻依存の副作用を
/// 持たずテスト容易性を確保する（[`crate::gate::TenantGate`] の
/// `RequestGate::check` 実装が `SystemTime::now()` から取得した値を渡す）。
///
/// 鍵セットが空（`keys` の件数が 0）の場合はすべてのトークンが
/// [`TokenError::UnknownKeyId`] になる。フェイルオープン（鍵なし = 常時許可）
/// にはしない（.claude/rules/security.md A01）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_hub_wiring::jwks::JwksKeySet;
/// use fandhe_backend_plugin_hub_wiring::jwt::{verify_token, TokenError};
///
/// let keys = JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap();
///
/// // 検証に失敗するトークン（空文字列）は必ず `Err` を返す（フェイルクローズ）。
/// let result = verify_token("", &keys, 0);
/// assert_eq!(result, Err(TokenError::MissingToken));
/// ```
pub fn verify_token(token: &str, keys: &JwksKeySet, now_unix: u64) -> Result<Claims, TokenError> {
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
    let kid = header.kid.ok_or(TokenError::MissingKeyId)?;

    let (n, e) = keys.find_by_kid(kid).ok_or(TokenError::UnknownKeyId)?;

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

    let public_key = RsaPublicKeyComponents { n, e };
    // ring の RSA PKCS#1 v1.5 検証は定数時間実装であり、タイミング攻撃を
    // 防ぐための自前実装を必要としない（.claude/rules/security.md A02）。
    // `RSA_VERIFY_PARAMS` の `min_bits = 2048` により弱鍵署名もここで拒否される。
    public_key
        .verify(RSA_VERIFY_PARAMS, signing_input.as_bytes(), &signature)
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
    use ring::rand::SystemRandom;
    use ring::signature::{KeyPair, RsaKeyPair};

    /// テスト専用の RSA 2048bit 秘密鍵（PKCS#8 DER）。
    /// `tests/fixtures/README.md` に生成コマンドと注意事項を記載。
    /// 本番使用禁止（.claude/rules/security.md シークレット混入防止）。
    const TEST_PKCS8: &[u8] = include_bytes!("../tests/fixtures/test-rsa-2048.pk8");
    const TEST_KID: &str = "test-kid-1";

    fn test_keypair() -> RsaKeyPair {
        RsaKeyPair::from_pkcs8(TEST_PKCS8).expect("valid pkcs8 fixture")
    }

    /// `RsaKeyPair` の公開鍵から JWKS JSON（1 鍵）を組み立てる。
    fn jwks_json_for(keypair: &RsaKeyPair, kid: &str) -> String {
        let components: RsaPublicKeyComponents<Vec<u8>> =
            RsaPublicKeyComponents::from(keypair.public_key());
        let n_b64 = URL_SAFE_NO_PAD.encode(&components.n);
        let e_b64 = URL_SAFE_NO_PAD.encode(&components.e);
        format!(
            r#"{{"keys":[{{"kty":"RSA","kid":"{kid}","n":"{n_b64}","e":"{e_b64}","use":"sig","alg":"RS256"}}]}}"#
        )
    }

    /// テストヘルパー: 指定したクレームで RS256 署名済みトークンを組み立てる。
    fn make_token(
        keypair: &RsaKeyPair,
        kid: &str,
        org_id: Option<&str>,
        exp: u64,
        alg: &str,
    ) -> String {
        let header = format!(r#"{{"alg":"{alg}","typ":"JWT","kid":"{kid}"}}"#);
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

    /// ヘッダに `kid` を含めないトークンを組み立てる（`MissingKeyId` 検証用）。
    fn make_token_without_kid(keypair: &RsaKeyPair, org_id: Option<&str>, exp: u64) -> String {
        let header = r#"{"alg":"RS256","typ":"JWT"}"#.to_string();
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

    #[test]
    fn valid_token_returns_claims() {
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");
        let claims = verify_token(&token, &keys, 0).expect("valid token");
        assert_eq!(claims.org_id, "org-1");
        assert_eq!(claims.exp, 9_999_999_999);
    }

    #[test]
    fn empty_token_is_missing() {
        let keys = JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap();
        assert_eq!(verify_token("", &keys, 0), Err(TokenError::MissingToken));
    }

    #[test]
    fn oversized_token_is_missing() {
        let keys = JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap();
        let huge = "a".repeat(MAX_TOKEN_LEN + 1);
        assert_eq!(verify_token(&huge, &keys, 0), Err(TokenError::MissingToken));
    }

    #[test]
    fn wrong_part_count_is_malformed() {
        let keys = JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap();
        assert_eq!(verify_token("a.b", &keys, 0), Err(TokenError::Malformed));
        assert_eq!(
            verify_token("a.b.c.d", &keys, 0),
            Err(TokenError::Malformed)
        );
    }

    #[test]
    fn invalid_base64_is_malformed() {
        let keys = JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap();
        assert_eq!(
            verify_token("!!!.!!!.!!!", &keys, 0),
            Err(TokenError::Malformed)
        );
    }

    #[test]
    fn invalid_header_json_is_malformed() {
        let keys = JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap();
        let header_b64 = URL_SAFE_NO_PAD.encode("not json");
        let payload_b64 = URL_SAFE_NO_PAD.encode(r#"{"org_id":"o","exp":1}"#);
        let token = format!("{header_b64}.{payload_b64}.sig");
        assert_eq!(verify_token(&token, &keys, 0), Err(TokenError::Malformed));
    }

    #[test]
    fn alg_none_is_rejected() {
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        // `alg=none` は署名検証自体が意味をなさないため、署名は付けず生成する。
        let header_b64 = URL_SAFE_NO_PAD.encode(format!(r#"{{"alg":"none","kid":"{TEST_KID}"}}"#));
        let payload_b64 = URL_SAFE_NO_PAD.encode(r#"{"org_id":"org-1","exp":9999999999}"#);
        let token = format!("{header_b64}.{payload_b64}.");
        assert_eq!(
            verify_token(&token, &keys, 0),
            Err(TokenError::InvalidAlgorithm)
        );
    }

    #[test]
    fn alg_hs256_downgrade_is_rejected() {
        // HS256 ダウングレード攻撃: RS256 用に配布された公開鍵情報を HMAC 鍵と
        // 誤用させる攻撃を、`alg` 完全一致検証で遮断する
        // （.claude/rules/security.md A05）。
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "HS256");
        assert_eq!(
            verify_token(&token, &keys, 0),
            Err(TokenError::InvalidAlgorithm)
        );
    }

    #[test]
    fn missing_kid_is_rejected() {
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let token = make_token_without_kid(&keypair, Some("org-1"), 9_999_999_999);
        assert_eq!(
            verify_token(&token, &keys, 0),
            Err(TokenError::MissingKeyId)
        );
    }

    #[test]
    fn unknown_kid_is_rejected() {
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let token = make_token(&keypair, "other-kid", Some("org-1"), 9_999_999_999, "RS256");
        assert_eq!(
            verify_token(&token, &keys, 0),
            Err(TokenError::UnknownKeyId)
        );
    }

    #[test]
    fn empty_key_set_rejects_every_token() {
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");
        assert_eq!(
            verify_token(&token, &keys, 0),
            Err(TokenError::UnknownKeyId)
        );
    }

    #[test]
    fn signature_mismatch_with_different_key_is_rejected() {
        let keypair = test_keypair();
        let other_keypair = RsaKeyPair::from_pkcs8(include_bytes!(
            "../tests/fixtures/test-rsa-2048-rotated.pk8"
        ))
        .expect("valid pkcs8 fixture");
        // JWKS には `keypair` の公開鍵を登録するが、トークンは別鍵で署名する
        // （署名不一致を模擬）。
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let token = make_token(
            &other_keypair,
            TEST_KID,
            Some("org-1"),
            9_999_999_999,
            "RS256",
        );
        assert_eq!(
            verify_token(&token, &keys, 0),
            Err(TokenError::InvalidSignature)
        );
    }

    #[test]
    fn tampered_signature_byte_is_rejected() {
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999, "RS256");
        let mut parts: Vec<&str> = token.split('.').collect();
        let mut sig = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        sig[0] ^= 0xFF;
        let tampered_sig = URL_SAFE_NO_PAD.encode(sig);
        parts[2] = &tampered_sig;
        let tampered = parts.join(".");
        assert_eq!(
            verify_token(&tampered, &keys, 0),
            Err(TokenError::InvalidSignature)
        );
    }

    #[test]
    fn expired_token_is_rejected() {
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 100, "RS256");
        assert_eq!(verify_token(&token, &keys, 200), Err(TokenError::Expired));
    }

    #[test]
    fn exp_equal_to_now_is_rejected() {
        // `exp <= now` を期限切れとする契約（境界値: 等しい場合も拒否）。
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 100, "RS256");
        assert_eq!(verify_token(&token, &keys, 100), Err(TokenError::Expired));
    }

    #[test]
    fn missing_org_id_is_rejected() {
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let token = make_token(&keypair, TEST_KID, None, 9_999_999_999, "RS256");
        assert_eq!(
            verify_token(&token, &keys, 0),
            Err(TokenError::MissingOrgId)
        );
    }

    #[test]
    fn blank_org_id_is_rejected() {
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let token = make_token(&keypair, TEST_KID, Some("   "), 9_999_999_999, "RS256");
        assert_eq!(
            verify_token(&token, &keys, 0),
            Err(TokenError::MissingOrgId)
        );
    }

    #[test]
    fn empty_org_id_is_rejected() {
        let keypair = test_keypair();
        let keys = JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let token = make_token(&keypair, TEST_KID, Some(""), 9_999_999_999, "RS256");
        assert_eq!(
            verify_token(&token, &keys, 0),
            Err(TokenError::MissingOrgId)
        );
    }
}
