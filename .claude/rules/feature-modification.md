# 追加機能改修フロー 運用規約（ドキュメント追随・完遂判定）

TASK-12.2-2（#82、REQ-12(b)）対応。機能要求を実装・テスト追加・ドキュメント更新まで
一貫して改修するフローのうち、**ドキュメント追随**と**完遂判定**についてエージェントが
従う規約。詳細フロー・責務分界は
[[feature-modification-flow]]（`docs/design/feature-modification-flow.md`）を参照。
機能要求 → 実装 → テストの基幹フロー整備は TASK-12.2-1（#81）を参照（本規約はその一部を
構成する）。

## ドキュメント追随チェックリスト

変更種別ごとに以下の追随先を確認する（欠く場合は完遂と扱わない）。

| 変更種別 | 追随先 | 強制手段 |
|---------|--------|---------|
| 公開 API 追加・変更 | doc comment + doc test | 機械（CI `doc` / `test` ジョブ） |
| エンドポイント・拡張点追加 | AGENTS.md（未作成の間は CLAUDE.md / `docs/design/`） | 運用（セルフレビュー） |
| クレート・feature 構成変更 | CLAUDE.md Repository Structure・README・`docs/design/` | 運用（セルフレビュー） |
| 依存の追加・更新 | `docs/dep-impact/records.md` | 機械補助（`scripts/dep-impact.sh`）+ 運用 |
| 運用フロー・規約変更 | `.claude/rules/` 該当規約 + CLAUDE.md Rules 表 | 運用（セルフレビュー） |

- 公開 API の doc comment・doc test は [[coding-rust]] / [[code-comment-style]] の既存
  規約に従って書けば、機械強制（`missing_docs` + rustdoc lint + doc test）により
  `ci-complete` の判定対象に自動的に含まれる。追加作業は不要
- 該当種別が上表にない変更は、少なくとも CLAUDE.md か対応する `docs/design/*.md` への
  記録を検討し、判断がつかない場合はレビューで要判断事項として提示する

## 完遂判定の 3 条件

改修の完遂は次の 3 条件すべての充足で判定する（REQ-14 を正とし再定義しない）。

1. **`ci-complete` 緑**: CI 集約ゲートが成功していること（[[ci-completion-criteria]]、
   判定対象ジョブは `.github/workflows/ci.yml` の実ジョブ構成に従う）
2. **受け入れ基準充足**: 人間判断によるレビューゲート（TASK-14.3、#41、未了）
3. **ドキュメント追随完了**: 本書チェックリストに従った更新が漏れなく行われていること

3 条件はすべて必須。優先順位や部分達成での完遂扱いはない。

## 未完遂時の扱い（fail-closed）

- 3 条件のいずれかが未充足のまま自動マージしない
- 部分完遂・スコープ外の残課題は [[out-of-scope-tracking]] に従い Issue 化して切り出す
- 未完遂・部分完遂を握りつぶさず、深刻な問題は main エージェントからユーザーへ報告する
  （[[security]] と同一原則）

## 実装・レビューフロー

- 実装・テスト追加後、PR 作成前に本書チェックリストでドキュメント追随を確認する
- 委譲マッピングは [[delegation-impl]] に従う（`reviewer` / `security-auditor` によるセルフ
  レビューを経てから完遂判定に進む）

## 参照

- 詳細フロー・責務分界: [[feature-modification-flow]]（`docs/design/feature-modification-flow.md`）
- CI 完遂判定基準: [[ci-completion-criteria]]（`docs/design/ci-completion-criteria.md`）
- スコープ外課題の追跡: [[out-of-scope-tracking]]
- セキュリティ規約: [[security]]
- 文体: [[japanese-style]]
