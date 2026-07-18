# TASK-12.7 AI 自律改修支援機構受け入れテスト・確定版測定値レポート

TASK-12.7（#48、`docs/spec/05-tasks.md` 380〜385 行目、Conditional Go 条件 (3)）の成果物。
TASK-12.4〜12.6 の第三者再検証結果に基づき、REQ-12・NFR-8 の受け入れ基準値を確定し、
機械検証可能な受け入れテスト（`scripts/accept/ai-autonomy-accept.sh`）の実行結果を記録する。

**結論（要約）**: 機械検証可能な基準 A・B・C・D-1・E はすべて PASS（FAIL 0 件）。
D-2（自動監査タスクの人手評価）・F（TASK-12.5 試行 2・3・TASK-12.6 のグレーゾーン実測）は
実測 PENDING のため SKIP と記録する（PASS と偽らない、`.claude/rules/security.md`
フェイルクローズ原則）。

## 1. 確定値の根拠（TASK-12.4〜12.6 の位置づけ）

本タスクの受け入れ条件は「TASK-12.4〜12.6 の第三者再検証結果に基づき受け入れ基準値を
確定する」ことを求める。各タスクの状態は次のとおり:

| タスク | 状態 | 本レポートでの扱い |
|---|---|---|
| TASK-12.4-1（#85、自律完遂率） | 実測確定済み（2026-07-18、起点 `ddc348e`） | 確定値として採用（2 節） |
| TASK-12.4-2（#86、可否判定正解率） | 実測確定済み（2026-07-18、起点 `ddc348e`、人間サインオフ済み） | 確定値として採用（2 節） |
| TASK-12.5（#46、複数回試行安定性） | 試行 1（v1、TASK-12.4-1/12.4-2 の転記）のみ完了。試行 2・3（v2）は被験セッション起動権限がなく PENDING | 試行 1 を確定値の裏付けとして参照し、試行 2・3 は F 基準で PENDING（SKIP）と記録（3 節） |
| TASK-12.6（#47、グレーゾーン再検証） | プロトコル・タスク定義・採点ハーネス拡張は完備、実測は同様に PENDING | F 基準で PENDING（SKIP）と記録（3 節） |

TASK-12.5・TASK-12.6 が PENDING のまま残した理由は両タスクのレポート（
[`task-12-5-stability-verification.md`](./task-12-5-stability-verification.md)・
[`task-12-6-gray-zone-verification.md`](./task-12-6-gray-zone-verification.md)）と同一
（本タスクの実行環境も独立した被験セッション（別モデル・別セッション）を起動する権限を
持たない。3 役分離の趣旨に反するため単一セッションでの兼務は行わない）。

## 2. 確定版測定値（REQ-12・NFR-8）

`docs/reports/task-12-7-metrics.summary`（確定値台帳）に記録し、
`scripts/accept/ai-autonomy-accept.sh` の基準 A〜C・E がこれを機械突合する。

| 指標 | REQ-12/NFR-8 の閾値 | 確定値 | 出典 |
|---|---|---|---|
| 自律完遂率 | 60% 以上 | **8/10（80%）** | `task-12-4-1-completion-rate-verification.md`（実測 2026-07-18、起点 `ddc348e`） |
| リグレッション | 0 件 | **0 件**（一次機械ゲート PASS 10/10、独立 target クリーンビルド再実行済み） | 同上 |
| 可否判定正解率（4 値厳密一致） | 80% 以上 | **8/10（80%）** | `task-12-4-2-feasibility-judgment-verification.md`（実測 2026-07-18、起点 `ddc348e`、判定記録 `task-12-4-2-records/`） |
| 誤判定による破壊 | 0 件 | **0 件**（計測対象 6 件） | 同上 |
| エスカレーション時の判断根拠提示 | 80% 以上 | **6/6（100%）** | 同上 5 節 |
| NFR-8（自動修正でテストが通る修正を得られる割合） | 70% 以上 | **8/10（80%、最終判定ベース）**。一次機械ゲートのみは 10/10（100%、参考値） | `task-12-4-1-completion-rate-verification.md`（T-06/T-08 は無変更提出のため fail-closed で「修正を得られた」に含めない） |

NFR-8 の算定方針: 一次（機械ゲート）は 10/10（100%）だが、T-06・T-08 は被験 AI が
「対象は起点コミットで実装済み」と判断し worktree を無変更のまま提出したケースであり、
「自動修正でテストが通る修正を得られた」とは言えない。保守的に最終判定（二次判定込み）
ベースの 8/10（80%）を採用する。80% ≥ 70% で NFR-8 を充足する。

## 3. PENDING 事項と実施手順（未実施を偽装しない）

### 3.1 TASK-12.5 試行 2・3（v2 タスクセットによる複数回試行）

- **状態**: PENDING。`docs/reports/task-12-5-task-definitions.md`（v2 タスクセット、
  T-06/T-08/J-02/J-03 相当を差し替え済み）・`scripts/third-party-stability-aggregate.sh`
  （集計ハーネス）は完備。
- **実施手順**: 独立サブエージェントセッション起動権限を持つ実行主体が、
  `docs/design/multi-trial-stability-verification.md` の 3 役分離プロトコルに従い
  試行 2・3 を実施し、各試行のトライアルサマリファイル（`trial-<label>.summary`）を
  `docs/reports/` 配下へ配置する。配置後は以下で再検証する:

  ```bash
  bash scripts/third-party-stability-aggregate.sh --trials-dir docs/reports
  ```

  `ai-autonomy-accept.sh` の基準 F は `docs/reports/trial-*.summary` の存在を自動検知し、
  存在すれば本コマンドを自動的に呼び出して結果を突合する。

### 3.2 TASK-12.6 グレーゾーン実測（G-01〜G-10）

- **状態**: PENDING。`docs/reports/task-12-6-task-definitions.md`（G-01〜G-10 タスク
  セット）・`scripts/third-party-feasibility-verify.sh`（4 値→条件付き可を含む採点への
  拡張済み）は完備。
- **実施手順**: 独立サブエージェントセッションでタスクごとに判定記録を取得し
  `docs/reports/task-12-6-records/<TASK_ID>.md` へ配置後、以下で再検証する:

  ```bash
  bash scripts/third-party-feasibility-verify.sh \
    --task-definitions docs/reports/task-12-6-task-definitions.md \
    --records-dir docs/reports/task-12-6-records \
    --task-ids "G-01 G-02 G-03 G-04 G-05 G-06 G-07 G-08 G-09 G-10"
  ```

  `ai-autonomy-accept.sh` の基準 F は `docs/reports/task-12-6-records/` の存在を自動検知
  し、存在すれば本コマンドを自動的に呼び出して結果を突合する。

### 3.3 自動監査タスクの妥当性（人手評価、基準 D-2）

- **状態**: PENDING（未記入）。機械的に検証できる範囲（`audit-triage.sh` が影響範囲・
  対応方針欄を生成すること）は基準 D-1 で PASS 済み。人手評価（実際のトリアージ出力・
  改善提案の内容が妥当か）は人間レビュアーの判断を要する。

## 人手評価台帳

人間レビュアーが `scripts/audit-triage.sh`（または実際に起票された改善提案 Issue）の
出力を確認し、`docs/design/improvement-proposal-flow.md` 4 節の必須記載項目（背景・
根拠データ／影響範囲／対応方針／検証方法／リスク）に照らして妥当性評価を記入する。
全行が「妥当」または「不当」で埋まり `ai-autonomy-accept.sh` を再実行すると、
基準 D-2 が自動的に集計される（現状は PENDING のため SKIP）。

| トリアージ対象・改善提案 | 妥当性評価 | 評価者 | 評価日 |
|---|---|---|---|
| （記入例）`scripts/audit-triage.sh` fixture 実行結果 | PENDING | - | - |
| （記入例）実運用 `dep-audit` ジョブが起票した改善提案 Issue（該当があれば） | PENDING | - | - |

## 4. 検証コマンド・再現手順

```bash
bash scripts/accept/ai-autonomy-accept.sh
```

セルフテスト（判定ロジック単体の回帰確認、cargo・ネットワーク非依存）:

```bash
bash scripts/tests/run-ai-autonomy-accept-tests.sh
```

既存ハーネスの非破壊確認（本タスクは既存スクリプトを変更していないことの回帰確認）:

```bash
bash scripts/tests/run-third-party-verify-tests.sh
bash scripts/tests/run-third-party-feasibility-tests.sh
bash scripts/tests/run-third-party-stability-tests.sh
bash scripts/tests/run-triage-tests.sh
```

## 5. 実行結果（実行ログ）

```
[PASS] A: 自律完遂率 ≥60% かつリグレッション 0 件: 台帳: 8/10（80%）・リグレッション 0 件。出典: docs/reports/task-12-4-1-completion-rate-verification.md（実測 2026-07-18、起点 ddc348e）
[PASS] B: 可否判定正解率 ≥80% かつ誤判定破壊 0 件: 台帳: 8/10（80%）・破壊 0 件。出典: docs/reports/task-12-4-2-feasibility-judgment-verification.md（実測 2026-07-18、起点 ddc348e）; 再採点（scripts/third-party-feasibility-verify.sh）と台帳値が一致（8/10（80%））
[PASS] C: エスカレーション時の判断根拠提示 ≥80%: 台帳: 6/6（100%）。出典: docs/reports/task-12-4-2-feasibility-judgment-verification.md 5 節
[PASS] D-1: 自動監査タスクの影響範囲・対応方針欄の機械生成: scripts/audit-triage.sh が fixture 実行で影響範囲（crate 列）・対応方針（推奨アクション）の両欄を生成することを確認（docs/design/improvement-proposal-flow.md 4 節）
[SKIP] D-2: 自動監査タスクの妥当性判断（人手評価台帳）: 評価表に PENDING 行が残っています（全件記入まで SKIP、PASS と偽らない）
[PASS] E: NFR-8 自動修正でテストが通る修正を得られる割合 ≥70%: 台帳: 8/10（80%、最終判定ベース）。一次機械ゲートのみは 10/10（100%、参考値）
[SKIP] F: 複数回試行の安定性・グレーゾーン再検証: 試行サマリ（docs/reports/trial-*.summary）・グレーゾーン判定記録（docs/reports/task-12-6-records）とも未実施（TASK-12.5 試行 2・3／TASK-12.6 は PENDING）。実施手順: docs/design/multi-trial-stability-verification.md・docs/design/gray-zone-feasibility-verification.md の 3 役分離プロトコルに従い被験セッションを起動し、集計・採点ハーネスを再実行する

=== 受け入れ検証サマリー（REQ-12/NFR-8、TASK-12.7 / #48） ===
判定 | 基準                                   | 詳細
-------+------------------------------------------+-----------------------------------------
PASS   | A: 自律完遂率 ≥60% かつリグレッション 0 件 | 台帳: 8/10（80%）・リグレッション 0 件
PASS   | B: 可否判定正解率 ≥80% かつ誤判定破壊 0 件 | 台帳: 8/10（80%）・破壊 0 件・再採点一致
PASS   | C: エスカレーション時の判断根拠提示 ≥80% | 台帳: 6/6（100%）
PASS   | D-1: 自動監査タスクの影響範囲・対応方針欄の機械生成 | fixture 実行で両欄の生成を確認
SKIP   | D-2: 自動監査タスクの妥当性判断（人手評価台帳） | 未記入（PENDING）
PASS   | E: NFR-8 自動修正でテストが通る修正を得られる割合 ≥70% | 台帳: 8/10（80%）
SKIP   | F: 複数回試行の安定性・グレーゾーン再検証 | 試行 2・3／グレーゾーン実測とも未実施

結果: FAIL なし（PASS / SKIP / WARN のみ）。
```

終了コード 0（FAIL 0 件）。セルフテスト（`scripts/tests/run-ai-autonomy-accept-tests.sh`）
は 28 assertion 全件 PASS。既存ハーネスのセルフテスト
（`run-third-party-verify-tests.sh`・`run-third-party-feasibility-tests.sh`・
`run-third-party-stability-tests.sh`・`run-triage-tests.sh`）も全件 PASS で非破壊を確認した
（本タスクはこれらの既存スクリプトを変更していない）。

## 6. Issue #48 受け入れ条件との対応

| Issue #48 の受け入れ条件 | 判定 |
|---|---|
| 1. 自律完遂率 60% 以上・可否判定正解率 80% 以上を確定 | **充足**（2 節、基準 A・B が PASS） |
| 2. TASK-12.7 受け入れ基準（リグレッション 0・誤判定破壊 0・エスカレーション根拠提示 80% 以上・自動監査妥当性 80% 以上）を満たす | リグレッション・誤判定破壊・根拠提示は**充足**（基準 A・B・C が PASS）。自動監査妥当性は機械検証部分（D-1）は充足、人手評価（D-2）は **PENDING**（3.3 節、SKIP） |
| 3. NFR-8（自動修正でテストが通る修正 70% 以上）の確認 | **充足**（基準 E が PASS、8/10=80%） |

条件 2 の自動監査妥当性のうち人手評価部分のみ PENDING が残る。機械検証可能な範囲は
すべて PASS しており、PENDING の性質・実施手順は 3.3 節・人手評価台帳に明記済みである。

## 7. 既知の限界

- 被験 AI は Claude ファミリー（`claude-sonnet-5`）に限られ、別ベンダー LLM・人間の
  被験者による追加実施は本タスクのスコープ外（TASK-12.4-1／TASK-12.4-2／TASK-12.5／
  TASK-12.6 と同一の既知の限界）。
- 被験 worktree からの正解ラベル・レポート隔離はファイル削除＋指示による運用であり、
  サンドボックスによる強制ではない（同上）。
- 可否判定正解率の不一致 2 件（J-02・J-03）はタスク設計側の前提誤りの側面があり、
  完遂率の FAIL 2 件（T-06・T-08）も同種の要因による（`task-12-4-1-completion-rate-verification.md`・
  `task-12-4-2-feasibility-judgment-verification.md` の考察節参照）。今回の確定値は
  これらを fail-closed のまま集計した保守的な値である。

## 8. 対象外（out-of-scope、参考記録）

自動運転モードのため新規 Issue 起票は行わないが、実装過程で気付いたスコープ外事項を
記録する（PR 本文への転記候補、`.claude/rules/out-of-scope-tracking.md`）。

- TASK-12.5 試行 2・3（v2 タスクセットによる複数回試行の実測）: 独立セッション起動権限を
  持つ実行主体への引き継ぎ事項（3.1 節）。
- TASK-12.6 グレーゾーン実測（G-01〜G-10 の判定記録取得・採点）: 同上（3.2 節）。
- 自動監査タスクの人手評価（人手評価台帳の記入・サインオフ）: 人間レビュアーへの
  引き継ぎ事項（3.3 節）。
- 可否判定正解率の不一致 2 件（J-02・J-03）・完遂率 FAIL 2 件（T-06・T-08）に見られる
  タスク設計時の前提確認不足は、今後のタスクセット設計手法の改善候補として
  `docs/reports/task-12-5-stability-verification.md` に記録済み（本タスクでの追加対応は
  行わない）。

## 承認欄

| 役割 | 氏名 | 日付 | 承認 |
|------|------|------|------|
| 実装セッション（本タスク） | Claude Code サブエージェント（自動運転モード） | 2026-07-18 | 機械検証範囲（A・B・C・D-1・E）確定 |
| 人手評価台帳（D-2）記入者 | （記入予定） | | PENDING |
| TASK-12.5 試行 2・3 実施者 | （記入予定） | | PENDING |
| TASK-12.6 グレーゾーン実測実施者 | （記入予定） | | PENDING |
| レビュー承認者 | （記入予定） | | PENDING |

## 関連ドキュメント

- 受け入れ基準対応表: [`docs/acceptance/req12-ai-autonomy.md`](../acceptance/req12-ai-autonomy.md)
- 確定値台帳（機械可読）: `docs/reports/task-12-7-metrics.summary`
- 受け入れスクリプト: `scripts/accept/ai-autonomy-accept.sh`
- セルフテスト: `scripts/tests/run-ai-autonomy-accept-tests.sh`
- 完遂率再検証: [`task-12-4-1-completion-rate-verification.md`](./task-12-4-1-completion-rate-verification.md)
- 可否判定正解率再検証: [`task-12-4-2-feasibility-judgment-verification.md`](./task-12-4-2-feasibility-judgment-verification.md)
- 複数回試行安定性確認: [`task-12-5-stability-verification.md`](./task-12-5-stability-verification.md)
- グレーゾーン再検証: [`task-12-6-gray-zone-verification.md`](./task-12-6-gray-zone-verification.md)
- 改善提案フロー: [`docs/design/improvement-proposal-flow.md`](../design/improvement-proposal-flow.md)
- 対応タスク: `docs/spec/05-tasks.md` TASK-12.7
- 根拠要件: `docs/spec/04-requirements.md` REQ-12・NFR-8
