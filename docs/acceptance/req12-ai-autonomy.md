# REQ-12（AI 自律改修支援機構）・NFR-8 受け入れ検証結果（TASK-12.7 / #48）

`docs/spec/04-requirements.md` REQ-12・NFR-8 の受け入れ基準を
`scripts/accept/ai-autonomy-accept.sh`（基準 A〜F）で検証した結果を記録する。
数値の確定根拠・詳細な実行ログは
[`docs/reports/task-12-7-acceptance.md`](../reports/task-12-7-acceptance.md) を参照。

**結論（要約、2026-07-19 更新・D-2 再評価反映）**: 基準 A・B・C・D-1・D-2・E・E-2・F
（基準 7・8 を含む）の**すべてが PASS** となり、**REQ-12・NFR-8 の受け入れ基準を全基準で
充足した**。従来唯一の未達だった **D-2（基準 6 の人手評価）は FAIL → PASS** へ更新した:
前回「不当」評価の理由（トリアージ出力が検証方法・リスクを欠く）はコミット `becf0e0`
（#226/#234）で解消され、現行スクリプトの同一 fixture 再実行出力（必須 5 項目すべてを
含む）を人間レビュアーが再評価し「妥当」と判定した（妥当 1/1 = 100%、閾値 80% 充足。
`docs/reports/task-12-7-acceptance.md` 3.3 節）。基準 8（グレーゾーン再検証）は Issue
#240 の v2 再測定による PASS（4 値正解率 9/10 = 90%、閾値 80% 充足）を維持する。初回
D-2 = FAIL の記録は履歴として同レポートに保持する（PASS と偽らない fail-closed 運用の
確定記録）。
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
| 6 | 自動監査タスクの妥当性判断 80% 以上 | 基準 D-1（機械: `audit-triage.sh` fixture 検証）+ D-2（人手評価台帳集計） | D-1: 改善提案必須 5 項目（背景・根拠データ／影響範囲（crate 列）／対応方針（推奨アクション）／検証方法／リスク）全欄の生成を確認（PASS）。D-2: **PASS**（人間レビュアー評価「妥当」1/1 = 100%。初回評価「不当」の理由だった検証方法・リスク欄の欠落を `becf0e0` で解消後、現行版の同一 fixture 実行出力を再評価。再評価 2026-07-19、`docs/reports/task-12-7-acceptance.md` 3.3 節） |
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

## 既知の限界・過去 FAIL の是正経緯とその扱い

- **TASK-12.5 試行 2・3・TASK-12.6 グレーゾーン実測は 2026-07-19 に実施済み**
  （Issue #218。独立サブエージェントセッションを被験 AI とする 3 役分離、正解ラベルは
  隔離コミットで被験 worktree から削除）。旧 PENDING は解消した。
- **D-2 = FAIL → PASS の経緯**: 初回評価は人間レビュアーが 5 項目テンプレート全体
  （背景・根拠データ／影響範囲／対応方針／検証方法／リスク）で評価し「不当」と確定した
  実測値だった（検証方法・リスク欄の欠落が理由）。是正はコミット `becf0e0`（#226/#234、
  `audit-triage.sh` 出力への検証方法・リスク欄の追加）で完了し、2026-07-19 に現行
  スクリプトの同一 fixture（`audit-patched.json`。参考として `audit-unpatched-warning.json`・
  `audit-clean.json` の出力も提示）再実行出力を同レビュアーが**同一評価軸**で再評価し
  「妥当」と判定した。基準を緩めた再評価・評価軸の事後変更ではなく、FAIL 理由の解消を
  確認した再評価である。併せて `ai-autonomy-accept.sh` の D-1 チェックを必須 5 項目全欄の
  存在確認へ拡張し、再欠落を機械検知できるようにした（初回「不当」の記録は
  `docs/reports/task-12-7-acceptance.md` 3.3 節・人手評価台帳の履歴として保持）。
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
