//! `RTCPeerConnection` の有界 drain API（イシュー #498）。
//!
//! WS 委譲タスクの世代キャンセル機構（`crates/core/src/plugin.rs` の
//! `GenerationCancel`/`UpgradeCancel`、イシュー #489〜#497）を WS 以外の長時間委譲
//! プラグインへ水平展開した第 1 弾。本クレートはパスインターセプト型のため
//! `UpgradeHandler` のような世代別 `watch` 購読は使わず、`WebRtcConfig::registry`
//! （プロセス内で世代を跨いで共有される）の**発火時点のスナップショットを close する
//! 「レジストリ drain 型」**で実現する（`docs/design/ws-cancellation-propagation.md`
//! 10 節「WS 以外への水平展開」を参照）。
//!
//! `crates/core` の `SessionDrain`（`webrtc` feature ゲート、`crate::plugin` 内）が
//! 最終 graceful shutdown（イシュー #313）・rebind 世代 drain（イシュー #485/#488）の
//! 両経路から本モジュールの関数を呼ぶ。呼び出しは detached タスクへ切り離されるため、
//! `run_until` の「grace + ε 以内に必ず戻る」フェイルセーフ（既存の permit 回収
//! タイムアウト）を妨げない（本モジュールの関数自体も 1 接続あたり
//! `per_close_timeout` で打ち切る有界処理であることに加え、呼び出し元がさらに
//! `tokio::spawn` で切り離す）。

use std::time::Duration;

use crate::config::WebRtcConfig;

/// アクティブな `RTCPeerConnection` すべてを、1 接続あたり `per_close_timeout` を
/// 上限として明示的に `close()` する（イシュー #498）。
///
/// `RTCPeerConnection::close()` は DTLS/SCTP の正規クローズシーケンスを実行する
/// （WebSocket の Close frame 送出に相当する正常終了手順）。`WebRtcConfig::
/// take_active_peers` でレジストリから対象を切り離してから並行に close するため、
/// close 中に新規の `activate_slot` がこのスナップショットへ割り込むことはない。
///
/// rebind（世代交代のみで `WebRtcConfig` 自体は新世代と共有され続ける）から呼ぶ
/// ことを想定し、[`drain_for_shutdown`] と異なり以降の新規登録は拒否しない
/// （終端 drain フラグは立てない）。rebind 発火後・本関数呼び出し完了前に新規
/// シグナリングが `activate_slot` に到達した場合、その接続は新世代のものと区別
/// できず生き残る（既知の限界。`config.signaling_timeout()` で有界なため無期限には
/// 残らない、`docs/design/ws-cancellation-propagation.md` 10 節を参照）。
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use fandhe_backend_plugin_webrtc::WebRtcConfig;
/// use fandhe_backend_plugin_webrtc::close_active_peers;
///
/// # #[tokio::main]
/// # async fn main() {
/// // Active な接続が 1 件もないレジストリに対しては即座に戻る（no-op）。
/// let config = WebRtcConfig::new();
/// close_active_peers(&config, Duration::from_secs(5)).await;
/// # }
/// ```
pub async fn close_active_peers(config: &WebRtcConfig, per_close_timeout: Duration) {
    let peers = config.take_active_peers();
    if peers.is_empty() {
        return;
    }
    // 各 close() を独立した tokio タスクへ切り離し、1 接続の close 遅延が他接続の
    // close 完了を待たせないようにする（並行 close、`.claude/rules/coding-rust.md`）。
    let mut tasks = Vec::with_capacity(peers.len());
    for pc in peers {
        tasks.push(tokio::spawn(async move {
            // close() 自体がハングする可能性（webrtc-rs 内部の I/O 待ち）を想定し、
            // per_close_timeout で打ち切る。打ち切り時は pc（Arc）をこのタスクの
            // スコープで手放すのみとし、以降の解放は既存の Drop 経路に委ねる
            // （呼び出し元 doc の「有界処理」契約を参照）。
            let _ = tokio::time::timeout(per_close_timeout, pc.close()).await;
        }));
    }
    for task in tasks {
        // 個々のタスクの JoinError（panic 等、通常運用では発生しない）は無視する。
        // 本関数の責務は「close を試みること」であり、タスク側の異常終了は
        // pc の生存管理（Arc の解放）を妨げない。
        let _ = task.await;
    }
}

/// 最終 graceful shutdown 向けの drain（イシュー #498）。
///
/// [`WebRtcConfig::begin_terminal_drain`] を呼んで以降の新規登録
/// （[`WebRtcConfig::activate_slot`]）を拒否させたうえで、[`close_active_peers`] で
/// 既存のアクティブ接続を明示的に close する。`WebRtcConfig` は `Clone` で世代を跨いで
/// 共有されるため、以降このプロセスで新たに生成される `RTCPeerConnection` は
/// （シグナリング自体は継続しても）レジストリへ登録されず即座に close される
/// （`crate::handler::complete_signaling` の `activate_slot` 呼び出し箇所を参照）。
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use fandhe_backend_plugin_webrtc::WebRtcConfig;
/// use fandhe_backend_plugin_webrtc::drain_for_shutdown;
///
/// # #[tokio::main]
/// # async fn main() {
/// let config = WebRtcConfig::new();
/// drain_for_shutdown(&config, Duration::from_secs(5)).await;
/// // Active な接続が 0 件のレジストリに対しても安全に呼べる（no-op）。
/// # }
/// ```
pub async fn drain_for_shutdown(config: &WebRtcConfig, per_close_timeout: Duration) {
    config.begin_terminal_drain();
    close_active_peers(config, per_close_timeout).await;
}
