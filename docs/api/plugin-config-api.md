# プラグイン設定 API リファレンス

## 1. 目的と位置づけ

- 本ページは fandhe-backend の各プラグイン（`crates/plugin-*`）の feature 名・登録方法・Config 型・既定値・注意点を一覧化するリファレンスである
- 一次情報源は各クレートの rustdoc（`crates/plugin-*/src/lib.rs` ほかの doc comment・doc test）であり、本ページは横断比較と契約の要約を担う
- 登録先 `Server` の全体像は [サーバ API](./server-api.md)、配線パターンの設計判断は `docs/design/plugin-boundary.md`（<https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/plugin-boundary.md>）を参照
- curl 例を含む使い方の詳細は [feature 構成別サンプル](../guide/feature-samples.md) を参照し、本ページでは重複させない

### 全プラグイン対応早見表

| プラグイン | feature 名 | 登録メソッド | 配線パターン種別 |
|-----------|-----------|-------------|-----------------|
| plugin-websocket | `websocket` | `Server::websocket(WebSocketConfig)` | Upgrade 型（`UpgradeHandler` 拡張点） |
| plugin-graphql | `graphql` | `Server::graphql(GraphQlConfig)` | パスインターセプト型 |
| plugin-openapi | `openapi` | `Server::openapi()` / `Server::openapi_with(OpenApiDoc)` | 設定登録型（静的サービング） |
| plugin-cors | `cors` | `Server::cors(CorsConfig)`（+ `Router::options_fallback` 配線） | レスポンス後処理型（`finalize_response` シーム） |
| plugin-compression | `compression` | `Server::compression(CompressionConfig)` | レスポンス後処理型（CORS の後に逐次適用） |
| plugin-static | `static` | `Server::static_files(StaticFilesConfig)` | パスインターセプト型 + `spawn_blocking` 変種 |
| plugin-tracing | `tracing` | `Server::tracing(TracingConfig)` | Middleware 型（`Middleware` 拡張点） |
| plugin-webrtc-proxy | `webrtc-proxy` | `Server::webrtc_proxy(ProxyConfig)` | パスインターセプト型（別プロセス委譲） |
| plugin-webrtc | `webrtc` | `Server::webrtc(WebRtcConfig)` | パスインターセプト型（in-process） |
| plugin-hub-wiring | なし（依存逆転型） | `Server::gate(TenantGate::new(config))` | Gate 型（`RequestGate` 拡張点） |

- feature はすべて `fandhe-backend-core` の Cargo feature（`hub-wiring` を除く）。無効時は依存・コード・バイナリ増がゼロになる（pay-for-what-you-use）
- いずれも**登録時のみ動作する opt-in**。feature 有効でも未登録ならフォールスルーし、既定挙動を変えない

## 2. プラグイン別リファレンス

### 2.1 plugin-websocket（`websocket`）

RFC 6455 ハンドシェイク検証・101 応答・tokio-tungstenite へのフレーミング委譲。

| 項目 | 内容 |
|------|------|
| Config 型 | `WebSocketConfig`（`Default` 実装あり） |
| builder メソッド | `with_path` / `with_max_message_size` / `with_max_frame_size` / `with_idle_timeout` / `without_idle_timeout` / `with_handler` / `with_close_grace` |
| 既定値 | `path = "/ws"`、`max_message_size = 1 MiB`、`max_frame_size = 256 KiB`、`idle_timeout = Some(60 秒)`、`close_grace = 10 秒` |
| メッセージハンドラ | `with_handler(impl WsMessageHandler)` で差し替え。既定は `EchoHandler`（後方互換） |

- 注意: サイズ上限はメモリ枯渇 DoS 対策。アイドルタイムアウトは既定で有効（fail-safe）であり、無効化は `without_idle_timeout` の明示操作でのみ可能
- 注意: `close_grace`（`with_close_grace`）はコアの世代キャンセル（最終 graceful
  shutdown・rebind 世代 drain）発火時の Close ハンドシェイク猶予。
  `fandhe_backend_plugin_websocket::handle_upgrade` の第 5 引数（キャンセル
  `Future`）が発火すると close code 1001 Going Away を送出し、`close_grace` を
  上限にクライアント応答を有界に待つ（v0.3.0 での BREAKING CHANGE、イシュー
  #492/#496）。`WsMessageHandler::on_message` が返す `Future` は任意の
  `await` 点で drop されうる契約（イシュー #499、
  [`docs/design/ws-cancellation-propagation.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/ws-cancellation-propagation.md)）

### 2.2 plugin-graphql（`graphql`）

`POST /graphql`（`GRAPHQL_PATH` 定数）のパスインターセプト。async-graphql による実クエリ実行。

| 項目 | 内容 |
|------|------|
| Config 型 | `GraphQlConfig` |
| 構築 | `GraphQlConfig::new(executor)`（`executor: Executor + Clone + Send + Sync + 'static`。`async_graphql::Schema` が満たす） |
| 既定値 | なし（スキーマ必須。深さ・複雑度制限の既定値も**提供しない**） |

- 注意: クエリ深さ・複雑度制限（`Schema::limit_depth` / `limit_complexity`）と introspection 無効化（`Schema::disable_introspection`）はスキーマ登録者の責務
- 不正な body は 400 + 固定エラー body（リクエスト由来の値を一切エコーしない）。未登録時は feature 有効でもフォールスルー

### 2.3 plugin-openapi（`openapi`）

`GET /openapi.json` / `GET /openapi.yaml` の静的サービング（明示登録時のみ）。

| 項目 | 内容 |
|------|------|
| 登録（組み込みスキーマ） | `Server::openapi()`（`ApiDoc` 由来の埋め込み `OPENAPI_JSON` / `OPENAPI_YAML` を配信） |
| 登録（独自スキーマ） | `Server::openapi_with(OpenApiDoc)` |
| Config 型 | `OpenApiDoc` |
| 構築 | `OpenApiDoc::from_json(json)`（構築時 JSON 検証、`Result<_, OpenApiDocError>`）+ `with_yaml(yaml)`（任意） |

- 注意: `openapi()` と `openapi_with()` は排他ではなく**後勝ち**（最後に呼んだ方の登録が残る）。YAML 変換依存は開発用 `gen-cli` feature に閉じ、サーバ経路には現れない

### 2.4 plugin-cors（`cors`）

実リクエスト応答への CORS ヘッダ付与（レスポンス後処理型）+ プリフライト応答関数の 2 点構成。外部依存ゼロ。

| 項目 | 内容 |
|------|------|
| Config 型 | `CorsConfig`（`CorsConfig::builder()` 経由でのみ構築） |
| builder メソッド | `allow_origin`（完全一致・複数回可） / `allow_any_origin` / `allow_methods` / `allow_headers` / `allow_credentials` / `max_age` / `expose_headers` / `build` |
| 既定値 | オリジン空リスト（何も許可しない）、`methods = None`（プリフライトは対象パスの実登録メソッドを反映）、credentials 無効、`max_age` なし |
| 構築時検証 | `build()` は `Result<CorsConfig, CorsConfigError>`。`allow_any_origin()` と `allow_credentials(true)` の併用は `AnyOriginWithCredentials` で拒否（トークン窃取経路を型レベルで排除） |
| プリフライト | `preflight_response` を利用者が `Router::options_fallback` へ直接配線する（`is_preflight` で判定可能）。配線例は [feature 構成別サンプル](../guide/feature-samples.md) |

- 注意: オリジン照合はバイト完全一致でありワイルドカード部分一致はない。ヘッダ付与に失敗した場合は当該ヘッダを付与しない側へ倒す（フェイルクローズ）

### 2.5 plugin-compression（`compression`）

条件充足レスポンスの gzip 圧縮（レスポンス後処理型の第 2 インスタンス、CORS の後に逐次適用）。外部依存は `flate2`（純 Rust の `rust_backend`）のみ。

| 項目 | 内容 |
|------|------|
| Config 型 | `CompressionConfig`（`CompressionConfig::builder()` 経由） |
| builder メソッド | `min_size` / `compressible_types`（丸ごと差し替え） / `add_compressible_type` / `build`（失敗しない） |
| 既定値 | `min_size = 1024` バイト、圧縮対象 `Content-Type` は `text/`（プレフィックス）・`application/json`・`application/javascript`・`application/xml`・`application/xhtml+xml`・`image/svg+xml` |
| 圧縮判定 | ステータス・`Content-Type`・body サイズ・`Accept-Encoding`（`q` 値解釈込み）をすべて満たす場合のみ圧縮。解釈不能・条件未充足は無圧縮のまま返す（フェイルセーフ） |

- 注意: gzip のみ（br はスコープ外）。秘密情報とリクエスト反映値を同一圧縮 body に同居させると BREACH 類似の情報漏洩リスクがある（rustdoc に明記）

### 2.6 plugin-static（`static`）

`GET` の静的ファイル配信（パスインターセプト型 + `spawn_blocking` 変種）。外部依存ゼロ（`fandhe-backend-http` + `tokio` の `rt` feature のみ）。

| 項目 | 内容 |
|------|------|
| Config 型 | `StaticFilesConfig`（`StaticFilesConfig::builder(mount, root)` 経由でのみ構築） |
| builder メソッド | `max_file_bytes` / `mime(ext, content_type)`（内蔵 MIME テーブルより優先する拡張マッピング） / `build` |
| 既定値 | `max_file_bytes = DEFAULT_MAX_FILE_BYTES`（8 MiB。1 リクエストあたりのメモリ使用上限そのもの）。`mime` 未登録時は内蔵テーブル（`.webmanifest` 等を含む）+ 既定 `application/octet-stream` |
| 構築時検証 | `build()` は `Result<_, StaticConfigError>`。`mount` の形式不正（`InvalidMount`）・`root` の canonicalize 失敗（`RootNotAccessible`）・非ディレクトリ（`RootNotADirectory`）・`mime` マッピングの拡張子/Content-Type 不正（`InvalidMimeMapping`、CR/LF 等のヘッダインジェクション対策込み）を起動前に検出 |

- 注意: 二層防御（I/O 前の字句検証 + canonicalize 後の root 配下検証）でパストラバーサル・シンボリックリンク脱出を拒否。先頭ドットセグメント（`.env`・`.git/config` 等）も配信拒否。未検出・検証失敗・サイズ超過は**一律 404**（存在秘匿）

### 2.7 plugin-tracing（`tracing`）

サンプリング付きトレーシング（Middleware 型の第 1 号）。決定的カウンタ方式 + 非同期・バッファ済み I/O。

| 項目 | 内容 |
|------|------|
| Config 型 | `TracingConfig`（公開フィールド `sample_interval: NonZeroU64` / `exclude_paths: Vec<String>`） |
| 構築・builder | `TracingConfig::default()` / `TracingConfig::new(sample_interval)` / `exclude_path(path)`（チェーン可能） |
| 既定値 | `sample_interval = 100`（100 リクエストに 1 回記録）、`exclude_paths` 空 |
| 出力初期化 | `init_tracing(TracingOutput) -> WorkerGuard`（tracing-appender の non_blocking writer。`WorkerGuard` は drop までログをフラッシュし続けるため保持必須） |

- 注意: `exclude_paths` はクエリ除去後パスとの**完全一致**のみ（プレフィックス・glob 非対応。ログ抑制範囲の意図しない拡大を防ぐ安全側の設計）。除外パスはサンプラーのカウンタも消費しない

### 2.8 plugin-webrtc-proxy（`webrtc-proxy`）

WebRTC シグナリングの別プロセス切り出し型プロキシ。`POST /rtc/offer`（`OFFER_PATH` 定数）を upstream へ委譲する。攻撃表面をプロセス境界で分離でき、MVP 推奨。

| 項目 | 内容 |
|------|------|
| Config 型 | `ProxyConfig` |
| 構築・builder | `ProxyConfig::new(upstream_addr)` + `with_upstream_path` / `with_connect_timeout` / `with_request_timeout` / `with_max_offer_bytes` / `with_max_answer_bytes` |
| 既定値 | `upstream_path = "/rtc/offer"`、`connect_timeout = 3 秒`、`request_timeout = 5 秒`、`max_offer_bytes = max_answer_bytes = 64 KiB` |

- 注意: offer / answer 双方向にサイズ上限を持つ（upstream 応答の肥大もフェイルクローズで遮断）。`webrtc` feature と同時有効時は `webrtc-proxy` が優先評価される

### 2.9 plugin-webrtc（`webrtc`）

in-process WebRTC（`webrtc-rs` 直接依存）。`POST /rtc/offer` を同一プロセス内で処理する。

| 項目 | 内容 |
|------|------|
| Config 型 | `WebRtcConfig` |
| 構築・builder | `WebRtcConfig::new()` + `with_max_offer_bytes` / `with_max_peer_connections` / `with_signaling_timeout`（getter: `max_offer_bytes` / `max_peer_connections` / `signaling_timeout`） |
| 既定値 | `max_offer_bytes = 64 KiB`、`max_peer_connections = 64`、`signaling_timeout = 10 秒` |
| drain API | `close_active_peers(&config, per_close_timeout)` / `drain_for_shutdown(&config, per_close_timeout)`（`drain` モジュール、イシュー #498） |

- 注意: `webrtc-rs` の依存ツリーが大きく攻撃表面が広いため、クレート境界で完全分離されている。まず `plugin-webrtc-proxy` の採用を検討すること
- 注意: `close_active_peers` / `drain_for_shutdown` はいずれも `WebRtcConfig::registry`
  上のアクティブな `RTCPeerConnection` を 1 接続あたり `per_close_timeout` の有界
  タイムアウトで並行に明示 close する。`drain_for_shutdown` のみ
  `WebRtcConfig::begin_terminal_drain` で以降の新規登録を拒否するフェイルクローズ
  判定を伴う（`close_active_peers` は rebind 用途を想定し新規登録は拒否しない）。
  コアの `SessionDrain`（`webrtc` feature ゲート、独立シーム）が最終 graceful
  shutdown・rebind の両経路から自動でこれらを呼ぶため、通常は利用側が直接呼ぶ
  必要はない（[`docs/design/ws-cancellation-propagation.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/ws-cancellation-propagation.md)
  10 節を参照）

### 2.10 plugin-hub-wiring（feature なし・依存逆転型）

hub 共通配線（JWT RS256 / JWKS / テナント境界強制・同意ゲート・outbox・監査）。他プラグインと異なりコア側 feature を持たず、**利用側サービスが本クレートを依存に追加してコアの `RequestGate` 拡張点へ登録する**（プラグイン → コアの一方向依存）。

| 項目 | 内容 |
|------|------|
| 登録 | `Server::gate(TenantGate::new(TenantGateConfig::from_jwks_json(jwks_json)?))` |
| `TenantGate` | `RequestGate` 実装。Bearer JWT の RS256 検証とテナント境界強制を同期・I/O なしで行う |
| `TenantGateConfig` | `new(SharedJwks)` / `from_jwks_json(&str)` で構築。`authenticator()` で `Authenticator`（検証結果キャッシュ。ゲート通過時に温まり、ハンドラ側の再検証で RS256 署名検証を再実行しない）を取得 |
| `SharedJwks` | 再起動なしの鍵ローテーション対応 JWKS 保持（`snapshot` / `set`）。JWKS の HTTP 取得・自動リフレッシュは利用側の責務 |
| `ConsentStore` / `OutboxStore` | ゲート通過後にハンドラ層から呼ぶストレージ抽象 trait。同梱はテスト用インメモリ実装（`InMemoryConsentStore` / `InMemoryOutboxStore`）のみで、DB 実装は利用側が提供 |
| `AuditSink` | 越境アクセス監査ログの出力先 trait（`MemoryAuditSink` 同梱）。外部応答は正当な 404 と越境 404 で完全同一のまま、監査ログのみで区別する |

- 注意: 検証**失敗**はキャッシュしない（キャッシュ汚染防止）。各型の詳細契約は rustdoc（`crates/plugin-hub-wiring/src/{gate,auth,jwks,consent,outbox,audit}.rs`）を参照

## 3. 契約・不変条件（横断）

- **opt-in 契約**: すべてのプラグインは feature 有効化 + `Server::xxx(...)` 登録の 2 段階が揃って初めて動作する。未登録時のリクエスト挙動は feature 無効時と同一
- **構築時検証（フェイルクローズ）**: 不正設定は実行時ではなく構築時に `Result` で拒否する（`CorsConfigError` / `StaticConfigError` / `OpenApiDocError`）。実行時に初めて失敗する経路を作らない
- **DoS 上限内蔵**: サイズ・接続数・タイムアウトの上限は既定で有効。無効化・緩和は明示操作のみ
- **レスポンス後処理の適用順**: `finalize_response` シームで CORS → compression の順に逐次適用される

## 4. セキュリティ観点

| プラグイン | 主な観点 |
|-----------|---------|
| websocket | メッセージ / フレームサイズ上限・アイドルタイムアウト既定有効（メモリ枯渇・接続占有 DoS 対策） |
| graphql | 深さ・複雑度制限と introspection 無効化は登録者責務（既定値を提供しないことを明示）。エラー応答にリクエスト由来値を含めない |
| cors | `Any` オリジン + credentials の最悪構成を構築時拒否。オリジンはバイト完全一致のみ |
| compression | BREACH 類似リスク（秘密 + 反映値の同居 body）を rustdoc に明記。判定不能は無圧縮側へ倒す |
| static | 二層防御によるパストラバーサル・シンボリックリンク脱出拒否、機密ファイル（先頭ドット）遮断、一律 404 による存在秘匿 |
| tracing | 除外は完全一致のみ（可観測性の穴の拡大防止）。ログに機密を出さない運用前提 |
| webrtc / webrtc-proxy | ペイロードサイズ・接続数・タイムアウト上限。攻撃表面の観点で proxy 型を推奨 |
| hub-wiring | RS256 + JWKS・鍵ローテーション・検証失敗の非キャッシュ・越境アクセスのフェイルクローズ遮断と監査 |

## 5. スコープ外・関連ドキュメント

- **スコープ外**: 各プラグインのプロトコル仕様詳細（RFC 6455 / GraphQL over HTTP / SDP 等）、curl での動作確認手順（[feature 構成別サンプル](../guide/feature-samples.md)）、hub-wiring の JWT / JWKS / outbox の詳細契約（rustdoc）、TLS 終端・multipart（v1 スコープ外。`docs/design/v1-scope-tls-multipart.md`）
- `Server` 本体の設定（接続数・body 上限・タイムアウト・graceful shutdown）: [サーバ API](./server-api.md)、[graceful shutdown ガイド](../guide/graceful-shutdown.md)
- ルーティングとの接続（`options_fallback` / パスインターセプトの前提）: [ルーティング API](./router-api.md)
- 拡張点の契約（`Middleware` / `UpgradeHandler` / `RequestGate`）: [拡張 API](./extension-api.md)、[拡張点ガイド](../guide/extension-points.md)
- 配線パターンの設計判断・境界検証: `docs/design/plugin-boundary.md`（<https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/plugin-boundary.md>）
- 入門: [Getting Started](../guide/getting-started.md)、[チュートリアル](../guide/tutorial.md)
