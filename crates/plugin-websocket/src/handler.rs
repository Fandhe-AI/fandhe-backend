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
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;

use futures_util::future::BoxFuture;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

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
    ///
    /// 送信キューの flush（イシュー #711）: `on_message`（`on_message_with_ctx`）
    /// 内で本 variant を返す前に [`WsSender::send`] で push したメッセージは、
    /// **`WebSocketConfig::close_grace`（既定 10 秒）の期限内に送出できた
    /// 範囲で** Close フレームより先に送出される。Close 処理の開始時点で
    /// 送信キューは閉じられ（`crate::session::flush_outbound`）、以後の
    /// `WsSender::send` 呼び出しはすべて [`WsSendError`] で失敗する（同じ
    /// クローンを保持する別タスクからの送信も対象）。クライアントが受信を
    /// 止めている等で `close_grace` を超過した場合、残りのキュー済み
    /// メッセージは送出されずに破棄され、Close フレーム自体も送らずに
    /// セッションを即座に終了する（二次 DoS 対策、Codex レビュー指摘対応。
    /// `crate::session::FlushOutcome::TimedOut` 参照）。「必ず先に送出」は
    /// `close_grace` 内に収まる場合の契約であり、無条件の保証ではない。
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

/// セッション（`crate::session::run_session`）がどの経路で終了したかを表す
/// 終了理由（イシュー #726、親 #705。設計は
/// `docs/design/ws-connection-context-and-close.md` 4 節）。
///
/// `crate::session::run_session_inner` が全終了経路（クライアントの
/// Close・EOF・idle timeout・shutdown/rebind キャンセル・受信上限超過・
/// プロトコル/IO エラー・ハンドラの Close・ハンドラのエラー）ごとに
/// 値を算出する。`crate::session::run_session`（既存の公開シグネチャを保つ
/// 薄いラッパー）がこの値を [`WsMessageHandler::on_close`] へ渡してから
/// 従来どおりの `Result<(), WsError>` を返す（イシュー #729。`on_open` が
/// 呼ばれた接続についてのみちょうど 1 回呼ぶフェイルクローズ対称契約は
/// [`WsMessageHandler::on_close`] の doc を参照）。
///
/// # 情報露出の最小化（`.claude/rules/security.md`）
///
/// クライアントが送った Close reason 文字列・URL パラメータ等の payload を
/// 一切保持しない `Copy` な種別値のみで構成する。`Debug` 出力にも機密は
/// 含まれない。
///
/// # 網羅性
///
/// `#[non_exhaustive]` のため、下流の `match` はワイルドカード腕
/// （`_ => ...`）を必要とする。将来 variant を追加してもこれは
/// 非破壊変更（0.4.2 以降のバージョン方針、設計 7 節）として扱う。
///
/// ```
/// use fandhe_backend_plugin_websocket::handler::{CloseReason, FailureKind};
///
/// fn describe(reason: CloseReason) -> &'static str {
///     match reason {
///         CloseReason::ClientClose => "client closed",
///         CloseReason::Eof => "eof",
///         CloseReason::IdleTimeout => "idle timeout",
///         CloseReason::Cancelled => "cancelled",
///         CloseReason::HandlerClose => "handler closed",
///         CloseReason::MessageTooLarge => "message too large",
///         CloseReason::Failed(FailureKind::Io) => "io failure",
///         // `#[non_exhaustive]` のため他の `Failed(_)` はワイルドカードで拾う。
///         _ => "other",
///     }
/// }
///
/// let reason = CloseReason::ClientClose;
/// assert_eq!(describe(reason), "client closed");
/// // `Copy` + `PartialEq` を持つため値のコピー・比較ができる。
/// let copied = reason;
/// assert_eq!(reason, copied);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// クライアントが Close フレームを送出した（正常終了）。
    ClientClose,
    /// Close ハンドシェイクなしに接続が切断された（読み取り EOF）。
    ///
    /// 到達経路は 2 つある（`crate::session::SessionFailure::recv` が
    /// 分類）。(1) `ws.next()` が `None` を返す経路（`ConnectionClosed`/
    /// `AlreadyClosed` 到達後の fused 呼び出し等）で、この場合セッション
    /// 側の `Result` は `Ok(())`。(2) tokio-tungstenite 0.30 で Close
    /// フレームなしの TCP 切断が観測される主経路である
    /// `tungstenite::Error::Protocol(ProtocolError::
    /// ResetWithoutClosingHandshake)`（イシュー #726 レビュー指摘対応で
    /// 本 variant へ分類するようになった）で、この場合 `Result` は
    /// `Err(WsError::Protocol(_))`（読み取り自体は失敗している）。
    /// いずれも「Close ハンドシェイクなしの切断」という本 variant の
    /// 定義に一致する。
    Eof,
    /// `WebSocketConfig::idle_timeout` の期限内にクライアントからの
    /// フレームが届かず、アイドルと判定してサーバー側から切断した。
    IdleTimeout,
    /// コアの世代キャンセルシグナル（最終 graceful shutdown・rebind
    /// 世代 drain）発火によりサーバー側から切断した。
    Cancelled,
    /// ユーザーハンドラ（`WsMessageHandler`）が `WsOutcome::Close` を
    /// 返し、サーバー側から Close ハンドシェイクを開始した。
    HandlerClose,
    /// 受信メッセージが `max_message_size` / `max_frame_size` を超過した
    /// （tungstenite 側で強制、`tungstenite::Error::Capacity` 経由）。
    MessageTooLarge,
    /// 上記以外の失敗で終了した。詳細種別は [`FailureKind`] のみを運び、
    /// `WsError`（I/O・プロトコルエラーの詳細）自体はここには含まれない
    /// （`WsError` は `Clone` を実装しないため、また情報露出を最小化する
    /// ため。エラー詳細は `Result` 側（`crate::session::run_session_inner`
    /// の戻り値の第 2 要素）から取得する）。
    Failed(FailureKind),
    /// [`WsSender::close`] によるサーバー起点の Close ハンドシェイク
    /// （イシュー #710）。ハンドラの戻り値 `WsOutcome::Close`
    /// （[`Self::HandlerClose`]）とは呼び出し経路が異なり、`on_open` 等から
    /// 任意タイミングで（`on_message` の外からも）呼べる `WsSender::close`
    /// によって開始された終了を表す。close 時点でキュー済みだった
    /// [`WsSender::send`] の push はすべてクライアントへ届いてから Close
    /// フレームが送出される（順序保証、[`WsSender::close`] の doc 参照）。
    SenderClose,
}

/// [`CloseReason::Failed`] が運ぶ失敗の種別。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 送受信中の I/O エラー（`tungstenite::Error::Io`）。
    Io,
    /// プロトコル違反・容量超過以外の tungstenite エラー
    /// （`tungstenite::Error::Protocol` や `ConnectionClosed`/
    /// `AlreadyClosed` を伴う送信失敗等）。
    Protocol,
    /// ユーザーハンドラ（`WsMessageHandler::on_message_with_ctx`）が
    /// `Err` を返した（`WsHandlerError` 相当）。
    Handler,
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

    /// セッション終了時（[`Self::on_open`] を呼んだ接続についてのみ）に
    /// ちょうど 1 回呼ばれる切断通知フック（イシュー #729、親 #705。設計は
    /// `docs/design/ws-connection-context-and-close.md` 4 節・9 節）。
    ///
    /// 既定実装は `ctx`・`reason` を無視する no-op で、既存ハンドラ
    /// （`on_message`/`on_open` のみを実装したもの）は無変更のまま
    /// コンパイル・動作する（後方互換）。接続単位の状態（CDP 互換サーバーの
    /// 購読レジストリ等）の後片付けや、切断理由の診断ログ出力に使う。
    ///
    /// # 呼ばれる条件・呼ばれない条件（`on_open` との対称性、フェイルクローズ）
    ///
    /// - [`Self::on_open`] が呼ばれた接続（101 応答送出成功後）についてのみ、
    ///   `crate::session::run_session` がセッション終了時に必ず 1 回呼ぶ
    ///   （`run_session_inner` の戻り値をラッパーが分解して呼ぶ構造上、
    ///   呼び出し箇所が 1 つしかないため、個々の脱出点に呼び出しを散らさず
    ///   ちょうど 1 回になる。`docs/design/ws-connection-context-and-close.md`
    ///   4 節の脱出点対応表を参照）。
    /// - ハンドシェイク検証失敗（400/426 応答）・101 応答送出前に世代
    ///   キャンセルが既に発火していた接続では呼ばれない（`on_open` が
    ///   呼ばれていない接続へ切断通知も渡さない、フェイルクローズの対称性。
    ///   `crate::handle_upgrade` の doc を参照）。
    ///
    /// # 保証外（既知の限界）
    ///
    /// 次の場合は本メソッドが呼ばれない、または呼ばれた後の状態について
    /// 追加の保証をしない（設計 4 節「不変条件」参照。本メソッド自身が
    /// panic しないことは実装者の責務であり、ここでの保証外には含めない）。
    ///
    /// - [`Self::on_message_with_ctx`]（既定実装経由の [`Self::on_message`]
    ///   を含む）や [`Self::on_open`] が panic した場合
    /// - プロセスの kill やランタイムの強制 drop によりセッションタスク自体が
    ///   実行を継続できなくなった場合
    ///
    /// # `reason` から取得できる情報
    ///
    /// `reason` は [`CloseReason`]（`#[non_exhaustive]`）で、クライアントが
    /// 送った Close reason 文字列等の payload は一切含まない種別値のみを
    /// 運ぶ（情報露出の最小化、`.claude/rules/security.md`）。失敗の詳細
    /// （`WsError`）自体は本メソッドには渡らず、`handle_upgrade` の戻り値
    /// （`Result`）側からのみ取得できる契約は変更しない（設計 4 節・10 節。
    /// `WsError` は `Clone` を実装しないため、また情報露出を最小化する
    /// ため）。
    ///
    /// # 呼び出し時点の `ctx` の状態
    ///
    /// 呼ばれた時点で `WebSocketStream` も outbound チャネルの受信側も
    /// drop 済みである。そのため `ctx.sender().send(..)` を呼んでも常に
    /// [`WsSendError`] になる。
    ///
    /// # 同期フック・実行コンテキスト
    ///
    /// [`Self::on_open`] と同じく本メソッドは**同期**（非 `async`）で、
    /// セッションタスク内で呼ばれる。呼ばれた時点ではコアの
    /// `max_connections` permit をまだ保持しており、`WebSocketConfig::
    /// close_grace` の上限の外で実行される。重い処理・ブロッキング処理は
    /// permit の解放と graceful shutdown の完了を遅らせるため、非同期処理を
    /// 要する場合は `tokio::spawn` で切り離す（`on_open` と同じ原則。
    /// ライブラリ側は本メソッドの実行にタイムアウトを設けない）。
    ///
    /// # Examples
    ///
    /// （`handle_upgrade` を実際に駆動し、クライアントが Close フレームを
    /// 送出した経路で `on_close` がちょうど 1 回・`CloseReason::ClientClose`
    /// で呼ばれることを確認する。`Arc<Mutex<Vec<CloseReason>>>` へ記録する
    /// パターンは、接続単位の後片付けを行うハンドラの基本形。）
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use std::time::Duration;
    /// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    /// use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
    /// use fandhe_backend_plugin_websocket::handler::{
    ///     CloseReason, WsConnContext, WsHandlerError, WsMessage, WsMessageHandler, WsOutcome,
    /// };
    /// use futures_util::future::BoxFuture;
    /// use futures_util::SinkExt;
    /// use tokio::io::AsyncReadExt;
    /// use tokio_tungstenite::WebSocketStream;
    /// use tokio_tungstenite::tungstenite::protocol::Role;
    ///
    /// struct RecordCloses(Arc<Mutex<Vec<CloseReason>>>);
    ///
    /// impl WsMessageHandler for RecordCloses {
    ///     fn name(&self) -> &'static str {
    ///         "record-closes"
    ///     }
    ///
    ///     fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
    ///         Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    ///     }
    ///
    ///     fn on_close(&self, _ctx: &WsConnContext, reason: CloseReason) {
    ///         self.0.lock().unwrap().push(reason);
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
    /// let closes = Arc::new(Mutex::new(Vec::new()));
    /// let config = WebSocketConfig::default().with_handler(RecordCloses(closes.clone()));
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
    /// client.close(None).await.ok();
    ///
    /// // `on_close` はセッションタスク内で同期に呼ばれるため、タスクを
    /// // join し終えた時点で実行済みであることが保証される（`sleep` は使わない）。
    /// let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
    ///
    /// let recorded = closes.lock().unwrap();
    /// assert_eq!(recorded.as_slice(), &[CloseReason::ClientClose]);
    /// # }
    /// ```
    fn on_close(&self, ctx: &WsConnContext, reason: CloseReason) {
        let _ = (ctx, reason);
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
///
/// # 送信キューを流れる内部表現（イシュー #710）
///
/// 公開 API 上はメッセージ（[`Self::send`]）と Close 指示（[`Self::close`]）の
/// 2 系統に見えるが、内部の bounded mpsc（`tx`）は両方を単一の
/// `OutboundItem` として同一キューへ直列に流す。これにより「close 時点で
/// キュー済みだった push はすべて Close より前にクライアントへ届く」という
/// 順序保証が、mpsc の FIFO 特性だけで構造的に成り立つ（2 本の別チャネルに
/// 分けると、消費側でのマージ順序を別途保証する必要が生じる。`crate::session`
/// の消費側は `OutboundItem` を分岐して処理する）。
///
/// `closing`（`Arc<Mutex<bool>>`、全 clone で共有）は「以後の enqueue を拒否
/// するか（[`Self::close`] の確定済み、またはセッションの終了処理で送信キューを
/// 封鎖済み）」の単一の真実源であり、`Self::commit` が enqueue 判定と同一ロック区間で
/// 読み書きすることで、「フラグ確認 → enqueue」の間に別タスクの `close` が
/// 割り込んで Close の後ろへメッセージが積まれる TOCTOU を排除する
/// （`.claude/rules/coding-rust.md` の「ロック保持中の `.await` を避ける」を
/// 守るため、ロックを取る前に `reserve()`/`try_reserve()` で `Permit` を
/// 確保し、ロック内では同期的な `Permit::send` のみを行う）。
///
/// # close 確定・封鎖を待機中の呼び出しへ即時伝える仕組み（PR #736 レビュー指摘対応）
///
/// `reserve()` は送信キューが満杯だと Ready にならないため、キュー満杯時に
/// 別 clone の [`Self::close`] が確定しても（またはセッションが送信キューを
/// 封鎖しても）、キューが実際にドレインされる
/// （あるいは受信側 `Receiver` が drop される）まで、保留中の [`Self::send`]/
/// [`Self::close`] は `WsSendError`/`WsCloseError::Closed` を返せない
/// （満杯キュー上の無関係な push の実配送速度に応答時間が従属してしまう。
/// `.claude/rules/security.md` のリソース枯渇対策上望ましくない）。
/// `closed_signal`（`Arc<watch::Sender<bool>>`）で `Self::commit`・
/// `Self::seal_for_session` が `closing` を true にした直後にブロードキャストし、
/// `Self::reserve_or_closed`
/// が `reserve()` とこの信号を手動 race させることで、キューの実ドレインを
/// 待たず即座に解放する。
#[derive(Clone)]
pub struct WsSender {
    tx: mpsc::Sender<OutboundItem>,
    /// 以後の enqueue を拒否するかどうかの単一の真実源（全 clone で共有）。
    /// [`Self::close`] の確定時、またはセッションが終了経路で送信キューを
    /// 封鎖した時（`Self::seal_for_session`）に true になる（`Self::commit`
    /// の doc を参照）。
    closing: Arc<Mutex<bool>>,
    /// close 確定・セッションの封鎖を待機中の呼び出しへ伝える watch シグナル
    /// （[`Self`] の doc を参照）。`watch::Sender::send` は `&self` で呼べるため
    /// `Arc` 越しに全 clone から共有できる。
    closed_signal: Arc<watch::Sender<bool>>,
    /// `closed_signal` の受信側を最低 1 個生存させ続けるための保持専用
    /// clone（watch チャネルは全 `Receiver` が drop されると `send` が
    /// 更新を伝えられなくなる。実際の待機は各呼び出しが
    /// `closed_signal.subscribe()` で作る一時 `Receiver` が担うため、
    /// 本フィールド自身の値は読まない）。
    _closed_signal_anchor: watch::Receiver<bool>,
}

/// [`WsSender`] の送信キューを流れる内部アイテム（`pub(crate)`、イシュー
/// #710）。[`WsMessageHandler`] 等の公開 API には出さず、`crate::session`
/// からのみ分岐・消費される。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OutboundItem {
    /// [`WsSender::send`] が enqueue したユーザー起点の push メッセージ。
    Message(WsMessage),
    /// [`WsSender::close`] が enqueue したサーバー起点の Close 指示。
    /// `code`/`reason` は [`WsSender::close`] の呼び出し時点で検証済み
    /// （RFC 6455 で送信が許される code・123 バイト以内の reason）。
    Close {
        /// close code（検証済み）。
        code: u16,
        /// close reason（検証済み、123 バイト以内）。
        reason: String,
    },
}

/// [`WsSender::close`] が受け付ける close reason の最大バイト長（イシュー
/// #710）。制御フレームの payload 上限（125 バイト）から close code 分の
/// 2 バイトを引いた値（RFC 6455 5.5 節）。
const MAX_CLOSE_REASON_BYTES: usize = 123;

impl WsSender {
    /// `msg` をセッションの送信キューへ入れる。
    ///
    /// チャネルが満杯の場合、受信側（`crate::session::run_session`）が
    /// キューを消費するまで `.await` で待機する契約（
    /// `crates/core/src/streaming.rs` の `BodyWriter::send` と同型の
    /// バックプレッシャ。無制限バッファ化を防ぐリソース枯渇 DoS 対策、
    /// `.claude/rules/security.md`）。
    ///
    /// セッションが既に終了している場合（受信側が drop 済み）、または
    /// 既に [`Self::close`] が呼ばれている場合は [`WsSendError`] を返す
    /// （イシュー #710 で追加。close 後の送信を一貫して拒否するフェイル
    /// クローズ契約）。セッションの世代キャンセル（最終 graceful
    /// shutdown・rebind 世代 drain）発火時は、ブロック中の呼び出しも
    /// `WebSocketConfig::close_grace` の満了を待たず即座にこのエラーで
    /// 解放される（`crate::session` が cancel 発火時に受信側 `Receiver` を
    /// 明示的に drop するため。イシュー #670 の受け入れ基準 3）。
    ///
    /// 送信キューが満杯の状態で本呼び出しが `reserve()` 待ちに入っている
    /// 間に別 clone の [`Self::close`] が確定した場合も、キューの実ドレインを
    /// 待たず即座に [`WsSendError`] を返す（`Self::reserve_or_closed` の
    /// doc を参照、PR #736 レビュー指摘対応）。
    ///
    /// `WsOutcome::Close`（イシュー #711）: ハンドラが `WsOutcome::Close`（または
    /// `Err`）を返した時点で、`crate::session::flush_outbound` が送信キューを
    /// 封鎖し、受信側を `close()`（drop ではなく）する。これにより、`ws.close()` の送出完了を
    /// 待たず（応答を読まないクライアント相手では送出自体が長時間ブロック
    /// しうる）、ブロック中の本メソッド呼び出しも即座にこのエラーで
    /// 解放される。閉鎖より前に本メソッドが `Ok` を返したメッセージは、
    /// `WebSocketConfig::close_grace`（既定 10 秒）の超過・世代キャンセル・
    /// 排出中の送信失敗で打ち切られない限り、セッション終了前に送出される
    /// （`WsOutcome::Close` の場合は Close フレームより先。ハンドラ `Err` の
    /// 場合は Close フレームを送らずに終了する）。打ち切られた場合、残りの
    /// メッセージと Close フレームは送出されない（二次 DoS 対策。
    /// `crate::session::FlushOutcome::TimedOut` を参照）。
    pub async fn send(&self, msg: WsMessage) -> Result<(), WsSendError> {
        let permit = self.reserve_or_closed().await.map_err(|()| WsSendError)?;
        self.commit(permit, OutboundItem::Message(msg), false)
            .map_err(|()| WsSendError)
    }

    /// サーバー起点で Close ハンドシェイクを開始する（イシュー #710、親
    /// #708「サーバー起点で任意タイミングに Close を送れる WebSocket
    /// API」）。`on_message` の戻り値 `WsOutcome::Close`
    /// （[`CloseReason::HandlerClose`]）と異なり、`on_open` から `spawn`
    /// したタスク等、ハンドラの外からも任意のタイミングで呼べる。
    ///
    /// # 順序保証
    ///
    /// 本呼び出しより前に [`Self::send`] が `Ok` を返した push メッセージは、
    /// すべてクライアントへ届いてから Close フレームが送出される
    /// （[`Self`] の「送信キューを流れる内部表現」節を参照。単一の bounded
    /// mpsc を経由するため FIFO 順序が保たれる）。close の `reserve()` 待ち
    /// 中に先に permit を得た `send` は Close より前に並ぶ。close が
    /// 確定した**後**に enqueue しようとした `send` は必ず `Err` になる
    /// （`Self::commit` が同一ロック区間で判定するため、「送信済みなのに
    /// Close の後ろへ積まれて破棄される」という静かなデータ欠落は起こらない）。
    ///
    /// セッションがハンドラの `Err`/`WsOutcome::Close` で送信キューを封鎖する
    /// 処理と競合した場合、本メソッドは封鎖より前に確定すれば `Ok` を返し、
    /// その Close は `close_grace` 超過・世代キャンセル・排出中の送信失敗で
    /// 打ち切られない限り送出される。封鎖より後なら
    /// [`WsCloseError::Closed`] を返す（`crate::session::flush_outbound` の
    /// doc を参照）。
    ///
    /// # 検証（RFC 6455 7.4 節・5.5 節）
    ///
    /// - `code`: 送信が許されない値（`<1000`・`1005`・`1006`・`1015`・
    ///   予約域 `1016..=2999`・`>=5000`）は [`WsCloseError::InvalidCode`] を
    ///   返す。この検証で拒否した場合、close 済みフラグは立たず、以後の
    ///   `send`/`close` は通常どおり成功しうる（不正なフレームを実際には
    ///   送出しない入力検証、`.claude/rules/security.md`）。
    /// - `reason`: UTF-8 は `&str` の型で保証されるが、バイト長が
    ///   `MAX_CLOSE_REASON_BYTES`（123 バイト、制御フレーム payload 上限
    ///   125 バイトから close code 2 バイトを引いた値）を超える場合は
    ///   [`WsCloseError::ReasonTooLong`] を返す（検証失敗時も close 済み
    ///   フラグは立たない）。
    ///
    /// # 2 回目以降の呼び出し・セッション終了後の呼び出し
    ///
    /// 検証を通過した後、既に close 済み（2 回目以降の呼び出し）、または
    /// セッションが既に終了している（受信側 drop 済み）場合は
    /// [`WsCloseError::Closed`] を返す（フェイルクローズ、[`Self::send`] の
    /// close 後の挙動と一貫させる）。
    ///
    /// # 完了タイミング
    ///
    /// `.await` から戻るのは Close 指示をキューへ enqueue した時点であり、
    /// ワイヤ上の送出完了を待たない（[`Self::send`] と同型のバック
    /// プレッシャ契約。チャネルが満杯なら受信側が消費するまで待機する）。
    ///
    /// 本メソッドが `Ok` を返した後、セッションは close の確定を観測した
    /// 時点（通常は close 確定の直後）から `WebSocketConfig::close_grace`
    /// （既定 10 秒）以内に、先行する push の送出・Close フレームの送出・応答
    /// 待ちを終えるか、接続を打ち切る（クライアントが受信を止めていても有界。
    /// 打ち切った場合の終了理由も `CloseReason::SenderClose`）。ただし、その
    /// 間に世代キャンセル・idle timeout が先に発火した場合は、その経路の契約
    /// （発火時点から `close_grace`、終了理由 `Cancelled`/`IdleTimeout`）に従う。
    /// close 未確定時の push の送出には期限を設けない。
    ///
    /// ワイヤ上の完了（セッション終了）を待ちたい場合の代替として
    /// [`Self::closed`] があるが、本メソッドが起こす `SenderClose` 経路
    /// （`crate::session`）では、実際に Close フレームを書き込みピアの
    /// 応答をドレインする処理（`close_and_drain`）より**前**に受信側
    /// `Receiver` が drop される。そのため [`Self::closed`] は Close
    /// フレームがワイヤへ送出される前、セッションが本当に終了するより
    /// 早い時点で完了しうる（cancel・idle timeout 経路と同じ挙動。
    /// 詳細・完了時点の一次情報は [`Self::closed`] の doc を参照）。
    ///
    /// # ハンドラ実行中に呼ぶ場合の注意（[`Self::closed`] とは異なる）
    ///
    /// [`Self::closed`] とは異なり、本メソッドを `on_message`/
    /// `on_message_with_ctx` の実装の中でインライン `await` してもデッド
    /// ロックしない（`crate::session::run_handler_with_outbound_drain` が
    /// ハンドラ実行中も outbound チャネルを消化し続けるため）。ただし、
    /// セッションがハンドラの戻り値の送出を始める直前に close の確定を判定する
    /// 時点（`WsOutcome::Reply` は送信キューと同じロックでの確認、
    /// `WsOutcome::Close` は送信キューの封鎖）より前に本メソッドが確定（同じ
    /// ロック区間での close 済みフラグの更新）していれば、その戻り値は**破棄
    /// され送出されない**（ハンドラの実行中か完了後の送信キュー排出中かを
    /// 問わない。RFC 6455 5.5.1 節: Close フレームの後にデータフレームを送れない
    /// ため）。判定より後に確定した場合、`Reply` は Close より先に送出されうる
    /// （close 確定の観測から `close_grace` で打ち切る）。呼び出し後に返す値に意味を持たせ
    /// たい場合は、本メソッドを `on_open` 等から `tokio::spawn` した別
    /// タスクから呼ぶ構成にする（ハンドラ自身の戻り値と競合しない）。
    ///
    /// # Examples
    ///
    /// （`on_open` で spawn したタスクから push を数件送ったあと
    /// `close(4000, "bye")` を呼ぶ。クライアントは push を順に受け取り、
    /// 最後に code 4000・reason "bye" の Close を受け取る。クライアントが
    /// 受信を止めていた場合も、セッションは close 確定から `close_grace`
    /// 以内に終わる。）
    ///
    /// ```
    /// use std::time::Duration;
    /// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    /// use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
    /// use fandhe_backend_plugin_websocket::handler::{
    ///     WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome,
    /// };
    /// use futures_util::future::BoxFuture;
    /// use futures_util::{SinkExt, StreamExt};
    /// use tokio::io::AsyncReadExt;
    /// use tokio_tungstenite::WebSocketStream;
    /// use tokio_tungstenite::tungstenite::protocol::Role;
    ///
    /// struct PushThenClose;
    ///
    /// impl WsMessageHandler for PushThenClose {
    ///     fn name(&self) -> &'static str {
    ///         "push-then-close"
    ///     }
    ///
    ///     fn on_open(&self, ctx: WsOpenContext) {
    ///         let sender = ctx.sender().clone();
    ///         tokio::spawn(async move {
    ///             for i in 0..3 {
    ///                 let _ = sender.send(WsMessage::Text(format!("push-{i}"))).await;
    ///             }
    ///             let _ = sender.close(4000, "bye").await;
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
    /// let config = WebSocketConfig::default().with_handler(PushThenClose);
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
    ///
    /// for i in 0..3 {
    ///     let msg = tokio::time::timeout(Duration::from_secs(2), client.next())
    ///         .await
    ///         .expect("push should arrive within timeout")
    ///         .expect("stream should not end")
    ///         .expect("frame should not error");
    ///     assert_eq!(msg.into_text().unwrap(), format!("push-{i}"));
    /// }
    ///
    /// let close_frame = tokio::time::timeout(Duration::from_secs(2), client.next())
    ///     .await
    ///     .expect("close frame should arrive within timeout")
    ///     .expect("stream should not end")
    ///     .expect("frame should not error");
    /// match close_frame {
    ///     tokio_tungstenite::tungstenite::Message::Close(Some(frame)) => {
    ///         assert_eq!(u16::from(frame.code), 4000);
    ///         assert_eq!(frame.reason.as_str(), "bye");
    ///     }
    ///     other => panic!("expected a close frame, got {other:?}"),
    /// }
    ///
    /// client.close(None).await.ok();
    /// let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
    /// # }
    /// ```
    pub async fn close(&self, code: u16, reason: &str) -> Result<(), WsCloseError> {
        if !CloseCode::from(code).is_allowed() {
            return Err(WsCloseError::InvalidCode);
        }
        if reason.len() > MAX_CLOSE_REASON_BYTES {
            return Err(WsCloseError::ReasonTooLong);
        }
        let permit = self
            .reserve_or_closed()
            .await
            .map_err(|()| WsCloseError::Closed)?;
        self.commit(
            permit,
            OutboundItem::Close {
                code,
                reason: reason.to_string(),
            },
            true,
        )
        .map_err(|()| WsCloseError::Closed)
    }

    /// `permit` を使って `item` を確定送出する共通ヘルパー（イシュー #710）。
    ///
    /// close 済みフラグの確認 → （`closing_after` なら）フラグを立てる →
    /// `permit.send`（同期）の 3 手順を [`Self::closing`] の同一ロック区間で
    /// 行うことで、「フラグ確認と enqueue の間に別タスクの [`Self::close`]
    /// が割り込んで Close の後ろへメッセージが積まれる」という TOCTOU を
    /// 構造的に排除する（[`Self`] の doc を参照。ロック保持中は `.await`
    /// しない、`.claude/rules/coding-rust.md`）。
    ///
    /// 既に close 済みの場合は `permit` を drop して `Err(())` を返す
    /// （呼び出し元が [`WsSendError`]/[`WsCloseError::Closed`] へ変換する）。
    ///
    /// **セッション側の封鎖との関係（PR #736 レビュー指摘対応）**:
    /// `crate::session::flush_outbound` は [`Self::seal_for_session`] で
    /// 同じ `closing` ロックの区間内に封鎖状態を立てる。ロックの前後関係から、
    /// 封鎖より前に本メソッドが `Ok` を返した項目は封鎖時点でキューに入って
    /// おり、封鎖より後の呼び出しは `Err` になる。このため同関数は permit
    /// 保持者の確定を待たず、`try_recv()` だけで排出を終えられる（tokio の
    /// 受信側の起床挙動に依存しない）。
    ///
    /// `closing_after` で close 済みへ遷移させた場合は、ロック解放後に
    /// `closed_signal` へブロードキャストし、送信キュー満杯で
    /// `Self::reserve_or_closed` の中で保留中の他 clone を即時に解放する
    /// （PR #736 レビュー指摘対応。`send` はロックを保持したまま行わない
    /// ため `.claude/rules/coding-rust.md` の制約を破らない）。
    fn commit(
        &self,
        permit: mpsc::Permit<'_, OutboundItem>,
        item: OutboundItem,
        closing_after: bool,
    ) -> Result<(), ()> {
        {
            let mut closed = self.closing.lock().unwrap_or_else(PoisonError::into_inner);
            if *closed {
                drop(permit);
                return Err(());
            }
            if closing_after {
                *closed = true;
            }
            permit.send(item);
        }
        if closing_after {
            // 受信側は `_closed_signal_anchor` が最低 1 個生存を保証するため
            // 送信は必ず成功する（戻り値は無視してよい）。
            let _ = self.closed_signal.send(true);
        }
        Ok(())
    }

    /// セッションの終了処理で送信キューを封鎖する（`pub(crate)`、
    /// `crate::session` 専用。PR #736 レビュー指摘対応）。`crate::session` は
    /// 受信側を閉じる・drop する前に必ず本メソッドを呼ぶ（受信側を所有する
    /// `OutboundGuard` の `seal`/`release`/`Drop` が呼ぶ）。
    ///
    /// `closing` を [`Self::commit`] と同じロック区間で true にし、以後の
    /// [`Self::send`]/[`Self::close`] を `Err` にする。ロック解放後に
    /// `closed_signal` を送り、送信キュー満杯で待機中の呼び出しを解放する。
    /// ロック保持中は `.await` せず、ほかのロックも取らない（デッドロックの
    /// 余地がない）。何度呼んでもよい。
    pub(crate) fn seal_for_session(&self) {
        *self.closing.lock().unwrap_or_else(PoisonError::into_inner) = true;
        // 受信側は `_closed_signal_anchor` が最低 1 個生存を保証するため
        // 送信は必ず成功する（戻り値は無視してよい）。
        let _ = self.closed_signal.send(true);
    }

    /// `closing` を [`Self::commit`] と同じロックで読み、[`Self::close`] の確定
    /// （またはセッションの封鎖）の有無を返す（`pub(crate)`、`crate::session`
    /// 専用。PR #736 codex P1 指摘対応）。
    ///
    /// セッションは Reply の送出を始める直前（間に `.await` を挟まない）に本
    /// メソッドで判定し、`true` なら Reply を破棄して Close 指示の処理へ進む。
    /// ロックの前後関係から、`true` を返したときは `close()` の Close 指示が
    /// すでにキューにある。Reply を送る継続経路ではセッションは封鎖しない
    /// （封鎖する防御分岐では受信側を無効化する）ため、`true` は `close()` の
    /// 確定と判定してよい。
    pub(crate) fn close_committed(&self) -> bool {
        *self.closing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// close 確定（またはセッションの封鎖）を観測する watch 受信側を返す
    /// （`pub(crate)`、`crate::session` 専用。Cursor Bugbot 指摘対応）。
    /// セッションは close 確定を観測した時点から `close_grace` 以内に Close
    /// ハンドシェイクを終えるか接続を打ち切る（`crate::session::CloseBound`）。
    /// 返り値は購読時点の値を既読扱いにするため、呼び出し側は `borrow()` で
    /// 現在値を確かめてから `changed()` を待つこと。
    pub(crate) fn subscribe_closing(&self) -> watch::Receiver<bool> {
        self.closed_signal.subscribe()
    }

    /// [`Self::send`]/[`Self::close`] が使う共通の `reserve()` ラッパー
    /// （PR #736 レビュー指摘対応。`crate::session::race2` と同型の手動
    /// race で、`tokio::select!`（`tokio` の `macros` feature を要求する）
    /// は使わない）。
    ///
    /// `self.tx.reserve()` を `closed_signal` の変化と race させ、送信
    /// キューが満杯で `reserve()` が保留中でも、別 clone の [`Self::close`]
    /// が確定した時点、またはセッションが送信キューを封鎖した時点
    /// （`Self::seal_for_session`）で（キューの実ドレイン・受信側 `Receiver`
    /// の drop を待たず）即座に `Err(())` を返す。
    ///
    /// close 確定シグナルを `reserve()` より先にポーリングする bias を持つ
    /// （fail-closed。両方が同時に Ready でも close 側を勝たせる。逆でも
    /// `Self::commit` が同一ロック区間で再判定するため安全性上の実害はない）。
    ///
    /// `closed_signal.subscribe()` は `watch` のバージョン管理に基づくため、
    /// subscribe から本メソッドの `.await` 完了までの間に `Self::commit` が
    /// 値を更新しても見逃さない（`borrow()` は常に最新値を返し、
    /// `changed()` は subscribe 時点のバージョンより新しい更新を必ず捉える。
    /// `Self` の doc も参照）。
    async fn reserve_or_closed(&self) -> Result<mpsc::Permit<'_, OutboundItem>, ()> {
        let mut closed_rx = self.closed_signal.subscribe();
        if *closed_rx.borrow() {
            return Err(());
        }
        let mut reserve = std::pin::pin!(self.tx.reserve());
        let mut changed = std::pin::pin!(closed_rx.changed());
        std::future::poll_fn(|cx| {
            if changed.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(()));
            }
            if let Poll::Ready(result) = reserve.as_mut().poll(cx) {
                return Poll::Ready(result.map_err(|_| ()));
            }
            Poll::Pending
        })
        .await
    }

    /// セッションが outbound push を受け付けなくなるまで待つ（イシュー #727。
    /// `on_close`（#729、予定）を使わない最小の代替として、切断待ちの表現を
    /// 提供する）。
    ///
    /// # 意味・完了タイミング
    ///
    /// 完了する時点は [`WsSender::send`] が [`WsSendError`] を返し始める時点と
    /// 同じ（受信側 `mpsc::Receiver` の `close()` または drop）である。
    /// `crate::session` のすべての終了経路（正常終了・ハンドラエラー・
    /// `WsOutcome::Close`・プロトコルエラー・future の drop）でこの
    /// `Receiver` は最終的に drop される。cancel（世代キャンセル）経路・
    /// idle timeout 経路では、Close ハンドシェイクのドレインより**前**に
    /// drop されるため、`closed()` はその時点で完了する（[`WsSender::send`]
    /// の doc にある「`close_grace` の満了を待たず解放」と同じ時点）。
    /// `WsOutcome::Close` 経路（イシュー #711）・ハンドラ `Err` 経路では、
    /// `ws.close()` の送出・セッション終了の**前**に受信側が `close()` される
    /// ため（drop ではないが `Sender::
    /// closed()` は同様に完了する）、`closed()` も同じく `ws.close()` の
    /// 完了を待たずに完了する。
    ///
    /// どの clone から呼んでも、同じ時点で完了する（[`mpsc::Sender::closed`]
    /// への薄い委譲であり、送信側の数に依存しない）。
    ///
    /// tokio の `Sender::closed` と同じく cancel-safe なので、`select!` や
    /// `timeout` と組み合わせて打ち切ってよい。
    ///
    /// # デッドロックに関する注意
    ///
    /// `on_message` / `on_message_with_ctx` の実装の中でこの `Future` を
    /// インラインで `await` してはならない。ハンドラの `Future` が実行中は
    /// `crate::session::run_handler_with_outbound_drain` がクライアントの
    /// 受信ストリームを読まないため、受信側 `Receiver` はハンドラが返るまで
    /// drop されず、自己デッドロックになる（世代キャンセル発火時のみ解除
    /// される）。`closed()` は `on_open` / `on_message_with_ctx` から
    /// `tokio::spawn` した別タスクでのみ使うこと。
    ///
    /// # 既知の限界
    ///
    /// ランタイムがタスクを強制終了した場合の完了通知は保証しない
    /// （タスク自体が消え、待機している `.await` も評価されなくなるため）。
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use std::time::Duration;
    /// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    /// use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
    /// use fandhe_backend_plugin_websocket::handler::{
    ///     WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome, WsSender,
    /// };
    /// use futures_util::future::BoxFuture;
    /// use futures_util::{SinkExt, StreamExt};
    /// use tokio::io::AsyncReadExt;
    /// use tokio_tungstenite::WebSocketStream;
    /// use tokio_tungstenite::tungstenite::protocol::Role;
    ///
    /// struct CaptureHandler {
    ///     captured: Arc<Mutex<Option<WsSender>>>,
    /// }
    ///
    /// impl WsMessageHandler for CaptureHandler {
    ///     fn name(&self) -> &'static str {
    ///         "capture"
    ///     }
    ///
    ///     fn on_open(&self, ctx: WsOpenContext) {
    ///         *self.captured.lock().unwrap() = Some(ctx.sender().clone());
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
    /// let captured = Arc::new(Mutex::new(None));
    /// let config = WebSocketConfig::default().with_handler(CaptureHandler { captured: captured.clone() });
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
    ///
    /// // on_open の実行を保証するため 1 往復エコーする（101 応答を読めた
    /// // だけでは on_open 実行を保証しない）。
    /// client.send(tokio_tungstenite::tungstenite::Message::Text("hi".into())).await.unwrap();
    /// let _ = client.next().await;
    ///
    /// let sender = captured.lock().unwrap().clone().unwrap();
    /// assert!(!sender.is_closed());
    ///
    /// client.close(None).await.unwrap();
    /// while client.next().await.is_some() {}
    /// let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
    ///
    /// tokio::time::timeout(Duration::from_secs(2), sender.closed())
    ///     .await
    ///     .expect("closed() は有界時間内に完了する");
    /// assert!(sender.is_closed());
    /// # }
    /// ```
    pub async fn closed(&self) {
        self.tx.closed().await
    }

    /// セッションが outbound push を受け付けなくなっているかを判定する
    /// （イシュー #727）。
    ///
    /// [`WsSender::closed`] と同じ時点（受信側 `Receiver` の drop）、
    /// [`WsSender::close`] が確定した時点（イシュー #710 で追加）、または
    /// セッションの終了処理が送信キューを封鎖した時点のいずれか早い方で
    /// `false` から `true` へ変わる。後 2 者の後は「outbound push を受け
    /// 付けなくなっている」（[`Self::send`] が [`WsSendError`] を返す）状態に
    /// 既に入っているため、[`Self::closed`]（受信側 drop まで完了しない）より
    /// 先に `true` を返しうる。
    ///
    /// # 参考値であること（TOCTOU）
    ///
    /// この判定は呼び出し直後の [`WsSender::send`] の成否を保証しない
    /// （`false` を見た直後にセッションが終了しうる）。フェイルクローズな
    /// 正の判定基準は引き続き `send` が返す [`Result`] であり、`is_closed`
    /// は事前フィルタとしてのみ使う（`.claude/rules/security.md`）。
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use std::time::Duration;
    /// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    /// use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade};
    /// use fandhe_backend_plugin_websocket::handler::{
    ///     WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome, WsSender,
    /// };
    /// use futures_util::future::BoxFuture;
    /// use futures_util::{SinkExt, StreamExt};
    /// use tokio::io::AsyncReadExt;
    /// use tokio_tungstenite::WebSocketStream;
    /// use tokio_tungstenite::tungstenite::protocol::Role;
    ///
    /// struct CaptureHandler {
    ///     captured: Arc<Mutex<Option<WsSender>>>,
    /// }
    ///
    /// impl WsMessageHandler for CaptureHandler {
    ///     fn name(&self) -> &'static str {
    ///         "capture-is-closed"
    ///     }
    ///
    ///     fn on_open(&self, ctx: WsOpenContext) {
    ///         *self.captured.lock().unwrap() = Some(ctx.sender().clone());
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
    /// let captured = Arc::new(Mutex::new(None));
    /// let config = WebSocketConfig::default().with_handler(CaptureHandler { captured: captured.clone() });
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
    /// client.send(tokio_tungstenite::tungstenite::Message::Text("hi".into())).await.unwrap();
    /// let _ = client.next().await;
    ///
    /// let sender = captured.lock().unwrap().clone().unwrap();
    /// assert!(!sender.is_closed());
    ///
    /// client.close(None).await.unwrap();
    /// while client.next().await.is_some() {}
    /// tokio::time::timeout(Duration::from_secs(2), server_task)
    ///     .await
    ///     .expect("session should end within timeout")
    ///     .unwrap();
    ///
    /// assert!(sender.is_closed());
    /// # }
    /// ```
    #[must_use]
    pub fn is_closed(&self) -> bool {
        *self.closing.lock().unwrap_or_else(PoisonError::into_inner) || self.tx.is_closed()
    }

    /// テスト専用: 送信キューの現在の空き容量（`mpsc::Sender::capacity`）。
    #[cfg(test)]
    pub(crate) fn capacity_for_test(&self) -> usize {
        self.tx.capacity()
    }

    /// テスト専用: [`Self::close`] の permit 確保だけを行い、確定（`commit`）を
    /// 呼び出し元へ委ねる（PR #736 レビュー指摘の回帰テスト用）。
    ///
    /// 本番の [`Self::close`] は permit 確保から `commit` までを同期的に
    /// （間に `.await` を挟まず）行うが、マルチスレッドランタイムでは
    /// その同期区間の最中に別スレッドのセッションが受信側を `close()`
    /// しうる。本ヘルパーと [`Self::commit_close_for_test`] の 2 段に分けることで、
    /// 「permit 保持中に受信側が閉じられ、その後に Close が確定する」
    /// 順序を単一スレッドのテストで決定的に再現する。
    #[cfg(test)]
    pub(crate) async fn reserve_close_for_test(
        &self,
    ) -> Result<mpsc::Permit<'_, OutboundItem>, WsCloseError> {
        self.reserve_or_closed()
            .await
            .map_err(|()| WsCloseError::Closed)
    }

    /// テスト専用: [`Self::reserve_close_for_test`] で確保した permit で
    /// Close 指示を確定する（[`Self::close`] の後半と同一の処理）。
    #[cfg(test)]
    pub(crate) fn commit_close_for_test(
        &self,
        permit: mpsc::Permit<'_, OutboundItem>,
        code: u16,
        reason: &str,
    ) -> Result<(), WsCloseError> {
        self.commit(
            permit,
            OutboundItem::Close {
                code,
                reason: reason.to_string(),
            },
            true,
        )
        .map_err(|()| WsCloseError::Closed)
    }
}

/// [`WsSender::send`] が返すエラー（セッションの終了処理（送信キューの封鎖）
/// 開始後、または [`WsSender::close`] 確定後の送信試行）。
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

/// [`WsSender::close`] が返すエラー（イシュー #710）。
///
/// `Display` はペイロード・内部状態を含まない固定文言とする（[`WsSendError`]
/// と同一の情報露出最小化方針、`.claude/rules/security.md`）。将来
/// variant を追加してもこれは非破壊変更として扱う（0.4.2 以降のバージョン
/// 方針）ため `#[non_exhaustive]` を付ける。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsCloseError {
    /// `code` が RFC 6455 7.4 節の意味で送信を許されない値だった
    /// （`<1000`・`1005`・`1006`・`1015`・予約域 `1016..=2999`・`>=5000`）。
    /// この検証は不正な Close フレームを実際には送出しないための入力検証
    /// であり（`.claude/rules/security.md`）、[`WsSender::close`] は
    /// close 済みフラグを立てずに拒否する。
    InvalidCode,
    /// `reason` のバイト長が制御フレームの payload 上限（125 バイト）から
    /// close code 分の 2 バイトを引いた 123 バイトを超えていた。
    /// [`InvalidCode`](Self::InvalidCode) と同様、close 済みフラグは
    /// 立たない。
    ReasonTooLong,
    /// 検証済みの `close` 呼び出しが、既に close 済み（2 回目以降の
    /// 呼び出し）、またはセッションの終了処理（送信キューの封鎖）が既に
    /// 始まっているために失敗した（フェイルクローズ、[`WsSendError`] の
    /// close 後の挙動と一貫させる）。
    Closed,
}

impl fmt::Display for WsCloseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::InvalidCode => "invalid websocket close code",
            Self::ReasonTooLong => "websocket close reason too long",
            Self::Closed => "websocket session already closed",
        };
        write!(f, "{msg}")
    }
}

impl StdError for WsCloseError {}

/// [`WsSender`] と、`crate::session::run_session` が受け取る側の
/// `mpsc::Receiver<OutboundItem>` のペアを構築する（`pub(crate)`、内部配線
/// 専用）。
///
/// 外部へは [`WsSender::send`]/[`WsSender::close`] のみを公開する非対称
/// API とし、受信側（`Receiver`）はクレート内部（`run_session`）にのみ渡す。
///
/// `capacity` は `1` に切り上げる（`mpsc::channel(0)` は panic するため、
/// `crates/core/src/streaming.rs` の `StreamingResponse::channel` と同一の
/// 防御）。
///
/// `crate::handle_upgrade` から呼ばれる（イシュー #671。101 応答送出成功後・
/// `on_open` 呼び出し直前に毎接続 1 回呼ばれる）。
pub(crate) fn channel(capacity: usize) -> (WsSender, mpsc::Receiver<OutboundItem>) {
    let (tx, rx) = mpsc::channel(capacity.max(1));
    let (closed_tx, closed_rx) = watch::channel(false);
    (
        WsSender {
            tx,
            closing: Arc::new(Mutex::new(false)),
            closed_signal: Arc::new(closed_tx),
            _closed_signal_anchor: closed_rx,
        },
        rx,
    )
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

    /// 既定 `on_close` が no-op（`ctx`・`reason` を無視するのみ）であることを
    /// 確認する（イシュー #729、受け入れ基準 4「既存ハンドラは無変更のまま
    /// コンパイル・動作する」の単体レベルの裏取り。実ハンドシェイク経由の
    /// 全終了経路の確認は `tests/on_close_e2e.rs`）。
    #[tokio::test]
    async fn default_on_close_is_noop() {
        let handler = UppercaseHandler;
        let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let ctx = WsConnContext::new(WsConnId::next(), sender, Vec::new());
        // no-op であることの確認は「panic しないこと」のみで、戻り値もない。
        handler.on_close(&ctx, CloseReason::ClientClose);
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
        assert_eq!(
            received,
            OutboundItem::Message(WsMessage::Text("hi".to_string()))
        );
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
        assert_eq!(
            received,
            OutboundItem::Message(WsMessage::Text("via-ctx".to_string()))
        );
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

    /// 受け入れ基準 2（イシュー #727）: 受信側 `Receiver` の生存中は
    /// `is_closed() == false` であり、`closed()` はまだ完了しない
    /// （短い `timeout` 内で `Err` を返す）。
    #[tokio::test]
    async fn sender_is_not_closed_while_receiver_alive() {
        let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);

        assert!(!sender.is_closed());
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(50), sender.closed()).await;
        assert!(
            result.is_err(),
            "closed() should not complete while the receiver is alive"
        );
    }

    /// 受け入れ基準 1・2（イシュー #727）: 受信側を drop すると
    /// `is_closed() == true` になり、`closed()` が有界時間内に完了する。
    #[tokio::test]
    async fn sender_closed_completes_after_receiver_drop() {
        let (sender, rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        drop(rx);

        assert!(sender.is_closed());
        tokio::time::timeout(std::time::Duration::from_secs(2), sender.closed())
            .await
            .expect("closed() should complete once the receiver is dropped");
    }

    /// 受け入れ基準 3（イシュー #727）: clone した `WsSender` でも、drop 前は
    /// `false`、drop 後は `true` になり、`closed()` が両方で完了する。
    #[tokio::test]
    async fn cloned_sender_observes_same_close() {
        let (sender, rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let cloned = sender.clone();

        assert!(!sender.is_closed());
        assert!(!cloned.is_closed());

        drop(rx);

        assert!(sender.is_closed());
        assert!(cloned.is_closed());
        tokio::time::timeout(std::time::Duration::from_secs(2), sender.closed())
            .await
            .expect("original sender's closed() should complete");
        tokio::time::timeout(std::time::Duration::from_secs(2), cloned.closed())
            .await
            .expect("cloned sender's closed() should complete");
    }

    /// 受け入れ基準 1（イシュー #727）: 先に `closed()` を待つタスクを
    /// spawn しておき、後から受信側を drop すると、その待機タスクが
    /// 有界時間内に起こされること（「後から終了」の順序を確認する）。
    #[tokio::test]
    async fn closed_wakes_pending_waiter_on_receiver_drop() {
        let (sender, rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let waiter_sender = sender.clone();
        let waiter = tokio::spawn(async move {
            waiter_sender.closed().await;
        });

        // waiter が `closed()` の poll に到達する猶予を与える（`yield_now`
        // で十分。実行順序を厳密に保証する必要はなく、drop 前に waiter が
        // pending であることを確認できれば良い）。
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        drop(rx);

        tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("waiter should be woken within timeout")
            .unwrap();
    }

    /// 受け入れ基準（イシュー #710）: RFC 6455 7.4 節で送信が許される
    /// close code は `Ok`、許されない code（`<1000`・`1005`・`1006`・
    /// `1015`・予約域・`>=5000`）は `InvalidCode` を返す。検証エラー後も
    /// close 済みフラグが立たないこと（`send` が引き続き成功すること）も
    /// 確認する。
    #[tokio::test]
    async fn close_validates_code_per_rfc6455() {
        for code in [1000u16, 1001, 1002, 1003, 1007, 1011, 1012, 3000, 4999] {
            let (sender, mut rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
            sender.close(code, "").await.unwrap_or_else(|err| {
                panic!("code {code} should be accepted, got {err}");
            });
            assert_eq!(
                rx.recv().await.unwrap(),
                OutboundItem::Close {
                    code,
                    reason: String::new(),
                }
            );
        }

        for code in [999u16, 1004, 1005, 1006, 1015, 1016, 2999, 5000] {
            let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
            assert_eq!(
                sender.close(code, "").await,
                Err(WsCloseError::InvalidCode),
                "code {code} should be rejected"
            );
            // 検証エラーは close 済みフラグを立てない: 直後の send が
            // 成功すること。
            assert!(!sender.is_closed());
            sender
                .send(WsMessage::Text("still open".to_string()))
                .await
                .expect("send should still succeed after a rejected close() call");
        }
    }

    /// 受け入れ基準（イシュー #710）: reason は 123 バイト（制御フレーム
    /// payload 上限 125 バイトから close code 2 バイトを引いた値）まで
    /// 許容し、超過は `ReasonTooLong`。マルチバイト文字の境界も確認する。
    #[tokio::test]
    async fn close_validates_reason_length() {
        let ascii_123 = "a".repeat(123);
        let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        sender.close(1000, &ascii_123).await.unwrap();

        let ascii_124 = "a".repeat(124);
        let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        assert_eq!(
            sender.close(1000, &ascii_124).await,
            Err(WsCloseError::ReasonTooLong)
        );

        // "あ" は UTF-8 で 3 バイト。41 個 = 123 バイトはちょうど境界で許容。
        let multibyte_123 = "あ".repeat(41);
        assert_eq!(multibyte_123.len(), 123);
        let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        sender.close(1000, &multibyte_123).await.unwrap();

        // 123 バイトの境界に ASCII 1 文字を足すと 124 バイトになり拒否される。
        let multibyte_124 = format!("{multibyte_123}a");
        assert_eq!(multibyte_124.len(), 124);
        let (sender, _rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        assert_eq!(
            sender.close(1000, &multibyte_124).await,
            Err(WsCloseError::ReasonTooLong)
        );
    }

    /// 受け入れ基準 1・2（イシュー #710）: close 後の `send` は
    /// `WsSendError`、2 回目の `close` は `WsCloseError::Closed` を返し、
    /// `is_closed()` は `true` になる。clone した別の `WsSender` からも
    /// 同じ結果になること。
    #[tokio::test]
    async fn close_then_send_and_second_close_are_rejected() {
        let (sender, mut rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        let cloned = sender.clone();

        sender.close(1000, "bye").await.unwrap();
        assert_eq!(
            rx.recv().await.unwrap(),
            OutboundItem::Close {
                code: 1000,
                reason: "bye".to_string(),
            }
        );

        assert!(sender.is_closed());
        assert!(cloned.is_closed());

        assert_eq!(
            sender.send(WsMessage::Text("late".to_string())).await,
            Err(WsSendError)
        );
        assert_eq!(cloned.close(1001, "again").await, Err(WsCloseError::Closed));
        assert_eq!(sender.close(1001, "again").await, Err(WsCloseError::Closed));
    }

    /// 受け入れ基準（イシュー #710）: セッション終了後（受信側 drop 済み）の
    /// `close` は `WsCloseError::Closed` を返す。
    #[tokio::test]
    async fn close_after_receiver_dropped_is_rejected() {
        let (sender, rx) = channel(DEFAULT_OUTBOUND_CAPACITY);
        drop(rx);
        assert_eq!(sender.close(1000, "bye").await, Err(WsCloseError::Closed));
    }

    /// 受け入れ基準（イシュー #710、順序保証）: 複数タスクが `send` を
    /// 連打する中で 1 回 `close` を呼んだとき、`Ok` を返した全メッセージが
    /// Close より前に並び、Close の後ろには何も積まれないこと。
    #[tokio::test]
    async fn concurrent_send_and_close_preserve_order() {
        const CAPACITY: usize = 2;
        const SENDERS: usize = 20;

        let (sender, mut rx) = channel(CAPACITY);

        let mut handles = Vec::with_capacity(SENDERS);
        for i in 0..SENDERS {
            let s = sender.clone();
            handles.push(tokio::spawn(async move {
                s.send(WsMessage::Text(format!("msg-{i}"))).await
            }));
        }
        // close する側も同じキューを競合させる。
        let closer = sender.clone();
        let close_handle = tokio::spawn(async move { closer.close(1000, "done").await });
        // テスト関数自身が持つ `sender` を明示的に drop する。生かしたままだと
        // 送信側（`mpsc::Sender`）のクローンが 1 個残り続け、下の `rx.recv()`
        // ループが `None` を観測できず無期限に `.await` してしまう
        // （全 clone が drop されて初めてチャネルが閉じる mpsc の契約）。
        drop(sender);

        // 受信側は最後まで読み切る（送信側が有界回数で解放されるよう、
        // 受信を並行して進める）。
        let mut items = Vec::new();
        while let Some(item) = rx.recv().await {
            items.push(item);
        }

        let send_results: Vec<Result<(), WsSendError>> = {
            let mut results = Vec::with_capacity(SENDERS);
            for h in handles {
                results.push(h.await.unwrap());
            }
            results
        };
        close_handle.await.unwrap().unwrap();

        let ok_count = send_results.iter().filter(|r| r.is_ok()).count();
        let close_pos = items
            .iter()
            .position(|item| matches!(item, OutboundItem::Close { .. }))
            .expect("close item should be present");

        // Close の後ろには何も積まれていない。
        assert_eq!(close_pos, items.len() - 1);
        // Close より前にある Message の件数と、Ok を返した send の件数が
        // 一致する（Ok を返したメッセージはすべて Close より前にある）。
        let messages_before_close = items[..close_pos]
            .iter()
            .filter(|item| matches!(item, OutboundItem::Message(_)))
            .count();
        assert_eq!(messages_before_close, ok_count);
    }

    /// 受け入れ基準（イシュー #710）: 容量を満杯にして `send` を待機させた
    /// 状態で `close` を enqueue すると、受信側が消費を進めた時点で待機中の
    /// `send` が有界時間内に解放される（無期限にブロックしない）。
    #[tokio::test]
    async fn close_releases_blocked_waiting_sender() {
        let (sender, mut rx) = channel(1);
        // 容量 1 を先に埋める。
        sender
            .send(WsMessage::Text("first".to_string()))
            .await
            .unwrap();

        let blocked_sender = sender.clone();
        let blocked = tokio::spawn(async move {
            blocked_sender
                .send(WsMessage::Text("blocked".to_string()))
                .await
        });
        tokio::task::yield_now().await;
        assert!(!blocked.is_finished());

        let closer = sender.clone();
        let close_handle = tokio::spawn(async move { closer.close(1000, "bye").await });

        // 受信側を消費して、待機中の send/close が有界時間内に進むことを
        // 確認する。
        let mut items = Vec::new();
        for _ in 0..3 {
            match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await {
                Ok(Some(item)) => items.push(item),
                Ok(None) => break,
                Err(_) => break,
            }
        }

        tokio::time::timeout(std::time::Duration::from_secs(2), close_handle)
            .await
            .expect("close() should complete within timeout")
            .unwrap()
            .unwrap();

        // 待機していた blocked send は、Close より先に permit を得られれば
        // Ok、後であれば Err になる。いずれにせよ有界時間内に解放される。
        tokio::time::timeout(std::time::Duration::from_secs(2), blocked)
            .await
            .expect("blocked send should be released within timeout")
            .unwrap()
            .ok();
    }

    /// PR #736 レビュー指摘対応（codex P1・Cursor Bugbot Medium）: `close` が
    /// 確定したあと、送信キューが満杯のまま**受信側が二度と消費しない**
    /// （実セッションで言えば、Close より手前の項目を書き出すソケット I/O が
    /// 停止したまま応答が返らない状況を模す）場合でも、`send` はキューの
    /// 実ドレインを待たず即座に `WsSendError` を返す。
    ///
    /// [`close_releases_blocked_waiting_sender`] は受信側が `rx.recv()` を
    /// 積極的に呼び続けることで解放されるケースを検証済みだが、それだけでは
    /// 「`close` 確定後、キューが満杯かつ二度と消費されない」という本質的な
    /// 危険シナリオを再現できない（受信側が消費を続ける限り、旧実装
    /// （`reserve().await` の完了後に `closing` を確認するだけの実装）でも
    /// 有界時間内に解放されてしまうため）。本テストは受信側の消費を
    /// `close` の Close 項目が積まれた直後で完全に止め、`closing` フラグの
    /// 確定を `reserve()` の実完了より先に検知できることを直接確認する。
    #[tokio::test]
    async fn send_after_close_on_permanently_full_queue_fails_fast() {
        const CAPACITY: usize = 3;

        let (sender, mut rx) = channel(CAPACITY);

        // 1 件送って即座に受信側が引き取る（実セッションで言えば、ソケット
        // I/O が停止する直前に消費済みの 1 件を模す）。以降 `rx` は一切
        // 消費しない（ソケット書き込みが永久に完了しない状況を模す）。
        sender
            .send(WsMessage::Text("in-flight".to_string()))
            .await
            .unwrap();
        rx.recv().await.unwrap();

        // 残り容量（CAPACITY 件）のうち 1 件分を空けたまま埋める。
        for i in 0..CAPACITY - 1 {
            sender
                .send(WsMessage::Text(format!("queued-{i}")))
                .await
                .unwrap();
        }

        // 空いている最後の 1 枠を `close` が取り、`closing` を確定させる。
        // 受信側は以降呼ばないため、Close 項目はキューに残り続ける。
        let closer = sender.clone();
        tokio::time::timeout(std::time::Duration::from_secs(2), closer.close(1000, "bye"))
            .await
            .expect("close should acquire the last free slot promptly")
            .unwrap();
        assert!(sender.is_closed());

        // ここでキューは満杯（CAPACITY/CAPACITY）かつ、テストが `rx` を
        // 二度と消費しないため永久に満杯のまま。旧実装は `reserve().await`
        // の完了を待つため無期限にブロックする。新実装は `closing` の
        // 確定を検知して即座に `Err` を返すはずなので、十分に短い時間内に
        // 完了することを要求する（`rx` を消費した場合に得られるはずの
        // 解放とは無関係に、`closing` の確定だけで解放されることの証明）。
        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            sender.send(WsMessage::Text("after-close".to_string())),
        )
        .await
        .expect(
            "send after close on a permanently-full, never-drained queue must not block \
             indefinitely (PR #736 review finding)",
        );
        assert_eq!(outcome, Err(WsSendError));
    }
}
