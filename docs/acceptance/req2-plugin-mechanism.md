# REQ-2 受け入れ検証レポート — プラグイン機構（TASK-2.4、#21）

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` / `bf-plugin-*` 等）
> 表記のまま保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。

> **再検証済み（イシュー #261）**: `docs/spec/04-requirements.md` REQ-2 が名指しする
> **WebSocket・GraphQL** の実プラグインペアで着脱を再検証済み。最新結果は末尾の
> 「再検証（#261、websocket + graphql 実ペア）」節を参照。以下の本文（2026-07-17 の
> `webrtc-proxy` + `graphql` 代替ペアでの実測・当時のスコープ判断）は実装当時の判断
> 経緯として一切改変せず保持する（`docs/design/framework-naming.md` 7 節と同方針）。

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

## 再検証（#261、websocket + graphql 実ペア）

TASK-2.4（#21）実施当時は実 WebSocket プラグイン（TASK-4.1、#22）が並行実装中だった
ため代替ペア（`webrtc-proxy` + `graphql`）で暫定実証したが（上記本文）、現在は実
WebSocket プラグイン（`crates/plugin-websocket`）と実 GraphQL 実行
（`async-graphql`、TASK-5.1・#38）がともに実装済みのため、
`docs/spec/04-requirements.md` REQ-2 が名指しする **websocket + graphql** の実
プラグインペアで再検証した（イシュー #261）。`scripts/accept/plugin-mechanism-accept.sh`
を対象 feature ペアのパラメータ化（環境変数 `REQ2_FEATURES`、既定
`"websocket graphql"`）へ更新し、既定値（実ペア）で再実行した。

### 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-19 |
| 対象コミット | `3e55c52`（origin/main 先端） |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |

### 判定サマリー

`bash scripts/accept/plugin-mechanism-accept.sh`（既定 `REQ2_FEATURES="websocket graphql"`）の実行結果（終了コード 0）。

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | 1: 対象プラグイン feature 存在確認 | `websocket`・`graphql` が `fandhe-backend-core` に存在 |
| PASS | 2: pay-for-what-you-use 機械検証 | `scripts/pay-for-what-you-use-check.sh` PASS |
| PASS | 2b: 対象ペア cargo tree 直接確認 | 両 feature で無効時不出現・有効時出現を確認（配線切れなし） |
| PASS | 3: build/test（no-default-features） | `cargo build`/`test` 成功 |
| PASS | 3: build/test（websocket 単独） | `cargo build`/`test` 成功（`websocket_upgrade.rs` 等の RFC 6455 ハンドシェイク動作確認を含む） |
| PASS | 3: build/test（graphql 単独） | `cargo build`/`test` 成功（`plugin_graphql_boundary.rs` の実クエリ実行動作確認を含む） |
| PASS | 3: build/test（websocket,graphql 同時有効） | `cargo build`/`test` 成功（REQ-2「2 種を着脱できる」の同時有効側を実証） |
| PASS | 3: build/test（all-features） | `cargo build`/`test` 成功 |
| PASS | 3b: プラグイン単体契約テスト（`fandhe-backend-plugin-websocket`） | `cargo test -p` 成功 |
| PASS | 3b: プラグイン単体契約テスト（`fandhe-backend-plugin-graphql`） | `cargo test -p` 成功 |
| PASS | 4: 安全性トレードオフ設計文書 | `docs/design/plugin-loading-tradeoffs.md` 存在確認 |
| SKIP | 5: 両 feature 無効時の性能維持（REQ-1 基準） | 専有計測枠（`benches/nfr6-exclusive.sh`、#178）が必要なため自動検証対象外（下記「基準 5 について」参照） |

旧代替ペア（`webrtc-proxy` + `graphql`）も `REQ2_FEATURES="webrtc-proxy graphql"` で
再実行し、同様に全基準 PASS・基準 5 SKIP を確認した（後方互換）。

陰性対照（フェイルクローズ検証）:

| 入力 | 結果 |
|------|------|
| `REQ2_FEATURES="no-such-feature graphql"`（存在しない feature） | 基準 1 で即 FAIL・終了コード非 0（cargo build 前に短絡） |
| `REQ2_FEATURES='foo;rm graphql'`（許可されない文字） | 基準 0 の入力検証で即 FAIL・終了コード非 0（cargo 未実行） |
| `REQ2_FEATURES="graphql"`（1 種のみ） | 基準 0 で即 FAIL・終了コード非 0（REQ-2 は最低 2 種を要求） |

### REQ-2 受け入れ基準との対応

`docs/spec/04-requirements.md` REQ-2 の「少なくとも 2 種のプラグイン（WebSocket・
GraphQL）を feature flag で着脱できる」という受け入れ基準を、仕様が名指しする実
プラグインペアそのもので充足したことを確認した:

- **WebSocket**: `crates/plugin-websocket`（RFC 6455 ハンドシェイク検証・101 応答・
  `tokio-tungstenite` へのフレーミング委譲、TASK-4.1・#22）
- **GraphQL**: `crates/plugin-graphql`（`async-graphql` による実クエリ実行、
  TASK-5.1・#38）

両 feature とも `crates/core/Cargo.toml` で `optional = true` + `dep:` 構文により
配線され、無効時は `cargo tree` に当該プラグインクレートが一切出現しない
（pay-for-what-you-use、上記基準 2b）。

### 基準 5（性能維持）について

旧レポート（上記本文）が SKIP 理由とした「axum-ref 等価計測用バイナリが #15/#71
BLOCKED」は陳腐化している（#15・#71 とも CLOSED、`crates/core/examples/core-bench.rs`
が整備済み）。本再検証では性能再計測は受け入れ条件に含めていないが、専有計測枠
（`benches/nfr6-exclusive.sh`、#178。並列 issue 実装ワークフロー下の host
contention により非専有環境では NFR-6 系の判定が不確定になるため導入された仕組み）
が必要なため、引き続き自動検証対象外として SKIP を維持する（判定不能を PASS と
偽らないフェイルクローズ原則、`.claude/rules/security.md`）。

手動再現手順（`core-bench` 使用）:

```bash
cargo build --release -p fandhe-backend-core --example core-bench --no-default-features
# benches/README.md の手順に従い、両 feature 無効構成での RPS を計測し
# REQ-1 基準（docs/spec/04-requirements.md）との比較を行う。
```

実測証跡は `benches/reports/task-1.6-1-performance.md` を参照。

### 実行環境上の注意（本再検証で確認した事象、フレームワーク実装への影響なし）

本再検証の初回実行時、`/tmp`（tmpfs、並列 worktree 実行による共有ホストの
リソース逼迫）が逼迫している状態で `cargo test`（doc test のリンク時に `/tmp` を
使用）が `Bus error` で失敗する事象を確認した。`TMPDIR` をディスクバック
ファイルシステム上のディレクトリへ切り替えて再実行したところ全構成 PASS した。
これは実行環境（共有ホストの `/tmp` 容量）に起因する事象であり、
`fandhe-backend` 側の実装・本受け入れスクリプトの不備ではない。

### 対象外（out-of-scope-tracking 対象）

- 両 feature 無効時の REQ-1 性能基準の実測再計測（専有計測枠 #178 の運用が必要。
  基準 5 は SKIP のまま維持）
- `plugin-mechanism-accept.sh` の fixture ベースセルフテスト新設（判定ロジックの
  大半が cargo 実行そのものであり fixture 化になじまないため見送り）
- `/tmp` 固定パスの `mktemp` 化（既存 accept スクリプト群横断の課題）
