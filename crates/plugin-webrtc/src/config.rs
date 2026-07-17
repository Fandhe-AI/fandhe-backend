//! in-process WebRTC プラグインの静的設定（[`WebRtcConfig`]）。
//!
//! [`crate::handler::try_handle_rtc_offer`] が参照する。生成した
//! `RTCPeerConnection` を保持するレジストリ（`WebRtcConfig::registry`、
//! `pub(crate)` の内部 API）もここで保持し、テスト容易性のため PoC-5 の
//! `OnceLock` グローバルは使わず [`WebRtcConfig`] インスタンス単位の
//! フィールドとする（`.claude/rules/coding-rust.md` の AI ファースト保守性）。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use webrtc::peer_connection::RTCPeerConnection;

/// SDP Offer の既定サイズ上限（64 KiB）。
///
/// `crates/plugin-webrtc-proxy::config::DEFAULT_MAX_PAYLOAD_BYTES` と同値に揃え、
/// 一般的な SDP（数 KiB 程度）に十分な余裕を持たせつつメモリ枯渇（DoS）を防ぐ
/// （.claude/rules/security.md）。
const DEFAULT_MAX_OFFER_BYTES: usize = 64 * 1024;

/// 同時に保持する `RTCPeerConnection` 数の既定上限。
///
/// 生成した `RTCPeerConnection` はプロセス内レジストリ（[`WebRtcConfig::registry`]）
/// にクローズ処理なしで保持され続ける（PoC-5 由来の制約、恒久対応は TASK-8.3・#28へ
/// 申し送り）。上限を設けず無制限に受理するとメモリ枯渇（DoS）に直結するため、
/// 超過時は [`crate::handler::try_handle_rtc_offer`] が 503 で拒否する
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
    registry: Arc<Mutex<Vec<Arc<RTCPeerConnection>>>>,
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

    /// 保持中の `RTCPeerConnection` レジストリへの共有ハンドルを返す。
    ///
    /// [`crate::handler::try_handle_rtc_offer`] が同時接続数上限の判定・登録に使う
    /// 内部 API（`pub(crate)`）。
    pub(crate) fn registry(&self) -> &Arc<Mutex<Vec<Arc<RTCPeerConnection>>>> {
        &self.registry
    }
}

impl Default for WebRtcConfig {
    fn default() -> Self {
        Self {
            max_offer_bytes: DEFAULT_MAX_OFFER_BYTES,
            max_peer_connections: DEFAULT_MAX_PEER_CONNECTIONS,
            signaling_timeout: DEFAULT_SIGNALING_TIMEOUT,
            registry: Arc::new(Mutex::new(Vec::new())),
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
        assert!(Arc::ptr_eq(config.registry(), cloned.registry()));
    }
}
