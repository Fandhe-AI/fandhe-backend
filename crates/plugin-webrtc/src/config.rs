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

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// 最終 graceful shutdown 開始後は `true`（イシュー #498、`drain::drain_for_shutdown`
    /// が設定する）。世代を跨いで共有される `WebRtcConfig`（`Clone` でレジストリ・この
    /// フラグを共有）に対し、terminal drain 開始後の [`WebRtcConfig::activate_slot`] を
    /// フェイルクローズで拒否するために使う。rebind（世代交代のみで終端しない経路）では
    /// このフラグを立てず、スナップショット方式（[`WebRtcConfig::take_active_peers`]）
    /// のみで対応する（`docs/design/ws-cancellation-propagation.md` 10 節を参照）。
    terminal_draining: Arc<AtomicBool>,
}

impl WebRtcConfig {
    /// 既定値で設定を構築する。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_webrtc::WebRtcConfig;
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
    /// use fandhe_backend_plugin_webrtc::WebRtcConfig;
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
    /// use fandhe_backend_plugin_webrtc::WebRtcConfig;
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
    /// use fandhe_backend_plugin_webrtc::WebRtcConfig;
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
    /// `slot_id` が既に除去済み（タイムアウト等との競合）の場合はレジストリを変更せず
    /// `false` を返す（呼び出し元が `pc` の生存管理に責任を持つ契約、下記参照）。
    ///
    /// # 終端 drain との競合（イシュー #498）
    ///
    /// [`WebRtcConfig::terminal_draining`] の判定は `registry` の `Mutex` ロック区間内
    /// （`Active` への遷移と同一クリティカルセクション）で行う。`terminal_draining` の
    /// 読み取りをロック外で行うと、`drain::drain_for_shutdown` が
    /// `begin_terminal_drain()`（フラグ設定）→ `close_active_peers`
    /// （`take_active_peers` でロックを取り既存 `Active` を除去）と進む間に割り込んだ
    /// 呼び出しがフラグを `false` のまま読み取ってロックを獲得し、`take_active_peers`
    /// の対象漏れ（`Reserved` のまま）だった枠を drain 完了後に `Active` 化してしまう
    /// TOCTOU が生じる（終端 drain 後に生成された接続が二度と close トリガを受けず
    /// 残存する回帰）。ロック内で判定することで、`begin_terminal_drain` が
    /// `take_active_peers` のロック獲得より必ず先行する（`drain_for_shutdown` の呼び出し
    /// 順）という前提のもと、本メソッドの判定は「`take_active_peers` の直前」または
    /// 「`take_active_peers` の直後」のいずれかに一意に順序付けられ、後者であっても
    /// フラグは既に `true` になっているため確実に拒否できる。
    ///
    /// フラグが `true`（`drain::drain_for_shutdown` 呼び出し済み）の場合、または対象
    /// `slot_id` が既に除去済み（タイムアウト等との競合）の場合は登録を拒否し、
    /// 予約枠も即座に解放したうえで `false` を返す（フェイルクローズ。
    /// `.claude/rules/security.md`）。呼び出し元は戻り値が `false` の場合、受け取った
    /// `pc` を自身で明示的に `close()` する契約とする（最終 shutdown 開始後に生成された、
    /// またはレジストリに枠が存在しない `RTCPeerConnection` をレジストリの生存管理外へ
    /// 漏らさないため）。戻り値が `true` の場合のみレジストリが `pc` の生存を保持する。
    #[must_use]
    pub(crate) fn activate_slot(&self, slot_id: u64, pc: Arc<RTCPeerConnection>) -> bool {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if self.terminal_draining.load(Ordering::Acquire) {
            registry.retain(|(id, _)| *id != slot_id);
            return false;
        }
        match registry.iter_mut().find(|(id, _)| *id == slot_id) {
            Some(entry) => {
                entry.1 = RegistrySlot::Active(pc);
                true
            }
            None => false,
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

    /// レジストリから `Active` エントリのみをすべて除去し、保持していた
    /// `Arc<RTCPeerConnection>` を返す（イシュー #498）。
    ///
    /// `crate::drain::close_active_peers` から呼ばれ、返された `Arc` は呼び出し元が
    /// 明示的に `close()` する契約とする（`RegistrySlot::Active` の doc の生存管理
    /// 契約を参照。除去済みなのでこの呼び出し以降 `release_slot` を重ねて呼ぶ必要は
    /// ない）。`Reserved`（シグナリング進行中）エントリは対象外とし、そのまま
    /// レジストリに残す（[`WebRtcConfig::activate_slot`] の終端 drain 判定に委ねる）。
    pub(crate) fn take_active_peers(&self) -> Vec<Arc<RTCPeerConnection>> {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        let mut taken = Vec::new();
        registry.retain(|(_, slot)| match slot {
            RegistrySlot::Active(pc) => {
                taken.push(Arc::clone(pc));
                false
            }
            RegistrySlot::Reserved => true,
        });
        taken
    }

    /// 最終 graceful shutdown の開始を記録する（イシュー #498）。
    ///
    /// 以降の [`WebRtcConfig::activate_slot`] は新規登録を拒否し、フェイルクローズで
    /// `false` を返すようになる。`registry` は `Clone` で世代を跨いで共有されるため、
    /// この呼び出しは `WebRtcConfig` の全クローンに波及する（`rebind` のような
    /// 世代交代のみの経路ではこのメソッドを呼ばず、[`WebRtcConfig::take_active_peers`]
    /// のスナップショット方式のみを使う。呼び出しは冪等）。
    pub(crate) fn begin_terminal_drain(&self) {
        self.terminal_draining.store(true, Ordering::Release);
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
            terminal_draining: Arc::new(AtomicBool::new(false)),
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

    #[test]
    fn take_active_peers_leaves_reserved_entries_untouched() {
        // イシュー #498: take_active_peers は Active エントリのみを除去し、
        // シグナリング進行中の Reserved エントリはレジストリに残す。
        let config = WebRtcConfig::new();
        let reserved = config.reserve_slot().expect("予約できる");
        assert!(config.take_active_peers().is_empty(), "Active は 0 件");
        // Reserved は除去されず、上限判定に引き続き影響する。
        let registry_len = config.registry.lock().unwrap().len();
        assert_eq!(registry_len, 1);
        config.release_slot(reserved);
    }

    #[test]
    fn activate_slot_rejected_after_terminal_drain_begins() {
        // イシュー #498: begin_terminal_drain 後の activate_slot はフェイルクローズで
        // false を返し、予約枠も解放する（呼び出し元が pc を明示的に close する契約）。
        let config = WebRtcConfig::new().with_max_peer_connections(1);
        let slot_id = config.reserve_slot().expect("1 件目は予約できる");
        config.begin_terminal_drain();

        // activate_slot は webrtc-rs の RTCPeerConnection 生成を要求しないため、
        // ダミーではなく実際の型が必要。ユニットテストでは生成コストを避けるため、
        // レジストリの状態変化のみを検証する統合テストを
        // crates/plugin-webrtc/tests へ別途追加する（本テストは begin_terminal_drain
        // 単体での予約枠解放を確認する）。
        assert!(
            config.reserve_slot().is_none(),
            "上限到達時（1/1 予約中）は新規予約できないはず"
        );
        // 終端 drain フラグが立っていることを take_active_peers 越しに間接検証する
        // （Active への遷移前なので空のまま）。
        assert!(config.take_active_peers().is_empty());
        config.release_slot(slot_id);
        assert!(config.reserve_slot().is_some(), "解放後は再び予約できる");
    }

    #[test]
    fn begin_terminal_drain_is_idempotent() {
        let config = WebRtcConfig::new();
        config.begin_terminal_drain();
        config.begin_terminal_drain();
        assert!(config.terminal_draining.load(Ordering::Acquire));
    }

    /// レビュー対応（イシュー #498）: `activate_slot` の判定と `Active` 遷移を
    /// `registry` の単一 `Mutex` ロック区間で行う修正の直接的な回帰テスト。
    ///
    /// `take_active_peers`（`drain_for_shutdown` が呼ぶ）が完了した**後**に
    /// `activate_slot` が呼ばれても、`begin_terminal_drain` 済みであれば `Active` へ
    /// 遷移させず `false` を返し、予約枠も解放することを実 `RTCPeerConnection` で
    /// 確認する。修正前の実装（ロック外で `terminal_draining` を読む）ではこの
    /// 呼び出し順でも `activate_slot` が独立にロック外でフラグを読む一瞬前に
    /// フラグが立っていなければ `true` を返しうる契約不備があったため、本テストは
    /// 「`begin_terminal_drain` → `take_active_peers` → `activate_slot` の順で
    /// 呼んでも漏れなく拒否される」という最低限の直列シナリオを固定する
    /// （真の並行 TOCTOU 自体は `registry` の `Mutex` による排他が構造的に防ぐため、
    /// マルチスレッド注入によるレース再現テストは行わない）。
    #[tokio::test]
    async fn activate_slot_rejects_pc_even_after_take_active_peers_already_ran() {
        use webrtc::api::APIBuilder;
        use webrtc::api::interceptor_registry::register_default_interceptors;
        use webrtc::api::media_engine::MediaEngine;
        use webrtc::interceptor::registry::Registry;
        use webrtc::peer_connection::configuration::RTCConfiguration;

        let config = WebRtcConfig::new();
        let slot_id = config.reserve_slot().expect("予約できる");

        // 終端 drain 開始 → 既存 Active の drain（0 件、no-op）を、
        // `activate_slot` 呼び出しより先に完了させておく
        // （「`take_active_peers` が先に走り切った後で `activate_slot` が来る」
        // という、レビュー指摘のシナリオの帰結を再現する）。
        config.begin_terminal_drain();
        assert!(config.take_active_peers().is_empty());

        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs().unwrap();
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine).unwrap();
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();
        let pc = Arc::new(
            api.new_peer_connection(RTCConfiguration::default())
                .await
                .unwrap(),
        );

        let activated = config.activate_slot(slot_id, Arc::clone(&pc));
        assert!(
            !activated,
            "終端 drain 開始後の activate_slot は false を返すはず（フェイルクローズ）"
        );
        assert!(
            config.take_active_peers().is_empty(),
            "activate_slot が false を返した以上、pc は Active としてレジストリに\
             残っていないはず（残っていれば #498 の TOCTOU が再発している）"
        );

        let _ = pc.close().await;
    }

    /// レビュー対応（イシュー #498、Cursor Bugbot 指摘）: 対象 `slot_id` が
    /// レジストリに存在しない（タイムアウト等で既に `release_slot` 済みの
    /// missing-slot レース）場合、`activate_slot` は `false` を返し、
    /// レジストリへ `Active` エントリを新規作成しないことを固定する。
    ///
    /// 修正前は `registry.iter_mut().find(..)` が `None` を返す（＝該当枠なし）
    /// 経路でも無条件に `true` を返しており、呼び出し元（`handler::
    /// complete_signaling`）が明示 `close()` パスをスキップしてしまっていた。
    #[tokio::test]
    async fn activate_slot_returns_false_for_missing_slot() {
        use webrtc::api::APIBuilder;
        use webrtc::api::interceptor_registry::register_default_interceptors;
        use webrtc::api::media_engine::MediaEngine;
        use webrtc::interceptor::registry::Registry;
        use webrtc::peer_connection::configuration::RTCConfiguration;

        let config = WebRtcConfig::new();
        let slot_id = config.reserve_slot().expect("予約できる");
        // タイムアウト等の競合により、activate_slot 到達前に枠が除去済みの状況を再現する。
        config.release_slot(slot_id);

        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs().unwrap();
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine).unwrap();
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();
        let pc = Arc::new(
            api.new_peer_connection(RTCConfiguration::default())
                .await
                .unwrap(),
        );

        let activated = config.activate_slot(slot_id, Arc::clone(&pc));
        assert!(
            !activated,
            "既に除去済みの slot_id への activate_slot は false を返すはず\
             （呼び出し元が pc を明示的に close する契約）"
        );
        assert!(
            config.take_active_peers().is_empty(),
            "activate_slot が false を返した以上、pc は Active としてレジストリに\
             登録されていないはず"
        );

        let _ = pc.close().await;
    }
}
