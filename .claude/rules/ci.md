# CI 実行環境規約

## Runner 方針（GitHub ホステッド既定）

GitHub Actions のジョブは **GitHub ホステッドランナー（`runs-on: ubuntu-latest` 等の
標準スペック）を既定とする**（ユーザー指示 2026-08-08。本リポジトリは 2026-08-08 に
public 化されており、public リポジトリでは標準ホステッドランナーが無料・分数消費なし）。

本規約は組織 runner 方針（ユーザー決定 2026-08-07）「リポジトリの**可視性**で runner を
決める: public は GitHub ホステッド（`ubuntu-latest` 等）、private は self-hosted」の
**public 側**の適用である。方針の正は Fandhe-AI/actions の
[`docs/runner-policy.md`](https://github.com/Fandhe-AI/actions/blob/main/docs/runner-policy.md)
（Fandhe-AI/actions#33 の成果物。対象リポジトリ一覧・codex-review の self-hosted
専用 runner 例外を含む）を参照し、本節では書き写さない（ドリフト防止）。

- 新規ジョブ追加時もホステッドランナーを使用し、`runs-on: self-hosted` を使わない。
  larger runner（有料の大型ホステッドランナー）も使わない
- **唯一の例外（codex-review の codex 実行ジョブ）**: `codex-review.yml` が呼び出す
  reusable workflow の codex 実行ジョブ（`runner-label: codex`）のみ、codex-home 方式の
  認証情報を runner 上に配置する構成のため self-hosted な codex 専用 runner の使用を
  認める（`docs/runner-policy.md` §3 の明文例外）。例外は codex 実行ジョブに閉じる:
  PR コメント投稿ジョブ（`post-feedback-runner-label`）は資格情報に触れないため
  ubuntu-latest を明示指定し、この例外を根拠に他ジョブを self-hosted 化しない
- 旧方針（private 前提の self-hosted 既定、PR #549）は public 化に伴い廃止。既存
  ワークフローの移行はトラッキングイシュー #550（子 #551〜#556）で実施した
- self-hosted カスタムラベル（`.github/actionlint.yaml` の `self-hosted-runner.labels`
  ホワイトリスト）は codex 例外のためにのみ残す。未登録ラベルは `scripts/actionlint.sh`
  が runner-label エラーで検知する（fail-closed 構成、不変）。ラベルの実体は
  Fandhe-AI/local-server の gha-runner 手順書で管理する
- codex 専用 runner は org（Fandhe-AI）レベルで登録されている。リポジトリの runner
  一覧（`gh api repos/{owner}/{repo}/actions/runners`）が 0 件でも正常

## ホステッド前提への読み替え（旧 self-hosted 前提だった箇所）

- ホステッドランナーはジョブごとにクリーンな使い捨て VM のため、ツールの
  「導入済みならスキップ」冪等判定（`Fandhe-AI/actions/cargo-tool-install` 等）は
  常に新規インストール側へ倒れる。バージョン固定・検証付きの毎ジョブ導入が主経路に
  なるため、既存の導入ステップは削除・弱体化しない
- Rust toolchain の導入は `dtolnay/rust-toolchain`（コミット SHA 固定）を正とする
  （self-hosted の永続環境前提だった `Fandhe-AI/actions/rust-toolchain-setup` の代替。
  イシュー #551 で確立、`docs-site.yml` build ジョブが先行事例）
- ビルドを伴うジョブは `actions/cache`（コミット SHA 固定）で cargo registry /
  `target` をキャッシュしてよい。キーは
  `<job-family>-<runner.os>-<toolchain cachekey>-<hashFiles('**/Cargo.toml')>`
  （本リポジトリは Cargo.lock 非コミットのため Cargo.toml を入力とする）、
  restore-keys は toolchain までの prefix フォールバック。キャッシュは最適化であり
  正当性の前提にしない（miss はフルビルドへフォールバック、fail-open）

## 運用ルール（runner 種別に依らず維持）

- **全ジョブに `timeout-minutes` を設定する**。ハングしたジョブが runner を無期限に
  占有するのを防ぐ（TASK-11.4 / NFR-10 の多層防御。テスト実行は cargo-nextest の
  テスト単位タイムアウトと併用する）
- **schedule 実行は軽量に保つ**。日次 schedule では dep-audit のみを走らせ、
  ビルドを伴うジョブ（fmt/clippy/test/openapi 系）は `if: github.event_name != 'schedule'`
  で除外する（旧 self-hosted 時代は runner 負荷抑制が動機。ホステッドでも無駄な
  ジョブ実行・Actions キュー消費を避ける原則として維持する）
- schedule 系ワークフロー同士は cron をずらして負荷を分散する
  （例: ci.yml 00:30 UTC / update-external.yml 00:00 UTC / bench-schedule.yml
  週次 02:00 UTC 日曜）
- **週次ベンチ workflow（`bench-schedule.yml`、イシュー #285）は「日次 schedule は
  dep-audit のみ」方針の例外ではない**。REQ-1/NFR-1 性能ベンチ（`benches/
  bench-accept-exclusive.sh`）はビルド + 専有計測を伴い重いため、ci.yml の日次
  schedule には相乗りさせず、別 workflow・週次実行に切り出すことで両立させる
  （設計比較は `docs/design/bench-scheduled-run.md` 参照）
- **セキュリティ**: ワークフローの `permissions` は最小権限（原則 `contents: read`）とし、
  fork からの PR に対してシークレットを露出するトリガー（`pull_request_target` 等）を
  追加しない（[[security]]。public 化により fork PR の可能性が現実化したため、旧
  self-hosted 時代より重要度が上がっている。codex 専用 runner（唯一の self-hosted
  例外）は永続環境のため、fork PR での実行拒否等の多層防御を弱体化しない）

## 検証

| 検証 | コマンド |
|------|---------|
| runs-on の確認 | `grep -rhE "^[[:space:]]*runs-on:" .github/workflows/ \| awk '{print $2}' \| sort -u \| grep -vxF ubuntu-latest`（移行完了後は出力が空であること。`runs-on` 直書きは `ubuntu-latest` のみで構成されていることを意味する。codex 例外は reusable workflow の `runner-label` input 経由のため本コマンドには現れない。`runs-on:` のスカラー表記のみ対応し、配列 `[a, b]` 表記は誤検知しうる。#551〜#555 で全ワークフローの移行が完了済みのため、`self-hosted` の出力は常に違反である） |
| ラベル未登録・typo の機械検知 | `bash scripts/actionlint.sh`（`.github/actionlint.yaml` 未登録の `runs-on` ラベルを runner-label エラーとして検知。actionlint 未導入環境では前提ツールエラーで exit 2 になる） |
| timeout の確認 | 各ジョブに `timeout-minutes` があることを目視確認 |

CI ジョブ構成の変更時は本ルールへの準拠を `reviewer` が確認する。
