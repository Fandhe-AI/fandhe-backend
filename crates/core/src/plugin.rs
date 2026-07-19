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
use tokio::sync::OwnedSemaphorePermit;

use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;

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
/// `webrtc-proxy`・`webrtc`・`graphql` のいずれの feature も無効時は
/// `server`/`head`/`body` を一切参照せず即座に `None` を返す（`cargo tree` で
/// `fandhe-backend-plugin-webrtc-proxy`・`fandhe-backend-plugin-webrtc`・`fandhe-backend-plugin-graphql` のいずれも
/// 現れないことに加え、本関数自体もコード上ゼロコストであることの根拠）。
/// `graphql` feature は TASK-2.4（#21）で追加した第 2 のプラグイン境界
/// インスタンスであり、TASK-5.1（#38）で実 GraphQL 実行へ差し替えた
/// （`crates/plugin-graphql` の doc を参照）。`webrtc-proxy`・`webrtc` と
/// 同じ設定登録型パターンのため、スキーマ未登録時は feature が有効でも
/// フォールスルーする。
///
/// `webrtc-proxy`・`webrtc` が同時に有効な場合（`--all-features` CI 構成）は
/// `webrtc-proxy`（別プロセス切り出し型、REQ-8 の MVP 推奨方式）を先に評価する。
/// 両方を `Server` に登録していても `webrtc-proxy` 側が `Some` を返した時点で
/// `webrtc` 側（in-process 型、TASK-8.1 / #26）は評価しない（実運用では通常
/// どちらか片方のみ登録するため、この優先順位が問題になるのは意図的に両方
/// 登録した場合に限る。`crates/core/src/server.rs` の `Server::webrtc_proxy` /
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
                fandhe_backend_plugin_webrtc_proxy::try_handle_rtc_offer(head, body, config).await
        {
            return Some(from_plugin_response(response));
        }
    }

    #[cfg(feature = "webrtc")]
    {
        if let Some(config) = server.webrtc_config()
            && let Some(response) =
                fandhe_backend_plugin_webrtc::try_handle_rtc_offer(head, body, config).await
        {
            return Some(response);
        }
    }

    // TASK-5.1（#38）: REQ-2 の「2 種のプラグイン着脱」受け入れ基準を実証した
    // TASK-2.4（#21）の固定応答スタブを実 GraphQL 実行へ差し替えた
    // （`crates/plugin-graphql` の doc を参照）。webrtc-proxy と同型の
    // 設定登録型パターン（`server.graphql_config()` が `Some` のときのみ実行）
    // を踏襲し、未登録時は feature が有効でもフォールスルーする。
    #[cfg(feature = "graphql")]
    {
        if let Some(config) = server.graphql_config()
            && let Some(response) =
                fandhe_backend_plugin_graphql::try_handle_graphql(head, body, config).await
        {
            return Some(from_graphql_response(response));
        }
    }

    // feature 構成によっては上の cfg ブロックの一部・全部が消え、引数が未使用
    // になりうる（webrtc-proxy 無効時は `server`・`body`、両方無効時は
    // `head` も未使用）。参照型（`Copy`）の再読み込みは各分岐での使用有無に
    // 関わらず安全なため、無条件の no-op で一括して警告を防ぐ。
    let _ = (server, head, body);

    None
}

/// `fandhe_backend_plugin_webrtc_proxy::Response`（プラグイン側の中間表現）を
/// [`fandhe_backend_http::response::Response`] へ変換する。
///
/// `content_type` は [`Response::with_content_type`] が `&'static str` のみを
/// 受け付ける制約に従う。プラグイン側の `content_type` フィールドも
/// `&'static str` に限定されているため、変換経路に外部入力由来の動的文字列が
/// 混入する余地はない（`crates/http/src/response.rs` の doc を参照）。
#[cfg(feature = "webrtc-proxy")]
fn from_plugin_response(response: fandhe_backend_plugin_webrtc_proxy::Response) -> Response {
    Response::new(response.status, response.body).with_content_type(response.content_type)
}

/// [`UpgradeHandler::matches`][crate::extension::UpgradeHandler::matches] が
/// `true` を返した接続をプラグイン側へ委譲するための、一定シグネチャの
/// Upgrade 型委譲シーム（TASK-4.1 / #22、専用タスク再 spawn は
/// TASK-4.2 / #23【条件(1)】）。
///
/// `crates/core/src/server.rs` の `handle_connection` から、読み取りバッファ
/// 解放（Conditional Go 条件(1)）・残余バイト列（`leftover`）退避の直後に
/// 呼ばれる。戻り値 `Some(stream)` は「委譲されず、呼び出し元が後続処理
/// （フォールバック応答）を続けるべき」ことを意味し、`websocket` feature
/// 無効時・`server.websocket_configs()` が空、または登録済みいずれの設定にも
/// `fandhe_backend_plugin_websocket::matches` が一致しない時のいずれかで発生する
/// （呼び出し元は 501 を返す、`server.rs` の
/// doc を参照）。`None` は「完全に委譲済みで呼び出し元はこれ以上ストリームに
/// 触れない」ことを意味する。
///
/// # 委譲後の専用タスク再 spawn（TASK-4.2 / #23）
///
/// マッチ確定時、ハンドシェイク + メッセージハンドラループ（既定エコー、
/// `WebSocketConfig::with_handler` でユーザー定義ハンドラへ差し替え可能。
/// Issue #179）を提供する `fandhe_backend_plugin_websocket::handle_upgrade`
/// を呼び出し元の `handle_connection` タスク内でインラインに await せず、
/// `tokio::spawn` した専用タスクへ完全に切り離す。`handle_connection` の
/// tokio タスクは `read_request`・応答直列化等を含む大きなステートマシンで
/// あり、そのままインライン await すると WS 接続の生存中ずっとこの大きな
/// ステートマシンがメモリ上に残ってしまう（PoC-7 実測、接続あたり RSS が
/// axum 比 155.2% となり Conditional Go 条件(1) の成功基準 110% を満たさな
/// かった残差要因）。マッチ確定と同時に `handle_connection` 側は即座に
/// `return`（呼び出し元で `None` を受けて終了）し、大きな future を解放
/// する。新タスクは WS セッションのみを載せた小さな future になる
/// （`docs/design/plugin-boundary.md` 5 節「委譲後の専用タスク再 spawn」
/// パターンを参照）。
///
/// # `permit` の契約（DoS 対策の維持、TASK-4.2 / #23）
///
/// 同時接続数上限は [`BoundServer::run`][crate::server::BoundServer::run] が
/// 保持する `OwnedSemaphorePermit` で強制される。素朴に再 spawn すると、
/// 元の `handle_connection` タスクは即座に終了して permit を解放してしまい、
/// 長時間生存する WS セッションが `max_connections` の上限強制から漏れる
/// （リソース枯渇 DoS のリグレッション、`.claude/rules/security.md`）。
/// これを避けるため、呼び出し元は `permit`（`&mut Option<OwnedSemaphorePermit>`）
/// を渡し、本関数はマッチ確定時に [`Option::take`] で permit の所有権を奪って
/// 新タスクへ move する。呼び出し元に残るのは `None` であり、`handle_connection`
/// 側の通常の permit drop 経路（呼び出し元のラッパー、`server.rs` を参照）は
/// 何も解放しない。permit は新タスク内で WS セッション終了まで保持され、
/// セッション終了と同時に（タスクの戻りで）解放される。
///
/// `websocket` feature 無効時は `stream`/`head`/`leftover`/`server`/`permit`
/// を一切参照せず即座に `Some(stream)` を返し、`fandhe_backend_plugin_websocket` への
/// 依存・呼び出しコード・`tokio::spawn` 呼び出しともバイナリに含まれない
/// （`cargo tree` で確認可能、pay-for-what-you-use）。
///
/// [`fandhe_backend_plugin_websocket::handle_upgrade`] 内のエラー（ハンドシェイク検証
/// 違反・I/O・プロトコルエラー）は新タスクの境界で吸収し、panic としてコア
/// 境界を越えさせない（`.claude/rules/coding-rust.md`）。エラー発生時は
/// 接続を静かにクローズしたものとして扱う（`handle_upgrade` 自体が 400/426
/// 応答の送出、または I/O エラー時の未送出クローズを内部で行っているため、
/// 呼び出し元がさらにフォールバック応答を重ねて送る必要はない）。新タスク内
/// で panic しても `tokio::spawn` のタスク境界で隔離され、コアの accept
/// ループへは波及しない。
pub(crate) async fn try_handle_upgrade<S>(
    stream: S,
    head: &RequestHead,
    leftover: Vec<u8>,
    server: &Server,
    permit: &mut Option<OwnedSemaphorePermit>,
) -> Option<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    #[cfg(feature = "websocket")]
    {
        if let Some(config) = server
            .websocket_configs()
            .iter()
            .find(|config| fandhe_backend_plugin_websocket::matches(head, config))
        {
            let config = config.clone();
            let head = head.clone();
            // permit の所有権をセッションタスクへ move する（上の doc の
            // 「permit の契約」を参照）。呼び出し元には `None` が残り、
            // 通常の permit drop 経路は何も解放しない。
            let permit = permit.take();
            tokio::spawn(async move {
                // セッションが終了する（このタスクの future が完了する）まで
                // permit を保持し、`max_connections` のカウントから漏れない
                // ようにする。
                let _permit = permit;
                let _ = fandhe_backend_plugin_websocket::handle_upgrade(
                    stream, &head, leftover, &config,
                )
                .await;
            });
            return None;
        }
    }

    #[cfg(not(feature = "websocket"))]
    {
        let _ = (head, &leftover, server, &permit);
    }

    Some(stream)
}

/// `fandhe_backend_plugin_graphql::Response`（プラグイン側の中間表現）を
/// [`fandhe_backend_http::response::Response`] へ変換する。[`from_plugin_response`] と
/// 同一の変換原則（`content_type` は `&'static str` 限定）に従う。
#[cfg(feature = "graphql")]
fn from_graphql_response(response: fandhe_backend_plugin_graphql::Response) -> Response {
    Response::new(response.status, response.body).with_content_type(response.content_type)
}
