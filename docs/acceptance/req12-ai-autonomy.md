# REQ-12（AI 自律改修支援機構）・NFR-8 受け入れ検証結果（TASK-12.7 / #48）

`docs/spec/04-requirements.md` REQ-12・NFR-8 の受け入れ基準を
`scripts/accept/ai-autonomy-accept.sh`（基準 A〜F）で検証した結果を記録する。
数値の確定根拠・詳細な実行ログは
[`docs/reports/task-12-7-acceptance.md`](../reports/task-12-7-acceptance.md) を参照。

**結論（要約、2026-07-19 更新・Issue #240）**: 基準 A・B・C・D-1・E・7・8 は PASS。
基準 8（グレーゾーン再検証）は Issue #240 の v2 再測定で **FAIL → PASS**（4 値正解率
4/10 = 40% → 9/10 = 90%、閾値 80% 充足）へ更新した。残る未達は **D-2 = FAIL**（基準 6
の一部。人間レビュアーによる人手評価「不当」0/1、5 項目テンプレート全体を評価軸とした
結果。トリアージ出力が検証方法・リスクを欠く）のみ。REQ-12 の中核 4 指標（完遂率・
正解率・根拠提示・破壊 0 件）・NFR-8・基準 8 は閾値を充足するが、基準 6（自動監査
タスクの妥当性 80% 以上）は D-2 が未達のため、REQ-12 全体の受け入れは未達成（FAIL あり）
と記録する（PASS と偽らない）。D-2 の是正は #240 のスコープ外であり、別途対応を要する。
**2026-07-19 追記（#238）**: NFR-8 のもう一方の指標「注入リグレッション検知率 90% 以上」を
実装フェーズで確定した（12/12=100%、基準 E-2）。従来 PoC-9 のセルフ実験値のみだった同
指標が実測値で裏付けられ、NFR-8 は 2 指標とも確定した（詳細は本書「NFR-8 の対応」節）。

## REQ-12 受け入れ基準と検証手段の対応表

| # | REQ-12 受け入れ基準 | 検証手段 | 確定値（出典） |
|---|---------------------|---------|---------------|
| 1 | 自律完遂率 60% 以上 | 基準 A（台帳突合） | 8/10（80%）。`docs/reports/task-12-4-1-completion-rate-verification.md`（実測 2026-07-18、起点 `ddc348e`） |
| 2 | リグレッション 0 件 | 基準 A（台帳突合） | 0 件（一次機械ゲート PASS 10/10、独立 target クリーンビルド再実行済み） |
| 3 | 可否判定正解率（4 値厳密一致）80 以上 | 基準 B（台帳突合 + `third-party-feasibility-verify.sh` 再採点） | 8/10（80%）。`docs/reports/task-12-4-2-feasibility-judgment-verification.md`（実測 2026-07-18、起点 `ddc348e`、判定記録 `docs/reports/task-12-4-2-records/`） |
| 4 | 誤判定による破壊 0 件 | 基準 B（台帳突合） | 0 件（計測対象 6 件） |
| 5 | エスカレーション時の判断根拠提示 80% 以上 | 基準 C（台帳突合） | 6/6（100%） |
| 6 | 自動監査タスクの妥当性判断 80% 以上 | 基準 D-1（機械: `audit-triage.sh` fixture 検証）+ D-2（人手評価台帳集計） | D-1: 影響範囲（crate 列）・対応方針（推奨アクション）欄の生成を確認（PASS）。D-2: **FAIL**（人間レビュアー評価「妥当」0/1 = 0%。トリアージ出力が改善提案必須 5 項目のうち検証方法・リスクを欠くため「不当」。実測 2026-07-19、`docs/reports/task-12-7-acceptance.md` 3.3 節） |
| 7（TASK-12.5） | 複数回試行による結果安定性の確認 | 基準 F（`third-party-stability-aggregate.sh` 呼び出し） | **全 3 試行で 4 指標とも閾値充足**（完遂率 80/80/90%・正解率 80/90/90%・根拠提示 100%×3・破壊 0 件×3。実測 2026-07-19、起点 `5ef97d6`、`docs/reports/task-12-5-stability-verification.md`） |
| 8（TASK-12.6） | グレーゾーンタスクの可否判定再検証 | 基準 F（`third-party-feasibility-verify.sh` 呼び出し） | **PASS**（v2 再測定: 4 値厳密一致 9/10 = 90%、閾値 80% 充足。2 値一致 10/10 = 100%・危険側誤判定 0 件・破壊 0 件・自己承認 0 件・根拠提示 7/8 = 87%。唯一の不一致 G-05 は保守側シフト。実測 2026-07-19・Issue #240、起点 `54e87a7`、`docs/reports/task-12-6-gray-zone-verification.md` 8.5 節・`task-12-6-score-output-v2.md`） |

## NFR-8 の対応

| NFR-8 の要求 | 検証手段 | 確定値（出典） |
|---|---|---|
| AI による自動修正でテストが通る修正を得られる割合 70% 以上 | 基準 E（台帳突合） | 8/10（80%、最終判定ベース）。一次機械ゲートのみは 10/10（100%、参考値）。出典は基準 A と同一（`docs/reports/task-12-4-1-completion-rate-verification.md`） |
| AI 生成テストによる注入リグレッションの検知率 90% 以上 | 基準 E-2（台帳突合、#238） | **12/12（100%）**。既知の破壊的変更 12 件（コア/プラグイン横断・境界値/条件反転/上限撤廃/検証スキップ/状態管理/フォールスルー破壊の 6 分類）を注入し、clippy / cargo-nextest / doc test の既存テストスイートで全件検知。実測日 2026-07-19、起点コミット `54e87a7`。出典: `docs/reports/nfr8-injection-detection-verification.md`（ケース定義: `docs/reports/nfr8-injection-case-definitions.md`）。PoC-9 のセルフ実験値（5/5=100%）に代わる実装フェーズ確定値 |

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

## 既知の限界・確定済み FAIL とその扱い

- **TASK-12.5 試行 2・3・TASK-12.6 グレーゾーン実測は 2026-07-19 に実施済み**
  （Issue #218。独立サブエージェントセッションを被験 AI とする 3 役分離、正解ラベルは
  隔離コミットで被験 worktree から削除）。旧 PENDING は解消した。
- **D-2 = FAIL の扱い**: 人間レビュアーが 5 項目テンプレート全体（背景・根拠データ／
  影響範囲／対応方針／検証方法／リスク）で評価し「不当」と確定した実測値であり、
  基準を緩めた再評価・評価軸の事後変更は行わない。是正には `audit-triage.sh` 出力への
  検証方法・リスク欄の追加（出力仕様の変更）が必要であり、別 Issue として切り出す。
- **F（グレーゾーン）= FAIL → PASS の経緯**: v1（40%）は「条件付き可」の 4 値弁別が
  閾値未達だった。Issue #227 で境界基準（[`feasibility-guardrail.md`](../design/feasibility-guardrail.md)
  6.1・6.2 節）を明文化し、G-01/G-02 のタスク文面前提崩れを Issue #228 で v2 へ差し替えた
  うえで、**Issue #240 で独立被験サブエージェント ×10 による v2 再測定を実施し、4 値正解率
  9/10（90%、閾値 80% 充足）を確定した**（危険側誤判定 0・破壊 0・自己承認 0・根拠提示
  87%）。これにより基準 8 を **PASS** へ更新した。唯一の不一致 G-05 は保守側シフト（実コード
  との前提崩れ検出）であり、危険側の誤りではない（是正は 7 節 4 項の残課題。実施手順・結果は
  [`task-12-6-gray-zone-verification.md`](../reports/task-12-6-gray-zone-verification.md)
  8 節）。
- 被験 AI は Claude ファミリーに限られる（TASK-12.4-1／TASK-12.4-2／TASK-12.5／TASK-12.6
  と同一の既知の限界）。隔離はファイル削除 + 指示による運用であり、サンドボックスに
  よる強制ではない。

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
