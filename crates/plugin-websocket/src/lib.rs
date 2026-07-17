//! `bf-plugin-websocket`: WebSocket プラグイン（TASK-4.1 / #22）。
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
//! 本クレート単体は `backend-framework-core` に依存しない。コアが本クレート
//! へ `optional = true` + `dep:` 構文の依存を張る（`websocket` feature 有効時
//! のみ）ため、逆方向の依存を張ると循環依存になる
//! （`docs/design/plugin-boundary.md` 6.1 節・`scripts/dep-direction-check.sh`
//! が機械的に検証する）。そのため [`UpgradeHandler`][core-upgrade-handler]
//! trait を実装するアダプタはコア側（`crates/core/src/server.rs`）に置かれ、
//! 本クレートは [`matches()`] / [`handle_upgrade`] という純関数 + [`WebSocketConfig`]
//! のみを公開する。コア側の配線は `crates/core/src/plugin.rs`（Upgrade 型
//! シーム `try_handle_upgrade`）が担う。
//!
//! [core-upgrade-handler]: https://github.com/Fandhe-AI/backend-framework/blob/main/crates/core/src/extension.rs
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
//!    処理を委譲し、セッション終了まで面倒を見る（`session::run_echo_session`）
//!
//! # workspace 内での依存方向
//!
//! `docs/spec/04-requirements.md` REQ-1 / `docs/spec/05-tasks.md` TASK-11.1 の方針に従い、
//! workspace 全体の依存方向は次の一方向を維持する（依存方向: server → routes → http::*）。
//! 本クレートはプラグイン層（`bf-plugin-*`）に位置し、コアの拡張点を実装する側であり、
//! コア（`backend-framework-core`）・`bf-routes` からプラグインへの逆依存は発生しない
//! （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。本クレートの
//! workspace 内 path 依存は `bf-http`（下位層の sans-IO パーサ）のみであり、
//! `backend-framework-core` には依存しない（上記「コアへの配線について」節を参照）。
//! 依存方向の機械検証は `scripts/dep-direction-check.sh` が担う。
//!
//! # pay-for-what-you-use
//!
//! 依存は `bf-http`（`RequestHead` 参照のみ）・`tokio`（`io-util` のみ）・
//! `tokio-tungstenite`（`handshake` feature のみ、TLS 系は無効）・
//! `futures-util`（`WebSocketStream` の Stream/Sink 駆動用）に限定する
//! （詳細は `Cargo.toml` のコメントを参照）。`websocket` feature 無効時は
//! コア（`backend-framework-core`）の依存グラフから本クレート自体が除外
//! される（`cargo tree -p backend-framework-core` で確認可能）。

mod config;
mod error;
mod handshake;
mod session;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

pub use config::WebSocketConfig;
pub use error::WsError;

use bf_http::request::RequestHead;

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
/// 戻り値 `Ok(())` は接続が正常に終了した（Close フレーム受信・EOF 等）
/// ことを意味する。`Err` はハンドシェイク検証違反（400/426 応答は送出済み）
/// またはフレーミング処理中の I/O・プロトコルエラーを意味する。呼び出し元
/// （`crates/core`）はこのエラーを panic に変換せず、接続クローズとして
/// 扱う契約とする（コア境界を越えて panic させない、
/// `.claude/rules/coding-rust.md`）。
///
/// # Examples
///
/// ```
/// use bf_http::request::{ParseOutcome, parse_request_head};
/// use bf_plugin_websocket::{WebSocketConfig, handle_upgrade, matches};
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
/// let result = handle_upgrade(server_side, &head, Vec::new(), &config).await;
/// assert!(result.is_ok());
/// # }
/// ```
pub async fn handle_upgrade<S>(
    mut stream: S,
    head: &RequestHead,
    leftover: Vec<u8>,
    config: &WebSocketConfig,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let validated = match handshake::validate(head) {
        Ok(validated) => validated,
        Err(WsError::UnsupportedVersion) => {
            stream.write_all(&handshake::serialize_426()).await?;
            return Err(WsError::UnsupportedVersion);
        }
        Err(err @ WsError::InvalidHandshake(_)) => {
            stream.write_all(&handshake::serialize_400()).await?;
            return Err(err);
        }
        Err(err) => return Err(err),
    };

    stream
        .write_all(&handshake::serialize_101(&validated.accept_key))
        .await?;

    session::run_echo_session(stream, leftover, config).await
}
