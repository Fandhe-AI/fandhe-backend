//! `idle_timeout` と サーバー起点 Ping keepalive（`with_ping_interval`）を
//! 組み合わせたときの挙動を検証する統合テスト（イシュー #714、親 #712）。
//!
//! `idle_timeout.rs` は `idle_timeout` 単体、`ping_keepalive_e2e.rs` は
//! keepalive 単体（すべて `without_idle_timeout()` 構成）をそれぞれ検証済み
//! で、両者を併用したときの相互作用は `ping_keepalive_e2e.rs` のモジュール
//! doc が「#714 のスコープ」として明示的にここへ委ねていた。本ファイルは
//! その組み合わせを、`WebSocketConfig::idle_timeout` フィールド doc が
//! 推奨する設定（`interval + pong_timeout < idle_timeout`）とその誤設定
//! （`interval >= idle_timeout`）の両方について、公開 API 経由（`handle_upgrade`
//! + `on_open`/`WsSender` + `on_close`）で固定する:
//!
//! 1. [`push_only_traffic_triggers_idle_timeout`]（AC2）: keepalive 無効では、
//!    サーバー起点 push だけのトラフィックでも `idle_timeout` が発火する
//!    （push はアイドル期限をリセットしない、Issue #175 の DoS 対策）。
//! 2. [`ping_keepalive_keeps_push_only_client_alive_beyond_idle_timeout`]
//!    （AC3・推奨設定）: 推奨どおり `interval + pong_timeout < idle_timeout`
//!    で設定すると、push だけを受けている生存クライアントの自動 Pong が
//!    `idle_timeout` を延長し続け、切断されない。
//! 3. [`ping_keepalive_closes_unresponsive_client_with_pong_timeout_before_idle`]
//!    （AC3・死活検出）: 同じ推奨設定で、Pong を一切返さない対向は
//!    `idle_timeout` より先に Pong 期限（`CloseReason::PongTimeout`）で
//!    切断される（「早い方のタイマーが勝つ」契約の固定）。
//! 4. [`ping_interval_not_shorter_than_idle_timeout_still_hits_idle_timeout`]
//!    （AC3・誤設定の固定）: `interval >= idle_timeout` にすると、最初の
//!    Ping が送られる前に `idle_timeout` が発火してしまう（誤設定の警告を
//!    テストで裏付ける）。
//!
//! `session.rs` の単体テスト `outbound_push_does_not_reset_idle_timeout` /
//! `ping_send_does_not_reset_idle_timeout` は「push・Ping 送出それ自体が
//! `idle_deadline` を進めない」という内部の一点を直接検証する契約テスト。
//! 本ファイルはその契約の上に成り立つ、利用者から見える結果（切断される／
//! されない・どの `CloseReason` になるか）を公開 API から検証する e2e。
//!
//! `idle_timeout.rs` / `ping_keepalive_e2e.rs` と同様、`tokio::io::duplex` +
//! `tokio-tungstenite` クライアントで `handle_upgrade` を実際に駆動し、
//! 実時間ではなく仮想時間（`#[tokio::test(start_paused = true)]`）で決定的に
//! 駆動する。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    CloseReason, WsConnContext, WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext,
    WsOutcome,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列
/// （`idle_timeout.rs` / `ping_keepalive_e2e.rs` と同一のリクエスト）。
fn handshake_request_bytes() -> &'static [u8] {
    b"GET /ws HTTP/1.1\r\n\
      Host: example.com\r\n\
      Upgrade: websocket\r\n\
      Connection: Upgrade\r\n\
      Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
      Sec-WebSocket-Version: 13\r\n\
      \r\n"
}

/// クライアント側ストリームから `\r\n\r\n` までを読み切る
/// （`ping_keepalive_e2e.rs` と同一のヘルパー）。
async fn read_http_response_line<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).await.expect("read response byte");
        assert_ne!(n, 0, "stream closed before response terminator");
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buf).expect("response must be valid utf-8")
}

/// ハンドシェイクを成立させ、101 応答を読み切ったクライアント
/// `WebSocketStream` とサーバタスクの `JoinHandle` を返す
/// （`ping_keepalive_e2e.rs::handshake` と同一のヘルパー）。
async fn handshake(
    config: WebSocketConfig,
) -> (
    WebSocketStream<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), fandhe_backend_plugin_websocket::WsError>>,
) {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };

    let (server_side, mut client_side) = tokio::io::duplex(64 * 1024);

    let server_task = tokio::spawn(async move {
        handle_upgrade(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
        )
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let client: WebSocketStream<_> =
        WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;

    (client, server_task)
}

/// [`handshake`] と同じくハンドシェイクを成立させるが、クライアント側を
/// `WebSocketStream` へ包まず生の `DuplexStream` のまま返す
/// （`ping_keepalive_e2e.rs::handshake_raw` と同一のヘルパー）。
///
/// AC3 の死活検出ケースでは「Pong を一切返さない対向」を作る必要があり、
/// tokio-tungstenite のクライアント実装が Ping に自動で Pong を返してしまう
/// ため、生バイトのまま一切読み書きしない（`std::mem::forget` で保持する）
/// ことで対向を確実に無応答にする。
async fn handshake_raw(
    config: WebSocketConfig,
) -> (
    tokio::io::DuplexStream,
    tokio::task::JoinHandle<Result<(), fandhe_backend_plugin_websocket::WsError>>,
) {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };

    let (server_side, mut client_side) = tokio::io::duplex(64 * 1024);

    let server_task = tokio::spawn(async move {
        handle_upgrade(
            server_side,
            &head,
            Vec::new(),
            &config,
            std::future::pending::<()>(),
        )
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    (client_side, server_task)
}

/// テスト共通ハンドラ: `on_open` で `push_every` が `Some` の場合のみ
/// その間隔で `WsSender::send` による push を送り続け（AC2/AC3 の
/// 「push だけのトラフィック」を作る）、`on_close` で通知された
/// `CloseReason` を記録する（`ping_keepalive_e2e.rs::RecordClose` と同型）。
///
/// `push_every: None` は AC3 の死活検出ケース専用。push を一切行わないことで
/// 送信キュー詰まりとの区別を排し、検証対象を Pong 期限判定のみに絞る
/// （advisor 指摘対応。duplex バッファ詰まりで理由が曖昧になることを避ける）。
struct KeepaliveRecorder {
    push_every: Option<Duration>,
    push_count: Arc<AtomicUsize>,
    closed: Arc<Mutex<Vec<CloseReason>>>,
}

impl WsMessageHandler for KeepaliveRecorder {
    fn name(&self) -> &'static str {
        "idle-keepalive-recorder"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        let Some(push_every) = self.push_every else {
            return;
        };
        let sender = ctx.sender().clone();
        let push_count = self.push_count.clone();
        tokio::spawn(async move {
            let mut seq: u64 = 0;
            loop {
                tokio::time::sleep(push_every).await;
                // セッション終了後は `send` が `Err` を返す（`WsSender::send`
                // の doc を参照）。以降ループを続けても無意味なので打ち切る。
                if sender
                    .send(WsMessage::Text(format!("push-{seq}")))
                    .await
                    .is_err()
                {
                    break;
                }
                push_count.fetch_add(1, Ordering::SeqCst);
                seq += 1;
            }
        });
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }

    fn on_close(&self, _ctx: &WsConnContext, reason: CloseReason) {
        self.closed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(reason);
    }
}

/// AC2: keepalive を有効化していない（既定 = 無効）構成で、サーバー起点
/// push だけのトラフィック（クライアントは一切送信しない）を流しても、
/// `idle_timeout` は push の送出では延長されず、既定どおりの時間で発火する
/// こと。
///
/// `crate::session` の単体テスト `outbound_push_does_not_reset_idle_timeout`
/// が固定する内部契約（push は `idle_deadline` を進めない）の、利用者から
/// 見える結果（実際に切断される）を検証する。
#[tokio::test(start_paused = true)]
async fn push_only_traffic_triggers_idle_timeout() {
    let idle_timeout = Duration::from_secs(10);
    let push_every = Duration::from_secs(2);
    let close_grace = Duration::from_secs(1);
    let push_count = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(Mutex::new(Vec::new()));
    let config = WebSocketConfig::default()
        .with_idle_timeout(idle_timeout)
        .with_close_grace(close_grace)
        .with_handler(KeepaliveRecorder {
            push_every: Some(push_every),
            push_count: push_count.clone(),
            closed: closed.clone(),
        });
    let (mut client, server_task) = handshake(config).await;

    let start = tokio::time::Instant::now();
    // 個々の読み取りの上限は生成的な余裕を持たせる（詰まっていればここで
    // パニックし、無期限ハングにしない）。
    let per_read_timeout = idle_timeout * 3;
    let close_frame = loop {
        let msg = tokio::time::timeout(per_read_timeout, client.next())
            .await
            .expect("must not hang waiting for push or close")
            .expect("stream should yield a message")
            .expect("no protocol error");
        match msg {
            Message::Text(_) => {}
            Message::Close(frame) => break frame,
            other => panic!("unexpected frame while waiting for push/close: {other:?}"),
        }
    };
    let elapsed = tokio::time::Instant::now() - start;

    if let Some(frame) = close_frame {
        assert_eq!(frame.code, CloseCode::Normal);
    }
    assert!(
        push_count.load(Ordering::SeqCst) >= 1,
        "push traffic must actually have flowed before idle timeout fired"
    );
    assert!(
        elapsed >= idle_timeout,
        "idle timeout must not fire earlier than idle_timeout: {elapsed:?}"
    );
    assert!(
        elapsed <= idle_timeout + push_every + close_grace + Duration::from_secs(5),
        "push traffic must not have reset idle_timeout \
         (a reset would delay this far beyond idle_timeout): {elapsed:?}"
    );

    // tokio-tungstenite は Close 受信直後は応答を内部に溜めるだけで、実際の
    // 書き込みは次回の read 駆動時に行われる（`idle_timeout.rs` と同じ理由）。
    let _ = client.next().await;

    let result = tokio::time::timeout(
        idle_timeout + close_grace + Duration::from_secs(5),
        server_task,
    )
    .await
    .expect("server task must not hang")
    .unwrap();
    assert!(
        result.is_ok(),
        "idle timeout should end the session normally: {result:?}"
    );
    assert_eq!(
        closed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &[CloseReason::IdleTimeout],
        "on_close must report IdleTimeout exactly once"
    );
}

/// AC3（推奨設定・生存）: `interval + pong_timeout < idle_timeout`
/// （フィールド doc の推奨、ここでは 4s + 2s < 10s）で keepalive を有効化
/// すると、push だけを受けている生存クライアント（tokio-tungstenite が
/// Ping へ自動で Pong を返す）は `idle_timeout` の何倍もの仮想時間が過ぎても
/// 切断されないこと。
#[tokio::test(start_paused = true)]
async fn ping_keepalive_keeps_push_only_client_alive_beyond_idle_timeout() {
    let idle_timeout = Duration::from_secs(10);
    let interval = Duration::from_secs(4);
    let pong_timeout = Duration::from_secs(2);
    let push_every = Duration::from_secs(2);
    let push_count = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(Mutex::new(Vec::new()));
    let config = WebSocketConfig::default()
        .with_idle_timeout(idle_timeout)
        .with_ping_interval(interval, pong_timeout)
        .unwrap()
        .with_handler(KeepaliveRecorder {
            push_every: Some(push_every),
            push_count: push_count.clone(),
            closed: closed.clone(),
        });
    let (mut client, server_task) = handshake(config).await;

    let start = tokio::time::Instant::now();
    let bound = idle_timeout * 5;
    let mut pings = 0usize;
    while tokio::time::Instant::now() - start < bound {
        let msg = tokio::time::timeout(idle_timeout, client.next())
            .await
            .expect("must not hang: ping keepalive should keep frames flowing")
            .expect("stream should yield a message")
            .expect("no protocol error");
        match msg {
            Message::Text(_) => {}
            Message::Ping(_) => pings += 1,
            Message::Close(_) => {
                panic!("must not receive Close while ping keepalive keeps a push-only client alive")
            }
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    assert!(
        pings >= 1,
        "keepalive must actually have sent Ping frames during the bound \
         (otherwise this test would pass even if keepalive were disabled)"
    );
    assert!(
        push_count.load(Ordering::SeqCst) >= 1,
        "push traffic must have kept flowing throughout"
    );

    client.close(None).await.expect("client close");
    let result = tokio::time::timeout(Duration::from_secs(30), server_task)
        .await
        .expect("server task must not hang after client close")
        .unwrap();
    assert!(
        result.is_ok(),
        "session should end cleanly on client close: {result:?}"
    );
    assert_eq!(
        closed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &[CloseReason::ClientClose],
        "on_close must report ClientClose, not IdleTimeout/PongTimeout"
    );
}

/// AC3（推奨設定・死活検出）: ケース 2 と同じ推奨設定で、Pong を一切返さない
/// 対向（[`handshake_raw`] で生バイトのまま保持し、読み書きを一切しない）は
/// `idle_timeout`（10s）より先に Pong 期限（`interval` + `pong_timeout` =
/// 4s + 2s = 6s）で `CloseReason::PongTimeout` として切断されること。
///
/// 「早い方のタイマーが勝つ」契約（`crate::session` モジュール doc・
/// `docs/design/ws-connection-context-and-close.md` 13 節）を、keepalive と
/// `idle_timeout` を併用した構成で固定する。
#[tokio::test(start_paused = true)]
async fn ping_keepalive_closes_unresponsive_client_with_pong_timeout_before_idle() {
    let idle_timeout = Duration::from_secs(10);
    let interval = Duration::from_secs(4);
    let pong_timeout = Duration::from_secs(2);
    let close_grace = Duration::from_secs(1);
    let push_count = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(Mutex::new(Vec::new()));
    let config = WebSocketConfig::default()
        .with_idle_timeout(idle_timeout)
        .with_ping_interval(interval, pong_timeout)
        .unwrap()
        .with_close_grace(close_grace)
        .with_handler(KeepaliveRecorder {
            // 検証対象を Pong 期限判定のみに絞る（ハンドラ doc を参照）。
            push_every: None,
            push_count: push_count.clone(),
            closed: closed.clone(),
        });
    let (client, server_task) = handshake_raw(config).await;
    std::mem::forget(client);

    let start = tokio::time::Instant::now();
    let result = tokio::time::timeout(interval + pong_timeout + close_grace * 4, server_task)
        .await
        .expect("server must not hang: pong timeout + close_grace bound the wait")
        .unwrap();
    let elapsed = tokio::time::Instant::now() - start;

    assert!(
        result.is_ok(),
        "pong timeout is policy-driven, not a protocol error: {result:?}"
    );
    assert!(
        elapsed < idle_timeout,
        "the earlier deadline (pong timeout) must win over idle_timeout: {elapsed:?}"
    );
    assert_eq!(push_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        closed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &[CloseReason::PongTimeout],
        "on_close must report PongTimeout (the earlier deadline), not IdleTimeout"
    );
}

/// AC3（誤設定の固定）: `interval`（10s）が `idle_timeout`（5s）以上の誤設定
/// では、最初の Ping が送出される前に `idle_timeout` が発火し、生きている
/// 受信専用クライアントでも `CloseReason::IdleTimeout` で切断されてしまう
/// こと（フィールド doc の警告をテストで裏付ける）。
#[tokio::test(start_paused = true)]
async fn ping_interval_not_shorter_than_idle_timeout_still_hits_idle_timeout() {
    let idle_timeout = Duration::from_secs(5);
    let interval = Duration::from_secs(10);
    let pong_timeout = Duration::from_secs(2);
    let push_every = Duration::from_secs(1);
    let close_grace = Duration::from_secs(1);
    let push_count = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(Mutex::new(Vec::new()));
    let config = WebSocketConfig::default()
        .with_idle_timeout(idle_timeout)
        .with_ping_interval(interval, pong_timeout)
        .unwrap()
        .with_close_grace(close_grace)
        .with_handler(KeepaliveRecorder {
            push_every: Some(push_every),
            push_count: push_count.clone(),
            closed: closed.clone(),
        });
    let (mut client, server_task) = handshake(config).await;

    let mut pings = 0usize;
    let per_read_timeout = idle_timeout * 3;
    let close_frame = loop {
        let msg = tokio::time::timeout(per_read_timeout, client.next())
            .await
            .expect("must not hang waiting for push or close")
            .expect("stream should yield a message")
            .expect("no protocol error");
        match msg {
            Message::Text(_) => {}
            Message::Ping(_) => pings += 1,
            Message::Close(frame) => break frame,
            other => panic!("unexpected frame: {other:?}"),
        }
    };

    assert_eq!(
        pings, 0,
        "interval (10s) >= idle_timeout (5s) is a misconfiguration: idle_timeout must fire \
         before the first Ping is ever sent"
    );
    if let Some(frame) = close_frame {
        assert_eq!(frame.code, CloseCode::Normal);
    }

    let _ = client.next().await;

    let result = tokio::time::timeout(
        idle_timeout + close_grace + Duration::from_secs(5),
        server_task,
    )
    .await
    .expect("server task must not hang")
    .unwrap();
    assert!(
        result.is_ok(),
        "idle timeout should end the session normally: {result:?}"
    );
    assert_eq!(
        closed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &[CloseReason::IdleTimeout],
        "on_close must report IdleTimeout, confirming the misconfiguration warning"
    );
}
