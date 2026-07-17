//! プラグインのパスインターセプト型・Upgrade 型シームヘルパー
//! （パスインターセプト型: TASK-2.1 / #18、Upgrade 型: TASK-4.1 / #22）。
//!
//! [`crate::server`] の `handle_connection` が呼ぶ固定シグネチャの委譲窓口を
//! 2 種類集約する。`#[cfg(feature = "...")]` を本モジュールの外へ一切漏らさ
//! ないことで、コアループ本体（`handle_connection`）を feature で分岐させ
//! ない設計規約（`crates/core/src/server.rs` 冒頭の doc、PoC-3）を守る:
//! - [`try_intercept`][]: リクエスト/レスポンス完結型プラグイン（WebRTC
//!   シグナリングプロキシ等）へのパスインターセプト
//! - [`try_handle_upgrade`][]: 長時間接続（WebSocket 等）への完全委譲
//!   （`UpgradeHandler::matches` 成立後、コア側の読み取りバッファ解放
//!   （Conditional Go 条件(1)）を経て呼ばれる）
//!
//! feature が 1 つも有効でない場合、両関数とも即座に `None` を返すだけの
//! 薄い関数となり、実行時コスト・依存・コード・`unsafe` を一切追加しない
//! （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。新しい
//! プラグインを配線する際は、本モジュールへ cfg-gated な分岐を追加する形で
//! 拡張する（`docs/design/plugin-boundary.md` のプラグイン境界パターン・
//! 適用指針を参照）。

use tokio::io::{AsyncRead, AsyncWrite};

use bf_http::request::RequestHead;
use bf_http::response::Response;

use crate::server::Server;

/// 登録済みプラグインへパスインターセプトを試みる。
///
/// `RequestGate` → `UpgradeHandler` の評価を通過し、既定 `Handler::handle` を
/// 呼ぶ直前に `handle_connection` が呼ぶ（`crates/core/src/server.rs` 冒頭の
/// 処理フロー doc を参照）。`Some(response)` は「このプラグインが処理を完結
/// させた」ことを意味し、呼び出し元は既定 `Handler` を呼ばずにこの応答を
/// そのまま送出する。`None` は「対象パスではない、またはプラグイン自体が
/// 無効」であり、呼び出し元は既定 `Handler::handle`（未登録時 404）へ
/// フォールスルーする。
///
/// `webrtc-proxy`・`webrtc` の両 feature が無効時は `server`/`head`/`body` を一切
/// 参照せず即座に `None` を返す（`cargo tree` で `bf-plugin-webrtc-proxy`・
/// `bf-plugin-webrtc` のいずれも現れないことに加え、本関数自体もコード上
/// ゼロコストであることの根拠）。
///
/// 両 feature が同時に有効な場合（`--all-features` CI 構成）は `webrtc-proxy`
/// （別プロセス切り出し型、REQ-8 の MVP 推奨方式）を先に評価する。両方を
/// `Server` に登録していても `webrtc-proxy` 側が `Some` を返した時点で `webrtc`
/// 側（in-process 型、TASK-8.1 / #26）は評価しない（実運用では通常どちらか
/// 片方のみ登録するため、この優先順位が問題になるのは意図的に両方登録した
/// 場合に限る。`crates/core/src/server.rs` の `Server::webrtc_proxy` /
/// `Server::webrtc` の doc を参照）。
pub(crate) async fn try_intercept(
    server: &Server,
    head: &RequestHead,
    body: &[u8],
) -> Option<Response> {
    #[cfg(feature = "webrtc-proxy")]
    {
        if let Some(config) = server.webrtc_proxy_config()
            && let Some(response) =
                bf_plugin_webrtc_proxy::try_handle_rtc_offer(head, body, config).await
        {
            return Some(from_plugin_response(response));
        }
    }

    #[cfg(feature = "webrtc")]
    {
        if let Some(config) = server.webrtc_config()
            && let Some(response) = bf_plugin_webrtc::try_handle_rtc_offer(head, body, config).await
        {
            return Some(response);
        }
    }

    // feature が 1 つも有効でない場合は上の cfg ブロックが丸ごと消え、引数が
    // 未使用になる。警告を出さないための明示的な no-op（いずれかの feature が
    // 有効な場合は上のブロックが全引数を使用するため、このブロック自体も cfg で
    // 排他する）。
    #[cfg(not(any(feature = "webrtc-proxy", feature = "webrtc")))]
    {
        let _ = (server, head, body);
    }

    None
}

/// `bf_plugin_webrtc_proxy::Response`（プラグイン側の中間表現）を
/// [`bf_http::response::Response`] へ変換する。
///
/// `content_type` は [`Response::with_content_type`] が `&'static str` のみを
/// 受け付ける制約に従う。プラグイン側の `content_type` フィールドも
/// `&'static str` に限定されているため、変換経路に外部入力由来の動的文字列が
/// 混入する余地はない（`crates/http/src/response.rs` の doc を参照）。
#[cfg(feature = "webrtc-proxy")]
fn from_plugin_response(response: bf_plugin_webrtc_proxy::Response) -> Response {
    Response::new(response.status, response.body).with_content_type(response.content_type)
}

/// [`UpgradeHandler::matches`][crate::extension::UpgradeHandler::matches] が
/// `true` を返した接続をプラグイン側へ委譲するための、一定シグネチャの
/// Upgrade 型委譲シーム（TASK-4.1 / #22）。
///
/// `crates/core/src/server.rs` の `handle_connection` から、読み取りバッファ
/// 解放（Conditional Go 条件(1)）・残余バイト列（`leftover`）退避の直後に
/// 呼ばれる。戻り値 `Some(stream)` は「委譲されず、呼び出し元が後続処理
/// （フォールバック応答）を続けるべき」ことを意味し、`websocket` feature
/// 無効時・`server.websocket_configs()` が空、または登録済みいずれの設定にも
/// `bf_plugin_websocket::matches` が一致しない時のいずれかで発生する
/// （呼び出し元は 501 を返す、`server.rs` の
/// doc を参照）。`None` は「完全に委譲済みで呼び出し元はこれ以上ストリームに
/// 触れない」ことを意味する。
///
/// `websocket` feature 無効時は `stream`/`head`/`leftover`/`server` を一切
/// 参照せず即座に `Some(stream)` を返し、`bf_plugin_websocket` への依存・
/// 呼び出しコードともバイナリに含まれない（`cargo tree` で確認可能、
/// pay-for-what-you-use）。
///
/// [`bf_plugin_websocket::handle_upgrade`] 内のエラー（ハンドシェイク検証
/// 違反・I/O・プロトコルエラー）は本関数の境界で吸収し、panic としてコア
/// 境界を越えさせない（`.claude/rules/coding-rust.md`）。エラー発生時は
/// 接続を静かにクローズしたものとして扱い `None` を返す（`handle_upgrade`
/// 自体が 400/426 応答の送出、または I/O エラー時の未送出クローズを内部で
/// 行っているため、呼び出し元がさらにフォールバック応答を重ねて送る必要は
/// ない）。
pub(crate) async fn try_handle_upgrade<S>(
    stream: S,
    head: &RequestHead,
    leftover: Vec<u8>,
    server: &Server,
) -> Option<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    #[cfg(feature = "websocket")]
    {
        if let Some(config) = server
            .websocket_configs()
            .iter()
            .find(|config| bf_plugin_websocket::matches(head, config))
        {
            let _ = bf_plugin_websocket::handle_upgrade(stream, head, leftover, config).await;
            return None;
        }
    }

    #[cfg(not(feature = "websocket"))]
    {
        let _ = (head, &leftover, server);
    }

    Some(stream)
}
