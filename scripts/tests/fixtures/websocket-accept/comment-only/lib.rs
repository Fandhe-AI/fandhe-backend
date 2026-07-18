//! run-websocket-accept-tests.sh 用フィクスチャ: 日本語 doc comment・行コメント中にのみ
//! "unsafe" という字句が現れる擬似ソース。websocket-accept.sh の基準 A' の grep
//! パイプラインが行コメントを除外し、誤検出しないことを検証する対象
//! （`scripts/accept/webrtc-accept.sh` の check_unsafe と同一パターン）。

// この関数は unsafe を一切使わない安全な実装のみで完結する。
pub fn safe_only(a: i32) -> i32 {
    // 以前は unsafe なポインタ操作を検討したが、安全な API のみで書き直した。
    a * 2
}
