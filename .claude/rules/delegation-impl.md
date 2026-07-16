# 委譲方針（作成・編集フェーズ）

## 原則

コードの作成・編集は技術レイヤに対応する builder エージェントへ委譲する。
main は要件分解・依存順の制御・受け入れ判断を担い、実装本体は subagent に任せる。
並列実行できる独立作業は同時に委譲する。

## パスベースの委譲マッピング

| 対象パス / 作業 | 委譲先 Agent |
|----------------|-------------|
| `crates/core`・`crates/http`・`crates/routes`（HTTP コア・ルーティング・拡張点） | `core-builder` |
| `crates/plugin-*`（websocket / graphql / openapi / webrtc / hub-wiring / tracing） | `plugin-builder` |
| `crates/axum-ref`・`benches/**`・負荷試験ハーネス | `bench-builder` |
| テスト実行・カバレッジ | `test-runner` |
| ファジング（パーサ堅牢性） | `fuzz-runner` |
| ドキュメント（CLAUDE.md / AGENTS.md / doc comment） | `docs-writer` |

## 実装後の標準フロー

1. `reviewer` でセルフレビュー（品質・アーキテクチャ準拠）
2. `security-auditor` でセキュリティ・依存監査（[[security]]）
3. `test-runner` でテスト（feature 構成ごと）
4. `linter` で `cargo fmt --check` / `cargo clippy -- -D warnings`
5. `create-commit` skill で [[conventional-commits]] 準拠のコミット（`--no-verify` 禁止）

## 注意

- feature ゲート漏れ・依存残留（[[pay-for-what-you-use]]）は plugin-builder / reviewer で必ず確認する
- スコープ外の課題は [[out-of-scope-tracking]] に従い記録し、現在の変更に混ぜない
- 調査・設計フェーズの委譲は [[delegation]] を参照
