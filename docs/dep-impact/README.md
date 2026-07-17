# docs/dep-impact/ — 依存インパクト計測・記録運用

`docs/spec/05-tasks.md` TASK-15.2（#17、REQ-15）の成果物。`pay-for-what-you-use`
（`.claude/rules/pay-for-what-you-use.md`）を「機能無効時に依存・バイナリサイズ・
`unsafe` が増えていない」という数値で検証できるようにするための運用ドキュメント。

**ゲートとの関係**（TASK-2.2、#19）: 本ディレクトリ・`dep-impact.sh` は変更前後の
計測値を人間が比較・記録する運用（下記手順）を担い、CI ゲートとしての PASS/FAIL
判定は行わない。プラグイン feature 無効時の依存・`unsafe`・コード 0 件を機械的に
PASS/FAIL 判定し CI に常設するのは `scripts/pay-for-what-you-use-check.sh`
（`.github/workflows/ci.yml` の `pay-for-what-you-use` ジョブ）であり、責務は分離
している。詳細は `docs/design/pay-for-what-you-use-check.md` を参照。

## 運用手順

`crates/plugin-*` を新規追加・変更する PR では、次の手順で依存インパクトを計測・記録する。

1. **変更前（`main`）の計測**: `main` を checkout した状態で `bash
   scripts/dep-impact.sh` を実行し、結果を控える。
2. **変更後（作業ブランチ）の計測**: 実装ブランチで同じコマンドを実行する。
3. **増分の確認**（`--no-default-features` 構成が対象。pay-for-what-you-use の核）:
   - 依存クレート数が増えていないこと
   - `unsafe` 件数が増えていないこと（`cargo-geiger` 導入時）
   - workspace 内 bin のリリースバイナリサイズが増えていないこと（対象 bin が存在する場合）
   - 増えている場合は feature ゲート漏れ（`#[cfg(feature = "...")]` の付け忘れ、
     `optional = true` / `dep:` 構文の付け忘れ）を疑う
4. **記録**: 変更前後の計測結果を PR 本文と本ディレクトリの `records.md` に追記する。
5. **確認担当**: 計測は `plugin-builder` が実施し、増分が pay-for-what-you-use に
   違反していないかは `reviewer` がセルフレビュー時に確認する
   （`.claude/rules/delegation-impl.md`）。

## 記録先

- `records.md`: 記録台帳。エントリごとに日付・対象 PR/イシュー・feature 構成・
  計測結果（依存クレート数・バイナリサイズ・`unsafe` 件数）を追記する。
- 本タスク（#17）では、`crates/plugin-*` が存在しない現行 workspace のベースラインを
  初回エントリとして記録した。以降のプラグイン追加 PR はこのベースラインとの差分を確認する。

## 計測コマンド

```bash
bash scripts/dep-impact.sh
```

詳細（前提ツール・出力形式）は `scripts/README.md` を参照。
