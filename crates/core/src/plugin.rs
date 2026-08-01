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
//!
//! イシュー #305（CORS プラグイン）で 3 つ目のシーム [`finalize_response`]
//! を追加した。`Middleware::on_response` はレスポンスへの参照を持たない
//! 観測専用契約のため CORS ヘッダ付与に使えず、「レスポンス後処理型」という
//! 新パターンが必要になった（`crates/plugin-cors/src/lib.rs` の crate doc・
//! `docs/design/plugin-boundary.md` の該当節を参照）。イシュー #321
//! （圧縮プラグイン）で本シームの第 2 インスタンスを追加し、複数プラグイン
//! を逐次適用（CORS → 圧縮の順、body を確定させる圧縮を必ず最後に適用）
//! できるよう構成した（`crates/plugin-compression/src/lib.rs` の crate doc
//! を参照）。イシュー #451 で 4 つ目のシーム [`finalize_streaming_head`] を
//! 追加し、`Handler::handle_streaming` によるストリーミング応答のヘッドへ
//! CORS ヘッダ付与を適用できるようにした（`finalize_streaming_head` の
//! doc・`docs/design/plugin-boundary.md` 5.9.7 節を参照）。イシュー #461 で
//! 5 つ目のシーム [`prepare_streaming_compression`] を追加し、body 全体を
//! バッファリングしない専用エンコーダ（[`StreamingBodyEncoder`]）経由で
//! チャンク単位のストリーミング gzip 圧縮を接続した
//! （`prepare_streaming_compression` の doc・`crates/plugin-compression/
//! src/lib.rs` の crate doc「チャンク単位のストリーミング gzip 圧縮」節を
//! 参照）。

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
/// `webrtc-proxy`・`webrtc`・`graphql`・`openapi`・`static` のいずれの feature も
/// 無効時は `server`/`head`/`body` を一切参照せず即座に `None` を返す
/// （`cargo tree` で `fandhe-backend-plugin-webrtc-proxy`・`fandhe-backend-plugin-webrtc`・
/// `fandhe-backend-plugin-graphql`・`fandhe-backend-plugin-openapi`・
/// `fandhe-backend-plugin-static` のいずれも現れないことに加え、本関数自体もコード上
/// ゼロコストであることの根拠）。
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

    // TASK-2.1（#256）: `GET /openapi.json` の静的サービング。`GET /openapi.yaml`
    // （#279、仕様（docs/spec/04-requirements.md）が「json と同等に yaml も提供」と
    // 明記することへの対応）も同一パターンで追加した。イシュー #320 で
    // `server.openapi_enabled(): bool` を `server.openapi_registration(): &OpenApiRegistration`
    // へ差し替え、フレームワーク固定スキーマ（`Embedded`）と利用者アプリ独自
    // スキーマ（`Custom`、`crates/plugin-openapi/src/custom.rs::OpenApiDoc`）の
    // 2 系統を同一分岐で扱う。`Disabled`（既定）時は feature が有効でも常に
    // フォールスルーする（`webrtc-proxy`・`graphql` と同じ設定登録型パターン、
    // `Server::openapi` / `Server::openapi_with` の doc を参照）。json/yaml は
    // `head.target` の完全一致（クエリ付きはフォールスルー）で排他的に分岐し、
    // 両方とも同一の登録状態（`openapi_registration`）を共有する。
    #[cfg(feature = "openapi")]
    {
        use crate::server::OpenApiRegistration;

        if head.method == "GET" && head.target == "/openapi.json" {
            let body = match server.openapi_registration() {
                OpenApiRegistration::Disabled => None,
                OpenApiRegistration::Embedded => Some(
                    fandhe_backend_plugin_openapi::OPENAPI_JSON
                        .as_bytes()
                        .to_vec(),
                ),
                OpenApiRegistration::Custom(doc) => Some(doc.json().to_vec()),
            };
            if let Some(body) = body {
                return Some(Response::new(200, body).with_content_type("application/json"));
            }
        }
        if head.method == "GET" && head.target == "/openapi.yaml" {
            let body = match server.openapi_registration() {
                OpenApiRegistration::Disabled => None,
                OpenApiRegistration::Embedded => Some(
                    fandhe_backend_plugin_openapi::OPENAPI_YAML
                        .as_bytes()
                        .to_vec(),
                ),
                // `with_yaml` 未登録（`yaml()` が `None`）なら既定 `Handler`
                // へフォールスルーする（`OpenApiDoc::with_yaml` の doc を参照）。
                OpenApiRegistration::Custom(doc) => doc.yaml().map(<[u8]>::to_vec),
            };
            if let Some(body) = body {
                return Some(
                    Response::new(200, body)
                        // RFC 9512 が定める YAML の正式メディアタイプ。MIME
                        // スニッフィングの余地を残さないため常に明示する
                        // （`.claude/rules/security.md` A05）。
                        .with_content_type("application/yaml"),
                );
            }
        }
    }

    // イシュー #318: 静的ファイル配信プラグイン。`server.static_files_config()`
    // が `Some`（明示登録済み）の場合のみ `fandhe_backend_plugin_static::try_handle_static`
    // へ委譲する（`graphql`・`openapi` と同じ設定登録型パターン、未登録時は
    // feature が有効でもフォールスルー）。ファイル I/O は
    // `fandhe_backend_plugin_static` 側の `spawn_blocking` に閉じており、
    // 本関数（ひいては `handle_connection` の非同期タスク）を直接ブロック
    // しない（`.claude/rules/coding-rust.md`）。
    //
    // イシュー #419: `StaticFilesConfigBuilder::fallthrough_on_miss` が
    // 有効な設定では、mount 一致でも配信対象を確定できなかった場合に
    // `try_handle_static` が `None` を返す。本関数はその `None` を通常の
    // 「対象外パス」と区別せず同じ経路で下側（既定 `Handler`）へ
    // フォールスルーするため、mount `/` で静的サイトと `Router` の動的
    // ルートを共存させられる。コード変更は不要（既存の `if let Some(...)`
    // が `None` をそのまま透過する）。
    #[cfg(feature = "static")]
    {
        if let Some(config) = server.static_files_config()
            && let Some(response) =
                fandhe_backend_plugin_static::try_handle_static(head, config).await
        {
            return Some(response);
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

/// レスポンス後処理型シーム（イシュー #305、CORS プラグインで新設。
/// イシュー #321 で圧縮プラグインの第 2 インスタンスを追加）。
///
/// `handle_connection_with_permit`（`crates/core/src/server.rs`）が
/// `try_intercept` 応答・既定 `Handler` 応答のいずれかを確定させた直後、
/// keep-alive 再判定・`serialize` の前に呼ぶ（モジュール冒頭の処理フロー
/// doc・本モジュール冒頭 doc の「3 つ目のシーム」を参照）。`try_intercept`
/// 応答（graphql・openapi 等のパスインターセプト型プラグイン応答）にも
/// 既定 `Handler` 応答にも同一の後処理を適用できる、`Handler` ラッパー方式
/// にはない利点を持つ（`docs/design/plugin-boundary.md` の設計比較を参照）。
///
/// 登録済みプラグインへ**逐次適用**する（CORS → 圧縮の順固定。圧縮は
/// body を確定させる後処理のため必ず最後、`crates/plugin-compression/
/// src/lib.rs` の crate doc を参照）。`cors` / `compression` いずれの
/// feature も無効、または対応する `Server::cors` / `Server::compression`
/// が未登録の場合はそれぞれの適用をスキップし、両方スキップ時は
/// `response` を無改変で返す（他のプラグインと同じ「設定登録型」の
/// フォールスルー、pay-for-what-you-use）。
///
/// # プリフライトとの二重付与防止
///
/// CORS プリフライト（`OPTIONS` + `Origin` + `Access-Control-Request-Method`）
/// は `fandhe_backend_routes::Router::options_fallback`
/// （利用者が `fandhe_backend_plugin_cors::preflight_response` を直接配線、
/// イシュー #304）で完結済みのため、本シームは
/// `fandhe_backend_plugin_cors::is_preflight` が `true` を返すリクエストには
/// 何もしない（プリフライト応答へ実リクエスト用のヘッダを重ねて付与しない
/// ようにするための判定）。
///
/// # `RequestGate` 拒否応答・パースエラー応答を通さない設計判断
///
/// 本関数は `handle_connection_with_permit` の中で `RequestGate` 拒否応答・
/// パースエラー応答（400 等）の送出経路とは別の、`try_intercept` /
/// `Handler::handle` の結果にのみ適用される（呼び出し箇所を参照）。拒否
/// 応答は最小情報で返すフェイルクローズ方針を維持するための意図的な設計
/// （`docs/design/plugin-boundary.md` の該当節・`.claude/rules/security.md`
/// を参照）。
///
/// 同期・`.await` なし（ヘッダ検査と `with_header` 呼び出しのみ）で
/// `Middleware` の非同期 I/O 禁止規約（`.claude/rules/coding-rust.md`）とは
/// 独立にコストを抑える。
///
/// # 公開 API 化は不採用（イシュー #462）
///
/// 本関数を含むレスポンス後処理型シーム一式（`finalize_response` /
/// `finalize_streaming_head`）を外部ユーザーへ公開 API 化するかはイシュー #462 で
/// 検討し、不採用と判断した。ユーザー向けのレスポンス改変には
/// [`crate::interceptor::Interceptor::map_response`] という既存の公開拡張点があり、
/// 両者の機能差はほぼ解消しているため（比較・不採用根拠は
/// `docs/design/finalize-seam-public-api.md` を参照）。
pub(crate) fn finalize_response(
    server: &Server,
    head: &RequestHead,
    response: Response,
) -> Response {
    #[allow(unused_mut)]
    let mut response = response;

    #[cfg(feature = "cors")]
    {
        response = apply_cors(server, head, response);
    }

    // イシュー #321: 圧縮は「最終 body を確定させる後処理」のため、他の
    // レスポンス後処理型プラグイン（現状は CORS のみ）より必ず後に適用する
    // （CORS はヘッダのみで body に触れないため実害はないが、規約として
    // 明文化する。`crates/plugin-compression/src/lib.rs` の crate doc を
    // 参照）。
    #[cfg(feature = "compression")]
    {
        if let Some(config) = server.compression_config() {
            response = fandhe_backend_plugin_compression::apply_compression(head, config, response);
        }
    }

    // feature 構成によっては上の cfg ブロックの一部・全部が消え、引数が
    // 未使用になりうる（`try_intercept` と同じ理由。冒頭の doc を参照）。
    // 参照型（`Copy`）の再読み込みは各分岐での使用有無に関わらず安全。
    let _ = (server, head);

    response
}

/// CORS ヘッダ付与ロジック（[`finalize_response`] / [`finalize_streaming_head`]
/// 共通ヘルパ、イシュー #451 で抽出）。
///
/// `server.cors_config()` が `Some`（`Server::cors` 登録済み）かつプリフライト
/// でない場合のみ [`fandhe_backend_plugin_cors::apply_cors_headers`] を適用する。
/// 両呼び出し元で判定条件を重複実装すると乖離のリスクがあるため、`cors`
/// feature 有効時にのみコンパイルされる本関数へ集約する
/// （`.claude/rules/security.md` A01/A05「CORS 設定不備」対策、判定ロジックの
/// 単一情報源化）。
#[cfg(feature = "cors")]
fn apply_cors(server: &Server, head: &RequestHead, response: Response) -> Response {
    if let Some(config) = server.cors_config()
        && !fandhe_backend_plugin_cors::is_preflight(head)
    {
        fandhe_backend_plugin_cors::apply_cors_headers(head, config, response)
    } else {
        response
    }
}

/// ストリーミング応答（[`crate::server::Handler::handle_streaming`]、イシュー
/// #319）のヘッド確定時に適用するレスポンス後処理型シーム（イシュー #451、
/// [`finalize_response`] の第 4 のシーム）。
///
/// `crate::server::write_streaming_response` が `Interceptor::map_response`
/// 適用直後・`head_response.body` クリア前に 1 回だけ呼ぶ（通常応答経路の
/// 「`map_response` の後に `finalize_response`」という順序と揃える、
/// `crate::interceptor` モジュール doc の評価順序一覧を参照）。
///
/// # 適用範囲: CORS のみ（設計判断、イシュー #451）
///
/// `cors` feature 有効かつ `Server::cors` 登録時のみ [`apply_cors`] を適用する。
/// gzip 圧縮は body を確定させる後処理であり、通常応答経路
/// （[`finalize_response`]）の `apply_compression`（body 全体前提）は
/// chunked framing がコアの直接書き出しループを経由するストリーミング設計
/// （bounded mpsc バックプレッシャ・`finish` 省略時は終端チャンクなしで
/// 打ち切りクローズという応答完全性契約、`crate::streaming` モジュール doc）
/// と両立できないため、本関数には接続しない（`crate::interceptor` モジュール
/// doc が `map_response` の body 改変を不採用にした判断と同根、
/// `docs/design/plugin-boundary.md` 5.9.7 節を参照）。
///
/// チャンク単位のストリーミング圧縮（イシュー #461）は body 全体を保持
/// しない専用エンコーダ（[`fandhe_backend_plugin_compression::StreamingGzipEncoder`]）
/// で実現するため、本関数（ヘッドの CORS 付与のみを担う）とは別の第 5 の
/// シーム [`prepare_streaming_compression`] へ切り出した
/// （`crate::server::write_streaming_response` が `finalize_streaming_head`
/// 呼び出し直後・`serialize_chunked_head` 呼び出し前に呼ぶ）。
///
/// `cors` feature 無効時・`Server::cors` 未登録時は `response` を無改変で
/// 返す薄い関数となり、実行時コスト・依存追加をゼロに保つ
/// （pay-for-what-you-use）。同期・`.await` なしで [`finalize_response`] と
/// 同じコスト特性を持つ。
///
/// 公開 API 化は [`finalize_response`] と同じくイシュー #462 で検討のうえ不採用。
/// ユーザー向けのレスポンス改変は [`crate::interceptor::Interceptor::map_response`]
/// を使う（`docs/design/finalize-seam-public-api.md` を参照）。
pub(crate) fn finalize_streaming_head(
    server: &Server,
    head: &RequestHead,
    response: Response,
) -> Response {
    #[allow(unused_mut)]
    let mut response = response;

    #[cfg(feature = "cors")]
    {
        response = apply_cors(server, head, response);
    }

    // feature 無効時は `server`/`head` が未使用になりうる（`finalize_response`
    // と同じ理由）。
    let _ = (server, head);

    response
}

/// [`prepare_streaming_compression`] が返す、ストリーミング応答 body の
/// チャンク単位変換器（イシュー #461）。
///
/// `compression` feature 無効時、または圧縮が確定しなかった場合は
/// identity（入力をそのまま返す）として振る舞う。`crate::server::
/// write_streaming_response` 本体を cfg-free に保つ既存原則（PoC-3、本
/// モジュール冒頭 doc）を守るため、`#[cfg(feature = "compression")]` は
/// 本型の内部にのみ閉じる。
#[cfg(feature = "compression")]
pub(crate) struct StreamingBodyEncoder {
    gzip: Option<fandhe_backend_plugin_compression::StreamingGzipEncoder>,
}

/// `compression` feature 無効時の [`StreamingBodyEncoder`]。フィールドを
/// 持たず、`transform`/`finish` は入力をそのまま透過する薄い型
/// （pay-for-what-you-use）。
#[cfg(not(feature = "compression"))]
pub(crate) struct StreamingBodyEncoder;

#[cfg(feature = "compression")]
impl StreamingBodyEncoder {
    fn identity() -> Self {
        Self { gzip: None }
    }

    fn gzip(encoder: fandhe_backend_plugin_compression::StreamingGzipEncoder) -> Self {
        Self {
            gzip: Some(encoder),
        }
    }

    /// `data` を変換する。圧縮確定時は [`fandhe_backend_plugin_compression::
    /// StreamingGzipEncoder::encode_chunk`] へ委譲し、未確定時は `data` を
    /// そのまま返す。`None` はエンコーダ失敗を意味し、呼び出し元
    /// （`write_streaming_response`）は書き込みエラーと同様に接続クローズを
    /// 行う契約（`crates/plugin-compression/src/lib.rs` crate doc の
    /// 「エンコーダ失敗時は接続クローズ」節を参照）。
    pub(crate) fn transform(&mut self, data: Vec<u8>) -> Option<Vec<u8>> {
        match self.gzip.as_mut() {
            Some(encoder) => encoder.encode_chunk(&data).ok(),
            None => Some(data),
        }
    }

    /// 残余データ（圧縮確定時は gzip trailer 含む）を取り出しつつエンコーダを
    /// 終端する。未確定時は空バイト列（送出不要の意）を返す。
    pub(crate) fn finish(self) -> Option<Vec<u8>> {
        match self.gzip {
            Some(encoder) => encoder.finish().ok(),
            None => Some(Vec::new()),
        }
    }
}

#[cfg(not(feature = "compression"))]
impl StreamingBodyEncoder {
    fn identity() -> Self {
        Self
    }

    pub(crate) fn transform(&mut self, data: Vec<u8>) -> Option<Vec<u8>> {
        Some(data)
    }

    pub(crate) fn finish(self) -> Option<Vec<u8>> {
        Some(Vec::new())
    }
}

/// ストリーミング応答（[`crate::server::Handler::handle_streaming`]）body の
/// チャンク単位圧縮を確定させる、第 5 のシーム（イシュー #461、
/// [`finalize_streaming_head`] の次段）。
///
/// `crate::server::write_streaming_response` が [`finalize_streaming_head`]
/// （CORS ヘッダ付与）呼び出し直後・`Response::serialize_chunked_head`
/// 呼び出し前に 1 回だけ呼ぶ。返す [`StreamingBodyEncoder`] を body 送出
/// ループが `RecvOutcome::Chunk` ごとに適用する。
///
/// `compression` feature 有効かつ `Server::compression` 登録済みの場合のみ
/// [`fandhe_backend_plugin_compression::begin_streaming_compression`] へ
/// 委譲し（`compress_streaming` opt-in 判定は同関数内部が担う、
/// `crates/plugin-compression/src/lib.rs` の doc を参照）、それ以外は
/// ヘッドを無改変で返し identity エンコーダを渡す（`cors`・`graphql` 等と
/// 同じ「設定登録型」フォールスルー、pay-for-what-you-use）。
///
/// # HTTP/1.0 経路には接続しない（設計判断、イシュー #461）
///
/// 呼び出し元は HTTP/1.1 chunked 経路でのみ本関数を呼ぶ契約
/// （`crates/plugin-compression/src/lib.rs` の
/// `begin_streaming_compression` doc「呼び出し契約」節を参照）。HTTP/1.0
/// （EOF 終端）応答は識別子のまま送出する。
pub(crate) fn prepare_streaming_compression(
    server: &Server,
    head: &RequestHead,
    response: Response,
) -> (Response, StreamingBodyEncoder) {
    #[cfg(feature = "compression")]
    {
        if let Some(config) = server.compression_config() {
            let (response, encoder) =
                fandhe_backend_plugin_compression::begin_streaming_compression(
                    head, config, response,
                );
            let encoder = match encoder {
                Some(encoder) => StreamingBodyEncoder::gzip(encoder),
                None => StreamingBodyEncoder::identity(),
            };
            return (response, encoder);
        }
    }

    // feature 構成によっては `server`/`head` が未使用になりうる（他の
    // シームと同じ理由。冒頭の doc を参照）。
    let _ = (server, head);

    (response, StreamingBodyEncoder::identity())
}
