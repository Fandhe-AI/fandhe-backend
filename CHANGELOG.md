# Changelog

公開対象 13 クレート（`fandhe-backend-http` / `fandhe-backend-routes` / `fandhe-backend-core` /
`fandhe-backend-plugin-*` 10 種）は lockstep（同一バージョン一斉更新）で運用する
（詳細は [`docs/design/crates-io-release.md`](docs/design/crates-io-release.md) 7 節）。
恒久非公開クレート（`axum-ref` / `ws-load-client` / `docs-site`）はこの一覧に含めない。

## [0.2.0] - 準備中（crates.io publish は人間承認の別プロセス、未実施）

イシュー [#437](https://github.com/Fandhe-AI/fandhe-backend/issues/437) の Phase A（機械作業）
時点の内容。実 publish は `v0.2.0` タグ push（または `release.yml` の
`workflow_dispatch`）→ verify → dry-run → GitHub Environments `crates-io-release` の
required reviewers 承認を経て実施される（[`docs/design/crates-io-release.md`](docs/design/crates-io-release.md)
6 節）。

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

### Fixed

- `fandhe-backend-plugin-static`: 末尾スラッシュ 1 個（`<mount>/dir/`）のディレクトリ
  URL で `index.html` を解決するよう修正（連続スラッシュ拒否は維持、イシュー
  [#418](https://github.com/Fandhe-AI/fandhe-backend/issues/418)、PR
  [#426](https://github.com/Fandhe-AI/fandhe-backend/pull/426)）

## [0.1.0] - 2026-07-21

公開対象 13 クレートの初回 publish。

