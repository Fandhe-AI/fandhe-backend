# REQ-5 受け入れ検証レポート — GraphQL 受け入れテスト（TASK-5.2、#53）

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` / `bf-plugin-*` 等）
> 表記のまま保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。

`docs/spec/04-requirements.md` REQ-5（GraphQL）の受け入れ基準のうち TASK-5.2 が担う
「GraphQL 受け入れテスト」を `scripts/accept/graphql-accept.sh` で検証した結果。
TASK-2.4（#21、パスインターセプト型境界の確立）・TASK-5.1（#38、`async-graphql` に
よる実クエリ実行実装、PR #144）は前提タスクとして `origin/main` へマージ済み（本
レポートはそれらの変更を前提とし、production コードの追加変更は行っていない。
`crates/core/examples/graphql_nfr6.rs`・`benches/graphql-nfr6-bench.sh`・
`scripts/accept/graphql-accept.sh` はいずれも test スコープの新規追加）。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-17 |
| 対象コミット（作業ブランチ起点、`origin/main`） | `e3da2960baeaf230cbd84c15d7b6f403742d008d` |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| oha | 1.15.0 |
| jq | 1.8.1 |

## 判定サマリー（`scripts/accept/graphql-accept.sh`）

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A: graphql 無効時の依存完全除外 | `cargo tree -p backend-framework-core -e normal --no-default-features \| grep -c -E 'async-graphql\|bf-plugin-graphql'` = 0 |
| WARN | A補足: graphql 有効時の依存インパクト（陽性対照） | 同条件 `--features graphql` = 7（`docs/dep-impact/records.md` TASK-5.1 エントリと整合。配線切れでないことの確認が目的で PASS/FAIL 判定には使わない） |
| PASS | A補足: pay-for-what-you-use-check.sh | 全プラグイン feature（graphql 含む、動的列挙）の依存・unsafe・バイナリサイズ完全除外を確認 |
| PASS | A': plugin-graphql 自コード unsafe 0件 | `crates/plugin-graphql/src` に unsafe 0 件（テキストベース走査） |
| PASS | B: `cargo test -p backend-framework-core --features graphql` | `plugin_graphql_boundary.rs`（`POST /graphql` 実クエリ実行・200・`application/json`・`hello:world`、未登録時 404 フォールスルー、無関係パスのフォールスルー）を含め成功 |
| PASS | B補足: `cargo test -p backend-framework-core --no-default-features` | `plugin_graphql_boundary_disabled.rs`（graphql feature 無効時の陰性対照）を含め成功 |
| PASS | B: `cargo test -p bf-plugin-graphql` | `try_handle_graphql` の契約テスト（クエリ実行・エラー処理・不正 JSON 拒否・メソッド不一致フォールスルー等 8 件）が成功 |
| PASS | B補足: graphql_nfr6 live 疎通確認 | `target/release/examples/graphql_nfr6` へ `POST /graphql {"query":"{ hello }"}` を送信し `{"data":{"hello":"world"}}` を確認 |
| FAIL | C: NFR 無関係パス影響 | RPS 比・p95 比とも実行のたびに大きく変動（詳細下記）。最終実行（採用値）は RPS 比 93.72% / p95 比 111.31% |

**終了コード: 1（FAIL あり、基準 C）**

## 基準 C（NFR）の詳細と判断

`benches/graphql-nfr6-bench.sh`（`oha` による empirical 計測。計測用バイナリ:
`crates/core/examples/minimal.rs` = ベースライン、`crates/core/examples/graphql_nfr6.rs` =
`graphql` feature 有効・`Server::graphql` 登録済み、RUNS=5・DURATION=5s・
CONNECTIONS=32）を 5 回実行した結果:

| 実行 | RPS 比（graphql / baseline） | p95 比（graphql / baseline） | `evaluate_nfr6_ratio` |
|------|------|------|------|
| 1 回目 | 108.51% | 50.25% | FAIL（RPS が実務帯上限超） |
| 2 回目 | 95.74% | 103.34% | WARN |
| 3 回目 | 93.98% | 104.09% | FAIL（RPS が実務帯下限未満） |
| 4 回目（採用値） | 93.72% | 111.31% | FAIL |
| 5 回目 | 105.47% | 56.59% | FAIL（RPS が実務帯上限超） |

詳細な生ログは `benches/reports/task-5.2-graphql-performance.md` を参照。

**判断**: `crate::plugin::try_intercept` は `graphql` パス以外に対して 1 回のパス比較の
みでフォールスルーする（`crates/core/src/plugin.rs`）ため、拡張点呼び出しコスト自体は
無視できるはずだが、5 回の実測は RPS 比 93.72〜108.51%・p95 比 50.25〜111.31% と
振れ幅が非常に大きく、`scripts/accept/lib/nfr6-ratio.sh`（`evaluate_nfr6_ratio`）の
実務許容帯 [95%, 105%]（RPS）・[0, 105%]（p95、片側）に安定して収まらない。狭義帯
（100.3〜100.8%相当）はいずれの実行でも達成しなかった。判定を PASS/WARN に丸めず、
最終実行の結果（4 回目、FAIL）を採用値として記録する（フェイルクローズ、
`.claude/rules/security.md`。捏造しない・都合の良い実行を恣意的に選ばない）。

**判断の背景**: 本実行環境は「複数イシューが並列実行されている」ワークフロー上の
worktree であり、他エージェントの並行実行によるホスト負荷ノイズが RPS・p95 の振れ幅
（特に p95 が baseline 比 50〜111% と 2 倍以上の開きを見せる点）に強く影響している
可能性が高い。`crates/core/src/plugin.rs` の `try_intercept` 自体の設計（無関係パスへの
1 回のパス比較のみ）からは、これほどの振れ幅が生じる理由がない。production コード
（`crates/core/src/plugin.rs`・`crates/plugin-graphql/src/lib.rs`）は本タスクで変更して
おらず、TASK-5.1（#38、PR #144）でマージ済みの実装のまま。

## 受け入れ条件チェックリストとの対応

- [x] 依存除外: 基準 A・A' が PASS（`cargo tree` 0 件・`pay-for-what-you-use-check.sh`
      PASS・unsafe 0 件）
- [ ] 性能影響誤差範囲: 基準 C は環境ノイズにより安定した PASS/WARN を得られず、
      最終実行は FAIL。**未達として記録する**（フェイルクローズ、PASS を偽らない）
- [x] 最小疎通: 基準 B が PASS（クエリ実行と結果 JSON 返却、境界テスト・契約テスト・
      live 疎通確認のいずれも成功）
- [x] 成果物（スクリプト・実行結果レポート）が揃っている

## BLOCKED / フォローアップ

- **NFR 計測環境の安定化**: 本 worktree 実行環境（並列 issue 実装ワークフロー下の
  sandbox）では `oha` 計測の振れ幅が実務許容帯を大きく超えて安定しない
  （5 回中 3 回 FAIL、2 回 WARN/FAIL 境界）。専有環境（他プロセスの並行負荷がない
  CI ランナー等）での再計測、または計測条件（DURATION・CONNECTIONS の引き上げに
  よる平滑化）の見直しが必要。性能最適化の深掘りではなく計測環境・条件の問題である
  可能性が高いため、`webrtc-nfr6-bench.sh`（TASK-8.4）の狭義帯未達（安定した FAIL）
  とは性質が異なる。out-of-scope-tracking 候補としてユーザーへ報告する
  （`.claude/rules/out-of-scope-tracking.md`）。
- **専有計測環境（#178）**: `benches/nfr6-exclusive.sh`（flock 相互排他 + 静穏確認、
  `docs/design/nfr6-exclusive-measurement.md`）を整備した。しかし本イシュー実装時点も
  並列 issue 実装ワークフロー実行中であり、静穏確認が成立せず GraphQL 対象の専有環境
  確定再計測はできなかった（`benches/reports/task-5.2-graphql-performance.md` 追補節）。
  上記 FAIL 記録は維持し、host が真に静穏な期間の再計測をフォローアップとする。

## 検証コマンド一覧（再現手順）

```bash
# A・B・C をまとめて実行
bash scripts/accept/graphql-accept.sh

# 判定ロジックのオフライン・セルフテスト（cargo 非依存）
bash scripts/tests/run-graphql-accept-tests.sh

# NFR 計測用バイナリのビルド（C の前提）
cargo build --release -p backend-framework-core --example minimal --no-default-features
cargo build --release -p backend-framework-core --example graphql_nfr6 --features graphql

# 最小疎通テスト（境界テスト・契約テスト）
cargo test -p backend-framework-core --features graphql
cargo test -p backend-framework-core --no-default-features
cargo test -p bf-plugin-graphql

# pay-for-what-you-use ゲート
bash scripts/pay-for-what-you-use-check.sh

# 依存インパクトの個別確認
cargo tree -p backend-framework-core -e normal --no-default-features | grep -c -E 'async-graphql|bf-plugin-graphql'                    # 0
cargo tree -p backend-framework-core -e normal --no-default-features --features graphql | grep -c -E 'async-graphql|bf-plugin-graphql'  # 7
```
