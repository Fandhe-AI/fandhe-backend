# CLAUDE.md

## Overview

**fandhe-backend**（正式名称確定、#200。命名根拠・可用性証跡・反映方針は
[`docs/design/framework-naming.md`](docs/design/framework-naming.md)
参照）は、AI によるセキュリティ脆弱性発見リスクに備え、Rust で新規構築する
軽量・高速・高並行なバックエンドフレームワーク。axum 級の性能を目標に、
**最小コア + Cargo feature 駆動プラグイン** 設計で、WebSocket / GraphQL / WebRTC /
OpenAPI 自動生成 / hub 配線 / 可観測性を段階的に拡張できる。

核となる 2 原則:
- **pay-for-what-you-use**: feature を無効化したら依存・コード・`unsafe`・バイナリ増をゼロにする
- **AI ファースト保守性**: doc test・網羅テスト・CI ガードレールで AI が安全に保守できる状態を保つ

公開対象 13 クレート（http / routes / core / plugin-* 10 種）を crates.io へ lockstep
バージョニングで公開する（恒久非公開: axum-ref / ws-load-client / docs-site）。現行公開版は
0.1.0（2026-07-21）、リポジトリの実装は 0.2.0（イシュー #437。breaking change 2 件を含み
publish は準備中、`CHANGELOG.md` 参照）。手順・区分は `docs/design/crates-io-release.md` 参照。

仕様書は [Fandhe-AI/fandhe-backend-spec](https://github.com/Fandhe-AI/fandhe-backend-spec) を
`docs/spec/`（submodule）に取り込む。実装は `docs/spec/06-roadmap.md` の MS-1〜MS-6 に従い、
最初のタスクは TASK-1.1（`cargo workspace`・CI 基盤整備）。

## Repository Structure

```
fandhe-backend/
├── CLAUDE.md              # 本ファイル（Claude Code 運用ガイド）
├── AGENTS.md              # 横断的設計規約（TASK-2.3）+ AI エージェント向け変更ガイド
│                            # （モジュール境界・変更手順・判定基準・エスカレーション基準、TASK-11.3）
├── README.md
├── LICENSE-MIT            # MIT ライセンス（デュアルライセンスの一方、イシュー #94）
├── LICENSE-APACHE         # Apache License 2.0（デュアルライセンスの一方、イシュー #94）
├── CONTRIBUTING.md        # 貢献ガイド（開発フロー・コミット規約・設計原則・ライセンス同意、イシュー #94）
├── skills-lock.json       # 導入スキルのロック
├── Makefile               # 開発タスクの入口（setup / build / test / lint / audit / docker-*。
│                            # CI と同一コマンドをローカル再現、`make help` で一覧）
├── lefthook.yml           # git hooks 定義（pre-commit: cargo fmt --check、commit-msg:
│                            # Conventional Commits 検証。`make hooks` で配線）
├── .editorconfig          # エディタ設定統一（Rust 4 スペース・YAML/TOML 等 2 スペース）
├── Dockerfile             # 開発用コンテナイメージ（rust:slim + rustfmt/clippy。本番配布用ではない）
├── compose.yaml           # 開発用 Docker Compose（`make docker-shell` / `make docker-test`。
│                            # cargo レジストリ・target をボリュームで高速化）
├── docs/
│   ├── spec/               # 仕様書 submodule（要件・タスク・ロードマップ）
│   ├── design/             # リポジトリ側設計ドキュメント（実装フェーズの設計判断を記録）
│   │   ├── crates-io-release.md  # crates.io 公開手順（名前確保・所有権・リリース CI、イシュー #94）
│   │   ├── interceptor-extension-point.md  # ユーザー向けインターセプト・レスポンス改変
│   │   │                            # 拡張点 `Interceptor` の設計判断（イシュー #420。3 拡張点で
│   │   │                            # 表現できない根拠・評価順序・fail-closed 除外を記録）
│   │   ├── finalize-seam-public-api.md  # `finalize_response` / `finalize_streaming_head`
│   │   │                            # （レスポンス後処理型シーム）の公開 API 化の採否検討
│   │   │                            # （イシュー #462。`Interceptor::map_response` との棲み分け・
│   │   │                            # 不採用根拠・再検討条件を記録。結論は不採用）
│   │   ├── docs-site-redesign.md  # GitHub Pages docs サイト刷新設計（イシュー #388、
│   │   │                            # 親 #384。3 カラムレイアウト・依存ゼロ全文検索・
│   │   │                            # 公開範囲規約（issue/TASK 番号記述の docs/design/ への
│   │   │                            # 集約）を fandhe-frontend 設計正典から翻訳、#389〜#399 の根拠）
│   │   ├── v1-scope-tls-multipart.md  # TLS 終端・multipart/form-data の v1 スコープ方針
│   │   │                            # （フレームワーク本体では扱わず、TLS はリバース
│   │   │                            # プロキシ前提・multipart は raw body 受理のみ、
│   │   │                            # イシュー #322。docs/spec 除外事項表 #8・#9 と対応）
│   │   └── ws-cancellation-propagation.md  # WS 委譲タスクへのキャンセル伝播機構の設計
│   │                            # （イシュー #490、REQ-4。最終 graceful shutdown（#313）・
│   │                            # rebind 世代 drain（#485/#488）双方の grace 超過
│   │                            # 強制クローズ対象外にある WebSocket 委譲セッション
│   │                            # へキャンセルを伝播する機構を設計。世代別
│   │                            # `tokio::sync::watch` + 委譲境界はキャンセル
│   │                            # `Future` として受け渡す方式を採用、`UpgradeHandler`
│   │                            # シグネチャ変更は不要（3 層構造）、
│   │                            # `fandhe_backend_plugin_websocket::handle_upgrade`
│   │                            # は breaking change として扱う方針を記録。コード
│   │                            # 実装は #491（コア配線）・#492（plugin-websocket
│   │                            # の Close ハンドシェイク実装）・#493（両経路の
│   │                            # 統合テスト・既知の限界 doc 更新）で完了済み。
│   │                            # #499（10 節）でキャンセルの適用範囲を受信待ちから
│   │                            # ユーザーハンドラ実行中・返信/Close 送出中へ拡大し、
│   │                            # `race_cancel` による即時打ち切り意味論を採用）
│   ├── guide/              # 利用者向けガイド（Getting Started・feature 構成別サンプル・
│   │                        # チュートリアル、TASK-11.5 / #95）。「どう作るか」の docs/design/ とは
│   │                        # 責務分離、「どう使うか」を扱う
│   ├── dep-impact/         # 依存インパクト（依存数・バイナリサイズ・unsafe 件数）記録台帳（TASK-15.2）
│   └── acceptance/         # REQ-1 等・NFR-7 等の受け入れ検証結果レポート（TASK-1.6-2 で追加、
│                            # NFR-7 分は #263、NFR-6 分は #282 で追加）。issue399-docs-site-visual.md は
│                            # docs サイト刷新（親 #384）の視覚確認・受け入れレポート（イシュー #399。
│                            # ライト/ダーク × 複数解像度のスクリーンショット証跡 `assets/issue399/`・
│                            # 実ブラウザ描画でのみ発見できた CSS カスケード順序バグの検出・修正記録）
├── Cargo.toml             # cargo workspace ルート（TASK-1.1 で構築、resolver = "3"）。
│                            # `[workspace.package] version` + `[workspace.dependencies]`
│                            # （内部 13 クレートを path + version で定義）で lockstep
│                            # バージョン・path 依存の version 併記を一元管理し、各クレートは
│                            # `version.workspace = true` + `{ workspace = true }` で継承する
│                            # （イシュー #452。`docs/design/crates-io-release.md` 7.2 節参照）
├── rust-toolchain.toml    # stable + rustfmt/clippy
├── crates/                # cargo workspace
│   ├── core                           # 最小コア。`webrtc-proxy`（TASK-2.1、#18）・`webrtc`
│   │                                    # （TASK-8.1、#26）・`websocket`（TASK-4.1、#22）・
│   │                                    # `graphql`（TASK-2.4、#21）・`openapi`（TASK-2.1、#256）・
│   │                                    # `cors`（イシュー #305）・`compression`
│   │                                    # （イシュー #321）・`static`（イシュー #318）の
│   │                                    # 8 feature で `dep:` 構文により
│   │                                    # 各プラグインを着脱可能に配線済み（`webrtc-proxy` 優先評価）。
│   │                                    # `openapi` は `Server::openapi()` の明示登録
│   │                                    # （opt-in）時のみ `GET /openapi.json` と
│   │                                    # `GET /openapi.yaml`（#279）を返す。`cors` は
│   │                                    # `Server::cors(config)` 登録時のみ「レスポンス後処理型」
│   │                                    # シーム（`crate::plugin::finalize_response`）経由で実
│   │                                    # リクエストへ CORS ヘッダを付与し、プリフライトは利用者が
│   │                                    # `fandhe_backend_plugin_cors::preflight_response` を
│   │                                    # `Router::options_fallback`（#304）へ直接配線する 2 点構成。
│   │                                    # `compression` は `Server::compression(config)` 登録時のみ
│   │                                    # 同一シーム（CORS の後、逐次適用）経由で条件充足レスポンスを
│   │                                    # gzip 圧縮する（`fandhe-backend-plugin-compression`、
│   │                                    # 外部依存は `flate2` のみ）。
│   │                                    # `BoundServer::run_until(shutdown)` で graceful shutdown
│   │                                    # （accept 停止 → in-flight 完了待ち → grace 超過時強制
│   │                                    # クローズ）に対応（既存 `run()` は `run_until` への薄い
│   │                                    # 委譲として後方互換を維持、`Server::shutdown_grace_period`
│   │                                    # で待機上限を設定可能、イシュー #313。
│   │                                    # `docs/design/graceful-shutdown.md` 参照）。
│   │                                    # `Handler::handle_streaming`（opt-in 既定メソッド）+
│   │                                    # `streaming::{StreamingResponse, BodyWriter}` で
│   │                                    # レスポンス側 chunked ストリーミング送信を提供（bounded
│   │                                    # mpsc によるバックプレッシャ・`finish` 省略時は終端
│   │                                    # チャンクなしで打ち切りクローズ、既存 `Handler::handle`
│   │                                    # 実装は無変更で後方互換維持、イシュー #319）。
│   │                                    # `static` / `compression` feature 有効時は設定型
│   │                                    # `StaticFilesConfig` / `CompressionConfig` を
│   │                                    # `plugin_static` / `plugin_compression` モジュールとして
│   │                                    # 再エクスポートし、プラグインクレートへの直接依存を
│   │                                    # 追加せずに構築可能にする（イシュー #421）。イシュー #435 で
│   │                                    # 本パターンを残り全設定登録型 feature（`websocket` /
│   │                                    # `graphql` / `cors` / `tracing` / `openapi` / `webrtc` /
│   │                                    # `webrtc-proxy`）へ水平展開し、対応する `plugin_*`
│   │                                    # モジュール経由でも各設定型を構築可能にした。
│   │                                    # `Interceptor`（`interceptor` モジュール、イシュー #420）で
│   │                                    # 3 拡張点で表現できないリダイレクト・レスポンス改変を
│   │                                    # ユーザー向けに提供。`intercept`（ルーティング・プラグイン
│   │                                    # 評価前、`RequestGate`/`UpgradeHandler` より後・
│   │                                    # `plugin::try_intercept` より前）と `map_response`
│   │                                    # （最終応答確定後・`finalize_response` より前）の 2 フック、
│   │                                    # `Server::interceptor` で複数登録可（登録順評価）。feature
│   │                                    # ゲート不要（外部依存ゼロ、pay-for-what-you-use）。
│   │                                    # `RequestGate::check` の拒否応答 `GateOutcome::Reject` は
│   │                                    # イシュー #424 で `status`/`body` の個別フィールドから
│   │                                    # 検証済み `Response`（`crates/http`）をそのまま運ぶ形へ
│   │                                    # 変更し、レート制限の `429 + Retry-After` 等ヘッダ付き
│   │                                    # 拒否応答をゲート実装から返せるようにした（`GateOutcome::
│   │                                    # reject(status, body)` ヘルパでヘッダなしの従来相当の
│   │                                    # 構築も可能。ヘッダ検証は `Response::with_header` の
│   │                                    # 既存フェイルクローズ機構に委ねる）。`map_response` は
│   │                                    # イシュー #434 で `Handler::handle_streaming`
│   │                                    # によるストリーミング応答のヘッド（ステータス・
│   │                                    # `Content-Type`・追加ヘッダ）にも適用対象を拡張した
│   │                                    # （`write_streaming_response` がヘッド確定時に 1 回・
│   │                                    # 登録順で適用。body は chunked framing がコアの直接
│   │                                    # 書き出しループを経由するため反映されず破棄する契約。
│   │                                    # `finalize_response`（CORS → 圧縮を逐次適用する
│   │                                    # 通常応答経路専用シーム）自体は引き続き未適用。
│   │                                    # イシュー #451 で第 4 のシーム
│   │                                    # `finalize_streaming_head` を新設し、`map_response`
│   │                                    # 適用直後のヘッドへ CORS ヘッダ付与を適用する
│   │                                    # ようにした。イシュー #461 で第 5 のシーム
│   │                                    # `prepare_streaming_compression` を追加し、body
│   │                                    # 全体を保持しない専用エンコーダ
│   │                                    # （`StreamingGzipEncoder`）経由でチャンク単位の
│   │                                    # ストリーミング gzip 圧縮を接続した
│   │                                    # （`CompressionConfigBuilder::compress_streaming`
│   │                                    # opt-in・既定 OFF、HTTP/1.1 chunked 経路限定、
│   │                                    # `docs/design/interceptor-extension-point.md`・
│   │                                    # `docs/design/plugin-boundary.md` 5.9.7 節・
│   │                                    # 5.10.6 節参照）。イシュー #468 で `finalize_response`
│   │                                    # を `async fn` へ変更し、body 長がしきい値
│   │                                    # （`CompressionConfigBuilder::blocking_threshold`、
│   │                                    # 既定 64 KiB）以上の gzip 圧縮を
│   │                                    # `tokio::task::spawn_blocking` へ切り離すように
│   │                                    # した（巨大応答の圧縮による tokio ワーカ
│   │                                    # スレッド長時間占有の緩和、しきい値未満は
│   │                                    # 従来どおりインライン実行。実測根拠は
│   │                                    # `benches/reports/
│   │                                    # issue468-compression-blocking.md`・
│   │                                    # `docs/design/plugin-boundary.md` 5.10.7 節）。
│   │                                    # イシュー #486 で `RequestGate::check` へ
│   │                                    # `ctx: &GateContext` 引数を追加し、accept した
│   │                                    # ソケットの実 peer address（`GateContext::
│   │                                    # peer_addr`）を gate 実装から参照可能にした
│   │                                    # （BREAKING CHANGE、`CHANGELOG.md` 移行手順・
│   │                                    # `docs/design/gate-peer-addr.md` 参照）。
│   │                                    # `tokio::io::duplex` 等の非ソケット経路では
│   │                                    # `None` になるフェイルクローズ契約。実 peer
│   │                                    # address を注入したい呼び出し元向けに新設の
│   │                                    # 公開 API `handle_connection_with_peer_addr`
│   │                                    # も追加した。イシュー #485 で
│   │                                    # `BoundServer::rebind_handle` /
│   │                                    # `RebindHandle::rebind` を追加し、稼働中の
│   │                                    # accept ループを止めずに listening アドレスを
│   │                                    # 差し替え可能にした（`rebind()` は呼び出し側で
│   │                                    # 新規 `TcpListener` を bind してから差し替えを
│   │                                    # 依頼する構造のため bind 失敗時は旧 listener・
│   │                                    # in-flight に無影響、fail-closed）。差し替え
│   │                                    # 直前までの「旧世代」接続は
│   │                                    # `Server::shutdown_grace_period` を上限に
│   │                                    # `run_until` 自体をブロックせず背景 drain し、
│   │                                    # 超過分は強制クローズする（既存の graceful
│   │                                    # shutdown・拡張点の評価順序は不変、
│   │                                    # `docs/design/rebind.md` 参照）。WS 委譲タスクの
│   │                                    # 世代キャンセル機構（`GenerationCancel`/
│   │                                    # `UpgradeCancel`、`websocket` feature ゲート、
│   │                                    # イシュー #489〜#497）に続き、イシュー #498 で
│   │                                    # `SessionDrain`（`webrtc` feature ゲート、独立
│   │                                    # シーム）を新設し、`Server::webrtc` 登録済みの
│   │                                    # `RTCPeerConnection`（`plugin-webrtc` の
│   │                                    # `WebRtcConfig::registry`）へも最終 graceful
│   │                                    # shutdown・rebind 世代 drain 両経路から有界な
│   │                                    # 明示 close を伝播するようにした（WS 以外の
│   │                                    # 長時間委譲プラグインへの水平展開第 1 弾、
│   │                                    # `docs/design/ws-cancellation-propagation.md`
│   │                                    # 10 節参照）。`RebindHandle::rebind` の doc に
│   │                                    # 「`Server::webrtc` 登録済みの場合、rebind と
│   │                                    # 無関係な進行中の WebRTC 通話もレジストリ全件が
│   │                                    # 強制切断される」という副作用を明記した
│   │                                    # （レビュー対応。実接続を確立し
│   │                                    # `BoundServer::rebind_handle().rebind()` 経由で
│   │                                    # force-close されることを end-to-end で検証する
│   │                                    # `crates/plugin-webrtc/tests/
│   │                                    # rebind_force_close.rs` を追加。同クレートの
│   │                                    # `WebRtcConfig::activate_slot` は
│   │                                    # `terminal_draining` の判定を `registry` の
│   │                                    # `Mutex` ロック外で読んでいたため、終端 drain
│   │                                    # （`drain_for_shutdown`）と並行するシグナリング
│   │                                    # 完了が drain 後に `Active` 化してしまう TOCTOU
│   │                                    # が起こりえた。判定を `Active` 遷移と同一ロック
│   │                                    # 区間へ統合し修正した）。
│   ├── http / routes                  # HTTP プリミティブ・ルーティング（`Router::route_param` で
│   │                                    # `{name}` パスパラメータ対応、TASK-176、#176。末尾
│   │                                    # ワイルドカードセグメント `{*name}` にも対応し、`/` を含む
│   │                                    # 残りパス全体を 1 個以上のセグメント条件で束縛（中間配置は
│   │                                    # 登録時エラー、静的ファイル配信プラグイン等の前提整備、
│   │                                    # イシュー #317）。chunked
│   │                                    # Transfer-Encoding 対応（sans-IO `ChunkedDecoder`、
│   │                                    # DoS 上限・fuzz target 追加、イシュー #181）。`RequestHead::path`
│   │                                    # / `query` でクエリ文字列を分離し `Router::dispatch` の
│   │                                    # パス照合をクエリ付きリクエストに対応させる（/search 前提整備、
│   │                                    # イシュー #258）。`query::parse_query` でクエリ文字列
│   │                                    # key-value 分解を sans-IO 純関数として提供（ゼロコピー・
│   │                                    # DoS 上限内蔵・非デコード、イシュー #306）。
│   │                                    # `form::parse_form` で `application/x-www-form-urlencoded`
│   │                                    # ボディパーサを提供（`query`/`percent` を合成し `+` → 空白
│   │                                    # 変換等のフォーム固有デコード仕様・DoS 上限・Content-Type
│   │                                    # 検証ヘルパを内蔵、イシュー #308）。
│   │                                    # `Router::options_fallback` で OPTIONS
│   │                                    # プリフライトを opt-in フックへ委譲可能にし、明示登録された
│   │                                    # OPTIONS ルートを常に優先しつつ未登録なら従来どおり
│   │                                    # 405 + `Allow` を維持（CORS プラグイン前提整備、イシュー #304）。
│   │                                    # `Response::with_set_cookie` + 構築時検証済み専用型
│   │                                    # `cookie::SetCookie` で `Set-Cookie` ヘッダを安全に構築
│   │                                    # （RFC 6265 cookie-name/cookie-value/path-value 検証、
│   │                                    # `HttpOnly`/`Secure`/`SameSite`/`Path`/`Max-Age` 属性対応、
│   │                                    # 認証・セッション実装前提整備、イシュー #303）。
│   │                                    # `cookie::parse_cookie_header` で RFC 6265 準拠の
│   │                                    # Cookie ヘッダ読み取りパーサを提供（cookie-pair 構文検証・
│   │                                    # DoS 上限内蔵・非デコード。`RequestHead::cookies` が複数
│   │                                    # `Cookie` ヘッダの結合・累積上限適用を担う、イシュー #309）。
│   │                                    # `error::IntoResponse` / `error::error_response` で
│   │                                    # エラーレスポンス共通化ヘルパを提供（serde 非依存、
│   │                                    # JSON エラーボディ標準形 `{"error":"..."}` を手実装
│   │                                    # エスケープで直列化、`message` は `&'static str` 限定で
│   │                                    # スタックトレース・内部情報の流出経路を型レベルで排除、
│   │                                    # イシュー #310）。`Router::fallback` /
│   │                                    # `Router::fallback_with` で静的・パラメータいずれのルートにも
│   │                                    # 一致しなかったリクエストの共通処理（カスタム 404・SPA の
│   │                                    # index.html 返却等）を登録可能にし、`FallbackPolicy` で
│   │                                    # 405（メソッド不一致）も委譲するかを個別選択（既定は 404
│   │                                    # のみ委譲する安全側、イシュー #316）。`Router::route_async` /
│   │                                    # `route_param_async` で async ハンドラを登録可能にし、
│   │                                    # 既定ハンドラ契約（`crates/core` の `Handler::handle`）を
│   │                                    # `fandhe_backend_routes::HandlerFuture`（boxed future）
│   │                                    # 返却へ移行（`Router::route`/`route_param` の同期登録 API は
│   │                                    # 内部アダプタで非破壊のまま維持。3 拡張点
│   │                                    # （`Middleware`/`UpgradeHandler`/`RequestGate`）は意図的に
│   │                                    # 同期のまま据え置き、`sqlx` 等の非同期 I/O をハンドラ本体で
│   │                                    # 直接 await 可能にする、イシュー #315、
│   │                                    # `docs/design/async-handler.md`）。
│   │                                    # `Response::serialize_chunked_head` /
│   │                                    # `serialize_streaming_head_http10` +
│   │                                    # `chunked::{encode_chunk, encode_terminator}`（sans-IO
│   │                                    # エンコーダ）でレスポンス側 chunked ストリーミング送信を
│   │                                    # 提供（`crates/core` の `Handler::handle_streaming` opt-in
│   │                                    # 拡張点から使用、既存の Content-Length 応答は無変更で
│   │                                    # 後方互換維持、イシュー #319）
│   │   └── fuzz/                      # cargo-fuzz 専用クレート（root workspace から exclude、TASK-15.3-1、#87）
│   ├── plugin-webrtc-proxy            # WebRTC シグナリングプロキシプラグイン（別プロセス切り出し型、
│   │                                    # TASK-8.2-2、#74。`crates/core` の `webrtc-proxy` feature 経由で配線、TASK-2.1、#18）
│   ├── plugin-webrtc                  # in-process WebRTC プラグイン（`webrtc-rs` 直接依存、TASK-8.1、#26。
│   │                                    # `crates/core` の `webrtc` feature 経由で配線。攻撃表面が大きいため
│   │                                    # `plugin-webrtc-proxy` が MVP 推奨、クレート境界で完全分離）。
│   │                                    # `close_active_peers` / `drain_for_shutdown`（`drain` モジュール）で
│   │                                    # `WebRtcConfig::registry` 上のアクティブな `RTCPeerConnection` を
│   │                                    # 1 接続あたり有界タイムアウトで明示的に close する API を追加した
│   │                                    # （イシュー #498。WS 委譲タスクの世代キャンセル機構（#489〜#497）を
│   │                                    # WS 以外の長時間委譲プラグインへ水平展開する第 1 弾。`crates/core` 側は
│   │                                    # `SessionDrain`（`webrtc` feature ゲート、独立シーム）が最終 graceful
│   │                                    # shutdown・rebind 世代 drain の両経路から発火する。`drain_for_shutdown`
│   │                                    # のみ `WebRtcConfig::begin_terminal_drain` で以降の新規登録を拒否する
│   │                                    # フェイルクローズ判定を伴う。設計・棲み分けは
│   │                                    # `docs/design/ws-cancellation-propagation.md` 10 節を参照）
│   ├── plugin-graphql                 # GraphQL プラグイン（パスインターセプト型、TASK-2.4、#21 で境界確立。
│   │                                    # REQ-2 の「2 種のプラグイン着脱」受け入れ基準は当初 webrtc-proxy と
│   │                                    # 共に実証（#21。実 WebSocket が並行実装中だったための代替ペア）、
│   │                                    # 現在は仕様が名指しする websocket と共に実ペアで再実証済み
│   │                                    # （イシュー #261、`docs/acceptance/req2-plugin-mechanism.md`）。
│   │                                    # TASK-5.1、#38 で async-graphql による実クエリ実行へ実装。
│   │                                    # `Server::graphql` にスキーマ登録した場合のみ `POST /graphql` を
│   │                                    # 処理し、未登録時は feature 有効でもフォールスルー）
│   ├── plugin-openapi                 # OpenAPI ドキュメント生成プラグイン（ApiDoc + utoipa::path 定義、TASK-3.1、#30。
│   │                                   # gen-openapi CLI・openapi.json 静的埋め込み、TASK-3.2、#31。
│   │                                   # `crates/core` の `openapi` feature 経由で配線、TASK-2.1、#256。
│   │                                   # `Server::openapi()` 登録時のみ `GET /openapi.json` を配信。
│   │                                   # `GET /openapi.yaml` も同一 opt-in・同一スキーマ源
│   │                                   # （ApiDoc）で配信（#279。YAML 変換依存は開発用
│   │                                   # `gen-cli` feature に閉じ、サーバ経路には現れない）。
│   │                                   # `OpenApiDoc::from_json`（構築時 JSON 検証済み）+
│   │                                   # `Server::openapi_with(doc)` で利用者アプリ独自の
│   │                                   # OpenAPI スキーマも `GET /openapi.json` /
│   │                                   # `GET /openapi.yaml` として配信可能（`Server::openapi()`
│   │                                   # とは後勝ちで排他、イシュー #320）
│   ├── plugin-websocket                # WebSocket プラグイン（RFC 6455 ハンドシェイク検証・101 応答・
│   │                                    # tokio-tungstenite へのフレーミング委譲、TASK-4.1、#22。
│   │                                    # `crates/core` の `websocket` feature 経由で `UpgradeHandler`
│   │                                    # 拡張点配線、Upgrade 型プラグイン境界パターンの第 1 号。
│   │                                    # ユーザー定義メッセージハンドラ API（`WsMessageHandler`、
│   │                                    # `WebSocketConfig::with_handler`、既定は `EchoHandler` で
│   │                                    # 後方互換維持、Issue #179）を追加）。`handle_upgrade`
│   │                                    # がキャンセル `Future` 引数（第 5 引数、BREAKING
│   │                                    # CHANGE）を受け取り、コアの世代キャンセルシグナル
│   │                                    # （最終 graceful shutdown・rebind 世代 drain）発火時に
│   │                                    # 正常な Close ハンドシェイク（close code 1001 Going
│   │                                    # Away → 応答を最大 10 秒待つ有界ドレイン）で切断する
│   │                                    # （イシュー #492。`crates/core` 側の配線・設計は
│   │                                    # `docs/design/ws-cancellation-propagation.md` 参照）。
│   │                                    # イシュー #499 でキャンセルの適用範囲を受信待ちから
│   │                                    # ユーザーハンドラ実行中・`WsOutcome::Reply`/`Close`
│   │                                    # 送出中へ拡大し、`race_cancel` で当該 `Future` を
│   │                                    # 即座に打ち切って Close ハンドシェイクへ分岐する
│   │                                    # （シグネチャ変更なし。`on_message` が返す `Future`
│   │                                    # は任意の `await` 点で drop されうる契約へ変更、
│   │                                    # `docs/design/ws-cancellation-propagation.md` 10 節）
│   ├── plugin-tracing                 # 可観測性（サンプリング付きトレーシング）プラグイン（TASK-10.1、#56。
│   │                                    # REQ-10・PoC-10（サンプリングなし構成で RPS 劣化 31.6%）を踏まえ、
│   │                                    # 決定的カウンタ方式のサンプリング + 既定で非同期・バッファ済み I/O
│   │                                    # （tracing-appender の non_blocking writer）を提供。`crates/core` の
│   │                                    # `tracing` feature 経由で `Middleware` 拡張点配線、Middleware 型
│   │                                    # プラグイン境界パターンの第 1 号、`docs/design/plugin-boundary.md` 5.6 節）
│   ├── plugin-cors                    # CORS プラグイン（イシュー #305。`Middleware::on_response` が
│   │                                    # レスポンスへの参照を持たない観測専用契約のため使えず、
│   │                                    # 「レスポンス後処理型」という新パターン（`crate::plugin::
│   │                                    # finalize_response`、固定シグネチャの非公開シーム）で配線。
│   │                                    # プリフライトは利用者が `preflight_response` を
│   │                                    # `Router::options_fallback`（#304）へ直接配線する 2 層構成。
│   │                                    # `crates/core` の `cors` feature 経由で `Server::cors(config)`
│   │                                    # 登録時のみ実リクエストへ CORS ヘッダを付与。外部依存ゼロ、
│   │                                    # `docs/design/plugin-boundary.md` の該当節を参照）
│   ├── plugin-compression             # レスポンス圧縮プラグイン（イシュー #321。`plugin-cors` が
│   │                                    # 確立した「レスポンス後処理型」シームの第 2 インスタンス、
│   │                                    # `finalize_response` で CORS の後に逐次適用）。gzip のみ実装
│   │                                    # （br はスコープ外）、外部依存は `flate2`（`rust_backend`、
│   │                                    # 純 Rust の miniz_oxide 実装に固定）のみ。ステータス・
│   │                                    # `Content-Type`・body サイズ・`Accept-Encoding` を判定基準に
│   │                                    # フェイルセーフに圧縮可否を決定。`crates/core` の
│   │                                    # `compression` feature 経由で `Server::compression(config)`
│   │                                    # 登録時のみ動作。BREACH 類似の情報漏洩リスクを doc に明記、
│   │                                    # `docs/design/plugin-boundary.md` 5.10 節を参照）。
│   │                                    # `StreamingGzipEncoder` +
│   │                                    # `begin_streaming_compression` で
│   │                                    # `Handler::handle_streaming`（#319）の chunked
│   │                                    # ストリーミング応答向けチャンク単位 gzip 圧縮も提供
│   │                                    # （イシュー #461、`compress_streaming` opt-in・
│   │                                    # 既定 OFF、`crates/core` 側の配線は上記参照。
│   │                                    # `docs/design/plugin-boundary.md` 5.10.6 節を参照）。
│   │                                    # イシュー #468 で `apply_compression` を
│   │                                    # `plan_compression` / `compress_body` /
│   │                                    # `attach_compressed` の 3 関数へ分割公開し、
│   │                                    # `CompressionConfigBuilder::blocking_threshold`
│   │                                    # （既定 64 KiB）で `crates/core` 側の
│   │                                    # `spawn_blocking` オフロード判定に使うしきい値を
│   │                                    # 設定可能にした（本クレート自体は `tokio` に依存
│   │                                    # せず、依存最小構成を維持。実測根拠は
│   │                                    # `docs/design/plugin-boundary.md` 5.10.7 節）
│   ├── plugin-static                  # 静的ファイル配信プラグイン（イシュー #318。パス
│   │                                    # インターセプト型（`try_intercept`）+ `spawn_blocking`
│   │                                    # 変種。`crates/core` の `static` feature 経由で
│   │                                    # `Server::static_files(config)` 登録時のみ `GET` を
│   │                                    # 配信。二層防御（I/O 前の字句検証（先頭ドット
│   │                                    # セグメント拒否で `.env`・`.git/config` 等の機密
│   │                                    # ファイル配信も遮断）+ canonicalize 後の root 配下
│   │                                    # 検証）でパストラバーサル・シンボリックリンク脱出を
│   │                                    # 拒否し、未検出・検証失敗・サイズ超過は一律 404。
│   │                                    # 末尾スラッシュ 1 個（`<mount>/dir/`）はディレクトリ
│   │                                    # 要求として index.html を解決（連続スラッシュ拒否は
│   │                                    # 維持、イシュー #418）。内蔵 MIME テーブルへ
│   │                                    # `.webmanifest`（`application/manifest+json`）等の
│   │                                    # PWA/SSG 頻出拡張子を追加し、`StaticFilesConfigBuilder::
│   │                                    # mime(ext, content_type)` で内蔵テーブルにない拡張子を
│   │                                    # 利用者が追加登録できる（`content_type` は `&'static str`
│   │                                    # 限定・`build()` 時に拡張子/値の形式検証でヘッダ
│   │                                    # インジェクションを遮断、イシュー #423）。
│   │                                    # `StaticFilesConfigBuilder::fallthrough_on_miss`
│   │                                    # （既定 `false`、イシュー #419）を有効にすると
│   │                                    # 未ヒット GET を一律 `None` で下流 `Handler`
│   │                                    # （`Router` 等）へフォールスルーし、mount `/`
│   │                                    # + 動的エンドポイント共存構成を可能にする。外部依存ゼロ
│   │                                    # （`fandhe-backend-http` + `tokio` の `rt` feature のみ）、
│   │                                    # `docs/design/plugin-boundary.md` 5.11 節を参照）
│   ├── plugin-*                       # 他の feature 着脱プラグイン（TASK-2.1 以降で追加予定）
│   ├── docs-site                      # GitHub Pages ドキュメントサイト生成ツール（SSG、
│   │                                    # fandhe-frontend の docs-site を移植。publish=false で
│   │                                    # 本体バイナリに含まれない。crates.io 依存は
│   │                                    # fandhe-frontend-core/app/server 0.1.0 のみ。
│   │                                    # 内蔵 linkcheck は fail-closed でリンク切れ時は書き出さない）。
│   │                                    # `src/script.rs` にダークモードトグル用の唯一の JS を
│   │                                    # 保持し、`build::build_site` が `out_dir/assets/site.js`
│   │                                    # へ書き出す（イシュー #390）。`layout::docs_page` は
│   │                                    # FOUC 抑止インラインスニペットを `<head>` 先頭付近
│   │                                    # （stylesheet より前）へ、`<script src>`（`defer`）を
│   │                                    # stylesheet の後へ埋め込み、ヘッダー右側の
│   │                                    # `div.docs-header-actions` に GitHub リンクと既定
│   │                                    # `hidden` のテーマトグルボタンを配置する（可視化・
│   │                                    # イベント配線は `site.js` 読み込み後にのみ行う
│   │                                    # fail-closed 構成。JS 無効時は `prefers-color-scheme`
│   │                                    # 追従へ退避）。`site/assets/site.js` と同名の静的
│   │                                    # アセットは生成物との衝突としてビルドエラーにする。
│   │                                    # `src/search.rs` が依存ゼロ全文検索インデックスを
│   │                                    # 生成し（イシュー #396）、`build::build_site` が各
│   │                                    # ページの本文（prev/next ナビ・サイドバー・ヘッダー
│   │                                    # を含まない）を走査してページ単位 4 KiB 切り詰め・
│   │                                    # 索引全体 1 MiB 上限（超過は fail-closed でビルド
│   │                                    # 失敗）を適用したのち `out_dir/assets/search-index.json`
│   │                                    # へ書き出す。`layout::docs_page` はヘッダー右側の
│   │                                    # 検索入力欄（既定 `hidden`）へ索引 URL を
│   │                                    # `data-search-index` 属性で埋め込み、`src/script.rs`
│   │                                    # の `SITE_JS` が実行時に索引を遅延 `fetch` して
│   │                                    # タイトル/見出し/本文の部分一致検索・結果描画を行う
│   │                                    # （`site/assets/search-index.json` も生成物との衝突
│   │                                    # としてビルドエラーにする。外部 JS ライブラリ・
│   │                                    # 追加クレート依存は一切増やさない）
│   └── axum-ref                       # 性能比較用参照実装（TASK-1.2 で追加）
├── templates/              # 利用者向け配布テンプレート（イシュー #364）
│   └── app                            # feature 一式（cors / compression / static / openapi）を
│                                        # 組み合わせた実運用形 ToDo API 雛形。root workspace
│                                        # 非メンバー（`[workspace] members = ["."]` の standalone
│                                        # workspace）+ `publish = false`。依存は `version` + `path`
│                                        # 併記で、リポジトリ内では常に最新実装に対して検証する
├── examples/               # Next.js 流 `with-<feature>` 独立サンプル群（イシュー #365）。
│                            # 1 サンプル = 1 機能で、各サンプルは templates/app と同じ
│                            # standalone workspace 構成（root workspace 非メンバー・
│                            # `publish = false`）。3 種のサンプル置き場との重複回避方針は
│                            # examples/README.md を参照
│   ├── with-cors                      # CORS の 2 層配線を見せる最小 ToDo API サンプル
│   ├── with-graphql                   # GraphQL の配線（`Server::graphql` へのスキーマ登録 +
│   │                                    # `POST /graphql` 最小クエリ実行）を見せるサンプル
│   │                                    # （イシュー #360）
│   ├── with-websocket                 # ユーザー定義 WebSocket メッセージハンドラ
│   │                                    # （`WebSocketConfig::with_handler`）を見せるサンプル
│   └── with-interceptor               # コア拡張点 `Interceptor` の 2 フック（`intercept`
│                                        # によるリダイレクト・`map_response` によるレスポンス
│                                        # 改変）を見せる最小サンプル（イシュー #433）
├── site/                   # GitHub Pages ドキュメントサイトコンテンツ（index.md・nav.toml・
│                            # assets/site.css。docs-site SSG ツールで生成対象。base_path=/fandhe-backend）
│   ├── guides.md                      # Guides セクション索引ページ（イシュー #393）。要約付き
│   │                                    # リンク一覧を持ちセクション先頭に配置、既存
│   │                                    # `docs/guide/README.md` は /guides/reading/ へ再登録し
│   │                                    # 他ページからの .md リンク互換を維持
│   ├── api.md                         # API Reference セクション索引ページ（イシュー #393）。
│   │                                    # 要約付きリンク一覧を持ちセクション先頭に配置
│   └── examples/                      # Examples セクション原稿（イシュー #392）。索引
│                                        # （examples.md）+ with-cors / with-graphql /
│                                        # with-websocket / templates-app の 4 紹介ページ。
│                                        # `examples/README.md`・各 README を再構成し、
│                                        # GitHub 上の実体への絶対 URL 導線を張る
├── ts/                     # openapi-typescript 連携パイプライン（TASK-6.1、#54、REQ-6）。
│                            # crates/plugin-openapi/openapi.json → openapi-typescript →
│                            # ts/src/generated/schema.d.ts（コミット対象）→ openapi-fetch
│                            # ベースの型安全クライアント（src/client.ts）の一方向パイプライン。
│                            # ビルド時専用で Rust バイナリ・依存ツリーに一切影響しない
│                            # （docs/design/openapi-typescript-pipeline.md 参照）
├── benches/               # 負荷生成・計測ハーネス（TASK-1.2 で追加、bench-builder 管轄）
│   ├── README.md                      # 再現手順・複数回計測/中央値評価の規約
│   ├── lib/common.sh                  # 共通関数（サーバ起動/停止・中央値算出・依存ツール検査）
│   ├── lib/exclusive.sh               # NFR-6 専有計測用の相互排他（flock）・静穏確認・環境
│   │                                    # スナップショット（#178。並列 issue 実装ワークフロー下の
│   │                                    # host contention による NFR-6 判定不確定への対処）
│   ├── nfr6-exclusive.sh               # 専有実行枠で webrtc/graphql/hub の NFR-6 を順次計測・
│   │                                    # 判定確定する wrapper（#178、docs/design/
│   │                                    # nfr6-exclusive-measurement.md 参照）
│   ├── bench-accept-exclusive.sh       # 専有実行枠で bench-accept.sh（REQ-1/NFR-1/NFR-2 判定）を
│   │                                    # 実行する wrapper。REQ-2 基準 5（両 feature 無効時のコア
│   │                                    # 性能維持）の再計測に使用（#260、benches/reports/
│   │                                    # task-2.4-plugin-accept.md）。`FAIL_RETRIES`（既定 0）で
│   │                                    # 単発 FAIL の限定再試行に対応（#285、`.github/workflows/
│   │                                    # bench-schedule.yml` から週次 + workflow_dispatch で定期
│   │                                    # 実行し、REQ-1/NFR-1 の性能退行を継続検知する）。ビルド
│   │                                    # 成功直後・静穏確認前に baseline/core バイナリの実在を
│   │                                    # 検査する fail-fast を追加（イシュー #480。self-hosted
│   │                                    # runner のホスト共有 `CARGO_TARGET_DIR` 注入により決め打ち
│   │                                    # パスに成果物が見つからない事象への対処、
│   │                                    # `benches/lib/common.sh` の `BENCH_TARGET_DIR` 導出と対）
│   └── bench-http.sh / bench-rss.sh / bench-footprint.sh  # RPS・負荷時 RSS・起動時間/バイナリサイズ計測
├── scripts/               # CI・運用スクリプト（TASK-15.2 で追加）
│   ├── README.md                      # 使い方・前提ツール・CI との対応
│   ├── dep-audit.sh                   # 全 feature 構成の cargo audit / cargo deny check（ci.yml dep-audit ジョブ）
│   ├── dep-impact.sh                  # 依存クレート数・バイナリサイズ・unsafe 件数の計測（markdown 出力）
│   ├── setup-required-checks.sh       # main の required status check（ci-complete）設定（TASK-14.1、#39）
│   ├── commit-msg-check.sh            # Conventional Commits 形式のシェル検証（lefthook の
│   │                                    # commit-msg フックから呼ばれる。外部依存なし）
│   ├── openapi-two-stage.sh           # gen-openapi --check → cargo build --all-features の 2 段階ビルド検証（TASK-3.2、#31）
│   ├── openapi-ts.sh                  # gen-openapi --check → ts/ の schema.d.ts 鮮度検証 → tsc --noEmit の openapi-typescript 連携パイプライン検証（TASK-6.1、#54）
│   ├── openapi-ts-negative.sh         # openapi-ts.sh の陰性対照（意図的な型不一致の tsc --noEmit エラー検出）CI 常設検証（TASK-6.2、#55）
│   ├── clean-worktrees.sh             # .claude/worktrees/ 残存ワークツリーの棚卸し・退避・削除（既定 dry-run、--apply で実削除、イシュー #221）
│   ├── standalone-crates-io-check.sh  # templates/・examples/ の path 依存を除去し crates.io 公開版のみで build/test 検証（standalone-crates-io.yml から週次 + PR paths で実行、イシュー #371。対象クレート直下に `.standalone-crates-io-skip`（理由必須）があれば crates.io 未再公開の新 API 依存を理由に build/test を SKIP し、全件 SKIP なら fail-closed で異常終了する。次回 crates.io 再公開時の削除手順は docs/design/crates-io-release.md 8 節、イシュー #433）
│   ├── docs-site-visual.sh            # 刷新後の docs サイトを headless chromium でライト/ダーク/no-JS ×
│   │                                    # 複数解像度撮影し `docs/acceptance/issue399-docs-site-visual.md`
│   │                                    # の視覚証跡一式を生成（イシュー #399。CI 常設化はしない）
│   └── accept/            # 受け入れ検証スクリプト（TASK-1.6-2 で追加、以降 REQ-2/5/6/8/13 分も収録）
│       ├── README.md                  # 検証基準・前提ツール・実行方法
│       ├── lib/common.sh              # PASS/FAIL/SKIP/WARN 集計の共通関数
│       └── core-deps-unsafe-audit.sh  # 依存数比・unsafe・audit/deny・LoC・拡張点・プラグイン非依存の検証本体
└── .claude/
    ├── agents/            # 目的別 sub-agent（research/implement/testing/quality/docs）
    ├── rules/             # 運用ルール（委譲・Rust 規約・セキュリティ 等）
    ├── skills/            # 導入スキル（.agents/skills への symlink）
    ├── workflows/         # implement-issue-tree.js（symlink）
    └── settings.json      # SessionStart / PostToolUse hooks
```

## 委譲方針（必読）

main（あなた）のコンテキストは有限。**調査・読解・実装・レビューは subagent へ委譲し、
main は判断・統合・ユーザー対話に集中する**。詳細は [rules/delegation.md](.claude/rules/delegation.md)
（調査・設計）と [rules/delegation-impl.md](.claude/rules/delegation-impl.md)（作成・編集）を参照。

### パスベースの委譲先（要約）

| 対象 | 委譲先 Agent |
|------|-------------|
| `crates/core`・`http`・`routes` の実装 | `core-builder` |
| `crates/plugin-*` の実装 | `plugin-builder` |
| `axum-ref`・ベンチ・負荷試験 | `bench-builder` |
| コード・仕様の横断調査 | `explorer` |
| 外部仕様（crate / RFC）の調査 | `reference-researcher` |
| テスト・カバレッジ | `test-runner` |
| ファジング | `fuzz-runner` |
| 差分レビュー | `reviewer` |
| セキュリティ・依存監査 | `security-auditor` |
| 整形・lint | `linter` |
| ドキュメント | `docs-writer` |

### model 配分

| 用途 | model |
|------|-------|
| 複雑な横断判断・アーキテクチャ設計 | opus または fable（fable は大規模設計・複雑な横断判断の最上位 tier） |
| 調査・生成・実装・レビュー | sonnet |
| 機械的集計・lint・ドキュメント更新 | haiku |

## Sub-agents

`.claude/agents/<category>/<name>.md` に定義。

| カテゴリ | subagent_type | model | 役割 |
|---------|--------------|-------|------|
| research | `explorer` | sonnet | workspace・仕様の横断調査（読み取り専用） |
| research | `reference-researcher` | sonnet | 外部仕様（crate / RFC / axum 等）調査 |
| implement | `core-builder` | sonnet | HTTP コア・ルーティング・3 拡張点 |
| implement | `plugin-builder` | sonnet | feature 着脱プラグイン（ws/graphql/openapi/webrtc/hub/tracing） |
| implement | `bench-builder` | sonnet | 参照実装・Criterion ベンチ・負荷試験 |
| testing | `test-runner` | sonnet | cargo test・llvm-cov |
| testing | `fuzz-runner` | sonnet | cargo-fuzz / afl（パーサ検証） |
| quality | `reviewer` | sonnet | 差分の品質・アーキテクチャ準拠レビュー |
| quality | `security-auditor` | sonnet | cargo audit/deny/geiger・OWASP・unsafe 監査 |
| quality | `linter` | haiku | cargo fmt --check・clippy -D warnings |
| docs | `docs-writer` | haiku | CLAUDE.md / AGENTS.md / doc comment |

## Rules

`.claude/rules/` に定義。

| ファイル | 内容 |
|---------|------|
| [delegation.md](.claude/rules/delegation.md) | 調査・設計フェーズの委譲原則・パスベース切り替え |
| [delegation-impl.md](.claude/rules/delegation-impl.md) | 作成・編集フェーズの委譲マッピング・実装後フロー |
| [coding-rust.md](.claude/rules/coding-rust.md) | Rust 規約（安全性・並行性・拡張点・テスト） |
| [pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md) | feature 無効時の依存・unsafe・バイナリ完全除外原則 |
| [security.md](.claude/rules/security.md) | OWASP Top 10・メモリ安全性・秘密情報混入防止 |
| [japanese-style.md](.claude/rules/japanese-style.md) | 日本語出力スタイル |
| [conventional-commits.md](.claude/rules/conventional-commits.md) | Conventional Commits 詳細規約 |
| [code-comment-style.md](.claude/rules/code-comment-style.md) | コメント・doc comment 規約 |
| [out-of-scope-tracking.md](.claude/rules/out-of-scope-tracking.md) | 実装対象外の追跡（Issue 化）規約 |
| [improvement-proposal.md](.claude/rules/improvement-proposal.md) | 改善提案フロー・起票・承認の運用規約 |
| [feature-modification.md](.claude/rules/feature-modification.md) | 機能要求→実装→テスト→ドキュメント追随→完遂判定の一貫改修フロー運用規約 |
| [feasibility-guardrail.md](.claude/rules/feasibility-guardrail.md) | 対応可否自律判断ガードレール（曖昧要求・危険要求の不可判定規約） |
| [ci.md](.claude/rules/ci.md) | CI 実行環境規約（self-hosted runner 必須・timeout・schedule 負荷抑制） |

## Current Skills

`npx skills add` で導入済み（`skills-lock.json` 管理、`.agents/skills` → `.claude/skills` symlink）。

- **開発フロー**: `create-commit` / `create-pr` / `create-issue` / `create-issue-tree` /
  `create-plan` / `implement-issue` / `implement-issue-tree` / `implement-review` /
  `implement-review-pr` / `update-issue-tree`
- **プロジェクト管理**: `project-init` / `project-add-items` / `project-create-issues` /
  `project-update-items` / `project-view-status` / `project-sync-issues` / `project-archive-done`
- **ドキュメント・コメント**: `update-docs` / `comment-code`
- **.claude 体系**: `init-claude` / `update-claude`
- **スキル運用**: `contribute-skill` / `sync-skills-lock`
- **リファレンス**: `rust` / `github-docs` / `commitlint` / `lefthook` / `editorconfig`

## Conventions

- **言語**: ユーザーとのやりとり・コメント・コミット/PR/Issue 本文は日本語（[japanese-style](.claude/rules/japanese-style.md)）
- **コミット**: Conventional Commits 厳守・`--no-verify` 禁止（[conventional-commits](.claude/rules/conventional-commits.md)）。
  作成は `create-commit`、PR は `create-pr` skill
- **セキュリティ**: 変更ごとに OWASP Top 10・依存監査（[security](.claude/rules/security.md)）。
  `security-auditor` に委譲
- **設計原則**: pay-for-what-you-use を全実装で遵守（[pay-for-what-you-use](.claude/rules/pay-for-what-you-use.md)）
- **ユーザー承認フロー**: `implement-issue` 等は計画をユーザー承認後に実装。Issue 起票も承認前提
- **スコープ管理**: スコープ外課題は放置せず [out-of-scope-tracking](.claude/rules/out-of-scope-tracking.md) に従い Issue 化

## hooks（settings.json）

- **SessionStart**: 日本語 / 委譲 / pay-for-what-you-use / Conventional Commits（`--no-verify` 禁止） /
  implement-issue の計画承認フローをリマインド
- **PostToolUse**（Edit|Write）: `.rs` ファイル編集時に `rustfmt` で自動整形（rustfmt 未導入時は no-op）
