//! コアループ（接続受理・リクエストループ）と 3 拡張点の実接続（TASK-1.4-2 / #70）。
//!
//! [`extension`][crate::extension] モジュールが定義する `Middleware` /
//! `UpgradeHandler` / `RequestGate` は trait 定義のみであり、実際に接続を
//! 受理してこれらを呼び出すのは本モジュールの責務。[`Server`] がビルダーとして
//! 拡張点実装（`Box<dyn ...>`）を保持し、[`Server::bind`] で得た [`BoundServer`]
//! の [`BoundServer::run`] が accept ループを回して 1 接続ごとに
//! [`handle_connection`] を spawn する。同時接続数は
//! `DEFAULT_MAX_CONNECTIONS`（[`Server::max_connections`] で変更可）を
//! 上限とし、リソース枯渇 DoS を防ぐ（`.claude/rules/security.md`）。
//!
//! # コアループ本体は feature で分岐しない
//!
//! `handle_connection` 内に `#[cfg(feature = "...")]` を一切持たない
//! （`docs/spec/03-poc` PoC-3 の設計規約）。プラグインの介入余地は一定
//! シグネチャの 3 種のヘルパーに閉じる:
//! - `plugin::try_handle_upgrade`（非公開 `plugin` モジュール、TASK-4.1 / #22）:
//!   長時間接続（WebSocket 等）への委譲。`websocket` feature 有効時は
//!   `fandhe_backend_plugin_websocket` へ完全委譲し、無効時は常に `Some(stream)` を返す
//!   スタブ挙動を維持する
//! - `plugin::try_intercept`（非公開 `plugin` モジュール）: リクエスト/
//!   レスポンス完結型プラグイン（WebRTC シグナリングプロキシ等）へのパス
//!   インターセプト。TASK-2.1（#18）で確立した feature flag + `dep:` 構文の
//!   プラグイン境界パターンの実装（`docs/design/plugin-boundary.md`）
//! - `plugin::finalize_response`（非公開 `plugin` モジュール、イシュー #305）:
//!   レスポンス後処理型プラグイン（CORS 等）への委譲。`Middleware::on_response`
//!   がレスポンスへの参照を持たない観測専用契約のため使えない場合の受け皿
//!
//! いずれも feature 分岐はヘルパー内部に閉じ、`handle_connection` 側は
//! ヘルパーのシグネチャを変えずに済む。
//!
//! `try_handle_upgrade`（本モジュール内の非公開シーム）は TASK-4.1（#22）で
//! `crate::plugin` モジュールへ移設し、`websocket` feature 有効時は
//! `fandhe_backend_plugin_websocket` へ実委譲する実装に差し替えた（feature 無効時は
//! 従来どおり常に `Some(stream)` を返すスタブ挙動を維持し、
//! `handle_connection` 側は 501 応答を返す）。移設に伴いシグネチャを
//! `&[Box<dyn UpgradeHandler>]` から `Vec<u8>`（残余バイト列）+ `&Server`
//! （設定取得用）へ変更した。これは「シームのシグネチャを変えない」という
//! 本モジュールの設計規約からの意図的な逸脱であり、複数 Upgrade 型プラグイン
//! が設定を必要とする将来を見据え、`&Server` 経由で任意の cfg-gated 設定へ
//! アクセスできるようにするため（`crate::plugin::try_handle_upgrade` の doc・
//! `docs/design/plugin-boundary.md` 5 節を参照）。
//!
//! # 1 接続あたりの処理フロー
//!
//! ```text
//! loop {
//!   read_request（fandhe_backend_http::connection、ヘッド + body 読了、タイムアウト付き）
//!     Ok(None)          → 正常クローズ
//!     Err(e)            → e に応じた 4xx/5xx（またはエラー応答なし）を返しクローズ
//!     Ok(Some(req)) →
//!       1. Middleware::on_request（登録順）
//!       2. RequestGate::check（登録順、最初の Reject を優先。フェイルクローズ）
//!       3. UpgradeHandler::matches（登録順。マッチしたら読み取りバッファを
//!          明示解放してから try_handle_upgrade へ委譲）
//!       3.5. Interceptor::intercept（ユーザー向けインターセプト拡張点、
//!          イシュー #420。登録順、最初の Some(response) なら以降の
//!          plugin::try_intercept・Handler::handle をスキップ）
//!       4. plugin::try_intercept（パスインターセプト型プラグイン。
//!          3.5 で確定済みならスキップ。Some(response) なら以降の
//!          Handler::handle をスキップ）
//!       5. Handler::handle（未登録時、または 3.5/4 が None の場合。
//!          未登録時は 404）
//!       5.4. Interceptor::map_response（ユーザー向けレスポンス改変拡張点、
//!          イシュー #420。登録順に逐次適用。3.5/4/5 いずれの応答にも適用）
//!       5.5. plugin::finalize_response（レスポンス後処理型プラグイン。
//!          5.4 適用後の応答に適用）
//!       6. レスポンス書き込み → Middleware::on_response
//!       7. should_keep_alive(head) が false なら接続を閉じる
//! }
//! ```
//!
//! `RequestGate` を `UpgradeHandler` より先に評価するのは、将来の hub
//! TenantGate（TASK-9.1）が WebSocket アップグレードも既定拒否でゲート
//! できるようにするため（フェイルクローズ、`docs/spec/04-requirements.md` REQ-9）。
//! 同じ理由で `plugin::try_intercept` も `RequestGate` より後（`UpgradeHandler`
//! の後）に評価し、ゲートの既定拒否がパスインターセプト型プラグインにも及ぶ
//! ようにする。`Interceptor::intercept`（イシュー #420）も同じ理由で
//! `RequestGate`/`UpgradeHandler` より後に評価し、ユーザーコードがゲートの
//! 既定拒否を迂回できないようにする（詳細な設計判断は [`crate::interceptor`]
//! モジュール doc を参照）。

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::task::JoinSet;

use fandhe_backend_http::body::BodyError;
use fandhe_backend_http::buffer::RecvBuffer;
use fandhe_backend_http::chunked::{ChunkedError, encode_chunk, encode_terminator};
use fandhe_backend_http::connection::{RequestError, read_request_with_limit, should_keep_alive};
use fandhe_backend_http::request::{HttpVersion, ParseError, RequestHead};
use fandhe_backend_http::response::Response;

use crate::extension::{GateContext, GateOutcome, Middleware, RequestGate, UpgradeHandler};
use crate::interceptor::Interceptor;
use crate::streaming::{RecvOutcome, StreamingResponse};

/// `read_request` 1 回あたりの読み取りタイムアウトの既定値（スロークライアント対策）。
///
/// ヘッド・body の読み取り待ち、および keep-alive 接続が次のリクエストを
/// 送ってくるまでのアイドル待ちの両方に同じ値を適用する。値のチューニングは
/// [`Server::read_timeout`] で行う（`.claude/rules/security.md` のリソース
/// 枯渇対策）。
///
/// `read_timeout` は「1 回の read 待ち」しか制限しないため、正当なタイムアウト
/// 間隔より短い間隔で送信し続けるクライアントに対しては単体で無力である。
/// この隙間は [`Server::max_connection_lifetime`]（接続の総生存期間上限）と
/// [`Server::max_requests_per_connection`]（keep-alive 中の最大リクエスト数）
/// で埋める（#70 レビュー指摘、`.claude/rules/security.md` のリソース枯渇観点）。
///
/// [`handle_connection`] は実際の read 待ちにこの値をそのまま使わず、
/// 残り生存期間（`max_connection_lifetime - 経過時間`）とのうち短い方を使う
/// （#70 Bugbot 指摘）。これにより「生存期間チェックの直後に最大
/// `read_timeout` だけ read がブロックし、その間 permit を握ったまま
/// 総生存期間を超過する」経路を塞ぐ。
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// レスポンス側 chunked ストリーミング送信（[`Handler::handle_streaming`]、
/// イシュー #319）の各書き込み待ちの既定タイムアウト。
///
/// `DEFAULT_READ_TIMEOUT` と同じスロークライアント対策の考え方を書き出し側にも
/// 適用する。producer からの次チャンク待ち（`StreamingResponse::recv`）・
/// ソケットへの実書き込み（`write_all`）双方に、`DEFAULT_READ_TIMEOUT` と
/// 同じ「残り生存期間との短い方」丸めパターン（`write_streaming_response` を
/// 参照）を適用し、スローリーダによる接続・semaphore permit の無期限占有を
/// 防ぐ（`.claude/rules/security.md` のリソース枯渇観点）。既存の非ストリーミング
/// 応答（`Response::serialize`）の一括 `write_all` にはタイムアウトを適用
/// しない（イシュー #319 の計画時点でのスコープ外。
/// `.claude/rules/out-of-scope-tracking.md` 対象候補）。
///
/// # producer 側の制約（チャンク間隔 30 秒以内）
///
/// この値は「ソケットへの実書き込み待ち」だけでなく「producer からの次
/// チャンク待ち（`streaming.recv()`）」にも同じ丸めパターンで適用される。
/// そのため `BodyWriter::send` / `finish` の呼び出し間隔が本値を超えて
/// 空くと、正常に稼働している producer でも接続が強制クローズされる
/// （SSE のハートビート間隔や long-poll のようにアイドル区間が長い実装は
/// 本値を超えないよう注意する）。ワイヤへ余計なバイトを出さずに待ち時間を
/// リセットしたい場合は、`BodyWriter::send(Vec::new())`（空チャンクは
/// `encode_chunk` の契約により無出力）を本値未満の間隔で呼び、内部的な
/// キープアライブとして使う（`Handler::handle_streaming` の doc を参照）。
/// `Server::write_timeout` のような値の調整 API は本イシューのスコープ外。
const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// 1 接続あたりの総生存期間の既定上限（リソース枯渇 DoS 対策）。
///
/// `read_timeout`（[`Server::read_timeout`]）は「1 回の read 待ち」しか
/// 制限しないため、これより短い間隔で（例えば 1 バイトずつ）送信し続ける
/// クライアントは、本上限がなければ
/// `DEFAULT_MAX_CONNECTIONS` の permit を無期限に占有できてしまう。
/// [`handle_connection`] はコネクション開始時刻からの経過時間がこの値に達した
/// 時点で（読み取り待ちに入る前に）接続を閉じ、permit を解放する。
/// 値のチューニングは [`Server::max_connection_lifetime`] で行う。
const DEFAULT_MAX_CONNECTION_LIFETIME: Duration = Duration::from_secs(300);

/// keep-alive 接続 1 本あたりに処理を許すリクエスト数の既定上限（リソース枯渇 DoS 対策）。
///
/// `DEFAULT_MAX_CONNECTION_LIFETIME` とは独立に、短時間に大量の軽量リクエストを
/// 送り続けて 1 接続でハンドラ処理を占有し続ける経路を塞ぐ。上限に達した
/// リクエストへの応答は `Connection: close` を伴い、以後 [`handle_connection`] は
/// 同じ接続で次のリクエストを待たない。値のチューニングは
/// [`Server::max_requests_per_connection`] で行う。
const DEFAULT_MAX_REQUESTS_PER_CONNECTION: usize = 1_000;

/// 同時接続数の既定上限（リソース枯渇 DoS 対策）。
///
/// accept ループが際限なく `tokio::spawn` すると、`read_timeout`
/// （[`Server::read_timeout`]）による 1 接続あたりのスロークライアント対策が
/// あっても、大量の同時接続による
/// fd・メモリ消費（リソース枯渇 DoS）は防げない（`.claude/rules/security.md`）。
/// [`BoundServer::run`] はこの上限を `tokio::sync::Semaphore` で強制し、
/// 上限に達している間は新規 `accept` 自体を保留する（カーネルの listen
/// backlog に滞留させ、あふれた分は OS 側で拒否させるフェイルクローズ設計）。
/// 値のチューニングは [`Server::max_connections`] で行う。
const DEFAULT_MAX_CONNECTIONS: usize = 10_000;

/// `listener.accept()` がエラーを返した際、次の accept 試行までの待機時間。
///
/// EMFILE/ENFILE（fd 枯渇）のように accept エラーが連続しうる状況で、
/// 待機なしにループし続けると CPU を専有するビジーループになる
/// （`.claude/rules/security.md` のリソース枯渇観点）。[`BoundServer::run`]
/// の doc を参照。
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(10);

/// graceful shutdown（[`BoundServer::run_until`]）の in-flight リクエスト完了待ち
/// 上限の既定値（イシュー #313）。
///
/// シャットダウンシグナル受信後、accept を止めた上でこの時間だけ処理中の
/// 接続の完了を待つ。上限を超えても完了しない接続は強制クローズする
/// （ハング防止のフェイルクローズ、`.claude/rules/security.md` の
/// リソース枯渇・可用性観点）。値のチューニングは
/// [`Server::shutdown_grace_period`] で行う。
const DEFAULT_SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(30);

/// rebind（イシュー #485）時に旧 listener の accept backlog から drain する
/// 接続件数の上限（イシュー #501）。
///
/// `rebind` は listener を差し替える直前に旧 listener を drop するため、
/// kernel の accept backlog に滞留していた「3-way handshake 完了済みだが
/// 未 `accept()`」の接続はサーブされず RST を受け取っていた
/// （`docs/design/rebind.md` 6 節）。この上限は
/// [`drain_listener_backlog`] が回収する件数を有界にし、攻撃者が旧
/// アドレスへ接続を送り続けても rebind の完了通知を遅延させない
/// （リソース枯渇 DoS 対策、`.claude/rules/security.md`）。
const REBIND_BACKLOG_DRAIN_LIMIT: usize = 1024;

/// リクエストに対する最終応答を生成する、コアが公開する既定ハンドラ拡張点。
///
/// 3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）とは異なり
/// 「拡張点は 3 種に集約」の対象ではなく、ルーティング結果を最終応答へ
/// 変換する既定レスポンダの差し込み口という位置づけ。`fandhe_backend_routes::Router`
/// （TASK-1.5 / #14、下記 `impl Handler for fandhe_backend_routes::Router` 参照）を
/// 直接登録できるほか、トイハンドラ・テスト用の固定レスポンダ等の任意実装も
/// 引き続き受け付ける。
///
/// イシュー #315（`docs/design/async-handler.md` 採用案 (c)）で async 契約へ
/// 移行した。3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）は
/// 意図的に同期のまま据え置き、本トレイトのみ async 化する非対称設計である点に
/// 注意（`extension.rs` モジュール doc の対比記載も参照）。戻り値は
/// `fandhe_backend_routes::HandlerFuture`（`Pin<Box<dyn Future<Output = Response> +
/// Send>>`、`'static` 契約）で、`async-trait` 等の外部依存を追加せず std のみで
/// 型消去する。実装者はハンドラ本体で `sqlx` 等の非同期 I/O を直接 `.await` できる。
pub trait Handler: Send + Sync {
    /// リクエストヘッドと body からレスポンスを組み立てる future を返す。
    ///
    /// 呼び出し元（[`handle_connection`]）が `.await` する。ハンドラ内 panic は
    /// 接続単位で spawn されたタスク内に閉じ込められ、他コネクションの処理を
    /// 妨げない（`docs/design/async-handler.md` 7 節、`crates/core/tests/
    /// async_handler.rs` で実証）。
    fn handle(&self, head: &RequestHead, body: &[u8]) -> fandhe_backend_routes::HandlerFuture;

    /// レスポンス側 chunked ストリーミング送信（イシュー #319）の opt-in 拡張点。
    ///
    /// [`Self::handle`]（イシュー #315 で async 化）とは非対称に、本メソッドは
    /// 同期のまま据え置く。ストリーミング応答自体は
    /// [`crate::streaming::StreamingResponse::channel`] が返す producer 側
    /// タスク（`tokio::spawn`）が非同期 I/O を担うため、拡張点自体を async に
    /// する必要がない（呼び出し元がチャンネルを組み立てるだけで即座に返る）。
    ///
    /// 既定実装は常に `None` を返し、`handle_connection_with_permit` は
    /// 通常どおり [`Self::handle`] の一括応答（`Content-Length`）経路を使う。
    /// 既存の `Handler` 実装はこのメソッドを override しなくてもコンパイル
    /// が通り、挙動も一切変わらない（後方互換、`.claude/rules/
    /// feature-modification.md` の受け入れ基準 2）。
    ///
    /// `Some(streaming)` を返す場合、呼び出し元（コアの書き出しループ）は
    /// [`crate::streaming::StreamingResponse`] を chunked framing で逐次
    /// 送信する。典型的な実装パターンは
    /// [`crate::streaming::StreamingResponse::channel`] で得た
    /// [`crate::streaming::BodyWriter`] を `tokio::spawn` した producer
    /// タスクへ move し、producer がデータ生成の都合に合わせて `send` /
    /// `finish` を呼ぶことである:
    ///
    /// # チャンク間隔の制約（30 秒以内）
    ///
    /// producer からの次チャンク待ちには `DEFAULT_WRITE_TIMEOUT`（30 秒）が
    /// 適用され、超過すると正常に稼働している producer でも接続が強制
    /// クローズされる（スロークライアント・スロープロデューサ対策、
    /// `.claude/rules/security.md` のリソース枯渇観点）。SSE
    /// （`text/event-stream`）のハートビート間隔や long-poll のようにイベント
    /// 発生がまばらな producer を実装する場合は、本値未満の間隔で
    /// `BodyWriter::send(Vec::new())` を呼んで待ち時間をリセットするとよい
    /// （空チャンクは `encode_chunk` の契約によりワイヤへは無出力のため、
    /// クライアントに余計なバイトを見せずに内部キープアライブとして使える）。
    ///
    /// ```
    /// use fandhe_backend_core::server::Handler;
    /// use fandhe_backend_core::streaming::StreamingResponse;
    /// use fandhe_backend_http::request::RequestHead;
    /// use fandhe_backend_http::response::Response;
    ///
    /// struct StreamingHandler;
    ///
    /// impl Handler for StreamingHandler {
    ///     fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
    ///         Box::pin(async { Response::empty(404) })
    ///     }
    ///
    ///     fn handle_streaming(
    ///         &self,
    ///         _head: &RequestHead,
    ///         _body: &[u8],
    ///     ) -> Option<StreamingResponse> {
    ///         let (response, writer) = StreamingResponse::channel(200, Some("text/plain"), 4);
    ///         tokio::spawn(async move {
    ///             writer.send(b"hello ".to_vec()).await.ok();
    ///             writer.send(b"world".to_vec()).await.ok();
    ///             writer.finish().await.ok();
    ///         });
    ///         Some(response)
    ///     }
    /// }
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// use fandhe_backend_http::request::ParseOutcome;
    ///
    /// let handler = StreamingHandler;
    /// let ParseOutcome::Complete { head, .. } =
    ///     fandhe_backend_http::request::parse_request_head(b"GET / HTTP/1.1\r\n\r\n").unwrap()
    /// else {
    ///     panic!("expected complete head");
    /// };
    /// assert!(handler.handle_streaming(&head, b"").is_some());
    /// # }
    /// ```
    fn handle_streaming(
        &self,
        _head: &RequestHead,
        _body: &[u8],
    ) -> Option<crate::streaming::StreamingResponse> {
        None
    }
}

/// `fandhe_backend_routes::Router`（依存方向 `server → routes → http::*` の実体化、
/// TASK-1.5 / #14）をそのままコアの既定ハンドラとして登録できるようにする。
///
/// [`Router::dispatch`][fandhe_backend_routes::Router::dispatch] へ委譲するだけの薄い
/// アダプタであり、ルーティングの意味論（method + target 完全一致を最優先し、
/// miss 時のみ `{name}` パスパラメータ（TASK-176、#176）を登録順で照合・
/// 404/405 のフェイルクローズ）は `crates/routes` 側の責務のまま変わらない。
/// `Router::dispatch` 自体は同期関数で `HandlerFuture` を返す設計のため
/// （ルーティング解決は同期・ハンドラ本体実行のみ非同期、イシュー #315）、
/// このアダプタも素通しでよい。
impl Handler for fandhe_backend_routes::Router {
    fn handle(&self, head: &RequestHead, body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        self.dispatch(head, body)
    }
}

/// `Server::openapi()` / `Server::openapi_with()` が設定する、`GET
/// /openapi.json` / `GET /openapi.yaml` の配信登録状態（`openapi` feature
/// 限定、TASK-2.1 / #256、イシュー #320）。
///
/// `crate::plugin::try_intercept` がこの enum を参照して応答内容を判定
/// する。`Server::openapi()` と `Server::openapi_with()` は排他ではなく
/// **後勝ち**（どちらを先に呼んでも、最後に呼んだ方の variant が残る。
/// builder メソッドが `self` を消費して返す一般的な直感に一致する）。
#[cfg(feature = "openapi")]
pub(crate) enum OpenApiRegistration {
    /// 未登録（既定、fail-closed）。feature が有効でも常にフォールスルー
    /// する（`Server::openapi` の doc・A01/A05 観点を参照）。
    Disabled,
    /// `Server::openapi()` で登録した、フレームワーク固定スキーマ
    /// （`fandhe_backend_plugin_openapi::OPENAPI_JSON` / `OPENAPI_YAML`）。
    Embedded,
    /// `Server::openapi_with(doc)` で登録した、利用者アプリ独自のスキーマ
    /// （イシュー #320）。
    Custom(fandhe_backend_plugin_openapi::OpenApiDoc),
}

/// 3 拡張点・既定ハンドラを登録するビルダー。
///
/// 各登録メソッドは `self` を消費して返すため、メソッドチェーンで組み立てる。
/// [`Server::bind`] を呼ぶと以降は不変（`Arc<Server>`）として複数コネクション
/// タスクから共有参照される。
///
/// ```
/// use fandhe_backend_core::server::Server;
///
/// let server = Server::new();
/// // bind() はソケットを開くため doctest では実行しない（`no_run` 相当）。
/// // 実際の起動例は crates/core/examples/minimal.rs を参照。
/// let _ = server;
/// ```
pub struct Server {
    middlewares: Vec<Box<dyn Middleware>>,
    gates: Vec<Box<dyn RequestGate>>,
    upgrade_handlers: Vec<Box<dyn UpgradeHandler>>,
    /// ユーザー向けインターセプト・レスポンス改変拡張点（イシュー #420）。
    /// `handle_connection_with_permit` が `RequestGate`/`UpgradeHandler` の後・
    /// `plugin::try_intercept` の前に `intercept` を、最終応答確定後・
    /// `plugin::finalize_response` の前に `map_response` を評価する（登録順、
    /// `crate::interceptor` モジュール doc の評価順序を参照）。feature ゲート
    /// 不要（外部依存ゼロの純コア機能、`.claude/rules/pay-for-what-you-use.md`）。
    interceptors: Vec<Box<dyn Interceptor>>,
    handler: Option<Box<dyn Handler>>,
    max_connections: usize,
    max_connection_lifetime: Duration,
    max_requests_per_connection: usize,
    /// body として許容する最大バイト数（既定
    /// `fandhe_backend_http::body::MAX_BODY_BYTES`、イシュー #311）。
    /// `handle_connection_with_permit` がこの値を `read_request_with_limit`
    /// （`fandhe_backend_http::connection`）へ渡し、固定長・chunked 両経路の
    /// 413 判定に使う。
    max_body_bytes: u64,
    read_timeout: Duration,
    keep_alive_enabled: bool,
    /// graceful shutdown（[`BoundServer::run_until`]）の in-flight 完了待ち
    /// 上限（イシュー #313）。既定は `DEFAULT_SHUTDOWN_GRACE_PERIOD`。
    shutdown_grace_period: Duration,
    /// `webrtc-proxy` feature（TASK-2.1 / #18）有効時のみ意味を持つ設定。
    /// `crate::plugin::try_intercept` がこのフィールドを参照して `POST
    /// /rtc/offer` を上流へ中継するかどうかを判定する。feature 無効時は
    /// フィールド自体が構造体から消え、依存・コードともゼロコストになる
    /// （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。
    #[cfg(feature = "webrtc-proxy")]
    webrtc_proxy_config: Option<fandhe_backend_plugin_webrtc_proxy::ProxyConfig>,
    /// `webrtc` feature（TASK-8.1 / #26）有効時のみ意味を持つ設定。
    /// `crate::plugin::try_intercept` がこのフィールドを参照して `POST /rtc/offer`
    /// を in-process の `RTCPeerConnection` シグナリングへ委譲するかどうかを判定
    /// する。feature 無効時はフィールド自体が構造体から消え、依存・コードとも
    /// ゼロコストになる（pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。
    #[cfg(feature = "webrtc")]
    webrtc_config: Option<fandhe_backend_plugin_webrtc::WebRtcConfig>,
    /// `websocket` feature（TASK-4.1 / #22）有効時のみ意味を持つ設定群。
    /// `crate::plugin::try_handle_upgrade` がこのフィールドを参照して
    /// `UpgradeHandler` 委譲成立後に `fandhe_backend_plugin_websocket::handle_upgrade` へ
    /// 渡す。`Server::websocket` を複数回呼ぶと複数パスを登録でき、
    /// 登録順に `fandhe_backend_plugin_websocket::matches` を評価して最初に一致した
    /// 設定を使う（`upgrade_handlers` 側の `WebSocketUpgradeAdapter` も
    /// 同じ登録順で `matches` するため、両者は常に整合する）。単一
    /// `Option` だと 2 回目の呼び出しで 1 回目の設定が上書きされ、最初に
    /// 登録したパスへのアップグレードが 501 になる不整合が生じるため
    /// `Vec` として保持する。feature 無効時はフィールド自体が構造体から
    /// 消え、依存・コードともゼロコストになる（pay-for-what-you-use、
    /// .claude/rules/pay-for-what-you-use.md）。
    #[cfg(feature = "websocket")]
    websocket_configs: Vec<fandhe_backend_plugin_websocket::WebSocketConfig>,
    /// `graphql` feature（TASK-5.1 / #38）有効時のみ意味を持つ、登録済み
    /// GraphQL スキーマ設定。`crate::plugin::try_intercept` がこのフィールド
    /// を参照して `POST /graphql` を実行するかどうかを判定する。`None`
    /// （未登録、既定）の場合は feature が有効でもフォールスルーする
    /// （`webrtc-proxy`・`webrtc` と同じ「設定登録型」パターン、
    /// `crates/plugin-graphql` の crate doc を参照）。feature 無効時は
    /// フィールド自体が構造体から消え、依存・コードともゼロコストになる
    /// （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。
    #[cfg(feature = "graphql")]
    graphql_config: Option<fandhe_backend_plugin_graphql::GraphQlConfig>,
    /// `openapi` feature（TASK-2.1 / #256）有効時のみ意味を持つ、`GET
    /// /openapi.json` / `GET /openapi.yaml` の配信登録状態
    /// （[`OpenApiRegistration`]）。`crate::plugin::try_intercept` がこの
    /// フィールドを参照して応答内容を判定する。既定 `Disabled`（未登録）
    /// では feature が有効でもフォールスルーする（`webrtc-proxy`・
    /// `graphql` と同じ「設定登録型」パターン。API 構造の開示を利用者の
    /// 明示的 opt-in に限定する意図、`.claude/rules/security.md` の
    /// A01/A05 観点）。feature 無効時はフィールド自体が構造体から消え、
    /// 依存・コードともゼロコストになる（pay-for-what-you-use）。
    #[cfg(feature = "openapi")]
    openapi_registration: OpenApiRegistration,
    /// `cors` feature（イシュー #305）有効時のみ意味を持つ、登録済み CORS
    /// 設定。`crate::plugin::finalize_response`（レスポンス後処理型シーム）が
    /// このフィールドを参照して実リクエスト応答へ CORS ヘッダを付与する
    /// かどうかを判定する。`None`（未登録、既定）の場合は feature が有効でも
    /// レスポンスを一切変更しない（`graphql`・`openapi` と同じ「設定登録型」
    /// パターン）。プリフライト応答は本フィールドを介さず、利用者が
    /// `fandhe_backend_plugin_cors::preflight_response` を直接
    /// `Router::options_fallback` へ配線する（`crates/plugin-cors/src/lib.rs`
    /// の doc を参照）。feature 無効時はフィールド自体が構造体から消え、
    /// 依存・コードともゼロコストになる（pay-for-what-you-use）。
    #[cfg(feature = "cors")]
    cors_config: Option<fandhe_backend_plugin_cors::CorsConfig>,
    /// `compression` feature（イシュー #321）有効時のみ意味を持つ、登録済み
    /// レスポンス圧縮設定。`crate::plugin::finalize_response`（レスポンス
    /// 後処理型シーム）がこのフィールドを参照し、`Some` の場合のみ
    /// `fandhe_backend_plugin_compression::apply_compression` を呼んで
    /// 実リクエスト応答へ gzip 圧縮を適用するかどうかを判定する。
    /// `None`（未登録、既定）の場合は feature が有効でもレスポンスを一切
    /// 変更しない（`cors`・`graphql`・`openapi` と同じ「設定登録型」
    /// パターン）。feature 無効時はフィールド自体が構造体から消え、
    /// 依存・コードともゼロコストになる（pay-for-what-you-use）。
    #[cfg(feature = "compression")]
    compression_config: Option<fandhe_backend_plugin_compression::CompressionConfig>,
    /// `static` feature（イシュー #318）有効時のみ意味を持つ、登録済み静的
    /// ファイル配信設定。`crate::plugin::try_intercept` がこのフィールドを
    /// 参照して `GET` リクエストを配信するかどうかを判定する。`None`
    /// （未登録、既定）の場合は feature が有効でもフォールスルーする
    /// （`graphql`・`openapi`・`cors` と同じ「設定登録型」パターン）。
    /// feature 無効時はフィールド自体が構造体から消え、依存・コードとも
    /// ゼロコストになる（pay-for-what-you-use）。
    #[cfg(feature = "static")]
    static_files_config: Option<fandhe_backend_plugin_static::StaticFilesConfig>,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            middlewares: Vec::new(),
            gates: Vec::new(),
            upgrade_handlers: Vec::new(),
            interceptors: Vec::new(),
            handler: None,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_connection_lifetime: DEFAULT_MAX_CONNECTION_LIFETIME,
            max_requests_per_connection: DEFAULT_MAX_REQUESTS_PER_CONNECTION,
            max_body_bytes: fandhe_backend_http::body::MAX_BODY_BYTES,
            read_timeout: DEFAULT_READ_TIMEOUT,
            keep_alive_enabled: true,
            shutdown_grace_period: DEFAULT_SHUTDOWN_GRACE_PERIOD,
            #[cfg(feature = "webrtc-proxy")]
            webrtc_proxy_config: None,
            #[cfg(feature = "webrtc")]
            webrtc_config: None,
            #[cfg(feature = "websocket")]
            websocket_configs: Vec::new(),
            #[cfg(feature = "graphql")]
            graphql_config: None,
            #[cfg(feature = "openapi")]
            openapi_registration: OpenApiRegistration::Disabled,
            #[cfg(feature = "cors")]
            cors_config: None,
            #[cfg(feature = "compression")]
            compression_config: None,
            #[cfg(feature = "static")]
            static_files_config: None,
        }
    }
}

impl Server {
    /// 拡張点・ハンドラを 1 件も持たない空の [`Server`] を作る。
    /// 同時接続数上限は `DEFAULT_MAX_CONNECTIONS`。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 同時接続数の上限を設定する（既定 `DEFAULT_MAX_CONNECTIONS`）。
    ///
    /// [`BoundServer::run`] の accept ループはこの上限に達している間、
    /// 新規接続の受理を保留する（リソース枯渇 DoS 対策、本モジュール冒頭の
    /// doc・`DEFAULT_MAX_CONNECTIONS` の doc を参照）。`0` を指定した場合は
    /// accept ループが永久に許可待ちでブロックし新規接続を一切受理できなく
    /// なるため、[`Server::bind`] 側で最低 `1` に切り上げる。
    #[must_use]
    pub fn max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// 1 接続あたりの総生存期間の上限を設定する（既定 `DEFAULT_MAX_CONNECTION_LIFETIME`）。
    ///
    /// [`handle_connection`] はコネクション開始からの経過時間がこの値に達すると、
    /// 次のリクエストの読み取り待ちに入る前に接続を閉じる（`DEFAULT_READ_TIMEOUT`
    /// の doc・`.claude/rules/security.md` のリソース枯渇観点を参照）。`Duration::ZERO`
    /// を指定すると最初のリクエストを読む前に接続を閉じてしまうため、実運用では
    /// 避けること。
    #[must_use]
    pub fn max_connection_lifetime(mut self, max: Duration) -> Self {
        self.max_connection_lifetime = max;
        self
    }

    /// keep-alive 接続 1 本あたりに処理を許すリクエスト数の上限を設定する
    /// （既定 `DEFAULT_MAX_REQUESTS_PER_CONNECTION`）。
    ///
    /// 上限に達したリクエストへの応答は `Connection: close` を伴い、以後
    /// [`handle_connection`] は同じ接続で次のリクエストを待たない
    /// （`DEFAULT_READ_TIMEOUT` の doc・`.claude/rules/security.md` の
    /// リソース枯渇観点を参照）。`0` を指定した場合でも最低 1 リクエストは処理してから閉じる
    /// （[`handle_connection`] 側で `.max(1)` に切り上げる）。
    #[must_use]
    pub fn max_requests_per_connection(mut self, max: usize) -> Self {
        self.max_requests_per_connection = max;
        self
    }

    /// body として許容する最大バイト数の上限を設定する
    /// （既定 `fandhe_backend_http::body::MAX_BODY_BYTES` = 1 MiB、イシュー #311）。
    ///
    /// [`handle_connection`] はこの値を `read_request_with_limit`
    /// （`fandhe_backend_http::connection`）へ渡し、`Content-Length` 固定長
    /// body・chunked transfer-coding の復号後総量の双方に適用する。上限を
    /// 超えたリクエストは axum の `RequestBodyLimitLayer` 相当として
    /// `413 Payload Too Large` を返す（body なしで拒否、内部の上限値は
    /// レスポンスへ含めない）。
    ///
    /// - `0` を指定すると body を持つリクエストを一律 413 で拒否する
    ///   （`Content-Length: 0` またはヘッダ不在の body なしリクエストは
    ///   引き続き正常応答する）。より厳しい側への設定でありフェイルクローズ
    ///   方向のため許容する
    /// - 既定より大きい値を設定すると、1 接続あたりの body バッファリング
    ///   最大メモリが増える。最悪ケースの概算は `max_body_bytes ×
    ///   max_connections`（[`Server::max_connections`]）に比例するため、
    ///   大値設定はリソース枯渇（DoS）耐性の後退になりうることを踏まえて
    ///   利用者が判断すること（`.claude/rules/security.md`）
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_core::Server;
    ///
    /// let server = Server::new().max_body_bytes(64 * 1024); // 64 KiB
    /// let _ = server;
    /// ```
    #[must_use]
    pub fn max_body_bytes(mut self, max: usize) -> Self {
        self.max_body_bytes = max as u64;
        self
    }

    /// `read_request` 1 回あたりの読み取りタイムアウトを設定する
    /// （既定 `DEFAULT_READ_TIMEOUT` = 30 秒）。
    ///
    /// ヘッド・body の読み取り待ち、および keep-alive 接続が次のリクエストを
    /// 送ってくるまでのアイドル待ちの両方に適用される（`DEFAULT_READ_TIMEOUT`
    /// の doc を参照）。
    ///
    /// - `Duration::ZERO` を指定すると、まだ届いていないリクエストの読み取り
    ///   待ちには即座にタイムアウトし、応答を送らず接続を閉じる（フェイルクローズ。
    ///   [`Server::max_connection_lifetime`] の `ZERO` 時の挙動と同じ「閉じる側に
    ///   倒れる」設計であり、実運用では避けること）。`tokio::time::timeout` は
    ///   内部の読み取りを先にポーリングするため、読み取り時点で既にデータが
    ///   到着済み（パイプライン済みリクエスト等）の場合はタイムアウトより先に
    ///   読み取りが完了しうる点に注意（`0` はあくまで「待たない」設定であり、
    ///   到着済みデータの処理自体を禁止するものではない）。
    /// - 実効タイムアウトは常に [`Server::max_connection_lifetime`] の残り生存
    ///   期間との短い方に丸められる（`DEFAULT_READ_TIMEOUT` の doc・#70 Bugbot
    ///   指摘を参照）。極端に大きい値を指定しても、接続の総占有時間は
    ///   `max_connection_lifetime` の上限を超えない。
    ///
    /// ```
    /// use std::time::Duration;
    /// use fandhe_backend_core::server::Server;
    ///
    /// let server = Server::new().read_timeout(Duration::from_secs(5));
    /// let _ = server;
    /// ```
    #[must_use]
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// keep-alive の有効/無効を設定する（既定 `true` = 有効）。
    ///
    /// `false` を指定すると、[`handle_connection`] は `should_keep_alive` の
    /// 判定結果によらず常に `Connection: close` を付けて応答し、1 接続で
    /// 1 リクエストのみ処理して閉じる（`RequestGate` 拒否応答・通常応答の
    /// 両経路に適用される）。エラー応答は本設定と無関係に常時クローズする
    /// （既存のフェイルセーフ挙動のまま）。
    ///
    /// 本設定は HTTP リクエスト/応答の keep-alive ループのみに適用され、
    /// `UpgradeHandler` への委譲（WebSocket 等）成立後のセッション寿命は
    /// スコープ外（Upgrade はこの keep-alive ループから離脱する既存契約の
    /// まま変わらない）。
    ///
    /// ```
    /// use fandhe_backend_core::server::Server;
    ///
    /// let server = Server::new().keep_alive(false);
    /// let _ = server;
    /// ```
    #[must_use]
    pub fn keep_alive(mut self, enabled: bool) -> Self {
        self.keep_alive_enabled = enabled;
        self
    }

    /// graceful shutdown（[`BoundServer::run_until`]）の in-flight リクエスト
    /// 完了待ち上限を設定する（既定 `DEFAULT_SHUTDOWN_GRACE_PERIOD` = 30 秒、
    /// イシュー #313）。
    ///
    /// シャットダウンシグナル受信後、[`BoundServer::run_until`] は新規接続の
    /// 受理を止めた上で、処理中の全接続（WebSocket セッション等の長時間
    /// 委譲先を含む。`BoundServer::run_until` の doc の「既知の限界」を参照）が
    /// 完了するのをこの時間だけ待つ。上限を超えても完了しない接続は
    /// 強制クローズし、必ず有界時間で `run_until` が戻る（ハング防止の
    /// フェイルクローズ、`.claude/rules/security.md` のリソース枯渇・
    /// 可用性観点）。
    ///
    /// ```
    /// use std::time::Duration;
    /// use fandhe_backend_core::server::Server;
    ///
    /// let server = Server::new().shutdown_grace_period(Duration::from_secs(10));
    /// let _ = server;
    /// ```
    #[must_use]
    pub fn shutdown_grace_period(mut self, grace: Duration) -> Self {
        self.shutdown_grace_period = grace;
        self
    }

    /// [`Middleware`] を登録する（登録順に `on_request` / `on_response` が呼ばれる）。
    #[must_use]
    pub fn middleware(mut self, middleware: impl Middleware + 'static) -> Self {
        self.middlewares.push(Box::new(middleware));
        self
    }

    /// [`RequestGate`] を登録する（登録順に評価し、最初の `Reject` を優先する）。
    #[must_use]
    pub fn gate(mut self, gate: impl RequestGate + 'static) -> Self {
        self.gates.push(Box::new(gate));
        self
    }

    /// [`UpgradeHandler`] を登録する（登録順に `matches` を評価する）。
    #[must_use]
    pub fn upgrade_handler(mut self, handler: impl UpgradeHandler + 'static) -> Self {
        self.upgrade_handlers.push(Box::new(handler));
        self
    }

    /// [`Interceptor`]（イシュー #420）を登録する。複数登録可能で、
    /// `intercept` は登録順に評価して最初の `Some` が勝ち、`map_response` は
    /// 登録順に逐次適用する（評価位置・fail-closed 除外は
    /// [`crate::interceptor`] モジュール doc を参照）。
    ///
    /// ```
    /// use fandhe_backend_core::Server;
    /// use fandhe_backend_core::interceptor::Interceptor;
    ///
    /// struct Noop;
    /// impl Interceptor for Noop {
    ///     fn name(&self) -> &'static str {
    ///         "noop"
    ///     }
    /// }
    ///
    /// let server = Server::new().interceptor(Noop);
    /// let _ = server;
    /// ```
    #[must_use]
    pub fn interceptor(mut self, interceptor: impl Interceptor + 'static) -> Self {
        self.interceptors.push(Box::new(interceptor));
        self
    }

    /// 既定ハンドラ（[`Handler`]）を登録する。未登録時は 404 を返す。
    #[must_use]
    pub fn handler(mut self, handler: impl Handler + 'static) -> Self {
        self.handler = Some(Box::new(handler));
        self
    }

    /// WebRTC シグナリングプロキシプラグイン（`crates/plugin-webrtc-proxy`）を
    /// 有効化する（`webrtc-proxy` feature 限定 API、TASK-2.1 / #18）。
    ///
    /// 登録すると `POST /rtc/offer` が `RequestGate` → `UpgradeHandler` の
    /// 評価を通過した後、既定 [`Handler`] より先にパスインターセプトされ、
    /// `config` が指す上流 WebRTC サービスへ中継される（対象外パスは素通り
    /// し、既定 `Handler` へフォールスルーする。`crate::plugin::try_intercept`
    /// の doc を参照）。
    #[cfg(feature = "webrtc-proxy")]
    #[must_use]
    pub fn webrtc_proxy(mut self, config: fandhe_backend_plugin_webrtc_proxy::ProxyConfig) -> Self {
        self.webrtc_proxy_config = Some(config);
        self
    }

    /// `plugin::try_intercept` が参照する、登録済み WebRTC プロキシ設定
    /// （`webrtc-proxy` feature 限定、TASK-2.1 / #18）。
    #[cfg(feature = "webrtc-proxy")]
    pub(crate) fn webrtc_proxy_config(
        &self,
    ) -> Option<&fandhe_backend_plugin_webrtc_proxy::ProxyConfig> {
        self.webrtc_proxy_config.as_ref()
    }

    /// in-process WebRTC プラグイン（`crates/plugin-webrtc`）を有効化する
    /// （`webrtc` feature 限定 API、TASK-8.1 / #26）。
    ///
    /// 登録すると `POST /rtc/offer` が `RequestGate` → `UpgradeHandler` の評価を
    /// 通過した後、既定 [`Handler`] より先にパスインターセプトされ、`config` を
    /// 使って `RTCPeerConnection` を生成しシグナリングを完結させる（対象外パスは
    /// 素通りし、既定 `Handler` へフォールスルーする。`crate::plugin::try_intercept`
    /// の doc を参照）。`webrtc-proxy`（別プロセス切り出し型）と同時に登録した
    /// 場合は `webrtc-proxy` が優先される（`crate::plugin::try_intercept` の doc）。
    #[cfg(feature = "webrtc")]
    #[must_use]
    pub fn webrtc(mut self, config: fandhe_backend_plugin_webrtc::WebRtcConfig) -> Self {
        self.webrtc_config = Some(config);
        self
    }

    /// `plugin::try_intercept` が参照する、登録済み in-process WebRTC 設定
    /// （`webrtc` feature 限定、TASK-8.1 / #26）。
    #[cfg(feature = "webrtc")]
    pub(crate) fn webrtc_config(&self) -> Option<&fandhe_backend_plugin_webrtc::WebRtcConfig> {
        self.webrtc_config.as_ref()
    }

    /// WebSocket プラグイン（`crates/plugin-websocket`）を有効化する
    /// （`websocket` feature 限定 API、TASK-4.1 / #22）。
    ///
    /// 登録すると `config.path`（既定 `/ws`）への `GET` + `Upgrade: websocket`
    /// リクエストが `RequestGate` の評価を通過した後、
    /// [`UpgradeHandler`] 拡張点経由で検知
    /// され（`WebSocketUpgradeAdapter` を内部で自動登録する）、
    /// `crate::plugin::try_handle_upgrade` が
    /// `fandhe_backend_plugin_websocket::handle_upgrade` へ完全委譲する
    /// （REQ-4「コア自身の HTTP パーサでアップグレードを検知し既存拡張点
    /// 経由で委譲する」という建て付けを維持する。`crates/plugin-websocket/src/lib.rs`
    /// の doc を参照）。異なる `path` で複数回呼び出すと複数パスを登録
    /// できる（`websocket_configs()` の doc を参照）。`config` に
    /// `WebSocketConfig::with_handler` でユーザー定義メッセージハンドラを
    /// 登録しておけば、Text/Binary 受信ごとにそのハンドラへ委譲される
    /// （既定は `EchoHandler`、Issue #179）。コア自身はハンドラ呼び出しに
    /// 関与せず、`fandhe_backend_plugin_websocket::handle_upgrade` 以下に閉じる。
    #[cfg(feature = "websocket")]
    #[must_use]
    pub fn websocket(mut self, config: fandhe_backend_plugin_websocket::WebSocketConfig) -> Self {
        self.upgrade_handlers
            .push(Box::new(WebSocketUpgradeAdapter {
                config: config.clone(),
            }));
        self.websocket_configs.push(config);
        self
    }

    /// `crate::plugin::try_handle_upgrade` が参照する、登録済み WebSocket
    /// 設定群（`websocket` feature 限定、TASK-4.1 / #22）。
    ///
    /// `Server::websocket` を呼んだ順に格納されており、`upgrade_handlers`
    /// 内の `WebSocketUpgradeAdapter` の登録順と一致する。呼び出し元は
    /// 登録順に `fandhe_backend_plugin_websocket::matches` を評価し、最初に一致した
    /// 設定を使うこと（複数パス登録時に先に登録したパスが後の登録で
    /// 上書きされて失われないようにするための契約）。
    #[cfg(feature = "websocket")]
    pub(crate) fn websocket_configs(&self) -> &[fandhe_backend_plugin_websocket::WebSocketConfig] {
        &self.websocket_configs
    }

    /// GraphQL プラグイン（`crates/plugin-graphql`）を有効化する
    /// （`graphql` feature 限定 API、TASK-5.1 / #38）。
    ///
    /// 登録すると `POST /graphql` が `RequestGate` → `UpgradeHandler` の評価を
    /// 通過した後、既定 [`Handler`] より先にパスインターセプトされ、`config`
    /// が保持するスキーマでクエリを実行する（対象外パスは素通りし、既定
    /// `Handler` へフォールスルーする。`crate::plugin::try_intercept` の doc
    /// を参照）。**未登録の場合は feature が有効でも常にフォールスルー**
    /// （404）する（`webrtc-proxy`・`webrtc` と同じ設定登録型パターン）。
    #[cfg(feature = "graphql")]
    #[must_use]
    pub fn graphql(mut self, config: fandhe_backend_plugin_graphql::GraphQlConfig) -> Self {
        self.graphql_config = Some(config);
        self
    }

    /// `plugin::try_intercept` が参照する、登録済み GraphQL スキーマ設定
    /// （`graphql` feature 限定、TASK-5.1 / #38）。
    #[cfg(feature = "graphql")]
    pub(crate) fn graphql_config(&self) -> Option<&fandhe_backend_plugin_graphql::GraphQlConfig> {
        self.graphql_config.as_ref()
    }

    /// OpenAPI ドキュメント配信プラグイン（`crates/plugin-openapi`）を
    /// フレームワーク固定スキーマ（[`ApiDoc`][fandhe_backend_plugin_openapi::ApiDoc]）で
    /// 有効化する（`openapi` feature 限定 API、TASK-2.1 / #256。`GET /openapi.yaml`
    /// 配信の追加は #279）。
    ///
    /// 登録すると `GET /openapi.json` と `GET /openapi.yaml` の両方が
    /// `RequestGate` → `UpgradeHandler` の評価を通過した後、既定 [`Handler`] より
    /// 先にパスインターセプトされ、`fandhe_backend_plugin_openapi::OPENAPI_JSON` /
    /// `OPENAPI_YAML`（いずれもコンパイル時埋め込みの静的文字列、同一スキーマ源）を
    /// それぞれ `Content-Type: application/json` / `application/yaml` で返す
    /// （対象外パス・メソッドは素通りし、既定 `Handler` へフォールスルーする。
    /// `crate::plugin::try_intercept` の doc を参照）。**未登録の場合は
    /// feature が有効でも常にフォールスルー**（404）する（`webrtc-proxy`・
    /// `graphql` と同じ設定登録型パターン）。API 構造の開示となるため、既定
    /// 非公開（fail-closed）とし利用者の明示登録を必須とする
    /// （`.claude/rules/security.md` の A01/A05 観点）。
    ///
    /// 利用者アプリ独自のスキーマを配信したい場合は [`Server::openapi_with`]
    /// を使う（イシュー #320）。両方呼んだ場合は最後に呼んだ方が勝つ
    /// （builder の直感に一致する後勝ちルール、内部の配信登録状態管理を
    /// 参照）。
    ///
    /// # Examples
    /// ```
    /// use fandhe_backend_core::Server;
    ///
    /// let server = Server::new().openapi();
    /// let _ = server;
    /// ```
    #[cfg(feature = "openapi")]
    #[must_use]
    pub fn openapi(mut self) -> Self {
        self.openapi_registration = OpenApiRegistration::Embedded;
        self
    }

    /// 利用者アプリ独自の OpenAPI ドキュメント
    /// （[`OpenApiDoc`][fandhe_backend_plugin_openapi::OpenApiDoc]）を登録して
    /// OpenAPI 配信プラグインを有効化する（`openapi` feature 限定 API、
    /// イシュー #320）。
    ///
    /// [`Server::openapi`]（フレームワーク固定スキーマ）とは異なり、利用者
    /// アプリが自前で生成した OpenAPI ドキュメント（`utoipa` 由来・他ツール
    /// 生成いずれも可）を `GET /openapi.json` / `GET /openapi.yaml` として
    /// 配信できる。
    /// [`OpenApiDoc::from_json`][fandhe_backend_plugin_openapi::OpenApiDoc::from_json]
    /// が構築時（本メソッド呼び出し前）に JSON 妥当性を一度だけ検証済みのため、
    /// 本メソッド自体は追加検証を行わない（fail-closed の検証責務は
    /// [`OpenApiDoc`][fandhe_backend_plugin_openapi::OpenApiDoc] 側、
    /// `crates/plugin-openapi/src/custom.rs` の doc を参照）。
    /// `OpenApiDoc::yaml()` が `None`（`with_yaml` 未呼び出し）の場合、
    /// `GET /openapi.yaml` は既定 `Handler` へフォールスルーする（404）。
    ///
    /// [`Server::openapi`] と `openapi_with` は排他ではなく**後勝ち**
    /// （`crate::plugin::try_intercept` は最後に登録された配信登録状態のみを
    /// 参照する）。両方を呼ぶ意味のある構成は通常ないが、builder パターンの
    /// 一貫性のため片方だけを許可する特別扱いはしない。
    ///
    /// # Examples
    /// ```
    /// use fandhe_backend_core::Server;
    /// use fandhe_backend_plugin_openapi::OpenApiDoc;
    ///
    /// let doc = OpenApiDoc::from_json(r#"{"openapi":"3.0.0","info":{"title":"t","version":"1"}}"#)
    ///     .expect("妥当な JSON");
    /// let server = Server::new().openapi_with(doc);
    /// let _ = server;
    /// ```
    #[cfg(feature = "openapi")]
    #[must_use]
    pub fn openapi_with(mut self, doc: fandhe_backend_plugin_openapi::OpenApiDoc) -> Self {
        self.openapi_registration = OpenApiRegistration::Custom(doc);
        self
    }

    /// `plugin::try_intercept` が参照する、`GET /openapi.json` /
    /// `GET /openapi.yaml` の配信登録状態（`openapi` feature 限定、
    /// TASK-2.1 / #256、イシュー #320）。
    #[cfg(feature = "openapi")]
    pub(crate) fn openapi_registration(&self) -> &OpenApiRegistration {
        &self.openapi_registration
    }

    /// CORS プラグイン（`crates/plugin-cors`）を有効化する（`cors` feature
    /// 限定 API、イシュー #305）。
    ///
    /// 登録すると `crate::plugin::finalize_response`（レスポンス後処理型
    /// シーム）が全レスポンス（`try_intercept` 応答・既定 `Handler` 応答の
    /// 双方）に対して `fandhe_backend_plugin_cors::apply_cors_headers` を適用し、
    /// 許可オリジンからの実リクエストへ CORS ヘッダを付与する。**未登録の
    /// 場合は feature が有効でも常にレスポンスを変更しない**
    /// （`webrtc-proxy`・`graphql`・`openapi` と同じ設定登録型パターン）。
    ///
    /// プリフライト（`OPTIONS` + `Origin` + `Access-Control-Request-Method`）は
    /// 本メソッドの登録対象外。利用者が
    /// `fandhe_backend_plugin_cors::preflight_response` を
    /// `fandhe_backend_routes::Router::options_fallback`（イシュー #304）へ
    /// 直接配線する 2 点構成とする（`crates/plugin-cors/src/lib.rs` の
    /// crate doc・`crates/core/examples/cors_demo.rs` を参照）。
    ///
    /// # Examples
    /// ```
    /// use fandhe_backend_core::Server;
    /// use fandhe_backend_plugin_cors::CorsConfig;
    ///
    /// let config = CorsConfig::builder()
    ///     .allow_origin("https://app.example.com")
    ///     .build()
    ///     .unwrap();
    /// let server = Server::new().cors(config);
    /// let _ = server;
    /// ```
    #[cfg(feature = "cors")]
    #[must_use]
    pub fn cors(mut self, config: fandhe_backend_plugin_cors::CorsConfig) -> Self {
        self.cors_config = Some(config);
        self
    }

    /// `crate::plugin::finalize_response` が参照する、登録済み CORS 設定
    /// （`cors` feature 限定、イシュー #305）。
    #[cfg(feature = "cors")]
    pub(crate) fn cors_config(&self) -> Option<&fandhe_backend_plugin_cors::CorsConfig> {
        self.cors_config.as_ref()
    }

    /// 圧縮プラグイン（`crates/plugin-compression`）を有効化する
    /// （`compression` feature 限定 API、イシュー #321）。
    ///
    /// 登録すると `crate::plugin::finalize_response`（レスポンス後処理型
    /// シーム）が全レスポンス（`try_intercept` 応答・既定 `Handler` 応答の
    /// 双方）に対して `fandhe_backend_plugin_compression::apply_compression`
    /// を適用し、`fandhe_backend_plugin_compression::CompressionConfig` の
    /// 判定基準（ステータス・`Content-Type`・body サイズ・
    /// `Accept-Encoding`）を満たすレスポンスを gzip 圧縮する。**未登録の
    /// 場合は feature が有効でも常にレスポンスを変更しない**
    /// （`cors`・`graphql`・`openapi` と同じ設定登録型パターン）。
    ///
    /// `cors` feature も同時に有効な場合、`finalize_response` は CORS
    /// ヘッダ付与を先に適用してから圧縮を適用する（body を確定させる
    /// 後処理は必ず最後、`crates/plugin-compression/src/lib.rs` の
    /// crate doc を参照）。
    ///
    /// # Examples
    /// ```
    /// use fandhe_backend_core::Server;
    /// use fandhe_backend_core::plugin_compression::CompressionConfig;
    ///
    /// let config = CompressionConfig::builder().build();
    /// let server = Server::new().compression(config);
    /// let _ = server;
    /// ```
    #[cfg(feature = "compression")]
    #[must_use]
    pub fn compression(
        mut self,
        config: fandhe_backend_plugin_compression::CompressionConfig,
    ) -> Self {
        self.compression_config = Some(config);
        self
    }

    /// `crate::plugin::finalize_response` が参照する、登録済み圧縮設定
    /// （`compression` feature 限定、イシュー #321）。
    #[cfg(feature = "compression")]
    pub(crate) fn compression_config(
        &self,
    ) -> Option<&fandhe_backend_plugin_compression::CompressionConfig> {
        self.compression_config.as_ref()
    }

    /// 静的ファイル配信プラグイン（`crates/plugin-static`）を有効化する
    /// （`static` feature 限定 API、イシュー #318）。
    ///
    /// 登録すると `crate::plugin::try_intercept` が `config.mount()`
    /// プレフィックスに一致する `GET` リクエストを
    /// `fandhe_backend_plugin_static::try_handle_static` へパスインター
    /// セプトし、`config.root()`（構築時に canonicalize 済み）配下のファイルを
    /// 返す。**未登録の場合は feature が有効でも常にフォールスルーする**
    /// （`graphql`・`openapi`・`cors` と同じ設定登録型パターン）。
    ///
    /// mount `/` で静的サイトと `Router` の動的エンドポイント（例
    /// `GET /healthz`）を共存させる構成には
    /// [`fandhe_backend_plugin_static::StaticFilesConfigBuilder::fallthrough_on_miss`]
    /// を有効にする（既定 `false`。未指定だと未ヒット GET が静的層で一律
    /// 404 確定し `Router` に到達しない、イシュー #419）。
    ///
    /// # Examples
    /// ```
    /// use fandhe_backend_core::Server;
    /// use fandhe_backend_core::plugin_static::StaticFilesConfig;
    ///
    /// let config = StaticFilesConfig::builder("/static", std::env::temp_dir())
    ///     .build()
    ///     .unwrap();
    /// let server = Server::new().static_files(config);
    /// let _ = server;
    /// ```
    #[cfg(feature = "static")]
    #[must_use]
    pub fn static_files(mut self, config: fandhe_backend_plugin_static::StaticFilesConfig) -> Self {
        self.static_files_config = Some(config);
        self
    }

    /// `crate::plugin::try_intercept` が参照する、登録済み静的ファイル配信
    /// 設定（`static` feature 限定、イシュー #318）。
    #[cfg(feature = "static")]
    pub(crate) fn static_files_config(
        &self,
    ) -> Option<&fandhe_backend_plugin_static::StaticFilesConfig> {
        self.static_files_config.as_ref()
    }

    /// トレーシングプラグイン（`crates/plugin-tracing`）を有効化する
    /// （`tracing` feature 限定 API、TASK-10.1 / #56）。
    ///
    /// 内部で `TracingMiddleware` を組み立てて既存の `middlewares` へ登録する
    /// （`webrtc-proxy` / `websocket` のような専用フィールドを `Server` に追加
    /// する必要はない。汎用 [`Server::middleware`] の薄いラッパーとして実装
    /// できる点が本プラグインの拡張点である `Middleware` の特徴。他プラグイン
    /// が使う「設定登録型」パターンとは異なる）。登録すると全リクエストの
    /// `on_response` フックで [`fandhe_backend_plugin_tracing::TracingLayer::record_response`]
    /// が呼ばれ、`config.exclude_paths`（TASK-10.3 / #58）に完全一致するパス
    /// は記録・サンプリング周期の消費のいずれも行わずスキップされ、それ以外は
    /// `config.sample_interval` に従いサンプリングされたリクエストの応答時
    /// 1 イベント（TASK-10.2 / #57 で span+2 イベントから統合）のみが記録される
    /// （`crates/plugin-tracing/src/layer.rs` の doc を参照）。ヘルスチェック等の
    /// 高頻度パスを `exclude_paths` に登録することで、TASK-10.4 の性能再検証
    /// （RPS 劣化 5% 以内）の前提を満たせる。記録先（非同期・バッファ済み I/O）は別途
    /// `fandhe_backend_plugin_tracing::init_tracing` で初期化する契約とし、本メソッドは
    /// グローバルサブスクライバの初期化には関与しない。
    #[cfg(feature = "tracing")]
    #[must_use]
    pub fn tracing(mut self, config: fandhe_backend_plugin_tracing::TracingConfig) -> Self {
        self.middlewares
            .push(Box::new(TracingMiddleware::new(&config)));
        self
    }

    /// `addr` に TCP リスナーをバインドし、[`BoundServer`] を返す。
    ///
    /// 以降 `self` は `Arc` で包まれ、accept したコネクションタスク間で
    /// 共有される（拡張点実装は `Send + Sync` を要求される理由）。
    pub async fn bind(self, addr: impl ToSocketAddrs) -> io::Result<BoundServer> {
        let listener = TcpListener::bind(addr).await?;
        // 0 を指定すると accept ループが永久に許可待ちでブロックし、
        // 新規接続を一切受理できなくなる（誤用によるデッドロック防止のため
        // 最低 1 に切り上げる）。この `permit_count` は下の `Semaphore::new` と
        // `permit_total`（[`BoundServer::run_until`] の in-flight 完了待ちが
        // 「全 permit の回収」を「全接続完了」とみなす根拠）の**唯一の
        // 発生源**であり、二重計算による乖離を避けるため 1 回だけ計算する。
        let permit_count = self.max_connections.max(1);
        let connection_limit = Arc::new(Semaphore::new(permit_count));
        // セマフォの総 permit 数は `usize`、`acquire_many_owned` は `u32` を取る。
        // `max_connections` が `u32::MAX` を超える極端な設定でも in-flight 完了待ち
        // 自体は成立させる（切り詰めても「全 permit 回収」の意味は保たれる。
        // 実運用でここまで大きい `max_connections` は想定しない）。
        let permit_total = u32::try_from(permit_count).unwrap_or(u32::MAX);
        Ok(BoundServer {
            listener,
            server: Arc::new(self),
            connection_limit,
            permit_total,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            rebind_tx: None,
            rebind_rx: None,
        })
    }
}

/// [`Server::websocket`] が内部登録する [`UpgradeHandler`] アダプタ
/// （`websocket` feature 限定、TASK-4.1 / #22）。
///
/// `UpgradeHandler::matches` は「委譲判定のみ」の契約（同期 API、
/// `crates/core/src/extension.rs` の doc）のため、本アダプタは
/// `fandhe_backend_plugin_websocket::matches`（純関数）を呼ぶだけの薄い委譲先とし、
/// 実際のハンドシェイク検証・フレーミング委譲（非同期処理）は
/// `crate::plugin::try_handle_upgrade` → `fandhe_backend_plugin_websocket::handle_upgrade`
/// が担う。`config` は `Server::websocket` 呼び出し時にクローンして保持する
/// （`upgrade_handlers: Vec<Box<dyn UpgradeHandler>>` は `Server` 本体と
/// ライフタイムを共有しないため、参照ではなく所有値として持つ）。
#[cfg(feature = "websocket")]
struct WebSocketUpgradeAdapter {
    config: fandhe_backend_plugin_websocket::WebSocketConfig,
}

#[cfg(feature = "websocket")]
impl UpgradeHandler for WebSocketUpgradeAdapter {
    fn name(&self) -> &'static str {
        "websocket"
    }

    fn matches(&self, head: &RequestHead) -> bool {
        fandhe_backend_plugin_websocket::matches(head, &self.config)
    }
}

/// [`Server::tracing`] が内部登録する [`Middleware`] アダプタ
/// （`tracing` feature 限定、TASK-10.1 / #56）。
///
/// `Middleware` は同期 API（dyn 互換性維持、`crates/core/src/extension.rs` の
/// doc）のため、本アダプタは `on_request` を no-op とし、`on_response` でのみ
/// `fandhe_backend_plugin_tracing::TracingLayer::record_response` へ委譲する（`crates/
/// plugin-tracing/src/layer.rs` の doc「記録は on_response の 1 点に集約する」
/// を参照。`Middleware` trait には request/response を跨いで per-request 状態を
/// 運ぶ経路がなく、`on_request` と `on_response` で独立にサンプリング判定すると
/// 同一リクエストの記録が対にならないため）。`TracingLayer` 自体が
/// `Sampler`（`AtomicU64`）を保持し内部可変性で判定するため、本アダプタは
/// `&self` の不変参照のみで足りる（AGENTS.md「規約: ミドルウェア非同期 I/O
/// 必須化」が要求する非ブロッキング操作の要件を満たす）。
#[cfg(feature = "tracing")]
struct TracingMiddleware {
    layer: fandhe_backend_plugin_tracing::TracingLayer,
}

#[cfg(feature = "tracing")]
impl TracingMiddleware {
    fn new(config: &fandhe_backend_plugin_tracing::TracingConfig) -> Self {
        Self {
            layer: fandhe_backend_plugin_tracing::TracingLayer::new(config),
        }
    }
}

#[cfg(feature = "tracing")]
impl Middleware for TracingMiddleware {
    fn name(&self) -> &'static str {
        "tracing"
    }

    fn on_request(&self, _head: &RequestHead) {
        // 記録は on_response に一本化する（本 struct の doc を参照）。
    }

    fn on_response(&self, head: &RequestHead, elapsed: Duration) {
        self.layer.record_response(head, elapsed);
    }
}

/// [`Server::bind`] が返す、リスニングソケットを保持した状態のサーバ。
pub struct BoundServer {
    listener: TcpListener,
    server: Arc<Server>,
    /// 同時接続数の上限を強制するセマフォ（`DEFAULT_MAX_CONNECTIONS` の doc を参照）。
    /// permit は [`BoundServer::run`] が spawn するコネクションタスクへ move し、
    /// タスク終了（`handle_connection` の戻り）時に自動で解放される。
    /// graceful shutdown（[`BoundServer::run_until`]）は shutdown 後に
    /// `permit_total` 個の permit を全回収することで「全 in-flight 接続が
    /// 完了した」を検知する（[`Server::bind`] の doc を参照）。
    connection_limit: Arc<Semaphore>,
    /// `connection_limit` の初期 permit 総数（[`Server::bind`] で
    /// `max_connections.max(1)` から一意に導出、`u32` へクランプ済み）。
    /// [`BoundServer::run_until`] が in-flight 完了待ちで
    /// `acquire_many_owned(permit_total)` に使う。
    permit_total: u32,
    /// graceful shutdown シグナル受信を各コネクションタスクへ伝える
    /// フラグ（イシュー #313）。[`BoundServer::run_until`] がシャットダウン
    /// 検知時に `true` を立て、[`handle_connection_with_permit`] の
    /// keep-alive 判定に反映される（処理中のリクエストは完走させつつ、
    /// 以降は `Connection: close` を付けて早期に接続を閉じる）。`rebind()`
    /// による listener 差し替え（イシュー #485）でも、差し替え直前の
    /// 「旧世代」の接続にだけ同じ役割のフラグを立てる（本フィールドは
    /// 常に「現行世代」を指す。世代交代の詳細は `RebindHandle::rebind` の
    /// doc・`docs/design/rebind.md` を参照）。
    shutdown_flag: Arc<AtomicBool>,
    /// [`BoundServer::rebind_handle`] が初回呼び出し時に生成する `mpsc`
    /// チャネルの送信側（イシュー #485）。2 回目以降の `rebind_handle` 呼び出しは
    /// これを `clone` して返すだけで、チャネル自体は再生成しない。
    /// `rebind_handle` を一度も呼ばなければ `None` のままであり、チャネルも
    /// 生成されない（pay-for-what-you-use。呼ばない利用者には mpsc の
    /// ランタイムコストを一切払わせない）。
    rebind_tx: Option<mpsc::Sender<RebindCommand>>,
    /// `rebind_tx` と対になる受信側。[`BoundServer::run_until`] が `self` から
    /// 取り出し、accept ループ内で shutdown・rebind コマンド・accept の
    /// 3-way race に使う。`rebind_handle` が一度も呼ばれなければ `None` のまま
    /// （その場合 `run_until` は rebind コマンドを一切ポーリングしない）。
    rebind_rx: Option<mpsc::Receiver<RebindCommand>>,
}

/// [`RebindHandle::rebind`] が [`BoundServer::run_until`] の accept ループへ
/// 送る、listener 差し替え 1 回分の指示（イシュー #485、非公開）。
///
/// `listener` は `RebindHandle::rebind` が呼び出し元で bind 済みの新規
/// `TcpListener`（bind 失敗時はそもそもこの構造体が作られない、fail-closed）。
/// `reply` は `run_until` 側が実際に listener を差し替えた直後に `()` を
/// 送って完了を通知するための oneshot。`RebindHandle::rebind` はこれの
/// 受信をもって「新アドレスへの差し替えが完了した」とみなし `local_addr` を
/// 返す。
struct RebindCommand {
    listener: TcpListener,
    reply: oneshot::Sender<()>,
}

/// 稼働中の [`BoundServer::run_until`] へ、リスニングアドレスの差し替えを
/// 指示するためのハンドル（イシュー #485）。[`BoundServer::rebind_handle`]
/// から得る。
///
/// `Clone` 可能で複数タスク・複数回の呼び出しに使い回せる。内部は
/// `run_until` の accept ループへコマンドを送る `mpsc::Sender` のみを保持する
/// 薄いラッパー。
///
/// # セキュリティ
///
/// [`RebindHandle::rebind`] に渡すアドレスへ、HTTP リクエスト由来の値
/// （クエリパラメータ・ヘッダ等の外部入力）を直接渡さないこと。信頼できない
/// 値を bind 先に使うと、意図しないインターフェースへの待受につながる
/// （`.claude/rules/security.md` の入力検証観点）。運用者が制御する設定値・
/// 環境変数からのみ呼び出すこと。
#[derive(Clone)]
pub struct RebindHandle {
    tx: mpsc::Sender<RebindCommand>,
}

impl RebindHandle {
    /// `addr` へ新規 `TcpListener` を bind し、成功したら稼働中の
    /// [`BoundServer::run_until`] accept ループへ listener の差し替えを
    /// 依頼する。差し替えが完了したら実際にバインドされたアドレスを返す。
    ///
    /// # fail-closed（bind 失敗時は旧 listener・in-flight に無影響）
    ///
    /// bind 自体はこのメソッド内で完結する。アドレス使用中等で bind に
    /// 失敗した場合は `Err` を返すのみで、`run_until` へは何も送信しない
    /// ため、旧 listener・処理中の接続には一切影響しない。
    ///
    /// # 差し替え後の挙動（世代別 drain）
    ///
    /// 差し替え成功後は新規接続のみ新アドレスで受理され、旧アドレスの
    /// listener は即座に閉じられる（以降の新規接続は OS レベルで拒否）。
    /// 旧アドレス経由で確立済みの in-flight 接続（「旧世代」）は
    /// [`Server::shutdown_grace_period`] を上限に完走を待ち、超過分は
    /// 強制クローズする。これは [`BoundServer::run_until`] の graceful
    /// shutdown（イシュー #313）と同じ仕組みを世代ごとに独立して適用した
    /// もので、drain 待ちは `run_until` 自体をブロックしない（背景タスクで
    /// 実行、新世代の accept ループは並行して動き続ける）。詳細な設計判断は
    /// `docs/design/rebind.md` を参照。
    ///
    /// # WebSocket 委譲セッションは `JoinSet` の drain 対象外（キャンセルは伝播する）
    ///
    /// 上記の世代別 drain（`JoinSet::shutdown` による強制 abort）が対象と
    /// するのは `run_until` が管理するコネクションタスク一式のみである。
    /// `UpgradeHandler` の委譲が成立し `handle_connection_with_permit` から
    /// WebSocket 専用タスクへ permit ごと `move` された接続は、この管理
    /// 対象の外にあるため、grace 超過時の `JoinSet` 強制 abort には含まれ
    /// ない。permit は世代を跨いで共有する `connection_limit` セマフォ経由
    /// のため、`run_until` 自体の最終 graceful shutdown・以降の drain 待ち
    /// には（grace 超過後の打ち切りを含め）反映される。
    ///
    /// イシュー #491（`docs/design/ws-cancellation-propagation.md`）で、
    /// この drain タスクの冒頭にて世代キャンセル（`crate::plugin::
    /// GenerationCancel::fire`）を発火し、WS 委譲タスクへ明示的な
    /// キャンセルシグナルを伝播する経路を追加した。#492 で
    /// `fandhe_backend_plugin_websocket::handle_upgrade` が正常な Close
    /// ハンドシェイク（close code 1001 Going Away を送出し、
    /// `WebSocketConfig::close_grace`（既定 10 秒）上限でクライアント応答を
    /// 待つ）を実装済みで、キャンセル発火後は
    /// ハードクローズではなくこの正常終端シーケンスを経由する。Close に
    /// 応答しないクライアントは `WebSocketConfig::close_grace`（既定 10 秒）
    /// 有界で終端し、detached
    /// タスクとして残ることがあるが `run_until`／`rebind` 自体の復帰には
    /// 影響しない（`crate::plugin::try_handle_upgrade` の doc「世代
    /// キャンセルシグナル」・統合テスト `crates/core/tests/
    /// ws_cancellation.rs`（イシュー #493）を参照）。
    ///
    /// # キャンセル安全性・並行呼び出し
    ///
    /// - **呼び出し元タスクのキャンセル**: `tx.send` が成功したあと
    ///   `reply_rx.await` の完了前に、この `rebind` を呼んでいる Future
    ///   自体が外部キャンセルされた場合（例: 呼び出し元が `tokio::select!`
    ///   や `timeout` で打ち切られた場合）、`run_until` 側は既に受理した
    ///   コマンドの処理を独立に進めるため listener の差し替えは実行され
    ///   うる。しかしキャンセルされた呼び出し元は差し替え後のアドレスを
    ///   受け取れない（`Ok(new_addr)` の返却前に Future が破棄されるため）。
    ///   差し替え自体の成否を確実に知りたい場合は、キャンセルせず
    ///   `rebind` の完了を待つこと。待てない事情がある場合は、意図した
    ///   アドレスへ改めて `rebind` を呼び出して結果を確認するか、新
    ///   アドレスへの実際の疎通確認（TCP 接続確立の成否）を運用側で行うこと。
    ///   `BoundServer` は `run_until(self, ...)` へ move されるため、
    ///   `rebind` 呼び出し元から `BoundServer::local_addr()` を参照すること
    ///   はできない
    /// - **複数ハンドル・複数タスクからの並行呼び出し**: `RebindHandle` は
    ///   `Clone` 可能で、複製したハンドルや別タスクから同時に `rebind` を
    ///   呼び出せる。内部の `mpsc::channel` は容量 1（3.2 節）で直列化
    ///   されるため各コマンドは 1 つずつ処理されるが、処理順序は呼び出し順
    ///   （`send` の到着順）に一致する保証のみで、「先に呼び出しを開始した
    ///   側が先に有効になる」とは限らない。後から処理されたコマンドが
    ///   最終的に有効な listener になり、先行して呼び出した側も
    ///   `reply_rx` の完了をもって `Ok` を返すため、複数の並行呼び出しが
    ///   すべて成功しつつ、実際に有効なアドレスは最後に処理されたものだけ
    ///   という状態になりうる。同一プロセス内から並行に複数の rebind を
    ///   発行する運用は避け、単一の呼び出し元（デプロイスクリプト等）から
    ///   逐次呼び出すこと
    ///
    /// # エラー
    ///
    /// - `addr` への bind に失敗した場合
    /// - `run_until` が既に終了している（一度も呼ばれていない、または
    ///   shutdown 済み）場合
    /// - `run_until` が shutdown を受理した直後（grace drain 開始前）に
    ///   `rebind_rx` を閉じる（イシュー #485）ため、shutdown 確定以降に
    ///   呼び出した・呼び出し中だった `rebind` は grace 期間の終了を待たず
    ///   速やかに `Err` を返す。bind 済みの新 `TcpListener` も同様に速やかに
    ///   drop されポートを保持し続けない
    ///
    /// # Examples
    /// ```no_run
    /// # #[tokio::main]
    /// # async fn main() -> std::io::Result<()> {
    /// use fandhe_backend_core::Server;
    ///
    /// let mut bound = Server::new().bind("127.0.0.1:0").await?;
    /// let rebind = bound.rebind_handle();
    /// tokio::spawn(async move { bound.run().await });
    ///
    /// let new_addr = rebind.rebind("127.0.0.1:0").await?;
    /// let _ = new_addr;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn rebind(&self, addr: impl ToSocketAddrs) -> io::Result<SocketAddr> {
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RebindCommand {
                listener,
                reply: reply_tx,
            })
            .await
            .map_err(|_| {
                io::Error::other("rebind: BoundServer::run_until はすでに終了しています")
            })?;
        reply_rx.await.map_err(|_| {
            io::Error::other("rebind: BoundServer::run_until が差し替え完了前に終了しました")
        })?;
        Ok(local_addr)
    }
}

/// [`BoundServer::run_until`] が shutdown Future・rebind コマンド・accept
/// Future を競合させた結果（本モジュール内の非公開ヘルパー、tokio `macros`
/// feature（`select!` が要求する）を追加しないための選択。
/// `std::future::poll_fn` + `std::pin::pin!` で同等のことを最小依存で実現する）。
enum Raced<T> {
    /// shutdown Future が先に完了した。
    Shutdown,
    /// [`RebindHandle::rebind`]（イシュー #485）からの listener 差し替え
    /// 指示を受信した。
    Rebind(RebindCommand),
    /// accept 側の Future が（shutdown・rebind より先に、またはどちらも
    /// なしで）完了した。
    Completed(T),
}

/// `shutdown`（1 度だけ pin して以後のループ反復をまたいで poll し続ける
/// 必要がある。呼び出し側が `Pin<&mut S>` を渡す契約）・`rebind_rx`
/// （[`BoundServer::rebind_handle`] が一度も呼ばれていなければ `None`。
/// `None` の間は常に pending 扱いでポーリングしない）・`accept`
/// （反復ごとに新規生成される Future）を競合させる。
///
/// `tokio::select!` は `macros` feature（proc-macro 系推移依存）を要求する
/// ため使わない（`crates/core/Cargo.toml` の tokio feature コメント・
/// `.claude/rules/pay-for-what-you-use.md` を参照）。`accept` は cancel-safe
/// （`shutdown`・`rebind` のどちらかが先に完了して drop されても、取得済み
/// permit が自動解放されるだけで接続を取りこぼさない。`BoundServer::run_until`
/// の doc を参照）。
///
/// ポーリング優先順位は shutdown > rebind > accept の固定順（イシュー #485）。
/// shutdown を最優先するのは既存の graceful shutdown（イシュー #313）と
/// 同じ理由（shutdown 直後の新規受理・差し替えを避ける）。rebind を accept
/// より先にポーリングするのは、同一 poll で両方 Ready になりうる場合に
/// 新規接続を「差し替え前の古い listener」で受理してしまう競合を避けるため。
async fn race_shutdown_or_accept<S, A>(
    mut shutdown: Pin<&mut S>,
    mut rebind_rx: Option<&mut mpsc::Receiver<RebindCommand>>,
    accept: A,
) -> Raced<A::Output>
where
    S: Future<Output = ()>,
    A: Future,
{
    let mut accept = std::pin::pin!(accept);
    std::future::poll_fn(move |cx| {
        // shutdown を最優先でポーリングする（既存 doc の理由をそのまま踏襲）。
        if shutdown.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Raced::Shutdown);
        }
        // rebind コマンドは shutdown の次に優先してポーリングする。
        // `rebind_rx` が `None`（`rebind_handle` 未呼び出し）の間はチャネル
        // 自体が存在しないため常に pending 扱いになる。送信側が全て drop
        // された場合（`RebindHandle` が全て破棄された場合）は
        // `poll_recv` が `Poll::Ready(None)` を返しうるが、これは
        // 「今後 rebind コマンドは来ない」ことを意味するだけで shutdown
        // 相当ではないため無視し、以降は accept のみをポーリングし続ける。
        if let Some(rx) = rebind_rx.as_mut()
            && let Poll::Ready(Some(cmd)) = rx.poll_recv(cx)
        {
            return Poll::Ready(Raced::Rebind(cmd));
        }
        match accept.as_mut().poll(cx) {
            Poll::Ready(value) => Poll::Ready(Raced::Completed(value)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

/// [`JoinSet`] をラップし、外部キャンセルによる `Drop` 時は `abort_all` では
/// なく `detach_all` する（Bugbot 指摘、review comment 3615287445）。
///
/// `tokio::task::JoinSet::drop` は保持中の未完了タスクを全て `abort` する。
/// `BoundServer::run_until` 内部では、grace 超過時に明示的な
/// [`JoinSet::shutdown`] 呼び出しで意図的に abort する（フェイルクローズ、
/// `run_until` の doc「上限超過時は強制クローズ」）が、それ以外の経路では
/// `join_set` は使い切られて空になってから関数を抜ける（`join_next` で
/// 全件回収済み）ため、通常完了時の `Drop` は無 op になる。
///
/// 問題になるのは、`run_until` が返す `Future` 自体が呼び出し側の
/// `tokio::select!` 等で外部キャンセルされ、accept ループや in-flight 完了
/// 待ちの途中で打ち切られるケースである。この場合 `join_set` にはまだ
/// 未完了タスクが残っており、素の `JoinSet::drop` だと全 in-flight
/// コネクションを即座に abort してしまう。これは「`run()` の cancel は
/// accept 停止のみで、処理中のリクエストは継続する」という従来（detached
/// `tokio::spawn` 時代）の挙動からの退行になるため、本ラッパーで `Drop` を
/// `detach_all`（タスクを JoinSet の追跡から外すだけで abort しない）に
/// 差し替え、外部キャンセル時も in-flight 接続をそのまま独立タスクとして
/// 完走させる。
struct CancelSafeJoinSet(JoinSet<()>);

impl std::ops::Deref for CancelSafeJoinSet {
    type Target = JoinSet<()>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for CancelSafeJoinSet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for CancelSafeJoinSet {
    fn drop(&mut self) {
        self.0.detach_all();
    }
}

/// `rebind()`（イシュー #485）で listener を差し替える際、差し替え直前
/// までに accept 済みだった「旧世代」のコネクションタスク一式（`old_join_set`）
/// を、`run_until` 自体をブロックせずに独立した背景タスクで drain する。
///
/// `run_until` の最終 graceful shutdown（イシュー #313）と同じ
/// 「grace 期間内に全タスク完了を待ち、超過分は強制クローズ」の手順を
/// 世代単位で適用する。`old_join_set` は `run_until` の accept ループから
/// `std::mem::replace` で切り離された時点で新規タスクが追加されることはない
/// （新規 accept は差し替え後の listener・新しい `JoinSet` へ積まれる）ため、
/// 「完了済みタスクを尽くす」だけで安全に drain できる。
///
/// 生成する `tokio::spawn` タスクは detached（`JoinHandle` を保持しない）。
/// これは許容される: 旧世代の各コネクションが握る permit は現行世代と
/// 共有の `connection_limit` セマフォ由来であり（`Server::bind` の doc
/// 「セマフォは世代を跨いで単一共有」を参照）、`run_until` 自体の最終
/// shutdown が行う `acquire_many_owned(permit_total)` はこの背景タスクの
/// 完了（または強制クローズ）を暗黙に待つことになる。
/// `old_cancel` はこの世代（`old_join_set`）に対応する
/// [`crate::plugin::GenerationCancel`]（イシュー #491）。drain タスクの
/// 冒頭で 1 回だけ発火し（`docs/design/ws-cancellation-propagation.md` 5.2
/// 節「drain 開始時に発火」）、この世代の WS 委譲タスクへキャンセルを
/// 伝播する。`websocket` feature 無効時 `fire()` は no-op（型 doc を参照）。
fn spawn_generation_drain(
    mut old_join_set: CancelSafeJoinSet,
    old_cancel: crate::plugin::GenerationCancel,
    grace: Duration,
) {
    tokio::spawn(async move {
        old_cancel.fire();
        let drained = tokio::time::timeout(grace, async {
            while old_join_set.join_next().await.is_some() {}
        })
        .await;
        if drained.is_err() {
            eprintln!(
                "fandhe_backend_core::server: rebind による旧世代接続の drain が猶予期間（{grace:?}）を超過したため強制クローズします"
            );
            old_join_set.shutdown().await;
        }
    });
}

/// `rebind()`（イシュー #485）で旧 listener を drop する直前に、kernel の
/// accept backlog に滞留していた「3-way handshake 完了済みだが未
/// `accept()`」の接続を非ブロッキング・有界に回収する（イシュー #501）。
///
/// `run_until` の `Raced::Rebind` 分岐から、listener 差し替え（`listener =
/// new_listener`）の**前**に呼ばれる。ここで回収した接続は旧世代
/// （差し替え前の `current_shutdown_flag`・`current_cancel`）としてサーブし、
/// `run_until` 側で旧世代 `join_set` へ積むことで
/// [`spawn_generation_drain`] の grace 付き drain 対象に自動的に合流する
/// （`docs/design/rebind.md` 7 節を参照）。
///
/// # 有界性（DoS 対策、`.claude/rules/security.md`）
///
/// 本関数は実行中一度も `Poll::Pending` で待機しない
/// （`std::future::poll_fn` が常に `Poll::Ready` を返すため `async fn` で
/// あっても時間有界）。以下のいずれかで直ちに打ち切る:
/// - `max`（[`REBIND_BACKLOG_DRAIN_LIMIT`]）件に達した
/// - `connection_limit` の permit が枯渇した（`try_acquire_owned` 失敗。
///   同時接続数上限を迂回しないフェイルクローズ、permit 枯渇時の滞留分は
///   従来どおり drain されず RST を受ける）
/// - backlog が空（`poll_accept` が `Pending` を返した）
/// - `poll_accept` がエラーを返した
///
/// `Pending` を観測した時点で `listener` への waker 登録が残りうるが、
/// 呼び出し元がこの直後に `listener` を drop するため無害（登録された
/// waker は listener と共に破棄される）。
async fn drain_listener_backlog(
    listener: &TcpListener,
    connection_limit: &Arc<Semaphore>,
    max: usize,
) -> Vec<(TcpStream, SocketAddr, OwnedSemaphorePermit)> {
    let mut drained = Vec::new();
    for _ in 0..max {
        // permit ゲートを迂回しない（fail-closed）。枯渇したら残りは
        // 従来どおり旧 listener の drop と共に破棄される。
        let Ok(permit) = Arc::clone(connection_limit).try_acquire_owned() else {
            break;
        };
        match std::future::poll_fn(|cx| Poll::Ready(listener.poll_accept(cx))).await {
            Poll::Ready(Ok((stream, peer_addr))) => drained.push((stream, peer_addr, permit)),
            // Pending = backlog 空。Err（ECONNABORTED 等）も fail-closed で
            // 打ち切る。いずれの分岐でも `permit` はスコープアウトで自動解放。
            Poll::Ready(Err(_)) | Poll::Pending => break,
        }
    }
    drained
}

impl BoundServer {
    /// バインドしたローカルアドレスを返す。`0` ポート指定時の実ポート確認に使う。
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// 稼働中の [`BoundServer::run_until`] へ listener 差し替え（rebind、
    /// イシュー #485）を指示するための [`RebindHandle`] を返す。
    ///
    /// `run_until`（または `run`）を呼ぶ**前**に呼び出す契約。初回呼び出しで
    /// 容量 1 の `mpsc` チャネルを遅延生成し（本メソッドを一度も呼ばなければ
    /// チャネルは作られない、pay-for-what-you-use）、2 回目以降は既存の
    /// 送信側を `clone` して返すだけで済む（`RebindHandle` 自体も `Clone`
    /// 可能なため、複数箇所へ配りたい場合は返り値を `clone` する方が簡潔）。
    ///
    /// # Examples
    /// ```no_run
    /// # #[tokio::main]
    /// # async fn main() -> std::io::Result<()> {
    /// use fandhe_backend_core::Server;
    ///
    /// let mut bound = Server::new().bind("127.0.0.1:0").await?;
    /// let rebind = bound.rebind_handle();
    /// tokio::spawn(async move { bound.run().await });
    /// let _new_addr = rebind.rebind("127.0.0.1:0").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn rebind_handle(&mut self) -> RebindHandle {
        if let Some(tx) = &self.rebind_tx {
            return RebindHandle { tx: tx.clone() };
        }
        // 容量 1: `RebindHandle::rebind` は listener を bind 済みの状態で
        // 1 コマンドを送るだけであり、`run_until` 側がそれを処理するまで
        // 次のコマンドを受け付ける必要はない（DoS 耐性、コマンド滞留を
        // 有界化する。`.claude/rules/security.md` のリソース枯渇対策）。
        let (tx, rx) = mpsc::channel(1);
        self.rebind_tx = Some(tx.clone());
        self.rebind_rx = Some(rx);
        RebindHandle { tx }
    }

    /// accept ループを回し、コネクションごとに [`handle_connection`] を spawn する。
    /// シャットダウン手段を持たない（`std::future::pending` を shutdown Future
    /// として渡すだけの）[`BoundServer::run_until`] の薄いラッパー（イシュー
    /// #313 で既存 API の後方互換を維持するために導入。挙動・シグネチャとも
    /// 従来のまま）。accept エラー処理・同時接続数上限の詳細は
    /// [`BoundServer::run_until`] の doc を参照。
    pub async fn run(self) -> io::Result<()> {
        self.run_until(std::future::pending::<()>()).await
    }

    /// `shutdown` Future が完了するまで accept ループを回し、その後
    /// graceful shutdown シーケンスを実行する（イシュー #313）。
    ///
    /// 各コネクションは独立した tokio タスクで処理されるため、1 接続の
    /// 処理停滞（スロークライアント等）が他接続をブロックしない。
    ///
    /// 同時接続数は `connection_limit` セマフォで上限を強制する。上限に
    /// 達している間は `accept` 自体を呼ばずに待機するため、あふれた接続は
    /// カーネルの listen backlog に滞留し、backlog も尽きれば OS 側で
    /// 拒否される（`DEFAULT_MAX_CONNECTIONS` の doc を参照）。
    ///
    /// # accept エラーの扱い（可用性）
    ///
    /// `listener.accept()` が返すエラー（例: `ECONNABORTED` = accept 前の
    /// クライアント切断、`EMFILE`/`ENFILE` = fd 枯渇）は、リスナー自体が
    /// 壊れたことを意味しない一過性のものが大半である。Tokio 公式ドキュメント
    /// （`TcpListener::accept`）も「多くの accept エラーはサーバ全体ではなく
    /// 個々の接続に紐づくものであり、ログに残してループを継続するのが
    /// 一般的な実践」と述べている。そのため本実装は accept エラーで
    /// `run_until` を終了させず、`ACCEPT_ERROR_BACKOFF` だけ待ってから次の
    /// accept を再試行する（`.claude/rules/security.md` の可用性・
    /// リソース枯渇観点。1 件の一過性エラーでリスナー全体が永久停止するのを
    /// 防ぐ）。戻り値が `io::Result` なのは将来の呼び出し側都合による API
    /// 安定性のためであり、現状の実装は（プロセス終了等の外的要因を除き）
    /// `Err` を返さず、`shutdown` 完了後に必ず `Ok(())` で戻る。
    ///
    /// # graceful shutdown シーケンス
    ///
    /// `shutdown` が完了すると、以下の順序で処理する:
    ///
    /// 1. **accept 停止**: shutdown フラグを立て、リスニングソケットを
    ///    明示的に `drop` する（以降の新規接続は OS レベルで拒否される。
    ///    受け入れ条件「シグナル受信後に新規接続を受け付けない」）。フラグは
    ///    `handle_connection_with_permit`（本クレート非公開）の keep-alive
    ///    判定にも伝わり、処理中のリクエストは完走させつつ、以後は
    ///    `Connection: close` を付けて早期に接続を閉じる（`BoundServer` の
    ///    非公開フィールド `shutdown_flag` の doc を参照）
    /// 2. **in-flight 完了待ち**: [`Server::shutdown_grace_period`]
    ///    （既定 `DEFAULT_SHUTDOWN_GRACE_PERIOD` = 30 秒）を上限に、
    ///    `connection_limit` の全 permit（`permit_total` 個）が解放される
    ///    のを待つ。WebSocket 委譲で専用タスクへ move された permit も
    ///    同じセマフォで解放されるため、WS セッションを含む全 in-flight を
    ///    漏れなく待てる（`crate::plugin::try_handle_upgrade` の doc
    ///    「permit の契約」を参照）
    /// 3. **上限超過時は強制クローズ**: 上限内に全 permit が解放されなければ、
    ///    警告ログを 1 行出した上で残存コネクションタスクを `JoinSet::shutdown`
    ///    で abort する（`TcpStream` が drop されソケットは即時クローズされる。
    ///    ハング防止のフェイルクローズ、受け入れ条件「上限時間・超過時強制
    ///    クローズ」）
    ///
    /// どちらの経路でも `run_until` は `Server::shutdown_grace_period` + ε
    /// 以内に必ず `Ok(())` で戻る。
    ///
    /// shutdown_flag 受信後（`Raced::Shutdown` に到達する前でも、accept
    /// 済みの各コネクションタスクからは shutdown シグナル発火直後に見える）
    /// は、`UpgradeHandler` がマッチする新規リクエストであっても Upgrade
    /// へ委譲せず 503 で拒否する（`handle_connection_with_permit` の
    /// Upgrade 分岐 doc を参照。Bugbot 指摘 review comment 3615144815、
    /// "Upgrade ignores shutdown flag" の是正）。
    ///
    /// `run_until` が返す `Future` 自体が呼び出し側の `tokio::select!` 等で
    /// 外部キャンセルされた場合（一般的な shutdown パターン）は、
    /// `CancelSafeJoinSet`（本モジュールの型 doc を参照）により in-flight
    /// 接続は abort されず、独立タスクとして完走する（Bugbot 指摘 review
    /// comment 3615287445、"Cancel aborts in-flight connections" の是正。
    /// 従来の detached `tokio::spawn` 時代の挙動を維持）。
    ///
    /// # 稼働中の再バインド（`rebind`、イシュー #485）
    ///
    /// [`BoundServer::rebind_handle`] を `run_until` 呼び出し前に取得して
    /// おくと、accept ループを止めずに listening アドレスを差し替えられる。
    /// accept ループは shutdown・rebind コマンド・accept の 3-way race
    /// （優先順位は shutdown > rebind > accept）で回っており、rebind
    /// コマンドを受理すると次の手順を踏む:
    ///
    /// 1. その時点までの「旧世代」向け `shutdown_flag` を `true` にする
    ///    （旧世代の keep-alive 接続は `Connection: close` で早期に閉じる。
    ///    通常の graceful shutdown と同じ機構）
    /// 2. 旧 listener を差し替える前に、accept backlog に滞留していた
    ///    接続を `drain_listener_backlog`（非公開関数）で非ブロッキング・有界に回収し、
    ///    旧世代接続としてサーブする（イシュー #501。`REBIND_BACKLOG_DRAIN_LIMIT`
    ///    件・`connection_limit` の permit ゲート範囲内に限定され、`run_until`
    ///    自体を待機させない。`docs/design/rebind.md` 7 節を参照）
    /// 3. listener を新しい `TcpListener`（[`RebindHandle::rebind`] が
    ///    bind 済み）へ差し替える。以降の accept は新アドレスに対してのみ
    ///    行われる
    /// 4. 旧世代のコネクションタスク一式（上記 2 で回収した分を含む）を
    ///    現行の `JoinSet` から切り離し、独立した背景タスクで
    ///    `Server::shutdown_grace_period` を上限に drain する（超過分は
    ///    強制クローズ）。この drain は `run_until` 自体をブロックせず、
    ///    新世代の accept ループと並行して進む
    /// 5. 新世代用の `shutdown_flag` を用意し、以降 accept する接続はこちらを使う
    ///
    /// `RebindHandle::rebind` は差し替え完了（上記 3 の直後）を待って
    /// `Ok(new_addr)` を返す。新アドレスへの bind 自体が失敗した場合は
    /// `run_until` の状態に一切影響しない（fail-closed、
    /// `RebindHandle::rebind` の doc を参照）。設計判断の詳細は
    /// `docs/design/rebind.md` を参照。
    ///
    /// # 既知の限界
    ///
    /// 上記の 503 拒否は shutdown_flag 受信「後」に到着した Upgrade
    /// リクエストにのみ適用される。shutdown_flag 受信前に既に Upgrade へ
    /// 委譲済みの WebSocket 専用タスク（`fandhe_backend_plugin_websocket`
    /// 側の `tokio::spawn`）は本関数が管理する `JoinSet` の外にあるため、
    /// grace 超過時の `JoinSet` 強制 abort 対象にはならない。ただし
    /// in-flight 完了待ちは permit 回収のタイムアウトで実装されており、
    /// `run_until` 自体は grace + ε 以内に必ず戻る（既存フェイルセーフ、
    /// イシュー #491 でも置き換えていない）。
    ///
    /// イシュー #491 で、`current_shutdown_flag.store(true, ...)` の直後
    /// （下記実装を参照）に世代キャンセル（`crate::plugin::
    /// GenerationCancel::fire`）を発火し、委譲済みの WS 専用タスクへ
    /// 明示的なキャンセルシグナルを伝播する経路を追加した。#492 で
    /// `fandhe_backend_plugin_websocket::handle_upgrade` が正常な Close
    /// ハンドシェイク（close code 1001 Going Away を送出し、
    /// `WebSocketConfig::close_grace`（既定 10 秒）上限でクライアント応答を
    /// 待つ）を実装済みで、キャンセル発火後は
    /// ハードクローズではなくこの正常終端シーケンスを経由する。Close に
    /// 応答しないクライアントは `WebSocketConfig::close_grace`（既定 10 秒）
    /// 有界で終端し、detached
    /// タスクとして残ることがあるが `run_until` 自体の「grace + ε 以内に
    /// 必ず戻る」フェイルセーフ（上記 permit 回収タイムアウト）には影響
    /// しない（`crate::plugin::try_handle_upgrade` の doc「世代キャンセル
    /// シグナル」・`docs/design/ws-cancellation-propagation.md`・統合テスト
    /// `crates/core/tests/ws_cancellation.rs`（イシュー #493）を参照）。
    pub async fn run_until<F>(self, shutdown: F) -> io::Result<()>
    where
        F: Future<Output = ()>,
    {
        let BoundServer {
            mut listener,
            server,
            connection_limit,
            permit_total,
            shutdown_flag,
            // `rebind_tx`（送信側）は使わない: 送信側は `RebindHandle` 経由で
            // 呼び出し元が保持しており、`run_until` 側は受信側だけを消費する。
            rebind_tx: _,
            mut rebind_rx,
        } = self;

        let mut shutdown = std::pin::pin!(shutdown);
        // `CancelSafeJoinSet` でラップし、`run_until` の Future 自体が外部
        // キャンセルされた場合に in-flight 接続が abort されるのを防ぐ
        // （`CancelSafeJoinSet` の doc・Bugbot 指摘 review comment
        // 3615287445 を参照）。
        let mut join_set = CancelSafeJoinSet(JoinSet::new());
        // 「現行世代」の shutdown フラグ。`rebind()`（イシュー #485）で
        // listener を差し替えるたびに、旧フラグを `true` にして新フラグへ
        // 差し替える（`race_shutdown_or_accept` の `Raced::Rebind` 分岐を参照）。
        let mut current_shutdown_flag = shutdown_flag;
        // 「現行世代」のキャンセル発火源（イシュー #491）。`shutdown_flag` と
        // 同じタイミングで世代交代する（`Raced::Rebind` 分岐・最終 shutdown
        // 直後を参照。`docs/design/ws-cancellation-propagation.md` 5.1 節
        // 「両経路が同一の世代構造体を発火源として共有する」）。
        let mut current_cancel = crate::plugin::GenerationCancel::new();

        loop {
            // 完了済みタスクを反復のたびに全件回収する（1 件だけ回収すると
            // accept 待ちが続く間に完了タスクが溜まり続けるため、`while` で
            // 尽くす。ポーリング自体は非ブロッキングでコストは小さい）。
            while join_set.try_join_next().is_some() {}

            // セマフォが閉じられることはない（`close()` を呼ぶ経路がない）ため
            // `acquire_owned` は必ず成功する。accept 側の Future は
            // 「permit 取得 → accept」を 1 つの Future にまとめる。shutdown・
            // rebind のどちらかが先に完了してこの Future が drop されても、
            // 取得済み permit は自動解放されるだけで接続を取りこぼさない
            // （cancel-safe）。
            let connection_limit_for_accept = Arc::clone(&connection_limit);
            let accept_fut = async {
                let permit = connection_limit_for_accept
                    .acquire_owned()
                    .await
                    .expect("connection_limit semaphore is never closed");
                match listener.accept().await {
                    // `peer_addr` はイシュー #486 で `GateContext` 経由
                    // `RequestGate::check` へ伝搬する（下記 spawn 先を参照）。
                    // 以前は破棄していた値。
                    Ok((stream, peer_addr)) => Some((stream, peer_addr, permit)),
                    Err(err) => {
                        // permit はここで（スコープを抜けると同時に）解放され、
                        // 次のループ先頭で再取得される。`run_until` の doc を参照。
                        drop(permit);
                        eprintln!("fandhe_backend_core::server: accept に失敗しました: {err}");
                        tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                        None
                    }
                }
            };

            match race_shutdown_or_accept(shutdown.as_mut(), rebind_rx.as_mut(), accept_fut).await {
                Raced::Shutdown => {
                    // shutdown 確定時点で rebind チャネルを即座に閉じる
                    // （grace drain 開始前。Bugbot 指摘対応、イシュー #485）。
                    // 閉じずに握ったままだと、以降の `RebindHandle::rebind` 呼び出しが
                    // `send`/`reply_rx` で最大 `shutdown_grace_period` までブロックし、
                    // 呼び出し側が bind 済みの新 `TcpListener` もチャネルバッファに
                    // 滞留したままポートを保持し続けてしまう。ここで
                    // `rebind_rx` を drop してチャネルを閉じることで、(a) 以後の
                    // `rebind()` の `send` は即座に失敗し fail-fast で `Err` を返す
                    // （既存の「run_until 終了済み」契約に合流）、(b) 送信済みで
                    // reply 待ちのコマンドも `reply_rx` が即クローズされてブロック
                    // が解消し、コマンドが保持する新 listener も直ちに drop されて
                    // ポートが解放される。
                    drop(rebind_rx.take());
                    break;
                }
                Raced::Rebind(RebindCommand {
                    listener: new_listener,
                    reply,
                }) => {
                    // 世代別 drain（`run_until` の doc「稼働中の再バインド」・
                    // `docs/design/rebind.md` を参照）。
                    // 1. 旧世代フラグを立て、keep-alive 接続を早期クローズ側へ倒す。
                    current_shutdown_flag.store(true, Ordering::Relaxed);
                    // 1.5. 旧 listener を drop する前に accept backlog を有界
                    // drain し、旧世代接続としてサーブする（イシュー #501、
                    // `drain_listener_backlog` の doc を参照）。旧フラグは
                    // 直前で既に true にしたため、各接続は 1 リクエスト
                    // 処理後に `Connection: close` で閉じ、Upgrade は 503 で
                    // 拒否される（`handle_connection_with_permit` の既存
                    // shutdown_flag 契約にそのまま合流する）。
                    for (stream, peer_addr, permit) in drain_listener_backlog(
                        &listener,
                        &connection_limit,
                        REBIND_BACKLOG_DRAIN_LIMIT,
                    )
                    .await
                    {
                        let server = Arc::clone(&server);
                        let shutdown_flag = Arc::clone(&current_shutdown_flag);
                        let cancel = current_cancel.handle();
                        join_set.spawn(async move {
                            handle_connection_with_permit(
                                &server,
                                stream,
                                Some(permit),
                                &shutdown_flag,
                                Some(peer_addr),
                                cancel,
                            )
                            .await;
                        });
                    }
                    // 2. listener を差し替える。以降の accept は新アドレスのみ。
                    listener = new_listener;
                    // 3. 旧世代の JoinSet を切り離し、grace 付きで背景 drain する
                    // （`run_until` 自体はブロックしない）。旧世代のキャンセル
                    // 発火源（イシュー #491）も同時に切り離し、drain タスク
                    // 冒頭での発火（`spawn_generation_drain` の doc を参照）へ
                    // 委ねる。
                    let old_join_set =
                        std::mem::replace(&mut join_set, CancelSafeJoinSet(JoinSet::new()));
                    let old_cancel = std::mem::replace(
                        &mut current_cancel,
                        crate::plugin::GenerationCancel::new(),
                    );
                    spawn_generation_drain(old_join_set, old_cancel, server.shutdown_grace_period);
                    // 4. 新世代用のフラグを用意する。
                    current_shutdown_flag = Arc::new(AtomicBool::new(false));
                    // 差し替え完了を通知する。呼び出し側（`RebindHandle::rebind`）が
                    // 既に `reply_rx` を drop していても（呼び出し元タスクが
                    // キャンセルされた等）送信失敗は無視してよい
                    // （通知を受け取る相手がいないだけで、差し替え自体は完了済み）。
                    let _ = reply.send(());
                    continue;
                }
                Raced::Completed(None) => continue,
                Raced::Completed(Some((stream, peer_addr, permit))) => {
                    let server = Arc::clone(&server);
                    let shutdown_flag = Arc::clone(&current_shutdown_flag);
                    // この接続が属する世代のキャンセルハンドル（イシュー
                    // #491）。WS 委譲が確定した場合のみ
                    // `crate::plugin::try_handle_upgrade` 内で消費される。
                    let cancel = current_cancel.handle();
                    join_set.spawn(async move {
                        // permit は WebSocket 委譲時に `handle_connection_with_permit`
                        // 内部で専用タスクへ move されうる（TASK-4.2 / #23、
                        // `crate::plugin::try_handle_upgrade` の doc「permit の契約」
                        // を参照）。move された場合ここでの `drop` は無 op（`None`）
                        // であり、二重解放や早期解放は起きない。
                        handle_connection_with_permit(
                            &server,
                            stream,
                            Some(permit),
                            &shutdown_flag,
                            Some(peer_addr),
                            cancel,
                        )
                        .await;
                    });
                }
            }
        }

        // 1. accept 停止: 以降の keep-alive 判定を早期クローズ側へ倒し、
        // リスニングソケットを閉じて新規接続を OS レベルで拒否する。
        // `current_shutdown_flag` は「現行世代」（rebind 済みなら差し替え後の
        // もの）を指す。旧世代のフラグは rebind の時点で既に `true` 済み。
        current_shutdown_flag.store(true, Ordering::Relaxed);
        // 現行世代のキャンセルを発火する（イシュー #491、設計 5.3 節
        // 「shutdown_flag を true にする直後に watch::Sender::send(true) を
        // 追加するだけ」）。`run_until` 自体の制御フローには分岐が増えず、
        // grace 待ち・強制クローズの実装は不変（下記コメント参照）。
        current_cancel.fire();
        drop(listener);

        // 2. in-flight 完了待ち（grace 上限）。
        let drain_result = tokio::time::timeout(
            server.shutdown_grace_period,
            Arc::clone(&connection_limit).acquire_many_owned(permit_total),
        )
        .await;

        match drain_result {
            Ok(Ok(_permits)) => {
                // 全接続が完了済み。spawn 済みタスクの JoinHandle を最後まで
                // drain する（`join_set.spawn` はタスクを既に起動しており、
                // ここでの `join_next` は完了確認のみで新規処理は発生しない）。
                while join_set.join_next().await.is_some() {}
            }
            _ => {
                // grace 超過、またはセマフォ側の異常（`close()` 経路がなく
                // 通常発生しない）。いずれもハング防止のため強制クローズへ
                // 倒す（フェイルクローズ、受け入れ条件「上限時間・超過時
                // 強制クローズ」）。
                eprintln!(
                    "fandhe_backend_core::server: graceful shutdown の猶予期間（{:?}）を超過したため残存接続を強制クローズします",
                    server.shutdown_grace_period
                );
                join_set.shutdown().await;
            }
        }

        Ok(())
    }
}

/// 1 コネクション分の keep-alive ループ本体。
///
/// `S` を [`AsyncRead`] + [`AsyncWrite`] にジェネリック化しているのは、
/// 実ソケット（[`TcpStream`]）だけでなく `tokio::io::duplex` を使った
/// ソケット不要の統合テストを可能にするため（AI ファースト保守性、
/// `.claude/rules/coding-rust.md`）。
///
/// 接続単位で読み取りバッファ `buf` を 1 本だけ確保し、`fandhe_backend_http::connection`
/// のパイプライン契約（未消費の残余バイトを `buf` に残す）に従って
/// 繰り返し `read_request` を呼ぶ。
///
/// 本関数の中に `#[cfg(feature = "...")]` を一切持たない（本モジュール冒頭の
/// doc を参照）。
///
/// 公開 API としては `permit` を持たない薄いラッパー
/// （`handle_connection_with_permit`（`pub(crate)`）に `None` を渡すだけ）を
/// 維持し、既存の呼び出し元・テスト（`tokio::io::duplex` を使う統合テスト等）
/// との互換性を保つ。実接続（[`BoundServer::run_until`]）は `permit` を伴う
/// 内部版を直接呼ぶ（本モジュール内の該当関数 doc を参照）。graceful
/// shutdown（イシュー #313）のシグナルも `BoundServer::run_until` 経由でしか
/// 発生しないため、本関数は「シャットダウンなし」（常に `false`）のフラグを
/// 内部で用意して渡す。
///
/// `tokio::io::duplex` 等の非ソケット経路であるため、`RequestGate::check` へ
/// 渡す [`GateContext::peer_addr`] は常に `None` になる（イシュー #486）。
/// 実 peer address を注入したい呼び出し元は [`handle_connection_with_peer_addr`]
/// を使う。
pub async fn handle_connection<S>(server: &Server, stream: S)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // `BoundServer::run_until` を経由しない呼び出し（直接統合テスト等）は
    // シャットダウン対象にならないため、常に `false` のローカルフラグを渡す。
    let no_shutdown = Arc::new(AtomicBool::new(false));
    // 世代の概念がない直接呼び出し経路のため、発火しないキャンセルハンドル
    // を渡す（イシュー #491、`crate::plugin::UpgradeCancel` の doc を参照）。
    handle_connection_with_permit(
        server,
        stream,
        None,
        &no_shutdown,
        None,
        crate::plugin::UpgradeCancel::disabled(),
    )
    .await;
}

/// [`handle_connection`] に実 peer address を注入できる版（イシュー #486）。
///
/// カスタム accept ループ・`tokio::io::duplex` 以外の非標準ソケット経路から
/// [`RequestGate::check`] へ実 peer address を伝搬させたい呼び出し元向けの
/// 公開 API。`BoundServer::run_until` が内部で呼ぶ経路（実 TCP accept）とは
/// 独立しており、両者は同じ `handle_connection_with_permit` に収斂する。
///
/// # Examples
///
/// ```no_run
/// # async fn example() -> std::io::Result<()> {
/// use fandhe_backend_core::{Server, handle_connection_with_peer_addr};
/// use tokio::net::TcpListener;
///
/// let server = Server::new();
/// let listener = TcpListener::bind("127.0.0.1:0").await?;
/// let (stream, peer_addr) = listener.accept().await?;
/// handle_connection_with_peer_addr(&server, stream, peer_addr).await;
/// # Ok(())
/// # }
/// ```
pub async fn handle_connection_with_peer_addr<S>(
    server: &Server,
    stream: S,
    peer_addr: std::net::SocketAddr,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let no_shutdown = Arc::new(AtomicBool::new(false));
    // 世代の概念がない直接呼び出し経路のため、発火しないキャンセルハンドル
    // を渡す（イシュー #491、`crate::plugin::UpgradeCancel` の doc を参照）。
    handle_connection_with_permit(
        server,
        stream,
        None,
        &no_shutdown,
        Some(peer_addr),
        crate::plugin::UpgradeCancel::disabled(),
    )
    .await;
}

/// [`handle_connection`] の内部実装（`pub(crate)`、TASK-4.2 / #23）。
///
/// `permit` は [`BoundServer::run_until`] が保持する同時接続数上限の
/// `OwnedSemaphorePermit`（`.claude/rules/security.md` のリソース枯渇 DoS
/// 対策）。関数が戻る時点で `permit`（ローカル所有の `Option`）はスコープを
/// 抜けて自動的に drop され、`Some` なら通常どおり解放される。WebSocket
/// への委譲が確定した場合は `crate::plugin::try_handle_upgrade` が
/// `permit.take()` で所有権をセッション専用タスクへ move し、この関数側の
/// `permit` は `None` のまま戻る（drop は no-op）。これにより、長時間生存
/// する WS セッションも `max_connections` のカウントから漏れない
/// （`crate::plugin::try_handle_upgrade` の doc「permit の契約」を参照）。
///
/// `shutdown_flag` は [`BoundServer::run_until`] の graceful shutdown
/// シグナル（イシュー #313）を伝える。`true` の間は keep-alive 判定を
/// 早期クローズ側へ倒すが、**処理中のリクエストへの応答は必ず完走させる**
/// （このフラグはループ先頭・次リクエストへ進むかどうかの判定にのみ関与し、
/// 現在処理中のリクエストを中断させることはない）。
///
/// `peer_addr` は accept したソケットの実 peer address（イシュー #486）。
/// 接続の生存期間中は不変のため、ループ先頭で 1 回だけ [`GateContext`] を
/// 構築し（`Copy` 型のためコピーコストは無視できる）、以降の
/// `RequestGate::check` 呼び出しへ都度渡す。
///
/// `cancel`（[`crate::plugin::UpgradeCancel`]、イシュー #491）は接続が
/// 属する世代のキャンセルハンドル。Upgrade 委譲が確定した場合のみ
/// `crate::plugin::try_handle_upgrade` へそのまま渡す（それ以外の分岐では
/// 使われず drop される。世代キャンセルは WS 委譲タスクのみが対象、
/// `docs/design/ws-cancellation-propagation.md` を参照）。
pub(crate) async fn handle_connection_with_permit<S>(
    server: &Server,
    mut stream: S,
    mut permit: Option<OwnedSemaphorePermit>,
    shutdown_flag: &Arc<AtomicBool>,
    peer_addr: Option<std::net::SocketAddr>,
    cancel: crate::plugin::UpgradeCancel,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let gate_ctx = GateContext::new(peer_addr);
    let mut buf = RecvBuffer::new();
    // 接続の総生存期間・keep-alive 中の総リクエスト数を計測する（#70 レビュー
    // 指摘、`.claude/rules/security.md` のリソース枯渇観点。DEFAULT_READ_TIMEOUT の
    // doc・Server::max_connection_lifetime / max_requests_per_connection の
    // doc を参照）。
    let connection_started_at = Instant::now();
    let mut requests_served: usize = 0;
    // 0 を指定しても最低 1 リクエストは処理してから閉じる
    // （Server::max_requests_per_connection の doc を参照）。
    let max_requests = server.max_requests_per_connection.max(1);

    loop {
        // 次のリクエストを読みに行く前に総生存期間の上限を確認する。これにより
        // read_timeout より短い間隔で送信し続けるクライアントであっても、
        // 接続が上限を超えて permit を占有し続けることはない。
        let elapsed_since_start = connection_started_at.elapsed();
        if elapsed_since_start >= server.max_connection_lifetime {
            return;
        }

        // read_request のタイムアウトは server.read_timeout（既定
        // DEFAULT_READ_TIMEOUT、Server::read_timeout でチューニング可能）と
        // 「残り生存期間」の短い方に丸める（#70 Bugbot 指摘: read_timeout を
        // そのまま使うと、直前の生存期間チェックを通過した直後に読み取りが
        // 最大 read_timeout だけブロックし、permit を握ったまま接続が
        // max_connection_lifetime を超過しうる）。これにより 1 回の read
        // 待ちで接続が生存期間上限を超えて居座ることはなく、超過前に必ず
        // タイムアウトして接続が閉じる（下の `Err(_elapsed) => return` 分岐）。
        let remaining_lifetime = server
            .max_connection_lifetime
            .saturating_sub(elapsed_since_start);
        let read_timeout = server.read_timeout.min(remaining_lifetime);

        let read_result = tokio::time::timeout(
            read_timeout,
            read_request_with_limit(&mut stream, &mut buf, server.max_body_bytes),
        )
        .await;

        let request = match read_result {
            // タイムアウト（スロークライアント・keep-alive アイドル超過）:
            // 応答を送らず接続を閉じる。
            Err(_elapsed) => return,
            Ok(Err(err)) => {
                if let Some(response) = error_response(&err) {
                    // エラー応答は常に接続クローズ（フェイルセーフ）。
                    let _ = stream.write_all(&response.serialize(false)).await;
                }
                return;
            }
            // buf が空の状態での EOF は正常なコネクション終了。
            Ok(Ok(None)) => return,
            Ok(Ok(Some(request))) => request,
        };

        requests_served += 1;

        let started_at = Instant::now();
        for middleware in &server.middlewares {
            middleware.on_request(&request.head);
        }

        // クライアントが keep-alive を要求していても、`Server::keep_alive(false)`
        // で無効化されている場合・総リクエスト数上限に達した場合・総生存期間
        // 上限に達した場合は `Connection: close` で応答し、この接続では次の
        // リクエストを待たない（#70 レビュー指摘、Server::keep_alive /
        // max_requests_per_connection / max_connection_lifetime の doc を参照）。
        // graceful shutdown（イシュー #313）: シャットダウンシグナル受信後は
        // 処理中のリクエストを完走させつつ、応答へ `Connection: close` を
        // 付けてこの接続を閉じる（`handle_connection_with_permit` の doc・
        // `BoundServer::run_until` の doc「accept 停止」を参照）。
        let keep_alive = server.keep_alive_enabled
            && should_keep_alive(&request.head)
            && requests_served < max_requests
            && connection_started_at.elapsed() < server.max_connection_lifetime
            && !shutdown_flag.load(Ordering::Relaxed);

        // RequestGate はルーティング・アップグレードより先に評価する
        // （フェイルクローズ、モジュール冒頭の doc を参照）。
        if let Some(rejection) = first_rejection(&server.gates, &request.head, &gate_ctx) {
            let GateOutcome::Reject { response } = rejection else {
                unreachable!("first_rejection only returns Reject outcomes")
            };
            // ゲート実装が組み立てた検証済み `Response` をそのまま送出する
            // （イシュー #424）。`Content-Length` / `Connection` は
            // `serialize(keep_alive)` がフレーミング管理の一元責務として
            // 上書き決定するため、ゲート側の値を尊重する必要はない
            // （`Response::with_header` が両ヘッダ名を予約名として拒否済み）。
            if stream
                .write_all(&response.serialize(keep_alive))
                .await
                .is_err()
            {
                return;
            }
            for middleware in &server.middlewares {
                middleware.on_response(&request.head, started_at.elapsed());
            }
            if !keep_alive {
                return;
            }
            continue;
        }

        if server
            .upgrade_handlers
            .iter()
            .any(|handler| handler.matches(&request.head))
        {
            // graceful shutdown（イシュー #313）: shutdown_flag は元々 HTTP
            // の keep-alive 判定にしか影響せず、Upgrade 分岐はこれを一切
            // 参照していなかった（Bugbot 指摘 review comment 3615144815）。
            // shutdown 後に Upgrade を許すと、その permit は
            // `crate::plugin::try_handle_upgrade` 内で JoinSet 外の
            // detached セッションタスクへ move され、grace force-close
            // （`BoundServer::run_until` の doc「上限超過時は強制クローズ」）
            // を過ぎても動き続けうる。shutdown 後の新規 Upgrade は 503 で
            // 明示的に拒否し、`Connection: close` で確実に閉じる。
            if shutdown_flag.load(Ordering::Relaxed) {
                let _ = stream
                    .write_all(&Response::empty(503).serialize(false))
                    .await;
                return;
            }

            // 長時間接続へ委譲する前に、パイプライン済みの可能性がある残余
            // バイト列（次リクエストの先頭・WebSocket の先行フレーム等）を
            // 退避してからコア側の読み取りバッファを明示的に解放する
            // （Conditional Go 条件 (1)）。`RecvBuffer` は縮小 API を
            // `pub(crate)` にしか公開しない（TASK-1.3-3 / #68）ため、`drop`
            // で旧バッファ（確保済み容量ごと）を丸ごと解放する。以降このループ
            // 反復では `buf` を読まない（両分岐とも `return` する）ため、
            // 代入ではなく明示的な `drop` で意図を示す。`leftover` は
            // `fandhe_backend_plugin_websocket::handle_upgrade` が
            // `WebSocketStream::from_partially_read` へそのまま渡す
            // （TASK-4.1 / #22、先行到着フレームを取りこぼさないため）。
            let leftover = buf.unread().to_vec();
            drop(buf);
            match crate::plugin::try_handle_upgrade(
                stream,
                &request.head,
                leftover,
                server,
                &mut permit,
                cancel,
            )
            .await
            {
                Some(mut stream) => {
                    // websocket feature 無効時、または `UpgradeHandler` が
                    // マッチしたのに対応する Upgrade 型プラグインが未登録の
                    // 場合、委譲先がない状態を黙って落とさず 501 で明示的に
                    // 拒否する（本モジュール冒頭の doc・
                    // `crate::plugin::try_handle_upgrade` の doc を参照）。
                    // on_response は「委譲時は呼ばない」契約のため呼ばない
                    // （この 501 応答は委譲失敗のフォールバックであり実処理の
                    // 完了ではないため）。結果として on_request は呼ばれるが
                    // 対になる on_response が呼ばれない非対称が生じる点は
                    // 意図的な仕様であり、Middleware 実装側は「on_request が
                    // 必ず on_response を伴う」と仮定しないこと。
                    let _ = stream
                        .write_all(&Response::empty(501).serialize(false))
                        .await;
                    return;
                }
                None => return,
            }
        }

        // ユーザー向けインターセプト拡張点（`Interceptor::intercept`、イシュー
        // #420）は `plugin::try_intercept` より先に評価する。利用者が
        // 登録済みプラグイン（plugin-static 等）の応答を先取りできるように
        // するためで、登録順に評価し最初の `Some` が勝つ（`crate::interceptor`
        // モジュール doc の評価順序を参照）。
        let intercepted = server
            .interceptors
            .iter()
            .find_map(|interceptor| interceptor.intercept(&request.head, &request.body));

        // パスインターセプト型プラグイン（TASK-2.1 / #18）は既定 Handler より
        // 先に評価する。`try_intercept` が `Some` を返した場合はプラグインが
        // 処理を完結させたことを意味し、既定 Handler は呼ばない
        // （モジュール冒頭の処理フロー doc・`crate::plugin::try_intercept` の
        // doc を参照）。ユーザー `Interceptor::intercept` が既に確定させた
        // 場合はここをスキップする。
        let intercepted = match intercepted {
            Some(response) => Some(response),
            None => crate::plugin::try_intercept(server, &request.head, &request.body).await,
        };

        // レスポンス側 chunked ストリーミング送信（イシュー #319）: `try_intercept` が
        // 委譲しなかった場合のみ、既定 `Handler` の opt-in 拡張点
        // `Handler::handle_streaming` を確認する。`Some` を返した場合はこの
        // ループ反復の残り（レスポンス書き込み・`on_response` 呼び出し・
        // keep-alive 判定）を `write_streaming_response` に委ね、下の通常
        // 一括応答経路（`handler.handle`・`finalize_response`・
        // `response.serialize`）は使わない。`Interceptor::map_response`
        // （イシュー #420）は `write_streaming_response` 内でヘッド確定時に
        // 適用する（イシュー #434、`crate::interceptor` モジュール doc の
        // 「ストリーミング応答への適用」節を参照）。続けて
        // `crate::plugin::finalize_streaming_head`（イシュー #451、
        // `finalize_response` の第 4 のシーム）で CORS ヘッダ付与のみを
        // 適用する。`finalize_response` 本体（CORS → 圧縮の逐次適用）は
        // `Response` 型を前提とする通常応答経路専用のまま据え置き、圧縮は
        // ストリーミング応答へ引き続き未適用とする（body 全体のバッファ
        // リングが必要になり #319 の設計と矛盾するため意図的に対象外、
        // `crate::plugin::finalize_streaming_head` の doc・
        // `docs/design/plugin-boundary.md` 5.9.7 節を参照）。
        if intercepted.is_none()
            && let Some(handler) = &server.handler
            && let Some(streaming) = handler.handle_streaming(&request.head, &request.body)
        {
            let keep_alive_after = write_streaming_response(
                &mut stream,
                streaming,
                &request.head,
                keep_alive,
                server,
                connection_started_at,
                shutdown_flag,
            )
            .await;
            // `on_response` は「応答が完走した場合にのみ呼ぶ」契約に統一する
            // （通常応答経路の write_all 失敗時に on_response を呼ばずに
            // return するのと同一の契約）。`write_streaming_response` の
            // `None`（タイムアウト・書き込みエラー・producer 打ち切り）は
            // 応答未完走であり、plugin-tracing 等の Middleware が
            // 「on_response は完了した応答を表す」と仮定して観測している
            // 場合にタイムアウト・打ち切りを成功応答としてカウントしない
            // ようにする（レビュー指摘、`crate::streaming` モジュール doc の
            // 「応答完全性」節と同じ fail-closed 方針）。
            match keep_alive_after {
                Some(keep_alive_after) => {
                    for middleware in &server.middlewares {
                        middleware.on_response(&request.head, started_at.elapsed());
                    }
                    if keep_alive_after {
                        continue;
                    }
                    return;
                }
                None => return,
            }
        }

        let response = match intercepted {
            Some(response) => response,
            None => match &server.handler {
                Some(handler) => handler.handle(&request.head, &request.body).await,
                None => Response::empty(404),
            },
        };

        // ユーザー向けレスポンス改変拡張点（`Interceptor::map_response`、
        // イシュー #420）は `finalize_response`（CORS → 圧縮）より前に適用する。
        // CORS ヘッダ付与・gzip 圧縮は改変後の最終 body に対して行われるべき
        // ため（`crate::interceptor` モジュール doc を参照）。登録順に逐次
        // 適用し、`RequestGate` 拒否応答・パースエラー応答（本関数内の別の
        // 送出経路）は意図的に通さない（fail-closed、`finalize_response` と
        // 同一の設計判断）。
        let response = server
            .interceptors
            .iter()
            .fold(response, |acc, interceptor| {
                interceptor.map_response(&request.head, acc)
            });

        // レスポンス後処理型シーム（イシュー #305、CORS プラグイン）。
        // `try_intercept` 応答・既定 `Handler` 応答のどちらが確定した場合でも
        // 同一の後処理を適用する（`crate::plugin::finalize_response` の doc を
        // 参照）。`RequestGate` 拒否応答・パースエラー応答（本関数内の別の
        // 送出経路）は意図的に通さない。
        let response = crate::plugin::finalize_response(server, &request.head, response).await;

        // #70 Bugbot 指摘（Stale keep-alive after lifetime）: 上の `keep_alive` は
        // `Handler::handle` 呼び出し前、`on_request` 直後の経過時間で決定している。
        // `handle` の処理時間が長引き、その間に `max_connection_lifetime` を
        // 超過した場合、`keep_alive` が古いまま `true` の応答を送ると
        // 「生存期間超過時は応答で `Connection: close` を返す」という契約
        // （上のコメント・`Server::max_connection_lifetime` の doc を参照）に
        // 反する。レスポンス生成直後・送信直前に生存期間のみ再チェックし、
        // 超過していれば `Connection: close` を確実に付与する
        // （`should_keep_alive` と `requests_served` 側の判定は `handle` の
        // 呼び出しで変化しないため再評価不要）。
        //
        // 同じ理由で `shutdown_flag` も再チェックする（Bugbot 指摘 review
        // comment 3615144800）: 最初の `keep_alive` 算出（`on_request` 直後）
        // より後、`try_intercept` / `Handler::handle` の非同期処理中に
        // shutdown が入ると、初回算出時点の `false` のまま応答してしまい
        // keep-alive を広告し続ける。送信直前に再読み込みして
        // `Connection: close` へ確実に倒す。
        let keep_alive = keep_alive
            && connection_started_at.elapsed() < server.max_connection_lifetime
            && !shutdown_flag.load(Ordering::Relaxed);

        if stream
            .write_all(&response.serialize(keep_alive))
            .await
            .is_err()
        {
            return;
        }
        for middleware in &server.middlewares {
            middleware.on_response(&request.head, started_at.elapsed());
        }

        if !keep_alive {
            return;
        }
    }
}

/// [`Handler::handle_streaming`]（イシュー #319）が `Some` を返した場合の
/// レスポンス書き込み専用経路。`handle_connection_with_permit` の通常応答
/// 書き込み（`response.serialize(keep_alive)` を 1 回 `write_all`）とは
/// 異なり、producer タスクが [`crate::streaming::BodyWriter`] 経由で送る
/// チャンクを逐次ソケットへ書き出す。
///
/// # `Interceptor::map_response` の適用（イシュー #434）
///
/// HTTP バージョン分岐前、ヘッド確定時に `server.interceptors` を登録順に
/// 適用する（[`crate::interceptor::Interceptor::map_response`]）。反映される
/// のは `status`・`Content-Type`・追加ヘッダのみで、mapped `Response` の
/// body は反映不能なため明示的に破棄する。以降の HTTP バージョン別ヘッド
/// 直列化・[`Response::is_bodyless_status`] 判定はすべて mapped 後の
/// `head_response` を参照する（`crate::interceptor` モジュール doc の
/// 「ストリーミング応答への適用」節を参照）。
///
/// # HTTP バージョン別の framing 選択
///
/// - HTTP/1.1: `crate::plugin::prepare_streaming_compression`（イシュー
///   #461、`finalize_streaming_head` の次段）でチャンク単位のストリーミング
///   gzip 圧縮を確定させたのち、[`Response::serialize_chunked_head`] で
///   `Transfer-Encoding: chunked` ヘッドを送り、以降 producer から届く
///   チャンクを `StreamingBodyEncoder`（圧縮確定時のみ変換、未確定時は
///   identity）に通してから [`encode_chunk`] / [`encode_terminator`] で
///   フレーミングして書き出す。
/// - HTTP/1.0: chunked を理解しない前提のクライアントへ配慮し
///   （`Response::serialize_streaming_head_http10` の doc を参照）、
///   フレーミングなしの生データを EOF（本関数の戻りが呼び出し元の
///   `return` を誘発し接続がクローズされること）で終端する。keep-alive は
///   常に無効。
///
/// # タイムアウト・生存期間
///
/// producer からの次チャンク待ち（[`StreamingResponse::recv`]）・実際の
/// `write_all` の両方に、[`DEFAULT_WRITE_TIMEOUT`]（`Server::write_timeout`
/// のようなチューニング API は本イシューのスコープ外）と「残り生存期間」の
/// 短い方を適用する（`handle_connection_with_permit` の read_timeout 丸め
/// パターンと同一の考え方）。タイムアウト・書き込みエラーは即座に接続を
/// クローズする（`None` を返す）。
///
/// # 戻り値
///
/// - `Some(true)`: 正常終端（[`crate::streaming::BodyWriter::finish`]）し、
///   かつ呼び出し元の keep-alive 判定・生存期間・shutdown 状態のいずれも
///   継続を許す場合。呼び出し元はこの接続で次のリクエストを読みに行ってよい
/// - `Some(false)`: 正常終端したが keep-alive を継続しない場合
///   （`Connection: close` を広告済み、または完了後に生存期間超過・
///   shutdown を検知した場合。プラン「完了後に max_connection_lifetime を
///   再チェックし、超過時はヘッダが keep-alive でも接続を閉じる」を実装）
/// - `None`: タイムアウト・書き込みエラー・producer が `finish` を呼ばずに
///   drop された（打ち切り）場合。呼び出し元は追加の応答を送らず接続を
///   即座にクローズする（応答完全性の fail-closed、`crate::streaming` の
///   モジュール doc を参照）
async fn write_streaming_response<S>(
    stream: &mut S,
    mut streaming: StreamingResponse,
    head: &RequestHead,
    keep_alive: bool,
    server: &Server,
    connection_started_at: Instant,
    shutdown_flag: &Arc<AtomicBool>,
) -> Option<bool>
where
    S: AsyncWrite + Unpin,
{
    // 各書き込み・受信待ちに適用する残り生存期間を都度算出するヘルパー。
    // `handle_connection_with_permit` の read_timeout 丸めパターン（#70
    // Bugbot 指摘）と同一の考え方: 生存期間超過後は即座に 0 として扱う
    // （`saturating_sub` によりタイムアウトが即発火し、余計な待ちを生まない）。
    let remaining_lifetime = |started: Instant| -> Duration {
        server
            .max_connection_lifetime
            .saturating_sub(started.elapsed())
    };

    // ユーザー向けレスポンス改変拡張点（`Interceptor::map_response`、イシュー
    // #420）をストリーミング応答のヘッド確定時に 1 回だけ適用する（イシュー
    // #434）。通常応答経路（`handle_connection_with_permit` の
    // `interceptors.iter().fold` 呼び出し）と同一の登録順逐次適用パターン。
    // 続けてレスポンス後処理型シーム（`crate::plugin::
    // finalize_streaming_head`、イシュー #451。CORS ヘッダ付与のみ、圧縮は
    // 対象外）を適用する（下の `finalize_streaming_head` 呼び出しを参照）。
    //
    // 適用範囲はステータス・ヘッダのみ。ストリーミング応答の実体は producer
    // タスクが `BodyWriter` 経由で逐次供給し、chunked framing は本関数が
    // `crate::streaming::{encode_chunk, encode_terminator}` で直接組み立てる
    // ため（`Response::body` を経由しない）、`map_response` /
    // `finalize_streaming_head` が返した `Response` の body は反映不能かつ
    // 反映してはならない。反映を許すとバックプレッシャ（bounded mpsc）・
    // 応答完全性契約（`finish` 省略時は終端チャンクなしで打ち切りクローズ）
    // と両立できず、body 全体のバッファリングが必要になり #319 の設計と
    // 矛盾する（`crate::interceptor` モジュール doc・`crate::plugin::
    // finalize_streaming_head` の doc・`docs/design/
    // interceptor-extension-point.md` を参照）。`serialize_chunked_head` /
    // `serialize_streaming_head_http10` は `debug_assert!(self.body.is_empty())`
    // を持つため、直列化前に必ずクリアする。
    let mut head_response = Response::empty(streaming.status);
    if let Some(content_type) = streaming.content_type {
        head_response = head_response.with_content_type(content_type);
    }
    head_response = server
        .interceptors
        .iter()
        .fold(head_response, |acc, interceptor| {
            interceptor.map_response(head, acc)
        });

    // レスポンス後処理型シーム（`crate::plugin::finalize_streaming_head`、
    // イシュー #451）を `map_response` の直後・body クリアの前に適用する。
    // 通常応答経路の順序（`map_response` → `finalize_response`）と一致させ、
    // CORS ヘッダ付与は利用者インターセプタによる改変後の最終ヘッドに対して
    // 行う。CORS はステータス・body に触れないヘッダのみの後処理のため、
    // 直後の `is_bodyless_status` 判定・`debug_assert!(body.is_empty())`
    // 契約には影響しない（`finalize_streaming_head` の doc を参照）。
    // チャンク単位のストリーミング圧縮（`crate::plugin::
    // prepare_streaming_compression`、イシュー #461）は HTTP/1.1 chunked
    // 経路専用のため、ここではまだ適用せず HTTP バージョン分岐後に呼ぶ
    // （下の該当コメントを参照）。
    head_response = crate::plugin::finalize_streaming_head(server, head, head_response);
    head_response.body = Vec::new();

    if head.version == HttpVersion::Http10 {
        // HTTP/1.0: chunked framing を使わず、ヘッドは常に Connection: close。
        let head_bytes = head_response.serialize_streaming_head_http10();
        let write_timeout = DEFAULT_WRITE_TIMEOUT.min(remaining_lifetime(connection_started_at));
        if tokio::time::timeout(write_timeout, stream.write_all(&head_bytes))
            .await
            .ok()?
            .is_err()
        {
            return None;
        }

        // RFC 9112 §6.3: 1xx・204・304 は body を持ち得ないため、ハンドラが
        // これらのステータスを `handle_streaming` から返しても body 送出
        // ループへ入らずヘッド送出のみで応答を完了させる（レビュー指摘、
        // イシュー #319。`Response::is_bodyless_status` の doc を参照）。
        // HTTP/1.0 は本来 body を EOF 終端するため、ここで body を送出
        // しないことがそのまま「応答完了」を意味する（追加のフレーミング
        // ヘッダを持たないため誤終端の余地がない）。keep-alive は HTTP/1.0
        // では常に無効。応答は完走しているため `on_response` を呼ぶ
        // `Some` を返す（`None` は打ち切り・エラー専用の契約、上の doc の
        // 「戻り値」節を参照）。判定は `map_response` 適用後のステータス
        // （`head_response.status`）で行う（イシュー #434）。インターセプタが
        // 例えば 200 → 204 へ書き換えた場合、ここでボディ送出をスキップする
        // ことと「ヘッダ側で Transfer-Encoding を出力しない」ことが対で成立し、
        // ヘッダとボディ有無の不整合によるレスポンス分割類の脅威を防ぐ。
        if Response::is_bodyless_status(head_response.status) {
            return Some(false);
        }

        loop {
            let recv_timeout = DEFAULT_WRITE_TIMEOUT.min(remaining_lifetime(connection_started_at));
            let outcome = tokio::time::timeout(recv_timeout, streaming.recv())
                .await
                .ok()?;
            match outcome {
                RecvOutcome::Chunk(data) => {
                    if data.is_empty() {
                        continue;
                    }
                    let write_timeout =
                        DEFAULT_WRITE_TIMEOUT.min(remaining_lifetime(connection_started_at));
                    if tokio::time::timeout(write_timeout, stream.write_all(&data))
                        .await
                        .ok()?
                        .is_err()
                    {
                        return None;
                    }
                }
                // HTTP/1.0 は EOF（接続クローズ）で body を終端するため、
                // ソケット上の挙動は正常終端・打ち切りとも「これ以上書かず
                // 接続を閉じる」で同一になる。ただし戻り値は HTTP/1.1 経路と
                // 同じく分離する: 打ち切り（producer が `finish` を呼ばずに
                // drop）を `Some(false)` にすると呼び出し元が完了応答として
                // `Middleware::on_response` を呼んでしまい、「on_response は
                // 完走した応答にのみ対応する」契約（`crate::streaming` の
                // 応答完全性の節）に反する（レビュー指摘、イシュー #319）。
                RecvOutcome::End => return Some(false),
                RecvOutcome::Aborted => return None,
            }
        }
    }

    // HTTP/1.1: Transfer-Encoding: chunked。
    //
    // `keep_alive` 引数は `handle_connection_with_permit` が `on_request`
    // 直後（`try_intercept`・`Handler::handle_streaming` 呼び出し前）に算出
    // したスナップショットであり、その後の非同期処理中に生存期間超過・
    // shutdown が入っても反映されない（レビュー指摘）。通常応答経路の
    // 「送信直前に生存期間・shutdown_flag を再チェックする」（#70 Bugbot
    // 指摘、上の非 streaming 経路のコメントを参照）と同じ理由で、chunked
    // ヘッド（`Connection` ヘッダ）を確定する直前にここで再評価する。
    let keep_alive = keep_alive
        && connection_started_at.elapsed() < server.max_connection_lifetime
        && !shutdown_flag.load(Ordering::Relaxed);

    // 第 5 のシーム（`crate::plugin::prepare_streaming_compression`、イシュー
    // #461）を `finalize_streaming_head`（CORS）の直後・`serialize_chunked_head`
    // の前に適用する。HTTP/1.1 chunked 経路専用（`begin_streaming_compression`
    // の「呼び出し契約」節を参照）で、HTTP/1.0 分岐（上の early return）へは
    // 接続しない。`Content-Encoding: gzip` 確定時はここでヘッドへ付与し、
    // 以降の body 送出ループが `body_encoder` 経由でチャンクを圧縮変換する。
    let (head_response, mut body_encoder) =
        crate::plugin::prepare_streaming_compression(server, head, head_response);

    let head_bytes = head_response.serialize_chunked_head(keep_alive);
    let write_timeout = DEFAULT_WRITE_TIMEOUT.min(remaining_lifetime(connection_started_at));
    if tokio::time::timeout(write_timeout, stream.write_all(&head_bytes))
        .await
        .ok()?
        .is_err()
    {
        return None;
    }

    // RFC 9112 §6.3: 1xx・204・304 は body を持ち得ないため
    // `Transfer-Encoding: chunked` を出力しない（`serialize_chunked_head`
    // 側の抑制）のと対で、body 送出ループ・終端チャンク（`0\r\n\r\n`）の
    // 送出自体もスキップする（レビュー指摘、イシュー #319）。ヘッダのみ
    // 抑制して終端チャンクを送ると、strict なクライアントは空行直後で
    // 応答終了と解釈するため、続けて書いた終端チャンクのバイト列が次の
    // 応答の先頭と誤読されるレスポンス分割（キープアライブ接続上の
    // スマグリング）を招く。応答自体は完走しているため、通常の `End` と
    // 同じく `on_response` を発火させる `Some` を返す。
    if Response::is_bodyless_status(head_response.status) {
        return Some(
            keep_alive
                && connection_started_at.elapsed() < server.max_connection_lifetime
                && !shutdown_flag.load(Ordering::Relaxed),
        );
    }

    loop {
        let recv_timeout = DEFAULT_WRITE_TIMEOUT.min(remaining_lifetime(connection_started_at));
        let outcome = tokio::time::timeout(recv_timeout, streaming.recv())
            .await
            .ok()?;
        match outcome {
            RecvOutcome::Chunk(data) => {
                // `body_encoder`（イシュー #461）を chunked framing の前に
                // 適用する。identity（圧縮未確定）時は入力をそのまま返し、
                // 圧縮確定時は `StreamingGzipEncoder::encode_chunk` の sync
                // flush 済み出力を返す（`crate::plugin::StreamingBodyEncoder`
                // の doc を参照）。`None` はエンコーダ失敗を意味し、
                // `Content-Encoding: gzip` を広告した後にストリームを
                // identity へ切り替えると破壊になるため、書き込みエラーと
                // 同様に接続を打ち切る（fail-closed、`crates/plugin-
                // compression/src/lib.rs` の該当節を参照）。
                let data = body_encoder.transform(data)?;
                let mut framed = Vec::with_capacity(data.len() + 16);
                encode_chunk(&data, &mut framed);
                if framed.is_empty() {
                    // encode_chunk は空データを無出力にする契約（誤終端防止、
                    // `fandhe_backend_http::chunked::encode_chunk` の doc）。
                    // 書き込むものがなければソケットへは触れない。
                    continue;
                }
                let write_timeout =
                    DEFAULT_WRITE_TIMEOUT.min(remaining_lifetime(connection_started_at));
                if tokio::time::timeout(write_timeout, stream.write_all(&framed))
                    .await
                    .ok()?
                    .is_err()
                {
                    return None;
                }
            }
            RecvOutcome::End => {
                // `body_encoder` の残余データ（圧縮確定時は gzip trailer を
                // 含む）を最後のチャンクとして終端チャンクの直前に送出する。
                // identity 時は常に空（`StreamingBodyEncoder::finish` の doc
                // を参照）で、この場合は残余チャンクを一切書かず既存の
                // 「終端チャンクのみ」の挙動を保つ。`None` はエンコーダ失敗
                // （fail-closed、上の `Chunk` 分岐と同じ契約）。
                let residual = body_encoder.finish()?;
                let mut terminator = Vec::new();
                if !residual.is_empty() {
                    encode_chunk(&residual, &mut terminator);
                }
                encode_terminator(&mut terminator);
                let write_timeout =
                    DEFAULT_WRITE_TIMEOUT.min(remaining_lifetime(connection_started_at));
                if tokio::time::timeout(write_timeout, stream.write_all(&terminator))
                    .await
                    .ok()?
                    .is_err()
                {
                    return None;
                }
                // プラン「完了後に max_connection_lifetime を再チェックし、
                // 超過時はヘッダが keep-alive でも接続を閉じる」を実装する。
                // `shutdown_flag` も同じ理由で再チェックする（レビュー指摘）:
                // 上の `keep_alive` 再評価はヘッド送出「直前」時点のもので
                // あり、ストリーム本体の送信中（producer からの chunk 待ち・
                // 各 `write_all`）に新規 shutdown が入ってもここまで反映
                // されない。ヘッダは送出済みのため `Connection: close` を
                // 今から追加送信することはできないが（keep-alive を広告した
                // 後にサーバ都合で閉じる非対称は構造上避けられない、上の
                // `keep_alive` 再評価コメントを参照）、少なくとも「この接続で
                // 次のリクエストを読みに行くかどうか」の判定には最新の
                // shutdown_flag を反映し、shutdown 後も新規リクエストを
                // 受け付け続けることは避ける（`max_connection_lifetime` の
                // 扱いと同一パターン）。
                return Some(
                    keep_alive
                        && connection_started_at.elapsed() < server.max_connection_lifetime
                        && !shutdown_flag.load(Ordering::Relaxed),
                );
            }
            // producer が finish を呼ばずに drop された（打ち切り）。応答
            // 完全性を保つため終端チャンクを送らず接続を閉じる
            // （`crate::streaming` モジュール doc の「応答完全性」節）。
            RecvOutcome::Aborted => return None,
        }
    }
}

/// 登録順に `gates` を評価し、最初の [`GateOutcome::Reject`] を返す。
/// 全件 `Allow` の場合は `None`。`ctx` は実 peer address を運ぶ
/// [`GateContext`]（イシュー #486）。
fn first_rejection(
    gates: &[Box<dyn RequestGate>],
    head: &RequestHead,
    ctx: &GateContext,
) -> Option<GateOutcome> {
    gates.iter().find_map(|gate| match gate.check(head, ctx) {
        GateOutcome::Allow => None,
        reject @ GateOutcome::Reject { .. } => Some(reject),
    })
}

/// [`RequestError`] を応答すべき HTTP ステータスへマッピングする。
///
/// `None` はエラー応答を送らず接続を閉じるべきケース（途中 EOF・I/O エラー）
/// を意味する。マッピング根拠は本モジュール冒頭の doc・実装計画のセキュリティ
/// 考慮（入力検証の全面依拠、フェイルセーフなクローズ）を参照。
///
/// chunked 関連（イシュー #181）: `Transfer-Encoding: chunked` 単独指定は
/// `fandhe_backend_http::body::body_length` が受理し `RequestError` にならないため、ここ
/// では現れない（200 系として通常どおり処理される）。`gzip` 等 chunked 以外の
/// coding・複数 TE ヘッダは従来どおり `TransferEncodingUnsupported` として
/// 501。`Content-Length` との共存は専用エラー `ContentLengthWithChunked` として
/// 400（RFC 9112 §6.3 のスマグリング対策の意味を明確化）。chunked デコード中の
/// DoS 上限超過・構文エラーは `RequestError::Chunked` 経由でここに到達し、
/// 総量超過は 413、それ以外（構文・チャンク総数・行長・trailer）は 400。
fn error_response(err: &RequestError) -> Option<Response> {
    match err {
        RequestError::Parse(ParseError::HeaderSectionTooLarge | ParseError::TooManyHeaders) => {
            Some(Response::empty(431))
        }
        RequestError::Parse(ParseError::InvalidRequestLine | ParseError::InvalidHeader) => {
            Some(Response::empty(400))
        }
        RequestError::Parse(ParseError::UnsupportedVersion) => Some(Response::empty(505)),
        RequestError::Body(BodyError::BodyTooLarge) => Some(Response::empty(413)),
        RequestError::Body(BodyError::TransferEncodingUnsupported) => Some(Response::empty(501)),
        RequestError::Body(
            BodyError::DuplicateContentLength
            | BodyError::InvalidContentLength
            | BodyError::ContentLengthWithChunked,
        ) => Some(Response::empty(400)),
        RequestError::Chunked(ChunkedError::BodyTooLarge) => Some(Response::empty(413)),
        RequestError::Chunked(
            ChunkedError::InvalidChunkSize
            | ChunkedError::ChunkLineTooLong
            | ChunkedError::TooManyChunks
            | ChunkedError::TrailerUnsupported
            | ChunkedError::InvalidLineTerminator,
        ) => Some(Response::empty(400)),
        RequestError::UnexpectedEof | RequestError::Io(_) => None,
    }
}

// `TcpStream` を明示的に参照し、`handle_connection` の型パラメータ `S` が
// 実ソケットに対しても解決可能であること（`BoundServer::run` の呼び出しが
// 成立すること）をコンパイル時に保証する。
#[allow(dead_code)]
fn _assert_handle_connection_accepts_tcp_stream(
    server: &Server,
    stream: TcpStream,
) -> impl Future + '_ {
    handle_connection(server, stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{GateContext, GateOutcome, Middleware, RequestGate, UpgradeHandler};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncReadExt;

    /// 呼び出し順・回数を記録するトイ `Middleware`。
    struct RecordingMiddleware {
        events: Mutex<Vec<&'static str>>,
    }

    impl Middleware for RecordingMiddleware {
        fn name(&self) -> &'static str {
            "recording"
        }
        fn on_request(&self, _head: &RequestHead) {
            self.events.lock().unwrap().push("on_request");
        }
        fn on_response(&self, _head: &RequestHead, _elapsed: Duration) {
            self.events.lock().unwrap().push("on_response");
        }
    }

    /// `Authorization` ヘッダ必須のフェイルクローズ `RequestGate`。
    struct RequireAuthGate;
    impl RequestGate for RequireAuthGate {
        fn name(&self) -> &'static str {
            "require-auth"
        }
        fn check(&self, head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
            match head.header("authorization") {
                Some(_) => GateOutcome::Allow,
                None => GateOutcome::reject(401, b"unauthorized".to_vec()),
            }
        }
    }

    /// 常に拒否する `RequestGate`（複数 gate の「最初の Reject」テスト用）。
    struct AlwaysRejectGate(u16);
    impl RequestGate for AlwaysRejectGate {
        fn name(&self) -> &'static str {
            "always-reject"
        }
        fn check(&self, _head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
            GateOutcome::reject(self.0, Vec::new())
        }
    }

    /// `429 + Retry-After` を返すトイ `RequestGate`（イシュー #424、ヘッダ付き
    /// 拒否応答がワイヤ上に正しく出力されることを確認するワイヤレベル統合
    /// テスト用）。
    struct RateLimitGate;
    impl RequestGate for RateLimitGate {
        fn name(&self) -> &'static str {
            "rate-limit"
        }
        fn check(&self, _head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
            let response = Response::new(429, b"{\"error\":\"rate limited\"}".to_vec())
                .with_content_type("application/json")
                .with_header("Retry-After", "30")
                .expect("リテラル値は構築時検証を通る");
            GateOutcome::Reject { response }
        }
    }

    struct AllowAllGate;
    impl RequestGate for AllowAllGate {
        fn name(&self) -> &'static str {
            "allow-all"
        }
        fn check(&self, _head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
            GateOutcome::Allow
        }
    }

    /// 観測した `GateContext::peer_addr()` を記録し、実 TCP accept 経路で
    /// 実 peer address が正しく伝搬することを検証する e2e テスト用ゲート
    /// （イシュー #486）。常に `Allow` を返し接続処理を妨げない。
    struct RecordingPeerAddrGate {
        observed: std::sync::Mutex<Vec<Option<std::net::SocketAddr>>>,
    }
    impl RequestGate for RecordingPeerAddrGate {
        fn name(&self) -> &'static str {
            "recording-peer-addr"
        }
        fn check(&self, _head: &RequestHead, ctx: &GateContext) -> GateOutcome {
            self.observed.lock().unwrap().push(ctx.peer_addr());
            GateOutcome::Allow
        }
    }
    // `Server::gate` は所有権を取って `Box<dyn RequestGate>` へ格納するため、
    // テスト側が観測結果を読み取れるよう `Arc` 越しに委譲実装する（トレイトは
    // 本クレート内のローカル trait のため orphan rule に抵触しない）。
    impl RequestGate for Arc<RecordingPeerAddrGate> {
        fn name(&self) -> &'static str {
            (**self).name()
        }
        fn check(&self, head: &RequestHead, ctx: &GateContext) -> GateOutcome {
            (**self).check(head, ctx)
        }
    }

    /// `Upgrade: websocket` ヘッダにマッチするトイ `UpgradeHandler`。
    struct WebSocketUpgrade;
    impl UpgradeHandler for WebSocketUpgrade {
        fn name(&self) -> &'static str {
            "websocket"
        }
        fn matches(&self, head: &RequestHead) -> bool {
            head.header("upgrade")
                .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        }
    }

    /// 固定レスポンスを返すトイ `Handler`。
    struct FixedHandler {
        status: u16,
        body: &'static [u8],
        calls: AtomicUsize,
    }
    impl Handler for FixedHandler {
        fn handle(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> fandhe_backend_routes::HandlerFuture {
            Box::pin(std::future::ready({
                self.calls.fetch_add(1, Ordering::Relaxed);
                Response::new(self.status, self.body.to_vec())
            }))
        }
    }

    async fn roundtrip(server: &Server, request: &[u8]) -> String {
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;
        client.write_all(request).await.unwrap();
        client.shutdown().await.unwrap();

        handle_connection(server, server_stream).await;

        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        String::from_utf8(out).unwrap()
    }

    /// [`roundtrip`] の shutdown_flag 版。`BoundServer::run_until` を経由せず
    /// `handle_connection_with_permit` を直接叩き、`shutdown_flag=true`
    /// （graceful shutdown シグナル受信後）の挙動を単体テストする（Bugbot
    /// 指摘 review comment 3615144800 / 3615144815 の回帰防止）。
    async fn roundtrip_with_shutdown_flag(server: &Server, request: &[u8]) -> String {
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;
        client.write_all(request).await.unwrap();
        client.shutdown().await.unwrap();

        let shutdown_flag = Arc::new(AtomicBool::new(true));
        handle_connection_with_permit(
            server,
            server_stream,
            None,
            &shutdown_flag,
            None,
            crate::plugin::UpgradeCancel::disabled(),
        )
        .await;

        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        String::from_utf8(out).unwrap()
    }

    #[tokio::test]
    async fn no_handler_registered_returns_404() {
        let server = Server::new();
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    #[tokio::test]
    async fn registered_handler_is_invoked() {
        let handler = FixedHandler {
            status: 200,
            body: b"hi",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("hi"));
    }

    /// TASK-1.5（#14）: `fandhe_backend_routes::Router` を `Server::handler` にそのまま登録できる
    /// （`impl Handler for fandhe_backend_routes::Router` の統合確認）。200・404・405 それぞれで
    /// ステータス行・`Content-Length`・body・`Connection: close` まで網羅的に検証する。
    #[tokio::test]
    async fn router_registered_as_handler_dispatches_by_method_and_target() {
        let router = fandhe_backend_routes::Router::new().route("GET", "/", |_head, _body| {
            Response::new(200, b"root".to_vec())
        });
        let server = Server::new().handler(router);

        let ok = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(ok.contains("Content-Length: 4\r\n"));
        assert!(ok.contains("Connection: close\r\n"));
        assert!(ok.ends_with("root"));

        let missing = roundtrip(
            &server,
            b"GET /missing HTTP/1.1\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(missing.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(missing.contains("Content-Length: 0\r\n"));

        let wrong_method =
            roundtrip(&server, b"POST / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(wrong_method.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
        assert!(wrong_method.contains("Content-Length: 0\r\n"));
        // TASK-177 / #177: 405 ワイヤ応答に登録済み method の Allow ヘッダが
        // コアループの直列化経路（fandhe_backend_routes::Router::dispatch → Response::serialize）
        // を通しても欠落しないことを確認する。
        assert!(wrong_method.contains("Allow: GET\r\n"));
    }

    /// TASK-176（#176）: `Router::route_param` で登録した `{name}` パスパラメータ
    /// ルートも `Server` 経由（`impl Handler for fandhe_backend_routes::Router`）で解決できる
    /// ことを end-to-end で確認する。
    #[tokio::test]
    async fn router_registered_as_handler_dispatches_path_params() {
        let router = fandhe_backend_routes::Router::new()
            .route_param("GET", "/hello/{name}", |_head, params, _body| {
                let name = params.get("name").unwrap_or("world");
                Response::new(200, format!("hello, {name}").into_bytes())
            })
            .expect("valid pattern");
        let server = Server::new().handler(router);

        let ok = roundtrip(
            &server,
            b"GET /hello/alice HTTP/1.1\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(ok.ends_with("hello, alice"));
    }

    #[tokio::test]
    async fn middleware_hooks_fire_in_order_with_positive_elapsed() {
        let mw = Arc::new(RecordingMiddleware {
            events: Mutex::new(Vec::new()),
        });
        struct MwProxy(Arc<RecordingMiddleware>);
        impl Middleware for MwProxy {
            fn name(&self) -> &'static str {
                "proxy"
            }
            fn on_request(&self, head: &RequestHead) {
                self.0.on_request(head);
            }
            fn on_response(&self, head: &RequestHead, elapsed: Duration) {
                self.0.on_response(head, elapsed);
            }
        }

        let server = Server::new().middleware(MwProxy(Arc::clone(&mw)));
        let _ = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;

        let events = mw.events.lock().unwrap();
        assert_eq!(*events, vec!["on_request", "on_response"]);
    }

    #[tokio::test]
    async fn gate_rejects_missing_authorization_with_401() {
        let server = Server::new().gate(RequireAuthGate);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(response.ends_with("unauthorized"));
    }

    #[tokio::test]
    async fn gate_allows_request_with_authorization_to_reach_handler() {
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().gate(RequireAuthGate).handler(handler);
        let response = roundtrip(
            &server,
            b"GET / HTTP/1.1\r\nAuthorization: Bearer x\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[tokio::test]
    async fn first_gate_rejection_wins_over_later_gates() {
        let server = Server::new()
            .gate(AlwaysRejectGate(403))
            .gate(AlwaysRejectGate(401));
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
    }

    #[tokio::test]
    async fn gate_reject_with_headers_reaches_the_wire() {
        // イシュー #424: `GateOutcome::Reject` が運ぶ `Response` の
        // `Retry-After` / `Content-Type` ヘッダがワイヤ上の応答へそのまま
        // 出力され、`Content-Length` / `Connection` はコア（`serialize`）
        // 管理のままであることを固定する。
        let server = Server::new().gate(RateLimitGate);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 429 Too Many Requests\r\n"));
        assert!(response.contains("Retry-After: 30\r\n"));
        assert!(response.contains("Content-Type: application/json\r\n"));
        assert!(response.contains("Content-Length: "));
        assert!(response.contains("Connection: close\r\n"));
        assert!(response.ends_with("{\"error\":\"rate limited\"}"));
    }

    #[tokio::test]
    async fn allow_all_gate_does_not_block_handler() {
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().gate(AllowAllGate).handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[tokio::test]
    async fn upgrade_match_returns_501_and_closes_without_invoking_handler() {
        let handler = FixedHandler {
            status: 200,
            body: b"should-not-be-called",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .upgrade_handler(WebSocketUpgrade)
            .handler(handler);
        let response = roundtrip(
            &server,
            b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\nConnection: keep-alive\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 501 Not Implemented\r\n"));
    }

    #[tokio::test]
    async fn non_matching_upgrade_handler_falls_through_to_handler() {
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .upgrade_handler(WebSocketUpgrade)
            .handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    /// Bugbot 指摘（review comment 3615144815、"Upgrade ignores shutdown
    /// flag"）の回帰防止テスト。graceful shutdown シグナル受信後
    /// （`shutdown_flag=true`）は `UpgradeHandler` がマッチしても Upgrade を
    /// 許可せず、503 で明示的に拒否して接続を閉じることを確認する。
    #[tokio::test]
    async fn upgrade_match_returns_503_when_shutdown_flag_is_set() {
        let handler = FixedHandler {
            status: 200,
            body: b"should-not-be-called",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .upgrade_handler(WebSocketUpgrade)
            .handler(handler);
        let response = roundtrip_with_shutdown_flag(
            &server,
            b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\nConnection: keep-alive\r\n\r\n",
        )
        .await;
        assert!(
            response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
            "shutdown 後の Upgrade は 503 で拒否されるはず（実際: {response}）"
        );
    }

    /// Bugbot 指摘（review comment 3615144800、"Stale keep-alive after
    /// shutdown"）の回帰防止テスト。`on_request` 直後の `keep_alive` 算出
    /// 時点では見えていなかった shutdown_flag を送信直前に再チェックし、
    /// クライアントが `keep-alive` を要求していても応答に必ず
    /// `Connection: close` を付与することを確認する。
    #[tokio::test]
    async fn keep_alive_request_gets_connection_close_when_shutdown_flag_is_set() {
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().handler(handler);
        let response = roundtrip_with_shutdown_flag(
            &server,
            b"GET / HTTP/1.1\r\nConnection: keep-alive\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(
            response.to_lowercase().contains("connection: close"),
            "shutdown 後は keep-alive 要求でも Connection: close を返すはず\
             （実際: {response}）"
        );
    }

    #[tokio::test]
    async fn keep_alive_connection_serves_two_requests_sequentially() {
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().handler(handler);
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;

        let write_task = tokio::spawn(async move {
            client
                .write_all(b"GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut out = Vec::new();
            client.read_to_end(&mut out).await.unwrap();
            out
        });

        handle_connection(&server, server_stream).await;
        let out = write_task.await.unwrap();
        let text = String::from_utf8(out).unwrap();

        assert_eq!(text.matches("HTTP/1.1 200 OK").count(), 2);
    }

    #[tokio::test]
    async fn max_requests_per_connection_forces_close_after_limit() {
        // keep-alive を要求し続けるクライアントでも、
        // max_requests_per_connection に達した時点で `Connection: close` に
        // 切り替わり、以後は同じ接続で次のリクエストを待たないことを確認する
        // （#70 レビュー指摘: keep-alive 中の総リクエスト数の上限）。
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .max_requests_per_connection(2)
            .handler(handler);
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;

        let write_task = tokio::spawn(async move {
            // 3 リクエストとも keep-alive を要求するが、上限は 2 件。
            client
                .write_all(
                    b"GET /a HTTP/1.1\r\n\r\n\
GET /b HTTP/1.1\r\n\r\n\
GET /c HTTP/1.1\r\n\r\n",
                )
                .await
                .unwrap();
            let mut out = Vec::new();
            client.read_to_end(&mut out).await.unwrap();
            out
        });

        handle_connection(&server, server_stream).await;
        let out = write_task.await.unwrap();
        let text = String::from_utf8(out).unwrap();

        // 2 件のみ応答され、3 件目は送られる前に接続が閉じられる。
        assert_eq!(text.matches("HTTP/1.1 200 OK").count(), 2);
        // 上限に達した最後の応答は Connection: close を伴う。
        assert!(text.contains("Connection: close"));
    }

    #[tokio::test]
    async fn keep_alive_disabled_forces_close_after_first_request() {
        // `Server::keep_alive(false)` は `should_keep_alive` の判定結果に
        // かかわらず常に `Connection: close` で応答し、1 接続 1 リクエストで
        // 閉じることを確認する（受け入れ条件 3、`max_requests_per_connection`
        // の上限到達時と同じ「以後の接続では次のリクエストを待たない」挙動を
        // keep_alive 無効化そのもので再現する）。
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().keep_alive(false).handler(handler);
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;

        let write_task = tokio::spawn(async move {
            // 2 リクエストとも keep-alive を要求するが、サーバ側で無効化済み。
            client
                .write_all(b"GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
            let mut out = Vec::new();
            client.read_to_end(&mut out).await.unwrap();
            out
        });

        handle_connection(&server, server_stream).await;
        let out = write_task.await.unwrap();
        let text = String::from_utf8(out).unwrap();

        // 1 件のみ応答され、2 件目は送られる前に接続が閉じられる。
        assert_eq!(text.matches("HTTP/1.1 200 OK").count(), 1);
        assert!(text.contains("Connection: close"));
    }

    #[tokio::test]
    async fn max_connection_lifetime_closes_before_next_read() {
        // 総生存期間の上限に達した接続は、次のリクエストの読み取り待ちに
        // 入る前に閉じられることを確認する（#70 レビュー指摘: 接続の総生存
        // 期間の上限。read_timeout（既定 DEFAULT_READ_TIMEOUT）では防げない、短い間隔で送信し続ける
        // クライアントによる permit 占有を防ぐ）。
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .max_connection_lifetime(Duration::from_millis(0))
            .handler(handler);
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;

        let write_task = tokio::spawn(async move {
            // 生存期間 0 のため handle_connection 側が読み取り前に接続を
            // 閉じている可能性があり、write が失敗しても構わない（無視する）。
            let _ = client.write_all(b"GET / HTTP/1.1\r\n\r\n").await;
            let mut out = Vec::new();
            let _ = client.read_to_end(&mut out).await;
            out
        });

        handle_connection(&server, server_stream).await;
        let out = write_task.await.unwrap();

        // 生存期間 0 のため、最初のリクエストすら読まれずに接続が閉じられる。
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn max_connection_lifetime_bounds_slow_read_below_read_timeout() {
        // #70 Bugbot 指摘: read_request のタイムアウトが常に read_timeout（既定 DEFAULT_READ_TIMEOUT）
        // （30 秒）固定だと、直前の生存期間チェックを通過した直後にスロー
        // クライアントが read をブロックさせた場合、生存期間の上限
        // （本テストでは 50ms）を大幅に超えて（最大で read_timeout 分）
        // permit を握り続けてしまう。修正後は残り生存期間で read タイムアウトを
        // 丸めるため、read がブロックしている接続も生存期間の上限近辺で
        // 閉じられることを確認する（30 秒の DEFAULT_READ_TIMEOUT を待たない）。
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .max_connection_lifetime(Duration::from_millis(50))
            .handler(handler);
        let (client, server_stream) = tokio::io::duplex(8192);

        // クライアントは何も送らず接続だけ張り続ける（スロークライアント）。
        // read_request は永久にブロックしうるが、生存期間に基づく短縮
        // タイムアウトにより DEFAULT_READ_TIMEOUT（30 秒）を待たずに閉じられるはず。
        let started = Instant::now();
        handle_connection(&server, server_stream).await;
        let elapsed = started.elapsed();
        drop(client);

        // DEFAULT_READ_TIMEOUT（30 秒）よりも十分短い時間で接続が閉じられたことを
        // 確認する（CI 環境のスケジューリング遅延を許容しつつ、修正前の
        // 挙動（30 秒待ち）とは明確に区別できるしきい値）。
        assert!(
            elapsed < Duration::from_secs(5),
            "生存期間の上限近辺で閉じられるはずが {elapsed:?} かかった（read_timeout 固定に戻っていないか確認）"
        );
    }

    #[tokio::test]
    async fn read_timeout_closes_slow_client_at_configured_value() {
        // `Server::read_timeout` で設定した値がそのまま read 待ちタイムアウトに
        // 適用されることを確認する（受け入れ条件 2）。既定 30 秒のままでは
        // 本テストのしきい値内に収まらないため、設定が効いたことを判別できる。
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .read_timeout(Duration::from_millis(50))
            .handler(handler);
        let (client, server_stream) = tokio::io::duplex(8192);

        // クライアントは何も送らず接続だけ張り続ける（スロークライアント）。
        let started = Instant::now();
        handle_connection(&server, server_stream).await;
        let elapsed = started.elapsed();
        drop(client);

        assert!(
            elapsed < Duration::from_secs(5),
            "read_timeout(50ms) を設定したのに {elapsed:?} かかった（既定 30 秒のまま反映されていないか確認）"
        );
    }

    #[tokio::test]
    async fn read_timeout_zero_fails_closed() {
        // `Server::read_timeout(Duration::ZERO)` は最初のリクエストの読み取り
        // 待ちにも即座にタイムアウトし、応答を送らず接続を閉じることを確認する
        // （受け入れ条件 2 のゼロ検証。フェイルクローズ、`Server::read_timeout`
        // の doc を参照）。クライアントが何も送らない状態（`tokio::io::duplex`
        // に未読データなし）で検証する。`tokio::time::timeout` は内部の読み取り
        // future を先にポーリングするため、送信済みデータがバッファにあると
        // タイムアウトより先に読み取りが完了して即座には閉じない可能性がある
        // （実測で確認済み）。データなしなら読み取り future は必ず Pending と
        // なり、ZERO タイムアウトが確実に先勝ちする。
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().read_timeout(Duration::ZERO).handler(handler);
        let (client, server_stream) = tokio::io::duplex(8192);

        let started = Instant::now();
        handle_connection(&server, server_stream).await;
        let elapsed = started.elapsed();
        drop(client);

        // 既定 30 秒を待たず、即座（数秒未満）に接続が閉じられる。
        assert!(
            elapsed < Duration::from_secs(5),
            "read_timeout(ZERO) を設定したのに {elapsed:?} かかった（フェイルクローズできていない）"
        );
    }

    #[tokio::test]
    async fn extreme_read_timeout_is_bounded_by_connection_lifetime() {
        // `read_timeout` に極端に大きい値（86400 秒）を設定しても、実効
        // タイムアウトは `max_connection_lifetime`（本テストでは 50ms）との
        // 短い方に丸められるため、接続占有はあくまで総生存期間上限で必ず
        // 打ち切られることを確認する（受け入れ条件 2 の極端値検証。
        // `max_connection_lifetime_bounds_slow_read_below_read_timeout` と
        // 同じ構図を read_timeout 側の値を変えて再現する）。
        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new()
            .read_timeout(Duration::from_secs(86_400))
            .max_connection_lifetime(Duration::from_millis(50))
            .handler(handler);
        let (client, server_stream) = tokio::io::duplex(8192);

        // クライアントは何も送らず接続だけ張り続ける（スロークライアント）。
        let started = Instant::now();
        handle_connection(&server, server_stream).await;
        let elapsed = started.elapsed();
        drop(client);

        assert!(
            elapsed < Duration::from_secs(5),
            "read_timeout を極端に大きくしても max_connection_lifetime の上限で \
             閉じられるはずが {elapsed:?} かかった（read_timeout が生存期間で \
             丸められていないか確認）"
        );
    }

    /// `Handler::handle` の処理中に指定時間だけスリープしてから固定レスポンスを
    /// 返すトイ `Handler`（生存期間超過タイミングを `handle` の中に作るための道具）。
    struct SlowHandler {
        sleep_for: Duration,
    }
    impl Handler for SlowHandler {
        fn handle(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> fandhe_backend_routes::HandlerFuture {
            Box::pin(std::future::ready({
                // `Handler::handle` は async 契約（boxed-future）だが、この
                // `std::future::ready` は構築時点で中身を同期評価するため、
                // `std::thread::sleep` は poll ではなく `handle` 呼び出し内で
                // 即座に実行される（本体側 await ではないため
                // `.claude/rules/coding-rust.md` の「ブロッキング処理を await
                // スレッドで実行しない」に抵触しない。処理時間の長期化を
                // 模擬するテスト専用ヘルパーであり、単体テストのみで使用）。
                std::thread::sleep(self.sleep_for);
                Response::new(200, b"ok".to_vec())
            }))
        }
    }

    #[tokio::test]
    async fn keep_alive_becomes_close_when_lifetime_expires_during_handle() {
        // #70 Bugbot 指摘（Stale keep-alive after lifetime）: keep_alive は
        // `on_request` 直後の経過時間だけで決めると、`Handler::handle` の処理が
        // 長引いて `max_connection_lifetime` を超えても応答が keep-alive の
        // ままになってしまう。修正後は `handle` 完了後に生存期間を再チェックし、
        // 超過していれば応答に `Connection: close` が付くことを確認する。
        let lifetime = Duration::from_millis(30);
        let handler = SlowHandler {
            // handle 呼び出しの前は生存期間内、handle 完了時には確実に
            // 超過しているように、生存期間よりわずかに長くスリープする。
            sleep_for: lifetime + Duration::from_millis(50),
        };
        let server = Server::new()
            .max_connection_lifetime(lifetime)
            .handler(handler);

        // keep-alive を要求する HTTP/1.1 リクエスト（Connection ヘッダなしは
        // HTTP/1.1 既定で keep-alive）。
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(
            response.contains("Connection: close"),
            "handle 中に生存期間を超過したにもかかわらず keep-alive のまま応答した: {response:?}"
        );
    }

    #[tokio::test]
    async fn invalid_request_line_returns_400() {
        let server = Server::new();
        let response = roundtrip(&server, b"G@T / HTTP/1.1\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    }

    #[tokio::test]
    async fn too_many_headers_returns_431() {
        let server = Server::new();
        let mut request = b"GET / HTTP/1.1\r\n".to_vec();
        for i in 0..200 {
            request.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
        }
        request.extend_from_slice(b"\r\n");
        let response = roundtrip(&server, &request).await;
        assert!(response.starts_with("HTTP/1.1 431 Request Header Fields Too Large\r\n"));
    }

    #[tokio::test]
    async fn body_too_large_returns_413() {
        let server = Server::new();
        let response = roundtrip(
            &server,
            b"POST / HTTP/1.1\r\nContent-Length: 999999999999\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"));
    }

    #[tokio::test]
    async fn transfer_encoding_gzip_returns_501() {
        // イシュー #181: chunked 以外の coding は従来どおり 501 未実装拒否。
        let server = Server::new();
        let response = roundtrip(
            &server,
            b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 501 Not Implemented\r\n"));
    }

    #[tokio::test]
    async fn transfer_encoding_chunked_is_accepted_and_dispatched() {
        // イシュー #181: 単独 `chunked` は body フレーミングとして受理され、
        // 通常のリクエストと同じくハンドラ解決まで進む（未登録ハンドラの
        // 404 まで到達すること = chunked デコード自体はエラーにならないこと
        // を固定する。501/400/413 のいずれでもないことが確認したい観点）。
        let server = Server::new();
        let response = roundtrip(
            &server,
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n0\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    #[tokio::test]
    async fn content_length_with_chunked_returns_400() {
        // イシュー #181: RFC 9112 §6.3 のスマグリング対策として共存を拒否する。
        let server = Server::new();
        let response = roundtrip(
            &server,
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 4\r\n\r\n4\r\nWiki\r\n0\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    }

    #[tokio::test]
    async fn chunked_invalid_size_returns_400() {
        let server = Server::new();
        let response = roundtrip(
            &server,
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nZZZ\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    }

    #[tokio::test]
    async fn chunked_body_too_large_returns_413() {
        use fandhe_backend_http::body::MAX_BODY_BYTES;

        let server = Server::new();
        let request = format!(
            "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n{:X}\r\n",
            MAX_BODY_BYTES + 1
        );
        let response = roundtrip(&server, request.as_bytes()).await;
        assert!(response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"));
    }

    #[tokio::test]
    async fn max_body_bytes_custom_limit_rejects_over_boundary() {
        // イシュー #311: Server::max_body_bytes で上限を上書きした場合、
        // 上限を超える固定長 body は 413 で拒否される。
        let server = Server::new().max_body_bytes(4);
        let response = roundtrip(
            &server,
            b"POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nabcde",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"));
    }

    #[tokio::test]
    async fn max_body_bytes_custom_limit_accepts_at_boundary() {
        // 上限ちょうどの body は受理され、通常どおりハンドラ解決（未登録
        // ハンドラの 404）まで進む（body_too_large_returns_413 との対で境界を
        // 固定する）。
        let server = Server::new().max_body_bytes(4);
        let response =
            roundtrip(&server, b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd").await;
        assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    #[tokio::test]
    async fn max_body_bytes_custom_limit_propagates_to_chunked() {
        // カスタム上限が chunked 経路にも伝搬することを固定する
        // （chunked_body_too_large_returns_413 のカスタム上限版）。
        let server = Server::new().max_body_bytes(4);
        let response = roundtrip(
            &server,
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nabcde\r\n0\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"));
    }

    #[tokio::test]
    async fn max_body_bytes_zero_rejects_body_but_accepts_bodyless() {
        // `0` は「body を持つリクエストを一律拒否する」設定として許容される
        // （フェイルクローズ方向。doc comment の明記事項を固定する）。
        let server = Server::new().max_body_bytes(0);
        let response = roundtrip(&server, b"POST / HTTP/1.1\r\nContent-Length: 1\r\n\r\nx").await;
        assert!(response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"));

        let server = Server::new().max_body_bytes(0);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\n\r\n").await;
        assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    #[tokio::test]
    async fn max_body_bytes_default_matches_unmodified_server() {
        // ビルダー未呼び出し時は既定 MAX_BODY_BYTES のまま、既存の
        // body_too_large_returns_413 と同一境界であることを固定する
        // （後方互換の担保）。
        use fandhe_backend_http::body::MAX_BODY_BYTES;

        let server = Server::new();
        let request = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        let response = roundtrip(&server, request.as_bytes()).await;
        assert!(response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"));
    }

    #[tokio::test]
    async fn immediate_eof_closes_without_response() {
        let server = Server::new();
        let (client, server_stream) = tokio::io::duplex(64);
        drop(client);
        // panic しないことのみを確認する（正常クローズであり応答は送らない）。
        handle_connection(&server, server_stream).await;
    }

    #[tokio::test]
    async fn max_connections_limits_concurrent_accept() {
        use tokio::time::timeout;

        let handler = FixedHandler {
            status: 200,
            body: b"ok",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().max_connections(1).handler(handler);
        let bound = server.bind("127.0.0.1:0").await.unwrap();
        let addr = bound.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = bound.run().await;
        });

        // client1 は何も送らず接続を張ったままにし、唯一の permit を占有する
        // （handle_connection が read_request の最初の読み取りで待機し続ける）。
        let client1 = TcpStream::connect(addr).await.unwrap();

        // client2 はリクエストを送るが、permit が枯渇しているため run() の
        // accept ループがまだこの接続を受理していないはずで、応答は来ない。
        let mut client2 = TcpStream::connect(addr).await.unwrap();
        client2
            .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut probe = [0u8; 1];
        let no_response_yet = timeout(Duration::from_millis(200), client2.read(&mut probe)).await;
        assert!(
            no_response_yet.is_err(),
            "max_connections が守られていれば client2 はまだ応答を受け取らないはず"
        );

        // client1 を閉じて permit を解放する。
        drop(client1);

        // run() が permit を取得して client2 を受理・処理するのを待つ。
        let mut out = Vec::new();
        timeout(Duration::from_secs(2), client2.read_to_end(&mut out))
            .await
            .expect("permit 解放後は client2 が受理されるはず")
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[tokio::test]
    async fn request_gate_receives_real_peer_addr_over_tcp() {
        // 実 TCP accept 経路で `GateContext::peer_addr()` がクライアント側の
        // `local_addr()` と一致することを検証する e2e テスト（イシュー #486）。
        let gate = Arc::new(RecordingPeerAddrGate {
            observed: std::sync::Mutex::new(Vec::new()),
        });
        let server = Server::new().gate(Arc::clone(&gate));
        let bound = server.bind("127.0.0.1:0").await.unwrap();
        let addr = bound.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = bound.run().await;
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        let client_local_addr = client.local_addr().unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();

        let observed = gate.observed.lock().unwrap();
        assert_eq!(
            observed.as_slice(),
            &[Some(client_local_addr)],
            "gate が観測した peer_addr はクライアントの local_addr と一致するはず"
        );
    }

    #[tokio::test]
    async fn handle_connection_duplex_path_yields_none_peer_addr() {
        // `tokio::io::duplex`（非ソケット）経路では `GateContext::peer_addr()` が
        // 常に `None` であることを固定する（イシュー #486、`GateContext` の
        // doc「`peer_addr` が `None` になる経路」）。
        let gate = Arc::new(RecordingPeerAddrGate {
            observed: std::sync::Mutex::new(Vec::new()),
        });
        let server = Server::new().gate(Arc::clone(&gate));
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;
        client
            .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        handle_connection(&server, server_stream).await;

        let observed = gate.observed.lock().unwrap();
        assert_eq!(observed.as_slice(), &[None]);
    }

    #[tokio::test]
    async fn handle_connection_with_peer_addr_injects_supplied_addr() {
        // `handle_connection_with_peer_addr`（公開 API、イシュー #486）が
        // 注入した `SocketAddr` を gate へ届けることを検証する。実ソケットの
        // 代わりに `tokio::io::duplex` を使い、注入経路そのもの
        // （`peer_addr: Some(..)` を明示指定した場合の伝搬）に焦点を絞る
        // （実 TCP accept 経路の検証は
        // `request_gate_receives_real_peer_addr_over_tcp` が別途担う）。
        let gate = Arc::new(RecordingPeerAddrGate {
            observed: std::sync::Mutex::new(Vec::new()),
        });
        let server = Server::new().gate(Arc::clone(&gate));
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::AsyncWriteExt as _;
        client
            .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        client.shutdown().await.unwrap();

        let injected_addr: std::net::SocketAddr = "203.0.113.9:9999".parse().unwrap();
        handle_connection_with_peer_addr(&server, server_stream, injected_addr).await;

        let observed = gate.observed.lock().unwrap();
        assert_eq!(observed.as_slice(), &[Some(injected_addr)]);
    }

    #[test]
    fn bound_server_run_accepts_tcp_stream_type() {
        // `_assert_handle_connection_accepts_tcp_stream` がコンパイルできることが、
        // `handle_connection` が実ソケット（TcpStream）に対しても型解決できる
        // ことの静的な証跡になる。
        fn _type_check() {
            let _ = _assert_handle_connection_accepts_tcp_stream;
        }
    }

    // --- レスポンス側 chunked ストリーミング送信（イシュー #319） ---

    /// `handle_streaming` で複数チャンクを返すトイ `Handler`。`chunks` を順に
    /// `send` し、最後に `finish` する（正常終端）。
    struct StreamingHandler {
        status: u16,
        content_type: Option<&'static str>,
        chunks: Vec<&'static [u8]>,
    }
    impl Handler for StreamingHandler {
        fn handle(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> fandhe_backend_routes::HandlerFuture {
            // handle_streaming が Some を返す限りこの経路は呼ばれないはず。
            // 呼ばれた場合は実装不備を検知できるよう識別可能な応答にする。
            Box::pin(std::future::ready(Response::empty(599)))
        }

        fn handle_streaming(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> Option<crate::streaming::StreamingResponse> {
            let (response, writer) =
                crate::streaming::StreamingResponse::channel(self.status, self.content_type, 4);
            let chunks: Vec<Vec<u8>> = self.chunks.iter().map(|c| c.to_vec()).collect();
            tokio::spawn(async move {
                for chunk in chunks {
                    if writer.send(chunk).await.is_err() {
                        return;
                    }
                }
                let _ = writer.finish().await;
            });
            Some(response)
        }
    }

    /// [`StreamingHandler`] の打ち切り（`finish` を呼ばない）版。producer は
    /// `chunks` を送った後、`finish` を呼ばずに `writer` を drop する。
    struct AbortingStreamingHandler {
        chunks: Vec<&'static [u8]>,
    }
    impl Handler for AbortingStreamingHandler {
        fn handle(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> fandhe_backend_routes::HandlerFuture {
            Box::pin(std::future::ready(Response::empty(599)))
        }

        fn handle_streaming(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> Option<crate::streaming::StreamingResponse> {
            let (response, writer) = crate::streaming::StreamingResponse::channel(200, None, 4);
            let chunks: Vec<Vec<u8>> = self.chunks.iter().map(|c| c.to_vec()).collect();
            tokio::spawn(async move {
                for chunk in chunks {
                    if writer.send(chunk).await.is_err() {
                        return;
                    }
                }
                // finish を呼ばずに drop（打ち切り）。
            });
            Some(response)
        }
    }

    #[tokio::test]
    async fn streaming_handler_sends_chunked_framing_and_terminator() {
        let handler = StreamingHandler {
            status: 200,
            content_type: Some("text/plain"),
            chunks: vec![b"foo", b"bar"],
        };
        let server = Server::new().handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Transfer-Encoding: chunked\r\n"));
        assert!(response.contains("Content-Type: text/plain\r\n"));
        assert!(!response.contains("Content-Length"));
        // chunk framing（hex サイズ行 + データ + CRLF）+ 終端。
        assert!(response.contains("3\r\nfoo\r\n"));
        assert!(response.contains("3\r\nbar\r\n"));
        assert!(response.ends_with("0\r\n\r\n"));
    }

    #[tokio::test]
    async fn handler_without_streaming_override_keeps_content_length_response() {
        // 既存 Handler（handle_streaming を override しない）が無変更で
        // コンパイル・従来どおり Content-Length 応答を返すことの後方互換回帰
        // （受け入れ基準 2）。
        let handler = FixedHandler {
            status: 200,
            body: b"hi",
            calls: AtomicUsize::new(0),
        };
        let server = Server::new().handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;
        assert!(response.contains("Content-Length: 2\r\n"));
        assert!(!response.contains("Transfer-Encoding"));
    }

    #[tokio::test]
    async fn streaming_response_without_finish_closes_without_terminator() {
        // producer が finish を呼ばずに drop した場合、受信側は終端チャンク
        // なしで接続を閉じる（応答完全性の fail-closed）。
        let handler = AbortingStreamingHandler {
            chunks: vec![b"partial"],
        };
        let server = Server::new().handler(handler);
        let response =
            roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: keep-alive\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("7\r\npartial\r\n"));
        // 終端チャンク（0\r\n\r\n）を送らずに閉じる。
        assert!(!response.ends_with("0\r\n\r\n"));
    }

    #[tokio::test]
    async fn streaming_response_continues_keep_alive_for_pipelined_next_request() {
        // chunked 応答後の keep-alive 継続（パイプライン次リクエスト処理）。
        let handler = StreamingHandler {
            status: 200,
            content_type: None,
            chunks: vec![b"ok"],
        };
        let server = Server::new().handler(handler);
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        client
            .write_all(b"GET / HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();

        handle_connection(&server, server_stream).await;

        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        let text = String::from_utf8(out).unwrap();

        // 2 回分の応答（chunk framing + 終端）が両方含まれることを確認する
        // （1 本目の応答後に接続が閉じられていれば 2 回目の応答は来ない）。
        let occurrences = text.matches("0\r\n\r\n").count();
        assert_eq!(
            occurrences, 2,
            "keep-alive 継続によりパイプライン済み 2 リクエスト目にも応答するはず: {text:?}"
        );
    }

    #[tokio::test]
    async fn streaming_bodyless_status_omits_transfer_encoding_and_terminator() {
        // レビュー指摘（イシュー #319）: `handle_streaming` が 204 を返した
        // 場合、RFC 9112 §6.3 により body を持ち得ないため
        // `Transfer-Encoding: chunked` も終端チャンク（`0\r\n\r\n`）も
        // 出力してはならない。producer が誤って chunk を送信しようとしても
        // （`chunks` に非空データを積んでいる）、それがワイヤへ漏れないこと
        // も合わせて確認する。
        let handler = StreamingHandler {
            status: 204,
            content_type: None,
            chunks: vec![b"leaked"],
        };
        let server = Server::new().handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 204 No Content\r\n"));
        assert!(!response.contains("Transfer-Encoding"));
        assert!(!response.contains("Content-Length"));
        assert!(
            !response.contains("leaked"),
            "body を持ち得ないステータスでは producer が送った chunk が \
             ワイヤへ漏れてはならない: {response:?}"
        );
        // ヘッド送出のみで終わる: 空行の直後に何も続かない。
        assert!(response.ends_with("\r\n\r\n"));
    }

    #[tokio::test]
    async fn streaming_bodyless_status_keeps_keep_alive_without_desync() {
        // レビュー指摘（イシュー #319）の核心: 204 応答後も keep-alive を
        // 継続でき、かつパイプライン済みの次リクエストが正しくパースできる
        // ことを確認する（ヘッダ抑制のみで終端チャンクを送っていた場合、
        // 次リクエストの手前に `0\r\n\r\n` が混入し応答分割・スマグリングを
        // 招く。本テストはそれが起きていないことの直接的な回帰防止）。
        let handler = StreamingHandler {
            status: 204,
            content_type: None,
            chunks: vec![],
        };
        let server = Server::new().handler(handler);
        let (mut client, server_stream) = tokio::io::duplex(8192);
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        client
            .write_all(b"GET / HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();

        handle_connection(&server, server_stream).await;

        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        let text = String::from_utf8(out).unwrap();

        // 2 回分の応答がそのまま連結され、いずれも 204 のステータス行から
        // 始まること（desync していれば 2 回目の応答が別のバイト列から
        // 始まり "HTTP/1.1 204" に一致しなくなる）。
        let expected =
            "HTTP/1.1 204 No Content\r\n\r\nHTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";
        assert_eq!(
            text, expected,
            "204 応答 2 連続はヘッド 2 個のみで構成され、チャンク終端の \
             混入による desync があってはならない"
        );
    }

    #[tokio::test]
    async fn http10_streaming_bodyless_status_omits_body_and_framing_headers() {
        // HTTP/1.0 経路でも同様に body・フレーミングヘッダを出力しない。
        let handler = StreamingHandler {
            status: 304,
            content_type: None,
            chunks: vec![b"leaked"],
        };
        let server = Server::new().handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.0\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 304 Not Modified\r\n"));
        assert!(response.contains("Connection: close\r\n"));
        assert!(!response.contains("Transfer-Encoding"));
        assert!(!response.contains("Content-Length"));
        assert!(
            !response.contains("leaked"),
            "body を持ち得ないステータスでは producer が送った chunk が \
             ワイヤへ漏れてはならない: {response:?}"
        );
        assert!(response.ends_with("\r\n\r\n"));
    }

    #[tokio::test]
    async fn streaming_backpressure_send_blocks_when_channel_is_full() {
        // バックプレッシャ: 容量超過時に send が pend することを確認する
        // （受け入れ基準 3。受信側がまだ何も読み出していない状態で容量を
        // 超えて送信を試みると `.await` で停止する）。
        let (_response, writer) = crate::streaming::StreamingResponse::channel(200, None, 1);
        writer.send(b"first".to_vec()).await.unwrap();

        let send_result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            writer.send(b"second".to_vec()),
        )
        .await;
        assert!(
            send_result.is_err(),
            "容量超過時、受信側が読み出さない限り send は完了しないはず"
        );
    }

    #[tokio::test]
    async fn http10_streaming_request_gets_connection_close_and_eof_terminated_body() {
        // HTTP/1.0 リクエストへのストリーミング応答は chunked framing を
        // 使わず、Connection: close + EOF 終端。
        let handler = StreamingHandler {
            status: 200,
            content_type: None,
            chunks: vec![b"hello"],
        };
        let server = Server::new().handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.0\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Connection: close\r\n"));
        assert!(!response.contains("Transfer-Encoding"));
        assert!(!response.contains("Content-Length"));
        // chunk framing を使わない生データがそのまま body として出力される。
        assert!(response.ends_with("hello"));
    }

    /// レビュー指摘（イシュー #319）の回帰防止テスト。streaming 応答経路
    /// でも通常応答経路（`keep_alive_request_gets_connection_close_when_shutdown_flag_is_set`）
    /// と同様、`shutdown_flag=true`（graceful shutdown シグナル受信後）で
    /// あれば、クライアントが keep-alive を要求していても chunked ヘッドを
    /// `Connection: close` で送出することを確認する。
    #[tokio::test]
    async fn streaming_keep_alive_request_gets_connection_close_when_shutdown_flag_is_set() {
        let handler = StreamingHandler {
            status: 200,
            content_type: None,
            chunks: vec![b"ok"],
        };
        let server = Server::new().handler(handler);
        let response = roundtrip_with_shutdown_flag(
            &server,
            b"GET / HTTP/1.1\r\nConnection: keep-alive\r\n\r\n",
        )
        .await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(
            response.contains("Connection: close\r\n"),
            "shutdown_flag が立っていれば chunked ヘッドも Connection: close を \
             広告するはず（実際: {response:?}）"
        );
    }

    /// レビュー指摘（イシュー #319）の回帰防止テスト。`write_streaming_response`
    /// が `None`（producer が `finish` を呼ばずに drop、= 打ち切り）を返した
    /// 場合、通常応答経路の write_all 失敗時と同様に `on_response` を呼ばない
    /// ことを確認する（「on_response は完走した応答にのみ対応する」契約の
    /// streaming/非 streaming 経路間での統一）。
    #[tokio::test]
    async fn streaming_abort_does_not_invoke_on_response() {
        let handler = AbortingStreamingHandler {
            chunks: vec![b"partial"],
        };
        let mw = Arc::new(RecordingMiddleware {
            events: Mutex::new(Vec::new()),
        });
        struct MwProxy(Arc<RecordingMiddleware>);
        impl Middleware for MwProxy {
            fn name(&self) -> &'static str {
                "proxy"
            }
            fn on_request(&self, head: &RequestHead) {
                self.0.on_request(head);
            }
            fn on_response(&self, head: &RequestHead, elapsed: Duration) {
                self.0.on_response(head, elapsed);
            }
        }
        let server = Server::new()
            .handler(handler)
            .middleware(MwProxy(Arc::clone(&mw)));

        let _ = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: keep-alive\r\n\r\n").await;

        let events = mw.events.lock().unwrap();
        assert_eq!(
            *events,
            vec!["on_request"],
            "打ち切り（finish なし drop）では on_response を呼ばないはず: {events:?}"
        );
    }

    /// レビュー指摘（イシュー #319）の回帰防止テスト。HTTP/1.0 経路でも
    /// HTTP/1.1 経路（`streaming_abort_does_not_invoke_on_response`）と同様、
    /// 打ち切り（producer が `finish` を呼ばずに drop）では `on_response` を
    /// 呼ばないことを確認する（「on_response は完走した応答にのみ対応する」
    /// 契約のバージョン間統一）。
    #[tokio::test]
    async fn http10_streaming_abort_does_not_invoke_on_response() {
        let handler = AbortingStreamingHandler {
            chunks: vec![b"partial"],
        };
        let mw = Arc::new(RecordingMiddleware {
            events: Mutex::new(Vec::new()),
        });
        struct MwProxy(Arc<RecordingMiddleware>);
        impl Middleware for MwProxy {
            fn name(&self) -> &'static str {
                "proxy"
            }
            fn on_request(&self, head: &RequestHead) {
                self.0.on_request(head);
            }
            fn on_response(&self, head: &RequestHead, elapsed: Duration) {
                self.0.on_response(head, elapsed);
            }
        }
        let server = Server::new()
            .handler(handler)
            .middleware(MwProxy(Arc::clone(&mw)));

        let _ = roundtrip(&server, b"GET / HTTP/1.0\r\n\r\n").await;

        let events = mw.events.lock().unwrap();
        assert_eq!(
            *events,
            vec!["on_request"],
            "HTTP/1.0 でも打ち切り（finish なし drop）では on_response を \
             呼ばないはず: {events:?}"
        );
    }

    // --- ストリーミング応答への Interceptor::map_response 適用（イシュー #434） ---

    /// ストリーミング応答ヘッドへヘッダを追加する `Interceptor`。実際の
    /// CORS/認可プラグイン等が `map_response` でヘッダ付与する典型例を模す。
    struct AddHeaderInterceptor {
        name: &'static str,
        header: (&'static str, &'static str),
    }
    impl Interceptor for AddHeaderInterceptor {
        fn name(&self) -> &'static str {
            self.name
        }
        fn map_response(&self, _head: &RequestHead, response: Response) -> Response {
            response
                .with_header(self.header.0.to_string(), self.header.1.to_string())
                .expect("test header は検証済み固定値")
        }
    }

    /// `map_response` が返した `Response` の body を差し替える `Interceptor`。
    /// ストリーミング応答では body が反映されず破棄されることを固定する
    /// ためのテスト専用実装。
    struct BodyInjectingInterceptor;
    impl Interceptor for BodyInjectingInterceptor {
        fn name(&self) -> &'static str {
            "body-injecting"
        }
        fn map_response(&self, _head: &RequestHead, mut response: Response) -> Response {
            response.body = b"INJECTED".to_vec();
            response
        }
    }

    /// ストリーミング応答のステータスを 204（bodyless）へ書き換える
    /// `Interceptor`。レスポンス分割回帰テスト用。
    struct RewriteToNoContentInterceptor;
    impl Interceptor for RewriteToNoContentInterceptor {
        fn name(&self) -> &'static str {
            "rewrite-to-204"
        }
        fn map_response(&self, _head: &RequestHead, mut response: Response) -> Response {
            response.status = 204;
            response
        }
    }

    #[tokio::test]
    async fn streaming_map_response_adds_header_to_chunked_head() {
        // `map_response` で追加したヘッダが chunked ヘッドに現れ、チャンク
        // framing・終端チャンクは既存の後方互換テストと同様に不変であること。
        let handler = StreamingHandler {
            status: 200,
            content_type: Some("text/plain"),
            chunks: vec![b"foo", b"bar"],
        };
        let server = Server::new()
            .handler(handler)
            .interceptor(AddHeaderInterceptor {
                name: "add-header",
                header: ("X-Mapped", "1"),
            });
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("X-Mapped: 1\r\n"));
        assert!(response.contains("Transfer-Encoding: chunked\r\n"));
        assert!(response.contains("3\r\nfoo\r\n"));
        assert!(response.contains("3\r\nbar\r\n"));
        assert!(response.ends_with("0\r\n\r\n"));
    }

    #[tokio::test]
    async fn streaming_map_response_body_is_discarded() {
        // `map_response` が非空 body の `Response` を返しても、ワイヤに
        // 現れるのは producer が送ったチャンクのみであること（body 破棄
        // 契約の固定、`crate::interceptor` モジュール doc を参照）。
        let handler = StreamingHandler {
            status: 200,
            content_type: None,
            chunks: vec![b"real-chunk"],
        };
        let server = Server::new()
            .handler(handler)
            .interceptor(BodyInjectingInterceptor);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;

        assert!(!response.contains("INJECTED"));
        assert!(response.contains("a\r\nreal-chunk\r\n"));
        assert!(response.ends_with("0\r\n\r\n"));
    }

    #[tokio::test]
    async fn streaming_map_response_status_rewrite_to_bodyless_suppresses_framing() {
        // 200 → 204 書き換え時、ステータス行が 204・Transfer-Encoding なし・
        // 終端チャンクなしで応答が完結すること（レスポンス分割防止の回帰）。
        let handler = StreamingHandler {
            status: 200,
            content_type: None,
            chunks: vec![b"unreachable"],
        };
        let server = Server::new()
            .handler(handler)
            .interceptor(RewriteToNoContentInterceptor);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 204 No Content\r\n"));
        assert!(!response.contains("Transfer-Encoding"));
        assert!(!response.contains("unreachable"));
        assert!(response.ends_with("\r\n\r\n"));
        assert!(!response.trim_end_matches("\r\n\r\n").ends_with('0'));
    }

    #[tokio::test]
    async fn streaming_map_response_applies_on_http10_head() {
        // HTTP/1.0 経路でも map_response のヘッダ改変が効くこと。
        let handler = StreamingHandler {
            status: 200,
            content_type: Some("text/plain"),
            chunks: vec![b"foo"],
        };
        let server = Server::new()
            .handler(handler)
            .interceptor(AddHeaderInterceptor {
                name: "add-header-http10",
                header: ("X-Mapped-10", "1"),
            });
        let response = roundtrip(&server, b"GET / HTTP/1.0\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("X-Mapped-10: 1\r\n"));
        assert!(response.contains("Connection: close\r\n"));
        assert!(!response.contains("Transfer-Encoding"));
        // HTTP/1.0 は生データを EOF 終端するため chunk framing を持たない。
        assert!(response.ends_with("foo"));
    }

    #[tokio::test]
    async fn streaming_without_interceptor_is_unaffected_by_map_response_wiring() {
        // インターセプタ未登録時のストリーミング応答は従来と同一
        // （後方互換、`crate::interceptor` モジュール doc の
        // pay-for-what-you-use 節と対応）。
        let handler = StreamingHandler {
            status: 200,
            content_type: Some("text/plain"),
            chunks: vec![b"foo", b"bar"],
        };
        let server = Server::new().handler(handler);
        let response = roundtrip(&server, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Transfer-Encoding: chunked\r\n"));
        assert!(response.contains("Content-Type: text/plain\r\n"));
        assert!(response.contains("3\r\nfoo\r\n"));
        assert!(response.contains("3\r\nbar\r\n"));
        assert!(response.ends_with("0\r\n\r\n"));
    }

    // `drain_listener_backlog`（イシュー #501）の単体テスト。実 `TcpListener`
    // を bind し、意図的に `accept()` を呼ばずにクライアント接続することで
    // kernel の accept backlog を決定的に作る。

    /// `TcpListener` を bind し、`n` 本のクライアント接続を確立して backlog に
    /// 滞留させる。テスト側は `accept()` を一切呼ばないため、返した listener
    /// の backlog には確実に `n` 件が滞留している。
    async fn listener_with_backlog(n: usize) -> TcpListener {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind は成功する");
        let addr = listener.local_addr().expect("local_addr は成功する");
        for _ in 0..n {
            // 接続確立後は何も送受信せず即座に drop してよい。3-way
            // handshake が完了した時点で kernel backlog に積まれる。
            let _ = TcpStream::connect(addr).await.expect("connect は成功する");
        }
        // backlog への到着は非同期のため、テストの決定性を保つ短い猶予を
        // 置く（`try_acquire`/`poll_accept` はここでは呼ばない）。
        tokio::time::sleep(Duration::from_millis(50)).await;
        listener
    }

    #[tokio::test]
    async fn drain_backlog_collects_pending_connections() {
        let listener = listener_with_backlog(3).await;
        let connection_limit = Arc::new(Semaphore::new(10));

        let drained = drain_listener_backlog(&listener, &connection_limit, 10).await;

        assert_eq!(drained.len(), 3);
        // 3 件分の permit が消費されたまま drained 側が保持している
        // （呼び出し元が回収した接続をサーブする前提、permit ゲートを
        // 迂回しないフェイルクローズ設計）。
        assert_eq!(connection_limit.available_permits(), 7);
    }

    #[tokio::test]
    async fn drain_backlog_stops_at_permit_exhaustion() {
        let listener = listener_with_backlog(2).await;
        // permit を 1 個しか持たないセマフォ: 2 件目の drain 試行は
        // `try_acquire_owned` が失敗し、そこで即座に打ち切られる
        // （同時接続数上限を迂回しない fail-closed 契約）。
        let connection_limit = Arc::new(Semaphore::new(1));

        let drained = drain_listener_backlog(&listener, &connection_limit, 10).await;

        assert_eq!(drained.len(), 1);
        assert_eq!(connection_limit.available_permits(), 0);
    }

    #[tokio::test]
    async fn drain_backlog_stops_at_max_cap() {
        let listener = listener_with_backlog(2).await;
        let connection_limit = Arc::new(Semaphore::new(10));

        // `max=1` により 2 件目の滞留接続は回収されない（件数上限による
        // 有界性、`REBIND_BACKLOG_DRAIN_LIMIT` の doc を参照）。
        let drained = drain_listener_backlog(&listener, &connection_limit, 1).await;

        assert_eq!(drained.len(), 1);
        assert_eq!(connection_limit.available_permits(), 9);
    }

    #[tokio::test]
    async fn drain_backlog_returns_empty_when_no_pending() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind は成功する");
        let connection_limit = Arc::new(Semaphore::new(10));

        // backlog が空の状態で await してもハングしない
        // （`poll_accept` の最初の `Pending` で即座に打ち切る非ブロッキング
        // 契約、`drain_listener_backlog` の doc「有界性」を参照）。
        let drained = tokio::time::timeout(
            Duration::from_millis(200),
            drain_listener_backlog(&listener, &connection_limit, 10),
        )
        .await
        .expect("backlog 空でもタイムアウトせず即座に返る");

        assert!(drained.is_empty());
        assert_eq!(connection_limit.available_permits(), 10);
    }
}
