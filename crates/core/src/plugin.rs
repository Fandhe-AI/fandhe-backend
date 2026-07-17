//! プラグインのパスインターセプト型シームヘルパー（TASK-2.1 / #18）。
//!
//! [`crate::server`] の `handle_connection` が既定 `Handler::handle` を呼ぶ
//! 直前に無条件で呼び出す固定シグネチャの委譲窓口。`#[cfg(feature = "...")]`
//! を本モジュールの外へ一切漏らさないことで、コアループ本体
//! （`handle_connection`）を feature で分岐させない設計規約
//! （`crates/core/src/server.rs` 冒頭の doc、PoC-3）を守る。
//!
//! feature が 1 つも有効でない場合、本モジュールは即座に `None` を返す
//! だけの薄い関数となり、実行時コスト・依存・コード・`unsafe` を一切追加
//! しない（pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）。
//! 新しいプラグインをパスインターセプト型で配線する際は、本モジュールへ
//! cfg-gated な分岐を追加する形で拡張する（`docs/design/plugin-boundary.md`
//! のプラグイン境界パターン・適用手順を参照）。

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
