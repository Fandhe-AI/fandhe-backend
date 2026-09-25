//! パスパラメータ付き WS ルーティングの統合テスト（イシュー #677、親 #673）。
//!
//! #674（`PathPattern` 単体）・#675（`WebSocketConfig::with_path_pattern` +
//! `handshake::matches` 照合）・#676（`WsOpenContext::param` 経由のパラメータ
//! 受け渡し）はいずれもパターン単体・設定単体のユニットテスト/doc test に
//! 留まっており、「2 つのパターンを同時登録し、別々の値で接続した
//! クライアントがそれぞれ正しいハンドラ・正しい値に届く」ことを実際に
//! ハンドシェイク〜メッセージ往復まで駆動して確認する統合テストが
//! なかった。本ファイルはその隙間を埋める。
//!
//! # 責務分割（クレート境界に起因、`docs/design/plugin-boundary.md` 6.1 節）
//!
//! `plugin-websocket` は `fandhe-backend-core` に（dev-dependency としても）
//! 依存できない契約のため、複数パターンを登録した際の「登録順に最初に
//! 一致した設定を使う」ディスパッチ・「どの設定にもマッチしなかった接続は
//! 通常の `Handler`（404 等）へフォールスルーする」という 2 つの挙動は
//! コア側（`crates/core/src/plugin.rs::try_handle_upgrade`）の実装であり、
//! 本クレートは所有していない。そのため本ファイルは、本クレートが公開する
//! [`matches()`] / [`handle_upgrade`] という純関数だけを使い、コアの
//! `.find()` ディスパッチを [`dispatch`] として模倣した上で検証する:
//!
//! - 受け入れ基準 1・2（正しいハンドラへの振り分け・正しいパラメータ値）は
//!   本ファイルで完結して検証する
//! - 受け入れ基準 3 のうち本クレートが持つ責務範囲（どの登録パターンも
//!   [`matches()`] を真にしない＝ upgrade 対象外と判定する）も本ファイルで
//!   検証する
//! - 受け入れ基準 3 の後半「実際に通常の HTTP 処理（404 等）になる」という
//!   応答レベルの確認は、実 `Server` + 実 TCP を使わないと観測できない
//!   （`try_handle_upgrade` が `None` を返した後の処理はコア側にある）ため、
//!   `crates/core/tests/websocket_upgrade.rs` 側に委ねる

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_plugin_websocket::handler::{
    WsHandlerError, WsMessage, WsMessageHandler, WsOpenContext, WsOutcome,
};
use fandhe_backend_plugin_websocket::{WebSocketConfig, handle_upgrade, matches};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use tokio::io::AsyncReadExt;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

/// `\r\n\r\n` までを読み切り、応答ヘッド部分を文字列として返す
/// （`handler_e2e.rs` / `handshake_e2e.rs` と同一実装。共有 `tests/common/`
/// は存在しないため既存 3 ファイルと同じ方式を踏襲する）。
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

/// コア側 `try_handle_upgrade` 内の `.find()`（登録順に最初に一致した設定を
/// 使う）ディスパッチを模倣するテスト専用ヘルパ。本クレートは
/// `fandhe-backend-core` に依存できないため、実装そのものを import
/// できずここで同型のロジックを再現する（上記モジュール doc 参照）。
fn dispatch<'a>(
    head: &fandhe_backend_http::request::RequestHead,
    configs: &'a [WebSocketConfig],
) -> Option<&'a WebSocketConfig> {
    configs.iter().find(|c| matches(head, c))
}

/// `on_open` で `ctx.param(param_name)` を読み取り `"{label}:{value}"` を
/// push するハンドラ（`handler.rs` doc test の `PushPageId` 例と同型）。
/// 2 パターン同時登録時にどちらのハンドラが選ばれ、どの値が渡ったかを
/// クライアント側で直接観測できるようにする。
struct PushParamHandler {
    label: &'static str,
    param_name: &'static str,
}

impl WsMessageHandler for PushParamHandler {
    fn name(&self) -> &'static str {
        self.label
    }

    fn on_open(&self, ctx: WsOpenContext) {
        let value = ctx.param(self.param_name).unwrap_or("missing").to_string();
        let tag = format!("{}:{value}", self.label);
        let sender = ctx.sender().clone();
        tokio::spawn(async move {
            let _ = sender.send(WsMessage::Text(tag)).await;
        });
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }
}

/// `request_target` でハンドシェイクを成立させ、クライアント側
/// `WebSocketStream` とサーバ側 `handle_upgrade` タスクを返す
/// （`handler_e2e.rs::spawn_session_with_request_target` と同型）。
async fn spawn_session_for_config(
    config: WebSocketConfig,
    request_target: &str,
) -> (
    WebSocketStream<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), fandhe_backend_plugin_websocket::WsError>>,
) {
    let request = format!(
        "GET {request_target} HTTP/1.1\r\n\
         Host: example.com\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n"
    );
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
    assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

    let client = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    (client, server_task)
}

/// ケース 1（受け入れ基準 1・2）: `/devtools/browser/{id}` と
/// `/devtools/page/{id}` の 2 パターンを同時登録し、それぞれの完全一致
/// パスへ接続したクライアントが、正しいハンドラ（`label`）・正しい
/// パラメータ値で push を受け取ることを確認する。登録順を入れ替えた
/// ケースも合わせて確認し、`.find()` 模倣が「登録順に最初に一致」を
/// 正しく反映することも担保する。
#[tokio::test]
async fn two_patterns_dispatch_to_correct_handler_with_matching_param() {
    let cases: [(Vec<WebSocketConfig>, &str, &str); 4] = [
        (
            vec![
                WebSocketConfig::default()
                    .with_path_pattern("/devtools/browser/{id}")
                    .unwrap()
                    .with_handler(PushParamHandler {
                        label: "browser",
                        param_name: "id",
                    }),
                WebSocketConfig::default()
                    .with_path_pattern("/devtools/page/{id}")
                    .unwrap()
                    .with_handler(PushParamHandler {
                        label: "page",
                        param_name: "id",
                    }),
            ],
            "/devtools/browser/ABC",
            "browser:ABC",
        ),
        (
            vec![
                WebSocketConfig::default()
                    .with_path_pattern("/devtools/browser/{id}")
                    .unwrap()
                    .with_handler(PushParamHandler {
                        label: "browser",
                        param_name: "id",
                    }),
                WebSocketConfig::default()
                    .with_path_pattern("/devtools/page/{id}")
                    .unwrap()
                    .with_handler(PushParamHandler {
                        label: "page",
                        param_name: "id",
                    }),
            ],
            "/devtools/page/XYZ",
            "page:XYZ",
        ),
        // 登録順を逆にしたケース: page を先に登録しても、正しいパスへの
        // 接続は依然として正しいハンドラへ振り分けられる。
        (
            vec![
                WebSocketConfig::default()
                    .with_path_pattern("/devtools/page/{id}")
                    .unwrap()
                    .with_handler(PushParamHandler {
                        label: "page",
                        param_name: "id",
                    }),
                WebSocketConfig::default()
                    .with_path_pattern("/devtools/browser/{id}")
                    .unwrap()
                    .with_handler(PushParamHandler {
                        label: "browser",
                        param_name: "id",
                    }),
            ],
            "/devtools/browser/ABC",
            "browser:ABC",
        ),
        (
            vec![
                WebSocketConfig::default()
                    .with_path_pattern("/devtools/page/{id}")
                    .unwrap()
                    .with_handler(PushParamHandler {
                        label: "page",
                        param_name: "id",
                    }),
                WebSocketConfig::default()
                    .with_path_pattern("/devtools/browser/{id}")
                    .unwrap()
                    .with_handler(PushParamHandler {
                        label: "browser",
                        param_name: "id",
                    }),
            ],
            "/devtools/page/XYZ",
            "page:XYZ",
        ),
    ];

    for (configs, path, expected_tag) in cases {
        let head_bytes = format!(
            "GET {path} HTTP/1.1\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             \r\n"
        );
        let head = match parse_request_head(head_bytes.as_bytes()).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => unreachable!(),
        };

        // 受け入れ基準 1: dispatch がどちらのハンドラを選んだかを
        // config の `handler_name()` で先に確認する。
        let matched = dispatch(&head, &configs).expect("path should match exactly one config");
        let expected_label = expected_tag.split(':').next().unwrap();
        assert_eq!(
            matched.handler_name(),
            expected_label,
            "dispatch が期待と異なるハンドラを選んだ: path={path}"
        );

        // 受け入れ基準 2: 実際にハンドシェイク〜push まで駆動し、
        // クライアントが期待どおりのタグ文字列を受け取ることを確認する。
        let matched = matched.clone();
        let (mut client, server_task) = spawn_session_for_config(matched, path).await;

        let pushed = tokio::time::timeout(std::time::Duration::from_secs(2), client.next())
            .await
            .expect("push should arrive within timeout")
            .expect("stream should not end")
            .expect("frame should not error");
        assert_eq!(pushed, Message::Text(expected_tag.into()));

        client.close(None).await.expect("close");
        let result = server_task.await.unwrap();
        assert!(result.is_ok(), "session should end cleanly: {result:?}");
    }
}

/// ケース 2（受け入れ基準 3、本クレートが持つ責務範囲）: 2 パターンを
/// 登録した状態で、どちらにも一致しないパス（別セグメント・id セグメント
/// 欠落・セグメント過多・無関係パス）は `dispatch` が `None` を返す。
/// これは「どの設定にも一致しない接続は upgrade されず、コア側
/// `try_handle_upgrade` が `None` を返して既定 `Handler`（通常の HTTP
/// 処理、404 等）へフォールスルーする」契約の前提部分にあたる。応答
/// レベルの実証は `crates/core/tests/websocket_upgrade.rs` 側で行う。
#[tokio::test]
async fn unmatched_path_matches_no_registered_pattern() {
    let configs = vec![
        WebSocketConfig::default()
            .with_path_pattern("/devtools/browser/{id}")
            .unwrap()
            .with_handler(PushParamHandler {
                label: "browser",
                param_name: "id",
            }),
        WebSocketConfig::default()
            .with_path_pattern("/devtools/page/{id}")
            .unwrap()
            .with_handler(PushParamHandler {
                label: "page",
                param_name: "id",
            }),
    ];

    for path in [
        "/devtools/other/ABC",         // 別セグメント
        "/devtools/browser/",          // id セグメント欠落
        "/devtools/browser/ABC/extra", // セグメント過多
        "/unrelated",                  // 無関係パス
    ] {
        let head_bytes = format!(
            "GET {path} HTTP/1.1\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             \r\n"
        );
        let head = match parse_request_head(head_bytes.as_bytes()).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => unreachable!(),
        };

        assert!(
            dispatch(&head, &configs).is_none(),
            "path {path} が誤って upgrade 対象と判定された"
        );
    }
}
