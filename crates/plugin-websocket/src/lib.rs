//! `fandhe-backend-plugin-websocket`: WebSocket プラグイン（TASK-4.1 / #22）。
//!
//! 拡張点対応: UpgradeHandler（try_handle_upgrade）
//! （機械可読宣言の規約・許可語彙は `docs/design/dependency-graph-contract.md` 3 節、
//! TASK-13.2 / #50）
//!
//! # 背景・REQ-4 との対応
//!
//! コアの `UpgradeHandler` 拡張点（`crates/core/src/extension.rs`）は
//! 「委譲判定のみ」を担い、実際のアップグレード処理（RFC 6455 ハンドシェイク
//! 検証・101 応答送出・フレーミング）はプラグイン側に閉じる契約
//! （`docs/spec/04-requirements.md` REQ-4）。本クレートはその WebSocket 実装
//! である。
//!
//! # コアへの配線について（循環依存の回避）
//!
//! 本クレート単体は `fandhe-backend-core` に依存しない。コアが本クレート
//! へ `optional = true` + `dep:` 構文の依存を張る（`websocket` feature 有効時
//! のみ）ため、逆方向の依存を張ると循環依存になる
//! （`docs/design/plugin-boundary.md` 6.1 節・`scripts/dep-direction-check.sh`
//! が機械的に検証する）。そのため [`UpgradeHandler`][core-upgrade-handler]
//! trait を実装するアダプタはコア側（`crates/core/src/server.rs`）に置かれ、
//! 本クレートは [`matches()`] / [`handle_upgrade`] という純関数 + [`WebSocketConfig`]
//! のみを公開する。コア側の配線は `crates/core/src/plugin.rs`（Upgrade 型
//! シーム `try_handle_upgrade`）が担う。
//!
//! [core-upgrade-handler]: https://github.com/Fandhe-AI/fandhe-backend/blob/main/crates/core/src/extension.rs
//!
//! # 処理フロー
//!
//! 1. コアが `UpgradeHandler::matches` 相当の判定として [`matches()`] を呼び、
//!    `GET` + 設定パス + `Upgrade: websocket` の粗い判定を行う
//! 2. マッチした接続はコア側で読み取りバッファ解放（Conditional Go 条件(1)）
//!    後、残余バイト列とともに [`handle_upgrade`] へ完全委譲される
//! 3. [`handle_upgrade`] は RFC 6455 4.2.1 の詳細検証（`handshake::validate`）
//!    を行い、成功時は 101 応答、失敗時は 400/426 応答を送出する
//! 4. 101 応答成功後は `tokio-tungstenite` の `WebSocketStream` へフレーミング
//!    処理を委譲し、セッション終了まで面倒を見る（`session::run_session`）。
//!    Text/Binary メッセージは [`handler::WsMessageHandler`]（既定
//!    [`handler::EchoHandler`]、Issue #179）へ委譲され、返り値
//!    （[`handler::WsOutcome`]）に従って返信送出・セッション継続/終了を
//!    決める。`WebSocketConfig::idle_timeout`（既定 60 秒、fail-safe で有効）
//!    が設定されている場合、受信アイドルが続く接続は正常な Close
//!    ハンドシェイクで切断する（リソース枯渇 DoS 対策、Issue #175。詳細は
//!    `session` モジュールの doc を参照）
//! 5. コア（`run_until`）から渡されるキャンセル `Future`（`handle_upgrade`
//!    第 5 引数、イシュー #492）が発火した場合も、アイドルタイムアウトと
//!    同型の正常な Close ハンドシェイク（close code 1001 Going Away）で
//!    切断する。ハンドシェイク開始前に既に発火済みなら 101 応答自体を
//!    送出せず即座に終了する。ハンドシェイク成立後は、受信待ちだけでなく
//!    ユーザーハンドラ実行中・返信/Close 送出中でも即座に打ち切って
//!    分岐する（イシュー #499、詳細は [`handle_upgrade`] の doc・`session`
//!    モジュールの doc を参照）
//!
//! # workspace 内での依存方向
//!
//! `docs/spec/04-requirements.md` REQ-1 / `docs/spec/05-tasks.md` TASK-11.1 の方針に従い、
//! workspace 全体の依存方向は次の一方向を維持する（依存方向: server → routes → http::*）。
//! 本クレートはプラグイン層（`fandhe-backend-plugin-*`）に位置し、コアの拡張点を実装する側であり、
//! コア（`fandhe-backend-core`）・`fandhe-backend-routes` からプラグインへの逆依存は発生しない
//! （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。本クレートの
//! workspace 内 path 依存は `fandhe-backend-http`（下位層の sans-IO パーサ）のみであり、
//! `fandhe-backend-core` には依存しない（上記「コアへの配線について」節を参照）。
//! 依存方向の機械検証は `scripts/dep-direction-check.sh` が担う。
//!
//! # pay-for-what-you-use
//!
//! 依存は `fandhe-backend-http`（`RequestHead` 参照のみ）・`tokio`（`io-util` のみ）・
//! `tokio-tungstenite`（`handshake` feature のみ、TLS 系は無効）・
//! `futures-util`（`WebSocketStream` の Stream/Sink 駆動用）に限定する
//! （詳細は `Cargo.toml` のコメントを参照）。`websocket` feature 無効時は
//! コア（`fandhe-backend-core`）の依存グラフから本クレート自体が除外
//! される（`cargo tree -p fandhe-backend-core` で確認可能）。
//!
//! # キャンセル `Future` の受け渡し（イシュー #492）
//!
//! [`handle_upgrade`] はコアから世代キャンセルシグナル（最終 graceful
//! shutdown・rebind 世代 drain）を通知する `Future` を受け取る
//! （`docs/design/ws-cancellation-propagation.md` 3.2 節 (i)）。委譲境界を
//! `tokio::sync::watch::Receiver` ではなく `Future` として越えることで、
//! 本クレートは `tokio` の `sync` feature を要求しない（本体依存は上記の
//! とおり `io-util`/`time` のみのまま）。統合テスト（`tests/cancellation.rs`）
//! はキャンセルトリガに `tokio::sync::oneshot` を使うため、
//! `[dev-dependencies]` にのみ `sync` feature を追加する（`Cargo.toml` の
//! コメントを参照。本体ビルド・公開依存グラフには影響しない）。

mod config;
mod error;
pub mod handler;
mod handshake;
mod session;

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

pub use config::WebSocketConfig;
pub use error::WsError;

use fandhe_backend_http::request::RequestHead;

/// リクエストが `config` の指すアップグレード対象に該当するかを判定する。
///
/// コア側 `UpgradeHandler` アダプタから呼ばれる（`handshake` モジュール内の
/// 判定処理への薄いラッパー）。
#[must_use]
pub fn matches(head: &RequestHead, config: &WebSocketConfig) -> bool {
    handshake::matches(head, config)
}

/// アップグレード確定後の接続を引き受け、ハンドシェイク検証・101/400/426
/// 応答送出・フレーミング委譲・セッション終了までを行う。
///
/// `leftover` は 101 応答送出前にクライアントから先行到着していた可能性の
/// ある残余バイト列（コア側 `RecvBuffer::unread` 由来。パイプライン済み
/// フレームを取りこぼさないため `WebSocketStream::from_partially_read` へ
/// そのまま引き渡す）。
///
/// 戻り値 `Ok(())` は接続が正常に終了した（Close フレーム受信・EOF・
/// キャンセル発火に伴う正常な Close ハンドシェイク完了等）ことを意味する。
/// `Err` はハンドシェイク検証違反（400/426 応答は送出済み）またはフレーミング
/// 処理中の I/O・プロトコルエラーを意味する。呼び出し元（`crates/core`）は
/// このエラーを panic に変換せず、接続クローズとして扱う契約とする
/// （コア境界を越えて panic させない、`.claude/rules/coding-rust.md`）。
///
/// `cancel` はコアの世代キャンセルシグナル（最終 graceful shutdown・rebind
/// 世代 drain、イシュー #490〜#492）が発火したときに解決する `Future`。
/// キャンセル不要な呼び出し元（テスト等）は `std::future::pending::<()>()`
/// を渡せる。**BREAKING CHANGE**（イシュー #492。0.1.0 系からの移行は
/// `CHANGELOG.md` を参照）:
/// - ハンドシェイク開始前に既に発火済みなら 101 応答を送出せず即座に
///   `Ok(())` で終了する（クライアントへ Switching Protocols を見せない）
/// - ハンドシェイク応答（101/400/426）の書き込み中に発火した場合も打ち切る
///   （停滞した slow client でも有界時間で解放するため）
/// - セッション確立後に発火した場合は、`config.idle_timeout` 発火時と同型の
///   正常な Close ハンドシェイク（close code 1001 Going Away・固定 reason）
///   を試み、`session` モジュール内定数 `CLOSE_GRACE`（10 秒）を上限に
///   打ち切る。受信待ちだけでなく、ユーザーハンドラ実行中・返信/Close
///   送出中でも即座に打ち切って分岐する（イシュー #499。ハンドラ `Future`
///   の中断安全性契約は [`handler::WsMessageHandler::on_message`] の doc
///   を参照。詳細は `session` モジュールの doc を参照）
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade, matches};
///
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
/// let config = WebSocketConfig::default();
/// assert!(matches(&head, &config));
///
/// // 実ソケットの代わりに duplex を使い、クライアント側が Close を即送信する
/// // ことでハンドシェイク成立後にセッションが正常終了する経路を確認する。
/// let (server_side, mut client_side) = tokio::io::duplex(4096);
/// use tokio::io::AsyncWriteExt;
/// tokio::spawn(async move {
///     // Close フレーム（マスク付き、payload なし）を送ってセッションを閉じる。
///     client_side.write_all(&[0x88, 0x80, 0, 0, 0, 0]).await.unwrap();
/// });
///
/// // キャンセル不要な呼び出しは `std::future::pending` を渡す。
/// let result = handle_upgrade(
///     server_side,
///     &head,
///     Vec::new(),
///     &config,
///     std::future::pending::<()>(),
/// )
/// .await;
/// assert!(result.is_ok());
/// # }
/// ```
pub async fn handle_upgrade<S, C>(
    mut stream: S,
    head: &RequestHead,
    leftover: Vec<u8>,
    config: &WebSocketConfig,
    cancel: C,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    let mut cancel = std::pin::pin!(cancel);

    // ハンドシェイク開始前に一度だけ非ブロッキングでキャンセル済みかを
    // 確認する。`crates/core/src/plugin.rs` の中間実装（#491）が採用していた
    // 「キャンセルを最優先でポーリングする」順序を踏襲し、101 応答を送出した
    // 直後にハードクローズする TOCTOU を避ける（発火済みなら 101 を一切
    // 送出しない）。
    let already_cancelled =
        std::future::poll_fn(|cx| Poll::Ready(cancel.as_mut().poll(cx).is_ready())).await;
    if already_cancelled {
        return Ok(());
    }

    let validated = match handshake::validate(head) {
        Ok(validated) => validated,
        Err(WsError::UnsupportedVersion) => {
            write_racing_cancel(&mut stream, cancel.as_mut(), &handshake::serialize_426()).await?;
            return Err(WsError::UnsupportedVersion);
        }
        Err(err @ WsError::InvalidHandshake(_)) => {
            write_racing_cancel(&mut stream, cancel.as_mut(), &handshake::serialize_400()).await?;
            return Err(err);
        }
        Err(err) => return Err(err),
    };

    // 101 応答の書き込み自体も cancel と race させる。停滞した slow client
    // （書き込みバッファが埋まり `write_all` が進まない）に対しても有界時間
    // で解放できるようにするため（上記関数 doc「BREAKING CHANGE」節を参照）。
    let cancelled_before_101 = write_racing_cancel(
        &mut stream,
        cancel.as_mut(),
        &handshake::serialize_101(&validated.accept_key),
    )
    .await?;
    if cancelled_before_101 {
        return Ok(());
    }

    session::run_session(stream, leftover, config, cancel).await
}

/// `bytes` を `stream` へ書き込みつつ `cancel` と race する。キャンセルが
/// 先に発火した場合は書き込みを打ち切って `Ok(true)` を返し（呼び出し元は
/// 応答が完了しなかったものとして扱う）、書き込みが完了した場合は
/// `Ok(false)` を返す。書き込み自体の I/O エラーは [`WsError`] へ変換して
/// 伝播する（[`handle_upgrade`] のハンドシェイク応答書き込み 3 箇所
/// （101/400/426）で共有する）。
async fn write_racing_cancel<S, C>(
    stream: &mut S,
    cancel: Pin<&mut C>,
    bytes: &[u8],
) -> Result<bool, WsError>
where
    S: AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    match race_cancel(cancel, stream.write_all(bytes)).await {
        None => Ok(true),
        Some(Ok(())) => Ok(false),
        Some(Err(err)) => Err(err.into()),
    }
}

/// `cancel` と `fut` を race させる。`cancel` を最優先でポーリングし
/// （TOCTOU 回避、上記 doc を参照）、先に発火すれば `None` を返す。`fut` が
/// 先に完了すれば `Some(fut の出力)` を返す。`session` モジュールの受信
/// ループ・アイドルタイムアウト経路と本モジュールのハンドシェイク応答
/// 書き込みで共有する手動 race ヘルパー（`std::future::poll_fn` +
/// `std::pin::pin!` のみで構成、追加依存なし。`crates/core/src/plugin.rs`
/// の中間実装が使っていたパターンと同型）。
pub(crate) async fn race_cancel<C, F>(mut cancel: Pin<&mut C>, fut: F) -> Option<F::Output>
where
    C: Future<Output = ()>,
    F: Future,
{
    let mut fut = std::pin::pin!(fut);
    std::future::poll_fn(|cx| {
        if cancel.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        if let Poll::Ready(output) = fut.as_mut().poll(cx) {
            return Poll::Ready(Some(output));
        }
        Poll::Pending
    })
    .await
}
