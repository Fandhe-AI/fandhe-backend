# scripts/ — CI・運用スクリプト

`docs/spec/05-tasks.md` TASK-15.2（#17）の成果物。依存監査（`cargo audit` / `cargo deny
check`）と依存インパクト計測を、feature 構成の増減に追従できる形でまとめたスクリプト集。
TASK-11.5-2（#78）でカバレッジ計測・受け入れテストスクリプトを追加した。
TASK-12.1-1（#79）で、audit 指摘・`unsafe` 追加を検知したときのトリアージ（分類・推奨
アクション提示）ロジックを追加した。改善提案フロー・運用規約は TASK-12.1-2（#80）で
[`docs/design/improvement-proposal-flow.md`](../docs/design/improvement-proposal-flow.md)・
[`.claude/rules/improvement-proposal.md`](../.claude/rules/improvement-proposal.md) として
整備済みである。本 README は各スクリプトの使い方・CI との対応関係を説明するに留める。
TASK-14.1（#39）で、CI 完遂判定基準（REQ-14）を branch protection に反映する
`setup-required-checks.sh` を追加した。

## スクリプト一覧

| スクリプト | 用途 | CI との対応 |
|-----------|------|-------------|
| `dep-audit.sh` | 全 feature 構成で `cargo audit`（`audit-triage.sh` 経由）・`cargo deny check` を実行する依存監査 | `.github/workflows/ci.yml` の `dep-audit` ジョブから呼ばれる |
| `audit-triage.sh` | `cargo audit --json` の指摘を「自動更新提案」「要エスカレーション」「情報（記録・監視）」に分類し markdown レポートを生成する | `dep-audit.sh` から呼ばれる。`dep-audit` ジョブは schedule / workflow_dispatch 実行時に限り、検知結果を Issue（`audit-triage` ラベル）として起票する |
| `unsafe-triage.sh` | workspace（`crates/*/src`・`crates/*/tests`）の `unsafe` 使用数を `unsafe-baseline.json` と比較し、増加・SAFETY コメント欠落を検知する | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `dep-impact.sh` | feature 構成ごとの依存クレート数・リリースバイナリサイズ・`unsafe` 件数を計測し markdown 表を出力する | CI からは呼ばれない。plugin 追加 PR でのローカル実行を想定（`docs/dep-impact/README.md` 参照） |
| `coverage.sh` | コア（`backend-framework-core`・`bf-http`。`axum-ref`・`bf-plugin-*` は除外）の行カバレッジを計測し `--fail-under-lines 80` でゲートする | `.github/workflows/ci.yml` の `coverage` ジョブから呼ばれる |
| `accept-task-11-5.sh` | TASK-11.5（#37）受け入れテスト一式（カバレッジ・doc 網羅率・AGENTS.md 各節・CI タイムアウト・依存方向一方向性）を PASS/FAIL/PENDING で判定する | CI からは呼ばれない。TASK-11.5 系イシューのローカル受け入れ確認を想定 |
| `unsafe-baseline.json` | `unsafe-triage.sh` のラチェット判定に使うクレート別ベースライン（コミット対象） | `unsafe-triage.sh --update-baseline` で再生成する |
| `tests/run-triage-tests.sh` | `audit-triage.sh` / `unsafe-triage.sh` のセルフテスト（ネットワーク・cargo ビルド不要） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `setup-required-checks.sh` | default branch の repository ruleset に `.github/workflows/ci.yml` の集約ゲートジョブ `ci-complete` を required status check として設定する | CI からは呼ばれない。管理者権限を持つ人間・CI 管理者がローカルで 1 回実行する運用（`docs/design/ci-completion-criteria.md` 参照） |

## 前提ツール

いずれのスクリプトも前提ツールを自動ダウンロードしない（`.claude/rules/security.md`・
`benches/` と同じ方針）。冒頭で存在検査を行い、見つからない場合は導入コマンドを案内して
終了する。

| ツール | 用途 | 導入コマンド |
|--------|------|-------------|
| `cargo-deny` | ライセンス・出所・重複バージョン監査 | `cargo install --locked cargo-deny@0.19.8` |
| `cargo-audit` | RustSec advisory DB による既知脆弱性検知 | `cargo install --locked cargo-audit@0.22.2` |
| `jq` | `cargo metadata`・`cargo audit --json` の JSON 解析（`audit-triage.sh`・`unsafe-triage.sh` も使用）・`setup-required-checks.sh` の ruleset ペイロード生成 | OS のパッケージマネージャ（例: `apt install jq`） |
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

- `cargo audit`（`audit-triage.sh` 経由）: `Cargo.lock`（.gitignore 対象のため実行前に
  無ければ `cargo generate-lockfile` で生成）を対象に 1 回実行する。`Cargo.lock` は
  feature 構成に関わらず workspace 全クレートの依存を解決した結果であるため、1 回の
  実行で全 feature 構成の依存をカバーできる。
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

## `audit-triage.sh` — audit 指摘のトリアージ

```bash
bash scripts/audit-triage.sh
bash scripts/audit-triage.sh --input <cargo-audit-json> --output <report.md>
```

`cargo audit --json` の出力を次の 3 区分に分類し、markdown レポートを標準出力する
（`--output` 指定時はファイルへも書き出す）。

| 区分 | 条件 | 推奨アクション |
|------|------|----------------|
| 自動更新提案 | vulnerability かつ `versions.patched` が非空 | 修正版への更新コマンドを提示（`cargo update -p <crate>`） |
| 要エスカレーション | vulnerability かつ patched なし（未修正） | 代替 crate 検討・`deny.toml` ignore（理由必須、ユーザー承認要）をユーザーへ提示 |
| 情報（記録・監視） | warnings（unmaintained / unsound / yanked / notice） | CI は失敗させず記録・監視のみ（cargo audit 既定の安全側動作） |

`--input` はテスト用フィクスチャ注入口で、指定時はネットワーク接続なしにロジックを検証
できる（`tests/fixtures/` 参照）。終了コードは `0`（vulnerability なし）/ `1`
（vulnerability あり、フェイルクローズ）/ `2`（前提ツール・引数エラー）。

`--vuln-ids-output <path>` は vulnerability（自動更新提案・要エスカレーション区分）の
advisory ID のみを改行区切りで書き出す。markdown レポート全体を正規表現で走査すると
「情報（記録・監視）」区分（warnings）の advisory ID まで拾ってしまい、CI が green
（warnings のみで `exit 0`）でも Issue が起票される不整合が生じるため、機械可読な
区別が必要な呼び出し元（`ci.yml` の Issue 起票ステップ）はこちらを使う。

advisory 由来の文字列（id・title・description 等）は信頼できない外部データとして扱い、
`eval` やシェル再解釈に渡さない（OWASP A03 対策、`.claude/rules/security.md`）。

`dep-audit` ジョブは schedule / workflow_dispatch 実行時に限り、vulnerability を検知
した advisory ごとに `audit-triage` ラベル付きの Issue を起票する（`gh issue list` で
重複起票を防止）。この自動起票は「フレームワークの自動監査機構」による能動的な提示で
あり、開発エージェントの Issue 起票承認規約（`.claude/rules/out-of-scope-tracking.md`）
とは別レイヤ。

## `unsafe-triage.sh` — unsafe 追加の検知トリアージ

```bash
bash scripts/unsafe-triage.sh
bash scripts/unsafe-triage.sh --update-baseline
```

`crates/*/src`・`crates/*/tests`（`target/` 除外）を走査し、クレート別の `unsafe`
使用数（`unsafe fn` / `unsafe impl` / `unsafe trait` / `unsafe extern` / `unsafe {` の
実利用パターンのみを対象。コメント中の \`unsafe\` 等の字面誤検知は避ける設計）・
`#[allow(unsafe_code)]` 使用数を `unsafe-baseline.json` と比較する「ラチェット」方式で
検知する。

- **増加検知**: いずれかのクレートで baseline より増加していれば `file:line` を報告して
  `exit 1`。対応: `// SAFETY:` コメントで根拠を記載し、レビュー承認を得たうえで
  `--update-baseline` によるベースライン更新を同一 PR に含める。
- **SAFETY コメント必須**: `unsafe` を含むファイルに `// SAFETY:` が 1 件もなければ
  baseline 内の増減に関わらず `exit 1`（`.claude/rules/coding-rust.md` の機械強制）。
- **減少**: `exit 0` で通過しつつベースライン縮小を情報提示する。

依存 crate 側の `unsafe` 増減は本スクリプトの対象外（`dep-impact.sh` の cargo-geiger
計測を参照）。テキストベースの走査のためコメント・文字列リテラル内の字面を誤検知しうる
限界があるが、誤検知は人間のレビューが確認する安全側に倒れるため許容する。

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
