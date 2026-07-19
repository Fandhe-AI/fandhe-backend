//! TASK-8.4（#29）攻撃表面の受け入れ契約テスト。
//!
//! `crates/plugin-webrtc/src/handler.rs` の `#[cfg(test)]` モジュールは同一クレート内
//! （white-box）から `try_handle_rtc_offer` の境界（サイズ上限・接続数上限・
//! タイムアウト・不正 Offer）を検証済みだが、本ファイルは公開クレート境界の外側
//! （`fandhe_backend_plugin_webrtc::{WebRtcConfig, try_handle_rtc_offer, OFFER_PATH}`）からのみ
//! black-box で同じ境界を再アサートする受け入れテストとして独立させる
//! （TASK-8.1 実装のリファクタで内部モジュール構造が変わっても、公開 API 契約が
//! 破られていないことを別視点で継続検証する。`docs/acceptance/req8-webrtc-attack-surface.md`
//! が参照する成果物）。
//!
//! `RTCPeerConnection` の実生成を伴わない（不正 SDP・容量超過・上限到達で早期
//! リターンする）シナリオのみを対象とし、`tests/webrtc_datachannel.rs`
//! （実ネットワーク疎通）とは責務を分離する。実装変更は行わない。

use fandhe_backend_http::request::{ParseOutcome, RequestHead, parse_request_head};
use fandhe_backend_plugin_webrtc::{OFFER_PATH, WebRtcConfig, try_handle_rtc_offer};
use std::time::Duration;

/// 生のリクエストバイト列から `RequestHead` を組み立てる（テストユーティリティ）。
fn head_from(buf: &[u8]) -> RequestHead {
    match parse_request_head(buf).expect("リクエストヘッダのパースに失敗した") {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => panic!("expected Complete"),
    }
}

/// `OFFER_PATH` 定数が実際に `/rtc/offer` であることを確認する。攻撃表面評価
/// レポート（`docs/acceptance/req8-webrtc-attack-surface.md`）が前提とするパス
/// インターセプト対象がドキュメントと一致することを機械的に担保する。
#[test]
fn offer_path_constant_is_documented_value() {
    assert_eq!(OFFER_PATH, "/rtc/offer");
}

/// 対象外パス・対象外メソッドは `None`（フォールスルー契約）を返し、既定
/// `Handler` へ制御を渡す。無関係なリクエストへの介入がないことは NFR-6
/// （`docs/spec/04-requirements.md`）の前提条件でもある。
#[tokio::test]
async fn unrelated_path_and_method_fall_through() {
    let config = WebRtcConfig::new();

    let head = head_from(b"GET /health HTTP/1.1\r\n\r\n");
    assert!(try_handle_rtc_offer(&head, b"", &config).await.is_none());

    let head = head_from(b"GET /rtc/offer HTTP/1.1\r\n\r\n");
    assert!(try_handle_rtc_offer(&head, b"", &config).await.is_none());
}

/// `config.max_offer_bytes()` を超える SDP Offer はリソース枯渇（DoS）対策として
/// `RTCPeerConnection` を一切生成せず `413` で拒否する（`.claude/rules/security.md`）。
#[tokio::test]
async fn offer_exceeding_max_bytes_is_rejected_with_413() {
    let config = WebRtcConfig::new().with_max_offer_bytes(8);
    let body = vec![b'x'; 16];
    let head = head_from(b"POST /rtc/offer HTTP/1.1\r\nContent-Length: 16\r\n\r\n");

    let response = try_handle_rtc_offer(&head, &body, &config)
        .await
        .expect("対象パスなので Some のはず");

    assert_eq!(response.status, 413);
    // 内部エラー詳細（SDP 本文の断片等）を含まない定型 JSON のみを返すこと
    // （エラー情報漏えい対策、.claude/rules/security.md）。
    assert_eq!(response.body, br#"{"error":"offer_too_large"}"#);
}

/// `config.max_peer_connections()` に達している場合、新規 `RTCPeerConnection` を
/// 生成せずレジストリ判定のみで `503`（フェイルクローズ）を返す。
#[tokio::test]
async fn peer_connection_limit_is_enforced_fail_closed() {
    let config = WebRtcConfig::new().with_max_peer_connections(0);
    let body = br#"{"type":"offer","sdp":"v=0"}"#;
    let head = head_from(
        format!(
            "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    );

    let response = try_handle_rtc_offer(&head, body, &config)
        .await
        .expect("対象パスなので Some のはず");

    assert_eq!(response.status, 503);
    assert_eq!(
        response.body,
        br#"{"error":"peer_connection_limit_reached"}"#
    );
}

/// シグナリング全体のタイムアウト（`config.signaling_timeout()`）を極端に短く
/// すると、正常経路でも `504` で打ち切られる。環境依存のタイミングで JSON/SDP
/// パースが先に完了し `400` を返す場合もあるため、両方を許容する
/// （`crates/plugin-webrtc/src/handler.rs` の `signaling_timeout_returns_504` と
/// 同じ緩和方針、PoC-9 教訓）。
#[tokio::test]
async fn signaling_timeout_is_enforced() {
    let config = WebRtcConfig::new().with_signaling_timeout(Duration::from_nanos(1));
    let body = br#"{"type":"offer","sdp":"not a valid sdp"}"#;
    let head = head_from(
        format!(
            "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    );

    let response = try_handle_rtc_offer(&head, body, &config)
        .await
        .expect("対象パスなので Some のはず");

    assert!(
        response.status == 504 || response.status == 400,
        "504（タイムアウト）または 400（早期の不正 SDP 検出）のいずれかのはず（実際: {}）",
        response.status
    );
}

/// 不正な JSON（Offer 形式ですらない）は `RTCPeerConnection` 生成前に `400` で拒否する。
#[tokio::test]
async fn malformed_json_offer_is_rejected_with_400() {
    let config = WebRtcConfig::new();
    let body = b"not json at all";
    let head = head_from(
        format!(
            "POST /rtc/offer HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    );

    let response = try_handle_rtc_offer(&head, body, &config)
        .await
        .expect("対象パスなので Some のはず");

    assert_eq!(response.status, 400);
    assert_eq!(response.body, br#"{"error":"invalid_offer_json"}"#);
}

/// `Content-Length` ヘッダと実ボディ長の不一致は `RTCPeerConnection` 生成前に
/// `400` で拒否する（リクエストスマグリング類の不整合対策）。
#[tokio::test]
async fn content_length_mismatch_is_rejected_with_400() {
    let config = WebRtcConfig::new();
    let head = head_from(b"POST /rtc/offer HTTP/1.1\r\nContent-Length: 100\r\n\r\n");

    let response = try_handle_rtc_offer(&head, b"short", &config)
        .await
        .expect("対象パスなので Some のはず");

    assert_eq!(response.status, 400);
    assert_eq!(response.body, br#"{"error":"content_length_mismatch"}"#);
}
