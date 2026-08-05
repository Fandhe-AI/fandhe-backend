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
//! 参照）。イシュー #468 で [`finalize_response`] を `async fn` へ変更し、
//! body 長がしきい値以上の圧縮を `tokio::task::spawn_blocking` へ切り離す
//! ようにした（巨大応答の gzip 圧縮が接続タスクの tokio ワーカスレッドを
//! 長時間占有し他タスクのテールレイテンシへ波及する問題への対処。実測・
//! 採否根拠は `docs/design/plugin-boundary.md` 5.10.7 節を参照）。

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::OwnedSemaphorePermit;

use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;

use crate::server::Server;

/// 世代（`shutdown_flag` + `CancelSafeJoinSet` のペア、`crates/core/src/
/// server.rs` の `BoundServer::run_until` を参照）ごとに 1 個保持する
/// キャンセル発火源（イシュー #491、`docs/design/ws-cancellation-propagation.md`
/// 5.1 節）。
///
/// 最終 graceful shutdown（イシュー #313）・rebind 世代 drain（イシュー
/// #485/#488）の両経路が同一の世代構造体（同一 `watch::Sender`）を発火源
/// として共有する（設計 5.1 節）。`websocket` feature 無効時はフィールドを
/// 持たない ZST になり、`fire`・`subscribe` は no-op（pay-for-what-you-use、
/// 設計 6.1 節）。
pub(crate) struct GenerationCancel {
    #[cfg(feature = "websocket")]
    tx: tokio::sync::watch::Sender<bool>,
}

impl GenerationCancel {
    /// 新しい世代のキャンセル発火源を構築する。`run_until` が世代交代の
    /// たび（rebind 時・`BoundServer::bind` 時）に呼ぶ。`websocket` feature
    /// 無効時は `watch::channel` を生成せずゼロコストで戻る。
    pub(crate) fn new() -> Self {
        #[cfg(feature = "websocket")]
        {
            let (tx, _rx) = tokio::sync::watch::channel(false);
            Self { tx }
        }
        #[cfg(not(feature = "websocket"))]
        {
            Self {}
        }
    }

    /// この世代のキャンセルを発火する。最終 shutdown は
    /// `current_shutdown_flag.store(true, ...)` の直後、rebind は
    /// `spawn_generation_drain` が生成する drain タスクの冒頭で 1 回だけ
    /// 呼ぶ（設計 5.2 節「drain 開始時に 1 回だけ発火する」）。複数回
    /// 呼んでも冪等（`true` を送り続けるだけ）。
    pub(crate) fn fire(&self) {
        #[cfg(feature = "websocket")]
        {
            // `watch::Sender::send` は「アクティブな受信側が 0 件」の場合、
            // 内部値を更新せず `Err` を返す（実測で確認: `Server::bind` 直後
            // でまだ 1 本も接続が委譲されていない世代・全 WS セッションが
            // 既に終了済みの世代では受信側が 0 件になりうる）。これでは
            // 「fire() が先に起きてから subscribe した新規レシーバが現在値
            // として true を観測できる」という設計 3.1 節の前提（値は常に
            // 最新を保持し続ける）が成立しない。`send_replace` は受信側の
            // 有無に関わらず内部値を無条件に更新するため、こちらを使う
            // （戻り値の旧値は使わないため破棄）。
            self.tx.send_replace(true);
        }
    }

    /// この世代に属する 1 コネクション分のキャンセルハンドル（[`UpgradeCancel`]）
    /// を発行する。`run_until` が接続を spawn する際に呼び、
    /// `handle_connection_with_permit` → `try_handle_upgrade` へ伝搬する。
    pub(crate) fn handle(&self) -> UpgradeCancel {
        #[cfg(feature = "websocket")]
        {
            UpgradeCancel {
                rx: Some(self.tx.subscribe()),
            }
        }
        #[cfg(not(feature = "websocket"))]
        {
            UpgradeCancel {}
        }
    }
}

/// 1 コネクション分のキャンセル購読ハンドル（[`GenerationCancel::handle`]
/// が発行、イシュー #491）。`Clone` 可能（`watch::Receiver` は `Clone`）。
///
/// [`handle_connection`][crate::server::handle_connection] /
/// [`handle_connection_with_peer_addr`][crate::server::handle_connection_with_peer_addr]
/// （`BoundServer::run_until` を経由しない直接呼び出し）は
/// [`UpgradeCancel::disabled`] を渡す。これらの経路には世代の概念がなく、
/// 発火するキャンセルシグナル自体が存在しないため（`handle_connection` の
/// doc「シャットダウンなし」と同じ扱い）。
pub(crate) struct UpgradeCancel {
    #[cfg(feature = "websocket")]
    rx: Option<tokio::sync::watch::Receiver<bool>>,
}

impl UpgradeCancel {
    /// 発火しないハンドル（`BoundServer::run_until` を経由しない直接呼び出し
    /// 向け、上記型 doc を参照）。
    pub(crate) fn disabled() -> Self {
        #[cfg(feature = "websocket")]
        {
            Self { rx: None }
        }
        #[cfg(not(feature = "websocket"))]
        {
            Self {}
        }
    }

    /// キャンセル発火を待つ `Future` へ変換する（設計 3.2 節 (i)。委譲境界
    /// 越しに `watch::Receiver` を直接渡さず `Future` として渡すことで、
    /// `plugin-websocket` 側に `tokio` の `sync` feature を要求しない）。
    ///
    /// # TOCTOU 回避（設計 3.1 節「消費側の必須実装」）
    ///
    /// 内部で [`tokio::sync::watch::Receiver::wait_for`] を使う。単純な
    /// `changed()` は「レシーバ生成後に届いた変更」しか検出できず、
    /// `fire()` が先に呼ばれてから本メソッドで新規 subscribe した場合に
    /// 永久に解決しなくなる（`GenerationCancel::handle` が `fire()` より
    /// 後に呼ばれる競合が実際に起こりうる: 委譲確定はリクエスト処理と
    /// 非同期に進むため）。`wait_for(|&v| v)` は現在値を先に確認してから
    /// 待つため、生成時点で既に `true` ならその場で即解決し、取りこぼしが
    /// 起きない。
    ///
    /// `disabled()`（`rx = None`）の場合は `std::future::pending()` を返し、
    /// 永久に解決しない（世代を持たない呼び出し経路向け、上記型 doc を
    /// 参照）。
    ///
    /// `websocket` feature 無効時は [`try_handle_upgrade`] が本メソッドを
    /// 呼ばない（websocket 分岐自体が消える）ため `#[cfg(feature =
    /// "websocket")]` で閉じ、dead code 警告を防ぐ（pay-for-what-you-use）。
    #[cfg(feature = "websocket")]
    pub(crate) fn into_future(
        self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        match self.rx {
            Some(mut rx) => Box::pin(async move {
                // 送信側（`GenerationCancel`）が全 drop された場合
                // （通常運用では起こらない: `run_until` が世代交代まで
                // 保持し続ける）、`wait_for` は `Err` を返す。これは
                // 「世代自体が終了した」ことを意味するため、キャンセル
                // なし扱いにはせず、キャンセル済みとして即座に解決する
                // 安全側の処理とする（設計 8 節「シグナル取りこぼし」）。
                let _ = rx.wait_for(|&v| v).await;
            }),
            None => Box::pin(std::future::pending()),
        }
    }
}

/// WebRTC セッション（`RTCPeerConnection`）の有界 drain シーム（イシュー #498、
/// `docs/design/ws-cancellation-propagation.md` 10 節「WS 以外への水平展開」）。
///
/// [`GenerationCancel`]/[`UpgradeCancel`]（WS 委譲タスク向け、`watch` チャネルによる
/// 世代別購読）とは独立した新シームとして追加する。`plugin-webrtc` はパスインターセプト
/// 型プラグイン（`try_intercept` から呼ばれ、リクエスト/レスポンスは完結するがセッション
/// 自体はプロセス内レジストリ `WebRtcConfig::registry` で世代非依存に生存し続ける）で
/// あり、`UpgradeHandler` のような 1 コネクション 1 世代購読の構造を持たないため、
/// 「発火時点のアクティブ接続スナップショットを close する」レジストリ drain 型で実現
/// する（`fandhe_backend_plugin_webrtc::{close_active_peers, drain_for_shutdown}` へ
/// 委譲）。`webrtc` feature 無効時・`Server::webrtc` 未登録時はいずれもフィールドを
/// 持たない ZST/no-op になる（pay-for-what-you-use、[[pay-for-what-you-use]]）。
///
/// `Clone` は `WebRtcConfig` 自体が `Arc` ベースで安価に共有可能なことに従う
/// （`run_until` が世代交代のたびに `spawn_generation_drain` へ複製を渡し、自身は
/// 最終 shutdown まで元のインスタンスを保持し続けるために必要、`GenerationCancel`
/// が世代ごとに新規構築されるのとは対照的な「単一インスタンスを使い回す」設計）。
#[derive(Clone)]
pub(crate) struct SessionDrain {
    #[cfg(feature = "webrtc")]
    config: Option<fandhe_backend_plugin_webrtc::WebRtcConfig>,
}

/// 1 接続あたりの `RTCPeerConnection::close()` 打ち切りタイムアウト（イシュー #498）。
///
/// `run_until` の「grace + ε 以内に必ず戻る」保証（既存の permit 回収タイムアウト、
/// `server.rs` の doc を参照）を [`SessionDrain::fire`] 自体が妨げないよう、`fire` は
/// この定数を使う drain 処理を detached タスクへ切り離す（`fire` 自体は同期関数のまま
/// 即座に戻る）。WS 側の `CLOSE_GRACE`（`crates/plugin-websocket`）と役割は同様だが、
/// DTLS/SCTP のクローズシーケンスは Close frame 応答待ちと性質が異なるため独立した
/// 定数として定義する。
#[cfg(feature = "webrtc")]
const SESSION_DRAIN_CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl SessionDrain {
    /// `Server` に登録済みの [`fandhe_backend_plugin_webrtc::WebRtcConfig`]（あれば）を
    /// 束縛して構築する。`run_until` が世代交代（rebind）・最終 shutdown のいずれの
    /// タイミングでも呼べるよう、`GenerationCancel` のような世代単位の再構築は行わず
    /// `run_until` の開始時に 1 度だけ構築する（`WebRtcConfig` 自体が世代を跨いで
    /// 共有される設計のため、世代ごとに作り直す必要がない）。
    pub(crate) fn new(server: &Server) -> Self {
        #[cfg(feature = "webrtc")]
        {
            Self {
                config: server.webrtc_config().cloned(),
            }
        }
        #[cfg(not(feature = "webrtc"))]
        {
            let _ = server;
            Self {}
        }
    }

    /// drain を発火する。`is_final` が `true` なら最終 graceful shutdown 相当
    /// （[`fandhe_backend_plugin_webrtc::drain_for_shutdown`]、以降の新規登録を拒否した
    /// うえで既存接続を close）、`false` なら rebind 世代 drain 相当
    /// （[`fandhe_backend_plugin_webrtc::close_active_peers`]、スナップショットの close
    /// のみ）を呼ぶ（両者の使い分け根拠は `docs/design/ws-cancellation-propagation.md`
    /// 10 節を参照）。
    ///
    /// 呼び出し元（`run_until`・`spawn_generation_drain`）をブロックしないよう、実際の
    /// drain 処理は `tokio::spawn` した detached タスクへ切り離す（`GenerationCancel::
    /// fire` が同期的に `watch::Sender::send_replace` するだけで完結するのとは異なり、
    /// 本シームの drain 処理は `RTCPeerConnection::close()` という有界だが非ゼロ時間の
    /// 非同期 I/O を伴うため）。`Server::webrtc` 未登録（`config` が `None`）の場合は
    /// 何もしない。
    pub(crate) fn fire(&self, is_final: bool) {
        #[cfg(feature = "webrtc")]
        {
            if let Some(config) = self.config.clone() {
                tokio::spawn(async move {
                    if is_final {
                        fandhe_backend_plugin_webrtc::drain_for_shutdown(
                            &config,
                            SESSION_DRAIN_CLOSE_TIMEOUT,
                        )
                        .await;
                    } else {
                        fandhe_backend_plugin_webrtc::close_active_peers(
                            &config,
                            SESSION_DRAIN_CLOSE_TIMEOUT,
                        )
                        .await;
                    }
                });
            }
        }
        #[cfg(not(feature = "webrtc"))]
        {
            let _ = is_final;
        }
    }
}

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
///
/// # 世代キャンセルシグナル（イシュー #491・#492）
///
/// `cancel`（[`UpgradeCancel`]、呼び出し元 `handle_connection_with_permit`
/// 経由で `run_until` の世代から伝搬）は [`UpgradeCancel::into_future`] で
/// `Future` へ変換したうえで、spawn 済みタスク内で
/// `fandhe_backend_plugin_websocket::handle_upgrade` の第 5 引数として
/// そのまま渡す（設計 3.2 節 (i)「委譲境界はキャンセル `Future` として渡す」）。
/// 発火時の切断シーケンス（ハンドシェイク前なら 101 を送出せず終了、
/// セッション確立後なら Close frame 送信 → `CLOSE_GRACE` 上限で応答待ち）は
/// `handle_upgrade`（`crates/plugin-websocket/src/lib.rs`・`session.rs`）が
/// 担う。コア側はキャンセル `Future` の生成・受け渡しのみに責務を限定し、
/// 手動 race（`std::future::poll_fn` 等）は行わない（イシュー #491 時点の
/// 中間実装が担っていたハードクローズ・TOCTOU 回避のための優先ポーリングは
/// `handle_upgrade` 側の `race_cancel`/`already_cancelled` チェックへ移った）。
/// permit はタスク完了（= `handle_upgrade` の戻り）まで保持され、Close
/// ハンドシェイク完了（`CLOSE_GRACE` 上限）で解放される。上記「permit の
/// 契約」を破らない。
pub(crate) async fn try_handle_upgrade<S>(
    stream: S,
    head: &RequestHead,
    leftover: Vec<u8>,
    server: &Server,
    permit: &mut Option<OwnedSemaphorePermit>,
    cancel: UpgradeCancel,
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
            let cancel_fut = cancel.into_future();
            tokio::spawn(async move {
                // セッションが終了する（このタスクの future が完了する）まで
                // permit を保持し、`max_connections` のカウントから漏れない
                // ようにする。
                let _permit = permit;
                // キャンセル発火時の切断シーケンス（101 送出前なら送出せず
                // 終了、セッション確立後なら Close frame 送信 → 有界応答待ち）
                // は `handle_upgrade` 側の責務（上の関数 doc「世代キャンセル
                // シグナル」を参照）。エラーは接続の静かなクローズとして扱い
                // panic に変換しない（呼び出し元契約、上の関数 doc を参照）。
                let _ = fandhe_backend_plugin_websocket::handle_upgrade(
                    stream, &head, leftover, &config, cancel_fut,
                )
                .await;
            });
            return None;
        }
    }

    #[cfg(not(feature = "websocket"))]
    {
        let _ = (head, &leftover, server, &permit, cancel);
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
/// 同期部分（ヘッダ検査・`with_header` 呼び出し・[`fandhe_backend_plugin_compression::
/// plan_compression`] 判定）は `Middleware` の非同期 I/O 禁止規約
/// （`.claude/rules/coding-rust.md`）とは独立にコストを抑える。イシュー #468
/// で本関数自体は `async fn` へ変更した（下記「巨大応答の spawn_blocking
/// オフロード」節を参照）が、CORS 適用・圧縮要否判定は引き続き同期のまま
/// 接続タスク上で行う（軽量なため）。
///
/// # 公開 API 化は不採用（イシュー #462）
///
/// 本関数を含むレスポンス後処理型シーム一式（`finalize_response` /
/// `finalize_streaming_head`）を外部ユーザーへ公開 API 化するかはイシュー #462 で
/// 検討し、不採用と判断した。ユーザー向けのレスポンス改変には
/// [`crate::interceptor::Interceptor::map_response`] という既存の公開拡張点があり、
/// 両者の機能差はほぼ解消しているため（比較・不採用根拠は
/// `docs/design/finalize-seam-public-api.md` を参照）。
///
/// # 巨大応答の spawn_blocking オフロード（イシュー #468）
///
/// `compression` feature 有効・`Server::compression` 登録済みの場合、
/// [`fandhe_backend_plugin_compression::plan_compression`] で圧縮確定
/// （`CompressionPlan::Compress`）と判定された body 長が
/// `config.blocking_threshold()` **以上**のときのみ、実際の gzip 圧縮
/// （[`fandhe_backend_plugin_compression::compress_body`]）を
/// `tokio::task::spawn_blocking` へ切り離す。しきい値未満は従来どおり
/// 接続タスク上で同期実行する（ディスパッチオーバーヘッドが圧縮コストを
/// 上回りやすい小さい応答向けの最適化、`CompressionConfigBuilder::
/// blocking_threshold` の doc・実測根拠は `docs/design/plugin-boundary.md`
/// 5.10.7 節を参照）。
///
/// オフロード時は body を `Arc<Vec<u8>>` で包み、クロージャへは `clone`
/// （参照カウント増のみ、コピーなし）を渡す。`join` 完了後は
/// `Arc::try_unwrap` で元の `Vec<u8>` を回収する（クロージャ側の参照は
/// 圧縮完了時点で解放済みのため通常は成功する。万一失敗した場合のみ
/// `(*arc).clone()` へフォールバックし、追加コピーの発生を通常経路では
/// ゼロにする）。
///
/// `spawn_blocking` が `Err`（クロージャ panic・runtime シャットダウン等、
/// 通常運用では発生しない）を返した場合は圧縮結果を空扱いにし、
/// [`fandhe_backend_plugin_compression::attach_compressed`] 既存の
/// フェイルセーフ（圧縮結果が空なら無圧縮のまま返す）へ合流させる。
/// `Content-Encoding: gzip` は `attach_compressed` が圧縮成功を確認した
/// 場合のみ付与するため、「gzip を広告して identity body を送る」不整合は
/// この経路でも構造上起こらない（応答完全性、`.claude/rules/security.md`）。
pub(crate) async fn finalize_response(
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
            use fandhe_backend_plugin_compression::CompressionPlan;

            match fandhe_backend_plugin_compression::plan_compression(head, config, response) {
                CompressionPlan::Skip(skipped) => response = skipped,
                CompressionPlan::Compress(mut planned) => {
                    // body だけを一時的に取り出す（`Response` の他フィールド
                    // は非公開のため、部分ムーブではなく `mem::take` で
                    // 借用制約を回避する）。
                    let body = std::mem::take(&mut planned.body);
                    let compressed = if body.len() >= config.blocking_threshold() {
                        // 巨大応答: 圧縮本体のみ spawn_blocking へ切り離す
                        // （上記関数 doc「巨大応答の spawn_blocking
                        // オフロード」節を参照）。
                        let body = std::sync::Arc::new(body);
                        let body_for_blocking = std::sync::Arc::clone(&body);
                        let compressed = tokio::task::spawn_blocking(move || {
                            fandhe_backend_plugin_compression::compress_body(&body_for_blocking)
                        })
                        .await
                        .unwrap_or_default();
                        planned.body = std::sync::Arc::try_unwrap(body)
                            .unwrap_or_else(|shared| (*shared).clone());
                        compressed
                    } else {
                        // しきい値未満: 従来どおり接続タスク上で同期実行。
                        let compressed = fandhe_backend_plugin_compression::compress_body(&body);
                        planned.body = body;
                        compressed
                    };
                    response =
                        fandhe_backend_plugin_compression::attach_compressed(planned, compressed);
                }
            }
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

/// [`GenerationCancel`] / [`UpgradeCancel`]（イシュー #491）の TOCTOU 回避を
/// 直接検証するユニットテスト（`docs/design/ws-cancellation-propagation.md`
/// 3.1 節「消費側の必須実装」の直接検証、実装計画 4 節のテスト計画 3 に対応）。
///
/// 統合テスト（`crates/core/tests/ws_cancellation.rs`）は最終 shutdown・
/// rebind 経路を実 TCP 接続で検証するが、「発火後に subscribe したレシーバが
/// 即座に解決するか」という TOCTOU 回避の核心は世代構造体を直接操作する
/// ユニットテストの方が決定的に検証できる（タイミング依存の実接続シナリオを
/// 経由しないため）。
#[cfg(all(test, feature = "websocket"))]
mod cancel_tests {
    use super::{GenerationCancel, UpgradeCancel};
    use std::time::Duration;

    /// 発火**後**に生成したハンドルの `into_future()` が即座に解決すること
    /// （設計 3.1 節「委譲確定と発火の競合」いずれの順序でも取りこぼしなく
    /// 検出できる」の直接検証）。単純な `changed()` ベースの実装であれば、
    /// この呼び出し順序では永久に解決しない（設計 3.1 節が明記する失敗
    /// パターン）。
    #[tokio::test]
    async fn fires_before_subscribe_still_resolves_immediately() {
        let generation = GenerationCancel::new();
        generation.fire();

        // 発火後に subscribe した新規ハンドル。
        let cancel = generation.handle();
        let fut = cancel.into_future();

        tokio::time::timeout(Duration::from_secs(1), fut)
            .await
            .expect(
                "fire() 後に subscribe したハンドルの into_future() は\
                 即座に解決するはず（wait_for ベースで TOCTOU を回避）",
            );
    }

    /// 発火**前**に生成したハンドルも、後続の `fire()` で解決すること
    /// （通常の通知経路の非退行確認）。
    #[tokio::test]
    async fn fires_after_subscribe_resolves_on_fire() {
        let generation = GenerationCancel::new();
        let cancel = generation.handle();
        let fut = cancel.into_future();

        generation.fire();

        tokio::time::timeout(Duration::from_secs(1), fut)
            .await
            .expect("fire() 後、既存の subscribe 済みハンドルも解決するはず");
    }

    /// [`UpgradeCancel::disabled`]（世代を持たない直接呼び出し経路向け）の
    /// `into_future()` は永久に解決しないこと（`handle_connection` /
    /// `handle_connection_with_peer_addr` が世代なしで発火しないハンドルを
    /// 渡す契約の直接検証）。
    #[tokio::test]
    async fn disabled_never_resolves() {
        let cancel = UpgradeCancel::disabled();
        let fut = cancel.into_future();

        let result = tokio::time::timeout(Duration::from_millis(200), fut).await;
        assert!(
            result.is_err(),
            "disabled() のハンドルは発火源を持たないため解決しないはず"
        );
    }
}

/// [`SessionDrain`]（イシュー #498、世代キャンセル機構の WS 以外への水平展開第 1 弾）
/// のユニットテスト。
///
/// 実 `RTCPeerConnection` を close する核心の振る舞い（`WebRtcConfig::
/// begin_terminal_drain`・`activate_slot` のフェイルクローズ判定・
/// `take_active_peers`）は `crates/plugin-webrtc/tests/session_drain.rs` が実
/// ICE/DTLS で直接検証する（`crates/core` に `webrtc-rs` 由来の dev-dep を持ち込ま
/// ない既存方針、`crates/core/tests/plugin_boundary_webrtc.rs` の crate doc を参照）。
/// 本モジュールはコア側の配線契約——`Server::webrtc` 未登録時・`webrtc` feature
/// 無効時の no-op、登録済み時の `fire` が panic しないこと——に責務を限定する。
#[cfg(all(test, feature = "webrtc"))]
mod session_drain_tests {
    use super::{Server, SessionDrain};

    /// `Server::webrtc` 未登録（`webrtc_config()` が `None`）の場合、
    /// `SessionDrain::fire` は `is_final` いずれの値でも panic せず即座に戻る
    /// （`fandhe_backend_plugin_webrtc` への委譲自体が発生しない no-op 経路）。
    #[tokio::test]
    async fn fire_is_noop_when_webrtc_not_registered() {
        let server = Server::new();
        let drain = SessionDrain::new(&server);

        drain.fire(true);
        drain.fire(false);

        // detached タスクへ切り離された処理（登録ありの場合）が万一残っていても
        // 本テストの完了を妨げないことを確認するため、一呼吸置く。
        tokio::task::yield_now().await;
    }

    /// `Server::webrtc` 登録済み・Active な接続が 0 件の状態で `fire` を呼んでも
    /// panic しないこと（`fandhe_backend_plugin_webrtc::{close_active_peers,
    /// drain_for_shutdown}` が空レジストリに対して安全な no-op であることの、
    /// コア側呼び出し経路を通した確認）。
    #[tokio::test]
    async fn fire_does_not_panic_with_empty_registry() {
        let server = Server::new().webrtc(fandhe_backend_plugin_webrtc::WebRtcConfig::new());
        let drain = SessionDrain::new(&server);

        drain.fire(false);
        drain.fire(true);

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
