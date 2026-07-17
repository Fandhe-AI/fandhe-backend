# 追加機能改修フロー — ドキュメント追随・完遂判定

TASK-12.2-2（#82、REQ-12(b)）の成果物。REQ-12(b)（`docs/spec/04-requirements.md`）は
「受け取った機能要求を、AI が実装・テスト追加・**ドキュメント更新まで一貫して**改修する」
フローを要求する。親タスク TASK-12.2（#43、`docs/spec/05-tasks.md`）は 2 サブタスクに
分解されている。

- **TASK-12.2-1（#81）**: 機能要求 → 実装 → テストの一貫改修フローの整備
- **TASK-12.2-2（本書、#82）**: フローの残り 2 要素 —— **(1) ドキュメント追随**（変更に
  応じて更新すべきドキュメントの特定と更新をフローの必須ステップとして組み込む）と
  **(2) 完遂判定**（何をもって改修「完遂」とみなすかの判定基準をフローの終端ゲートとして
  組み込む）

本書執筆時点（origin/main 6f42352）で TASK-12.2-1（#81）は未マージのため、本書は
(1)・(2) の範囲を自己完結で記述する。#81 がマージされ次第、両者を統合した単一の
改修フロードキュメントに再編する（本書の (1)・(2) 節はそのまま引き継ぐ想定）。

## 1. 責務分界

- 機能要求 → 実装 → テスト追加の基幹フロー整備は TASK-12.2-1（#81）のスコープ。
- 対応可否ガードレール（可 / 不可 / 要エスカレーション判定）は TASK-12.3（#44）のスコープ。
- レビューゲート（人間承認）の運用定義は TASK-14.3（#41、未了）のスコープ。
- 本書は「変更に対してどのドキュメントを追随させるか」と「何をもって完遂とみなすか」の
  **フロー定義**に責務を限定する。個々の判定ロジック・レビュー運用の詳細には立ち入らない。

既存の関連ドキュメントとの関係:

- CI 完遂判定の機械部分（`ci-complete` 集約ゲート）は TASK-14.1（#39）の
  [`ci-completion-criteria.md`](./ci-completion-criteria.md) が実装済み。本書はこれを
  **改修フローの完遂判定として参照・組み込む**のであって、再実装しない。
- ドキュメント追随のうち doc comment・doc test は TASK-11.2（#75/#76）で
  `missing_docs` + rustdoc lint + doc test として既に機械強制されている（CI `doc` /
  `test` ジョブ）。本書はこの既存の機械強制を A 節の表で参照する。
- `docs/design/<flow>.md` + `.claude/rules/<flow>.md` + CLAUDE.md Rules 表更新という
  ドキュメント構成パターンは TASK-12.1-2（#80）の
  [`improvement-proposal-flow.md`](./improvement-proposal-flow.md) /
  [`.claude/rules/improvement-proposal.md`](../../.claude/rules/improvement-proposal.md) の
  体裁を踏襲する。

## 2. 改修フロー全体像

```mermaid
flowchart LR
    A[機能要求] --> B[可否判断]
    B --> C[実装]
    C --> D[テスト追加]
    D --> E[ドキュメント追随]
    E --> F[セルフレビュー]
    F --> G[PR]
    G --> H[完遂判定]
    H --> I[マージ・クローズ]
```

| 段階 | 内容 | 担い手 |
|------|------|--------|
| 機能要求 | 要求を受け取る | 人間 / 上流エージェント |
| 可否判断 | 実施可能・安全・影響範囲が許容内かを判定。判定規約は [`feasibility-guardrail.md`](./feasibility-guardrail.md)（TASK-12.3-1、#83）、判定記録の機械検証は `scripts/feasibility-check.sh`（TASK-12.3-2、#84） | AI エージェント |
| 実装 | 要求を実装（TASK-12.2-1 のスコープ） | AI エージェント（`core-builder` / `plugin-builder` 等） |
| テスト追加 | 実装に対応するテストを追加（TASK-12.2-1 のスコープ） | AI エージェント（`test-runner`） |
| **ドキュメント追随** | 変更種別に応じたドキュメントを更新（本書 3 節） | AI エージェント |
| セルフレビュー | 品質・アーキテクチャ準拠・セキュリティ監査（`delegation-impl.md` の実装後標準フロー） | `reviewer` / `security-auditor` |
| PR | Conventional Commits 準拠でコミット・PR 作成 | AI エージェント |
| **完遂判定** | `ci-complete` + レビューゲートの 2 条件充足を確認（本書 4 節） | 自動 CI + 人間レビュアー |
| マージ・クローズ | 完遂判定通過後にマージ | 人間（マージ操作） |

改修フローの**自動マージは行わない**（フェイルクローズ）。実装は必ず CI 通過とレビュー
ゲートを経る（[`improvement-proposal-flow.md`](./improvement-proposal-flow.md) 2 節と
同一原則）。

## 3. ドキュメント追随（変更種別 → 追随ドキュメントのマッピング）

変更を「完遂」とみなすには、コード変更だけでなく対応するドキュメントの追随が必須である
（4 節の完遂判定の条件 (3)）。変更種別ごとに追随すべきドキュメントと、その追随を担保する
手段（機械強制か運用チェックリストか）を以下に整理する。

| 変更種別 | 追随すべきドキュメント | 強制手段 |
|---------|----------------------|---------|
| 公開 API 追加・変更 | doc comment + doc test（`# Examples`。[`coding-rust.md`](../../.claude/rules/coding-rust.md)・[`code-comment-style.md`](../../.claude/rules/code-comment-style.md) 準拠） | **機械**: `missing_docs` + rustdoc lint（CI `doc` ジョブ）・doc test（CI `test` ジョブ）。TASK-11.2（#75/#76）で実装済み |
| エンドポイント・拡張点追加 | AGENTS.md（TASK-11.3、#35、**未作成**）。未作成の間は CLAUDE.md（Repository Structure・Rules 表）または該当する `docs/design/*.md` に記録する代替運用とする | 運用: セルフレビュー（`reviewer`）のチェック項目 |
| クレート・feature 構成変更 | CLAUDE.md の Repository Structure 節・ルート README・該当する `docs/design/*.md` | 運用: セルフレビューのチェック項目 |
| 依存の追加・更新 | `docs/dep-impact/records.md`（`scripts/dep-impact.sh` による計測） | 機械補助（計測スクリプト）+ 運用（記録の要否判断） |
| 運用フロー・規約変更 | `.claude/rules/` 該当規約 + CLAUDE.md の Rules 表 | 運用: セルフレビューのチェック項目 |

- 機械強制できる部分（doc comment / doc test）は既に `ci-complete` の判定対象（CI `doc` /
  `test` ジョブ）に含まれており、本書による追加実装は不要である
  （[`ci-completion-criteria.md`](./ci-completion-criteria.md) の機械判定/人間判断の
  分界表と同じ流儀）。
- 上表のいずれにも該当しない変更種別に遭遇した場合、どのドキュメントに追随させるべきか
  不明な状態で実装を完遂とみなさない。判断がつかない場合は安全側に倒し、少なくとも
  CLAUDE.md か対応する `docs/design/*.md` への記録を検討したうえで、要判断事項として
  レビューで提示する。
- **AGENTS.md 未作成期間の扱い**: TASK-11.3（#35）は本書のスコープ外であり、本書は
  AGENTS.md の作成そのものを要求しない。AGENTS.md が存在しない間、エンドポイント・拡張点
  追加の追随先は上表の代替運用（CLAUDE.md / `docs/design/`）とし、AGENTS.md 作成後は
  当該項目をそちらへ移行する。
- ドキュメント追随が漏れた変更は完遂と扱わない（4 節へ接続）。

## 4. 完遂判定のフロー組み込み

### 4.1 完遂の定義

REQ-14（`docs/spec/04-requirements.md`）を正とし、本書では再定義しない。改修の完遂は
次の 3 条件すべての充足をもって判定する。

1. **`ci-complete` 緑**: CI 集約ゲート `ci-complete` が成功していること。
   [`ci-completion-criteria.md`](./ci-completion-criteria.md) が定義する fail-closed 集約
   （`success` 以外は一律「未完遂」）に従う。本書執筆時点（`.github/workflows/ci.yml`）で
   `ci-complete` の判定対象ジョブは `fmt` / `clippy` / `test` / `doc` / `dep-audit` /
   `coverage` / `unsafe-triage` の 7 ジョブである（`ci-completion-criteria.md` 執筆時点の
   5 ジョブから `coverage`（TASK-11.5-2、#113）・`unsafe-triage`（TASK-12.1-1、#79）が
   追加されている。ジョブ追加・改名時は `ci-completion-criteria.md` の「ジョブ追加・改名
   時の運用」節に従い両ドキュメントを同期する）。
2. **機能要求の受け入れ基準充足**: 人間判断（レビューゲート、TASK-14.3、#41、**未了**）。
   本書は判定基準の定義とフロー上の位置付けのみを記述し、レビュー運用の詳細確立は
   TASK-14.3 のスコープとする。
3. **ドキュメント追随の完了**: 本書 3 節のマッピングに従い、変更種別に対応するドキュメント
   更新が漏れなく行われていること。

### 4.2 フロー上の位置

完遂判定は 2 節の改修フローの終端ゲートであり、PR 作成後・マージ前に評価する。

```mermaid
flowchart LR
    G[PR] --> H1{ci-complete 緑?}
    H1 -->|No| X1[未完遂・修正して再実行]
    H1 -->|Yes| H2{受け入れ基準充足?<br/>レビューゲート}
    H2 -->|No| X2[未完遂・要修正 or エスカレーション]
    H2 -->|Yes| H3{ドキュメント追随完了?}
    H3 -->|No| X3[未完遂・3節マッピングに従い追随]
    H3 -->|Yes| I[完遂・マージ・クローズ]
```

3 条件はすべて必須であり、優先順位や部分達成での完遂扱いはない。

### 4.3 未完遂時の扱い（fail-closed）

- 判定不通過のままマージしない。`ci-complete` が赤の場合、レビューゲートが未承認の場合、
  ドキュメント追随が漏れている場合のいずれも「未完遂」として扱い、自動マージは行わない
  （[`improvement-proposal-flow.md`](./improvement-proposal-flow.md) 2 節と同一原則）。
- 部分完遂・スコープ外の残課題は
  [`out-of-scope-tracking.md`](../../.claude/rules/out-of-scope-tracking.md) に従い
  Issue 化して切り出す。現在の改修に混入させない。
- 未完遂・部分完遂の状態を握りつぶさない。深刻な問題は
  [`security.md`](../../.claude/rules/security.md) の原則と同様に、main エージェントから
  ユーザーへ明確に報告する。

## 5. セキュリティ考慮事項（OWASP Top 10 観点）

- **A01 / A04（アクセス制御・安全でない設計）**: 完遂判定は fail-closed を維持する。
  自動マージ・ゲート迂回を認める記述は含まない。required status check（`ci-complete`）や
  レビューゲートを弱める設定変更は本書のスコープ外であり、行わない。
- **A03（インジェクション）**: 本書中のコマンド例は外部由来文字列をシェル再解釈させない
  形（変数のクォート等）で記載する（[`improvement-proposal-flow.md`](./improvement-proposal-flow.md)
  と同一方針）。
- **A02 / シークレット**: ドキュメント・規約・コミットにトークン・鍵・PII を含めない
  （[`security.md`](../../.claude/rules/security.md) 準拠）。CI 権限の拡大
  （`permissions` 追加）は行わない。
- **A05（設定ミス）**: CI ワークフロー・ruleset・hooks には触れない（本書はドキュメントの
  みの変更）。`ci-complete` の `needs` 等の変更が必要な場合は別タスクとして扱う。
- **A06（脆弱な依存）**: 依存の追加・更新時は `docs/dep-impact/records.md` への記録を
  ドキュメント追随の必須項目とすることで（3 節）、依存監査（REQ-15）の運用を強化する。
- **握りつぶし禁止**: 未完遂・部分完遂の扱いは
  [`out-of-scope-tracking.md`](../../.claude/rules/out-of-scope-tracking.md) 経由の
  Issue 化と main エージェントからのユーザー報告を必須とする（4.3 節）。

## 関連ドキュメント

- 運用規約（エージェント向け要約）: [`.claude/rules/feature-modification.md`](../../.claude/rules/feature-modification.md)
- CI 完遂判定基準: [`ci-completion-criteria.md`](./ci-completion-criteria.md)
- 改善提案フロー: [`improvement-proposal-flow.md`](./improvement-proposal-flow.md)
- 依存インパクト計測: [`docs/dep-impact/README.md`](../dep-impact/README.md)
- スコープ外課題の追跡規約: [`.claude/rules/out-of-scope-tracking.md`](../../.claude/rules/out-of-scope-tracking.md)
- セキュリティ規約: [`.claude/rules/security.md`](../../.claude/rules/security.md)
