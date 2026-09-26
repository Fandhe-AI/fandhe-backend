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
use std::sync::atomic::{AtomicU64, Ordering};

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

/// プロセス内で一意な WebSocket 接続識別子（イシュー #704、親 #702）。
///
/// `on_open`（[`WsOpenContext::conn_id`]）・`on_message_with_ctx`
/// （[`WsConnContext::conn_id`]）の双方で同一接続なら同じ値が観測される
/// ことを保証する。`crate::lib::handle_upgrade` が 101 応答送出成功後に
/// `Self::next`（`pub(crate)`）で 1 回だけ発行し、同一セッションの生存期間中は変わらない
/// （設計は `docs/design/ws-connection-context-and-close.md` 3 節）。
///
/// # 一意性の範囲・セキュリティ上の注意
///
/// 一意性は**単一プロセス内**（複数の `WebSocketConfig` 登録をまたいでも
/// 一意）に限る。プロセス再起動・複数プロセス・分散環境をまたいだ一意性は
/// 保証しない。発行元は `AtomicU64`（`unsafe` 不使用）のみで、クライアント
/// 入力からは一切導出されない。値は単調増加する連番であり**推測可能**
/// なため、認可トークンやセッションの秘密として使ってはならない
/// （識別子であって資格情報ではない、`.claude/rules/security.md`）。
/// u64 を使い切ることは実用上の懸念にならない。
///
/// 利用者はこの値を `on_open`（[`WsOpenContext::conn_id`]）・
/// `on_message_with_ctx`（[`WsConnContext::conn_id`]）経由でのみ取得する
/// （本型自身に構築 API は公開しない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WsConnId(u64);

/// [`WsConnId`] の発行カウンタ（イシュー #704）。プロセス起動時 1 から
/// 単調増加する（`0` を「未発行」の番兵として予約する意図はなく、単に
/// 開始値を 1 とする慣習に揃えただけ。`Relaxed` で十分な理由は、この
/// カウンタが「他のメモリ操作との happens-before 関係」を要求しない
/// 単純な一意値発行専用であるため）。
static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

impl WsConnId {
    /// 新しい一意な接続 ID を発行する（`pub(crate)`。`crate::handle_upgrade`
    /// が 101 応答送出成功後に 1 回だけ呼ぶ。クライアント入力から独立した
    /// サーバー側発行専用のため公開 API にはしない）。
    pub(crate) fn next() -> Self {
        Self(NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for WsConnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
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
    ///
    /// 既定では `crate::session` から直接ではなく、[`Self::on_message_with_ctx`]
    /// の既定実装経由で呼ばれる（イシュー #704。`on_message_with_ctx` を
    /// オーバーライドしたハンドラでは、実行時に本メソッドは呼ばれない）。
    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>>;

    /// [`Self::on_message`] に接続コンテキスト（[`WsConnContext`]）を
    /// 付加した経路（イシュー #704、親 #702）。
    ///
    /// 既定実装は `ctx` を無視して [`Self::on_message`] へ委譲するため、
    /// 既存ハンドラ（`on_message` のみを実装したもの）は無変更のまま
    /// コンパイル・動作する（後方互換。設計は
    /// `docs/design/ws-connection-context-and-close.md` 3 節）。接続単位の
    /// 状態・イベント購読を扱いたいハンドラは本メソッドをオーバーライドし、
    /// `ctx.conn_id()` をキーに自前の `Mutex<HashMap<WsConnId, _>>` 等で
    /// 状態を管理する（本 trait 自体は接続ごとのインスタンス化を提供しない。
    /// 設計 2 節が比較した「案 A: ハンドラファクトリ」は不採用）。
    ///
    /// `on_message_with_ctx` をオーバーライドする場合、trait 制約上
    /// [`Self::on_message`] にも何らかの実装が必要になる（オーバーライド
    /// していれば実行時には呼ばれない、トレードオフ）。両メソッドを
    /// provided 化して互いの既定実装に委譲させる構成は、いずれもオーバー
    /// ライドしないハンドラで無限再帰（スタックオーバーフロー）になるため
    /// 採らない。
    ///
    /// # ライフタイム
    ///
    /// `&'a self` と `ctx: &'a WsConnContext` を同一の明示ライフタイム `'a`
    /// に統一している。`&self` を持つメソッドの戻り値型に省略記法 `'_` を
    /// 使うと `self` の借用にのみ束縛され、`ctx` を捕捉した `Future` を
    /// 返すオーバーライド実装がコンパイルできないため
    /// （`docs/design/ws-connection-context-and-close.md` 3 節「計画からの
    /// 変更点」）。
    ///
    /// # 中断安全性の契約（イシュー #499 を継承）
    ///
    /// [`Self::on_message`] と同じ契約が適用される。返す `Future` は
    /// コアの世代キャンセルシグナル発火時に任意の `await` 点で drop
    /// されうる。
    ///
    /// # outbound 消化（自己送信の安全性）
    ///
    /// 本メソッド実行中に `ctx.sender()` から容量
    /// （`DEFAULT_OUTBOUND_CAPACITY` = 8）を超えて `send(...).await` しても
    /// デッドロックしない。`crate::session::run_session` は本メソッドの
    /// `Future` を単独 `await` せず、outbound 到着と race させて都度
    /// 消化するため（PR #725 レビュー指摘対応、設計は
    /// `docs/design/ws-connection-context-and-close.md` 6 節。排出開始
    /// 時点で既に格納済みだった push は本メソッドが返す
    /// `WsOutcome::Reply`/`Close` より先に送出される保証があり、それ以外の
    /// 相対順序は不定）。
    ///
    /// # Examples
    ///
    /// （`with_path_pattern` を登録した `WebSocketConfig` で実ハンドシェイクを
    /// 駆動し、`on_message_with_ctx` 内で `ctx.conn_id()` と `ctx.param("id")`
    /// を読み取ってクライアントへ返す。`WsConnContext::new` は `pub(crate)`
    /// のため doc test は実際のハンドシェイクを経由する。）
    ///
    /// ```
    /// use std::time::Duration;
    /// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    /// use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
    /// use fandhe_backend_plugin_websocket::handler::{
    ///     WsConnContext, WsHandlerError, WsMessage, WsMessageHandler, WsOutcome,
    /// };
    /// use futures_util::future::BoxFuture;
    /// use futures_util::{SinkExt, StreamExt};
    /// use tokio::io::AsyncReadExt;
    /// use tokio_tungstenite::WebSocketStream;
    /// use tokio_tungstenite::tungstenite::protocol::Role;
    ///
    /// struct EchoConnIdAndParam;
    ///
    /// impl WsMessageHandler for EchoConnIdAndParam {
    ///     fn name(&self) -> &'static str {
    ///         "echo-conn-id-and-param"
    ///     }
    ///
    ///     fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
    ///         // on_message_with_ctx をオーバーライドしているため実行時には
    ///         // 呼ばれない（トレードオフ、上記 doc を参照）。
    ///         Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    ///     }
    ///
    ///     fn on_message_with_ctx<'a>(
    ///         &'a self,
    ///         ctx: &'a WsConnContext,
    ///         _msg: WsMessage,
    ///     ) -> BoxFuture<'a, Result<WsOutcome, WsHandlerError>> {
    ///         Box::pin(async move {
    ///             let id = ctx.param("id").unwrap_or("none");
    ///             let text = format!("{}:{}", ctx.conn_id(), id);
    ///             Ok(WsOutcome::Reply(vec![WsMessage::Text(text)]))
    ///         })
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
    /// let buf = b"GET /devtools/page/ABC123 HTTP/1.1\r\n\
    ///     Upgrade: websocket\r\n\
    ///     Connection: Upgrade\r\n\
    ///     Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
    ///     Sec-WebSocket-Version: 13\r\n\
    ///     \r\n";
    /// let head = match parse_request_head(buf).unwrap() {
    ///     ParseOutcome::Complete { head, .. } => head,
    ///     ParseOutcome::Incomplete => unreachable!(),
    /// };
    /// let config = WebSocketConfig::default()
    ///     .with_path_pattern("/devtools/page/{id}")
    ///     .unwrap()
    ///     .with_handler(EchoConnIdAndParam);
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
    /// client.send(tokio_tungstenite::tungstenite::Message::Text("ping".into())).await.unwrap();
    ///
    /// let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
    ///     .await
    ///     .expect("reply should arrive within timeout")
    ///     .expect("stream should not end")
    ///     .expect("frame should not error");
    /// let text = msg.into_text().unwrap();
    /// let (conn_id_str, param) = text.split_once(':').expect("expected \"conn_id:param\"");
    /// assert!(conn_id_str.parse::<u64>().is_ok());
    /// assert_eq!(param, "ABC123");
    ///
    /// client.close(None).await.ok();
    /// let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
    /// # }
    /// ```
    fn on_message_with_ctx<'a>(
        &'a self,
        ctx: &'a WsConnContext,
        msg: WsMessage,
    ) -> BoxFuture<'a, Result<WsOutcome, WsHandlerError>> {
        let _ = ctx;
        self.on_message(msg)
    }

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
/// 追加が破壊的変更にならないようにする。イシュー #676 で
/// [`WebSocketConfig::with_path_pattern`][crate::config::WebSocketConfig::with_path_pattern]
/// 由来のパスパラメータを保持する `params` フィールドを追加した
/// （[`Self::param`] / [`Self::params`] 参照）。イシュー #704（親 #702）で
/// [`Self::conn_id`] を追加し、`on_message_with_ctx`（[`WsConnContext::conn_id`]）
/// と同一の接続識別子を `on_open` の時点から観測できるようにした。
#[non_exhaustive]
pub struct WsOpenContext {
    conn_id: WsConnId,
    sender: WsSender,
    /// マッチしたパスパラメータ（登録順）。パターン未登録の
    /// `WebSocketConfig`（完全一致パス）では常に空。
    ///
    /// `crate::handshake::match_config_path` が返す借用 `PathParams<'a>`
    /// は `head`（`crate::handle_upgrade` のスタックフレーム内で生存）に
    /// 依存しており `'static` にできないため、`handle_upgrade` が
    /// `on_open` 呼び出し時点で所有 `Vec` へコピーしたものを保持する
    /// （コピー量は `crate::pattern::MAX_PATTERN_SEGMENTS` ×
    /// `crate::pattern::MAX_SEGMENT_BYTES` で有界、新たな DoS 懸念には
    /// ならない）。
    params: Vec<(String, String)>,
}

impl WsOpenContext {
    /// `conn_id`・`sender`・抽出済みパスパラメータを包んだコンテキストを
    /// 構築する（`pub(crate)`、`crate::handle_upgrade` からのみ呼ばれる）。
    pub(crate) fn new(conn_id: WsConnId, sender: WsSender, params: Vec<(String, String)>) -> Self {
        Self {
            conn_id,
            sender,
            params,
        }
    }

    /// この接続の一意な識別子を返す（イシュー #704）。同一接続であれば
    /// `on_message_with_ctx` 経由の [`WsConnContext::conn_id`] と常に
    /// 同じ値になる。
    #[must_use]
    pub fn conn_id(&self) -> WsConnId {
        self.conn_id
    }

    /// このセッションへ push するための `WsSender` への参照を返す。
    /// `tokio::spawn` するタスクへ渡すには `.clone()` する。
    #[must_use]
    pub fn sender(&self) -> &WsSender {
        &self.sender
    }

    /// `name` に対応するパスパラメータの値を返す（イシュー #676）。
    ///
    /// 値は % デコードしない生の文字列（`crate::pattern::PathParams` と
    /// 同じ非デコード契約。デコードが必要な場合は呼び出し側の責務）。
    /// `WebSocketConfig::with_path_pattern` 未登録（完全一致パス）の場合、
    /// あるいは `name` が登録パターンに存在しない場合は常に `None`。
    ///
    /// # Examples
    ///
    /// （`with_path_pattern` を登録した `WebSocketConfig` で実ハンドシェイクを
    /// 駆動し、`on_open` 内で `ctx.param("id")` を読み取った値をクライアントへ
    /// push する。CDP（Chrome DevTools Protocol）互換サーバーが
    /// `/devtools/page/{id}` の `id` からセッションを特定するユースケースの
    /// 最小形。）
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
    /// struct PushPageId;
    ///
    /// impl WsMessageHandler for PushPageId {
    ///     fn name(&self) -> &'static str {
    ///         "push-page-id"
    ///     }
    ///
    ///     fn on_open(&self, ctx: WsOpenContext) {
    ///         let id = ctx.param("id").unwrap_or("unknown").to_string();
    ///         let sender = ctx.sender().clone();
    ///         tokio::spawn(async move {
    ///             let _ = sender.send(WsMessage::Text(id)).await;
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
    /// let buf = b"GET /devtools/page/ABC123 HTTP/1.1\r\n\
    ///     Upgrade: websocket\r\n\
    ///     Connection: Upgrade\r\n\
    ///     Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
    ///     Sec-WebSocket-Version: 13\r\n\
    ///     \r\n";
    /// let head = match parse_request_head(buf).unwrap() {
    ///     ParseOutcome::Complete { head, .. } => head,
    ///     ParseOutcome::Incomplete => unreachable!(),
    /// };
    /// let config = WebSocketConfig::default()
    ///     .with_path_pattern("/devtools/page/{id}")
    ///     .unwrap()
    ///     .with_handler(PushPageId);
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
    /// assert_eq!(msg.into_text().unwrap(), "ABC123");
    ///
    /// client.close(None).await.ok();
    /// let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
    /// # }
    /// ```
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// マッチしたパスパラメータ全件を登録順（パターン上の出現順）に返す
    /// （イシュー #676）。単一パラメータの取得には [`Self::param`] の方が
    /// 簡潔。
    pub fn params(&self) -> impl Iterator<Item = (&str, &str)> {
        self.params.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

impl fmt::Debug for WsOpenContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // パスパラメータは攻撃者が URL セグメントとして自由に制御できる
        // 入力のため、Debug 出力には含めない（ログ・診断出力への機密混入
        // 防止、`.claude/rules/security.md`）。conn_id はサーバー側の
        // 単調カウンタ発行でありクライアント入力に由来しないため出力する
        // （イシュー #704）。
        f.debug_struct("WsOpenContext")
            .field("conn_id", &self.conn_id)
            .finish_non_exhaustive()
    }
}

/// `on_message_with_ctx`（イシュー #704、親 #702）に渡す接続コンテキスト。
///
/// `WsOpenContext` と同じ設計パターン（`#[non_exhaustive]` + 非公開
/// フィールド + アクセサ）を踏襲するが、意図的に別型として新設する。
/// `WsOpenContext` は「一度だけ消費される値」（`on_open` 呼び出し時に
/// 1 回だけ渡される）であるのに対し、`WsConnContext` は「セッション生存中
/// 繰り返し参照される値」（`on_message_with_ctx` の呼び出しごとに渡される）
/// という用途の違いがあるため、型を分離して将来どちらかだけにフィールドを
/// 追加する場合の型結合を避ける（設計は
/// `docs/design/ws-connection-context-and-close.md` 3 節）。
///
/// # `WsSender` を自身が保持する副作用
///
/// `crate::lib::handle_upgrade` が `on_open` 呼び出し前に構築し、
/// `crate::session::run_session` へ貸し出してセッション生存期間中ずっと
/// 保持する。すなわち本型自身が `WsSender`（`mpsc::Sender` のクローン）を
/// 1 個保持し続けるため、全ハンドラ・呼び出し元が他のクローンを drop
/// しても、セッションが終了するまで outbound チャネルの送信側は閉じない
/// （`crate::session` モジュール doc・
/// `docs/design/ws-connection-context-and-close.md` 5 節を参照）。
#[non_exhaustive]
pub struct WsConnContext {
    conn_id: WsConnId,
    sender: WsSender,
    /// マッチしたパスパラメータ（登録順）。`WsOpenContext::params` と同じ
    /// 非デコード契約・DoS 上限（`crate::pattern::MAX_PATTERN_SEGMENTS` ×
    /// `crate::pattern::MAX_SEGMENT_BYTES`）を共有する。
    params: Vec<(String, String)>,
}

impl WsConnContext {
    /// `conn_id`・`sender`・抽出済みパスパラメータを包んだコンテキストを
    /// 構築する（`pub(crate)`、`crate::handle_upgrade` からのみ呼ばれる）。
    pub(crate) fn new(conn_id: WsConnId, sender: WsSender, params: Vec<(String, String)>) -> Self {
        Self {
            conn_id,
            sender,
            params,
        }
    }

    /// この接続の一意な識別子を返す（イシュー #704）。同一接続であれば
    /// `on_open` 経由の [`WsOpenContext::conn_id`] と常に同じ値になる。
    #[must_use]
    pub fn conn_id(&self) -> WsConnId {
        self.conn_id
    }

    /// このセッションへ push するための `WsSender` への参照を返す。
    /// `tokio::spawn` するタスクへ渡すには `.clone()` する。
    #[must_use]
    pub fn sender(&self) -> &WsSender {
        &self.sender
    }

    /// `name` に対応するパスパラメータの値を返す（[`WsOpenContext::param`]
    /// と同一の非デコード契約）。`WebSocketConfig::with_path_pattern`
    /// 未登録（完全一致パス）の場合、あるいは `name` が登録パターンに
    /// 存在しない場合は常に `None`。
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// マッチしたパスパラメータ全件を登録順に返す（[`WsOpenContext::params`]
    /// と同一の契約）。単一パラメータの取得には [`Self::param`] の方が簡潔。
    pub fn params(&self) -> impl Iterator<Item = (&str, &str)> {
        self.params.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

impl fmt::Debug for WsConnContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // WsOpenContext::Debug と同一のセキュリティ根拠: パスパラメータは
        // 攻撃者制御下の URL セグメントのため出力しない。conn_id はサーバー
        // 側発行のため出力する。
        f.debug_struct("WsConnContext")
            .field("conn_id", &self.conn_id)
            .finish_non_exhaustive()
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
        handler.on_open(WsOpenContext::new(WsConnId::next(), sender, Vec::new()));
    }

    /// `WsOpenContext::sender()` が返す参照を `.clone()` して送信すると、
    /// `channel()` の受信側へ届くことを確認する（`on_open` 実装がクローンして
    /// `tokio::spawn` するパターンの前提を検証する）。
    #[tokio::test]
    async fn open_context_sender_clone_delivers_message() {
        let (sender, mut rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let ctx = WsOpenContext::new(WsConnId::next(), sender, Vec::new());
        let cloned = ctx.sender().clone();
        cloned
            .send(WsMessage::Text("hi".to_string()))
            .await
            .unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received, WsMessage::Text("hi".to_string()));
    }

    /// パラメータ未登録（`Vec::new()`）の `WsOpenContext` は `param`/`params`
    /// が常に空を返すことを確認する（イシュー #676、受け入れ基準 2 の単体
    /// レベルの裏取り。実ハンドシェイク経由の確認は `tests/handler_e2e.rs`）。
    #[tokio::test]
    async fn open_context_without_params_returns_none_and_empty_iter() {
        let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let ctx = WsOpenContext::new(WsConnId::next(), sender, Vec::new());
        assert_eq!(ctx.param("id"), None);
        assert_eq!(ctx.params().count(), 0);
    }

    /// 複数パラメータを保持する `WsOpenContext` から `param`/`params` の
    /// 両方で取得できることを確認する（イシュー #676）。
    #[tokio::test]
    async fn open_context_with_params_exposes_param_and_params() {
        let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let params = vec![
            ("id".to_string(), "XYZ".to_string()),
            ("post_id".to_string(), "42".to_string()),
        ];
        let ctx = WsOpenContext::new(WsConnId::next(), sender, params);
        assert_eq!(ctx.param("id"), Some("XYZ"));
        assert_eq!(ctx.param("post_id"), Some("42"));
        assert_eq!(ctx.param("missing"), None);
        let collected: Vec<(&str, &str)> = ctx.params().collect();
        assert_eq!(collected, vec![("id", "XYZ"), ("post_id", "42")]);
    }

    /// 受け入れ基準 2: `WsConnId::next()` を並行に多数回呼んでも、発行される
    /// 値がすべて異なること（プロセス内一意性、イシュー #704）。
    #[tokio::test]
    async fn conn_id_next_is_unique_across_concurrent_callers() {
        const N: usize = 200;
        let mut handles = Vec::with_capacity(N);
        for _ in 0..N {
            handles.push(tokio::spawn(async { WsConnId::next() }));
        }
        let mut ids = std::collections::HashSet::with_capacity(N);
        for handle in handles {
            let id = handle.await.expect("task should not panic");
            assert!(
                ids.insert(id),
                "WsConnId::next() produced a duplicate: {id}"
            );
        }
        assert_eq!(ids.len(), N);
    }

    /// `WsConnId` の `Display` が内部値をそのまま 10 進表示することを確認する。
    #[tokio::test]
    async fn conn_id_display_shows_decimal_value() {
        let a = WsConnId::next();
        let b = WsConnId::next();
        // 単調増加のカウンタなので、後続の発行は必ず先行より大きい。
        assert!(a.to_string().parse::<u64>().unwrap() < b.to_string().parse::<u64>().unwrap());
    }

    /// 受け入れ基準 1: `WsConnContext` の各アクセサ（`conn_id`/`sender`/
    /// `param`/`params`）が構築時に渡した値をそのまま返すこと。
    #[tokio::test]
    async fn conn_context_accessors_return_constructed_values() {
        let (sender, mut rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let conn_id = WsConnId::next();
        let params = vec![("id".to_string(), "XYZ".to_string())];
        let ctx = WsConnContext::new(conn_id, sender, params);

        assert_eq!(ctx.conn_id(), conn_id);
        assert_eq!(ctx.param("id"), Some("XYZ"));
        assert_eq!(ctx.param("missing"), None);
        assert_eq!(ctx.params().collect::<Vec<_>>(), vec![("id", "XYZ")]);

        // `sender()` から送った値が対応する受信側へ届くこと。
        ctx.sender()
            .clone()
            .send(WsMessage::Text("via-ctx".to_string()))
            .await
            .unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received, WsMessage::Text("via-ctx".to_string()));
    }

    /// 受け入れ基準（`Debug` の機密混入防止）: `WsConnContext`/`WsOpenContext`
    /// の `Debug` 出力にパスパラメータの値が含まれないこと（攻撃者制御下の
    /// URL セグメントのため、`.claude/rules/security.md`）。
    #[tokio::test]
    async fn conn_context_and_open_context_debug_redact_params() {
        const SECRET: &str = "SECRET-XYZ";

        let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let conn_ctx = WsConnContext::new(
            WsConnId::next(),
            sender,
            vec![("token".to_string(), SECRET.to_string())],
        );
        let conn_debug = format!("{conn_ctx:?}");
        assert!(
            !conn_debug.contains(SECRET),
            "WsConnContext::Debug leaked a path parameter value: {conn_debug}"
        );

        let (sender2, _rx2) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let open_ctx = WsOpenContext::new(
            WsConnId::next(),
            sender2,
            vec![("token".to_string(), SECRET.to_string())],
        );
        let open_debug = format!("{open_ctx:?}");
        assert!(
            !open_debug.contains(SECRET),
            "WsOpenContext::Debug leaked a path parameter value: {open_debug}"
        );
    }

    /// 受け入れ基準 3（後方互換）: `on_message` のみを実装したハンドラで
    /// 既定の `on_message_with_ctx` を呼ぶと、`on_message` を直接呼んだ
    /// 場合と同じ結果になること。
    #[tokio::test]
    async fn default_on_message_with_ctx_delegates_to_on_message() {
        let handler = UppercaseHandler;
        let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let ctx = WsConnContext::new(WsConnId::next(), sender, Vec::new());

        let via_ctx = handler
            .on_message_with_ctx(&ctx, WsMessage::Text("hi".to_string()))
            .await
            .unwrap();
        let direct = handler
            .on_message(WsMessage::Text("hi".to_string()))
            .await
            .unwrap();
        assert_eq!(via_ctx, direct);
        assert_eq!(
            via_ctx,
            WsOutcome::Reply(vec![WsMessage::Text("HI".to_string())])
        );
    }
}
