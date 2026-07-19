//! JWKS（JSON Web Key Set、[RFC 7517]）のパースとローテーション可能な保持。
//!
//! [`crate::jwt::verify_token`] が RS256 検証に使う公開鍵（`n`/`e`）を
//! [`JwksKeySet`] として供給する。JWKS の**取得**（HTTP フェッチ・自動リフレッシュ）は
//! 本クレートの責務外（`RequestGate::check` は同期・I/O なしの契約、
//! `crates/core/src/extension.rs` doc）であり、利用側サービスが取得した JSON
//! ドキュメントを [`JwksKeySet::from_json`] でパースして注入する。
//! 鍵ローテーション（再起動なしの差し替え）は [`SharedJwks`] が担う。
//!
//! [RFC 7517]: https://www.rfc-editor.org/rfc/rfc7517

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use std::sync::{Arc, RwLock};

/// JWKS ドキュメント（JSON テキスト）の長さ上限（バイト）。
///
/// パース前にこの上限を強制し、巨大ドキュメントによる JSON パースコストの
/// 浪費（DoS、.claude/rules/security.md）を遮断する。
const MAX_DOC_LEN: usize = 64 * 1024;

/// 採用する鍵の件数上限。
///
/// 上限超過時は [`JwksError::TooManyKeys`] としてドキュメント全体を拒否する
/// （フェイルクローズ。一部だけ採用して残りを黙って無視すると、利用側が
/// 意図した鍵集合と実際に使われる鍵集合が乖離しうる）。
const MAX_KEYS: usize = 32;

/// base64url デコード後の `n`（モジュラス）の長さ上限（バイト）。
///
/// [`crate::jwt::RSA_VERIFY_PARAMS`]（`RSA_PKCS1_2048_8192_SHA256`）が許容する
/// 最大 8192 bit = 1024 byte に上限を合わせる。これを超える `n` はどのみち
/// 署名検証時に ring から拒否されるため、パース段階で早期に弾き JSON デコード
/// 後の無駄な保持を避ける。
const MAX_N_LEN: usize = 1024;

/// base64url デコード後の `e`（公開指数）の長さ上限（バイト）。
///
/// 一般的な公開指数（65537 = 3 byte）を大きく超える異常値は鍵として扱わない。
const MAX_E_LEN: usize = 8;

/// JWKS パース・検証の失敗理由。
///
/// フェイルクローズ契約（.claude/rules/security.md A01）: 本エラーを返した
/// 呼び出しは [`JwksKeySet`] を一切構築しない。[`crate::gate::TenantGate`] は
/// 注入時点でのパース失敗を「鍵セットなし」（空 [`JwksKeySet`]）として扱わず
/// 呼び出し元へ伝播させ、誤って全リクエストを許可する経路を作らない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwksError {
    /// ドキュメント長が上限（`MAX_DOC_LEN`、64 KiB）を超過した。
    DocumentTooLarge,
    /// トップレベル JSON 構造（`{"keys": [...]}`）が不正。
    InvalidJson,
    /// 採用可能な鍵の件数が上限（`MAX_KEYS`、32 件）を超過した。
    TooManyKeys,
    /// `n` または `e` の base64url デコードに失敗した。
    InvalidKeyEncoding,
    /// デコード後の `n` または `e` が長さ上限を超過した。
    KeyTooLarge,
    /// 同一 `kid` を持つ鍵が複数存在する（鍵選択の一意性が壊れるため拒否）。
    DuplicateKeyId,
}

/// JWKS JSON のトップレベル構造（[RFC 7517] Section 5）。
#[derive(Deserialize)]
struct JwksDoc<'a> {
    #[serde(borrow)]
    keys: Vec<RawJwk<'a>>,
}

/// JWKS の 1 エントリの生の JSON 表現（[RFC 7517] Section 4 / [RFC 7518] Section 6.3.1）。
#[derive(Deserialize)]
struct RawJwk<'a> {
    kty: &'a str,
    kid: Option<&'a str>,
    n: Option<&'a str>,
    e: Option<&'a str>,
    #[serde(rename = "use")]
    use_: Option<&'a str>,
    alg: Option<&'a str>,
}

/// 注入された JWKS から導出した、RS256 検証に即座に使える鍵集合。
///
/// 空の鍵集合（`keys: []` を含むドキュメント、または本来の JWKS 未注入）は
/// フェイルクローズの前提（.claude/rules/security.md A01）に従い、
/// [`crate::gate::TenantGate`] の `RequestGate::check` 実装側であらゆる
/// リクエストを拒否する（フェイルオープンにしない）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JwksKeySet {
    keys: Vec<RsaJwkPublic>,
}

/// [`JwksKeySet`] が保持する公開鍵エントリの公開版（`n`/`e` を参照可能にする）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct RsaJwkPublic {
    kid: String,
    n: Vec<u8>,
    e: Vec<u8>,
}

impl JwksKeySet {
    /// JWKS JSON ドキュメントをパースする。
    ///
    /// 検証順序: ドキュメント長上限 → JSON 構造 → 各エントリの `kty`/`use`/
    /// `alg` 適格性・`kid` 必須・`n`/`e` の base64url デコードと長さ上限 →
    /// 件数上限 → `kid` 重複検査。いずれかに失敗すればドキュメント全体を
    /// 拒否する（部分採用しない、fail-closed）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::jwks::JwksKeySet;
    ///
    /// // 空の `keys` 配列は有効な JWKS だが、鍵ゼロの `JwksKeySet` になる。
    /// let keys = JwksKeySet::from_json(r#"{"keys":[]}"#).expect("valid empty jwks");
    /// assert!(keys.is_empty());
    /// ```
    pub fn from_json(json: &str) -> Result<Self, JwksError> {
        if json.len() > MAX_DOC_LEN {
            return Err(JwksError::DocumentTooLarge);
        }

        let doc: JwksDoc = serde_json::from_str(json).map_err(|_| JwksError::InvalidJson)?;

        let mut keys: Vec<RsaJwkPublic> = Vec::with_capacity(doc.keys.len());
        for raw in &doc.keys {
            // RSA 以外・署名用途以外・RS256 以外の鍵は採用しない
            // （アルゴリズム混同・鍵誤用の防止、.claude/rules/security.md A05）。
            if raw.kty != "RSA" {
                continue;
            }
            if let Some(use_) = raw.use_
                && use_ != "sig"
            {
                continue;
            }
            if let Some(alg) = raw.alg
                && alg != "RS256"
            {
                continue;
            }

            let kid = raw.kid.ok_or(JwksError::InvalidJson)?;
            let n_b64 = raw.n.ok_or(JwksError::InvalidJson)?;
            let e_b64 = raw.e.ok_or(JwksError::InvalidJson)?;

            let n = decode_key_component(n_b64, false)?;
            let e = decode_key_component(e_b64, true)?;

            keys.push(RsaJwkPublic {
                kid: kid.to_string(),
                n,
                e,
            });
        }

        // 件数上限は「採用済み（RSA/sig/RS256 でフィルタ後）」の鍵集合に対して
        // 適用する（doc comment・Bugbot 指摘対応）。非 RSA・非 sig 等のエントリを
        // 大量に含む JWKS ドキュメントでも、実際に採用される RS256 鍵が
        // MAX_KEYS 未満であれば TooManyKeys を誤って返さない。
        if keys.len() > MAX_KEYS {
            return Err(JwksError::TooManyKeys);
        }

        // 重複 kid はここで検出する（`find_by_kid` が先頭一致を返すだけだと
        // 「どちらの鍵が使われるか」が JSON の並び順に依存する曖昧さを生む
        // ため、曖昧な状態そのものを拒否する）。
        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                if keys[i].kid == keys[j].kid {
                    return Err(JwksError::DuplicateKeyId);
                }
            }
        }

        Ok(Self { keys })
    }

    /// `kid` に一致する鍵の `(n, e)` を返す。見つからなければ `None`
    /// （呼び出し元の [`crate::jwt::verify_token`] は
    /// [`crate::jwt::TokenError::UnknownKeyId`] として扱う）。
    pub(crate) fn find_by_kid(&self, kid: &str) -> Option<(&[u8], &[u8])> {
        self.keys
            .iter()
            .find(|k| k.kid == kid)
            .map(|k| (k.n.as_slice(), k.e.as_slice()))
    }

    /// 採用済みの鍵件数。利用側サービスが JWKS 注入直後に「意図した件数の
    /// 鍵が採用されたか」を確認する用途（設定ミス検知）を想定する。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::jwks::JwksKeySet;
    ///
    /// let keys = JwksKeySet::from_json(r#"{"keys":[]}"#).expect("valid empty jwks");
    /// assert_eq!(keys.len(), 0);
    /// assert!(keys.is_empty());
    /// ```
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// [`Self::len`] が 0 かどうか。
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// base64url（パディングなし）デコード + 先頭ゼロバイト除去 + 長さ上限検査。
///
/// `ring::signature::RsaPublicKeyComponents` は `n`/`e` を
/// 「先頭ゼロバイトなしのビッグエンディアン」として要求し、先頭ゼロバイトが
/// 残っていると鍵として拒否される（ring 内部の `io::Positive::from_be_bytes`）。
/// JWK の `n`/`e`（[RFC 7518] Section 6.3.1）はビッグエンディアンの base64url
/// であり先頭ゼロの禁止までは規定しないため、ここで明示的に剥がす。
///
/// `is_exponent` で `n`（モジュラス、上限 [`MAX_N_LEN`]）と `e`（公開指数、
/// 上限 [`MAX_E_LEN`]）のどちらの上限を適用するかを呼び出し側が明示する。
/// 過去に両者へ共通の緩い上限（`MAX_N_LEN.max(MAX_E_LEN)`）を使っていたため、
/// `e` に最大 1024 byte（`MAX_N_LEN` 相当）まで意図せず通る余地があった。
fn decode_key_component(b64: &str, is_exponent: bool) -> Result<Vec<u8>, JwksError> {
    let raw = URL_SAFE_NO_PAD
        .decode(b64)
        .map_err(|_| JwksError::InvalidKeyEncoding)?;
    // スライスの先頭から非ゼロバイトまでを skip する。全バイトがゼロ
    // （鍵として無効）の場合は空スライスになり、以降の長さ 0 チェックで
    // 弾かれる想定はないが ring 側の `from_be_bytes` が空入力を拒否するため
    // 結果的に検証時エラーとして安全側に倒れる。
    let stripped: Vec<u8> = {
        let first_nonzero = raw.iter().position(|&b| b != 0);
        match first_nonzero {
            Some(idx) => raw[idx..].to_vec(),
            None => Vec::new(),
        }
    };
    let max_len = if is_exponent { MAX_E_LEN } else { MAX_N_LEN };
    if stripped.len() > max_len {
        return Err(JwksError::KeyTooLarge);
    }
    Ok(stripped)
}

/// [`JwksKeySet`] の再起動なしローテーションを可能にするハンドル。
///
/// `Arc<RwLock<Arc<JwksKeySet>>>` の 2 段構成: 外側の `RwLock` は
/// [`SharedJwks::set`] による差し替え（書き込み）と
/// [`SharedJwks::snapshot`] による現行鍵集合の取得（読み取り）を仲介する。
/// `snapshot` が返すのは内側 `Arc<JwksKeySet>` の clone（参照カウント増分のみ）
/// のため、[`crate::gate::TenantGate`] の `RequestGate::check` 実装はロックを
/// 短時間保持するだけで済み、取得後の検証処理中はロックを保持しない
/// （ロック保持中の `.await` を避ける契約、.claude/rules/coding-rust.md。
/// ただし `check` 自体が同期関数のため `.await` は元より発生しない）。
#[derive(Debug, Clone)]
pub struct SharedJwks(Arc<RwLock<Arc<JwksKeySet>>>);

impl SharedJwks {
    /// 初期鍵集合を保持する [`SharedJwks`] を作る。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::jwks::{JwksKeySet, SharedJwks};
    ///
    /// let initial = JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap();
    /// let shared = SharedJwks::new(initial);
    /// assert!(shared.snapshot().is_empty());
    /// ```
    pub fn new(initial: JwksKeySet) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(initial))))
    }

    /// JWKS JSON ドキュメントをパースして [`SharedJwks::new`] する便宜関数。
    ///
    /// # Errors
    ///
    /// パースに失敗した場合は [`JwksError`] を返す（フェイルクローズ:
    /// 呼び出し元は失敗時に「鍵なし」へフォールバックしてはならない。
    /// パース失敗は設定ミスであり、サービス起動自体を止めるべき事象である）。
    pub fn from_json(json: &str) -> Result<Self, JwksError> {
        Ok(Self::new(JwksKeySet::from_json(json)?))
    }

    /// 現行鍵集合の `Arc` clone を返す（読み取りロックを短時間保持するのみ）。
    ///
    /// `RwLock` が汚染（poisoned）された場合でも `into_inner()` で内部値を
    /// 取り出し、panic させずに読み取りを継続する（意図的な回復戦略）。
    /// 本ロックが保護するのは `Arc<JwksKeySet>` の差し替えのみで、汚染時に
    /// 中途半端な書き込み状態が残ることはない（[`Self::set`] は
    /// `Arc` の再代入のみで途中状態を持たない）ため、汚染後も安全に読み取れる。
    /// ここで panic させると 1 リクエストの panic が全リクエストの認証経路へ
    /// 連鎖する DoS を招くため、`.unwrap()` へは変更しないこと。
    pub fn snapshot(&self) -> Arc<JwksKeySet> {
        Arc::clone(&self.0.read().unwrap_or_else(|e| e.into_inner()))
    }

    /// 鍵集合を差し替える（鍵ローテーション）。既存の [`Self::snapshot`] 済み
    /// 参照は古い鍵集合を指したまま有効であり続ける（`Arc` による世代分離）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::jwks::{JwksKeySet, SharedJwks};
    ///
    /// let shared = SharedJwks::new(JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap());
    /// shared.set(JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap());
    /// ```
    pub fn set(&self, next: JwksKeySet) {
        let mut guard = self.0.write().unwrap_or_else(|e| e.into_inner());
        *guard = Arc::new(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwk_json(kid: &str, n_b64: &str, e_b64: &str, use_: &str, alg: &str) -> String {
        format!(
            r#"{{"keys":[{{"kty":"RSA","kid":"{kid}","n":"{n_b64}","e":"{e_b64}","use":"{use_}","alg":"{alg}"}}]}}"#
        )
    }

    #[test]
    fn empty_keys_is_valid() {
        let keys = JwksKeySet::from_json(r#"{"keys":[]}"#).unwrap();
        assert!(keys.find_by_kid("k1").is_none());
    }

    #[test]
    fn oversized_document_is_rejected() {
        let huge = format!(r#"{{"keys":[],"padding":"{}"}}"#, "a".repeat(MAX_DOC_LEN));
        assert_eq!(
            JwksKeySet::from_json(&huge),
            Err(JwksError::DocumentTooLarge)
        );
    }

    #[test]
    fn invalid_json_is_rejected() {
        assert_eq!(
            JwksKeySet::from_json("not json"),
            Err(JwksError::InvalidJson)
        );
    }

    #[test]
    fn missing_keys_field_is_rejected() {
        assert_eq!(JwksKeySet::from_json("{}"), Err(JwksError::InvalidJson));
    }

    #[test]
    fn valid_rsa_key_is_found_by_kid() {
        let n = URL_SAFE_NO_PAD.encode([1u8, 2, 3]);
        let e = URL_SAFE_NO_PAD.encode([1u8, 0, 1]);
        let json = jwk_json("kid-1", &n, &e, "sig", "RS256");
        let keys = JwksKeySet::from_json(&json).unwrap();
        let (found_n, found_e) = keys.find_by_kid("kid-1").expect("kid-1 present");
        assert_eq!(found_n, &[1u8, 2, 3][..]);
        assert_eq!(found_e, &[1u8, 0, 1][..]);
        assert!(keys.find_by_kid("kid-2").is_none());
    }

    #[test]
    fn leading_zero_bytes_are_stripped() {
        let n = URL_SAFE_NO_PAD.encode([0u8, 0, 1, 2, 3]);
        let e = URL_SAFE_NO_PAD.encode([0u8, 1, 0, 1]);
        let json = jwk_json("kid-1", &n, &e, "sig", "RS256");
        let keys = JwksKeySet::from_json(&json).unwrap();
        let (found_n, found_e) = keys.find_by_kid("kid-1").unwrap();
        assert_eq!(found_n, &[1u8, 2, 3][..]);
        assert_eq!(found_e, &[1u8, 0, 1][..]);
    }

    #[test]
    fn non_rsa_kty_is_skipped() {
        let json = r#"{"keys":[{"kty":"EC","kid":"kid-1","n":"AQ","e":"AQ"}]}"#;
        let keys = JwksKeySet::from_json(json).unwrap();
        assert!(keys.find_by_kid("kid-1").is_none());
    }

    #[test]
    fn use_other_than_sig_is_skipped() {
        let n = URL_SAFE_NO_PAD.encode([1u8, 2, 3]);
        let e = URL_SAFE_NO_PAD.encode([1u8, 0, 1]);
        let json = jwk_json("kid-1", &n, &e, "enc", "RS256");
        let keys = JwksKeySet::from_json(&json).unwrap();
        assert!(keys.find_by_kid("kid-1").is_none());
    }

    #[test]
    fn alg_other_than_rs256_is_skipped() {
        let n = URL_SAFE_NO_PAD.encode([1u8, 2, 3]);
        let e = URL_SAFE_NO_PAD.encode([1u8, 0, 1]);
        let json = jwk_json("kid-1", &n, &e, "sig", "RS384");
        let keys = JwksKeySet::from_json(&json).unwrap();
        assert!(keys.find_by_kid("kid-1").is_none());
    }

    #[test]
    fn missing_kid_is_rejected() {
        let json = r#"{"keys":[{"kty":"RSA","n":"AQ","e":"AQ"}]}"#;
        assert_eq!(JwksKeySet::from_json(json), Err(JwksError::InvalidJson));
    }

    #[test]
    fn invalid_base64_is_rejected() {
        let json = r#"{"keys":[{"kty":"RSA","kid":"k","n":"!!!","e":"AQ"}]}"#;
        assert_eq!(
            JwksKeySet::from_json(json),
            Err(JwksError::InvalidKeyEncoding)
        );
    }

    #[test]
    fn oversized_modulus_is_rejected() {
        let n = URL_SAFE_NO_PAD.encode(vec![1u8; MAX_N_LEN + 1]);
        let e = URL_SAFE_NO_PAD.encode([1u8, 0, 1]);
        let json = jwk_json("kid-1", &n, &e, "sig", "RS256");
        assert_eq!(JwksKeySet::from_json(&json), Err(JwksError::KeyTooLarge));
    }

    #[test]
    fn too_many_keys_is_rejected() {
        let n = URL_SAFE_NO_PAD.encode([1u8, 2, 3]);
        let e = URL_SAFE_NO_PAD.encode([1u8, 0, 1]);
        let mut entries = Vec::new();
        for i in 0..(MAX_KEYS + 1) {
            entries.push(format!(
                r#"{{"kty":"RSA","kid":"k{i}","n":"{n}","e":"{e}"}}"#
            ));
        }
        let json = format!(r#"{{"keys":[{}]}}"#, entries.join(","));
        assert_eq!(JwksKeySet::from_json(&json), Err(JwksError::TooManyKeys));
    }

    /// 件数上限はフィルタ後（採用済み RS256 鍵集合）に適用される
    /// （Bugbot 指摘対応、PR #158）。生の `keys` 配列が `MAX_KEYS` を超えても、
    /// 非 RSA・非 `sig` エントリで大半が除外され採用鍵が上限未満なら
    /// `TooManyKeys` を誤って返さない。
    #[test]
    fn many_non_rsa_entries_do_not_trigger_too_many_keys() {
        let n = URL_SAFE_NO_PAD.encode([1u8, 2, 3]);
        let e = URL_SAFE_NO_PAD.encode([1u8, 0, 1]);
        let mut entries = Vec::new();
        // 非 RSA（EC）エントリを MAX_KEYS を超える件数だけ生の keys 配列に積む。
        // これらはフィルタで除外されるため採用鍵数には数えない。
        for i in 0..(MAX_KEYS + 8) {
            entries.push(format!(r#"{{"kty":"EC","kid":"ec-{i}"}}"#));
        }
        // 採用対象の RS256 鍵は MAX_KEYS 未満の件数のみ混在させる。
        for i in 0..3 {
            entries.push(format!(
                r#"{{"kty":"RSA","kid":"rsa-{i}","n":"{n}","e":"{e}","use":"sig","alg":"RS256"}}"#
            ));
        }
        let json = format!(r#"{{"keys":[{}]}}"#, entries.join(","));
        let keys = JwksKeySet::from_json(&json).expect("adopted key count is under MAX_KEYS");
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn duplicate_kid_is_rejected() {
        let n = URL_SAFE_NO_PAD.encode([1u8, 2, 3]);
        let e = URL_SAFE_NO_PAD.encode([1u8, 0, 1]);
        let json = format!(
            r#"{{"keys":[{{"kty":"RSA","kid":"dup","n":"{n}","e":"{e}"}},{{"kty":"RSA","kid":"dup","n":"{n}","e":"{e}"}}]}}"#
        );
        assert_eq!(JwksKeySet::from_json(&json), Err(JwksError::DuplicateKeyId));
    }

    #[test]
    fn shared_jwks_rotation_replaces_snapshot_source() {
        let n1 = URL_SAFE_NO_PAD.encode([1u8, 2, 3]);
        let e1 = URL_SAFE_NO_PAD.encode([1u8, 0, 1]);
        let n2 = URL_SAFE_NO_PAD.encode([4u8, 5, 6]);
        let e2 = URL_SAFE_NO_PAD.encode([1u8, 0, 1]);

        let shared = SharedJwks::from_json(&jwk_json("kid-1", &n1, &e1, "sig", "RS256")).unwrap();
        assert!(shared.snapshot().find_by_kid("kid-1").is_some());
        assert!(shared.snapshot().find_by_kid("kid-2").is_none());

        shared.set(JwksKeySet::from_json(&jwk_json("kid-2", &n2, &e2, "sig", "RS256")).unwrap());
        assert!(shared.snapshot().find_by_kid("kid-1").is_none());
        assert!(shared.snapshot().find_by_kid("kid-2").is_some());
    }
}
