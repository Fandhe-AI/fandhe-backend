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
pub trait WsMessageHandler: Send + Sync + 'static {
    /// 診断用のハンドラ名（`UpgradeHandler::name` と同じ流儀）。
    fn name(&self) -> &'static str;

    /// メッセージ受信時に呼ばれ、返信または Close の指示を返す。
    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>>;
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
}
