# TASK-12.5 複数回試行による結果安定性確認レポート

TASK-12.5（#46、`docs/spec/05-tasks.md` 366〜371 行目、Conditional Go 条件 (3)）の
結果記録。プロトコルは
[`multi-trial-stability-verification.md`](../design/multi-trial-stability-verification.md)、
v2 タスクセットは [`task-12-5-task-definitions.md`](./task-12-5-task-definitions.md)、
集計ハーネスは [`third-party-stability-aggregate.sh`](../../scripts/third-party-stability-aggregate.sh)
を参照。

## 実施環境

| 項目 | 値 |
|------|-----|
| 起点コミット（origin/main、本タスク着手時点） | `0cdc728`（`test(global): TASK-12.4 第三者検証の実測定を実施し結果を記録 #167`） |
| v2 タスク定義コミット | 本レポートと同一コミットで `task-12-5-task-definitions.md` を先行確定する |
| 実施エージェント（本タスク #46 の実装セッション） | Claude Code サブエージェント（自動運転モード、隔離 worktree） |
| **本レポート作成日** | **2026-07-18** |
| **試行数** | 試行 1（v1、TASK-12.4-1／TASK-12.4-2 実測値の転記）+ 試行 2・試行 3（v2、本タスクで実施予定） |

## 1. 試行 1（v1）: TASK-12.4-1／TASK-12.4-2 実測値の転記

以下は [`task-12-4-1-completion-rate-verification.md`](./task-12-4-1-completion-rate-verification.md)・
[`task-12-4-2-feasibility-judgment-verification.md`](./task-12-4-2-feasibility-judgment-verification.md)
（いずれも 2026-07-18 実測、PR #167）からの転記であり、本タスクで新たに測定した値では
ない。転記元の実測環境: 起点コミット `ddc348e`、被験 AI 計 20 セッション（完遂率 10 +
可否判定 10、いずれも model: claude-sonnet-5）、独立 target ディレクトリでの再実行済み。

| 指標 | REQ-12 の閾値 | 試行 1（v1）実測値 | 充足 |
|------|--------------|-------------------|------|
| 自律完遂率 | 60% 以上 | **8/10（80%）** | 充足 |
| 可否判定正解率（4 値厳密一致） | 80% 以上 | **8/10（80%）** | 充足（境界値） |
| エスカレーション時の判断根拠提示割合 | 80% 以上 | **6/6（100%）** | 充足 |
| 誤判定による破壊 | 0 件 | **0 件**（計測対象 6 件） | 充足 |

v1 の FAIL・不一致の内訳（試行 2・3 の v2 セットで差し替えた根拠、詳細は各転記元レポート
の考察節を参照）:

- 完遂率 FAIL 2 件: T-06・T-08（いずれも「対象が起点コミットで実装済み」という設計前提
  誤りによる fail-closed 判定。実装欠陥・リグレッションは 0 件）
- 可否判定不一致 2 件: J-02（対象クレートの実状食い違い）・J-03（clippy 警告の前提が
  起点コミットで成立していなかった）

## 2. 試行 2・試行 3（v2）: 実測定の状況

**PENDING（未実施）。** 本タスク（#46）を実行している自動運転サブエージェントの権限・
利用可能ツールには、他の Claude セッション（サブエージェント・別モデル）を新規に起動
する手段が与えられていない。`third-party-verification.md`／`third-party-feasibility-verification.md`
が定義する 3 役分離のうち (B) 被験 AI 役（タスクごとに新規起動する独立セッション）を
本セッションが自ら担うことは、TASK-12.4-1／TASK-12.4-2 初版の未実施理由と同一の制約
（PoC-9 と同型の自己評価バイアスの再生産を避けるため、単一セッションでの兼務を行わない）
により実施していない。

このため、v2 セット（T-01〜T-10・J-01〜J-10、計 20 タスク）に対する試行 2・試行 3 の
実際の被験実装・判定記録取得・機械採点は**未実施**である。これは「結果を偽らない」こと
を最優先した結果であり、TASK-12.4-1／TASK-12.4-2 の前例（`task-12-4-1-completion-rate-verification.md`
「未実施の理由」節・`task-12-4-2-feasibility-judgment-verification.md` 2 節）を踏襲する。

### 実施済み範囲（本タスクで確定した成果物）

| 成果物 | 状態 |
|--------|------|
| 複数回試行の安定性確認プロトコル（K=3・v2 再設計規約・前提事前検証手順・安定性判定基準） | 完了（`docs/design/multi-trial-stability-verification.md`） |
| v2 タスク定義（N=20、T-06/T-08/J-02/J-03 差し替え・前提事前検証記録・被験実行前に確定） | 完了（`docs/reports/task-12-5-task-definitions.md`） |
| 試行横断集計ハーネス `scripts/third-party-stability-aggregate.sh` | 完了・合成 fixture によるセルフテストで動作確認済み（下記） |
| ハーネスのセルフテスト `scripts/tests/run-third-party-stability-tests.sh` | 完了・23 アサーション全件 PASS（正常系・閾値未達検知・破壊検知・不正入力 fail-closed の各経路を検証） |

被験実行に必要な環境（独立セッション・別モデルの起動権限を持つ実行主体）が整い次第、
上記プロトコル・v2 タスク定義・集計ハーネスをそのまま再利用して試行 2・試行 3 を実施
できる状態にある。実施手順の概略:

1. 起点コミット（`0cdc728` の子孫、実施時点の最新 origin/main）を固定し、ベースライン
   テストログを取得する。
2. v2 の T-01〜T-10 を独立サブエージェントセッション×10（1 タスク=1 セッション=1 使い
   捨て worktree、独立 `CARGO_TARGET_DIR`）へ実装させ、`scripts/third-party-verify.sh`
   で判定する。
3. v2 の J-01〜J-10 を独立サブエージェントセッション×10 へ判定させ、
   `scripts/third-party-feasibility-verify.sh` で機械採点する。
4. 各指標を「トライアルサマリファイル」（`scripts/third-party-stability-aggregate.sh`
   の doc コメント参照）へ書き起こし、`--trials-dir` で本レポートの 3 節へ集計結果を
   転記する。

## 3. 安定性の判定基準に対する現況

[`multi-trial-stability-verification.md`](../design/multi-trial-stability-verification.md)
4.2 節の「安定」の定義（全試行で REQ-12 閾値を充足し、かつレンジを記録すること）を
満たすには試行 2・試行 3 の実測値が必要である。試行 1（v1）単独では閾値をすべて充足
しているが、**試行が 1 件のみでは「複数回試行による安定性」を確認したことにはならない**
（TASK-12.5 の要求そのもの）。したがって、本レポート時点での安定性の結論は以下のとおり
未確定である。

| 指標 | 試行数 | min | max | レンジ | 平均 | 安定性の判定 |
|------|-------|-----|-----|--------|------|-------------|
| 自律完遂率 | 1（v1 のみ） | 80% | 80% | 0 pt | 80% | PENDING（試行 2・3 待ち） |
| 可否判定正解率 | 1（v1 のみ） | 80% | 80% | 0 pt | 80% | PENDING（試行 2・3 待ち） |
| 判断根拠提示割合 | 1（v1 のみ） | 100% | 100% | 0 pt | 100% | PENDING（試行 2・3 待ち） |
| 誤判定による破壊 | 1（v1 のみ） | 0 件 | 0 件 | 0 件 | 0 件 | PENDING（試行 2・3 待ち） |

v1/v2 共通タスク（T-01〜T-05・T-07・T-09・T-10 の 8 件、J-01・J-04〜J-10 の 8 件）での
比較表・セット全体値の併記（`multi-trial-stability-verification.md` 3.4 節）は、試行 2・
3 の実測値が得られ次第この節へ追記する。

**確定は TASK-12.7（#48、確定値の受け入れテスト）のスコープである**（`multi-trial-stability-verification.md`
4.2 節）。本レポートは「観測された事実」の記録に留め、「安定」「不安定」の断定は行わない。

## 4. 既知の限界

- 被験 AI は（試行 2・3 を実施する場合も）Claude ファミリーに限られる想定であり、別
  ベンダー LLM・人間の被験者による追加実施は本タスクのスコープ外（TASK-12.4-1／
  TASK-12.4-2 と同一の既知の限界）。
- 被験 worktree からの正解ラベル・レポート隔離はファイル削除＋指示による運用であり、
  サンドボックスによる強制ではない。
- v2 セットは v1 の 4 件（T-06/T-08/J-02/J-03）を差し替えたため、v1/v2 は完全に同一の
  タスクセットではない（3 節参照）。

## 5. 対象外（out-of-scope、参考記録）

自動運転モードのため新規 Issue 起票は行わないが、実装過程で気付いたスコープ外事項を
記録する（次回 PR 本文への転記候補）。

- グレーゾーンタスクの再検証（曖昧要求と明確要求の境界事例）は TASK-12.6（#47）の
  スコープであり、本タスクでは扱わない。
- AGENTS.md（#35）は「AI エージェント向け変更ガイド」節まで整備済みだが、本体の完全性は
  本タスクの検証対象外。

## 承認欄

| 項目 | 状態 |
|------|------|
| 人間レビューによる本レポート・v2 タスク定義・集計ハーネスの承認 | PENDING（自動運転のため PR 経由でサインオフを依頼する） |
| 試行 2・試行 3 の実測定の実施・数値確定 | PENDING（独立セッション起動権限を持つ実行主体への引き継ぎ事項） |
| 安定性の最終判定 | PENDING（TASK-12.7／#48 のスコープ） |

## 関連ドキュメント

- プロトコル: [`multi-trial-stability-verification.md`](../design/multi-trial-stability-verification.md)
- v2 タスク定義: [`task-12-5-task-definitions.md`](./task-12-5-task-definitions.md)
- 集計ハーネス: `scripts/third-party-stability-aggregate.sh`
- 集計ハーネスセルフテスト: `scripts/tests/run-third-party-stability-tests.sh`
- 試行 1 転記元（完遂率）: [`task-12-4-1-completion-rate-verification.md`](./task-12-4-1-completion-rate-verification.md)
- 試行 1 転記元（可否判定正解率）: [`task-12-4-2-feasibility-judgment-verification.md`](./task-12-4-2-feasibility-judgment-verification.md)
- 対応タスク: `docs/spec/05-tasks.md` TASK-12.5
- 根拠要件: `docs/spec/04-requirements.md` REQ-12
