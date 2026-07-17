# scripts/ — CI・運用スクリプト

`docs/spec/05-tasks.md` TASK-15.2（#17）の成果物。依存監査（`cargo audit` / `cargo deny
check`）と依存インパクト計測を、feature 構成の増減に追従できる形でまとめたスクリプト集。
TASK-11.5-2（#78）でカバレッジ計測・受け入れテストスクリプトを追加した。
TASK-14.1（#39）で、CI 完遂判定基準（REQ-14）を branch protection に反映する
`setup-required-checks.sh` を追加した。

## スクリプト一覧

| スクリプト | 用途 | CI との対応 |
|-----------|------|-------------|
| `dep-audit.sh` | 全 feature 構成で `cargo audit`・`cargo deny check` を実行する依存監査 | `.github/workflows/ci.yml` の `dep-audit` ジョブから呼ばれる |
| `dep-impact.sh` | feature 構成ごとの依存クレート数・リリースバイナリサイズ・`unsafe` 件数を計測し markdown 表を出力する | CI からは呼ばれない。plugin 追加 PR でのローカル実行を想定（`docs/dep-impact/README.md` 参照） |
| `coverage.sh` | コア（`backend-framework-core`・`bf-http`。`axum-ref`・`bf-plugin-*` は除外）の行カバレッジを計測し `--fail-under-lines 80` でゲートする | `.github/workflows/ci.yml` の `coverage` ジョブから呼ばれる |
| `accept-task-11-5.sh` | TASK-11.5（#37）受け入れテスト一式（カバレッジ・doc 網羅率・AGENTS.md 各節・CI タイムアウト・依存方向一方向性）を PASS/FAIL/PENDING で判定する | CI からは呼ばれない。TASK-11.5 系イシューのローカル受け入れ確認を想定 |
| `setup-required-checks.sh` | default branch の repository ruleset に `.github/workflows/ci.yml` の集約ゲートジョブ `ci-complete` を required status check として設定する | CI からは呼ばれない。管理者権限を持つ人間・CI 管理者がローカルで 1 回実行する運用（`docs/design/ci-completion-criteria.md` 参照） |

## 前提ツール

いずれのスクリプトも前提ツールを自動ダウンロードしない（`.claude/rules/security.md`・
`benches/` と同じ方針）。冒頭で存在検査を行い、見つからない場合は導入コマンドを案内して
終了する。

| ツール | 用途 | 導入コマンド |
|--------|------|-------------|
| `cargo-deny` | ライセンス・出所・重複バージョン監査 | `cargo install --locked cargo-deny@0.19.8` |
| `cargo-audit` | RustSec advisory DB による既知脆弱性検知 | `cargo install --locked cargo-audit@0.22.2` |
| `jq` | `cargo metadata` の JSON 解析・`setup-required-checks.sh` の ruleset ペイロード生成 | OS のパッケージマネージャ（例: `apt install jq`） |
| `cargo-geiger`（`dep-impact.sh` のみ、任意） | `unsafe` 件数の計測 | `cargo install --locked cargo-geiger` |
| `cargo-llvm-cov`（`coverage.sh` のみ） | LLVM source-based coverage 計測 | `cargo install --locked cargo-llvm-cov@0.8.7` |
| `llvm-tools-preview`（`coverage.sh` のみ、rustup component） | `cargo-llvm-cov` の instrumented coverage に必要 | `rustup component add llvm-tools-preview` |
| `gh`（`setup-required-checks.sh` のみ） | repository ruleset API 呼び出し（`gh auth login` 済みの認証を利用） | https://cli.github.com/ |

## `setup-required-checks.sh` — required status check の設定

```bash
bash scripts/setup-required-checks.sh
```

- default branch（通常 `main`）を対象に、`ci-complete`（`.github/workflows/ci.yml` の集約
  ゲートジョブ）を required status check とする repository ruleset を作成・更新する。
- 同名 ruleset（`main-required-checks`）が既にあれば更新、無ければ新規作成するため
  複数回実行しても安全（冪等）。
- required_status_checks のみを設定する。PR 必須化・人間承認必須・force push 禁止などは
  TASK-14.3（#41、担当: 人間）のスコープであり本スクリプトは変更しない。
- リポジトリ管理者権限が無いトークンで実行すると 403 で失敗する。その場合は本スクリプトを
  握りつぶさず、権限を持つ人間が手動実行する。

## `dep-audit.sh` — 依存監査

```bash
bash scripts/dep-audit.sh
```

- `cargo audit`: `Cargo.lock`（.gitignore 対象のため実行前に無ければ `cargo
  generate-lockfile` で生成）を対象に 1 回実行する。`Cargo.lock` は feature 構成に
  関わらず workspace 全クレートの依存を解決した結果であるため、1 回の実行で全 feature
  構成の依存をカバーできる。
- `cargo deny check`: `--no-default-features` / default / 各 feature 単体 /
  `--all-features` の構成ごとに実行する。feature 一覧は `cargo metadata --no-deps` から
  動的に列挙するため、`crates/plugin-*`（TASK-2.1 以降）で feature が増えても本スクリプト
  ・`ci.yml` の変更なしに監査対象へ自動的に加わる。
  - 実装メモ: `cargo deny check` 自体には `--features` 系の CLI フラグが存在しない
    （feature 構成は `deny.toml` の `[graph]` セクションでのみ制御される）。本スクリプトは
    `cargo metadata --format-version 1 <feature フラグ>` で構成ごとの依存グラフ JSON を
    生成し、`cargo deny check --metadata-path <json>` に渡すことで `deny.toml` を書き換え
    ずに構成を切り替えている。
- 1 構成でも違反（advisory 検知・ライセンス違反・出所違反）があれば非 0 で終了する
  （フェイルクローズ、`.claude/rules/security.md`）。

## `dep-impact.sh` — 依存インパクト計測

```bash
bash scripts/dep-impact.sh
```

feature 構成（no-default / default / all-features）ごとの依存クレート数（workspace
メンバー除外）、workspace 内 bin ターゲットのリリースビルドサイズ、（`cargo-geiger`
導入時のみ）`unsafe` 件数を markdown 表で標準出力する。運用（記録先・比較手順）は
`docs/dep-impact/README.md` を参照。

## `coverage.sh` — コア行カバレッジ計測（TASK-11.5-2、#78）

```bash
bash scripts/coverage.sh
# 閾値を一時的に変更する場合（動作確認・陰性対照用）
FAIL_UNDER_LINES=99 bash scripts/coverage.sh
```

- `cargo-llvm-cov nextest --workspace --all-features`（`.config/nextest.toml` の
  `profile ci`）で 1 回計測し、`cargo llvm-cov report` のパッケージフィルタ（再計測なし）
  で「コア」（`cargo metadata` から動的決定。`axum-ref`・`bf-plugin-*` を除外した残り。
  現状は `backend-framework-core`・`bf-http`）とワークスペース全体（プラグイン含む、
  参考情報）の両方のサマリを出し分ける。
- コア対象の行カバレッジが既定 80%（`FAIL_UNDER_LINES` で変更可能）未満の場合は
  非 0 で終了する（退行ゲート）。lcov は `target/llvm-cov/lcov.info`（`.gitignore`
  対象の `target/` 配下、リポジトリへは混入しない）に出力する。
- doc test（`cargo test --doc`）は stable ツールチェーンでは instrumented coverage の
  対象にできない（`cargo-llvm-cov` の `--doctests` は nightly 専用）ため、本スクリプトの
  計測対象は単体・統合テスト（nextest 経由）のみ。

## `accept-task-11-5.sh` — TASK-11.5 受け入れテスト（#78）

```bash
bash scripts/accept-task-11-5.sh
```

TASK-11.5（#37）が要求する 5 項目（カバレッジ 80% 以上・doc コメント網羅率 100%・
AGENTS.md 各節・CI テストタイムアウト設定・依存方向の一方向性）をチェックごとに
PASS / FAIL / PENDING で出力する。AGENTS.md 本体は TASK-11.3（#35）のスコープのため、
未作成時は FAIL ではなく PENDING（#35 待ち）として区別する。FAIL が 1 件でもあれば
非 0 で終了し、PENDING のみなら 0 で終了する。
