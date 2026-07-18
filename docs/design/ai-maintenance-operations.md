# AI 保守運用体制（基盤・頻度・範囲・責任分界）

Issue #93 対応。`docs/spec/01-brainstorm.md`「未解決の疑問点」（192 行目）に残置された
「AI 保守を成立させる具体的な体制・ツール: どの AI エージェント基盤で、どの頻度・範囲の
自動保守/監査を回すか（設計思想 4 の運用像）」に応える。REQ-11（AI ファーストな保守性の
構造規約）・REQ-12（AI 自律改修支援機構）で自律改修の**機構**はすでに実装済みであり、
本ドキュメントはそれらを「どの基盤で・どの頻度で・どの範囲に・誰の責任で」回すかという
**運用面の定義**を統合して記述する。新規の機構・規約は追加しない。

## 1. 目的・出典

- 出典: [`docs/spec/01-brainstorm.md`](../spec/01-brainstorm.md) 192 行目（未解決の疑問点）
- 対応要件: REQ-11（AI ファーストな保守性の構造規約）・REQ-12（AI 自律改修支援機構）
- 位置づけ: 本ドキュメントは既存の運用資産（後述 2〜4 節）を「体制」として統合定義する
  ものであり、Rust コード・CI 設定の変更は伴わない（docs 専業タスク）
- `docs/spec/**` は書き換えず、本ドキュメントから相対リンクで参照するに留める

## 2. 基盤（どの AI エージェント基盤で回すか）

| レイヤ | 実体 | 参照 |
|--------|------|------|
| エージェントレイヤ | Claude Code（main + `.claude/agents/` の目的別 sub-agent。`CLAUDE.md` の委譲方針・model 配分に従う） | [`CLAUDE.md`](../../CLAUDE.md) の「委譲方針」「Sub-agents」節 |
| 機械実行レイヤ | GitHub Actions self-hosted runner（Fandhe-AI org レベル登録） + `scripts/` の検知・検証スクリプト群 | [`.claude/rules/ci.md`](../../.claude/rules/ci.md)、`scripts/README.md` |
| 記録・承認レイヤ | GitHub Issues（`audit-triage` / `improvement-proposal` / `feature-request` ラベル）・PR・レビューゲート（ruleset） | [`review-gate.md`](./review-gate.md) |

エージェントレイヤは「検知・トリアージ・提案・実装・テスト追加・セルフレビュー」を担い、
機械実行レイヤは PR/push ごとの `ci-complete` 集約ゲートと日次 schedule によるフェイル
クローズの強制を担う。両レイヤの出力は記録・承認レイヤ（Issue/PR）を経由し、最終判断は
常に人間のレビューゲートに帰属する（後述 5 節）。

## 3. 頻度（どの頻度で回すか）

| 頻度区分 | 対象 | 実体 |
|---------|------|------|
| 日次 | 依存脆弱性監査（dep-audit）。検知時は `audit-triage` ラベル Issue を自動起票 | `.github/workflows/ci.yml` の `schedule: cron: "30 0 * * *"`（00:30 UTC）+ `scripts/audit-triage.sh` |
| 日次 | 外部仕様（spec submodule 等）の更新追随 | `.github/workflows/update-external.yml` の `schedule: cron: '0 0 * * *'`（00:00 UTC） |
| 変更ごと（PR/push） | `ci-complete` 集約ゲート（fmt / clippy / test / unsafe ラチェット / feature 構成ビルド / OpenAPI・TS パイプライン鮮度 等） | `.github/workflows/ci.yml`（`schedule` 以外のイベントで実行される各ジョブ） |
| 要求受領ごと（都度） | 機能要求受付 → 可否判定 → 一貫改修 | [`feature-modification-flow.md`](./feature-modification-flow.md)・[`feasibility-guardrail.md`](./feasibility-guardrail.md) |
| 不定期・マイルストーンごと | 第三者検証・fuzzing 本実行・性能ベンチ再計測 | `docs/acceptance/`・`scripts/third-party-*.sh`・`scripts/fuzz.sh` |

2 つの schedule（ci.yml 00:30 UTC / update-external.yml 00:00 UTC）は
[`.claude/rules/ci.md`](../../.claude/rules/ci.md) の「schedule 系ワークフロー同士は
cron をずらして負荷を分散する」規約に従い、意図的に時刻をずらしてある。

## 4. 範囲（どの範囲を自動保守/監査するか）

### 対象

- 依存監査（`cargo audit` / `cargo deny check`、`scripts/dep-audit.sh`）
- `unsafe` 増分のラチェット検知（`scripts/unsafe-triage.sh`、`scripts/unsafe-baseline.json`）
- 全 feature 構成ビルド・`cargo tree` による依存残留確認（pay-for-what-you-use、
  `scripts/pay-for-what-you-use-check.sh`）
- テスト・カバレッジ（`cargo test` / `cargo llvm-cov`、`scripts/coverage.sh`）
- 依存方向一方向性の検証（`scripts/dep-direction-check.sh`）
- 機能改修時のテスト追加漏れ検知（`scripts/feature-flow-check.sh`）
- OpenAPI/TS 連携パイプラインの鮮度検証（`scripts/openapi-ts.sh` / `scripts/openapi-ts-negative.sh`）
- 対応可否判定記録の形式検証（`scripts/feasibility-check.sh`）

### 対象外（明示）

- **自動マージ**: [`improvement-proposal.md`](../../.claude/rules/improvement-proposal.md)・
  [`feature-modification.md`](../../.claude/rules/feature-modification.md) の両規約が
  明記するとおり、実装は必ず CI 通過とレビューゲートを経て人間が判断する。自動マージは
  行わない
- **`docs/spec/**` の書き換え**: submodule であり、AI エージェント・自動機構のいずれも
  内容を書き換えない（本ドキュメントを含む `docs/design/**` から相対リンクで参照するのみ）
- **受け入れ基準の妥当性判断**: 機械検証は形式・記録の有無を検査するに留まり、
  「その受け入れ基準が正しいか」の判断は人間のレビューゲートが担う

## 5. 責任分界

| 主体 | 責務 |
|------|------|
| AI エージェント（Claude Code） | 検知・トリアージ・改善提案・実装・テスト追加・セルフレビュー・エスカレーション（fail-closed。判断がつかない場合は「可」と判定せず人間判断を仰ぐ） |
| 自動機構（GitHub Actions / scripts） | 機械判定（`ci-complete`）・フェイルクローズの強制（検知時は非 0 終了で CI を失敗させる） |
| 人間（リポジトリ管理者） | 承認ゲート・レビューゲート・受け入れ基準充足判断・エスカレーション対応・自動マージしない運用の最終担保 |
| hub 運用チーム | 本運用定義（基盤・頻度・範囲・責任分界）への合意主体（6 節） |

fail-closed 原則（自動マージ禁止・検知時は CI 非 0 終了・判断不能時は人間判断を仰ぐ）は
[`.claude/rules/security.md`](../../.claude/rules/security.md)・
[`.claude/rules/improvement-proposal.md`](../../.claude/rules/improvement-proposal.md)・
[`.claude/rules/feasibility-guardrail.md`](../../.claude/rules/feasibility-guardrail.md)
の既存方針と整合させ、本ドキュメントはこれを後退させる記述をしない。

## 6. hub 運用チームとの合意状況

Issue #93 の概要は「hub 運用チームと合意のうえ定義する」ことを求めている。本ドキュメントは
実装済みの機構（2〜4 節に記載した既存の scripts / workflows / rules）に基づくドラフト
確定版であり、**合意そのものは本ドキュメントを含む PR のレビューゲート（人間承認）で
記録する**。自動運転モードで合意待ちのままブロックすることは避け、安全側（=「承認ゲートで
担保する」）に倒して定義を進めた。レビュー時に修正要求が生じた場合は本ドキュメントを
追随更新する。

## 7. 将来の乖離防止

- 頻度（cron 値・schedule 対象ジョブ）・範囲（対象スクリプト・ジョブ名）の記述は CI 実体
  （`.github/workflows/ci.yml` / `.github/workflows/update-external.yml`）からの転記で
  あり、CI 実体が変更された場合は本ドキュメントとの乖離が生じ得る。**`.github/workflows/`
  を正**とし、CI 構成を変更した場合は本ドキュメントを追随更新する
- 運用体制の変更を伴う新規自動化（例: 週次 fuzz の schedule 追加）は本ドキュメントの
  スコープ外であり、[`.claude/rules/out-of-scope-tracking.md`](../../.claude/rules/out-of-scope-tracking.md)
  に従い必要なら別 Issue で扱う

## 8. 参照

- [`docs/spec/01-brainstorm.md`](../spec/01-brainstorm.md)（未解決の疑問点の出典）
- [`CLAUDE.md`](../../CLAUDE.md)（委譲方針・Sub-agents・model 配分）
- [`AGENTS.md`](../../AGENTS.md)（AI エージェント向け変更ガイド）
- [`.claude/rules/ci.md`](../../.claude/rules/ci.md)（self-hosted runner・schedule 負荷抑制規約）
- [`.claude/rules/security.md`](../../.claude/rules/security.md)
- [`.claude/rules/improvement-proposal.md`](../../.claude/rules/improvement-proposal.md)
- [`.claude/rules/feature-modification.md`](../../.claude/rules/feature-modification.md)
- [`.claude/rules/feasibility-guardrail.md`](../../.claude/rules/feasibility-guardrail.md)
- [`improvement-proposal-flow.md`](./improvement-proposal-flow.md)
- [`feature-modification-flow.md`](./feature-modification-flow.md)
- [`feasibility-guardrail.md`](./feasibility-guardrail.md)
- [`review-gate.md`](./review-gate.md)
- [`ci-completion-criteria.md`](./ci-completion-criteria.md)
