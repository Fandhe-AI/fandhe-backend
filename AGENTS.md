# AGENTS.md

## 文書の位置づけ

本リポジトリで作業するすべての AI エージェント・開発者が従う設計規約集。二つの役割を持つ:

1. 実装コード（`crates/**`）から直接参照される横断的な設計規約（例:
   「規約: ミドルウェア非同期 I/O 必須化」節）。`CLAUDE.md` / `.claude/rules/` と
   内容を重複させない
2. AI エージェントが安全に改修するための変更ガイド（「AI エージェント向け変更ガイド」
   節、TASK-11.3・#35）。REQ-11 が要求する機械可読性のため、モジュール境界・変更手順・
   完了判定・エスカレーション基準等の要点を本書に集約するが、各項目の一次情報源
   （`docs/design/*.md`・`.claude/rules/*.md`）を正とし、詳細はそちらを参照する

全体の運用ガイドは `CLAUDE.md`、Rust コーディング規約の詳細は `.claude/rules/`
（特に [coding-rust.md](.claude/rules/coding-rust.md)）を参照する。

## 規約: ミドルウェア非同期 I/O 必須化

TASK-2.3（`docs/spec/05-tasks.md`、Phase 1 / MS-1、親 Issue #4、前提 TASK-2.1 #18）
対応。`docs/spec/04-requirements.md` REQ-2 受け入れ基準・NFR-7 を満たす規約文書。

### 規約本文

全リクエストに介入する `Middleware` 実装（`crates/core/src/extension.rs` の
`Middleware` trait、`on_request` / `on_response`）は**非同期・バッファ済み I/O を
既定**とする。同期ブロッキング I/O 実装（同期 `eprintln!`・同期ファイル書き込み・
`std::net` 直接利用等）は**不採用**とする。

`Middleware` trait 自体は dyn 互換性（`Box<dyn Middleware>` としてコアループが
拡張点を保持する構成）を保つため `async fn` を持たない同期 API として定義される
（`crates/core/src/extension.rs` モジュール doc「非同期・I/O に関する規約」節）。
本規約はこの同期 API の**制約下で守るべき実装契約**であり、trait のシグネチャ変更
を求めるものではない。

### 実装パターン

I/O が必要な実装は、フック（`on_request` / `on_response`）内では非同期チャネルへの
送信、またはアトミックカウンタの更新等の**非ブロッキング操作に留め**、実際の I/O
（ファイル書き込み・ネットワーク送信等）は別タスク（バックグラウンドタスク・
`tracing-appender` の non-blocking writer 等）に委譲する。

### 根拠（PoC-3 実測、`docs/spec/03-poc/plugin-mechanism/README.md`）

全リクエストに介入するミドルウェア型プラグイン（ロギング）を素朴な同期 I/O
（リクエストごとの同期 `eprintln!`）で実装すると、`/health` の RPS が
**725,024 → 44,108 RPS（無効時比 25.0%）** まで劣化した。同一の `Middleware`
trait 実装のまま I/O を停止し、アトミックカウンタの更新のみに切り替えて計測
（`ACCESS_LOG_QUIET=1`）すると **177,549 RPS（無効時比 100.5%）** まで回復した。

この切り分けにより、劣化要因は「`Middleware` trait 呼び出し（動的束縛）のコスト
自体」ではなく「プラグインが選んだ I/O 実装の質（同期か非同期か）」であることが
実証された。

補足として、PoC-10（`docs/spec/04-requirements.md` REQ-10）でも同旨の実測がある。
可観測性ミドルウェアを同期 writer で実装した場合に RPS が **63.0% 劣化**すること
に加え、非同期 writer に切り替えても span/event 生成の CPU コストにより RPS が
31.6% 劣化する事例が確認されており、**非同期 I/O 化だけでは pay-for-what-you-use
の性能目標を満たさない場合がある**（サンプリング・イベント数削減・高頻度パス除外
等の追加対策は REQ-10 側のスコープであり、本規約は「同期 I/O の不採用」という
最小限の必須要件を定めるものである）。

### 出典リンク

- `docs/spec/03-poc/plugin-mechanism/README.md`（PoC-3 性能比較表・発見事項）
- `docs/spec/02-poc-plan.md`（PoC-3 計画）
- `docs/spec/04-requirements.md`（REQ-2・NFR-7、参考: REQ-10・PoC-10）
- `docs/spec/05-tasks.md`（TASK-2.3）
- `crates/core/src/extension.rs`（`Middleware` trait 定義・同旨の契約を doc comment に記載）
- `docs/acceptance/nfr7-middleware-async-io.md`（NFR-7 受け入れ検証レポート、#263）

### 適用範囲と検証責務

標準提供ミドルウェア有効化時のコア RPS 劣化は 5% 以内を維持する（NFR-7 受け入れ
基準）。レビュー時の本規約準拠確認は `reviewer` / `plugin-builder`、性能検証は
`bench-builder` が担う（[delegation-impl.md](.claude/rules/delegation-impl.md)）。

### 可用性・可観測性に関する注記

- **リソース枯渇（DoS）耐性**: 全リクエストのホットパスに載るミドルウェアが同期
  I/O を行うと、スロー I/O（ディスク詰まり・パイプブロック等）発生時にワーカー
  スレッドが枯渇し、サービス全体が応答不能に陥りうる。本規約はこのリスクを構造的
  に排除する（[security.md](.claude/rules/security.md) の「リソース枯渇（DoS）」
  観点）。
- **ログ欠落の許容可否**: 非同期・バッファ済みログはバックプレッシャ時にイベント
  欠落（drop）が起こりうる。セキュリティ監査イベント等、欠落を許容できないログの
  扱いは、標準ロギング／トレーシング実装（REQ-10・`plugin-tracing` 系タスク）側
  の設計事項として別途定める。本規約はこの論点を暗黙に決定しない。

## 規約: ハンドラ契約は async・3 拡張点は同期のまま（イシュー #315）

`docs/design/async-handler.md`（採用案 (c)）対応。**既定ハンドラ**（`crates/core/
src/server.rs` の `Handler` trait、`fandhe_backend_routes::Router` の `route_async` /
`route_param_async`）と、上記「ミドルウェア非同期 I/O 必須化」規約が対象とする
**3 拡張点**（`Middleware` / `UpgradeHandler` / `RequestGate`）とでは、非同期化の
扱いが非対称である点に注意する:

- **`Handler::handle`**: `fandhe_backend_routes::HandlerFuture`（`Pin<Box<dyn
  Future<Output = Response> + Send>>`、`async-trait` 等の外部依存なし・std のみで
  型消去）を返す **async 契約**。実装者はハンドラ本体で `sqlx` 等の非同期 I/O を
  直接 `.await` できる。既存の同期登録 API（`Router::route` / `route_param`）は
  内部で `std::future::ready` に適合させ後方互換を維持する
- **3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）**: 意図的に**同期の
  まま据え置く**（`dyn` 互換性・呼び出しコストの単純さを優先、`docs/design/
  async-handler.md` 2 節）。I/O が必要な場合は上記「ミドルウェア非同期 I/O
  必須化」規約（非同期チャネル送信・別タスクへの委譲）に従う
- **`Interceptor`（`crates/core/src/interceptor.rs`、イシュー #420）**: 3 拡張点に
  次いで追加した、ユーザー向けインターセプト・レスポンス改変拡張点。同じ理由
  （`dyn` 互換性）で同期のまま据え置く。同期ブロッキング I/O 禁止契約も
  `Middleware` と同一（`crate::interceptor` モジュール doc を参照）

この非対称性は意図的な設計判断であり、3 拡張点を「ハンドラに揃えて async 化する」
提案は本規約と衝突する。3 拡張点の async 化を検討する場合は
`docs/design/async-handler.md` の再評価条件（8 節）に従い設計文書を更新してから
着手すること。

`RequestGate::check` はイシュー #486 で `ctx: &GateContext` 引数を追加した
（`crates/core/src/extension.rs`）。`GateContext::peer_addr()` は accept した
ソケットの実 peer address を運ぶが、この変更は上記の同期契約そのものには
影響しない（`GateContext` は `Copy` 型の単純な値渡しであり、`async fn` 化や
I/O を一切伴わない）。詳細は `docs/design/gate-peer-addr.md` を参照。

## AI エージェント向け変更ガイド

TASK-11.3（#35、`docs/spec/05-tasks.md` Phase 3 / MS-3、`docs/spec/04-requirements.md`
REQ-11）対応。AI がこのリポジトリを安全に改修するための、モジュール境界・変更手順・
完了判定・アサーション規約・安全性方針・エスカレーション基準を機械可読な形でまとめる。
運用・委譲の詳細は `CLAUDE.md`、Rust コーディング規約の詳細は
[coding-rust.md](.claude/rules/coding-rust.md) を正とし、本節は重複させず要点と
一次情報源への参照のみを記載する。

### モジュール境界

workspace 内クレート間の依存方向は次の一方向を維持する（`crates/core/src/lib.rs`
モジュール doc・`scripts/dep-direction-check.sh` と同一の宣言）。

```text
server → routes → http::*
```

- `crates/core` はこの依存グラフの末端に位置し、`crates/plugin-*` の固有シンボルには
  一切依存しない（pay-for-what-you-use、
  [pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md)）。
  プラグインは feature 経由でコアの拡張点を実装する側であり、コアからプラグインへの
  依存は発生しない設計とする
- **既知の例外（是正中）**: `crates/core` → `fandhe-backend-plugin-webrtc-proxy`
  （`webrtc-proxy` feature 経由）は現状の依存グラフで許可リスト化された例外であり、
  是正は Issue #136（`fix(core): crates/core が fandhe_backend_plugin_webrtc_proxy に直接依存し
  依存方向一方向性に違反`）で追跡する。新規変更でこの例外を拡大しない
- 機械検証: `bash scripts/dep-direction-check.sh`（`cargo metadata` の依存エッジを
  許可リストと照合、循環依存検出、コアへのプラグイン固有シンボル混入を grep 検出）

crates 一覧と責務（`crates/` 直下、`ls` で最新を確認できる）:

| クレート | 責務 |
|---------|------|
| `core` | HTTP/1.1 パーサ・keep-alive・3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）+ ユーザー向けインターセプト・レスポンス改変拡張点（`Interceptor`、イシュー #420、feature ゲート不要）を持つ最小コア |
| `http` | sans-IO な HTTP プリミティブ（`fandhe-backend-http`）。workspace 内で最下層 |
| `routes` | ルーティング（`fandhe-backend-routes`）。`server → routes → http::*` の中間層 |
| `plugin-websocket` | WebSocket（RFC 6455 ハンドシェイク・`UpgradeHandler` 拡張点） |
| `plugin-graphql` | GraphQL プラグイン境界 |
| `plugin-openapi` | OpenAPI ドキュメント生成 |
| `plugin-webrtc` | in-process WebRTC（`webrtc-rs` 直接依存） |
| `plugin-webrtc-proxy` | WebRTC シグナリングプロキシ（別プロセス切り出し型） |
| `plugin-cors` | CORS（プリフライトは `Router::options_fallback` 経由・実リクエストへのヘッダ付与は新設のレスポンス後処理型シーム `crate::plugin::finalize_response` 経由。3 拡張点いずれにも載らない 5 番目のプラグイン境界パターン、`docs/design/plugin-boundary.md` 5.9 節） |
| `plugin-compression` | レスポンス圧縮（gzip）。`plugin-cors` と同じ `finalize_response` シームの第 2 インスタンス（CORS の後に逐次適用、イシュー #321）。ステータス・`Content-Type`・body サイズ・`Accept-Encoding` の判定基準を満たす場合のみフェイルセーフに圧縮。外部依存は `flate2` のみ、`docs/design/plugin-boundary.md` 5.10 節） |
| `plugin-static` | 静的ファイル配信（パスインターセプト型 `try_intercept` + `spawn_blocking` 変種、`Server::static_files(config)` 登録時のみ応答。パストラバーサル対策は二層防御（字句検証 + canonicalize 後の root 配下検証）、`docs/design/plugin-boundary.md` 5.11 節、イシュー #318） |
| `plugin-hub-wiring` | hub 共通配線（`RequestGate` 上の `TenantGate`。JWT (RS256 + JWKS) 検証 → `org_id` 抽出 → フェイルクローズ。依存逆転型プラグイン、`docs/design/plugin-boundary.md` 5.6 節）。越境アクセス監査ログ（`audit` モジュール、`cross_tenant_attempt` カテゴリ。「正当な 404」と「越境 404」を外部応答同一のまま監査ログのみで区別、TASK-9.6・#89） |
| `axum-ref` | 性能比較用参照実装 |

### レスポンス側 chunked ストリーミング送信（`Handler::handle_streaming`、イシュー #319）

`Handler`（`crates/core/src/server.rs`）は「3 拡張点」（`Middleware` /
`UpgradeHandler` / `RequestGate`）の対象ではなく、既定ハンドラの差し込み口という
既存の位置づけを持つ。`Handler::handle_streaming` はこの `Handler` の opt-in
既定メソッドとして追加した拡張点であり、`Some(StreamingResponse)` を返す実装のみが
`Transfer-Encoding: chunked` の逐次送信経路（`crates/core/src/streaming.rs` +
`fandhe_backend_http::chunked::{encode_chunk, encode_terminator}` +
`fandhe_backend_http::response::Response::serialize_chunked_head` /
`serialize_streaming_head_http10`）を使う。既定実装は常に `None` を返すため、
既存の `Handler::handle` のみの実装は無変更でコンパイル・従来どおりの
`Content-Length` 応答を維持する（後方互換）。

- producer 側の典型パターンは `StreamingResponse::channel` で得た `BodyWriter` を
  `tokio::spawn` した非同期タスクへ move し、`send` / `finish` を呼ぶこと
  （`Handler::handle_streaming` の doc test を参照）。`BodyWriter::send` は
  bounded mpsc の容量超過時に `.await` で停止する（バックプレッシャ、
  サーバ側バッファを有界に保つ）
- `finish` を呼ばずに producer が drop された場合は打ち切りとして扱われ、
  受信側（`write_streaming_response`）は終端チャンクを送らず接続をクローズする
  （応答完全性の fail-closed。RFC 9112 の length 整合性維持）
- producer からの次チャンク待ちには `DEFAULT_WRITE_TIMEOUT`（30 秒、
  `crates/core/src/server.rs`）が適用され、`send` / `finish` の呼び出し間隔が
  これを超えると正常な producer でも接続が強制クローズされる。SSE
  （`text/event-stream`）のハートビート間隔や long-poll のようにアイドル区間が
  長い producer は、本値未満の間隔で `BodyWriter::send(Vec::new())`（空チャンクは
  無出力）を呼んでワイヤへ余計なバイトを出さずに待ち時間をリセットする
  （`Handler::handle_streaming` の doc を参照）
- HTTP/1.0 リクエストへは chunked framing を使わず、`Connection: close` +
  EOF 終端で応答する（`Response::serialize_streaming_head_http10` の doc を参照）
- `crate::plugin::finalize_response`（CORS 等のレスポンス後処理型シーム）は
  `Response` 型を前提とするため `StreamingResponse` には適用しない
  （イシュー #319 計画時点のスコープ外を維持）
- ユーザー向け `Interceptor::map_response`（イシュー #420）はイシュー #434 で
  ストリーミング応答にも適用対象へ拡張した。`write_streaming_response` が
  ヘッド確定時（HTTP/1.0・HTTP/1.1 共通、1 回のみ）に `server.interceptors` を
  登録順に適用し、ステータス・`Content-Type`・追加ヘッダのみを反映する。
  mapped `Response` の body は producer のチャンクと排他（body を経由しない
  chunked framing）のため反映されず破棄する契約（`crate::interceptor` モジュール
  doc の「ストリーミング応答への適用」節・`docs/design/
  interceptor-extension-point.md` を参照）

### 変更手順

拡張点変更は、まず 3 種 trait（`Middleware` / `UpgradeHandler` / `RequestGate`、
`crates/core/src/extension.rs`）のいずれかに載るか判定することから入る
（[coding-rust.md](.claude/rules/coding-rust.md)）。feature の新規追加・変更は
[pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md) と
[feature-modification-flow.md](docs/design/feature-modification-flow.md) に従う。
**3 種 trait のいずれにも載らない場合**（例: CORS のようにレスポンス内容自体を
書き換える必要があり `Middleware::on_response` の観測専用契約に収まらないケース）
は、`crates/core/src/plugin.rs` に `try_intercept` / `try_handle_upgrade` と
同型の固定シグネチャ・cfg-gated な新シームを追加できるか検討する
（`docs/design/plugin-boundary.md` 5.9 節「レスポンス後処理型パターン」を参照。
安易な新パターン追加は避け、既存 3 種で表現できないことを確認してから導入する）。

- 上記はプラグイン（feature 着脱）側の受け皿。**利用者コード（アプリ側）向けの
  リダイレクト・レスポンス改変**が既存 3 種に載らない場合は、`crates/core/src/
  interceptor.rs` の `Interceptor` trait（イシュー #420、feature ゲート不要の
  追加拡張点）を検討する。`crate::interceptor` モジュール doc に評価順序・
  fail-closed 除外・「3 拡張点で表現できない」根拠を明記済み。安易な追加拡張点の
  増設は避け、既存の `Interceptor`（intercept / map_response の 2 フック）で
  表現できないか先に確認すること

#### 新規エンドポイント追加手順

1. `fandhe_backend_routes::Router::route()`（完全一致）または `fandhe_backend_routes::Router::route_param()`
   （`{name}` パスパラメータ、TASK-176・#176）へのルート登録。未マッチ（静的・
   パラメータいずれにも一致しない）リクエストの共通処理が必要な場合は
   `Router::fallback()` / `Router::fallback_with()`（イシュー #316）を使う。405
   （メソッド不一致）も fallback へ流すかは `FallbackPolicy` で個別選択でき、既定
   （`FallbackPolicy::NotFoundOnly`）は 404 のみを委譲し 405 + `Allow` を維持する
   安全側
2. ハンドラ実装（対象クレートは「モジュール境界」節の crates 一覧・
   [delegation-impl.md](.claude/rules/delegation-impl.md) のパスベース委譲に従い判断する）
3. doc コメント + doc test（`# Examples`）を付与する
   （[code-comment-style.md](.claude/rules/code-comment-style.md)）
4. 「アサーション網羅性」節に従う網羅的アサーション付きテストを追加する
5. **本 AGENTS.md の更新をサブタスクとして必ず含める**。エンドポイント・拡張点追加時に
   本書が古びていないかを確認し、必要な追随を行う（本節が確立する運用。
   [feature-modification-flow.md](docs/design/feature-modification-flow.md) 8 節が
   参照する追随先）

### 変更完了の判定基準

変更ごとに以下をすべて満たすことを確認する。コマンドの正確な集合・CI ジョブ構成は
[ci-completion-criteria.md](docs/design/ci-completion-criteria.md) を正とし、本節では
二重管理しない（ジョブ追加・改名時は同書側が更新される）。

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`（`.config/nextest.toml` の
  `slow-timeout` 設定によりテスト単位タイムアウト付き。`cargo nextest run` でも可）
  + `cargo test --doc`
- `cargo doc`（`RUSTDOCFLAGS="-D warnings"`）
- CI 集約ゲート `ci-complete` が green
- ドキュメント追随が完了していること
  （[feature-modification-flow.md](docs/design/feature-modification-flow.md) 8 節の
  変更種別 → 追随ドキュメントのマッピングに従う）
- 受け入れ基準を充足していること（人間判断によるレビューゲート。
  [feature-modification-flow.md](docs/design/feature-modification-flow.md) 9 節）

上記のいずれかが未充足のまま「変更完了」とみなさない（fail-closed）。

### アサーション網羅性

PoC-9（`docs/spec/03-poc/ai-first-maintainability/README.md`）では、HTTP レスポンスの
ボディ内容のみを検証しステータス行・`Content-Type` を検証しないテストがバグを見逃す
事例が確認された。この教訓に基づき、HTTP レスポンスを検証するテストは次を**すべて**
検証する:

- ステータス行（ステータスコード）
- ヘッダ（少なくとも `Content-Type` / `Content-Length`）
- ボディ

ボディ内容の一致のみで「テストが通った」と判断しない。新規エンドポイント追加・既存
エンドポイント変更のテストはこの規約に従う（「変更手順」節・
[feature-modification.md](.claude/rules/feature-modification.md) の「実装にはテスト
追加を伴う」と併せて適用する）。

### 安全性方針

- `unsafe` は最小限に留め、使う場合は `// SAFETY:` コメントで不変条件と安全性の根拠を
  必ず書く（[coding-rust.md](.claude/rules/coding-rust.md)）
- workspace lints は 2 層防御を敷く（詳細は
  [unsafe-deny-lints.md](docs/design/unsafe-deny-lints.md)）: 第 1 層は `forbid`
  （`#[allow]` による抑制自体を禁止）、第 2 層は `deny`（正当理由があれば局所
  `#[allow]` + レビューで例外化可能）
- OWASP Top 10 観点（入力検証・認証認可・インジェクション・リソース枯渇・
  暗号/シークレット管理・可観測性）は
  [security.md](.claude/rules/security.md) を正とする
- pay-for-what-you-use（feature 無効時の依存・コード・`unsafe`・バイナリ増ゼロ）は
  [pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md) を正とする
- WebRTC（`plugin-webrtc` / `plugin-webrtc-proxy`）の安全性方針の詳細（プロセス分離等）は
  [webrtc-process-isolation.md](docs/design/webrtc-process-isolation.md)、および攻撃表面と
  「使う/使わない」サービスの安全性方針の詳細は本書「規約: WebRTC の攻撃表面と
  「使う/使わない」サービスの安全性方針」節（TASK-8.4、#29）を参照

### エスカレーション基準

対応可否の自律判断は「可 / 条件付き可 / 不可・要エスカレーション / 不可（明確な拒否）」
の 4 値で判定する（詳細は
[feasibility-guardrail.md](docs/design/feasibility-guardrail.md)、運用規約は
[feasibility-guardrail.md（rules）](.claude/rules/feasibility-guardrail.md)）。

判定の 3 軸（いずれか 1 つでも不充足なら「可」と判定しない、fail-closed）:

1. 実施可能か（検証可能な受け入れ基準に落ちるか）
2. 安全か（[security.md](.claude/rules/security.md)・OWASP Top 10 と整合するか）
3. 影響範囲が許容内か（クレート・feature・利用者への影響が特定・限定できるか）

不可判定 4 カテゴリ（代表例、網羅列挙ではない）:

| カテゴリ | 判定条件 | 判定区分 |
|---------|---------|---------|
| 曖昧要求 | 受け入れ基準がなく曖昧語のみで完遂を測定不能 | 不可・要エスカレーション |
| 未定義依存 | 依存・接続情報・方式が未定義 | 不可・要エスカレーション |
| 安全性方針との衝突 | 既存安全性方針（DoS 耐性・境界検証等）を後退させる | 不可・要エスカレーション |
| 明確な脆弱性を招く要求 | OWASP Top 10 に直結する脆弱性（RCE・インジェクション等）が明白 | 不可（明確な拒否） |

判断不能な場合は安全側に倒し、実装を進めずエスカレーションする（fail-closed 原則。
判定記録の形式検証は `bash scripts/feasibility-check.sh --input <record>` で行う）。

上記の各機構を「どの基盤で・どの頻度で・どの範囲に・誰の責任で」回すかという運用面の
定義は [ai-maintenance-operations.md](docs/design/ai-maintenance-operations.md) を参照
（Issue #93、REQ-11・REQ-12。新規の機構・規約は追加せず、既存資産の統合定義）。

## 規約: WebRTC の攻撃表面と「使う/使わない」サービスの安全性方針

TASK-8.4（`docs/spec/05-tasks.md`、Phase 2 / MS-2、#29）対応。`docs/spec/04-requirements.md`
REQ-8（WebRTC）受け入れ基準・NFR-6（拡張の非侵襲性）を満たす運用規約文書。

### 背景: 2 クレートの対照

fandhe-backend は WebRTC を 2 つの独立クレートで提供し、**クレート境界で完全に
分離**する（相互 path 依存なし。`docs/dep-impact/records.md` の TASK-8.4 エントリで
機械検証済み）。

| クレート | feature | 依存モデル | 攻撃表面 |
|---------|---------|-----------|---------|
| `crates/plugin-webrtc` | `webrtc` | `webrtc-rs`（0.17.1 系）を**プロセス内**に直接組み込む（in-process） | 大（`webrtc` feature 単体で `cargo tree -p fandhe-backend-core --features webrtc` に webrtc 系依存 23 件、release バイナリサイズ約 11 倍、TASK-8.4 実測。`docs/dep-impact/records.md`） |
| `crates/plugin-webrtc-proxy` | `webrtc-proxy` | `webrtc-rs` に**一切依存しない**軽量シグナリングプロキシ。重い WebRTC サービスは別プロセスへ切り出す | 小（`webrtc-rs` 依存が本体プロセスに一切現れない） |

`crates/core/src/plugin.rs` の `try_intercept` は両 feature が同時に有効な場合
（`--all-features` CI 構成）、`webrtc-proxy` を先に評価する（REQ-8 の MVP 推奨方式を
優先する運用判断。両方を `Server` に登録した場合は `webrtc-proxy` が優先され、
`webrtc` 側の設定は評価されない）。

### 安全性方針

- **WebRTC を使わないサービス**: `webrtc`・`webrtc-proxy` のどちらの feature も有効化
  しない。依存・`unsafe`・バイナリ増をゼロに保つ（pay-for-what-you-use、
  [pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md)）。`cargo tree -p
  fandhe-backend-core` にいずれの feature 無効時も webrtc 系依存が一切現れないこと
  を維持する。
- **WebRTC を使うサービス**: 可能な限り `plugin-webrtc-proxy`（`webrtc-proxy` feature）
  による**別プロセス切り出し**を第一選択とする。`webrtc-rs` の巨大な依存グラフ・
  パーサ群をコアプロセスから隔離し、脆弱性発生時の影響範囲・監査対象を限定できる。
- **in-process `plugin-webrtc`（`webrtc` feature）を選ぶ場合**: 別プロセス切り出しの
  運用コスト（プロセス間通信・デプロイ構成の複雑化）が許容できない場合に限り検討する。
  有効化すると `webrtc-rs` の巨大な依存グラフ・パーサ群がコアプロセスに直接組み込まれ、
  ICE 接続性チェックはクライアント SDP 由来のアドレスへ UDP 送信を発生させ得る（WebRTC
  の構造上不可避）。STUN/TURN は既定で設定しない（`RTCConfiguration::default()`）。
  Offer サイズ上限・接続数上限（503 フェイルクローズ）・シグナリングタイムアウト
  （504）は維持されている（`crates/plugin-webrtc/tests/attack_surface.rs` で受け入れ
  観点から再アサート済み）が、依存グラフそのものの大きさは変わらない。

### NFR-6（無関係パスへの性能影響）に関する留意事項

NFR-6 は「パス一致時のみ介入する拡張点は、無関係なパスへの RPS・レイテンシ影響が
誤差範囲内（100.3〜100.8%相当）である」ことを求める。この帯は GraphQL（PoC-3、依存
インパクトが軽微なパスインターセプト型）由来の実測に基づく。TASK-8.4 の empirical
計測（`benches/webrtc-nfr6-bench.sh`、`benches/reports/task-8.4-webrtc-nfr6.md`）では、
`webrtc` feature 有効時の無関係パス（`GET /`）RPS が baseline 比おおむね 94〜95%、
p95 レイテンシがおおむね 106〜108% となり、狭義の 100.3〜100.8% 帯には収まらなかった。
`try_intercept` 自体は対象外パスに対して 1 回のパス比較のみでフォールスルーするため
（`crates/core/src/plugin.rs`）、この差は拡張点の呼び出しコストではなく、バイナリ
サイズが約 11 倍に達すること（icache/TLB 圧迫等）に起因すると考えられる。**WebRTC を
使うサービスがこの性能影響を避けたい場合も、`plugin-webrtc-proxy` による別プロセス
切り出しが有効な緩和策となる**（プロキシプロセスとコアプロセスが分離するため、コア
プロセスのバイナリサイズ・性能特性は影響を受けない）。

### 出典リンク

- `docs/design/webrtc-process-isolation.md`（別プロセス切り出しの設計判断）
- `docs/design/webrtc-rs-version-strategy.md`（`webrtc-rs` バージョン戦略、TASK-8.3）
- `docs/acceptance/req8-webrtc-attack-surface.md`（TASK-8.4 攻撃表面評価・受け入れ判定）
- `docs/dep-impact/records.md`（依存インパクト計測記録）
- `docs/spec/04-requirements.md`（REQ-8・NFR-6）
- `docs/spec/05-tasks.md`（TASK-8.1〜TASK-8.4）

### 適用範囲と検証責務

`webrtc`/`webrtc-proxy` 両 feature の依存完全除外・クレート境界分離の機械検証は
`scripts/accept/webrtc-accept.sh`、NFR-6 の empirical 計測は `bench-builder` が担う
（[delegation-impl.md](.claude/rules/delegation-impl.md)）。

## 規約: WebSocket セッションのアイドルタイムアウト既定値と DoS 耐性

Issue #175 対応。`crates/plugin-websocket` のセッション処理
（`crates/plugin-websocket/src/session.rs` の `run_session`。Issue #179 で
`run_echo_session` から改名し、Text/Binary メッセージをユーザー定義
`WsMessageHandler`（既定 `EchoHandler`）へ委譲する建て付けへ拡張した）は、
クライアントからのフレーム受信が一定時間ないアイドル接続を無期限に保持しない
（リソース枯渇 DoS 対策、[security.md](.claude/rules/security.md)）。

### 既定値の根拠

`WebSocketConfig::idle_timeout` の既定値は `Some(60 秒)`（fail-safe: 既定で有効）。
一般的なリバースプロキシの読み取りタイムアウト既定（例: nginx
`proxy_read_timeout` 60s）と同水準に揃えており、正当なクライアントは通常の通信
または Ping で容易に接続を維持できる。既存の負荷試験ハーネス
（`benches/bench-ws-load.sh`、ハートビート既定 2 秒間隔）はこの既定値に抵触しない
（実装時点で確認済み）。

### 無効化は明示操作のみ

アイドルタイムアウトの無効化は `WebSocketConfig::without_idle_timeout()` の明示
呼び出しでのみ行える。暗黙の設定漏れで保護が外れることはない
（フェイルセーフ、[pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md)
と同様の「既定で安全側」の考え方）。

### 発火時の切断シーケンスと二次 DoS 対策

タイムアウト発火時はサーバ側から Close フレーム（1000 Normal Closure）を送出し、
正常な Close ハンドシェイクで切断する（プロトコル違反ではないため `WsError` に
新規 variant を追加せず `Ok(())` を返す）。Close 応答を返さない（無視する）
クライアントが「クローズ送出済みだが応答待ち」の状態で接続を無期限に保持し続ける
経路を防ぐため、クライアントの Close 応答（または EOF）のドレインには固定の
猶予（`WebSocketConfig::close_grace`、既定 10 秒・ビルダー
`with_close_grace` で設定可能、イシュー #500）を設け、超過時も `Ok(())` で
確実に接続を終端する（fail-closed）。

### 検証

発火・非発火（通信継続で維持）・Ping のみでの維持・無効化・Close 無視クライアント
への `close_grace` 適用は `crates/plugin-websocket/tests/idle_timeout.rs` の統合
テストで検証する。

## レビュー基準（Codex PR 自動レビュー）

Codex による PR 自動レビュー（`.github/workflows/codex-review.yml`。Fandhe-AI/actions の
reusable workflow（実装本体は `Fandhe-AI/actions/.github/workflows/codex-review.yml`）を
SHA 固定で呼び出す薄い wrapper、イシュー #529）と人間レビューが共通で用いる基準。
Codex は本ファイルを自動読込する。Codex code review は既定で P0/P1 のみを
表示・報告対象とするため、本プロジェクトとして必ず検出したい項目は下記で優先度を明示的に
格上げして定義する。ここに列挙のない一般的な品質問題は Codex 側の既定の重要度判断に従う。

### 優先度の定義

| 優先度 | 意味 | CI ゲート |
|--------|------|-----------|
| P0 | マージ不可。脆弱性・データ破壊・契約破壊に直結 | ジョブ失敗 |
| P1 | 修正必須。設計原則・拡張点契約・運用規約への違反 | ジョブ失敗 |
| P2 | 修正推奨。可読性・保守性・テスト網羅の改善 | 通過（コメントのみ） |
| P3 | 任意。好みの範囲の提案 | 通過（コメントのみ） |

### 命名規則

- クレート名は `fandhe-backend-<name>` / `fandhe-backend-plugin-<name>`、feature 名は
  ケバブケース。ディレクトリ名（`crates/<name>`）はプレフィックスなし
- Rust 識別子は Rust API Guidelines と周辺コードの慣例に従う（モジュール・関数・feature
  内部識別子は snake_case、型・trait は UpperCamelCase、定数は SCREAMING_SNAKE_CASE）
- コミット・PR タイトルは Conventional Commits（`.claude/rules/conventional-commits.md`。
  type/scope は英語規約、description は日本語可）
- 逸脱は P2。ただし公開 API（crates.io 公開 13 クレートの公開シンボル）の命名逸脱は
  破壊的変更なしに直せなくなるため P1

### 禁止事項（明示的に P0/P1 へ格上げ）

- **feature ゲート漏れ**（feature 無効時に依存・コード・`unsafe`・バイナリ増が残る
  pay-for-what-you-use 違反、`.claude/rules/pay-for-what-you-use.md`）: **P1**
- **`// SAFETY:` コメントのない `unsafe`**、および不変条件の根拠が不十分な `unsafe`: **P0**
- **ライブラリコード（`crates/**`）での `.unwrap()` / `.expect()`**（テスト・examples を
  除く。panic をライブラリ境界の外へ漏らす経路全般を含む）: **P1**
- **`Middleware` フック内の同期ブロッキング I/O**（本ファイル「規約: ミドルウェア非同期
  I/O 必須化」違反）、および **ロック保持中の `.await`**: **P1**
- **CI ワークフローの規約違反**（`runs-on: self-hosted` 以外の指定・`timeout-minutes`
  欠落・`pull_request_target` 等の secrets 露出トリガー追加、`.claude/rules/ci.md`）: **P1**
- **公開 API の doc comment / doc test 欠落**（AI ファースト保守性、
  `.claude/rules/code-comment-style.md`）: **P2**（セキュリティ上の契約・fail-closed
  条件が未記載の場合は **P1**）

### セキュリティ観点（明示的に P0 へ格上げ、`.claude/rules/security.md`）

- 入力検証の欠落・後退（HTTP パーサ・ルーティング・プラグイン入口での境界・サイズ上限・
  エンコーディング検証。既存の DoS 上限やタイムアウトを撤廃・緩和する差分を含む）
- シークレット（API キー・トークン・パスワード）・PII のコード・ログ・CI 設定への混入
- インジェクション経路（ヘッダ・ログ・GraphQL・シェル実行）
- パストラバーサル・シンボリックリンク脱出等、OWASP Top 10 に直結する欠陥
- fail-closed で設計された既存分岐の fail-open 化

### 運用

- gate の判定は `.github/codex/review-schema.json` に従う構造化出力を `jq` で 2 段判定
  する: (1) `review_completed == true` の確認（fail-closed。レビュー手順（diff 取得・
  `AGENTS.md` 読み取り）自体を完遂できなかった場合は findings の有無に関わらずジョブを
  失敗させる。イシュー #524／親 #523）→ (2) P0/P1 でジョブ失敗。基準の追加・格上げは
  本節の編集のみで反映される
- レビュー制御用ファイル（prompt: `.github/codex/prompts/review.md`・schema:
  `.github/codex/review-schema.json`・本節を含む `AGENTS.md`「レビュー基準」節）は
  PR の checkout（merge ref）から直接消費せず、PR の base コミット（信頼済み参照）
  から取得した内容を使う。prompt/schema は呼び出し先の reusable workflow
  （`Fandhe-AI/actions/.github/workflows/codex-review.yml` の `Extract review control
  files from base branch` ステップ）が `git show` で $RUNNER_TEMP へ抽出し、
  `AGENTS.md`「レビュー基準」節は prompt 自体が `git show HEAD^1:AGENTS.md` で明示的に
  ベースブランチ側を読む（Codex CLI の cwd 自動読込に頼ると checkout 側＝PR 自身の
  改変後の内容を読んでしまうため使わない）。この prompt 側の指示だけでは、CLI 自体が
  cwd（checkout ルート、`.git` を含むため project root と判定される）配下の
  AGENTS.md / AGENTS.override.md を起動時に自動でコンテキストへ注入する既定動作は
  塞げない（イシュー #524 の PR #526 に対する Codex 自身のレビューで P0 指摘）ため、
  reusable workflow 側は 2 重に対処する: (a) `Run Codex review` ステップで
  `--config project_doc_max_bytes=0` を渡しこの自動読込機構自体を無効化する、(b)
  `Extract review control files from base branch` ステップで checkout 側の root
  `AGENTS.md` を base 版へ上書き・`AGENTS.override.md` を削除する（working tree の
  書き換えは `git diff HEAD^1 HEAD` 等 git オブジェクト参照ベースの prompt の手順には
  影響しない）。PR 差分が
  これらのファイルを改変しても、その改変は当の PR 自身のレビュー実行には反映されない
  （base ブランチへのマージ後に限り以降の PR へ反映される）。レビュー対象の diff から
  直接 prompt/schema/基準を読み込む構成だと、diff がレビュー指示自体を弱める方向へ
  改変してもその改変済み指示がそのまま自分自身のレビューに使われる自己参照構成に
  なるため（イシュー #524、PR #526 の Codex レビュー P0/P1 指摘を受けて導入）
- 上記の base 参照化により自己参照は成立しなくなったため、レビュー制御用ファイル
  （prompt/schema/本節）自体を変更する差分は、パスが一致するというだけの理由で
  自動的に「レビュー指示の改変」（プロンプトインジェクション規則、P0）扱いにしない。
  内容を読み、P0/P1 の禁止事項・`review_completed` 判定基準を弱める変更かどうかで
  判定する（`.github/codex/prompts/review.md` に判定基準を明記。PR #526 に対する
  Codex 自身の誤検知——base 参照化と同一コミットにもかかわらず、制御用ファイルへの
  差分というだけでパス一致から一律 P0 と判定した——を受けて追加）
- gate の失敗が実際にマージを止めるかは branch protection の required status check 設定に
  依存する。現状の required check は `ci.yml` の `ci-complete` のみで本ワークフローは
  含まれないため、gate は advisory（人間レビューの補助）であり機械的なマージ阻止では
  ない。上記の base 参照化により PR 差分による本節・レビュー指示文の即時無効化は防げるが、
  最終判断は引き続き人間レビューが担う。required check 化する場合は `CODEX_HOME_DIR`
  未設定時のジョブ skip との両立を別途設計する
- rustfmt / clippy / テスト成否は既存 CI（`ci.yml`）が機械判定するため、本レビューの
  対象外とする
