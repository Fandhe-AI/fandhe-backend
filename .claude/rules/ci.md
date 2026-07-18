# CI 実行環境規約

## self-hosted runner の使用（必須）

GitHub Actions のジョブは**すべて `runs-on: self-hosted` で実行する**。
GitHub ホステッドランナー（`ubuntu-latest` / `macos-latest` / `windows-latest` 等）は使用しない。

- 新規ワークフロー・新規ジョブを追加するときも必ず `runs-on: self-hosted` を指定する
- runner はリポジトリレベルではなく **org（Fandhe-AI）レベルで登録**されている。
  リポジトリの runner 一覧（`gh api repos/{owner}/{repo}/actions/runners`）が 0 件でも正常

## self-hosted 前提の運用ルール

- **全ジョブに `timeout-minutes` を設定する**。ハングしたジョブが runner を無期限に
  占有するのを防ぐ（TASK-11.4 / NFR-10 の多層防御。テスト実行は cargo-nextest の
  テスト単位タイムアウトと併用する）
- **schedule 実行は軽量に保つ**。日次 schedule では dep-audit のみを走らせ、
  ビルドを伴うジョブ（fmt/clippy/test/openapi 系）は `if: github.event_name != 'schedule'`
  で除外して runner の負荷を抑える
- schedule 系ワークフロー同士は cron をずらして負荷を分散する
  （例: ci.yml 00:30 UTC / update-external.yml 00:00 UTC）
- **セキュリティ**: self-hosted runner は永続環境のため、ワークフローの `permissions` は
  最小権限（原則 `contents: read`）とし、fork からの PR に対してシークレットを露出する
  トリガー（`pull_request_target` 等）を追加しない（[[security]]）

## 検証

| 検証 | コマンド |
|------|---------|
| runs-on の確認 | `grep -rn "runs-on" .github/workflows/`（全行が `self-hosted` であること） |
| timeout の確認 | 各ジョブに `timeout-minutes` があることを目視確認 |

CI ジョブ構成の変更時は本ルールへの準拠を `reviewer` が確認する。
