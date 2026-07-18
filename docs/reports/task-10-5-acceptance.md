# TASK-10.5 受け入れテスト実行結果レポート

Issue #60（TASK-10.5、REQ-10）の成果物。`scripts/accept/tracing-accept.sh`（D/E チェック
拡張）による TASK-10.5「依存インパクト記録・文書化・受け入れテスト」受け入れ確認の
実行結果を記録する。TASK-10.4（#59）の既存 A〜C チェックも同一実行に含まれる。

## 実施日時・環境

- 実施日時: 2026-07-18
- OS: Linux 7.0.0-27-generic x86_64（Ubuntu）
- rustc / cargo: 1.96.0（stable, 2026-05-25）
- oha: 1.15.0
- 前提コミット: `8897059`（TASK-10.4 / #59 / PR #159 マージ済み、origin/main）

## 総合判定

```
$ bash scripts/accept/tracing-accept.sh
=== REQ-10 / TASK-10.4 受け入れ検証（サンプリング適用後性能再検証） ===
workspace root: <worktree>

[PASS] A: tracing 無効時の依存完全除外: cargo tree -p backend-framework-core -e normal --no-default-features | grep -c -E 'bf-plugin-tracing|tracing-appender|tracing-subscriber' = 0
[WARN] A補足: tracing 有効時の依存インパクト（陽性対照）: cargo tree -p backend-framework-core -e normal --no-default-features --features tracing | grep -c -E 'bf-plugin-tracing|tracing-appender|tracing-subscriber' = 4
[PASS] B: cargo test -p backend-framework-core --no-default-features: tracing feature 無効時のフォールスルーを含め成功
[PASS] B: cargo test -p backend-framework-core --features tracing: plugin_tracing_boundary.rs（サンプリング判定・除外パス）を含め成功
[PASS] B: cargo test -p bf-plugin-tracing: Sampler / TracingConfig / TracingLayer の契約テストが成功
[PASS] C: NFR サンプリング適用後の性能影響: シナリオA（サンプリング + イベント統合 + /health 除外）RPS 比 98.59% / p95 比 102.27%（受け入れ帯: RPS 比 >= 95% かつ p95 比 <= 110%）
[PASS] D: 依存インパクト記録の存在（docs/dep-impact/records.md）: plugin-tracing エントリを検出
[PASS] D: 連携方式設計文書の存在（docs/design/tracing-integration.md）: ファイル存在を確認
[PASS] E: 依存クレート数増分の機械検証: 無効時 9 件 → 有効時 33 件（union 展開、新規 +24 件）。records.md 記録値 +24 件と許容帯（±5）内で一致

結果: FAIL なし（PASS / SKIP / WARN のみ）。
```

**終了コード: 0**（FAIL 0 件。PASS 8 件・WARN 1 件、SKIP 0 件）。

WARN は「A補足」（`tracing` 有効時の依存インパクト陽性対照、既存 TASK-10.4 実装の
情報チェックであり FAIL 扱いではない。`grep -c` は行出現数 4 件＝
`tracing`/`tracing-core`/`tracing-subscriber`/`tracing-appender` 本体行に相当し、
配線切れがないことの確認が目的）。

## 各チェックの詳細

### A: `tracing` feature 無効時の依存完全除外（TASK-10.4、既存）

pay-for-what-you-use の完全除外を再確認。`bf-plugin-tracing` / `tracing-appender` /
`tracing-subscriber` は無効構成の依存ツリーに 0 件（`docs/dep-impact/records.md` の
本タスクエントリと一致）。

### B: テスト回帰（TASK-10.4、既存）

`cargo test -p backend-framework-core --no-default-features` /
`--features tracing` / `cargo test -p bf-plugin-tracing` の 3 構成すべて成功。

### C: NFR サンプリング適用後の性能影響（TASK-10.4、既存）

RPS 比 98.59%（受け入れ帯 ≥95%）・p95 比 102.27%（受け入れ帯 ≤110%）で受け入れ基準
を満たす。TASK-10.4（#59）実施時の実測（RPS 劣化 3.34%・p95 悪化 2.77%）とはビルド
環境・実行タイミングによる差異があるが同一オーダーで整合する。

### D: 依存インパクト記録・連携方式設計文書の存在検証（TASK-10.5、新規）

- `docs/dep-impact/records.md` の「2026-07-18 — `crates/plugin-tracing` 依存インパクト
  記録（#60、TASK-10.5）」エントリを検出（PASS）
- `docs/design/tracing-integration.md` の存在を確認（PASS）

### E: 依存クレート数増分の機械検証（TASK-10.5、新規）

`cargo tree -p backend-framework-core --features tracing` の union 展開（`name
vX.Y.Z` 形式のユニークパッケージ集合の差分）で無効時 9 件・有効時 33 件・新規 +24 件
を機械算出し、`docs/dep-impact/records.md` の記録値（+24）と許容帯（±5）で一致を確認
（PASS）。

## 受け入れ基準との対応（TASK-10.5、REQ-10、`docs/spec/05-tasks.md`）

| 受け入れ基準 | 判定 | 根拠 |
|---|---|---|
| 依存インパクト実測記録（依存クレート数・バイナリサイズ・RSS の増分） | 充足 | `docs/dep-impact/records.md` 該当エントリ（依存 +24 クレート・バイナリ +32.6%・RSS +132.1%〜+145.4%、PoC-10 実測との比較込み） |
| feature 無効時の依存完全除外確認 | 充足 | A/D/E チェック PASS（本レポート） |
| `tracing` エコシステムとの連携方式（サンプリング設定・記録粒度の切り替え方法）の設計文書化 | 充足 | `docs/design/tracing-integration.md` |
| 受け入れテストスクリプトと実行結果の記録 | 充足 | `scripts/accept/tracing-accept.sh`（D/E 追加）+ 本レポート |

## 補足検証（実装後フロー）

- `bash scripts/pay-for-what-you-use-check.sh`: 本タスクは `tracing` feature ゲート・
  依存構成を変更しないため既存結果から変化なし（実行して回帰なきことを別途確認）
- `cargo build --workspace --all-targets`（feature なし）・`--features tracing` の
  各構成ビルド成功を確認
- Rust コード変更なし（docs + シェルスクリプトのみ）のため `cargo fmt --check` /
  `cargo clippy -- -D warnings` は現状維持
