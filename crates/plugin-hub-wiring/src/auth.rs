//! JWT 検証結果のリクエストスコープキャッシュ（TASK-9.3 / #63）。
//!
//! [`crate::gate::TenantGate`]（`RequestGate` 拡張点）は判定結果のみを
//! `GateOutcome` としてコアへ返す契約（`crates/core/src/extension.rs` doc）
//! のため、ゲート通過後にハンドラ側で `org_id` 等のクレームが必要な場合、
//! 従来は [`crate::jwt::verify_token`] を再呼び出しするしかなく、1 リクエスト
//! につき RS256 署名検証（RSA-2048、数十 µs 級）が 2 回（ゲート + ハンドラ）
//! 走っていた。本モジュールはこの重複を、検証成功済みトークンのキャッシュで
//! 解消する。
//!
//! [`Authenticator`] を [`TenantGate`](crate::gate::TenantGate) と利用側サービスの
//! ハンドラで **共有** することで、ゲート通過時点の検証でキャッシュが温まり、
//! 同一リクエスト内のハンドラ呼び出しが必ずヒットになる。同一トークンでの
//! 連続リクエスト（大量リクエスト時の検証コスト）にも同様にヒットする。
//!
//! # キャッシュヒットを許す条件（正しさ、.claude/rules/security.md A01）
//!
//! 検証をスキップしても意味論が変わらない場合のみキャッシュヒットを許可する。
//!
//! 1. **鍵ローテーション無効化**: エントリに検証時点の `Arc<JwksKeySet>` を
//!    保持し、読み出し時に現行 [`crate::jwks::SharedJwks::snapshot`] と
//!    `Arc::ptr_eq` 比較する。[`crate::jwks::SharedJwks::set`] は必ず新しい
//!    `Arc` を作るため、ローテーション後は全エントリが自動的にミス扱いになり
//!    再検証される。
//! 2. **`exp` 再判定**: ヒット時にも `claims.exp <= now_unix` を毎回判定し、
//!    期限切れなら [`crate::jwt::TokenError::Expired`] を返しエントリを破棄する
//!    （キャッシュ経由で期限切れトークンを許可しない）。
//! 3. **成功のみキャッシュ**: 検証失敗はキャッシュしない。失敗の再検証コストは
//!    要求元に払わせ、無効トークンの大量投入によるキャッシュ汚染を防ぐ
//!    （DoS 耐性、.claude/rules/security.md）。
//!
//! # シークレット管理（.claude/rules/security.md A02）
//!
//! キャッシュキーはトークン文字列そのものではなく **SHA-256 ハッシュ**
//! （`ring::digest::SHA256`）を用いる。生トークンをキャッシュに保持しない。
//! 保持する値（[`crate::jwt::Claims`]）は `org_id`・`exp` のみで鍵材料・署名は
//! 含まない。

use crate::jwks::{JwksKeySet, SharedJwks};
use crate::jwt::{Claims, TokenError, verify_token};
use bf_http::request::RequestHead;
use ring::digest::{self, SHA256};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// キャッシュに保持するエントリ件数の上限。
///
/// 満杯時は期限切れエントリを優先的に掃き出し、なお満杯なら FIFO で最古の
/// エントリを退避する。上限を設けることでメモリ枯渇（DoS）を防ぐ
/// （.claude/rules/security.md）。
const CACHE_CAPACITY: usize = 1024;

/// RFC 6750 の `Bearer` スキーム名（大文字小文字を区別しない）。
const BEARER_SCHEME: &str = "bearer";

/// `Authorization: Bearer <token>` ヘッダからトークン文字列を抽出する。
///
/// [`crate::gate::TenantGate`] の `RequestGate::check` 実装（RFC 6750 パース処理）から
/// 移設した共用ヘルパー（[`Authenticator::authenticate`] と両方から呼ばれる）。
///
/// # Examples
///
/// ```
/// use bf_http::request::{parse_request_head, ParseOutcome};
/// use bf_plugin_hub_wiring::auth::bearer_token;
///
/// let raw = b"GET / HTTP/1.1\r\nAuthorization: Bearer abc.def.ghi\r\n\r\n";
/// let head = match parse_request_head(raw).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// assert_eq!(bearer_token(&head), Some("abc.def.ghi"));
/// ```
pub fn bearer_token(head: &RequestHead) -> Option<&str> {
    let authorization = head.header("authorization")?;

    // RFC 6750 (`credentials = auth-scheme 1*SP token68`): スキーム名
    // `Bearer` は大文字小文字を区別せず、スキーム名とトークンの間には 1 個
    // 以上の SP が許容される。固定長プレフィックス一致だと「Bearer」の後に
    // スペースが 2 個以上並ぶ正当なヘッダを誤って拒否してしまうため、
    // スキーム名部分とスペース列を明示的に分離して剥がす。
    let scheme_end = authorization.find(' ')?;
    // `find(' ')` は ASCII 空白（1 バイト固定）の位置を返す。ASCII バイトは
    // UTF-8 マルチバイト列の継続バイトとして現れ得ないため、この位置での
    // スライスは常に char 境界であり安全。
    let scheme = &authorization[..scheme_end];
    if !scheme.eq_ignore_ascii_case(BEARER_SCHEME) {
        return None;
    }
    let token = authorization[scheme_end..].trim_start_matches(' ');
    if token.is_empty() {
        return None;
    }
    Some(token)
}

/// キャッシュキー（トークン文字列の SHA-256 ハッシュ）。生トークンを
/// キャッシュに保持しないためのキー変換（.claude/rules/security.md A02）。
type CacheKey = [u8; 32];

fn cache_key(token: &str) -> CacheKey {
    let digest = digest::digest(&SHA256, token.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(digest.as_ref());
    key
}

/// 検証成功済みトークン 1 件分のキャッシュエントリ。
#[derive(Clone)]
struct CacheEntry {
    claims: Claims,
    /// 検証時点で使用した鍵集合。読み出し時に現行 `SharedJwks::snapshot()` と
    /// `Arc::ptr_eq` で比較し、ローテーション後のエントリを無効化する。
    verified_against: Arc<JwksKeySet>,
}

/// 検証成功済み JWT のリクエストスコープ・プロセススコープキャッシュ。
///
/// `RwLock<HashMap<..>>` + FIFO キー順（`VecDeque`）で構成する。ロックは
/// 短時間保持のみで、ロック中に署名検証・`.await` を行わない
/// （`RequestGate::check` の同期・非ブロッキング契約を [`Authenticator`] からも
/// 維持する、.claude/rules/coding-rust.md）。
struct TokenCache {
    entries: RwLock<CacheState>,
    hits: AtomicU64,
    misses: AtomicU64,
}

struct CacheState {
    map: HashMap<CacheKey, CacheEntry>,
    /// 挿入順（FIFO 退避用）。`map` と要素数を常に一致させる。
    order: VecDeque<CacheKey>,
}

impl TokenCache {
    fn new() -> Self {
        Self {
            entries: RwLock::new(CacheState {
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// キャッシュ参照。鍵集合が現行と一致し（`Arc::ptr_eq`）、かつ `exp` が
    /// 未到達の場合のみ `Some` を返す。ローテーション済み・期限切れの
    /// エントリはここで破棄しミス扱いにする。
    fn get(&self, key: &CacheKey, current_keys: &Arc<JwksKeySet>, now_unix: u64) -> Option<Claims> {
        // 読み取りロックのみでヒット判定する（書き込みが必要な破棄は別途
        // 上位の `remove` 呼び出しに任せ、ここでは短時間の読み取りに留める）。
        let guard = self.entries.read().unwrap_or_else(|e| e.into_inner());
        let entry = guard.map.get(key)?;
        if !Arc::ptr_eq(&entry.verified_against, current_keys) {
            return None;
        }
        if entry.claims.exp <= now_unix {
            return None;
        }
        Some(entry.claims.clone())
    }

    /// 無効化されたエントリ（鍵ローテーション後・期限切れ）を明示的に破棄する。
    fn remove(&self, key: &CacheKey) {
        let mut guard = self.entries.write().unwrap_or_else(|e| e.into_inner());
        if guard.map.remove(key).is_some() {
            guard.order.retain(|k| k != key);
        }
    }

    /// 検証成功時のみ呼ばれる挿入（失敗はキャッシュしない、DoS 耐性）。
    /// 容量超過時は期限切れエントリを優先的に掃き出し、なお満杯なら FIFO で
    /// 最古のエントリを退避する（メモリ有界性の保証）。
    fn insert(&self, key: CacheKey, claims: Claims, keys: Arc<JwksKeySet>, now_unix: u64) {
        let mut guard = self.entries.write().unwrap_or_else(|e| e.into_inner());
        if guard.map.contains_key(&key) {
            // 既存キーの上書き（同一トークンの再検証、例: ローテーション後の
            // 再登録）は順序を変えず値のみ差し替える。
            if let Some(entry) = guard.map.get_mut(&key) {
                entry.claims = claims;
                entry.verified_against = keys;
            }
            return;
        }

        if guard.map.len() >= CACHE_CAPACITY {
            // 期限切れエントリを先に掃く（現行鍵集合とのローテーション有無に
            // 関わらず `exp` のみで判定してよい: どのみち再検証時に弾かれる）。
            let expired: Vec<CacheKey> = guard
                .map
                .iter()
                .filter(|(_, e)| e.claims.exp <= now_unix)
                .map(|(k, _)| *k)
                .collect();
            for k in &expired {
                guard.map.remove(k);
            }
            // `guard.map` と `guard.order` は同一構造体の異なるフィールドであり、
            // フィールドを直接分割して借用すれば同時アクセスできる（`self.foo()`
            // のようなメソッド越しだと借用チェッカが分割を追えず失敗するため、
            // フィールドへ直接アクセスする）。
            let CacheState { map, order } = &mut *guard;
            order.retain(|k| map.contains_key(k));

            // なお満杯なら FIFO で最古のエントリを退避する。
            while guard.map.len() >= CACHE_CAPACITY {
                let Some(oldest) = guard.order.pop_front() else {
                    break;
                };
                guard.map.remove(&oldest);
            }
        }

        guard.order.push_back(key);
        guard.map.insert(
            key,
            CacheEntry {
                claims,
                verified_against: keys,
            },
        );
    }

    fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
}

/// JWT 検証をリクエストスコープ・プロセススコープでキャッシュする認証器。
///
/// [`Clone`]（内部は `Arc` 共有）で、[`crate::gate::TenantGate`] と利用側
/// サービスのハンドラで同一インスタンスを共有することを想定する。ゲート
/// （`RequestGate::check`）が最初に `authenticate` を呼びキャッシュを温め、
/// ハンドラ側の呼び出しはそれをヒットとして再利用する。
///
/// [`crate::gate::TenantGateConfig::authenticator`] から取得できる。
///
/// # Examples
///
/// ```
/// use bf_plugin_hub_wiring::gate::TenantGateConfig;
///
/// let config = TenantGateConfig::from_jwks_json(r#"{"keys":[]}"#).unwrap();
/// let authenticator = config.authenticator();
/// assert_eq!(authenticator.cache_hits(), 0);
/// assert_eq!(authenticator.cache_misses(), 0);
/// ```
#[derive(Clone)]
pub struct Authenticator {
    jwks: SharedJwks,
    cache: Arc<TokenCache>,
}

impl std::fmt::Debug for Authenticator {
    // `TokenCache` はトークン由来のハッシュキーとクレームのみを保持し秘密材料は
    // 含まないが、内部構造（`RwLock`/`HashMap` の中身）をそのまま `derive` で
    // 露出させない方針とし、統計値（hits/misses）のみを表示する。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Authenticator")
            .field("cache_hits", &self.cache_hits())
            .field("cache_misses", &self.cache_misses())
            .finish()
    }
}

impl Authenticator {
    /// [`SharedJwks`] ハンドルを共有する `Authenticator` を作る。
    pub(crate) fn new(jwks: SharedJwks) -> Self {
        Self {
            jwks,
            cache: Arc::new(TokenCache::new()),
        }
    }

    /// リクエストヘッダから `Authorization: Bearer` トークンを取り出して
    /// 検証する。キャッシュヒット時は署名検証を行わない。
    ///
    /// 検証順序・エラー種別は [`crate::jwt::verify_token`] と同一（呼び出し元の
    /// 401/403 マッピングは変わらない）。トークン欠落・スキーム不正の場合は
    /// [`crate::jwt::TokenError::MissingToken`] を返す。
    pub fn authenticate(&self, head: &RequestHead) -> Result<Claims, TokenError> {
        // フェイルクローズ: `SystemTime::now()` が UNIX epoch 秒への変換に
        // 失敗した場合（クロック異常）に `0` を渡すと `exp <= now_unix` の
        // 期限切れ判定が常に false になり、あらゆる `exp` を「期限内」として
        // 誤許可してしまう（.claude/rules/security.md フェイルクローズ原則、
        // `crate::gate::TenantGate::check` と同一の根拠）。`u64::MAX` を渡す
        // ことで、正の `exp` を持つトークンは無条件に `Expired` 側へ倒れる。
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(u64::MAX);
        self.authenticate_at(head, now_unix)
    }

    /// [`Self::authenticate`] の時刻注入版（テスト用）。
    fn authenticate_at(&self, head: &RequestHead, now_unix: u64) -> Result<Claims, TokenError> {
        let token = bearer_token(head).ok_or(TokenError::MissingToken)?;
        let key = cache_key(token);
        let current_keys = self.jwks.snapshot();

        if let Some(claims) = self.cache.get(&key, &current_keys, now_unix) {
            self.cache.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(claims);
        }
        // ヒットしなかった理由（未挿入・鍵ローテーション後・期限切れ）を
        // 問わず、無効化済みエントリが残っていれば破棄しておく
        // （次回参照までの無駄な `ptr_eq`/`exp` 再判定コストを避ける）。
        self.cache.remove(&key);
        self.cache.misses.fetch_add(1, Ordering::Relaxed);

        // ミス時のみ実際の署名検証を行う（成功のみキャッシュ、DoS 耐性）。
        let claims = verify_token(token, &current_keys, now_unix)?;
        self.cache
            .insert(key, claims.clone(), current_keys, now_unix);
        Ok(claims)
    }

    /// キャッシュヒット件数（計測・テスト用、`AtomicU64` Relaxed で集計）。
    pub fn cache_hits(&self) -> u64 {
        self.cache.hits()
    }

    /// キャッシュミス件数（計測・テスト用）。
    pub fn cache_misses(&self) -> u64 {
        self.cache.misses()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwks::JwksKeySet;
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use bf_http::request::{ParseOutcome, parse_request_head};
    use ring::rand::SystemRandom;
    use ring::signature::{self, KeyPair, RsaKeyPair, RsaPublicKeyComponents};

    const TEST_KID: &str = "test-kid-1";

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

    fn head_with_bearer(token: &str) -> RequestHead {
        let raw = format!("GET / HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n").into_bytes();
        head_from(&raw)
    }

    #[test]
    fn second_call_with_same_token_is_a_hit_and_returns_same_claims() {
        let keypair = test_keypair();
        let authenticator =
            Authenticator::new(SharedJwks::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap());
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999);
        let head = head_with_bearer(&token);

        let first = authenticator.authenticate_at(&head, 0).unwrap();
        let second = authenticator.authenticate_at(&head, 0).unwrap();

        assert_eq!(first, second);
        assert_eq!(authenticator.cache_hits(), 1);
        assert_eq!(authenticator.cache_misses(), 1);
    }

    #[test]
    fn rotation_invalidates_cache_and_forces_reverification() {
        let keypair = test_keypair();
        let shared = SharedJwks::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let authenticator = Authenticator::new(shared.clone());
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999);
        let head = head_with_bearer(&token);

        authenticator.authenticate_at(&head, 0).unwrap();
        assert_eq!(authenticator.cache_misses(), 1);

        // ローテーション（同一 kid で新しい Arc へ差し替え）。
        shared.set(JwksKeySet::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap());

        // 同一トークンだが `Arc::ptr_eq` が不一致になるため必ずミス→再検証。
        let result = authenticator.authenticate_at(&head, 0).unwrap();
        assert_eq!(result.org_id, "org-1");
        assert_eq!(authenticator.cache_misses(), 2);
        assert_eq!(authenticator.cache_hits(), 0);
    }

    #[test]
    fn rotation_to_different_key_rejects_old_token_without_stale_hit() {
        let keypair = test_keypair();
        let rotated = rotated_keypair();
        let shared = SharedJwks::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap();
        let authenticator = Authenticator::new(shared.clone());
        let old_token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999);
        let head = head_with_bearer(&old_token);

        authenticator.authenticate_at(&head, 0).unwrap();

        shared.set(JwksKeySet::from_json(&jwks_json_for(&rotated, TEST_KID)).unwrap());

        // 旧鍵で署名したトークンは新鍵セットでは検証に失敗する（ミスとして
        // 扱われ、失敗はキャッシュされない = フェイルクローズが維持される）。
        let result = authenticator.authenticate_at(&head, 0);
        assert_eq!(result, Err(TokenError::InvalidSignature));
    }

    #[test]
    fn cached_hit_still_rejects_after_expiry_and_evicts_entry() {
        let keypair = test_keypair();
        let authenticator =
            Authenticator::new(SharedJwks::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap());
        let token = make_token(&keypair, TEST_KID, Some("org-1"), 100);
        let head = head_with_bearer(&token);

        // now=0 では有効期限内。
        authenticator.authenticate_at(&head, 0).unwrap();
        assert_eq!(authenticator.cache_misses(), 1);

        // now=200 では `exp`(100) <= now により期限切れ。ヒット判定は行わず
        // ミス→再検証（`verify_token` 自身も `Expired` を返す）。
        let result = authenticator.authenticate_at(&head, 200);
        assert_eq!(result, Err(TokenError::Expired));
        assert_eq!(authenticator.cache_hits(), 0);
        assert_eq!(authenticator.cache_misses(), 2);
    }

    #[test]
    fn verification_failure_is_not_cached() {
        let keypair = test_keypair();
        let authenticator =
            Authenticator::new(SharedJwks::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap());
        let token = make_token(&keypair, "unknown-kid", Some("org-1"), 9_999_999_999);
        let head = head_with_bearer(&token);

        assert_eq!(
            authenticator.authenticate_at(&head, 0),
            Err(TokenError::UnknownKeyId)
        );
        assert_eq!(
            authenticator.authenticate_at(&head, 0),
            Err(TokenError::UnknownKeyId)
        );
        // 失敗はキャッシュしないため 2 回とも実際の検証（ミス）が走る。
        assert_eq!(authenticator.cache_misses(), 2);
        assert_eq!(authenticator.cache_hits(), 0);
    }

    #[test]
    fn capacity_eviction_keeps_cache_size_bounded() {
        let keypair = test_keypair();
        let authenticator =
            Authenticator::new(SharedJwks::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap());

        for i in 0..(CACHE_CAPACITY + 16) {
            let token = make_token(&keypair, TEST_KID, Some("org-1"), 9_999_999_999 + i as u64);
            let head = head_with_bearer(&token);
            authenticator.authenticate_at(&head, 0).unwrap();
        }

        let guard = authenticator
            .cache
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner());
        assert!(guard.map.len() <= CACHE_CAPACITY);
        assert_eq!(guard.map.len(), guard.order.len());
    }

    #[test]
    fn missing_authorization_header_is_missing_token() {
        let keypair = test_keypair();
        let authenticator =
            Authenticator::new(SharedJwks::from_json(&jwks_json_for(&keypair, TEST_KID)).unwrap());
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        assert_eq!(
            authenticator.authenticate_at(&head, 0),
            Err(TokenError::MissingToken)
        );
    }
}
