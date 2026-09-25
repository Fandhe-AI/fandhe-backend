//! ユーザー定義 WebSocket メッセージハンドラ API（Issue #179、親 #91）。
//!
//! `crate::session` はこのモジュールが定義する [`WsMessageHandler`] を
//! Text/Binary メッセージ受信ごとに呼び出し、返り値（[`WsOutcome`]）に
//! 従って返信送出・セッション継続/終了を判断する。tokio-tungstenite の
//! `Message` 型を公開 API へ漏らさないよう、内部依存のバージョン更新から
//! 絶縁する独自表現 [`WsMessage`] を介する（`docs/design/plugin-boundary.md`
//! 5.2 節、依存方向はコア → 本クレートの単方向のみで、本クレートは
//! `fandhe-backend-core` に依存しない制約は不変）。
//!
//! `async fn` はトレイトオブジェクトと非互換のため、`crates/plugin-graphql`
//! の先例（`BoxExecuteFn`）に倣い、追加の依存を増やさず既存の `futures-util`
//! （`std` feature、`Cargo.toml` 参照）が提供する
//! [`futures_util::future::BoxFuture`] で型消去する（pay-for-what-you-use、
//! `.claude/rules/pay-for-what-you-use.md`。async-trait 等の新規依存は
//! 追加しない）。

use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use tokio::sync::mpsc;

/// ユーザーコードとやり取りするメッセージ表現。
///
/// tokio-tungstenite の `Message` から変換して [`WsMessageHandler::on_message`]
/// へ渡される（`crate::session` が変換を担う）。Ping/Pong/Close は
/// tungstenite 側で既存どおり処理されるため、本 API には現れない
/// （ハンドラは Text/Binary のみを扱う契約）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsMessage {
    /// UTF-8 検証済みのテキストフレーム（結合済み）。
    Text(String),
    /// バイナリフレーム（結合済み）。
    Binary(Vec<u8>),
}

/// [`WsMessageHandler::on_message`] の処理結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsOutcome {
    /// 返信メッセージ群（0 件以上）を到着順に送出し、セッションを継続する。
    /// 空の `Vec` は「返信なしで継続」を表す。
    Reply(Vec<WsMessage>),
    /// サーバ側から Close ハンドシェイクを開始し、セッションを正常終了する。
    Close,
}

/// `crate::channel` の既定容量（イシュー #671。`crate::lib::handle_upgrade`
/// が `on_open` 呼び出し前に毎接続 1 個生成する `WsSender`/`Receiver` ペアの
/// バッファサイズ）。
///
/// `crates/core/src/streaming.rs` の `DEFAULT_CHANNEL_CAPACITY = 8`（レスポンス
/// ストリーミング用 bounded mpsc の既定容量）と同じ値・同じ考え方を踏襲する
/// （無制限バッファ化による DoS を避ける、`.claude/rules/security.md`）。
/// 容量を利用者が調整できる公開 API は本イシューのスコープ外とする。
pub(crate) const DEFAULT_OUTBOUND_CAPACITY: usize = 8;

/// ユーザーハンドラが返すエラーの型消去（`Box<dyn Error + Send + Sync>`）。
///
/// `Display` はユーザーが与えた文脈のみを表示する契約とし、受信メッセージの
/// ペイロードを本型自身が付加することはない（ログ・診断名にリクエスト内容を
/// 含めない、`.claude/rules/security.md`）。ペイロードを含めるかどうかの
/// 責務はユーザーハンドラ実装側にあり、本 API はそれを強制も検査もしない点に
/// 留意する。
#[derive(Debug)]
pub struct WsHandlerError(Box<dyn StdError + Send + Sync>);

impl WsHandlerError {
    /// 任意のエラーからハンドラエラーを構築する。
    pub fn new(err: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self(err.into())
    }
}

impl fmt::Display for WsHandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "websocket handler error: {}", self.0)
    }
}

impl StdError for WsHandlerError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.0.as_ref())
    }
}

/// Text/Binary メッセージ受信ごとに呼ばれるユーザー定義ハンドラ。
///
/// `crate::session::run_session` がメッセージごとに直列 `await` する
/// （並び順の保証・自然なバックプレッシャのため。並行処理したい場合は
/// 実装側で自前に `tokio::spawn` する建て付けとする）。実装は同期
/// ブロッキング I/O を行わない契約とする（`.claude/rules/coding-rust.md` の
/// 「Tokio 上でブロッキング処理を await スレッドで実行しない」と同一原則。
/// 本 trait はコアの `Middleware` 拡張点ではないためコンパイル時には
/// 強制できず、実装者が守るべき契約として doc で明示する）。
///
/// `WebSocketConfig::with_handler` で登録する（`WebSocketConfig` は
/// `Arc<dyn WsMessageHandler>` として保持するため、`Send + Sync + 'static`
/// を要求する）。
///
/// # 中断安全性の契約（イシュー #499）
///
/// `on_message` が返す `Future` は、コアの世代キャンセルシグナル（最終
/// graceful shutdown・rebind 世代 drain）発火時に**任意の `await` 点で
/// drop されうる**（`crate::session` が `race_cancel` でキャンセルと
/// race させるため。Rust async の標準的なキャンセル意味論であり、
/// `tokio::select!` / `tokio::time::timeout` と同型）。実装は中断されても
/// 不変条件を壊さない（drop-safe な）ことを要求され、完了保証が必要な処理
/// （外部リソースへの書き込み確定等）は本 trait の `await` から切り離し、
/// `tokio::spawn` で独立したタスクとして実行する。中断された場合、返す
/// はずだった `WsOutcome::Reply` の返信は破棄され、送出されない。
pub trait WsMessageHandler: Send + Sync + 'static {
    /// 診断用のハンドラ名（`UpgradeHandler::name` と同じ流儀）。
    fn name(&self) -> &'static str;

    /// メッセージ受信時に呼ばれ、返信または Close の指示を返す。
    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>>;

    /// 接続確立直後（101 応答送出成功後・`WebSocketStream` 構築前）に一度だけ
    /// 呼ばれるフック（イシュー #671。親 #669「サーバー起点で任意タイミングに
    /// push できる WebSocket API」の第 2 段）。
    ///
    /// 既定実装は `ctx` を無視する no-op で、既存ハンドラは無変更のまま
    /// コンパイル・動作する（後方互換）。`ctx.sender()` を `.clone()` して
    /// `tokio::spawn` したタスクへ move すれば、クライアントのメッセージ
    /// 受信を待たずに任意タイミングで push できる。
    ///
    /// 本メソッドは**同期**（非 `async`）である。呼び出し元（`crate::
    /// handle_upgrade`）はセッションループに入る前に一度だけ同期的に呼び、
    /// 戻り値を待たずに処理を進める。`.await` を要する処理は本メソッド内部で
    /// `tokio::spawn` して切り離す（`.claude/rules/coding-rust.md` の「Tokio
    /// 上でブロッキング処理を await スレッドで実行しない」と同一原則。
    /// `on_message` が持つ #499 の中断安全性契約・`race_cancel` とは無関係で、
    /// 本フック自体はキャンセルと race しない）。ハンドシェイクが失敗した
    /// 接続（400/426 応答）や、101 応答送出前に世代キャンセルが既に発火して
    /// いた接続では呼ばれない（`crate::handle_upgrade` の doc を参照。
    /// フェイルクローズ: 確立していないセッションへ `WsSender` を渡さない）。
    ///
    /// spawn したタスクは、セッション終了後（`WsSender::send` が
    /// [`WsSendError`] を返した時点）に自発的に終了すべきである。本フックは
    /// spawn されたタスク自体のライフサイクルを追跡・強制終了しない（世代
    /// キャンセル・最終 graceful shutdown・rebind 世代 drain はセッションの
    /// 受信ループ・ハンドラ実行・返信送出を打ち切るのみで、`on_open` から
    /// spawn した独立タスクまでは追跡しない契約。過大な保証をしない）。
    ///
    /// # Examples
    ///
    /// （`handle_upgrade` を実際に駆動して `WsOpenContext` を取得する完全な
    /// 例。`WsOpenContext::new` / `channel` は `pub(crate)` のため外部から
    /// 直接構築できず、doc test は実際のハンドシェイクを経由する。）
    ///
    /// ```
    /// use std::time::Duration;
    /// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    /// use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
    /// use fandhe_backend_plugin_websocket::handler::{
    ///     WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome,
    /// };
    /// use futures_util::future::BoxFuture;
    /// use futures_util::StreamExt;
    /// use tokio::io::AsyncReadExt;
    /// use tokio_tungstenite::WebSocketStream;
    /// use tokio_tungstenite::tungstenite::protocol::Role;
    ///
    /// struct PushOnOpen;
    ///
    /// impl WsMessageHandler for PushOnOpen {
    ///     fn name(&self) -> &'static str {
    ///         "push-on-open"
    ///     }
    ///
    ///     fn on_open(&self, ctx: WsOpenContext) {
    ///         let sender = ctx.sender().clone();
    ///         tokio::spawn(async move {
    ///             let _ = sender.send(WsMessage::Text("hello from server".to_string())).await;
    ///         });
    ///     }
    ///
    ///     fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
    ///         Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    ///     }
    /// }
    ///
    /// # async fn read_http_response_line<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> String {
    /// #     let mut buf = Vec::new();
    /// #     let mut byte = [0u8; 1];
    /// #     loop {
    /// #         let n = stream.read(&mut byte).await.unwrap();
    /// #         assert_ne!(n, 0);
    /// #         buf.push(byte[0]);
    /// #         if buf.ends_with(b"\r\n\r\n") { break; }
    /// #     }
    /// #     String::from_utf8(buf).unwrap()
    /// # }
    /// #
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let buf = b"GET /ws HTTP/1.1\r\n\
    ///     Upgrade: websocket\r\n\
    ///     Connection: Upgrade\r\n\
    ///     Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
    ///     Sec-WebSocket-Version: 13\r\n\
    ///     \r\n";
    /// let head = match parse_request_head(buf).unwrap() {
    ///     ParseOutcome::Complete { head, .. } => head,
    ///     ParseOutcome::Incomplete => unreachable!(),
    /// };
    /// let config = WebSocketConfig::default().with_handler(PushOnOpen);
    ///
    /// let (server_side, mut client_side) = tokio::io::duplex(4096);
    /// let server_task = tokio::spawn(async move {
    ///     handle_upgrade(server_side, &head, Vec::new(), &config, std::future::pending::<()>()).await
    /// });
    ///
    /// let response = read_http_response_line(&mut client_side).await;
    /// assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
    ///
    /// let mut client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    /// let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
    ///     .await
    ///     .expect("push should arrive within timeout")
    ///     .expect("stream should not end")
    ///     .expect("frame should not error");
    /// assert_eq!(msg.into_text().unwrap(), "hello from server");
    ///
    /// client.close(None).await.ok();
    /// let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
    /// # }
    /// ```
    fn on_open(&self, ctx: WsOpenContext) {
        let _ = ctx;
    }
}

/// `on_open` に渡す接続確立コンテキスト（イシュー #671、親 #669）。
///
/// 非公開フィールド + アクセサという構成（`crates/core/src/extension.rs` の
/// `GateContext` と同型）に加え `#[non_exhaustive]` を付け、将来のフィールド
/// 追加（パスパラメータ、イシュー #676 で検討予定）が破壊的変更にならない
/// ようにする。
#[non_exhaustive]
pub struct WsOpenContext {
    sender: WsSender,
}

impl WsOpenContext {
    /// `sender` を包んだコンテキストを構築する（`pub(crate)`、`crate::
    /// handle_upgrade` からのみ呼ばれる）。
    pub(crate) fn new(sender: WsSender) -> Self {
        Self { sender }
    }

    /// このセッションへ push するための `WsSender` への参照を返す。
    /// `tokio::spawn` するタスクへ渡すには `.clone()` する。
    #[must_use]
    pub fn sender(&self) -> &WsSender {
        &self.sender
    }
}

impl fmt::Debug for WsOpenContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsOpenContext").finish_non_exhaustive()
    }
}

/// 既定のエコーハンドラ（受信メッセージをそのまま返送する）。
///
/// `WebSocketConfig::default()` が使う実体であり、Issue #179 以前の
/// エコー専用挙動との後方互換を担保する。
///
/// # Examples
///
/// ```
/// use fandhe_backend_plugin_websocket::handler::{EchoHandler, WsMessage, WsMessageHandler, WsOutcome};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let handler = EchoHandler;
/// assert_eq!(handler.name(), "echo");
/// let outcome = handler
///     .on_message(WsMessage::Text("hello".to_string()))
///     .await
///     .unwrap();
/// assert_eq!(outcome, WsOutcome::Reply(vec![WsMessage::Text("hello".to_string())]));
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct EchoHandler;

impl WsMessageHandler for EchoHandler {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

/// `WebSocketConfig` が保持するハンドラの既定値を構築する（`pub(crate)`、
/// `config.rs` の `Default` 実装から呼ばれる）。
pub(crate) fn default_handler() -> Arc<dyn WsMessageHandler> {
    Arc::new(EchoHandler)
}

/// サーバー起点で任意タイミングに WebSocket メッセージを push するための
/// 送信ハンドル（イシュー #670。親 #669「サーバー起点で任意タイミングに
/// push できる WebSocket API」の第 1 段）。
///
/// `crate::session::run_session` の受信ループへ bounded mpsc 経由で合流し、
/// クライアントからの受信・ユーザーハンドラの返信（[`WsOutcome::Reply`]）と
/// 同一の `WebSocketStream` を単一タスクが排他的に所有した状態で直列に
/// 送出される（フレームが混ざらないことを構造的に保証する。詳細は
/// `crate::session` モジュールの doc を参照）。
///
/// イシュー #671 で [`WsMessageHandler::on_open`] 経由の公開経路が追加され、
/// `crate::handle_upgrade` が接続確立ごとに [`WsOpenContext`] を通じてハン
/// ドラへ渡す。
///
/// clone 可能で、複数タスクから同時に `send` してよい（内部の
/// `mpsc::Sender` がそのままクローン可能なことに由来する）。
#[derive(Clone)]
pub struct WsSender {
    tx: mpsc::Sender<WsMessage>,
}

impl WsSender {
    /// `msg` をセッションの送信キューへ入れる。
    ///
    /// チャネルが満杯の場合、受信側（`crate::session::run_session`）が
    /// キューを消費するまで `.await` で待機する契約（
    /// `crates/core/src/streaming.rs` の `BodyWriter::send` と同型の
    /// バックプレッシャ。無制限バッファ化を防ぐリソース枯渇 DoS 対策、
    /// `.claude/rules/security.md`）。
    ///
    /// セッションが既に終了している場合（受信側が drop 済み）は
    /// [`WsSendError`] を返す。セッションの世代キャンセル（最終 graceful
    /// shutdown・rebind 世代 drain）発火時は、ブロック中の呼び出しも
    /// `WebSocketConfig::close_grace` の満了を待たず即座にこのエラーで
    /// 解放される（`crate::session` が cancel 発火時に受信側 `Receiver` を
    /// 明示的に drop するため。イシュー #670 の受け入れ基準 3）。
    pub async fn send(&self, msg: WsMessage) -> Result<(), WsSendError> {
        self.tx.send(msg).await.map_err(|_| WsSendError)
    }
}

/// [`WsSender::send`] が返すエラー（セッション終了後の送信試行）。
///
/// `Display` はペイロード・内部状態を含まない固定文言とする（ログ・診断
/// 名に送信内容や内部状態を含めない、`.claude/rules/security.md`。既存
/// `crates/core/src/streaming.rs` の `StreamClosed` と同型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WsSendError;

impl fmt::Display for WsSendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "websocket session already closed")
    }
}

impl StdError for WsSendError {}

/// [`WsSender`] と、`crate::session::run_session` が受け取る側の
/// `mpsc::Receiver<WsMessage>` のペアを構築する（`pub(crate)`、内部配線
/// 専用）。
///
/// 外部へは [`WsSender::send`] のみを公開する非対称 API とし、受信側
/// （`Receiver`）はクレート内部（`run_session`）にのみ渡す。
///
/// `capacity` は `1` に切り上げる（`mpsc::channel(0)` は panic するため、
/// `crates/core/src/streaming.rs` の `StreamingResponse::channel` と同一の
/// 防御）。
///
/// `crate::handle_upgrade` から呼ばれる（イシュー #671。101 応答送出成功後・
/// `on_open` 呼び出し直前に毎接続 1 回呼ばれる）。
pub(crate) fn channel(capacity: usize) -> (WsSender, mpsc::Receiver<WsMessage>) {
    let (tx, rx) = mpsc::channel(capacity.max(1));
    (WsSender { tx }, rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 大文字化して返すトイハンドラ。カスタムハンドラ委譲の基本形を
    /// 単体テストで確認する（統合テストは `tests/handler_e2e.rs`）。
    struct UppercaseHandler;

    impl WsMessageHandler for UppercaseHandler {
        fn name(&self) -> &'static str {
            "uppercase"
        }

        fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
            Box::pin(async move {
                let reply = match msg {
                    WsMessage::Text(t) => WsMessage::Text(t.to_uppercase()),
                    WsMessage::Binary(b) => WsMessage::Binary(b),
                };
                Ok(WsOutcome::Reply(vec![reply]))
            })
        }
    }

    #[tokio::test]
    async fn custom_handler_transforms_text() {
        let handler = UppercaseHandler;
        let outcome = handler
            .on_message(WsMessage::Text("hi".to_string()))
            .await
            .unwrap();
        assert_eq!(
            outcome,
            WsOutcome::Reply(vec![WsMessage::Text("HI".to_string())])
        );
    }

    #[tokio::test]
    async fn handler_error_display_does_not_require_payload() {
        let err = WsHandlerError::new("boom");
        assert_eq!(err.to_string(), "websocket handler error: boom");
    }

    /// 既定 `on_open` が no-op（`ctx` を無視するのみ）であることを確認する
    /// （イシュー #671、受け入れ基準 1「既定は no-op で後方互換」）。
    #[tokio::test]
    async fn default_on_open_is_noop() {
        let handler = UppercaseHandler;
        let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        // no-op であることの確認は「panic しないこと」のみで、戻り値もない。
        handler.on_open(WsOpenContext::new(sender));
    }

    /// `WsOpenContext::sender()` が返す参照を `.clone()` して送信すると、
    /// `channel()` の受信側へ届くことを確認する（`on_open` 実装がクローンして
    /// `tokio::spawn` するパターンの前提を検証する）。
    #[tokio::test]
    async fn open_context_sender_clone_delivers_message() {
        let (sender, mut rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let ctx = WsOpenContext::new(sender);
        let cloned = ctx.sender().clone();
        cloned
            .send(WsMessage::Text("hi".to_string()))
            .await
            .unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received, WsMessage::Text("hi".to_string()));
    }
}
