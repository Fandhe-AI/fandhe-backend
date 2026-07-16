//! シグナリングプロキシの静的設定（[`ProxyConfig`]）。
//!
//! [`handler::try_handle_rtc_offer`](crate::handler::try_handle_rtc_offer) と
//! [`client::forward_offer`](crate::client::forward_offer) の双方から参照される。
//! 上流アドレスをリクエスト由来の値で決めず、本設定（ビルド時・起動時の静的値）
//! のみを転送先とすることで SSRF を構造的に防止する契約（.claude/rules/security.md）。

use std::time::Duration;

/// シグナリングプロキシの設定。
///
/// 上流 WebRTC サービスの接続先・タイムアウト・ペイロードサイズ上限を保持する。
/// フィールドはすべて非公開とし、[`ProxyConfig::new`] または [`Default`] 経由での
/// 構築を強制することで、不変条件（`max_offer_bytes`/`max_answer_bytes` が 0
/// でない等）を将来のバリデーション追加時にも一箇所で守れるようにする。
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    upstream_addr: String,
    upstream_path: String,
    connect_timeout: Duration,
    request_timeout: Duration,
    max_offer_bytes: usize,
    max_answer_bytes: usize,
}

/// [`ProxyConfig::upstream_path`] の既定値。
const DEFAULT_UPSTREAM_PATH: &str = "/rtc/offer";

/// 上流への接続確立を待つ既定タイムアウト（スロー上流対策、.claude/rules/security.md）。
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// 上流からの応答受信を待つ既定タイムアウト（接続確立後、body 受信完了まで）。
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// SDP Offer/Answer の既定サイズ上限（64 KiB）。
///
/// 一般的な SDP は数 KiB 程度に収まるため、64 KiB は実用上十分な余裕を持ちつつ
/// メモリ枯渇（DoS）を防ぐ値として選定した（.claude/rules/security.md）。
const DEFAULT_MAX_PAYLOAD_BYTES: usize = 64 * 1024;

impl ProxyConfig {
    /// 上流アドレス（`host:port`）を指定して設定を構築する。
    ///
    /// その他のフィールドは既定値（[`Default`] 実装参照）で初期化される。
    /// `upstream_addr` は MVP ではプライベートネットワーク内の HTTP/1.1 サーバを
    /// 前提とし、TLS/mTLS は本タスクのスコープ外（out-of-scope-tracking 対象）。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000");
    /// assert_eq!(config.upstream_addr(), "127.0.0.1:9000");
    /// assert_eq!(config.upstream_path(), "/rtc/offer");
    /// ```
    pub fn new(upstream_addr: impl Into<String>) -> Self {
        Self {
            upstream_addr: upstream_addr.into(),
            ..Self::default()
        }
    }

    /// 上流のリクエストパスを上書きする（既定 `/rtc/offer`）。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000").with_upstream_path("/webrtc/offer");
    /// assert_eq!(config.upstream_path(), "/webrtc/offer");
    /// ```
    pub fn with_upstream_path(mut self, path: impl Into<String>) -> Self {
        self.upstream_path = path.into();
        self
    }

    /// 接続確立タイムアウトを上書きする。
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000")
    ///     .with_connect_timeout(Duration::from_millis(500));
    /// assert_eq!(config.connect_timeout(), Duration::from_millis(500));
    /// ```
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// 上流応答待ちタイムアウトを上書きする。
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000")
    ///     .with_request_timeout(Duration::from_millis(1500));
    /// assert_eq!(config.request_timeout(), Duration::from_millis(1500));
    /// ```
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// クライアントから受理する SDP Offer の最大バイト数を上書きする。
    ///
    /// リソース枯渇（DoS）対策として上限を明示的に絞り込みたい場合に使う
    /// （.claude/rules/security.md）。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000").with_max_offer_bytes(1024);
    /// assert_eq!(config.max_offer_bytes(), 1024);
    /// ```
    pub fn with_max_offer_bytes(mut self, max_bytes: usize) -> Self {
        self.max_offer_bytes = max_bytes;
        self
    }

    /// 上流から受理する SDP Answer の最大バイト数を上書きする。
    ///
    /// リソース枯渇（DoS）対策として上限を明示的に絞り込みたい場合に使う
    /// （.claude/rules/security.md）。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000").with_max_answer_bytes(2048);
    /// assert_eq!(config.max_answer_bytes(), 2048);
    /// ```
    pub fn with_max_answer_bytes(mut self, max_bytes: usize) -> Self {
        self.max_answer_bytes = max_bytes;
        self
    }

    /// 上流アドレス（`host:port`）。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000");
    /// assert_eq!(config.upstream_addr(), "127.0.0.1:9000");
    /// ```
    pub fn upstream_addr(&self) -> &str {
        &self.upstream_addr
    }

    /// 上流へ転送する際のリクエストパス。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000");
    /// assert_eq!(config.upstream_path(), "/rtc/offer");
    /// ```
    pub fn upstream_path(&self) -> &str {
        &self.upstream_path
    }

    /// 接続確立タイムアウト。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000");
    /// assert!(config.connect_timeout().as_secs() > 0);
    /// ```
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// 上流応答待ちタイムアウト。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000");
    /// assert!(config.request_timeout().as_secs() > 0);
    /// ```
    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    /// クライアントから受理する SDP Offer の最大バイト数。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000");
    /// assert_eq!(config.max_offer_bytes(), 64 * 1024);
    /// ```
    pub fn max_offer_bytes(&self) -> usize {
        self.max_offer_bytes
    }

    /// 上流から受理する SDP Answer の最大バイト数。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_webrtc_proxy::ProxyConfig;
    ///
    /// let config = ProxyConfig::new("127.0.0.1:9000");
    /// assert_eq!(config.max_answer_bytes(), 64 * 1024);
    /// ```
    pub fn max_answer_bytes(&self) -> usize {
        self.max_answer_bytes
    }
}

impl Default for ProxyConfig {
    /// 上流アドレス未設定（空文字列）の既定値を返す。
    ///
    /// `upstream_addr` は空文字列のままだと [`crate::client::forward_offer`] が
    /// 接続エラーとして扱う（フェイルクローズ、.claude/rules/security.md）ため、
    /// 実運用では必ず [`ProxyConfig::new`] で上流アドレスを指定すること。
    fn default() -> Self {
        Self {
            upstream_addr: String::new(),
            upstream_path: DEFAULT_UPSTREAM_PATH.to_string(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_offer_bytes: DEFAULT_MAX_PAYLOAD_BYTES,
            max_answer_bytes: DEFAULT_MAX_PAYLOAD_BYTES,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_overrides_apply() {
        let config = ProxyConfig::new("10.0.0.1:8080")
            .with_upstream_path("/webrtc/offer")
            .with_connect_timeout(Duration::from_millis(500))
            .with_request_timeout(Duration::from_millis(1500))
            .with_max_offer_bytes(1024)
            .with_max_answer_bytes(2048);

        assert_eq!(config.upstream_addr(), "10.0.0.1:8080");
        assert_eq!(config.upstream_path(), "/webrtc/offer");
        assert_eq!(config.connect_timeout(), Duration::from_millis(500));
        assert_eq!(config.request_timeout(), Duration::from_millis(1500));
        assert_eq!(config.max_offer_bytes(), 1024);
        assert_eq!(config.max_answer_bytes(), 2048);
    }

    #[test]
    fn default_has_empty_upstream_addr_and_standard_path() {
        let config = ProxyConfig::default();
        assert_eq!(config.upstream_addr(), "");
        assert_eq!(config.upstream_path(), DEFAULT_UPSTREAM_PATH);
        assert_eq!(config.max_offer_bytes(), DEFAULT_MAX_PAYLOAD_BYTES);
        assert_eq!(config.max_answer_bytes(), DEFAULT_MAX_PAYLOAD_BYTES);
    }
}
