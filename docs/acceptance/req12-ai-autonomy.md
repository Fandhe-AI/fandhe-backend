# REQ-12（AI 自律改修支援機構）・NFR-8 受け入れ検証結果（TASK-12.7 / #48）

`docs/spec/04-requirements.md` REQ-12・NFR-8 の受け入れ基準を
`scripts/accept/ai-autonomy-accept.sh`（基準 A〜F）で検証した結果を記録する。
数値の確定根拠・詳細な実行ログは
[`docs/reports/task-12-7-acceptance.md`](../reports/task-12-7-acceptance.md) を参照。

**結論（要約）**: 機械検証可能な基準（A・B・C・D-1・E）はすべて PASS（FAIL 0 件）。
D-2（自動監査タスクの人手評価）・F（複数回試行の安定性・グレーゾーン再検証）は
実測 PENDING のため SKIP（PASS と偽らない）。TASK-12.4-1／TASK-12.4-2 の第三者実測値
（起点コミット `ddc348e`）を確定値として採用し、REQ-12 の 4 指標・NFR-8 はいずれも
閾値を充足する。

## REQ-12 受け入れ基準と検証手段の対応表

| # | REQ-12 受け入れ基準 | 検証手段 | 確定値（出典） |
|---|---------------------|---------|---------------|
| 1 | 自律完遂率 60% 以上 | 基準 A（台帳突合） | 8/10（80%）。`docs/reports/task-12-4-1-completion-rate-verification.md`（実測 2026-07-18、起点 `ddc348e`） |
| 2 | リグレッション 0 件 | 基準 A（台帳突合） | 0 件（一次機械ゲート PASS 10/10、独立 target クリーンビルド再実行済み） |
| 3 | 可否判定正解率（4 値厳密一致）80 以上 | 基準 B（台帳突合 + `third-party-feasibility-verify.sh` 再採点） | 8/10（80%）。`docs/reports/task-12-4-2-feasibility-judgment-verification.md`（実測 2026-07-18、起点 `ddc348e`、判定記録 `docs/reports/task-12-4-2-records/`） |
| 4 | 誤判定による破壊 0 件 | 基準 B（台帳突合） | 0 件（計測対象 6 件） |
| 5 | エスカレーション時の判断根拠提示 80% 以上 | 基準 C（台帳突合） | 6/6（100%） |
| 6 | 自動監査タスクの妥当性判断 80% 以上 | 基準 D-1（機械: `audit-triage.sh` fixture 検証）+ D-2（人手評価台帳集計） | D-1: 影響範囲（crate 列）・対応方針（推奨アクション）欄の生成を確認（PASS）。D-2: 人手評価未実施（SKIP、PENDING） |
| 7（TASK-12.5） | 複数回試行による結果安定性の確認 | 基準 F（`third-party-stability-aggregate.sh` 呼び出し） | 試行 1（v1）のみ。試行 2・3（v2）は被験セッション起動権限がなく PENDING（SKIP） |
| 8（TASK-12.6） | グレーゾーンタスクの可否判定再検証 | 基準 F（`third-party-feasibility-verify.sh` 呼び出し） | 実測未実施（PENDING、SKIP） |

## NFR-8 の対応

| NFR-8 の要求 | 検証手段 | 確定値（出典） |
|---|---|---|
| AI による自動修正でテストが通る修正を得られる割合 70% 以上 | 基準 E（台帳突合） | 8/10（80%、最終判定ベース）。一次機械ゲートのみは 10/10（100%、参考値）。出典は基準 A と同一（`docs/reports/task-12-4-1-completion-rate-verification.md`） |

## 検証コマンド・再現手順

```bash
bash scripts/accept/ai-autonomy-accept.sh
```

セルフテスト（判定ロジック単体の回帰確認、cargo・ネットワーク非依存）:

```bash
bash scripts/tests/run-ai-autonomy-accept-tests.sh
```

`--ledger <file>` / `--audit-fixtures-dir <dir>` / `--acceptance-doc <file>` /
`--reports-dir <dir>` で各基準の検証対象を差し替え可能
（`scripts/tests/run-ai-autonomy-accept-tests.sh` の注入口）。

## 既知の限界・PENDING 事項

- **TASK-12.5 試行 2・3・TASK-12.6 グレーゾーン実測**: いずれも被験セッション（別モデル・
  別セッション）を起動する権限が本タスクの実行環境にないため PENDING。プロトコル・
  タスク定義・採点/集計ハーネスは完備しており、独立セッション起動が可能な実行主体が
  `docs/design/multi-trial-stability-verification.md`・
  `docs/design/gray-zone-feasibility-verification.md` の手順に従い実施可能。
- **自動監査タスクの人手評価**: `docs/reports/task-12-7-acceptance.md` の人手評価台帳は
  未記入（PENDING）。人間レビュアーによる評価記入後、`ai-autonomy-accept.sh` を再実行
  すると D-2 が機械集計される。
- 被験 AI は Claude ファミリーに限られる（TASK-12.4-1／TASK-12.4-2／TASK-12.5／TASK-12.6
  と同一の既知の限界）。

## 参照

- 詳細実行ログ・確定値の経緯: [`docs/reports/task-12-7-acceptance.md`](../reports/task-12-7-acceptance.md)
- 確定値台帳（機械可読）: `docs/reports/task-12-7-metrics.summary`
- 完遂率再検証: [`docs/reports/task-12-4-1-completion-rate-verification.md`](../reports/task-12-4-1-completion-rate-verification.md)
- 可否判定正解率再検証: [`docs/reports/task-12-4-2-feasibility-judgment-verification.md`](../reports/task-12-4-2-feasibility-judgment-verification.md)
- 複数回試行安定性確認: [`docs/reports/task-12-5-stability-verification.md`](../reports/task-12-5-stability-verification.md)
- グレーゾーン再検証: [`docs/reports/task-12-6-gray-zone-verification.md`](../reports/task-12-6-gray-zone-verification.md)
- 改善提案フロー: [`docs/design/improvement-proposal-flow.md`](../design/improvement-proposal-flow.md)
- 対応タスク: `docs/spec/05-tasks.md` TASK-12.7
- 根拠要件: `docs/spec/04-requirements.md` REQ-12・NFR-8
