# Changelog

公開対象 13 クレート（`fandhe-backend-http` / `fandhe-backend-routes` / `fandhe-backend-core` /
`fandhe-backend-plugin-*` 10 種）は lockstep（同一バージョン一斉更新）で運用する
（詳細は [`docs/design/crates-io-release.md`](docs/design/crates-io-release.md) 7 節）。
恒久非公開クレート（`axum-ref` / `ws-load-client` / `docs-site`）はこの一覧に含めない。

## [Unreleased]

### BREAKING CHANGES

1. **`fandhe-backend-core`**: `RequestGate::check` のシグネチャへ `ctx: &GateContext`
   引数を追加しました（イシュー
   [#486](https://github.com/Fandhe-AI/fandhe-backend/issues/486)）。
   `GateContext::peer_addr()` で accept したソケットの実 peer address を
   `RequestGate` 実装から参照できるようになります（`X-Forwarded-For` 等の
   偽装可能なヘッダとは別物で、実 TCP 接続の peer を運びます）。
   - **移行手順**: 既存の `fn check(&self, head: &RequestHead) -> GateOutcome` を
     `fn check(&self, head: &RequestHead, _ctx: &GateContext) -> GateOutcome`
     へ書き換えるだけで従来どおり動作します。peer address に基づく判定
     （CIDR 照合等）を追加したい実装は `ctx.peer_addr()` を参照してください。
     `tokio::io::duplex` 等の非ソケット経路（`handle_connection`）では常に
     `None` になるため、peer addr を判定に必須とする実装は `None` の場合に
     必ず `GateOutcome::Reject` を返してください（フェイルクローズ、
     `GateContext` の doc を参照）。実 peer address を注入したい呼び出し元は
     新設の `handle_connection_with_peer_addr` を使ってください。

### Added

- `fandhe-backend-core`: 稼働中の `BoundServer` へ listener 差し替え（rebind）を
  指示できる `BoundServer::rebind_handle` / `RebindHandle::rebind` を追加
  （イシュー [#485](https://github.com/Fandhe-AI/fandhe-backend/issues/485)）。
  `rebind()` は呼び出し側で新規 `TcpListener` を bind してから accept ループへ
  差し替えを依頼する構造のため、bind 失敗時は旧 listener・in-flight 接続に
  一切影響しない（fail-closed）。差し替え直前までの「旧世代」接続は
  `Server::shutdown_grace_period` を上限に完走を待ち、超過分は強制クローズする
  （`run_until` 自体はブロックしない、詳細は
  `docs/design/rebind.md` を参照）。`rebind_handle` を一度も呼ばなければ
  `mpsc` チャネルは生成されず、既存の `run()` / `run_until` の挙動は不変
  （pay-for-what-you-use）
- `fandhe-backend-plugin-compression`: `CompressionConfigBuilder::blocking_threshold`
  （既定 64 KiB）を追加し、`apply_compression` を `plan_compression` /
  `compress_body` / `attach_compressed` の 3 関数へ分割公開（イシュー
  [#468](https://github.com/Fandhe-AI/fandhe-backend/issues/468)）
- `fandhe-backend-core`: `UpgradeHandler` 経由で WebSocket へ委譲された接続へ、
  最終 graceful shutdown（#313）・rebind 世代 drain（#485/#488）双方の世代
  キャンセルシグナルを配線しました（イシュー
  [#491](https://github.com/Fandhe-AI/fandhe-backend/issues/491)、設計は
  [#490](https://github.com/Fandhe-AI/fandhe-backend/issues/490)・
  `docs/design/ws-cancellation-propagation.md`）。世代ごとの
  `tokio::sync::watch` チャネルを drain 開始時（最終 shutdown は
  `shutdown_flag` を立てた直後、rebind は背景 drain タスクの冒頭）に 1 回
  発火し、委譲済み WS 専用タスクへ伝播します。新規依存・tokio feature 追加は
  ゼロ（コアの既存 `sync` feature 内で完結、`plugin-websocket` への委譲境界は
  キャンセル `Future` として渡し `sync` feature を要求しない）。
  **現時点の挙動は中間実装**であり、キャンセル発火時は
  `fandhe_backend_plugin_websocket::handle_upgrade` の `Future` を drop する
  ハードクローズ（TCP を即座に切断）を行います。WS プロトコルレベルの正常な
  Close フレーム送信への置き換えはイシュー
  [#492](https://github.com/Fandhe-AI/fandhe-backend/issues/492) が担い、
  その際 `handle_upgrade` のシグネチャへキャンセル `Future` 引数を追加する
  breaking change が入る予定です（本エントリはその変更が実際に入る #492 側で
  更新）。

### Changed

- `fandhe-backend-core`: `finalize_response`（非公開シーム）が body 長
  `blocking_threshold` 以上の gzip 圧縮を `tokio::task::spawn_blocking` へ
  切り離すようになりました。接続タスクの tokio ワーカスレッドが巨大応答の
  圧縮で長時間占有され他タスクのテールレイテンシへ波及する問題への対処
  （しきい値未満は従来どおりインライン実行、応答は不変。実測根拠は
  `benches/reports/issue468-compression-blocking.md`・`docs/design/
  plugin-boundary.md` 5.10.7 節、イシュー
  [#468](https://github.com/Fandhe-AI/fandhe-backend/issues/468)）

## [0.2.0] - 2026-08-01

イシュー [#437](https://github.com/Fandhe-AI/fandhe-backend/issues/437) で公開対象
13 クレートを lockstep バンプし、`v0.2.0` タグ push → verify → dry-run →
GitHub Environments `crates-io-release` の承認を経て `release.yml` から
crates.io へ公開済み（[`docs/design/crates-io-release.md`](docs/design/crates-io-release.md)
6 節・7.1 節）。

### BREAKING CHANGES

1. **`fandhe-backend-core`**: `GateOutcome::Reject` を `{ status: u16, body: Vec<u8> }` から、
   構築時検証済みの `Response`（`fandhe-backend-http`）を運ぶ `{ response: Response }` へ
   変更しました（イシュー [#424](https://github.com/Fandhe-AI/fandhe-backend/issues/424)、
   PR [#431](https://github.com/Fandhe-AI/fandhe-backend/pull/431)）。
   - **移行手順**: ヘッダ不要の拒否応答は `GateOutcome::reject(status, body)` ヘルパへ
     書き換えてください（従来と同じワイヤ出力になります）。ヘッダ付き拒否応答
     （例: レート制限の `429 + Retry-After`）は `Response` を構築し
     `GateOutcome::Reject { response }` を返してください。拒否応答が必ず `Response`
     構築時のヘッダ検証（CR/LF/NUL 拒否・予約ヘッダ拒否）を通るようになり、
     `RequestGate` 実装からのヘッダインジェクションを型レベルで防ぐセキュリティ強化を
     兼ねています。
2. **`fandhe-backend-plugin-static`**: 非 `#[non_exhaustive]` の公開 enum
   `StaticConfigError` へ `InvalidMimeMapping` バリアントを追加しました（イシュー
   [#423](https://github.com/Fandhe-AI/fandhe-backend/issues/423)、PR
   [#430](https://github.com/Fandhe-AI/fandhe-backend/pull/430)）。
   - **移行手順**: `StaticConfigError` を網羅 `match` しているコードに
     `InvalidMimeMapping` の腕（または `_` フォールバック）を追加してください。

### Added

- `fandhe-backend-core`: 3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）で
  表現できないリダイレクト・レスポンス改変向けの拡張点 `Interceptor` を追加
  （`intercept` / `map_response` の 2 フック、`Server::interceptor` で複数登録可）
  （イシュー [#420](https://github.com/Fandhe-AI/fandhe-backend/issues/420)、PR
  [#428](https://github.com/Fandhe-AI/fandhe-backend/pull/428)）
- `fandhe-backend-core`: `Interceptor::map_response` を `Handler::handle_streaming` の
  ストリーミング応答ヘッドにも適用（PR
  [#443](https://github.com/Fandhe-AI/fandhe-backend/pull/443)）
- `fandhe-backend-core`: `static` / `compression` feature 有効時に設定型
  `StaticFilesConfig` / `CompressionConfig` を `plugin_static` / `plugin_compression`
  として再エクスポート（プラグインクレートへの直接依存を追加不要に）（イシュー
  [#421](https://github.com/Fandhe-AI/fandhe-backend/issues/421)、PR
  [#429](https://github.com/Fandhe-AI/fandhe-backend/pull/429)）
- `fandhe-backend-plugin-static`: `StaticFilesConfigBuilder::mime(ext, content_type)` で
  内蔵 MIME テーブルにない拡張子を利用者が追加登録可能に。内蔵テーブルへ
  `.webmanifest` 等の PWA/SSG 頻出拡張子も追加（イシュー
  [#423](https://github.com/Fandhe-AI/fandhe-backend/issues/423)、PR
  [#430](https://github.com/Fandhe-AI/fandhe-backend/pull/430)）
- `fandhe-backend-plugin-static`: `StaticFilesConfigBuilder::fallthrough_on_miss`
  （既定 `false`）で未ヒット GET を下流 `Handler` へフォールスルー可能に（mount `/` +
  動的エンドポイント共存構成、イシュー
  [#419](https://github.com/Fandhe-AI/fandhe-backend/issues/419)、PR
  [#427](https://github.com/Fandhe-AI/fandhe-backend/pull/427)）
- `fandhe-backend-plugin-compression`: `CompressionConfigBuilder::compress_streaming`
  （既定 `false`、opt-in）で `Handler::handle_streaming` の chunked ストリーミング
  応答へチャンク単位の gzip 圧縮を適用可能に。body 全体をバッファリングしない
  専用エンコーダ（`StreamingGzipEncoder`）+ コア側の第 5 のシーム
  （`prepare_streaming_compression`）で、既存のバックプレッシャ・応答完全性契約を
  壊さずに接続（HTTP/1.1 chunked 経路限定、HTTP/1.0 は対象外。イシュー
  [#461](https://github.com/Fandhe-AI/fandhe-backend/issues/461)）

### Fixed

- `fandhe-backend-plugin-static`: 末尾スラッシュ 1 個（`<mount>/dir/`）のディレクトリ
  URL で `index.html` を解決するよう修正（連続スラッシュ拒否は維持、イシュー
  [#418](https://github.com/Fandhe-AI/fandhe-backend/issues/418)、PR
  [#426](https://github.com/Fandhe-AI/fandhe-backend/pull/426)）

## [0.1.0] - 2026-07-21

公開対象 13 クレートの初回 publish。

