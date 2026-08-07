# CI 実行環境規約

## self-hosted runner の使用（必須）

GitHub Actions のジョブは**すべてセルフホストランナーで実行する**。`runs-on` には
`self-hosted`、または `.github/actionlint.yaml` の `self-hosted-runner.labels`
ホワイトリストに登録されたセルフホストカスタムラベル（専用プール選択用。例:
`codex-review.yml` の `no-sudo`）のみを指定する。
GitHub ホステッドランナー（`ubuntu-latest` / `macos-latest` / `windows-latest` 等）は
**引き続き使用しない**（この原則はカスタムラベル許容後も不変）。

本規約は組織 runner 方針（ユーザー決定 2026-08-07）「リポジトリの**可視性**で runner を
決める: public は GitHub ホステッド（`ubuntu-latest` 等）、private は self-hosted」の
private 側の適用である。方針の正は Fandhe-AI/actions の
[`docs/runner-policy.md`](https://github.com/Fandhe-AI/actions/blob/main/docs/runner-policy.md)
（Fandhe-AI/actions#33 の成果物。対象リポジトリ一覧・codex-review の self-hosted
専用 runner 例外を含む）を参照する。本リポジトリは private のため self-hosted 既定の
現行構成（本規約・全ワークフロー）と整合しており、実態変更はない。

- 新規ワークフロー・新規ジョブを追加するときは原則 `self-hosted` を指定し、専用プールが
  必要な場合のみ `.github/actionlint.yaml` へラベルを登録した上でカスタムラベルを使う
  （未登録ラベルは `scripts/actionlint.sh` が runner-label エラーで検知する fail-closed 構成）
- カスタムラベルの実体（どのプールに何台登録されているか）は Fandhe-AI/local-server の
  gha-runner 手順書で管理する（`.github/actionlint.yaml` のコメントに記載）
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
  （例: ci.yml 00:30 UTC / update-external.yml 00:00 UTC / bench-schedule.yml
  週次 02:00 UTC 日曜）
- **週次ベンチ workflow（`bench-schedule.yml`、イシュー #285）は「日次 schedule は
  dep-audit のみ」方針の例外ではない**。REQ-1/NFR-1 性能ベンチ（`benches/
  bench-accept-exclusive.sh`）はビルド + 専有計測を伴い重いため、ci.yml の日次
  schedule には相乗りさせず、別 workflow・週次実行に切り出すことで両立させる
  （設計比較は `docs/design/bench-scheduled-run.md` 参照）
- **セキュリティ**: self-hosted runner は永続環境のため、ワークフローの `permissions` は
  最小権限（原則 `contents: read`）とし、fork からの PR に対してシークレットを露出する
  トリガー（`pull_request_target` 等）を追加しない（[[security]]）

## 検証

| 検証 | コマンド |
|------|---------|
| runs-on の確認 | `grep -rhE "^[[:space:]]*runs-on:" .github/workflows/ \| awk '{print $2}' \| sort -u \| grep -vxFf <(printf '%s\n' self-hosted; sed -n 's/^[[:space:]]*- //p' .github/actionlint.yaml)`（出力が空であること。`self-hosted` またはホワイトリスト登録済みラベルのみで構成されていることを意味する。この `sed` は `actionlint.yaml` 内の `- ` 始まりの行を一律抽出するため、同ファイルに `self-hosted-runner.labels` 以外のリスト値キーを追加する場合は本コマンドの前提が崩れる点に注意。また `runs-on:` のスカラー表記のみ対応し、配列 `[a, b]` 表記は誤検知しうる） |
| ラベル未登録・typo の機械検知 | `bash scripts/actionlint.sh`（`.github/actionlint.yaml` 未登録の `runs-on` ラベルを runner-label エラーとして検知。actionlint 未導入環境では前提ツールエラーで exit 2 になる） |
| timeout の確認 | 各ジョブに `timeout-minutes` があることを目視確認 |

CI ジョブ構成の変更時は本ルールへの準拠を `reviewer` が確認する。
