//! E2E テスト（イシュー #672、親 #669「サーバー起点で任意タイミングに push
//! できる WebSocket API」の最終段）: `WsSender`（#670）+
//! `WsMessageHandler::on_open`（#671）で確立した「サーバー起点 push」経路の
//! 受け入れ基準 3 項目を検証する。
//!
//! `handler_e2e.rs` ケース 8〜10（#671）は単発 push・`on_open` の呼び出し
//! 回数・ハンドシェイク失敗時に呼ばれないことを検証済みで、本ファイルとは
//! 重複しない。本ファイルは以下を検証する:
//!
//! 1. 受信がない状態でのサーバー push を、クライアントが**複数回**受信できる
//! 2. コマンドへの応答（`WsOutcome::Reply`）とイベントの push を交互に大量に
//!    送っても、両カテゴリのフレームが順序どおり・欠落なく届く（破損しない）
//! 3. 接続を切断・キャンセルした後に `WsSender::send` を呼ぶと、有界時間内に
//!    エラーが返る（panic しない・無期限ブロックしない）

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome, WsSender,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// 有効な `GET /ws` アップグレードリクエストの生バイト列
/// （`handler_e2e.rs` / `cancellation.rs` と同一のテスト用固定リクエスト）。
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
/// （`handler_e2e.rs` と同一のヘルパー）。
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

/// テスト共通: ハンドシェイクを成立させ、クライアント側 `WebSocketStream` と
/// サーバ側 `handle_upgrade` タスクを返す（`handler_e2e.rs` と同一実装、
/// キャンセルは `std::future::pending` で無期限 pending にする）。
async fn spawn_session(
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

    let client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    (client, server_task)
}

/// テスト共通: ハンドシェイクを成立させ、キャンセルトリガ（`oneshot`）を
/// 呼び出し側で握った状態でセッションを開始する（`cancellation.rs` と同一
/// パターン）。テストケース 4（キャンセル後の `WsSender::send`）専用。
async fn spawn_session_with_cancel(
    config: WebSocketConfig,
) -> (
    WebSocketStream<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), fandhe_backend_plugin_websocket::WsError>>,
    tokio::sync::oneshot::Sender<()>,
) {
    let head = match parse_request_head(handshake_request_bytes()).unwrap() {
        ParseOutcome::Complete { head, .. } => head,
        ParseOutcome::Incomplete => unreachable!(),
    };
    let (server_side, mut client_side) = tokio::io::duplex(64 * 1024);
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

    let server_task = tokio::spawn(async move {
        handle_upgrade(server_side, &head, Vec::new(), &config, async move {
            let _ = cancel_rx.await;
        })
        .await
    });

    let response = read_http_response_line(&mut client_side).await;
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    (client, server_task, cancel_tx)
}

/// 受け入れ基準 1: `on_open` で受け取った `WsSender` を使い、クライアントが
/// 一切送信しないまま複数件の push を順番に送出するハンドラ。
struct SequentialPushHandler {
    count: usize,
}

impl WsMessageHandler for SequentialPushHandler {
    fn name(&self) -> &'static str {
        "sequential-push"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        let sender = ctx.sender().clone();
        let count = self.count;
        tokio::spawn(async move {
            for i in 0..count {
                // セッション終了後は `send` が `Err` を返すため、以降のループを
                // 続けても無意味なタスクの空回りになる。即座に打ち切る。
                if sender
                    .send(WsMessage::Text(format!("push-{i}")))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

/// ケース 1（受け入れ基準 1）: 受信がない状態でのサーバー push を、クライア
/// ントが複数回・順序どおりに受信できる。
#[tokio::test]
async fn multiple_server_pushes_arrive_in_order_without_client_activity() {
    const PUSH_COUNT: usize = 5;
    let config =
        WebSocketConfig::default().with_handler(SequentialPushHandler { count: PUSH_COUNT });
    let (mut client, server_task) = spawn_session(config).await;

    for i in 0..PUSH_COUNT {
        let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("push should arrive within timeout")
            .expect("stream should not end")
            .expect("frame should not error");
        assert_eq!(msg, Message::Text(format!("push-{i}").into()));
    }

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// 受け入れ基準 2: `on_open` で push タスクを起動しつつ、`on_message` は
/// 受信した Text をそのまま `"echo:{...}"` として返す（コマンド応答と
/// イベント push が同一 `WebSocketStream` へ交互に混ざる状況を作る）。
struct InterleavedPushHandler {
    push_count: usize,
}

impl WsMessageHandler for InterleavedPushHandler {
    fn name(&self) -> &'static str {
        "interleaved-push"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        let sender = ctx.sender().clone();
        let push_count = self.push_count;
        tokio::spawn(async move {
            for i in 0..push_count {
                if sender
                    .send(WsMessage::Text(format!("push:{i}")))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move {
            let WsMessage::Text(t) = msg else {
                return Ok(WsOutcome::Reply(vec![]));
            };
            Ok(WsOutcome::Reply(vec![WsMessage::Text(format!("echo:{t}"))]))
        })
    }
}

/// [`interleaved_replies_and_pushes_do_not_corrupt_frames`] が受信フレームを
/// 到着順のまま分類するためのタグ（イシュー #672 レビュー対応。カテゴリ別
/// `Vec` へ振り分けるだけでは「全 push が先着し、その後に全 echo が続く」
/// という非交錯パターンでも通過してしまうため、到着順の全体列をまず記録し、
/// 交錯の有無を別途検証する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceivedFrame {
    Echo(usize),
    Push(usize),
}

/// ケース 2（受け入れ基準 2）: コマンドへの応答（echo）とイベント push を
/// 交互に大量送出してもフレームが壊れない。単一の送信元（本セッションの
/// `WebSocketStream` を排他的に所有する 1 タスク）が直列に送出するため、
/// 各カテゴリ内の順序は決定的である。欠落・重複・破損・並び替わりのすべて
/// を検出するため、到着順の全体列（`ReceivedFrame` の `Vec`）を記録し、
/// (a) カテゴリごとの部分列が欠落・重複・並び替わりなく元の順序を保つこと、
/// (b) 実際に両カテゴリが入り混じって届いたこと（一方が完全に先行し他方が
/// 完全に後続する退化パターンでないこと）の両方を検証する。
///
/// (b) を確実にするため、クライアントは「コマンドを 1 件送信し、その
/// echo が返るまで受信を続ける（途中に挟まる push フレームは収集しつつ
/// 読み飛ばす）」というリクエスト・レスポンス相関を 1 件ずつ繰り返す
/// （イシュー #672 レビュー対応）。相関なしに「1 件送信 → 1 件受信」を
/// 機械的に繰り返すだけの設計では、実測上（本テストのデバッグ実行で
/// 確認済み）on_open が起動する push タスクが `WsSender` の bounded
/// channel（容量 8）を継続的に再充填し続け、`run_session` の受信ループが
/// 300 件の push をすべて出し切るまで対応する echo を 1 件も返さない
/// （outbound チャネルが Ready な限り優先され続ける）挙動が再現し、結局
/// 「push 300 件 → echo 300 件」という 1 ブロックの退化パターンに終始
/// した。対応する echo を待ってから次のコマンドを送る相関設計にすることで、
/// 各コマンドの往復ごとに「送信 → （0 件以上の push）→ 対応する echo」の
/// サイクルが強制され、300 往復にわたって push が echo の間へ実際に
/// 混在する状況を作る。
///
/// なお、相関設計のみでは既定の `#[tokio::test]`（current-thread フレーバー）
/// 下でも退化パターンが実測で再現した（cooperative スケジューラ下では
/// outbound チャネルの `recv()` が `ws.next()` の初回 poll より先に確実に
/// Ready 化するため、`run_session` の反復ごとの優先順反転があっても
/// outbound 側が実質的に勝ち続けた）。`flavor = "multi_thread"` に
/// することで push タスク・セッションタスク・本テストタスクが実際の
/// OS スレッド間で並行実行されるようになり、退化が再現しないことを
/// 実測で確認した。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interleaved_replies_and_pushes_do_not_corrupt_frames() {
    const PUSH_COUNT: usize = 300;
    const CMD_COUNT: usize = 300;
    // カテゴリの切り替わり回数（`order` を先頭から走査して隣接要素の
    // 判別子が変化した回数）がこの値未満なら、実質的に「1 ブロックずつ」
    // としか届いておらず交錯を検証できていないとみなす。相関設計により
    // 300 往復それぞれで push が混在しうるため、通常は数十〜数百回の
    // 切り替わりが発生する。ごく少数の切り替わりでも「交互に送る状況」
    // 自体は成立するため、しきい値は「1 ブロックの退化パターンではない」
    // ことを検出できる最小限の保守的な値に留める。
    const MIN_CATEGORY_TRANSITIONS: usize = 4;

    let config = WebSocketConfig::default().with_handler(InterleavedPushHandler {
        push_count: PUSH_COUNT,
    });
    let (mut client, server_task) = spawn_session(config).await;

    let mut order: Vec<ReceivedFrame> = Vec::with_capacity(CMD_COUNT + PUSH_COUNT);

    let collect = async {
        // 受信 1 フレームを分類して `order` へ積む（echo/push いずれかの
        // ペイロード以外は破損とみなして panic する）。
        async fn recv_one(
            client: &mut WebSocketStream<tokio::io::DuplexStream>,
            order: &mut Vec<ReceivedFrame>,
        ) -> ReceivedFrame {
            let frame = client
                .next()
                .await
                .expect("stream should not end before all frames arrive")
                .expect("frame should not error");
            let Message::Text(text) = frame else {
                panic!("unexpected non-text frame: {frame:?}");
            };
            let text = text.to_string();
            let received = if let Some(rest) = text.strip_prefix("echo:") {
                let idx: usize = rest
                    .strip_prefix("cmd:")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| panic!("unexpected echo payload: {text}"));
                ReceivedFrame::Echo(idx)
            } else if let Some(rest) = text.strip_prefix("push:") {
                let idx: usize = rest
                    .parse()
                    .unwrap_or_else(|_| panic!("unexpected push payload: {text}"));
                ReceivedFrame::Push(idx)
            } else {
                panic!("unexpected frame payload (possible corruption): {text}");
            };
            order.push(received);
            received
        }

        // 各コマンドを送信し、対応する echo が届くまで受信を続ける
        // （途中に挟まる push は `order` へ記録しつつ読み飛ばす）。
        for i in 0..CMD_COUNT {
            client
                .send(Message::Text(format!("cmd:{i}").into()))
                .await
                .expect("send command");
            loop {
                match recv_one(&mut client, &mut order).await {
                    ReceivedFrame::Echo(idx) => {
                        assert_eq!(idx, i, "echo must correspond to the most recently sent cmd");
                        break;
                    }
                    ReceivedFrame::Push(_) => {}
                }
            }
        }

        // 全コマンド送信・対応 echo 受信が完了した後、on_open が起動した
        // push タスクの残り（往復の合間に届かなかった分）を出し切るまで
        // 受信を続ける。
        while order.len() < CMD_COUNT + PUSH_COUNT {
            recv_one(&mut client, &mut order).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), collect)
        .await
        .expect("all frames should arrive within timeout");

    let echoes: Vec<usize> = order
        .iter()
        .filter_map(|f| match f {
            ReceivedFrame::Echo(i) => Some(*i),
            ReceivedFrame::Push(_) => None,
        })
        .collect();
    let pushes: Vec<usize> = order
        .iter()
        .filter_map(|f| match f {
            ReceivedFrame::Push(i) => Some(*i),
            ReceivedFrame::Echo(_) => None,
        })
        .collect();
    let expected_echoes: Vec<usize> = (0..CMD_COUNT).collect();
    let expected_pushes: Vec<usize> = (0..PUSH_COUNT).collect();
    assert_eq!(
        echoes, expected_echoes,
        "echo replies must arrive in order without loss or duplication"
    );
    assert_eq!(
        pushes, expected_pushes,
        "pushes must arrive in order without loss or duplication"
    );

    // 受け入れ基準 2 が要求する「交互に送る状況」そのものの検証: 到着順の
    // 全体列でカテゴリが実際に何度も入れ替わったことを確認する。これが
    // ゼロ・僅少のままだと、上記 2 つの `assert_eq!` は「push が全件先着し、
    // その後に echo が全件到着する」だけの非交錯ケースでも通過してしまい、
    // 交互送出時の破損有無という本来の検証意図を満たさない。
    let category_transitions = order
        .windows(2)
        .filter(|pair| std::mem::discriminant(&pair[0]) != std::mem::discriminant(&pair[1]))
        .count();
    assert!(
        category_transitions >= MIN_CATEGORY_TRANSITIONS,
        "echoes and pushes must actually interleave on the wire (got {category_transitions} \
         category transitions, expected at least {MIN_CATEGORY_TRANSITIONS}); a low count means \
         one category arrived as a single contiguous block before the other, which does not \
         exercise interleaved send/receive at all"
    );

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");
}

/// 受け入れ基準 3 用: `on_open` で受け取った `WsSender` を外部の `slot` へ
/// 保存するだけのハンドラ（テストコードから直接 `send` を呼び、セッション
/// 終了後の挙動を検証するために使う）。
struct CaptureSenderHandler {
    slot: Arc<Mutex<Option<WsSender>>>,
}

impl WsMessageHandler for CaptureSenderHandler {
    fn name(&self) -> &'static str {
        "capture-sender"
    }

    fn on_open(&self, ctx: WsOpenContext) {
        *self.slot.lock().unwrap() = Some(ctx.sender().clone());
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

/// ケース 3（受け入れ基準 3・前半）: クライアント切断でセッションが終了した
/// 後に `WsSender::send`（および clone）を呼ぶと、無期限ブロック・panic
/// せず有界時間内に `Err` を返す。
#[tokio::test]
async fn send_after_client_disconnect_returns_error_without_panicking() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(CaptureSenderHandler {
        slot: Arc::clone(&slot),
    });
    let (mut client, server_task) = spawn_session(config).await;

    client.close(None).await.expect("close");
    let result = server_task.await.unwrap();
    assert!(result.is_ok(), "session should end cleanly: {result:?}");

    // ここに到達した時点で `handle_upgrade` は完了しており、`on_open` は
    // セッション確立時に必ず呼ばれているため `slot` は `Some` のはず
    // （`on_open` が呼ばれなかった場合に静かにテストが無意味化しないための
    // ガード）。
    let sender = slot
        .lock()
        .unwrap()
        .take()
        .expect("on_open must have been called for an established session");

    // 保持していた `WsSender` 自体、およびそのクローン（長寿命タスクが
    // clone を保持し続ける想定を模す）の双方で送信を試み、いずれも即座に
    // エラーが返ることを確認する。
    let cloned = sender.clone();
    for s in [sender, cloned] {
        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            s.send(WsMessage::Text("late".to_string())),
        )
        .await
        .expect("send must not hang after session end");
        assert!(
            outcome.is_err(),
            "send after session end must return WsSendError, not succeed"
        );
    }
}

/// ケース 4（受け入れ基準 3・後半）: 世代キャンセル発火によりセッションが
/// 終了した後に `WsSender::send` を呼んでも、有界時間内に `Err` を返す
/// （`cancellation.rs` のキャンセルトリガ駆動パターンを流用）。
#[tokio::test]
async fn send_after_cancellation_returns_error_without_panicking() {
    let slot: Arc<Mutex<Option<WsSender>>> = Arc::new(Mutex::new(None));
    let config = WebSocketConfig::default().with_handler(CaptureSenderHandler {
        slot: Arc::clone(&slot),
    });
    let (mut client, server_task, cancel_tx) = spawn_session_with_cancel(config).await;

    cancel_tx.send(()).expect("cancel receiver must be alive");

    // サーバは Close フレーム（1001 Going Away）を送出する。クライアント側
    // の tokio-tungstenite は Close 受信時に自動で Close 応答を返す。
    let closed = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("close frame should arrive before test timeout")
        .expect("stream should yield a message")
        .expect("no protocol error");
    match closed {
        Message::Close(Some(frame)) => assert_eq!(frame.code, CloseCode::Away),
        other => panic!("expected Close(Some(1001 Away)), got {other:?}"),
    }
    // 応答送出を駆動する（`cancellation.rs` と同じ二段構えの `next()` 呼び
    // 出しパターン）。
    let _ = client.next().await;

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should finish within grace period")
        .unwrap();
    assert!(
        result.is_ok(),
        "cancellation should end the session normally: {result:?}"
    );

    let sender = slot
        .lock()
        .unwrap()
        .take()
        .expect("on_open must have been called for an established session");

    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        sender.send(WsMessage::Text("late".to_string())),
    )
    .await
    .expect("send must not hang after cancellation");
    assert!(
        outcome.is_err(),
        "send after cancellation must return WsSendError, not succeed"
    );
}
