# CLAUDE.md

## Overview

**backend-framework** は、AI によるセキュリティ脆弱性発見リスクに備え、Rust で新規構築する
軽量・高速・高並行なバックエンドフレームワーク（名称は仮称）。axum 級の性能を目標に、
**最小コア + Cargo feature 駆動プラグイン** 設計で、WebSocket / GraphQL / WebRTC /
OpenAPI 自動生成 / hub 配線 / 可観測性を段階的に拡張できる。

核となる 2 原則:
- **pay-for-what-you-use**: feature を無効化したら依存・コード・`unsafe`・バイナリ増をゼロにする
- **AI ファースト保守性**: doc test・網羅テスト・CI ガードレールで AI が安全に保守できる状態を保つ

仕様書は [Fandhe-AI/backend-framework-spec](https://github.com/Fandhe-AI/backend-framework-spec) を
`docs/spec/`（submodule）に取り込む。実装は `docs/spec/06-roadmap.md` の MS-1〜MS-6 に従い、
最初のタスクは TASK-1.1（`cargo workspace`・CI 基盤整備）。

## Repository Structure

```
backend-framework/
├── CLAUDE.md              # 本ファイル（Claude Code 運用ガイド）
├── README.md
├── skills-lock.json       # 導入スキルのロック
├── docs/
│   ├── spec/               # 仕様書 submodule（要件・タスク・ロードマップ）
│   ├── design/             # リポジトリ側設計ドキュメント（実装フェーズの設計判断を記録）
│   └── dep-impact/         # 依存インパクト（依存数・バイナリサイズ・unsafe 件数）記録台帳（TASK-15.2）
├── Cargo.toml             # cargo workspace ルート（TASK-1.1 で構築、resolver = "3"）
├── rust-toolchain.toml    # stable + rustfmt/clippy
├── crates/                # cargo workspace
│   ├── core                           # 最小コア（TASK-1.1 で作成、実体は TASK-1.3 以降）
│   ├── http / routes                  # HTTP プリミティブ・ルーティング（TASK-1.3〜1.4 以降で追加予定）
│   │   └── fuzz/                      # cargo-fuzz 専用クレート（root workspace から exclude、TASK-15.3-1、#87）
│   ├── plugin-*                       # feature 着脱プラグイン（TASK-2.1 以降で追加予定）
│   └── axum-ref                       # 性能比較用参照実装（TASK-1.2 で追加）
├── benches/               # 負荷生成・計測ハーネス（TASK-1.2 で追加、bench-builder 管轄）
│   ├── README.md                      # 再現手順・複数回計測/中央値評価の規約
│   ├── lib/common.sh                  # 共通関数（サーバ起動/停止・中央値算出・依存ツール検査）
│   └── bench-http.sh / bench-rss.sh / bench-footprint.sh  # RPS・負荷時 RSS・起動時間/バイナリサイズ計測
├── scripts/               # CI・運用スクリプト（TASK-15.2 で追加）
│   ├── README.md                      # 使い方・前提ツール・CI との対応
│   ├── dep-audit.sh                   # 全 feature 構成の cargo audit / cargo deny check（ci.yml dep-audit ジョブ）
│   ├── dep-impact.sh                  # 依存クレート数・バイナリサイズ・unsafe 件数の計測（markdown 出力）
│   └── setup-required-checks.sh       # main の required status check（ci-complete）設定（TASK-14.1、#39）
└── .claude/
    ├── agents/            # 目的別 sub-agent（research/implement/testing/quality/docs）
    ├── rules/             # 運用ルール（委譲・Rust 規約・セキュリティ 等）
    ├── skills/            # 導入スキル（.agents/skills への symlink）
    ├── workflows/         # implement-issue-tree.js（symlink）
    └── settings.json      # SessionStart / PostToolUse hooks
```

## 委譲方針（必読）

main（あなた）のコンテキストは有限。**調査・読解・実装・レビューは subagent へ委譲し、
main は判断・統合・ユーザー対話に集中する**。詳細は [rules/delegation.md](.claude/rules/delegation.md)
（調査・設計）と [rules/delegation-impl.md](.claude/rules/delegation-impl.md)（作成・編集）を参照。

### パスベースの委譲先（要約）

| 対象 | 委譲先 Agent |
|------|-------------|
| `crates/core`・`http`・`routes` の実装 | `core-builder` |
| `crates/plugin-*` の実装 | `plugin-builder` |
| `axum-ref`・ベンチ・負荷試験 | `bench-builder` |
| コード・仕様の横断調査 | `explorer` |
| 外部仕様（crate / RFC）の調査 | `reference-researcher` |
| テスト・カバレッジ | `test-runner` |
| ファジング | `fuzz-runner` |
| 差分レビュー | `reviewer` |
| セキュリティ・依存監査 | `security-auditor` |
| 整形・lint | `linter` |
| ドキュメント | `docs-writer` |

### model 配分

| 用途 | model |
|------|-------|
| 複雑な横断判断・アーキテクチャ設計 | opus または fable（fable は大規模設計・複雑な横断判断の最上位 tier） |
| 調査・生成・実装・レビュー | sonnet |
| 機械的集計・lint・ドキュメント更新 | haiku |

## Sub-agents

`.claude/agents/<category>/<name>.md` に定義。

| カテゴリ | subagent_type | model | 役割 |
|---------|--------------|-------|------|
| research | `explorer` | sonnet | workspace・仕様の横断調査（読み取り専用） |
| research | `reference-researcher` | sonnet | 外部仕様（crate / RFC / axum 等）調査 |
| implement | `core-builder` | sonnet | HTTP コア・ルーティング・3 拡張点 |
| implement | `plugin-builder` | sonnet | feature 着脱プラグイン（ws/graphql/openapi/webrtc/hub/tracing） |
| implement | `bench-builder` | sonnet | 参照実装・Criterion ベンチ・負荷試験 |
| testing | `test-runner` | sonnet | cargo test・llvm-cov |
| testing | `fuzz-runner` | sonnet | cargo-fuzz / afl（パーサ検証） |
| quality | `reviewer` | sonnet | 差分の品質・アーキテクチャ準拠レビュー |
| quality | `security-auditor` | sonnet | cargo audit/deny/geiger・OWASP・unsafe 監査 |
| quality | `linter` | haiku | cargo fmt --check・clippy -D warnings |
| docs | `docs-writer` | haiku | CLAUDE.md / AGENTS.md / doc comment |

## Rules

`.claude/rules/` に定義。

| ファイル | 内容 |
|---------|------|
| [delegation.md](.claude/rules/delegation.md) | 調査・設計フェーズの委譲原則・パスベース切り替え |
| [delegation-impl.md](.claude/rules/delegation-impl.md) | 作成・編集フェーズの委譲マッピング・実装後フロー |
| [coding-rust.md](.claude/rules/coding-rust.md) | Rust 規約（安全性・並行性・拡張点・テスト） |
| [pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md) | feature 無効時の依存・unsafe・バイナリ完全除外原則 |
| [security.md](.claude/rules/security.md) | OWASP Top 10・メモリ安全性・秘密情報混入防止 |
| [japanese-style.md](.claude/rules/japanese-style.md) | 日本語出力スタイル |
| [conventional-commits.md](.claude/rules/conventional-commits.md) | Conventional Commits 詳細規約 |
| [code-comment-style.md](.claude/rules/code-comment-style.md) | コメント・doc comment 規約 |
| [out-of-scope-tracking.md](.claude/rules/out-of-scope-tracking.md) | 実装対象外の追跡（Issue 化）規約 |
| [improvement-proposal.md](.claude/rules/improvement-proposal.md) | 改善提案フロー・起票・承認の運用規約 |
| [feature-modification.md](.claude/rules/feature-modification.md) | 機能要求→実装→テスト→ドキュメント追随→完遂判定の一貫改修フロー運用規約 |
| [feasibility-guardrail.md](.claude/rules/feasibility-guardrail.md) | 対応可否自律判断ガードレール（曖昧要求・危険要求の不可判定規約） |

## Current Skills

`npx skills add` で導入済み（`skills-lock.json` 管理、`.agents/skills` → `.claude/skills` symlink）。

- **開発フロー**: `create-commit` / `create-pr` / `create-issue` / `create-issue-tree` /
  `create-plan` / `implement-issue` / `implement-issue-tree` / `implement-review` /
  `implement-review-pr` / `update-issue-tree`
- **プロジェクト管理**: `project-init` / `project-add-items` / `project-create-issues` /
  `project-update-items` / `project-view-status` / `project-sync-issues` / `project-archive-done`
- **ドキュメント・コメント**: `update-docs` / `comment-code`
- **.claude 体系**: `init-claude` / `update-claude`
- **スキル運用**: `contribute-skill` / `sync-skills-lock`
- **リファレンス**: `rust` / `github-docs` / `commitlint` / `lefthook` / `editorconfig`

## Conventions

- **言語**: ユーザーとのやりとり・コメント・コミット/PR/Issue 本文は日本語（[japanese-style](.claude/rules/japanese-style.md)）
- **コミット**: Conventional Commits 厳守・`--no-verify` 禁止（[conventional-commits](.claude/rules/conventional-commits.md)）。
  作成は `create-commit`、PR は `create-pr` skill
- **セキュリティ**: 変更ごとに OWASP Top 10・依存監査（[security](.claude/rules/security.md)）。
  `security-auditor` に委譲
- **設計原則**: pay-for-what-you-use を全実装で遵守（[pay-for-what-you-use](.claude/rules/pay-for-what-you-use.md)）
- **ユーザー承認フロー**: `implement-issue` 等は計画をユーザー承認後に実装。Issue 起票も承認前提
- **スコープ管理**: スコープ外課題は放置せず [out-of-scope-tracking](.claude/rules/out-of-scope-tracking.md) に従い Issue 化

## hooks（settings.json）

- **SessionStart**: 日本語 / 委譲 / pay-for-what-you-use / Conventional Commits（`--no-verify` 禁止） /
  implement-issue の計画承認フローをリマインド
- **PostToolUse**（Edit|Write）: `.rs` ファイル編集時に `rustfmt` で自動整形（rustfmt 未導入時は no-op）
