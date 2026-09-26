//! 2 クライアント同時接続 e2e テスト（イシュー #707、親 #702）。
//!
//! `on_close_e2e.rs`（単一接続の全終了経路網羅）・`server_push_e2e.rs` /
//! `handler_push_ordering_e2e.rs`（単一接続の push・順序保証）はいずれも
//! 1 接続単位の検証に留まる。本ファイルは、1 つの [`WsMessageHandler`]
//! インスタンスを複数接続が共有する実運用形（CDP 互換サーバーの
//! `/devtools/page/{id}` に複数タブが同時接続する構成、`docs/design/
//! ws-connection-context-and-close.md` 11 節が示す想定利用パターン「案 B」）
//! で、接続ごとの状態・サーバー起点 push・切断通知が他方の接続へ混入しない
//! ことを検証する。
//!
//! 受け入れ基準との対応:
//!
//! - 基準 1（状態・push の分離）→
//!   `two_clients_isolated_state_and_push_then_client_close_only_fires_own_on_close`
//!   の手順 1〜5
//! - 基準 2（片方切断時に自分の `on_close` だけが呼ばれ他方は継続）→
//!   同テストの手順 6〜9、および EOF 経路の
//!   `two_clients_eof_on_one_only_fires_its_on_close_and_other_continues`
//! - 基準 3（3 OS CI 通過）→ 本ファイルはインメモリ `tokio::io::duplex` のみを
//!   使い、ソケット・ファイルシステム・シンボリックリンク等の OS 依存 API を
//!   使わない。タイミング依存の否定アサーションも使わない（後述）ため、
//!   `ci.yml` の 3 OS matrix（ubuntu-latest/macos-latest/windows-latest）で
//!   差異なく通る想定
//!
//! # 同期方針（sleep 不使用・フレーク対策）
//!
//! - `sleep` を使わず、すべての待機を `tokio::time::timeout` 付きの実イベント
//!   待ち（101 応答の読み取り・エコー応答の到着・`JoinHandle` の join）で行う
//!   （`on_close_e2e.rs` と同一方針）
//! - 101 応答を読めただけでは `on_open` の実行完了は保証されない
//!   （`WsSender::closed` の doc test の注意と同じ）。各クライアントで
//!   1 往復エコーしてから状態（`conns`/`opens`）を参照する
//! - 「push が他方に届かない」という否定の検証はタイミング窓ではなく
//!   **順序バリア + 厳密ドレイン**の 2 段で行う: (1) A へ push → A で
//!   受信確認 → B で 1 往復し、B の次フレームが B 自身の返信であることを
//!   確認する（順序バリア。誤配送が次にアサーションするフレームより前に
//!   届けば直後の `assert_eq!` が検出する）。(2) Close 送出後のドレインは
//!   `drain_expect_only_close` で Close フレーム・EOF 以外を受信したら
//!   即座に `panic!` する（厳密ドレイン。誤配送が最後のアサーション後・
//!   Close ドレイン中に届いた場合でも、無言で読み捨てずに検出する）。
//!   両方を組み合わせることで、セッション終了までの全期間にわたり
//!   誤配送が「無言で消える」経路を残さない。ただし、クライアント
//!   Close 受信時にサーバー側が outbound チャネルの受信側を drop する
//!   実装（`session.rs` の `ClientClose` 経路）のため、Close 受信と
//!   同時刻にまだワイヤへ書き出されていない push はサーバー側で破棄され
//!   うる。この分はブラックボックステストの検出範囲外という既知の限界
//!   として残る
//! - `on_close` の判定はサーバタスクの `JoinHandle` を timeout 付きで
//!   join し終えた後に行う（`on_close` はセッションタスク内で同期に呼ばれる
//!   ため、join 完了時点で実行済みが保証される。`on_close_e2e.rs` と同方針）
//!
//! `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` を使い、
//! 2 セッションを実際に並行実行させて「同時接続」を実体化する。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    CloseReason, WsConnContext, WsConnId, WsHandlerError, WsMessage, WsMessageHandler,
    WsOpenContext, WsOutcome, WsSender,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, WsError, handle_upgrade};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

/// 有効な WebSocket アップグレードリクエストの生バイト列を `target` パスで
/// 構築する（`on_close_e2e.rs` 等と同じ固定 `Sec-WebSocket-Key`。
/// `tokio::io::duplex` は接続ごとに独立したペアなので、2 接続で同一キーを
/// 使い回してよい）。
fn handshake_request_bytes_for(target: &str) -> String {
    format!(
        "GET {target} HTTP/1.1\r\n\
         Host: example.com\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n"
    )
}

/// クライアント側ストリームから `\r\n\r\n` までを読み切る
/// （`on_close_e2e.rs` と同一のヘルパー）。
async fn read_http_response_line<S: AsyncRead + Unpin>(stream: &mut S) -> String {
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

/// 101 応答（`handshake::serialize_101`）を厳密に検証する共通ヘルパー
/// （AGENTS.md「アサーション網羅性」節。`on_close_e2e.rs` と同一の期待値、
/// 本ファイルの `handshake_request_bytes_for` が使う `Sec-WebSocket-Key` は
/// RFC 6455 4.2.2 の既知ベクタで固定のため `Sec-WebSocket-Accept` も固定
/// となる）。
fn assert_101_response(response: &str) {
    assert_eq!(
        response,
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\
         \r\n",
        "101 response must exactly match handshake::serialize_101's output \
         (status line + Upgrade/Connection/Sec-WebSocket-Accept headers, no body)"
    );
}

/// `handle_upgrade` を 1 接続分駆動し、クライアント側 `WebSocketStream` と
/// サーバタスクの `JoinHandle` を返す（`server_push_e2e.rs` の
/// `spawn_session` と同型。`target` ごとに独立した `duplex` ペアを使う）。
async fn spawn_session(
    config: WebSocketConfig,
    target: &str,
) -> (
    WebSocketStream<tokio::io::DuplexStream>,
    JoinHandle<Result<(), WsError>>,
) {
    let request = handshake_request_bytes_for(target);
    let head = match parse_request_head(request.as_bytes()).unwrap() {
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
    assert_101_response(&response);

    let client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    (client, server_task)
}

/// 接続ごとの状態（`on_open` で登録し `on_message_with_ctx` が更新する）。
struct ConnState {
    /// ハンドシェイク時のパスパラメータ `id`（`on_open` 時点でコピーして
    /// 保持。以降のメッセージ処理では `WsConnContext::param` 経由でも同じ
    /// 値が取れるが、`on_close` 後にも push 用ヘルパーから参照したいため
    /// `Shared` 側にも保持する）。
    page_id: String,
    /// サーバー起点 push 用の送信ハンドル（テストから `sender_for` 経由で
    /// clone して使う）。
    sender: WsSender,
    /// この接続で `on_message_with_ctx` が呼ばれた回数（接続ごとに独立して
    /// 増えることを検証する）。
    count: u32,
}

/// 複数接続で共有する記録用状態（`Registry` ハンドラが `Arc` で保持する）。
#[derive(Default)]
struct Shared {
    /// 接続 ID をキーにした接続単位状態（設計文書 11 節が示す想定実装
    /// パターン「案 B」: `Mutex<HashMap<WsConnId, _>>`）。
    conns: Mutex<HashMap<WsConnId, ConnState>>,
    /// `on_open` の呼び出し記録（`(接続 ID, page_id)`）。
    opens: Mutex<Vec<(WsConnId, String)>>,
    /// `on_close` の呼び出し記録（`(接続 ID, page_id, 終了理由)`）。
    closes: Mutex<Vec<(WsConnId, Option<String>, CloseReason)>>,
}

/// `Shared::conns` から `page_id` に対応する接続を探し、接続 ID と
/// `WsSender` の clone を返す（テストからサーバー起点 push を送るための
/// ヘルパー）。
fn sender_for(shared: &Shared, page_id: &str) -> (WsConnId, WsSender) {
    let conns = shared.conns.lock().unwrap();
    let (id, state) = conns
        .iter()
        .find(|(_, state)| state.page_id == page_id)
        .unwrap_or_else(|| panic!("no connection registered for page_id={page_id}"));
    (*id, state.sender.clone())
}

/// `/devtools/page/{id}` パターンへ登録する共有ハンドラ。1 つの
/// インスタンスを `Arc` で複数接続が共有し、`WsConnId` をキーに接続単位の
/// 状態を分離する（設計文書 11 節「案 B」の実装例）。
struct Registry {
    shared: Arc<Shared>,
}

impl WsMessageHandler for Registry {
    fn name(&self) -> &'static str {
        "multi-client-registry"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        // `on_message_with_ctx` をオーバーライドしているため実行時には
        // 呼ばれない。フォールバックとしてエコーのみ実装する
        // （trait 制約上、本メソッドにも実装が必要、handler.rs の doc 参照）。
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }

    fn on_open(&self, ctx: WsOpenContext) {
        let page_id = ctx.param("id").unwrap_or("").to_string();
        self.shared
            .opens
            .lock()
            .unwrap()
            .push((ctx.conn_id(), page_id.clone()));
        self.shared.conns.lock().unwrap().insert(
            ctx.conn_id(),
            ConnState {
                page_id,
                sender: ctx.sender().clone(),
                count: 0,
            },
        );
    }

    fn on_message_with_ctx<'a>(
        &'a self,
        ctx: &'a WsConnContext,
        msg: WsMessage,
    ) -> BoxFuture<'a, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move {
            let text = match msg {
                WsMessage::Text(t) => t,
                WsMessage::Binary(_) => "binary".to_string(),
            };
            // ハンドラ内では assert/panic しない（panic は on_close の
            // 保証外経路になり、失敗がハングやタイムアウトとして曖昧に
            // 現れるため）。不一致はクライアント側のアサーションで検出する。
            let mut conns = self.shared.conns.lock().unwrap();
            let Some(state) = conns.get_mut(&ctx.conn_id()) else {
                return Ok(WsOutcome::Reply(vec![WsMessage::Text(
                    "missing".to_string(),
                )]));
            };
            state.count += 1;
            let reply = format!(
                "{}|{}|{}|{}",
                ctx.param("id").unwrap_or(""),
                state.page_id,
                state.count,
                text
            );
            Ok(WsOutcome::Reply(vec![WsMessage::Text(reply)]))
        })
    }

    fn on_close(&self, ctx: &WsConnContext, reason: CloseReason) {
        let page_id = ctx.param("id").map(str::to_string);
        self.shared
            .closes
            .lock()
            .unwrap()
            .push((ctx.conn_id(), page_id, reason));
        self.shared.conns.lock().unwrap().remove(&ctx.conn_id());
    }
}

/// Close 送出後のドレインを厳密化するヘルパー（レビュー指摘対応、PR #734）。
///
/// 素朴な `while next().await.is_some() {}` は、Close 応答以外の任意の
/// フレーム（他方の接続へ誤配送された push 等）を無言で読み捨ててしまう。
/// 本ヘルパーは Close フレーム・EOF・`ConnectionClosed`/`AlreadyClosed`
/// のみを正常なドレインとして許容し、それ以外（push の誤配送を含む
/// Text/Binary 等）を受信したら即座に `panic!` して検出する。
///
/// これにより、push の誤配送を検出する範囲が「次にアサーションする
/// フレームが届くまで」から「セッション終了（Close ドレイン完了）まで」
/// 全体へ拡張される。ただし、サーバー側がクライアント Close 受信時に
/// outbound チャネルの受信側を drop する実装（`session.rs` の
/// `ClientClose` 経路）のため、Close 受信と同時刻にまだ書き出されていない
/// push はサーバー側で破棄されワイヤに現れない可能性があり、その分は
/// 本テストの検出範囲外（ブラックボックステストの既知の限界）である。
///
/// `Protocol(ResetWithoutClosingHandshake)` は、サーバーが `ClientClose`
/// 経路で自身の Close 応答を送らずにストリームを終端するため
/// tokio-tungstenite 0.30 で観測される正常系の主経路であり（
/// `on_close_e2e.rs` の `on_close_client_close_called_once` と同一の
/// 既知挙動、`docs/design/ws-connection-context-and-close.md` 4 節）、
/// push 誤配送とは無関係なので正常終了として扱う。
async fn drain_expect_only_close(client: &mut WebSocketStream<tokio::io::DuplexStream>) {
    loop {
        match client.next().await {
            None => break,
            Some(Ok(Message::Close(_))) => continue,
            Some(Ok(other)) => panic!(
                "unexpected frame while draining close handshake \
                 (possible push contamination from the other connection): {other:?}"
            ),
            Some(Err(
                tokio_tungstenite::tungstenite::Error::ConnectionClosed
                | tokio_tungstenite::tungstenite::Error::AlreadyClosed
                | tokio_tungstenite::tungstenite::Error::Protocol(
                    tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                ),
            )) => break,
            Some(Err(other)) => {
                panic!("unexpected error while draining close handshake: {other:?}")
            }
        }
    }
}

/// テスト用の 1 往復エコー: `text` を送って応答テキストを受け取る
/// （timeout 付き。`on_open` 実行完了の同期も兼ねる）。
async fn roundtrip(client: &mut WebSocketStream<tokio::io::DuplexStream>, text: &str) -> String {
    client
        .send(Message::Text(text.into()))
        .await
        .expect("send text");
    let msg = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("reply should arrive before test timeout")
        .expect("stream should not end")
        .expect("no error");
    match msg {
        Message::Text(t) => t.to_string(),
        other => panic!("expected text reply, got {other:?}"),
    }
}

/// テスト 1（受け入れ基準 1・2）: 2 クライアントが同時接続した状態で、
/// 状態・push が接続ごとに分離されること、片方の Close 切断時にその接続
/// についてのみ `on_close` が呼ばれ、他方は送受信・push を継続できること。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_isolated_state_and_push_then_client_close_only_fires_own_on_close() {
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default()
        .with_path_pattern("/devtools/page/{id}")
        .unwrap()
        .with_handler(Registry {
            shared: Arc::clone(&shared),
        });

    let (mut client_a, task_a) = spawn_session(config.clone(), "/devtools/page/A").await;
    let (mut client_b, task_b) = spawn_session(config.clone(), "/devtools/page/B").await;

    // 手順 2: 1 往復ずつ（on_open 完了の同期も兼ねる）。
    assert_eq!(roundtrip(&mut client_a, "hello").await, "A|A|1|hello");
    assert_eq!(roundtrip(&mut client_b, "hello").await, "B|B|1|hello");

    // 手順 3: opens がちょうど 2 件、接続 ID が異なり page_id が対応すること。
    let (id_a, id_b) = {
        let opens = shared.opens.lock().unwrap();
        assert_eq!(opens.len(), 2, "on_open should be called exactly twice");
        let id_a = opens
            .iter()
            .find(|(_, page)| page == "A")
            .map(|(id, _)| *id)
            .expect("page A should be registered");
        let id_b = opens
            .iter()
            .find(|(_, page)| page == "B")
            .map(|(id, _)| *id)
            .expect("page B should be registered");
        assert_ne!(id_a, id_b, "each connection must get a distinct WsConnId");
        (id_a, id_b)
    };

    // 手順 4: 交互送信で各返信のカウンタが接続ごとに独立していること。
    assert_eq!(roundtrip(&mut client_a, "m2").await, "A|A|2|m2");
    assert_eq!(roundtrip(&mut client_b, "m2").await, "B|B|2|m2");
    assert_eq!(roundtrip(&mut client_a, "m3").await, "A|A|3|m3");
    assert_eq!(roundtrip(&mut client_a, "m4").await, "A|A|4|m4");
    assert_eq!(roundtrip(&mut client_b, "m3").await, "B|B|3|m3");

    // 手順 5: A への push が B に混入しないこと（順序バリアで検証。
    // push の受信は WebSocketStream::next で直接待つ）。
    let (_, sender_a) = sender_for(&shared, "A");
    sender_a
        .send(WsMessage::Text("push-A".to_string()))
        .await
        .expect("push to A should succeed");
    let pushed_to_a = tokio::time::timeout(Duration::from_secs(5), client_a.next())
        .await
        .expect("push-A should arrive before test timeout")
        .expect("stream should not end")
        .expect("no error");
    assert_eq!(pushed_to_a, Message::Text("push-A".into()));
    // B の次フレームが B 自身の返信であること（push-A が紛れ込んでいない）。
    assert_eq!(roundtrip(&mut client_b, "m4").await, "B|B|4|m4");

    // 対称に B → A も確認する。
    let (_, sender_b) = sender_for(&shared, "B");
    sender_b
        .send(WsMessage::Text("push-B".to_string()))
        .await
        .expect("push to B should succeed");
    let pushed_to_b = tokio::time::timeout(Duration::from_secs(5), client_b.next())
        .await
        .expect("push-B should arrive before test timeout")
        .expect("stream should not end")
        .expect("no error");
    assert_eq!(pushed_to_b, Message::Text("push-B".into()));
    assert_eq!(roundtrip(&mut client_a, "m5").await, "A|A|5|m5");

    // 手順 6〜7: A を Close → ドレイン → join。closes に A のみ記録される
    // こと・conns から A が削除され B は残存すること・sender_a が閉じて
    // sender_b は開いたままであること。
    client_a
        .close(None)
        .await
        .expect("client A close should send");
    tokio::time::timeout(
        Duration::from_secs(5),
        drain_expect_only_close(&mut client_a),
    )
    .await
    .expect("client A drain should complete before test timeout");
    let result_a = tokio::time::timeout(Duration::from_secs(5), task_a)
        .await
        .expect("server task A should finish before test timeout")
        .expect("task A should not panic");
    assert!(
        result_a.is_ok(),
        "session A should end normally: {result_a:?}"
    );

    {
        let closes = shared.closes.lock().unwrap();
        assert_eq!(
            closes.len(),
            1,
            "on_close should be called exactly once so far"
        );
        assert_eq!(
            closes[0],
            (id_a, Some("A".to_string()), CloseReason::ClientClose)
        );
    }
    {
        let conns = shared.conns.lock().unwrap();
        assert!(!conns.contains_key(&id_a), "A's state must be removed");
        assert!(conns.contains_key(&id_b), "B's state must remain");
    }
    assert!(
        sender_a.is_closed(),
        "sender_a must report closed after A's on_close"
    );
    assert!(
        sender_a
            .send(WsMessage::Text("late".to_string()))
            .await
            .is_err(),
        "send to a closed connection must fail"
    );
    assert!(
        !sender_b.is_closed(),
        "sender_b must remain open while B is still connected"
    );
    assert!(!task_b.is_finished(), "task B must still be running");

    // 手順 8: B の継続性（カウンタがリセットされず継続、push も届く）。
    // B の roundtrip はここまで hello/m2/m3/m4 の 4 回済みなのでカウンタは 5。
    assert_eq!(roundtrip(&mut client_b, "m6").await, "B|B|5|m6");
    sender_b
        .send(WsMessage::Text("push-B-2".to_string()))
        .await
        .expect("push to B should still succeed after A closed");
    let pushed_to_b_2 = tokio::time::timeout(Duration::from_secs(5), client_b.next())
        .await
        .expect("push-B-2 should arrive before test timeout")
        .expect("stream should not end")
        .expect("no error");
    assert_eq!(pushed_to_b_2, Message::Text("push-B-2".into()));

    // 手順 9: B を close → join。closes がちょうど 2 件で each 接続に
    // on_open/on_close がちょうど 1 回ずつ対応すること。
    client_b
        .close(None)
        .await
        .expect("client B close should send");
    tokio::time::timeout(
        Duration::from_secs(5),
        drain_expect_only_close(&mut client_b),
    )
    .await
    .expect("client B drain should complete before test timeout");
    let result_b = tokio::time::timeout(Duration::from_secs(5), task_b)
        .await
        .expect("server task B should finish before test timeout")
        .expect("task B should not panic");
    assert!(
        result_b.is_ok(),
        "session B should end normally: {result_b:?}"
    );

    let closes = shared.closes.lock().unwrap();
    assert_eq!(
        closes.len(),
        2,
        "on_close should be called exactly twice in total"
    );
    assert_eq!(
        closes[1],
        (id_b, Some("B".to_string()), CloseReason::ClientClose)
    );
    let close_ids: Vec<WsConnId> = closes.iter().map(|(id, _, _)| *id).collect();
    assert_ne!(
        close_ids[0], close_ids[1],
        "the two on_close calls must be for distinct connections"
    );
    let open_ids: std::collections::HashSet<WsConnId> = shared
        .opens
        .lock()
        .unwrap()
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let close_id_set: std::collections::HashSet<WsConnId> = close_ids.into_iter().collect();
    assert_eq!(
        open_ids, close_id_set,
        "on_open/on_close must each fire exactly once per connection"
    );
}

/// テスト 2（受け入れ基準 2 の別経路）: 一方の接続を Close ハンドシェイク
/// なしに drop（EOF）した場合も、その接続についてのみ `on_close` が
/// `CloseReason::Eof` で呼ばれ、もう一方は影響を受けず継続できること。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_eof_on_one_only_fires_its_on_close_and_other_continues() {
    let shared = Arc::new(Shared::default());
    let config = WebSocketConfig::default()
        .with_path_pattern("/devtools/page/{id}")
        .unwrap()
        .with_handler(Registry {
            shared: Arc::clone(&shared),
        });

    let (mut client_a, task_a) = spawn_session(config.clone(), "/devtools/page/A").await;
    let (mut client_b, task_b) = spawn_session(config.clone(), "/devtools/page/B").await;

    assert_eq!(roundtrip(&mut client_a, "hello").await, "A|A|1|hello");
    assert_eq!(roundtrip(&mut client_b, "hello").await, "B|B|1|hello");

    let (id_a, id_b) = {
        let opens = shared.opens.lock().unwrap();
        let id_a = opens
            .iter()
            .find(|(_, page)| page == "A")
            .map(|(id, _)| *id)
            .expect("page A should be registered");
        let id_b = opens
            .iter()
            .find(|(_, page)| page == "B")
            .map(|(id, _)| *id)
            .expect("page B should be registered");
        (id_a, id_b)
    };
    let (_, sender_b) = sender_for(&shared, "B");

    // Close ハンドシェイクを行わず A の `WebSocketStream` を drop する
    // （EOF 経路。`on_close_e2e.rs` の `on_close_eof_called_once` と同型）。
    drop(client_a);

    // `Result` は経路によって `Ok`/`Err` いずれもありうる
    // （`CloseReason::Eof` の doc を参照）。値は固定せず reason のみ主張する。
    let _ = tokio::time::timeout(Duration::from_secs(5), task_a)
        .await
        .expect("server task A should finish before test timeout")
        .expect("task A should not panic");

    {
        let closes = shared.closes.lock().unwrap();
        assert_eq!(
            closes.len(),
            1,
            "on_close should be called exactly once so far"
        );
        assert_eq!(closes[0].0, id_a);
        assert_eq!(closes[0].1, Some("A".to_string()));
        assert_eq!(
            closes[0].2,
            CloseReason::Eof,
            "expected Eof, got {:?}",
            closes[0].2
        );
    }
    {
        let conns = shared.conns.lock().unwrap();
        assert!(!conns.contains_key(&id_a));
        assert!(conns.contains_key(&id_b));
    }
    assert!(!sender_b.is_closed(), "sender_b must remain open");
    assert!(!task_b.is_finished(), "task B must still be running");

    // B は継続して送受信・push できること。
    assert_eq!(roundtrip(&mut client_b, "m2").await, "B|B|2|m2");
    sender_b
        .send(WsMessage::Text("push-B".to_string()))
        .await
        .expect("push to B should succeed after A's EOF");
    let pushed = tokio::time::timeout(Duration::from_secs(5), client_b.next())
        .await
        .expect("push-B should arrive before test timeout")
        .expect("stream should not end")
        .expect("no error");
    assert_eq!(pushed, Message::Text("push-B".into()));

    // B を close → join。closes が 2 件で B が ClientClose であること。
    client_b
        .close(None)
        .await
        .expect("client B close should send");
    tokio::time::timeout(
        Duration::from_secs(5),
        drain_expect_only_close(&mut client_b),
    )
    .await
    .expect("client B drain should complete before test timeout");
    let result_b = tokio::time::timeout(Duration::from_secs(5), task_b)
        .await
        .expect("server task B should finish before test timeout")
        .expect("task B should not panic");
    assert!(
        result_b.is_ok(),
        "session B should end normally: {result_b:?}"
    );

    let closes = shared.closes.lock().unwrap();
    assert_eq!(
        closes.len(),
        2,
        "on_close should be called exactly twice in total"
    );
    assert_eq!(
        closes[1],
        (id_b, Some("B".to_string()), CloseReason::ClientClose)
    );
}
