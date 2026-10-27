//! DPoP proof の `jti` の再使用検知（`dpop` feature、イシュー #903）。
//!
//! 役割: [`verify_proof`](super::verify_proof) は純関数で状態を持たないため、写した proof を
//! `iat` の窓の中で再送する使い回しを防げない。[`ReplayStore`] は「鍵のサムプリントと `jti` の
//! 組」を `iat` の窓の間だけ有界に保持し、2 回目以降を拒否する。ゲート（`JwtAuthenticator`）
//! ごとに 1 つ持たせる前提の部品で、配線は #904 が担う。設計は
//! `docs/design/jwt-verify-plugin.md` 9.6 節。
//!
//! 同期・I/O なしの契約（NFR-7）を保つ。ロックの保持区間は `HashMap`/`BinaryHeap` の操作だけで、
//! 非同期待機・I/O・署名検証・ハッシュ計算を含まない。本モジュールは tokio に依存しない。

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fmt;
use std::sync::Mutex;

use ring::digest::{Context, SHA256};

use super::{DpopConfigError, DpopError, IatWindow, VerifiedProof};

/// 保持数の既定の上限。既定の窓（65 秒）で約 2,000 rps に相当する。
pub const DEFAULT_REPLAY_CAPACITY: usize = 131_072;
/// 保持数の上限として指定できる最大値。
pub const MAX_REPLAY_CAPACITY: usize = 4_194_304;

type Key = [u8; 32];

#[derive(Default)]
struct ReplayState {
    /// キー → 期限（UNIX 秒）。
    map: HashMap<Key, u64>,
    /// 期限の昇順で取り出すための索引。`map` と要素が 1 対 1 に対応する。
    heap: BinaryHeap<Reverse<(u64, Key)>>,
    /// ハンドラが受け取り済み（`claim`）のキー。`map` の部分集合で、期限切れと同時に除く。
    claimed: HashSet<Key>,
}

/// `jti` の保持器。鍵のサムプリントと `jti` の組を `iat` の窓の間だけ保持する。
///
/// - **保持の単位**: サムプリントと `jti` の組の SHA-256（生の `jti` は保持しない）。
/// - **期限**: `iat + past`。`verify_proof` は `iat + past >= now` の間 proof を受理するので、
///   期限は `now > iat + past` になって初めて過ぎたと扱う。未来側の `iat` により最長
///   `past + future` 秒保持しうる。
/// - **満杯時**: 期限切れを除いてもなお上限に達していれば、新しい proof を
///   [`DpopError::ReplayStoreFull`] で拒否する（フェイルクローズ）。
/// - **容量の目安**: 「ピーク時の DPoP の rps × (past + future)」。既定 131,072 は既定の窓
///   （過去 60 秒 + 未来 5 秒）で約 2,000 rps に相当する。
/// - **メモリ**: 1 項目あたり、`HashMap` に鍵 32 B + 値 8 B（制御バイトと負荷率の余裕を含め
///   約 50〜90 B）、`BinaryHeap` に 40 B（`Vec` の伸長の余裕で最大約 2 倍）。保持数が減っても
///   確保済みの容量は縮めない（高水位のまま）。上限分は事前に確保しない。
/// - **掃除**: `register` のたびに期限切れを先頭から取り除く。各項目は 1 度しか取り出されないため
///   償却では 1 項目あたり O(log n) だが、長い無通信の後の最初の呼び出しだけ取り出しが多くなる。
///
/// # 既知の限界
///
/// `jti` の一度だけの検査はゲートごと・プロセスごとで、複数インスタンス間では共有しない。
/// 写した proof を `iat` の窓の中で別インスタンスへ送る使い回しは防げないため、窓を短くして
/// 余地を限ること。アクセストークンだけの写しは、鍵を持たない者には proof を作れないため使えない。
///
/// # 呼び出しの契約
///
/// - 構築時に渡す [`IatWindow`] と [`register`](Self::register) の `now_unix` は、同じ要求で
///   `verify_proof` に渡したものと揃えること。食い違うと再使用の隙間が開く。
/// - `register` は 1 要求につき 1 回、すべての検査が通った最後に呼ぶ。同じ要求で 2 回呼ぶと
///   2 回目は [`DpopError::Replayed`] になる。
/// - 確認と登録は同じロック区間で原子的に行う。`contains` と `insert` を分けた入口は持たない。
///
/// ```
/// use base64::Engine;
/// use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
/// use fandhe_backend_plugin_jwt::dpop::{
///     DpopError, IatWindow, ProofContext, PublicOrigin, ReplayStore, verify_proof,
/// };
/// use fandhe_backend_plugin_jwt::{Algorithm, AllowedAlgorithms};
/// use ring::digest::{SHA256, digest};
/// use ring::rand::SystemRandom;
/// use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
///
/// let rng = SystemRandom::new();
/// let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
/// let kp = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
///     .unwrap();
/// let pt = kp.public_key().as_ref();
/// let header = format!(
///     r#"{{"typ":"dpop+jwt","alg":"ES256","jwk":{{"kty":"EC","crv":"P-256","x":"{}","y":"{}"}}}}"#,
///     B.encode(&pt[1..33]),
///     B.encode(&pt[33..65])
/// );
/// let ath = B.encode(digest(&SHA256, b"access-token").as_ref());
/// let payload = format!(
///     r#"{{"jti":"j1","htm":"GET","htu":"https://api.example/resource","iat":1000,"ath":"{ath}"}}"#
/// );
/// let msg = format!("{}.{}", B.encode(header), B.encode(payload));
/// let sig = kp.sign(&rng, msg.as_bytes()).unwrap();
/// let proof = format!("{msg}.{}", B.encode(sig.as_ref()));
///
/// let htu = PublicOrigin::new("https://api.example")?.expected_htu("/resource")?;
/// let algs = AllowedAlgorithms::only(Algorithm::Es256);
/// let window = IatWindow::default();
/// let ctx = ProofContext::new("GET", &htu, "access-token", &algs, window);
/// let store = ReplayStore::new(window);
///
/// let verified = verify_proof(&proof, &ctx, 1010)?;
/// assert_eq!(store.register(&verified, 1010), Ok(()));
/// // 窓の中で同じ proof を再送すると拒否される。
/// let again = verify_proof(&proof, &ctx, 1020)?;
/// assert_eq!(store.register(&again, 1020), Err(DpopError::Replayed));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct ReplayStore {
    state: Mutex<ReplayState>,
    past_secs: u64,
    capacity: usize,
}

impl ReplayStore {
    /// 既定の上限（[`DEFAULT_REPLAY_CAPACITY`]）で構築する。`window` は `ProofContext` に渡す
    /// ものと同じ値にすること。
    pub fn new(window: IatWindow) -> Self {
        Self {
            state: Mutex::new(ReplayState::default()),
            past_secs: window.past().as_secs(),
            capacity: DEFAULT_REPLAY_CAPACITY,
        }
    }

    /// 上限を指定して構築する。0 は [`DpopConfigError::ReplayCapacityZero`]、
    /// [`MAX_REPLAY_CAPACITY`] 超は [`DpopConfigError::ReplayCapacityTooLarge`]。
    ///
    /// ```
    /// use fandhe_backend_plugin_jwt::dpop::{DpopConfigError, IatWindow, ReplayStore};
    /// assert!(ReplayStore::with_capacity(IatWindow::default(), 1000).is_ok());
    /// assert_eq!(
    ///     ReplayStore::with_capacity(IatWindow::default(), 0).unwrap_err(),
    ///     DpopConfigError::ReplayCapacityZero
    /// );
    /// ```
    pub fn with_capacity(window: IatWindow, capacity: usize) -> Result<Self, DpopConfigError> {
        if capacity == 0 {
            return Err(DpopConfigError::ReplayCapacityZero);
        }
        if capacity > MAX_REPLAY_CAPACITY {
            return Err(DpopConfigError::ReplayCapacityTooLarge);
        }
        Ok(Self {
            capacity,
            ..Self::new(window)
        })
    }

    /// 検証済みの proof を登録する。確認と登録を原子的に行う唯一の入口。
    ///
    /// 同じ組が窓の中にあれば [`DpopError::Replayed`]、期限切れを除いても上限に達していれば
    /// [`DpopError::ReplayStoreFull`]。拒否した proof は登録しない。`VerifiedProof` だけを
    /// 受けるので、署名のない入力で保持を埋めることはできない。
    pub fn register(&self, proof: &VerifiedProof, now_unix: u64) -> Result<(), DpopError> {
        // ハッシュ計算はロックの外で行う。
        let key = replay_key(proof.thumbprint().as_str(), proof.jti());
        self.register_key(key, proof.iat(), now_unix)
    }

    fn register_key(&self, key: Key, iat: u64, now_unix: u64) -> Result<(), DpopError> {
        let expiry = iat.saturating_add(self.past_secs);
        // ロック区間: コレクション操作のみ（非同期待機・I/O・署名検証・ハッシュ計算なし）。
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *g;
        // `expiry < now` になった項目だけ除く（`verify_proof` は `iat + past >= now` で受理）。
        while let Some(&Reverse((exp, k))) = st.heap.peek() {
            if exp >= now_unix {
                break;
            }
            st.heap.pop();
            if st.map.get(&k) == Some(&exp) {
                st.map.remove(&k);
                st.claimed.remove(&k);
            }
        }
        if st.map.contains_key(&key) {
            return Err(DpopError::Replayed);
        }
        if st.map.len() >= self.capacity {
            return Err(DpopError::ReplayStoreFull);
        }
        st.heap.push(Reverse((expiry, key)));
        st.map.insert(key, expiry);
        Ok(())
    }

    /// ゲートが登録した証明を、ハンドラが一度だけ受け取る。`authenticate_after_gate` 専用。
    ///
    /// 期限内に登録済みで、まだ受け取られていない場合に限り `true` を返し、同じロック区間で
    /// 受け取り済みに印を付ける。2 回目以降は `false`。これにより、ゲートが受理した proof を
    /// ゲートを通らない経路へ再送しても、ハンドラの検査は一度も通らない（登録だけでは認証成功に
    /// ならない）。公開 API にはせず、確認と登録を原子的に行う唯一の入口は `register` のまま。
    pub(crate) fn claim(&self, proof: &VerifiedProof, now_unix: u64) -> bool {
        // ハッシュ計算はロックの外で行う。
        let key = replay_key(proof.thumbprint().as_str(), proof.jti());
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let live = g.map.get(&key).is_some_and(|&exp| exp >= now_unix);
        live && g.claimed.insert(key)
    }

    /// 現在の保持数。
    pub fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map
            .len()
    }

    /// 保持が空か。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 保持数の上限。
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// `jti`・サムプリントは出さず、保持数と上限だけを出す。
impl fmt::Debug for ReplayStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplayStore")
            .field("len", &self.len())
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

/// SHA-256(サムプリント ‖ `jti`)。サムプリントは SHA-256 の base64url で常に 43 文字の固定長の
/// ため、区切りなしの連結でも組の曖昧さは生じない。
fn replay_key(thumbprint: &str, jti: &str) -> Key {
    let mut c = Context::new(&SHA256);
    c.update(thumbprint.as_bytes());
    c.update(jti.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(c.finish().as_ref());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn win(past: u64) -> IatWindow {
        IatWindow::new(Duration::from_secs(past), Duration::ZERO).unwrap()
    }

    fn k(n: u32) -> Key {
        replay_key("thumb", &n.to_string())
    }

    fn consistent(s: &ReplayStore) -> bool {
        let g = s.state.lock().unwrap();
        g.map.len() == g.heap.len()
    }

    #[test]
    fn boundary_is_inclusive_of_iat_plus_past() {
        let s = ReplayStore::new(win(60));
        assert_eq!(s.register_key(k(1), 100, 100), Ok(()));
        // now == iat + past: verify_proof はまだ受理するので保持している。
        assert_eq!(s.register_key(k(1), 100, 160), Err(DpopError::Replayed));
        // now == iat + past + 1: 期限切れ。
        assert_eq!(s.register_key(k(1), 100, 161), Ok(()));
    }

    #[test]
    fn never_exceeds_capacity() {
        let s = ReplayStore::with_capacity(win(10), 5).unwrap();
        for i in 0..200u32 {
            let _ = s.register_key(k(i), u64::from(i), u64::from(i));
            assert!(s.len() <= 5);
            assert!(consistent(&s));
        }
    }

    #[test]
    fn rejected_key_is_not_registered() {
        let s = ReplayStore::with_capacity(win(10), 1).unwrap();
        assert_eq!(s.register_key(k(1), 100, 100), Ok(()));
        assert_eq!(
            s.register_key(k(2), 100, 100),
            Err(DpopError::ReplayStoreFull)
        );
        // 期限が過ぎて空きができた後は Replayed ではなく Ok になる。
        assert_eq!(s.register_key(k(2), 200, 200), Ok(()));
        assert_eq!(s.len(), 1);
        assert!(consistent(&s));
    }

    #[test]
    fn extreme_iat_does_not_panic() {
        let s = ReplayStore::new(win(300));
        assert_eq!(s.register_key(k(1), u64::MAX, u64::MAX), Ok(()));
        assert_eq!(
            s.register_key(k(1), u64::MAX, u64::MAX),
            Err(DpopError::Replayed)
        );
    }

    #[test]
    fn debug_hides_contents() {
        let s = ReplayStore::new(win(60));
        s.register_key(k(1), 1, 1).unwrap();
        let d = format!("{s:?}");
        assert!(d.contains("len") && d.contains("capacity"));
    }
}
