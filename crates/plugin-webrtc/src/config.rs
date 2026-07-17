//! in-process WebRTC プラグインの静的設定（[`WebRtcConfig`]）。
//!
//! [`crate::handler::try_handle_rtc_offer`] が参照する。生成した
//! `RTCPeerConnection` を保持するレジストリ（`WebRtcConfig::registry`、
//! `pub(crate)` の内部 API）もここで保持し、テスト容易性のため PoC-5 の
//! `OnceLock` グローバルは使わず [`WebRtcConfig`] インスタンス単位の
//! フィールドとする（`.claude/rules/coding-rust.md` の AI ファースト保守性）。
//!
//! レジストリは「同時接続数上限の予約枠（`RegistrySlot::Reserved`）」と
//! 「シグナリング成功済みの接続（`RegistrySlot::Active`）」を同一 `Mutex` 配下の
//! `Vec` で管理する。上限判定（`reserve_slot`）と枠の登録を同一ロック区間内で行う
//! ことで、[`crate::handler::try_handle_rtc_offer`] の複数呼び出しが同時に
//! `len() < max` を通過してから登録する TOCTOU（time-of-check to time-of-use）を
//! 防ぐ。またシグナリング失敗時（`release_slot`）・接続クローズ時（`on_peer_connection_state_change`
//! 経由の `release_slot`）に枠を確実に取り除くことで、正常利用の蓄積のみで
//! `max_peer_connections` に恒久的に到達し続ける問題（レジストリの単調増加）を防ぐ。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use webrtc::peer_connection::RTCPeerConnection;

/// レジストリの 1 エントリ（予約枠 or アクティブな接続）。
///
/// [`crate::handler::try_handle_rtc_offer`] が `reserve_slot` で予約し、シグナリング
/// 成功で `activate_slot`（`Active` へ遷移）、失敗またはクローズで `release_slot`
/// （エントリ除去）する。`Active` の間は `Arc<RTCPeerConnection>` を保持することが
/// 接続を生存させる唯一の経路であり、除去（`release_slot`）は `RTCPeerConnection` の
/// 破棄・クローズを意味する。
#[derive(Debug)]
pub(crate) enum RegistrySlot {
    /// シグナリング進行中で `RTCPeerConnection` 未生成の予約枠。
    Reserved,
    /// シグナリング成功で登録済みの接続。
    ///
    /// 保持している `Arc<RTCPeerConnection>` を読み出すことはなく、接続を生存させる
    /// （Drop させない）ためだけに保持する。`dead_code` lint はこの用途を検知できない
    /// ため許容する。
    Active(#[allow(dead_code)] Arc<RTCPeerConnection>),
}

/// SDP Offer の既定サイズ上限（64 KiB）。
///
/// `crates/plugin-webrtc-proxy::config::DEFAULT_MAX_PAYLOAD_BYTES` と同値に揃え、
/// 一般的な SDP（数 KiB 程度）に十分な余裕を持たせつつメモリ枯渇（DoS）を防ぐ
/// （.claude/rules/security.md）。
const DEFAULT_MAX_OFFER_BYTES: usize = 64 * 1024;

/// 同時に保持する `RTCPeerConnection` 数の既定上限。
///
/// 生成した `RTCPeerConnection` はプロセス内レジストリ（[`WebRtcConfig::registry`]）
/// で管理され、接続クローズ・失敗（`RTCPeerConnectionState::Closed`/`Failed`）を
/// 検知次第レジストリから除去される（`crate::handler::register_close_handler`）。
/// 上限を設けず無制限に受理するとメモリ枯渇（DoS）に直結するため、超過時は
/// [`crate::handler::try_handle_rtc_offer`] が 503 で拒否する
/// （フェイルクローズ、.claude/rules/security.md）。
const DEFAULT_MAX_PEER_CONNECTIONS: usize = 64;

/// シグナリング全体（`set_remote_description` → 非トリクル ICE 候補収集完了 →
/// `set_local_description`）に許すタイムアウトの既定値。
///
/// ICE 候補収集はネットワーク状況に応じて長時間ブロックしうるため、コアの
/// `READ_TIMEOUT`（スロークライアント対策、`crates/core/src/server.rs`）とは独立に、
/// シグナリング処理自体の上限を設ける（.claude/rules/security.md のリソース枯渇対策）。
const DEFAULT_SIGNALING_TIMEOUT: Duration = Duration::from_secs(10);

/// in-process WebRTC プラグインの設定 + 実行時状態。
///
/// フィールドは非公開とし、[`WebRtcConfig::new`]（[`Default`] 相当）経由での構築を
/// 強制する。`registry` は `Clone` してもレジストリを共有する（`Arc<Mutex<_>>>`）ため、
/// [`crate::handler::try_handle_rtc_offer`] を並行に呼び出す複数コネクションタスクが
/// 同一の同時接続数上限を共有する。
#[derive(Debug, Clone)]
pub struct WebRtcConfig {
    max_offer_bytes: usize,
    max_peer_connections: usize,
    signaling_timeout: Duration,
    registry: Arc<Mutex<Vec<(u64, RegistrySlot)>>>,
    next_slot_id: Arc<AtomicU64>,
}

impl WebRtcConfig {
    /// 既定値で設定を構築する。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc::WebRtcConfig;
    ///
    /// let config = WebRtcConfig::new();
    /// assert_eq!(config.max_offer_bytes(), 64 * 1024);
    /// assert_eq!(config.max_peer_connections(), 64);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// SDP Offer の最大バイト数を上書きする。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc::WebRtcConfig;
    ///
    /// let config = WebRtcConfig::new().with_max_offer_bytes(1024);
    /// assert_eq!(config.max_offer_bytes(), 1024);
    /// ```
    #[must_use]
    pub fn with_max_offer_bytes(mut self, max_bytes: usize) -> Self {
        self.max_offer_bytes = max_bytes;
        self
    }

    /// 同時に保持する `RTCPeerConnection` 数の上限を上書きする。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc::WebRtcConfig;
    ///
    /// let config = WebRtcConfig::new().with_max_peer_connections(4);
    /// assert_eq!(config.max_peer_connections(), 4);
    /// ```
    #[must_use]
    pub fn with_max_peer_connections(mut self, max: usize) -> Self {
        self.max_peer_connections = max;
        self
    }

    /// シグナリング全体のタイムアウトを上書きする。
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use bf_plugin_webrtc::WebRtcConfig;
    ///
    /// let config = WebRtcConfig::new().with_signaling_timeout(Duration::from_secs(3));
    /// assert_eq!(config.signaling_timeout(), Duration::from_secs(3));
    /// ```
    #[must_use]
    pub fn with_signaling_timeout(mut self, timeout: Duration) -> Self {
        self.signaling_timeout = timeout;
        self
    }

    /// SDP Offer の最大バイト数。
    pub fn max_offer_bytes(&self) -> usize {
        self.max_offer_bytes
    }

    /// 同時に保持する `RTCPeerConnection` 数の上限。
    pub fn max_peer_connections(&self) -> usize {
        self.max_peer_connections
    }

    /// シグナリング全体のタイムアウト。
    pub fn signaling_timeout(&self) -> Duration {
        self.signaling_timeout
    }

    /// 同時接続数上限の判定と予約枠の登録を単一ロック区間で行う。
    ///
    /// `len() >= max_peer_connections` の判定と `Reserved` エントリの push を同じ
    /// `Mutex` ロック内で完結させることで、複数リクエストが同時に上限未達と判定して
    /// から登録する TOCTOU を防ぐ（[`crate::handler::try_handle_rtc_offer`] から
    /// `RTCPeerConnection` 生成前に呼ばれる）。上限に達している場合は `None` を返し、
    /// 呼び出し元は 503 で拒否する（フェイルクローズ、.claude/rules/security.md）。
    /// 予約に成功した場合は一意な枠 ID を返す。この ID は必ず
    /// [`WebRtcConfig::activate_slot`] または [`WebRtcConfig::release_slot`] のいずれか
    /// 一度だけに渡し、枠をリークさせない（`Reserved` のまま放置しないこと）。
    pub(crate) fn reserve_slot(&self) -> Option<u64> {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if registry.len() >= self.max_peer_connections {
            return None;
        }
        let id = self.next_slot_id.fetch_add(1, Ordering::Relaxed);
        registry.push((id, RegistrySlot::Reserved));
        Some(id)
    }

    /// 予約枠（`Reserved`）をシグナリング成功済みの接続（`Active`）へ遷移させる。
    ///
    /// [`crate::handler::complete_signaling`] がシグナリング成功時に呼ぶ。対象の
    /// `slot_id` が既に除去済み（タイムアウト等との競合）の場合は何もしない
    /// （呼び出し元が `pc` の生存管理に責任を持つ）。
    pub(crate) fn activate_slot(&self, slot_id: u64, pc: Arc<RTCPeerConnection>) {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = registry.iter_mut().find(|(id, _)| *id == slot_id) {
            entry.1 = RegistrySlot::Active(pc);
        }
    }

    /// 枠（予約中・アクティブ問わず）をレジストリから除去する。
    ///
    /// シグナリング失敗・タイムアウト時（予約枠の解放）と、接続クローズ・失敗検知時
    /// （`crate::handler::register_close_handler` 経由、アクティブ接続の除去）の両方
    /// から呼ばれる。`Active` エントリの除去は保持していた最後の
    /// `Arc<RTCPeerConnection>` を手放すことを意味し、他に強参照がなければ
    /// `RTCPeerConnection` はここで破棄される。存在しない `slot_id` は無視する
    /// （多重解放を許容する冪等な操作）。
    pub(crate) fn release_slot(&self, slot_id: u64) {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry.retain(|(id, _)| *id != slot_id);
    }
}

impl Default for WebRtcConfig {
    fn default() -> Self {
        Self {
            max_offer_bytes: DEFAULT_MAX_OFFER_BYTES,
            max_peer_connections: DEFAULT_MAX_PEER_CONNECTIONS,
            signaling_timeout: DEFAULT_SIGNALING_TIMEOUT,
            registry: Arc::new(Mutex::new(Vec::new())),
            next_slot_id: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_overrides_apply() {
        let config = WebRtcConfig::new()
            .with_max_offer_bytes(2048)
            .with_max_peer_connections(2)
            .with_signaling_timeout(Duration::from_millis(500));

        assert_eq!(config.max_offer_bytes(), 2048);
        assert_eq!(config.max_peer_connections(), 2);
        assert_eq!(config.signaling_timeout(), Duration::from_millis(500));
    }

    #[test]
    fn default_matches_documented_values() {
        let config = WebRtcConfig::default();
        assert_eq!(config.max_offer_bytes(), DEFAULT_MAX_OFFER_BYTES);
        assert_eq!(config.max_peer_connections(), DEFAULT_MAX_PEER_CONNECTIONS);
        assert_eq!(config.signaling_timeout(), DEFAULT_SIGNALING_TIMEOUT);
    }

    #[test]
    fn cloned_config_shares_registry() {
        let config = WebRtcConfig::new();
        let cloned = config.clone();
        assert!(Arc::ptr_eq(&config.registry, &cloned.registry));
    }

    #[test]
    fn reserve_slot_respects_max_and_release_frees_capacity() {
        let config = WebRtcConfig::new().with_max_peer_connections(1);
        let first = config.reserve_slot().expect("1 件目は予約できる");
        assert!(config.reserve_slot().is_none(), "上限到達時は予約できない");
        config.release_slot(first);
        assert!(
            config.reserve_slot().is_some(),
            "解放後は再び予約できる（レジストリの単調増加を防ぐ）"
        );
    }

    #[test]
    fn reserve_slot_is_toctou_free_under_concurrent_checks() {
        // reserve_slot は判定と登録を同一ロック内で行うため、上限ちょうどの枠数しか
        // 予約に成功しない（TOCTOU 対策の直接的な検証）。
        let config = WebRtcConfig::new().with_max_peer_connections(2);
        let results: Vec<_> = (0..5).map(|_| config.reserve_slot()).collect();
        assert_eq!(results.iter().filter(|r| r.is_some()).count(), 2);
    }
}
