//! サーバー起点 Ping keepalive の統合テスト（イシュー #713、親 #712）。
//!
//! `idle_timeout.rs` / `on_close_e2e.rs` と同様、`tokio::io::duplex` +
//! `tokio-tungstenite` クライアントで `handle_upgrade` を実際に駆動する。
//! `WebSocketConfig::with_ping_interval` は `idle_timeout` とは独立した
//! 死活監視のため、`idle_timeout` を無効化した構成で Ping keepalive 単体の
//! 挙動を検証する（両者を組み合わせた場合の推奨設定・doc・組み合わせ
//! テストは `tests/idle_keepalive_e2e.rs`（イシュー #714）を参照）。
//!
//! 実時間ではなく仮想時間（`#[tokio::test(start_paused = true)]`）で駆動し、
//! テスト自体が実時間で待たずに決定的に終わることを保証する。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    CloseReason, WsConnContext, WsHandlerError, WsMessage, WsMessageHandler, WsOutcome,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列
/// （`idle_timeout.rs` と同一のリクエスト）。
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
/// （`idle_timeout.rs` と同一のヘルパー）。
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
/// （`idle_timeout.rs::handshake` と同一のヘルパー）。
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
/// `WebSocketStream`（tokio-tungstenite）へ包まず生の `DuplexStream` の
/// ままにして返す。tokio-tungstenite のクライアント実装は Ping を読むと
/// 自動で Pong を送出する（`tungstenite::protocol::WebSocket::read` の
/// 既定動作）ため、「Pong を一切返さない対向」を検証するテストではこの
/// 自動応答が交絡要因になる。生バイトで読み書きすることでこれを避ける
/// （[`build_masked_text_frame`] / [`read_raw_frame`] と組み合わせて使う）。
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

/// RFC 6455 準拠のマスク付き Text フレームを生バイト列として構築する
/// （クライアント→サーバー方向のフレームは必ずマスクされる。テスト専用の
/// 最小実装で、payload は 125 バイト以内・拡張ペイロード長は扱わない）。
fn build_masked_text_frame(payload: &str) -> Vec<u8> {
    build_masked_control_frame(0x1, payload.as_bytes())
}

/// RFC 6455 準拠のマスク付き Pong フレームを生バイト列として構築する
/// （[`build_masked_text_frame`] と同型。tokio-tungstenite クライアントの
/// 自動 Pong 応答を経由せず、テストから明示的に 1 個だけ Pong を送るために
/// 使う）。`payload` はサーバーが Pong 期限解除の一致確認に使う識別
/// ペイロード（イシュー #713 レビュー指摘対応）で、一致させたい対象の
/// Ping フレームから読み取った値をそのまま渡す。
fn build_masked_pong_frame(payload: &[u8]) -> Vec<u8> {
    build_masked_control_frame(0xa, payload)
}

/// [`build_masked_text_frame`] / [`build_masked_pong_frame`] の共通実装。
/// `opcode` は下位 4 ビットのみ使う（FIN=1 固定、テスト専用の最小実装）。
fn build_masked_control_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    assert!(
        payload.len() <= 125,
        "test helper only supports short payloads (no extended length)"
    );
    // マスクキーは固定値（テストの決定性のために乱数を使わない。RFC 6455 は
    // マスクキーの予測可能性を暗号学的に問題視しないクライアント実装を
    // 禁じていない——サーバーはどのマスクキーでも受理する）。
    let mask: [u8; 4] = [0x12, 0x34, 0x56, 0x78];
    let mut frame = Vec::with_capacity(2 + mask.len() + payload.len());
    frame.push(0x80 | (opcode & 0x0f)); // FIN=1, opcode
    frame.push(0x80 | payload.len() as u8); // MASK=1, payload len
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    frame
}

/// サーバー→クライアント方向の 1 フレームを生バイトで読み、opcode と
/// payload を返す（サーバー送信フレームはマスクされない、RFC 6455 5.1 節。
/// テスト専用の最小実装で、拡張ペイロード長は扱わない）。読むだけで、
/// Ping を検出しても Pong を送出しない（[`handshake_raw`] の doc を参照）。
async fn read_raw_frame<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> (u8, Vec<u8>) {
    let mut header = [0u8; 2];
    stream
        .read_exact(&mut header)
        .await
        .expect("read frame header");
    let opcode = header[0] & 0x0f;
    assert_eq!(
        header[1] & 0x80,
        0,
        "server frames must not be masked (RFC 6455 5.1 節)"
    );
    let len = usize::from(header[1] & 0x7f);
    assert!(
        len <= 125,
        "test helper only supports short payloads (no extended length)"
    );
    let mut payload = vec![0u8; len];
    if len > 0 {
        stream
            .read_exact(&mut payload)
            .await
            .expect("read frame payload");
    }
    (opcode, payload)
}

/// `on_close` で通知された `CloseReason` を記録するだけのトイハンドラ
/// （`EchoHandler` と同じくメッセージはそのまま返送する。`on_close_e2e.rs`
/// の `Recording`（`shared: Arc<Shared>` を保持し呼び出し元へも同じ
/// `Arc` を渡すパターン）を踏襲する。`WebSocketConfig::with_handler` は
/// ハンドラを値で受け取り内部で `Arc<dyn WsMessageHandler>` へ包むため、
/// 呼び出し元が結果を読み取るには、ハンドラ自身ではなく内部の共有状態を
/// `Arc` で持たせて clone を手元に残す必要がある）。
struct RecordClose {
    reason: Arc<Mutex<Option<CloseReason>>>,
}

impl WsMessageHandler for RecordClose {
    fn name(&self) -> &'static str {
        "record-close"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }

    fn on_close(&self, _ctx: &WsConnContext, reason: CloseReason) {
        *self
            .reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reason);
    }
}

/// Text 受信ごとに `delay` だけ `await` してから同じ内容を返送するハンドラ。
/// ハンドラ実行中に届いたフレームは、ハンドラが完了して次の受信待ちに戻る
/// までサーバーが読まないことを検証するために使う（`crate::session` モジュール
/// doc「サーバー起点 Ping keepalive」節）。
struct SlowEcho {
    delay: Duration,
}

impl WsMessageHandler for SlowEcho {
    fn name(&self) -> &'static str {
        "slow-echo"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        let delay = self.delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Ok(WsOutcome::Reply(vec![msg]))
        })
    }
}

/// 受け入れ基準 4（既定は無効）: `with_ping_interval` を呼ばない既定構成
/// では、長い仮想時間を進めても Ping が 1 つも送出されないこと。
#[tokio::test(start_paused = true)]
async fn disabled_by_default_sends_no_ping() {
    let config = WebSocketConfig::default().without_idle_timeout();
    let (mut client, server_task) = handshake(config).await;

    // 通常の keepalive 間隔として想定される値を大きく超える期間、何も
    // 届かないことを確認する（有界な `timeout` で、テスト自体が無期限に
    // ハングしないようにする）。
    let outcome = tokio::time::timeout(Duration::from_secs(120), client.next()).await;
    assert!(
        outcome.is_err(),
        "disabled ping keepalive must not send anything even after a long idle period"
    );

    client.close(None).await.expect("close");
    let result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server task should finish after client-initiated close")
        .unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// 受け入れ基準 1・3: 有効化すると `interval` ごとに Ping が届き、
/// クライアントが読み続けて（tungstenite が自動で Pong を返す）いる限り、
/// `pong_timeout` の何倍もの時間が経過しても接続が維持されること。
#[tokio::test(start_paused = true)]
async fn client_reading_keeps_connection_alive_across_many_intervals() {
    let interval = Duration::from_secs(10);
    let pong_timeout = Duration::from_secs(5);
    let config = WebSocketConfig::default()
        .without_idle_timeout()
        .with_ping_interval(interval, pong_timeout)
        .unwrap();
    let (mut client, server_task) = handshake(config).await;

    // interval の 5 倍を超える期間、Ping を受け取り続けられること
    // （tokio-tungstenite クライアントは `next()` 呼び出し時に Ping へ
    // 自動で Pong を返す）。
    for i in 0..5 {
        let received = tokio::time::timeout(interval * 2, client.next())
            .await
            .unwrap_or_else(|_| panic!("ping #{i} should arrive within 2x interval"))
            .expect("stream should yield a message")
            .expect("no protocol error");
        assert!(
            matches!(received, Message::Ping(_)),
            "expected Ping frame, got {received:?}"
        );
    }

    client.close(None).await.expect("close");
    let result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server task should finish")
        .unwrap();
    assert!(
        result.is_ok(),
        "session kept alive by ping/pong should end cleanly on client close: {result:?}"
    );
}

/// 受け入れ基準 2: Ping 送出後 `pong_timeout` 以内に Pong が届かない
/// クライアント（読み取りを止めている）は切断され、`on_close` が
/// `CloseReason::PongTimeout` でちょうど 1 回呼ばれること。
///
/// クライアントは `client.next()` を一切呼ばないため、tungstenite の
/// 自動 Pong 応答（読み取り駆動）が発生せず、サーバー側の Pong 期限が
/// 確実に切れる。接続自体は `forget` して保持したままにし（drop による
/// EOF を発生させない、`idle_timeout.rs::server_terminates_even_if_client_
/// ignores_close` と同じ理由）、Pong 期限の発火のみで切断されることを
/// 確認する。
#[tokio::test(start_paused = true)]
async fn unresponsive_client_is_closed_with_pong_timeout() {
    let interval = Duration::from_millis(200);
    let pong_timeout = Duration::from_millis(100);
    // Close ハンドシェイクのドレインは既定 10 秒（`DEFAULT_CLOSE_GRACE`）を
    // 上限に応答を待つ。読み取りを止めたクライアント（本テスト）からは
    // 応答が来ないため、有界なテストにするには明示的に短い値へ変更する
    // 必要がある（`idle_timeout.rs::server_terminates_even_if_client_
    // ignores_close` と同じ理由）。
    let close_grace = Duration::from_millis(100);
    let reason = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default()
        .without_idle_timeout()
        .with_ping_interval(interval, pong_timeout)
        .unwrap()
        .with_close_grace(close_grace)
        .with_handler(RecordClose {
            reason: reason.clone(),
        });
    let (client, server_task) = handshake(config).await;

    std::mem::forget(client);

    let result = tokio::time::timeout(interval + pong_timeout + close_grace * 4, server_task)
        .await
        .expect("server must not hang: pong timeout + close_grace bound the wait")
        .unwrap();
    assert!(
        result.is_ok(),
        "pong timeout is policy-driven, not a protocol error: {result:?}"
    );
    assert_eq!(
        *reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        Some(CloseReason::PongTimeout),
        "on_close must report PongTimeout exactly once"
    );
}

/// PR #738 レビュー指摘（イシュー #713）の固定: 送出中の Ping と一致しない
/// payload の Pong は無視され、Pong 期限が解除されないこと。対向が
/// サーバーの Ping に一切応答せず、任意の（詐称した）Pong を送り続けても
/// `CloseReason::PongTimeout` で切断される契約を検証する
/// （`crate::session` モジュール doc「サーバー起点 Ping keepalive」節の
/// 「Pong 期限」を参照）。
///
/// tokio-tungstenite のクライアント（`WebSocketStream`）は Ping を読むと
/// 正しい payload で自動的に Pong を返してしまうため、任意の（間違った）
/// payload を送出するには生バイトで読み書きするクライアント
/// （[`handshake_raw`]）が必須。
#[tokio::test(start_paused = true)]
async fn pong_with_mismatched_payload_does_not_clear_pong_timeout() {
    const OPCODE_PING: u8 = 0x9;

    let interval = Duration::from_millis(200);
    let pong_timeout = Duration::from_millis(100);
    let close_grace = Duration::from_millis(100);
    let reason = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default()
        .without_idle_timeout()
        .with_ping_interval(interval, pong_timeout)
        .unwrap()
        .with_close_grace(close_grace)
        .with_handler(RecordClose {
            reason: reason.clone(),
        });
    let (mut client, server_task) = handshake_raw(config).await;

    // 送出された Ping の payload を読み取り、それとは異なる payload の
    // Pong を組み立てる（unsolicited・詐称の再現）。
    let (opcode, ping_payload) = tokio::time::timeout(interval * 2, read_raw_frame(&mut client))
        .await
        .expect("first ping should arrive");
    assert_eq!(opcode, OPCODE_PING, "expected Ping frame");

    let mut wrong_payload = ping_payload.clone();
    if wrong_payload.is_empty() {
        wrong_payload.push(0);
    } else {
        wrong_payload[0] ^= 0xff;
    }
    assert_ne!(
        wrong_payload, ping_payload,
        "test payload must actually differ from the real ping payload"
    );

    client
        .write_all(&build_masked_pong_frame(&wrong_payload))
        .await
        .expect("client write should succeed");

    let result = tokio::time::timeout(interval + pong_timeout + close_grace * 4, server_task)
        .await
        .expect("server must not hang: pong timeout + close_grace bound the wait")
        .unwrap();
    assert!(
        result.is_ok(),
        "pong timeout is policy-driven, not a protocol error: {result:?}"
    );
    assert_eq!(
        *reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        Some(CloseReason::PongTimeout),
        "a Pong with a mismatched payload must not clear the pong deadline"
    );
}

/// 受け入れ基準 2・3 の境界事例: `pong_timeout` より長く実行されるハンドラの
/// 実行中に Text → Pong の順で届いていた場合でも、誤って `PongTimeout` に
/// しないこと。
///
/// `crate::session` モジュール doc「サーバー起点 Ping keepalive」節が述べる
/// とおり、受信は本モジュール内の 1 か所で逐次処理するため、ハンドラ実行中に
/// 届いた Pong はハンドラが完了して次の受信待ちに戻るまで読まれない。この間
/// Pong 期限（送出済み Ping への応答期限）を過ぎていても、`ws.next()` を優先
/// する契約により、ハンドラ完了直後に読まれるバッファ済みの Pong で期限は
/// 解除され、誤切断しない。
///
/// tokio-tungstenite のクライアントは Ping を読むと自動で Pong を返す
/// （`tungstenite::protocol::WebSocket::read` の doc。次に `read`/`write`/
/// `flush` を呼んだ時点で実際に送出される）ため、`WebSocketStream` を使うと
/// 「どちらの Pong（自動応答／明示送出）が期限を解除したか」が曖昧になる
/// （このクライアントで Ping を読んだ後に別のフレームを送ると、その送出に
/// 相乗りして自動 Pong が先に flush され、続けて明示 Pong を送ると 2 個目の
/// 独立した Pong になってしまう）。本テストは Pong を厳密に 1 個だけ送出
/// して検証したいため、生バイトで読み書きする [`handshake_raw`] を使う。
#[tokio::test(start_paused = true)]
async fn slow_handler_does_not_lose_pong_buffered_during_handler_execution() {
    const OPCODE_TEXT: u8 = 0x1;
    const OPCODE_PING: u8 = 0x9;

    let interval = Duration::from_millis(200);
    let pong_timeout = Duration::from_millis(100);
    let handler_delay = Duration::from_millis(500);
    let config = WebSocketConfig::default()
        .without_idle_timeout()
        .with_ping_interval(interval, pong_timeout)
        .unwrap()
        .with_handler(SlowEcho {
            delay: handler_delay,
        });
    let (mut client, server_task) = handshake_raw(config).await;

    // 最初の Ping を受け取り、未応答の Ping（Pong 期限）が立った状態にする。
    // payload はサーバーが Pong 期限解除の一致確認に使う識別ペイロード
    // （イシュー #713 レビュー指摘対応）で、この Ping への正当な応答として
    // 認識させるため後続の明示 Pong にそのまま使う。
    let (opcode, payload) = tokio::time::timeout(interval * 2, read_raw_frame(&mut client))
        .await
        .expect("first ping should arrive");
    assert_eq!(opcode, OPCODE_PING, "expected Ping frame");

    // Text を送ってハンドラ（500ms スリープ）を起動した直後、サーバーが
    // それを読み切ってから戻ってくるまでの間に、Pong を明示的に 1 個だけ
    // 送る（読まれない、が自動応答は発生しない）。Pong 期限（100ms）は
    // ハンドラのスリープ中に過ぎるが、サーバーはハンドラ完了後の次の受信
    // 待ちでこの Pong をまず読むため、誤切断してはならない。
    client
        .write_all(&build_masked_text_frame("hi"))
        .await
        .expect("client write should succeed");
    client
        .write_all(&build_masked_pong_frame(&payload))
        .await
        .expect("client write should succeed");

    let (opcode, payload) = tokio::time::timeout(handler_delay * 4, read_raw_frame(&mut client))
        .await
        .expect(
            "handler should finish and reply within a bounded time \
                 (a premature close would end the stream instead)",
        );
    assert_eq!(
        opcode, OPCODE_TEXT,
        "expected the echoed reply (a premature PongTimeout close would not reply)"
    );
    assert_eq!(payload, b"hi", "expected the echoed reply payload");

    // 生クライアントで正規の Close ハンドシェイクを組み立てるのは複雑な
    // ため、drop で終える（EOF・TCP リセット相当の終了理由になる。本テスト
    // の検証対象はハンドラ完了直後のエコー到達までで、後続の終了経路は
    // 対象外）。
    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(5), server_task).await;
}

/// 受け入れ基準 2 の境界事例（「Pong 期限」判定の固定）: 未応答の Ping
/// （Pong 期限）が立った状態でも、Pong を送らず Text だけを送り続ける限り
/// クライアントは切断されないこと。
///
/// 期限を過ぎても読めるフレームが残っている間は判定に至らず、受信待ちで
/// 読めるフレームがなくなった時点で切断が確定する契約のため、Text を
/// 途切れず届けている間は切断されない（意図した挙動、`crate::session`
/// モジュール doc を参照）。
///
/// tokio-tungstenite のクライアント（`WebSocketStream`）は Ping を読むと
/// 自動で Pong を返してしまうため、「Pong を返さない対向」を検証するには
/// 生バイトで読み書きするクライアント（[`handshake_raw`]）が必須。
///
/// `start_paused` の仮想時計は runnable なタスクがある限り進まないため、
/// Text の送受信だけを繰り返すループでは `next_ping_at` に到達せず Ping が
/// 1 回も送出されない「空振り」になりうる。これを避けるため、各往復では
/// **先に Text を書き込み、そのあとで sleep する**（書き込みはサーバー側の
/// 読み取りタスクを即座に起こすため、直前までに書き込んだフレームは
/// Pending にならず読める）。
///
/// **`pong_timeout` を安全側の余裕を持って設定する**: Pong 期限は最初の
/// Ping 送出時刻に固定され、Pong が来ない限り更新されない
/// （`crate::session::Keepalive::pending`）。仮想時計は明示的な
/// `sleep`/`advance` でのみ進み、バッファ済みフレームの排出自体は仮想時間を
/// 消費しないため、「Pong 期限を過ぎた瞬間に受信待ちが空だと即座に切断
/// される」という契約上、往復の間隔（累積 sleep）が Pong 期限に迫る/超える
/// 構成では往復のタイミング次第で切断されうる（これは実装のバグではなく
/// 契約どおりの挙動——`idle_timeout` と同型の DoS 対策）。本テストは
/// `pong_timeout` を往復回数分の累積間隔より十分大きく取り、Pong 期限に
/// 迫らない範囲で「Ping 送出後も Pong なしで Text 往復を継続できる」ことを
/// 検証する。
#[tokio::test(start_paused = true)]
async fn client_sending_text_without_pong_is_not_disconnected() {
    const OPCODE_TEXT: u8 = 0x1;
    const OPCODE_PING: u8 = 0x9;

    let interval = Duration::from_millis(20);
    let pong_timeout = Duration::from_millis(500);
    let round_gap = Duration::from_millis(10);
    let rounds: u32 = 12;
    // 累積 sleep（10ms × 12 = 120ms）が interval（20ms）を跨いで実際に
    // Ping が送出されることを保証しつつ、Pong 期限（最初の Ping 送出時刻 +
    // 500ms）には遠く及ばない範囲に収める（上の doc を参照）。
    assert!(
        round_gap * rounds + interval < pong_timeout / 2,
        "test parameters must stay well clear of the fixed pong_deadline"
    );
    let config = WebSocketConfig::default()
        .without_idle_timeout()
        .with_ping_interval(interval, pong_timeout)
        .unwrap();
    let (mut client, server_task) = handshake_raw(config).await;

    let mut saw_ping = false;
    for i in 0..rounds {
        client
            .write_all(&build_masked_text_frame(&format!("msg-{i}")))
            .await
            .expect("client write should succeed");
        tokio::time::sleep(round_gap).await;

        // エコー（Text）が届くまで、間に挟まる Ping フレーム（Pong を返さ
        // ないため最大 1 回だけ観測される）は読み飛ばす。
        loop {
            let (opcode, payload) =
                tokio::time::timeout(round_gap * 10, read_raw_frame(&mut client))
                    .await
                    .unwrap_or_else(|_| {
                        panic!("echo #{i} should arrive (must not be disconnected)")
                    });
            match opcode {
                OPCODE_PING => {
                    saw_ping = true;
                }
                OPCODE_TEXT => {
                    assert_eq!(
                        payload,
                        format!("msg-{i}").into_bytes(),
                        "expected echoed text #{i}"
                    );
                    break;
                }
                other => panic!(
                    "unexpected opcode {other:#x} while waiting for echo #{i} \
                     (a premature close would appear here)"
                ),
            }
        }
    }

    assert!(
        saw_ping,
        "the keepalive must actually send at least one Ping during the loop \
         (otherwise this test would pass even if Ping sending were entirely broken)"
    );

    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(5), server_task).await;
}
