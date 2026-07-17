# 改善提案 運用規約

TASK-12.1-2（#80、REQ-12(a)）対応。AI が能動的に改善提案（コードベース・依存・性能・
脆弱性の分析結果）を提示する際にエージェントが従う規約。詳細フロー・分析軸ごとの入力
ソース対応は [[improvement-proposal-flow]]（`docs/design/improvement-proposal-flow.md`）
を参照。

## 必須記載項目

改善提案（Issue・レポート）には以下を必須記載する（欠く場合は「改善提案」として扱わず
追加調査を要する下書きとする）:

- **背景・根拠データ**: トリアージ出力・ベンチ結果・依存グラフ等、検知に使った一次データ
- **影響範囲**: どのクレート・プラグイン・feature 構成・利用者に影響するか
- **対応方針（推奨アクション）**: 具体的な次アクション
- **検証方法**: 対応後の確認手段（再実行するスクリプト・CI ジョブ名）
- **リスク**: 対応した場合／しなかった場合のリスク

## 起票の 2 レイヤ

- **自動レイヤ（承認不要）**: CI（`dep-audit` ジョブ）による `audit-triage` ラベル Issue
  起票は既存実装の追認であり承認不要。schedule / workflow_dispatch 実行時に限る
- **エージェントレイヤ（承認前提）**: AI が能動分析から新規に改善提案 Issue を起票する
  場合は [[out-of-scope-tracking]] と同一原則でユーザー承認を得る。ラベルは
  `improvement-proposal` を使い、`audit-triage` ラベル（自動レイヤ）と混同しない
- 既存 Issue の有無は `gh issue list --search "<KEYWORD>" --state open` で確認してから
  承認を得る（重複起票防止）

## トリアージ検知への一次対応

- **脆弱性**（`scripts/audit-triage.sh`）: 「自動更新提案」は承認後の PR 上で
  `cargo update -p <crate>` を適用し全 feature 構成で `scripts/dep-audit.sh` 再実行・CI
  通過を条件とする。「要エスカレーション」は代替 crate 検討または `deny.toml` ignore
  追加（理由必須・ユーザー承認必須）。「情報」は記録・監視のみで CI を失敗させない
- **コードベース（unsafe）**（`scripts/unsafe-triage.sh`）: ラチェット検知・`// SAFETY:`
  欠落は CI 失敗として扱い、原因箇所を特定して修正するか、正当な増加であれば
  `--update-baseline` で明示的にベースラインを更新する

## 握りつぶし禁止

- 検知結果（vulnerability・unsafe 増加・性能退行）を確認せずに無視・スキップしない
- フェイルクローズ原則（検知時は CI を非 0 で終了）を維持し、深刻な問題は main エージェント
  からユーザーへ明確に報告する（[[security]] と同一原則）

## 実装・承認ゲート

- 改善提案の**自動適用・自動マージは行わない**。実装は必ず CI 通過（`fmt` / `clippy -D
  warnings` / `test`）とレビューゲートを経る
- 委譲マッピングは [[delegation-impl]] に従う

## 参照

- 詳細フロー: [[improvement-proposal-flow]]（`docs/design/improvement-proposal-flow.md`）
- スコープ外課題の追跡: [[out-of-scope-tracking]]
- セキュリティ規約: [[security]]
- 文体: [[japanese-style]]
