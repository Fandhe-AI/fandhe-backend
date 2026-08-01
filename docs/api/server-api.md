# fandhe-backend-core サーバ API リファレンス

## 1. 目的と位置づけ

本書は `fandhe-backend-core` クレートが公開するサーバ構築 API（`Server` ビルダー・
`BoundServer`・`Handler` trait・`streaming` モジュール）の全体像・契約・feature 前提を
俯瞰する読み物である。個々のシグネチャ・doc test を含む一次情報源は rustdoc
（`crates/core/src/server.rs`・`crates/core/src/streaming.rs` の doc comment）であり、
本書と記述が食い違う場合は rustdoc を正とする。

- 対象クレート: `crates/core`（crate 名 `fandhe-backend-core`）
- 主要型はクレート直下に re-export されている（`fandhe_backend_core::Server` /
  `BoundServer` / `Handler` / `StreamingResponse` / `BodyWriter` / `StreamClosed`）
- 3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）の契約は
  [extension-api.md](./extension-api.md) を参照
- 導入手順は [../guide/getting-started.md](../guide/getting-started.md)、feature 構成別の
  組み合わせ例は [../guide/feature-samples.md](../guide/feature-samples.md) を参照

## 2. 公開 API 一覧

### 2.1 `Server` ビルダー（無条件公開）

各メソッドは `self` を消費して返すメソッドチェーン形式。`bind()` 以降は
`Arc<Server>` として不変共有される。

| メソッド | シグネチャ概略 | 説明 |
|---------|---------------|------|
| `Server::new` | `fn new() -> Server` | 拡張点・ハンドラを持たない空のサーバを作る |
| `max_connections` | `fn (usize) -> Server` | 同時接続数上限（既定 10,000）。`0` は `bind` 側で `1` に切り上げ |
| `max_connection_lifetime` | `fn (Duration) -> Server` | 1 接続の総生存期間上限（既定 300 秒） |
| `max_requests_per_connection` | `fn (usize) -> Server` | keep-alive 接続 1 本あたりのリクエスト数上限（既定 1,000）。`0` でも最低 1 件は処理 |
| `max_body_bytes` | `fn (usize) -> Server` | body 許容最大バイト数（既定 1 MiB）。超過は 413 |
| `read_timeout` | `fn (Duration) -> Server` | read 1 回あたりのタイムアウト（既定 30 秒） |
| `keep_alive` | `fn (bool) -> Server` | keep-alive の有効/無効（既定有効）。無効時は常に `Connection: close` |
| `shutdown_grace_period` | `fn (Duration) -> Server` | graceful shutdown の in-flight 完了待ち上限（既定 30 秒） |
| `middleware` | `fn (impl Middleware + 'static) -> Server` | 観測専用フックを登録（登録順に呼び出し） |
| `gate` | `fn (impl RequestGate + 'static) -> Server` | 早期拒否ゲートを登録（登録順評価・最初の Reject 優先） |
| `upgrade_handler` | `fn (impl UpgradeHandler + 'static) -> Server` | Upgrade 委譲判定を登録（登録順に `matches` 評価） |
| `interceptor` | `fn (impl Interceptor + 'static) -> Server` | インターセプト・レスポンス改変拡張点を登録（複数登録可・登録順評価、詳細は [interceptor-api.md](./interceptor-api.md) 参照） |
| `handler` | `fn (impl Handler + 'static) -> Server` | 既定ハンドラを登録。未登録時は 404 |
| `bind` | `async fn (impl ToSocketAddrs) -> io::Result<BoundServer>` | TCP リスナーをバインドし `BoundServer` を返す |

### 2.2 feature ゲート付きプラグイン登録メソッド

すべて対象 feature 有効時のみ存在する（無効時はメソッド・依存・コードとも消える。
pay-for-what-you-use）。いずれも「設定登録型」であり、**feature を有効にしただけでは
動作せず、メソッドで明示登録した場合のみ有効化される**（fail-closed）。

| メソッド | feature | シグネチャ概略 | 説明 |
|---------|---------|---------------|------|
| `webrtc_proxy` | `webrtc-proxy` | `fn (ProxyConfig) -> Server` | `POST /rtc/offer` を上流 WebRTC サービスへ中継（別プロセス切り出し型） |
| `webrtc` | `webrtc` | `fn (WebRtcConfig) -> Server` | `POST /rtc/offer` を in-process の `RTCPeerConnection` で処理。`webrtc-proxy` と同時登録時は `webrtc-proxy` 優先 |
| `websocket` | `websocket` | `fn (WebSocketConfig) -> Server` | 指定パス（既定 `/ws`）への WebSocket アップグレードを受理。複数回呼び出しで複数パス登録可 |
| `graphql` | `graphql` | `fn (GraphQlConfig) -> Server` | `POST /graphql` を登録スキーマで実行 |
| `openapi` | `openapi` | `fn () -> Server` | フレームワーク固定スキーマで `GET /openapi.json` / `GET /openapi.yaml` を配信 |
| `openapi_with` | `openapi` | `fn (OpenApiDoc) -> Server` | 利用者アプリ独自スキーマを配信。`openapi()` とは後勝ち |
| `cors` | `cors` | `fn (CorsConfig) -> Server` | 実リクエスト応答へ CORS ヘッダを付与（プリフライトは `Router::options_fallback` へ別途配線） |
| `compression` | `compression` | `fn (CompressionConfig) -> Server` | 条件充足レスポンスを gzip 圧縮（CORS 付与の後に適用） |
| `static_files` | `static` | `fn (StaticFilesConfig) -> Server` | マウントプレフィックス配下の `GET` に静的ファイルを配信 |
| `tracing` | `tracing` | `fn (TracingConfig) -> Server` | サンプリング付きトレーシング Middleware を内部登録（記録先初期化は `init_tracing` が別途担う） |

各設定型（`CorsConfig` 等）の詳細は [plugin-config-api.md](./plugin-config-api.md) を参照。

### 2.3 `BoundServer`

| メソッド | シグネチャ概略 | 説明 |
|---------|---------------|------|
| `local_addr` | `fn (&self) -> io::Result<SocketAddr>` | バインド済みローカルアドレス。`0` ポート指定時の実ポート確認に使う |
| `run` | `async fn (self) -> io::Result<()>` | accept ループを回す。シャットダウン手段を持たない `run_until` への薄い委譲 |
| `run_until` | `async fn <F: Future<Output = ()>>(self, shutdown: F) -> io::Result<()>` | `shutdown` 完了まで accept し、その後 graceful shutdown シーケンスを実行 |

graceful shutdown の詳細な挙動・利用パターンは
[../guide/graceful-shutdown.md](../guide/graceful-shutdown.md) を参照。

### 2.4 `Handler` trait

コアが公開する既定ハンドラ拡張点。同期 3 拡張点（`extension` モジュール、
「拡張点は 4 種に集約」のうち `Interceptor` を除く 3 種）とは別枠の、ルーティング
結果を最終応答へ変換する差し込み口である。

| メソッド | シグネチャ概略 | 必須/opt-in | 説明 |
|---------|---------------|------------|------|
| `handle` | `fn (&self, &RequestHead, &[u8]) -> HandlerFuture` | 必須 | リクエストから応答を組み立てる future を返す。`HandlerFuture` は `Pin<Box<dyn Future<Output = Response> + Send>>`（`fandhe-backend-routes` 定義） |
| `handle_streaming` | `fn (&self, &RequestHead, &[u8]) -> Option<StreamingResponse>` | opt-in（既定実装は `None`） | `Some` を返すと chunked ストリーミング送信経路に切り替わる。既存実装は無変更で後方互換 |

`fandhe_backend_routes::Router` は `impl Handler for Router` により
そのまま `Server::handler` へ登録できる（`Router::dispatch` への薄いアダプタ。
ルーティング意味論は `crates/routes` 側の責務のまま）。`Router` 自体の API は
[router-api.md](./router-api.md) を参照。

### 2.5 `streaming` モジュール

レスポンス側 chunked ストリーミング送信の opt-in API。使い方の詳細は
[../guide/streaming.md](../guide/streaming.md) を参照。

| 型・メソッド | シグネチャ概略 | 説明 |
|-------------|---------------|------|
| `StreamingResponse::new` | `fn (u16) -> (StreamingResponse, BodyWriter)` | 既定チャネル容量（8）でストリーミング応答を組み立てる |
| `StreamingResponse::channel` | `fn (u16, Option<&'static str>, usize) -> (StreamingResponse, BodyWriter)` | status / Content-Type / bounded mpsc 容量を明示指定。容量 `0` は `1` に切り上げ |
| `StreamingResponse::status` | 公開フィールド `pub status: u16` | 応答ステータスコード |
| `BodyWriter::send` | `async fn (&self, Vec<u8>) -> Result<(), StreamClosed>` | 1 チャンク送出。チャネル満杯時はバックプレッシャで待機。空データはワイヤ無出力 |
| `BodyWriter::finish` | `async fn (self) -> Result<(), StreamClosed>` | 正常終端。`self` 消費で「finish 後の send」を型で防ぐ |
| `StreamClosed` | `struct`（`Error` 実装） | 受信側（コアの書き出しループ）終了後の送信を示すエラー |

## 3. 契約・不変条件

1. **ビルダーは bind 後不変**: `Server::bind` 以降は `Arc<Server>` として複数
   コネクションタスクから共有参照される。拡張点実装に `Send + Sync` が要求される
   のはこのため。
2. **プラグインは明示登録が必須（fail-closed）**: feature 有効かつ未登録の場合、
   パスインターセプト型はフォールスルー（404）、レスポンス後処理型は無変更。
3. **リクエスト処理順序**: `Middleware::on_request` → `RequestGate::check` →
   `UpgradeHandler::matches` → パスインターセプト型プラグイン → `Handler::handle` →
   レスポンス後処理型プラグイン → 書き込み → `Middleware::on_response`。
   詳細は [extension-api.md](./extension-api.md) を参照。
4. **`Handler::handle` は async・`handle_streaming` は同期という非対称設計**:
   `handle` はハンドラ本体で非同期 I/O（`sqlx` 等）を直接 `.await` できる。
   `handle_streaming` はチャンネルを組み立てて即座に返すだけでよいため同期のまま。
   ハンドラ内 panic は接続単位の spawn タスクに閉じ込められ、他接続へ波及しない。
5. **ストリーミングの応答完全性**: `BodyWriter::finish` を呼ばずに drop した場合、
   コアは終端チャンクを送出せず接続をクローズする。打ち切られた応答を完全な応答と
   してクライアント・キャッシュに誤認させない（RFC 9112 の length 整合性）。
6. **`run()` の後方互換**: `run()` は `run_until(std::future::pending())` への薄い
   委譲であり、挙動・シグネチャとも従来のまま。
7. **graceful shutdown は有界時間で必ず戻る**: `run_until` は shutdown 後、
   accept 停止 → in-flight 完了待ち（`shutdown_grace_period` 上限）→ 超過分の
   強制クローズの順で処理し、grace + ε 以内に必ず `Ok(())` で戻る。
   `run_until` 自体が外部キャンセル（`tokio::select!` 等）された場合、in-flight
   接続は abort されず独立タスクとして完走する。
8. **`openapi()` / `openapi_with()` は後勝ち**: 排他ではなく、最後に呼んだ方の
   登録が残る（ビルダーの直感に一致）。

## 4. セキュリティ観点

- **リソース枯渇（DoS）対策は多層**: 同時接続数（semaphore 強制、超過分は listen
  backlog → OS 拒否）・接続生存期間・接続あたりリクエスト数・body サイズ・
  読み取りタイムアウトの 5 上限がすべて既定で有効。実効 read タイムアウトは常に
  残り生存期間との短い方へ丸められ、大値設定でも総占有時間は
  `max_connection_lifetime` を超えない。
- **`max_body_bytes` の増加はトレードオフ**: 最悪ケースのバッファリングメモリは
  `max_body_bytes × max_connections` に比例する。大値設定は DoS 耐性の後退に
  なりうることを踏まえて判断する。413 応答に内部の上限値は含めない。
- **ストリーミングにも書き込みタイムアウト**: producer からの次チャンク待ち・
  ソケット実書き込みの双方に 30 秒（固定、調整 API なし）が適用される。SSE の
  ハートビート等でアイドル区間が長い producer は、30 秒未満の間隔で
  `BodyWriter::send(Vec::new())`（ワイヤ無出力）を呼び内部キープアライブとする。
- **OpenAPI 配信は既定非公開**: API 構造の開示となるため、`Server::openapi()` の
  明示登録を必須とする（fail-closed）。
- **shutdown 後の Upgrade は 503 拒否**: shutdown フラグ受信後に到着した
  Upgrade リクエストはプラグインへ委譲せず拒否する。
- **accept エラーで停止しない（可用性）**: 一過性 accept エラー（`ECONNABORTED`・
  fd 枯渇等）はログの上バックオフ付きで再試行し、リスナー全体を停止させない。
- **圧縮の情報漏洩リスク**: `compression` 登録時は BREACH 類似の攻撃を考慮する
  （`crates/plugin-compression` の rustdoc に明記）。

## 5. スコープ外・関連ドキュメント

- 3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）の契約:
  [extension-api.md](./extension-api.md)
- `Interceptor`（4 種目の拡張点、インターセプト・レスポンス改変）の契約:
  [interceptor-api.md](./interceptor-api.md)
- HTTP プリミティブ（`RequestHead` / `Response` / パーサ群）: [http-api.md](./http-api.md)
- ルーティング（`Router`）: [router-api.md](./router-api.md)
- プラグイン設定型: [plugin-config-api.md](./plugin-config-api.md)
- 導入・チュートリアル: [../guide/getting-started.md](../guide/getting-started.md) /
  [../guide/tutorial.md](../guide/tutorial.md)
- ストリーミング送信の利用ガイド: [../guide/streaming.md](../guide/streaming.md)
- graceful shutdown の利用ガイド: [../guide/graceful-shutdown.md](../guide/graceful-shutdown.md)
- TLS 終端・multipart/form-data はフレームワーク本体のスコープ外
  （TLS はリバースプロキシ前提。方針は `docs/design/v1-scope-tls-multipart.md` を参照）
- 設計判断の記録（graceful shutdown・async ハンドラ・プラグイン境界）は
  `docs/design/graceful-shutdown.md`・`docs/design/async-handler.md`・
  `docs/design/plugin-boundary.md` を参照
