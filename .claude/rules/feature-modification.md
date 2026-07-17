# 機能改修（機能要求→実装→テスト）運用規約

TASK-12.2-1（#81、REQ-12(b)）対応。外部から受け取った機能要求を AI が実装・テスト追加
まで一貫して改修する際にエージェントが従う規約。詳細フロー・段階ごとの担い手・#82・
TASK-12.3 との境界は [[feature-modification-flow]]（`docs/design/feature-modification-flow.md`）
を参照。改善提案フロー（自動検知起点）の運用規約は [[improvement-proposal]] を参照
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

## 委譲

- 実装は [[delegation-impl]] のパスベース委譲マッピングに従う
  （`crates/core` 等 → `core-builder`、`crates/plugin-*` → `plugin-builder`）
- pay-for-what-you-use（[[pay-for-what-you-use]]）を全実装で遵守する

## 実装・承認ゲート

- 機能改修の**自動適用・自動マージは行わない**。実装は必ず CI 通過（`fmt` / `clippy -D
  warnings` / `test`）とレビューゲートを経る（[[improvement-proposal]] と同一原則）
- ドキュメント追随（CLAUDE.md / doc comment 更新の機械確認）・完遂判定への組み込みは
  #82 のスコープ。本規約の対象外

## 握りつぶし禁止

- `feature-flow-check.sh` の検知結果（テスト追加漏れ）を確認せずに無視・スキップしない
- 深刻な問題は握りつぶさず main エージェントからユーザーへ明確に報告する（[[security]]）

## 参照

- 詳細フロー: [[feature-modification-flow]]（`docs/design/feature-modification-flow.md`）
- 対になるフロー（改善提案）: [[improvement-proposal]]
- 委譲マッピング: [[delegation-impl]]
- Rust 規約: [[coding-rust]]
- pay-for-what-you-use: [[pay-for-what-you-use]]
- スコープ外課題の追跡: [[out-of-scope-tracking]]
- セキュリティ規約: [[security]]
- 文体: [[japanese-style]]
