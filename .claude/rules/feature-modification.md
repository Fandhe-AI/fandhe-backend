# 機能改修（機能要求→実装→テスト→ドキュメント追随→完遂判定）運用規約

TASK-12.2-1（#81、REQ-12(b)）+ TASK-12.2-2（#82、REQ-12(b)）対応。外部から受け取った
機能要求を AI が実装・テスト追加・ドキュメント追随・完遂判定まで一貫して改修する際に
エージェントが従う規約。詳細フロー・段階ごとの担い手・責務分界は
[[feature-modification-flow]]（`docs/design/feature-modification-flow.md`）を参照。
改善提案フロー（自動検知起点）の運用規約は [[improvement-proposal]] を参照
（本規約は「外部からの機能要求」起点の対になるフロー）。

## 機能要求の必須記載項目

`.github/ISSUE_TEMPLATE/feature-request.yml`（ラベル `feature-request`）で受け付ける。
必須: 概要・**受け入れ基準**・影響範囲の想定。

## 受け入れ基準なし要求は実装着手しない

- Issue form の必須化は GitHub UI レベルに留まり、API 経由の起票やテンプレート外の
  Issue には強制力が及ばない。実装着手前に受け入れ基準の記載を必ず確認する
- 受け入れ基準を欠く要求は実装に着手せず差し戻す（追加調査を要する下書きとして扱う。
  [[improvement-proposal]] の「必須記載項目を欠く提案は改善提案として扱わない」と同一原則）
- 曖昧要求・危険要求の可否判定（可 / 不可 / 要エスカレーション）の詳細ガードレールは
  TASK-12.3（#83/#84）のスコープ。本規約は「受け入れ基準の有無」という最小条件のみを扱う

## 実装にはテスト追加を伴う

- 実装変更（`crates/<name>/src/**/*.rs`）には同一クレートのテスト追加
  （`crates/<name>/tests/**` の変更、または `#[test]` / `#[tokio::test]` /
  `#[cfg(test)]` / doc test の追加）を伴わせる
- 機械チェックは `scripts/feature-flow-check.sh --base <base-rev>` で行う
  （`scripts/tests/run-feature-flow-tests.sh` がセルフテスト）
- テスト追加を省略する場合は `--allow-no-tests <crate> "<理由>"` で理由を必須明記し、
  レビューで人間が理由の妥当性を確認する前提とする。暗黙スキップは行わない
  （フェイルクローズ、[[security]]）
- 公開 API には doc test を付ける（[[coding-rust]]・[[code-comment-style]]）

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

## 委譲

- 実装は [[delegation-impl]] のパスベース委譲マッピングに従う
  （`crates/core` 等 → `core-builder`、`crates/plugin-*` → `plugin-builder`）
- pay-for-what-you-use（[[pay-for-what-you-use]]）を全実装で遵守する

## 実装・承認ゲート

- 機能改修の**自動適用・自動マージは行わない**。実装は必ず CI 通過（`fmt` / `clippy -D
  warnings` / `test`）とレビューゲートを経る（[[improvement-proposal]] と同一原則）
- 実装・テスト追加後、PR 作成前に本書のドキュメント追随チェックリストで追随を確認する
- 委譲マッピングは [[delegation-impl]] に従う（`reviewer` / `security-auditor` によるセルフ
  レビューを経てから完遂判定に進む）

## 握りつぶし禁止

- `feature-flow-check.sh` の検知結果（テスト追加漏れ）を確認せずに無視・スキップしない
- 深刻な問題は握りつぶさず main エージェントからユーザーへ明確に報告する（[[security]]）

## 参照

- 詳細フロー・責務分界: [[feature-modification-flow]]（`docs/design/feature-modification-flow.md`）
- 対になるフロー（改善提案）: [[improvement-proposal]]
- 委譲マッピング: [[delegation-impl]]
- Rust 規約: [[coding-rust]]
- pay-for-what-you-use: [[pay-for-what-you-use]]
- CI 完遂判定基準: [[ci-completion-criteria]]（`docs/design/ci-completion-criteria.md`）
- スコープ外課題の追跡: [[out-of-scope-tracking]]
- セキュリティ規約: [[security]]
- 文体: [[japanese-style]]
