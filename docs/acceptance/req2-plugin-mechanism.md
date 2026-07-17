# REQ-2 受け入れ検証レポート — プラグイン機構（TASK-2.4、#21）

`docs/spec/04-requirements.md` REQ-2（プラグイン機構）の受け入れ基準を
`scripts/accept/plugin-mechanism-accept.sh` で検証した結果。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-17 |
| 対象コミット（origin/main 先端。本ブランチは未 push） | `6134ec0`（`ci(global): TASK-2.2 pay-for-what-you-use 検証機構整備 (#134)`） |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| cargo-audit | 0.22.2 |
| cargo-deny | 0.19.8 |

## スコープ判断（実装時点の分岐と整合）

TASK-2.4 着手時点（依存グラフ上 `TASK-2.1 → TASK-2.2 → TASK-2.4` / `TASK-2.1 →
TASK-2.3 → TASK-2.4` であり `TASK-4.1`（実 WebSocket、#22）・`TASK-5.1`（実
GraphQL、#38）の前段）で、実 WebSocket プラグイン（`crates/plugin-websocket`、
TASK-4.1）が別 PR（#137）として並行実装中であることを確認した。同一クレート・
同一の `crates/core` 配線箇所を対象とする重複実装を避けるため
（`.claude/rules/out-of-scope-tracking.md`）、本タスクの「2 種のプラグイン」は
次の組み合わせで実証する:

1. **`webrtc-proxy` feature**（TASK-2.1 / #18 で確立済み、パスインターセプト型）
2. **`graphql` feature**（本タスクで新設、`crates/plugin-graphql`、パスインター
   セプト型の第 2 インスタンス）

実 GraphQL 実行（`async-graphql` 等）・実 WebSocket（RFC 6455 ハンドシェイク・
フレーミング）はいずれもスコープ外のまま（下記「スコープ外」参照）。

## 判定サマリー

`bash scripts/accept/plugin-mechanism-accept.sh` の実行結果（終了コード 0）。

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | 1: 2 種プラグイン feature 存在確認 | `webrtc-proxy`・`graphql` の両 feature が `backend-framework-core` に存在 |
| PASS | 2: pay-for-what-you-use 機械検証 | `scripts/pay-for-what-you-use-check.sh`（TASK-2.2）が PASS。graphql feature 追加後も動的列挙により無改修で検証対象化 |
| PASS | 3: build/test（no-default-features） | `cargo build`/`cargo test` 成功 |
| PASS | 3: build/test（graphql） | `cargo build`/`cargo test` 成功 |
| PASS | 3: build/test（webrtc-proxy） | `cargo build`/`cargo test` 成功 |
| PASS | 3: build/test（all-features） | `cargo build`/`cargo test` 成功 |
| PASS | 4: 安全性トレードオフ設計文書 | `docs/design/plugin-loading-tradeoffs.md` を新設 |
| SKIP | 5: 両 feature 無効時の性能維持（REQ-1 基準） | 下記「性能検証（手動・未実施）」を参照 |

**終了コード: 0（FAIL なし）**

## 個別基準への対応関係（REQ-2 受け入れ基準）

| REQ-2 受け入れ基準 | 対応状況 |
|--------------------|---------|
| プラグイン無効時、依存クレート・`unsafe`・コードが 0 件（`cargo tree`/`cargo geiger`/バイナリサイズ） | PASS（基準 2、`scripts/pay-for-what-you-use-check.sh` 内で個別検証済み。`webrtc-proxy`・`graphql` いずれも無効構成でバイナリシンボル・依存グラフから完全除外を確認） |
| 少なくとも 2 種のプラグインを feature flag で着脱でき、両方無効のコア性能が REQ-1 の性能基準を維持する | 前半 PASS（基準 1・3）。後半 SKIP（基準 5、下記参照） |
| コンパイル時方式と実行時動的ロード方式の安全性トレードオフが設計文書として記録されている | PASS（基準 4、`docs/design/plugin-loading-tradeoffs.md`） |
| 全リクエストに介入する `Middleware` 実装は非同期 I/O を用いる設計規約が `AGENTS.md` に明記されている | TASK-2.3（#20）で対応済み（本タスクのスコープ外、変更なし） |
| 新規プラグイン追加時、既存 3 種の拡張点で表現できない場合にのみ新規 trait 追加を検討する設計原則を開発規約に明記 | `docs/design/plugin-boundary.md` 5 節・`crates/plugin-graphql` の doc コメントが既存の `plugin::try_intercept` シームを踏襲する形で実践（新規 trait は追加していない） |

## 性能検証（手動・未実施）

両 feature 無効時のコア性能が REQ-1 の性能基準
（RPS axum 比 90% 以上・p95/p99 110% 以内・アイドル RSS 110% 以内・バイナリサイズ
同等以下・起動時間絶対差 20ms 未満）を維持することの計測には、axum-ref 等価の
4 エンドポイント（`GET /health` / `GET /hello/{name}` / `GET /users/{id}` /
`POST /echo`）を実装したコア側計測用バイナリ（`CORE_BIN`）が必要だが、
`benches/reports/task-1.6-1-performance.md`（TASK-1.6-1、#71）に記録のとおり
本 worktree 実行時点で当該バイナリは未整備で **BLOCKED** のままである。

計測用バイナリ整備は TASK-2.4（本タスク）で新規に着手するにはスコープが大きく
（`crates/core/examples/bench-endpoints.rs` の新設・`bf_routes::Router` を用いた
4 エンドポイント実装・`benches/bench-accept.sh` との結合確認を要する）、かつ
#15/#71 が既に同一課題を追跡中であるため、本タスクでの新規実装は行わない
（`.claude/rules/out-of-scope-tracking.md`、下記「スコープ外」参照）。

`CORE_BIN` 整備後の手動実行手順（`benches/README.md` 記載の再現手順に準拠）:

```bash
cargo build --release --example bench-endpoints -p backend-framework-core
CORE_BIN=target/release/examples/bench-endpoints \
  REPORT_MD=benches/reports/task-2.4-plugin-accept.md \
  ./benches/bench-accept.sh
```

参考情報として、`webrtc-proxy`・`graphql` の両 feature を有効化したビルドでも
無関係パス（`/health` 等）への性能影響は各プラグインの `try_handle_*` が対象外
パスで即座に `None` を返す設計（`crates/plugin-graphql`・
`crates/plugin-webrtc-proxy` の doc コメント参照）により実質ゼロと見込まれるが、
これも `CORE_BIN` 整備後に実測で確認する。

## スコープ外（`.claude/rules/out-of-scope-tracking.md` に従い記録）

- 実 WebSocket 実装（RFC 6455 ハンドシェイク・フレーミング） → TASK-4.1（#22、
  PR #137 で並行実装中）
- 実 GraphQL 実行（`async-graphql` 統合） → TASK-5.1（#38）
- axum-ref 等価計測用バイナリ（`crates/core/examples/bench-endpoints.rs`）の
  新設・`benches/bench-accept.sh` による REQ-1 性能受け入れ判定の解消 →
  TASK-1.6-1（#71）・#15
- 有効時の無関係パス性能影響の実測ゲート化 → TASK-4.4（#25）・TASK-5.2（#53）
- `scripts/accept/core-deps-unsafe-audit.sh` 基準 E「コアループの feature 非分岐」
  チェックが `crates/core/src/plugin.rs`・`crates/core/src/server.rs` の
  `#[cfg(feature = ...)]`（TASK-2.1 で意図的に導入したプラグイン境界シーム）を
  検知して FAIL する事象を本タスク実行時に確認した。TASK-2.1（#18）マージ時点
  から既存の事象であり本タスクが新規に生じさせたものではないため、本タスクの
  変更対象には含めない（別途チェックロジックの見直しを追跡する）

## 参照

- `docs/design/plugin-loading-tradeoffs.md`（安全性トレードオフ設計文書）
- `docs/design/plugin-boundary.md`（プラグイン境界パターン、7 節を本タスクで更新）
- `crates/plugin-graphql/src/lib.rs`（第 2 プラグイン境界インスタンスの doc）
- `scripts/accept/plugin-mechanism-accept.sh`（本レポートの実行スクリプト）
- `benches/reports/task-1.6-1-performance.md`（性能受け入れ BLOCKED の記録）
