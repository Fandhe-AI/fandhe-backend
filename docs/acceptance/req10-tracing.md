# REQ-10 受け入れ検証レポート — 可観測性（tracing）依存インパクト・受け入れテスト（TASK-10.4 #59 / TASK-10.5 #60）

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-plugin-*` 等）表記のまま
> 保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。

## 本レポートの位置づけ

`docs/spec/04-requirements.md` REQ-10（可観測性 / tracing）の受け入れ記録は、他 REQ
（`req1`〜`req9`・`req12`・`req13`・`req15` 等）が従う `docs/acceptance/req<N>-<topic>.md`
の命名・配置パターンに対し `docs/reports/task-10-5-acceptance.md`（TASK-10.5、#60）に
のみ存在し不整合だった（イシュー #219）。本レポートはその一次記録の「基準・判定・
証跡」を `docs/acceptance/` の命名パターンへ集約転記したものであり、**作成時点での
再実測は行っていない**。実測値はすべて `docs/reports/task-10-5-acceptance.md`
（2026-07-18 実施）からの転記であり、一次記録の実行ログ・詳細解説はそちらを参照する。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実施日 | 2026-07-18 |
| OS | Linux 7.0.0-27-generic x86_64（Ubuntu） |
| rustc / cargo | 1.96.0（stable, 2026-05-25） |
| oha | 1.15.0 |
| 前提コミット | `8897059`（TASK-10.4 / #59 / PR #159 マージ済み、`origin/main`） |

## 判定サマリー（`scripts/accept/tracing-accept.sh`）

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A: `tracing` 無効時の依存完全除外 | `cargo tree -p backend-framework-core -e normal --no-default-features \| grep -c -E 'bf-plugin-tracing\|tracing-appender\|tracing-subscriber'` = 0 |
| WARN | A補足: `tracing` 有効時の依存インパクト（陽性対照） | 同条件 `--features tracing` = 4（`tracing`/`tracing-core`/`tracing-subscriber`/`tracing-appender` 本体行に相当。配線切れでないことの確認が目的で PASS/FAIL 判定には使わない） |
| PASS | B: `cargo test -p backend-framework-core --no-default-features` | `tracing` feature 無効時のフォールスルーを含め成功 |
| PASS | B: `cargo test -p backend-framework-core --features tracing` | `plugin_tracing_boundary.rs`（サンプリング判定・除外パス）を含め成功 |
| PASS | B: `cargo test -p bf-plugin-tracing` | `Sampler` / `TracingConfig` / `TracingLayer` の契約テストが成功 |
| PASS | C: NFR サンプリング適用後の性能影響 | シナリオ A（サンプリング + イベント統合 + `/health` 除外）RPS 比 98.59% / p95 比 102.27%（受け入れ帯: RPS 比 ≥95% かつ p95 比 ≤110%） |
| PASS | D: 依存インパクト記録の存在（`docs/dep-impact/records.md`） | `plugin-tracing` エントリを検出 |
| PASS | D: 連携方式設計文書の存在（`docs/design/tracing-integration.md`） | ファイル存在を確認 |
| PASS | E: 依存クレート数増分の機械検証 | 無効時 9 件 → 有効時 33 件（union 展開、新規 +24 件）。`records.md` 記録値 +24 件と許容帯（±5）内で一致 |

**終了コード: 0**（FAIL 0 件。PASS 8 件・WARN 1 件、SKIP 0 件）。

WARN は「A補足」（`tracing` 有効時の依存インパクト陽性対照。既存 TASK-10.4 実装の
情報チェックであり FAIL 扱いではない）のみで、隠さず転記する
（フェイルクローズ、`.claude/rules/security.md`）。

## 受け入れ基準チェックリストとの対応（TASK-10.5、REQ-10、`docs/spec/05-tasks.md`）

| 受け入れ基準 | 判定 | 根拠 |
|---|---|---|
| 依存インパクト実測記録（依存クレート数・バイナリサイズ・RSS の増分） | 充足 | `docs/dep-impact/records.md` 該当エントリ（依存 +24 クレート・バイナリ +32.6%・RSS +132.1%〜+145.4%、PoC-10 実測との比較込み） |
| `tracing` feature 無効時の依存完全除外確認 | 充足 | A/D/E チェック PASS（一次記録） |
| `tracing` エコシステムとの連携方式（サンプリング設定・記録粒度の切り替え方法）の設計文書化 | 充足 | `docs/design/tracing-integration.md` |
| 受け入れテストスクリプトと実行結果の記録 | 充足 | `scripts/accept/tracing-accept.sh`（D/E 追加）+ `docs/reports/task-10-5-acceptance.md`（一次記録）+ 本レポート |

## 検証コマンド一覧（再現手順）

```bash
# A〜E をまとめて実行（事前ビルドが前提）
bash scripts/accept/tracing-accept.sh

# 前提ビルド（tracing-accept.sh は自動ビルドしない）
cargo build --release -p backend-framework-core --example minimal --no-default-features
cargo build --release -p backend-framework-core --example tracing_nfr --features tracing

# 最小疎通テスト（境界テスト・契約テスト）
cargo test -p backend-framework-core --no-default-features
cargo test -p backend-framework-core --features tracing
cargo test -p bf-plugin-tracing

# pay-for-what-you-use ゲート
bash scripts/pay-for-what-you-use-check.sh

# 依存インパクトの個別確認
cargo tree -p backend-framework-core -e normal --no-default-features | grep -c -E 'bf-plugin-tracing|tracing-appender|tracing-subscriber'                    # 0
cargo tree -p backend-framework-core -e normal --no-default-features --features tracing | grep -c -E 'bf-plugin-tracing|tracing-appender|tracing-subscriber'  # 4
```

## 関連

- 一次記録（実行ログ・詳細解説の正本）: `docs/reports/task-10-5-acceptance.md`
- 性能詳細（TASK-10.4）: `benches/reports/task-10.4-tracing-performance.md`
- 連携方式設計文書: `docs/design/tracing-integration.md`
- 依存インパクト記録台帳: `docs/dep-impact/records.md`
- 関連 Issue: #59（TASK-10.4）・#60（TASK-10.5）・#219（本レポート新設）
