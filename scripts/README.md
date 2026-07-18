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
`setup-required-checks.sh` を追加した。TASK-14.3（#41）で、同スクリプトへ PR 必須化・
force push/削除禁止のルールを追加し、受け入れテスト `tests/run-review-gate-tests.sh` を
新設した（`docs/design/review-gate.md` 参照）。
TASK-12.2-1（#81）で、機能要求の実装にテスト追加が伴うことを機械チェックする
`feature-flow-check.sh` を追加した。フロー全体・運用規約は
[`docs/design/feature-modification-flow.md`](../docs/design/feature-modification-flow.md)・
[`.claude/rules/feature-modification.md`](../.claude/rules/feature-modification.md) を参照。
TASK-12.3-2（#84）で、対応可否自律判断ガードレール（TASK-12.3-1、#83、
`docs/design/feasibility-guardrail.md`）の判定記録バリデータ `feasibility-check.sh` と
セルフテスト `tests/run-guardrail-tests.sh` を追加した。
TASK-1.5（#14）で、`crates/routes`（`bf-routes`）新設に伴い `server → routes → http::*`
の依存方向一方向性を CI 常設で機械検証する `dep-direction-check.sh` を追加した。
TASK-2.2（#19）で、プラグイン feature 無効時の依存・`unsafe`・コード 0 件を
cargo tree/geiger・バイナリサイズ・全構成ビルドで PASS/FAIL 判定する
`pay-for-what-you-use-check.sh` を追加した（`docs/design/pay-for-what-you-use-check.md`
参照）。
TASK-3.2（#31）で、「`gen-openapi` CLI 実行 → `openapi.json` 生成 → サーバー本体ビルド」の
2 段階ビルド順序をローカル・CI 双方から同一コマンドで再現する `openapi-two-stage.sh` を
追加した。
TASK-13.1（#49）で、新規プロトコル追加コミットの変更ファイルが拡張点（`Middleware` /
`UpgradeHandler` / `RequestGate`。実体は `crates/core/src/plugin.rs` の固定シーム）へ
閉包しているかを機械判定する `extension-closure-check.sh` を追加した
（`docs/design/extension-closure-verification.md` に WebSocket/WebRTC/GraphQL 実例の
検証結果を記録）。
TASK-6.1（#54）で、`openapi-two-stage.sh` の後段として「`openapi.json` →
`openapi-typescript` → TS 型 → `tsc --noEmit`」の openapi-typescript 連携パイプラインを
ローカル・CI 双方から同一コマンドで再現する `openapi-ts.sh` を追加した
（`docs/design/openapi-typescript-pipeline.md` 参照。生成物・クライアントライブラリ本体は
`ts/` 配下、Rust 側依存には一切影響しない）。
TASK-6.2（#55）で、`openapi-ts.sh` の「`tsc --noEmit` が成功すること」だけでは検証
できない「生成型が実質的な制約として機能していること」を陰性対照として CI 常設化する
`openapi-ts-negative.sh`・受け入れテスト `accept/openapi-ts-accept.sh` を追加した
（`docs/design/openapi-typescript-pipeline.md` TASK-6.2 節・`docs/acceptance/
req6-typescript-types.md` 参照）。

## スクリプト一覧

| スクリプト | 用途 | CI との対応 |
|-----------|------|-------------|
| `dep-audit.sh` | 全 feature 構成で `cargo audit`（`audit-triage.sh` 経由）・`cargo deny check` を実行する依存監査 | `.github/workflows/ci.yml` の `dep-audit` ジョブから呼ばれる |
| `openapi-two-stage.sh` | `gen-openapi` CLI（`bf-plugin-openapi` の `gen-cli` feature）を `--check` 実行し `crates/plugin-openapi/openapi.json` の鮮度を検証してから `cargo build --workspace --all-features` を実行する（`--update` で in-place 再生成も可能） | `.github/workflows/ci.yml` の `openapi-two-stage` ジョブから呼ばれる |
| `audit-triage.sh` | `cargo audit --json` の指摘を「自動更新提案」「要エスカレーション」「情報（記録・監視）」に分類し markdown レポートを生成する | `dep-audit.sh` から呼ばれる。`dep-audit` ジョブは schedule / workflow_dispatch 実行時に限り、検知結果を Issue（`audit-triage` ラベル）として起票する |
| `unsafe-triage.sh` | workspace（`crates/*/src`・`crates/*/tests`）の `unsafe` 使用数を `unsafe-baseline.json` と比較し、増加・SAFETY コメント欠落を検知する | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `dep-impact.sh` | feature 構成ごとの依存クレート数・リリースバイナリサイズ・`unsafe` 件数を計測し markdown 表を出力する | CI からは呼ばれない。plugin 追加 PR でのローカル実行を想定（`docs/dep-impact/README.md` 参照） |
| `coverage.sh` | コア（`backend-framework-core`・`bf-http`。`axum-ref`・`bf-plugin-*` は除外）の行カバレッジを計測し `--fail-under-lines 80` でゲートする | `.github/workflows/ci.yml` の `coverage` ジョブから呼ばれる |
| `accept-task-11-5.sh` | TASK-11.5（#37）受け入れテスト一式（カバレッジ・doc 網羅率・AGENTS.md 各節・CI タイムアウト・依存方向一方向性）を PASS/FAIL/PENDING で判定する | CI からは呼ばれない。TASK-11.5 系イシューのローカル受け入れ確認を想定 |
| `unsafe-baseline.json` | `unsafe-triage.sh` のラチェット判定に使うクレート別ベースライン（コミット対象） | `unsafe-triage.sh --update-baseline` で再生成する |
| `tests/run-triage-tests.sh` | `audit-triage.sh` / `unsafe-triage.sh` のセルフテスト（ネットワーク・cargo ビルド不要） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `feature-flow-check.sh` | 実装変更（`crates/<name>/src/**/*.rs`）に同一クレートのテスト追加が伴うことを検証する（機能改修フロー、REQ-12(b)） | CI からは呼ばれない（必須ゲート化は #82）。セルフテストのみ CI 化 |
| `tests/run-feature-flow-tests.sh` | `feature-flow-check.sh` のセルフテスト（ネットワーク・cargo ビルド不要） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `setup-required-checks.sh` | default branch の repository ruleset に `ci-complete` required status check・PR 必須化・force push/削除禁止を設定する | CI からは呼ばれない。管理者権限を持つ人間・CI 管理者がローカルで 1 回実行する運用（`docs/design/ci-completion-criteria.md`・`docs/design/review-gate.md` 参照） |
| `tests/run-review-gate-tests.sh` | レビューゲート運用（TASK-14.3）の受け入れテスト。`--offline` は lint 表・ci.yml 構成の存在確認のみ（CI 常設）、既定モードは deny lint 検出・ruleset 検証を含むフル層（受け入れ実施時に手動/任意実行） | `--offline` は `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `fuzz.sh` | `crates/http/fuzz`（cargo-fuzz、pinned nightly）の全 fuzz target を実行する。`--max-total-time` で 1 target あたりの実行秒数を切り替え、`--list` で target 名のみ列挙する（TASK-15.3-1、#87） | `.github/workflows/ci.yml` の `fuzz-smoke` ジョブから `--max-total-time 60` で呼ばれる |
| `feasibility-check.sh` | 対応可否自律判断ガードレール（`docs/design/feasibility-guardrail.md`）の判定記録（markdown）を検証する。`--template` は規約準拠のテンプレートを標準出力、`--input <record.md>` は形式・必須項目・fail-closed 原則を検証する | CI からは呼ばれない。判定記録の作成・着手前確認をエージェント・人間がローカルで実行する運用（`.claude/rules/feasibility-guardrail.md` 参照） |
| `tests/run-guardrail-tests.sh` | `feasibility-check.sh` のセルフテスト（PoC-9 T-11〜T-15 判定例 fixture・正常系・異常系、ネットワーク・cargo ビルド不要） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `dep-direction-check.sh` | `server → routes → http::*` の依存方向一方向性を (1) `cargo metadata` 依存エッジのホワイトリスト照合・循環検出、(2) core/routes/http の `src/lib.rs` 依存方向宣言の存在確認、(3) routes・http のプラグイン固有シンボル非依存 grep の 3 段で機械検証する（TASK-1.5、#14）。`--metadata-file <path>` で `cargo metadata` の代わりに JSON を注入できる（セルフテスト用） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `tests/run-dep-direction-tests.sh` | `dep-direction-check.sh` のセルフテスト。`tests/fixtures/dep-direction/*.json` を注入し正常グラフ・逆方向エッジ（循環）・コア→プラグイン依存（ホワイトリスト違反）・dev-dependency 除外を検証する（ネットワーク・cargo ビルド不要） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `pay-for-what-you-use-check.sh` | プラグイン feature 無効時の依存・`unsafe`・コードが 0 件であることを (a) feature 動的列挙 (b) `cargo tree` (c) `cargo geiger` (d) バイナリサイズ・シンボル表 (e) 全構成ビルド の 5 段で PASS/FAIL 判定する（TASK-2.2、#19）。`--metadata-file`/`--tree-negative-file`/`--tree-positive-dir`/`--geiger-packages-file`/`--size-negative`/`--size-positive`/`--symbols-file`/`--skip-build-steps` でセルフテスト用の実データ注入・実ビルド回避ができる | `.github/workflows/ci.yml` の `pay-for-what-you-use` ジョブから呼ばれる |
| `tests/run-pay-for-what-you-use-tests.sh` | `pay-for-what-you-use-check.sh` のセルフテスト。`tests/fixtures/pay-for-what-you-use/*` を注入し (a)〜(d) の判定ロジック（列挙 0 件・命名規約違反・依存漏れ・配線切れ・他プラグイン混入・geiger 漏れ・サイズ逆転・シンボル混入）を検証する（ネットワーク・cargo ビルド不要） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `third-party-verify.sh` | TASK-12.4-1（#85）第三者検証ハーネス。被験 AI（別セッション・別モデル）が実装した使い捨て worktree に対し `fmt --check` / `clippy -D warnings` / `cargo nextest run --profile ci` + `cargo test --doc` を実行し PASS/FAIL/PENDING を判定する | CI からは呼ばれない。TASK-12.4-1 の完遂率再測定を実施する際にローカルで呼び出す運用（`docs/design/third-party-verification.md` 参照） |
| `tests/run-third-party-verify-tests.sh` | `third-party-verify.sh` のセルフテスト。`--offline` は引数検証・PENDING 判定のみ（cargo ビルド不要）、既定モードは fixture worktree を作成して PASS/FAIL 検出まで確認するフル層 | CI からは呼ばれない（フル層は cargo ビルドを伴い時間を要するため） |
| `third-party-feasibility-verify.sh` | 可否判定正解率の第三者再検証（TASK-12.4-2、#86。TASK-12.6、#47 で「条件付き可」対応・`--task-ids` オプションへ拡張）の機械採点ハーネス。タスク定義（正解ラベル）と被験 AI の判定記録を突き合わせ、正解率・誤判定による破壊・判断根拠提示割合を算出する | CI からは呼ばれない。人間が実測定時にローカル/手動実行する（`docs/design/third-party-feasibility-verification.md`・`docs/design/gray-zone-feasibility-verification.md` 参照） |
| `tests/run-third-party-feasibility-tests.sh` | `third-party-feasibility-verify.sh` のセルフテスト（ネットワーク・cargo ビルド不要、合成 fixture で採点ロジックのみを検証。TASK-12.6、#47 で「条件付き可」系アサーションを追加） | CI からは呼ばれない（Rust 非変更の範囲でローカル/任意実行を想定） |
| `extension-closure-check.sh` | 変更ファイル一覧を A（プラグインクレート内）/ B（コア側許容シーム）/ C（テスト）/ D（ドキュメント・運用）/ E（違反）の 4+1 カテゴリに分類し、E が 1 件でもあれば FAIL（拡張点への閉包違反）とする。`--commit <sha>`（`git diff-tree` で変更ファイル取得）または `--files-from <file>`（セルフテスト用注入口）を指定する（TASK-13.1、#49） | `scripts/extension-closure-gate.sh`（下記）から呼ばれる。人間が実コミット検証で直接実行することも可能（TASK-13.2/#50 で `.github/workflows/ci.yml` の `fetch-depth: 0` により shallow clone 制約を解消） |
| `tests/run-extension-closure-tests.sh` | `extension-closure-check.sh` のセルフテスト。`tests/fixtures/extension-closure/*.txt` を注入し閉包（PASS）・`crates/http`/`crates/routes` 混入（FAIL）・空リスト/不正入力（フェイルクローズ）を検証する（ネットワーク・cargo ビルド不要） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `extension-closure-gate.sh` | 拡張点への変更影響範囲閉包 PR ゲート（TASK-13.2、#50、`docs/design/dependency-graph-contract.md` 4 節）。`crates/plugin-*` / `crates/core/src/plugin.rs` を含まない差分は SKIP、含む差分は `extension-closure-check.sh` で閉包判定し、E ファイルがあれば `docs/design/*.md` への理由記載の有無を照合する（記載済みなら WARN 付き PASS、未記載なら FAIL）。`--base <ref>`（CI）または `--files-from <file>`（セルフテスト用注入口） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから `pull_request` イベント時のみ呼ばれる |
| `tests/run-extension-closure-gate-tests.sh` | `extension-closure-gate.sh` のセルフテスト。`tests/fixtures/extension-closure-gate/*.txt` を注入し SKIP/PASS/理由明記済み WARN-PASS/FAIL・フェイルクローズ挙動（不正 ref・引数欠落等）を検証する（ネットワーク・cargo ビルド不要） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `openapi-ts.sh` | `gen-openapi --check`（stage 1）→ `npm ci --ignore-scripts` + `openapi-typescript` による `ts/src/generated/schema.d.ts` 鮮度検証（stage 2）→ `tsc --noEmit`（stage 3）の openapi-typescript 連携パイプラインを検証する（TASK-6.1、#54）。`--update` で `openapi.json`・`schema.d.ts` を in-place 再生成できる。node/npm 未導入時は自動ダウンロードせず導入コマンドを案内して非 0 終了する | `.github/workflows/ci.yml` の `openapi-ts` ジョブから呼ばれる |
| `tests/run-openapi-ts-tests.sh` | `openapi-ts.sh` のセルフテスト。`tests/fixtures/openapi-ts/*` を注入し引数検証・`schema.d.ts` diff 鮮度判定・node/npm 不在時の fail-closed 挙動・CI ジョブ存在確認を検証する（ネットワーク・cargo ビルド不要） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |
| `openapi-ts-negative.sh` | `openapi-ts.sh` の陰性対照（意図的な型不一致が `tsc --noEmit` のエラーとして検出されること）を検証する（TASK-6.2、#55）。N1: `ts/src/negative/type-mismatch.ts`（4 類型）を `tsc --noEmit -p tsconfig.negative.json` にかけ期待 TS エラーコードを確認、N2: `openapi.json` の一時コピーへ型不一致を注入し一時ディレクトリで `schema.d.ts` を再生成して既存 `usage.ts` の型検査失敗を確認する。同一実行内の陽性対照成功も前提条件とする fail-closed 判定（`docs/design/openapi-typescript-pipeline.md` 参照） | `.github/workflows/ci.yml` の `openapi-ts` ジョブから呼ばれる |
| `tests/run-openapi-ts-negative-tests.sh` | `openapi-ts-negative.sh` のセルフテスト。`tests/fixtures/openapi-ts-negative/*` を注入し引数検証・node/npm 不在時の fail-closed 挙動・N1/N2 の期待エラーコード判定ロジック・discrimination（誤った理由での失敗を PASS と誤認しないこと）・CI ステップ存在確認を検証する（ネットワーク・cargo ビルド不要） | `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる |

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
| `cargo-geiger@0.13.0`（`pay-for-what-you-use-check.sh` のみ、必須） | 無効構成の依存グラフに `unsafe` 計上対象のプラグインクレートが含まれないことを検証するゲート。`dep-impact.sh` と異なり未導入・実行失敗は FAIL（フェイルクローズ） | `cargo install --locked cargo-geiger@0.13.0` |
| `cargo-llvm-cov`（`coverage.sh` のみ） | LLVM source-based coverage 計測 | `cargo install --locked cargo-llvm-cov@0.8.7` |
| `llvm-tools-preview`（`coverage.sh` のみ、rustup component） | `cargo-llvm-cov` の instrumented coverage に必要 | `rustup component add llvm-tools-preview` |
| `gh`（`setup-required-checks.sh`・`tests/run-review-gate-tests.sh` のフル層 ruleset 検証） | repository ruleset API 呼び出し（`gh auth login` 済みの認証を利用） | https://cli.github.com/ |
| `cargo-fuzz`（`fuzz.sh` のみ） | libFuzzer ベースの fuzz 実行 | `cargo install --locked cargo-fuzz@0.13.2` |
| nightly ツールチェーン（`fuzz.sh` のみ。`fuzz.sh` の `PINNED_NIGHTLY` が単一真実源） | サニタイザ計装ビルドに必要（`rust-toolchain.toml` の既定 stable は変更しない） | `rustup toolchain install <PINNED_NIGHTLY> --profile minimal` |
| C コンパイラ（`fuzz.sh` のみ） | `libfuzzer-sys` の C++ ランタイムビルドに必要 | OS のパッケージマネージャ（例: `apt install build-essential`） |
| Node.js（`openapi-ts.sh` のみ、`ts/package.json` の `volta`/`engines` フィールドが単一真実源。動作確認済み: 24.13.0） | `openapi-typescript`・`tsc` の実行に必要 | `curl https://get.volta.sh \| bash && volta install node@24.13.0 npm@11.6.2` |
| npm（`openapi-ts.sh` のみ、動作確認済み: 11.6.2） | `ts/` の依存インストール（`npm ci --ignore-scripts`）・スクリプト実行 | Node.js（volta）に同梱 |

## `setup-required-checks.sh` — required status check の設定

```bash
bash scripts/setup-required-checks.sh
```

- default branch（通常 `main`）を対象に、`ci-complete`（`.github/workflows/ci.yml` の集約
  ゲートジョブ）を required status check とする repository ruleset を作成・更新する。
- 同名 ruleset（`main-required-checks`）が既にあれば更新、無ければ新規作成するため
  複数回実行しても安全（冪等）。
- TASK-14.3（#41）以降、required_status_checks に加えて `pull_request`
  （`required_approving_review_count: 0`、main への直 push 禁止）・`non_fast_forward`
  （force push 禁止）・`deletion`（ブランチ削除禁止）を設定する。承認数・strict policy を
  含む運用定義は `docs/design/review-gate.md` を参照。
- リポジトリ管理者権限が無いトークンで実行すると 403 で失敗する。その場合は本スクリプトを
  握りつぶさず、権限を持つ人間が手動実行する。

## `tests/run-review-gate-tests.sh` — レビューゲート運用の受け入れテスト（TASK-14.3、#41）

```bash
bash scripts/tests/run-review-gate-tests.sh --offline  # CI 常設・軽量（ネットワーク/cargo 不要）
bash scripts/tests/run-review-gate-tests.sh            # フル層（受け入れ実施時に手動/任意実行）
```

- `--offline`: `Cargo.toml` の `[workspace.lints.clippy]`/`[workspace.lints.rust]` の
  lint 表と `.github/workflows/ci.yml` の `ci-complete` 集約構成が退行していないかを
  静的に確認する。`unsafe-triage` ジョブから常時呼ばれる。
- フル層（既定）: 上記に加えて (1) `git archive HEAD` による一時複製へ PoC-9 模擬パターンを
  注入し `cargo clippy` が `uninit_vec` で失敗し `#[allow]` 変種が `E0453` で失敗することを
  確認する deny lint 検出テスト、(2) `gh api` で ruleset `main-required-checks` の
  `pull_request`/`non_fast_forward`/`deletion`/`required_status_checks` を確認する
  ruleset 検証テストを実行する。いずれもリポジトリの作業ツリー・共有設定を変更しない
  読み取り専用テストである。
- 詳細（レビューゲートの定義・受け入れテストの設計判断・実施記録）は
  `docs/design/review-gate.md` を参照。

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

## `pay-for-what-you-use-check.sh` — pay-for-what-you-use 機械検証（TASK-2.2、#19）

```bash
bash scripts/pay-for-what-you-use-check.sh
```

プラグイン feature（`crates/core/Cargo.toml` の `dep:bf-plugin-*`）を動的列挙し、
無効時に当該プラグインの依存クレート・`unsafe`・コードが 0 件であることを (a) feature
列挙 (b) `cargo tree` (c) `cargo geiger` (d) バイナリサイズ・シンボル表 (e) 全構成
ビルド の 5 段で PASS/FAIL 判定する。設計判断・`dep-impact.sh` との役割分担の詳細は
`docs/design/pay-for-what-you-use-check.md` を参照。

- (a) の列挙が 0 件、または feature 命名規約（`docs/design/plugin-boundary.md` 2 節）
  違反はフェイルクローズで FAIL とする。
- (c) の `cargo-geiger` は本スクリプトでは必須ツール扱い（`dep-impact.sh` は任意）。
  未導入・実行失敗はいずれも FAIL とし、握りつぶさない。self-hosted ランナーの
  共有 `CARGO_TARGET_DIR` に起因する並行ジョブ間の状態汚染を避けるため、
  `CARGO_TARGET_DIR=target/pay-for-what-you-use-check-geiger` で専用隔離した上で
  最大 2 回まで再試行し、2 回とも失敗した場合は捕捉した stderr を CI ログへ出力する
  （`docs/design/pay-for-what-you-use-check.md` 3.3 節参照）。
- (d) のビルドは共有 `target/` を汚さないよう `target/pay-for-what-you-use-check*`
  専用ディレクトリを使う。

`tests/run-pay-for-what-you-use-tests.sh` はこのスクリプトのセルフテスト
（ネットワーク・cargo ビルド不要、`unsafe-triage` ジョブから呼ばれる）。

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

## `feature-flow-check.sh` — 機能改修の実装/テスト同時性チェック（TASK-12.2-1、#81）

```bash
bash scripts/feature-flow-check.sh --base origin/main
bash scripts/feature-flow-check.sh --base origin/main --allow-no-tests pseudo-crate "理由"
```

- `git diff --name-only -z <base>...<head>` で `crates/<name>/src/**/*.rs` の変更を検出し、
  同一クレートのテスト変更（`crates/<name>/tests/**`、または src 差分の追加行に
  `#[test]` / `#[tokio::test]` / `#[cfg(test)]` / doc test フェンス）が伴わなければ
  `exit 1`（フェイルクローズ）。
- `--allow-no-tests <crate> "<理由>"` で理由必須の明示的除外が可能（警告出力・レビューで
  人間が確認する前提）。
- CI からは呼ばれない。PR の必須ゲートとしての組み込みは #82（完遂判定への組み込み）の
  スコープ。フロー全体・運用規約は
  [`docs/design/feature-modification-flow.md`](../docs/design/feature-modification-flow.md)・
  [`.claude/rules/feature-modification.md`](../.claude/rules/feature-modification.md) を参照。

## `third-party-verify.sh` — 第三者検証ハーネス（TASK-12.4-1、#85）

```bash
bash scripts/third-party-verify.sh --worktree <path> --task-id <ID> \
  [--baseline-tests <起点コミットの cargo test 出力ログ>]
```

`docs/design/third-party-verification.md`（TASK-12.4-1）の第三者検証プロトコルにおける
完遂の一次判定（機械ゲート）を実行する。被験 AI（別セッション・別モデル）が実装した
使い捨て worktree を対象に `cargo fmt --check` / `cargo clippy --all-features -- -D
warnings` / `cargo nextest run --workspace --all-features --profile ci`（テスト単位
タイムアウト付き、`.github/workflows/ci.yml` の test ジョブと同一ランナー） / `cargo test
--doc --workspace --all-features` を実行し、PASS / FAIL / PENDING を判定する（判定区分・
完遂率の算出方法は同書 5 節を参照）。`cargo-nextest` が未導入の環境では PENDING とする。

- `--worktree` にはメイン working copy 自体を指定できない（誤爆防止）。git worktree で
  ないディレクトリ・存在しないパスは PENDING として扱う。
- `--baseline-tests` には起点コミットの `cargo nextest run --workspace --all-features
  --profile ci` 出力ログを渡す。与えた場合、起点コミットで既に失敗していたテストは
  リグレッションとして扱わず、新規に失敗したテストのみを検出する。省略時、またはログ
  ファイルが見つからない場合は個々のテスト失敗をそのまま FAIL とする（fail-closed。
  突合できない場合に楽観判定はしない）。
- 各ゲートには上限時間（既定 600 秒、`THIRD_PARTY_VERIFY_TIMEOUT` で変更可）を設け、
  被験実装のハングを「未完遂」として扱う（完遂率を楽観方向へ歪めない）。nextest 側も
  `.config/nextest.toml`（profile: ci）のテスト単位タイムアウト（60 秒 slow-timeout・
  120 秒強制終了）を併用する多層防御。
- 被験 AI が生成したコードは信頼しない前提のため、実行は指定 worktree 内に閉じ、メイン
  working copy・共有設定には触れない。

`tests/run-third-party-verify-tests.sh` はこのハーネス自体のセルフテスト。`--offline`
は引数検証・PENDING 判定のみ（高速）、既定モードは fixture worktree を作成し PASS/FAIL
検出まで確認するフル層（cargo ビルドを伴うため時間を要する。CI からは呼ばれない）。

## `third-party-feasibility-verify.sh` — 可否判定正解率の第三者再検証・機械採点（TASK-12.4-2、#86／TASK-12.6、#47）

```bash
bash scripts/third-party-feasibility-verify.sh \
  --task-definitions docs/reports/task-12-4-2-task-definitions.md \
  --records-dir <被験 AI の判定記録ディレクトリ> \
  [--task-ids "<空白区切りタスク ID 列。省略時は既定 J-01 ... J-10>"] \
  [--worktrees-dir <被験 worktree ディレクトリ>] \
  [--output <report.md>]
```

- TASK-12.6（#47）でグレーゾーン（判定区分 4 値のうち「条件付き可」）を含めたタスクセット
  （`docs/reports/task-12-6-task-definitions.md`、G-01〜G-10）の採点にも対応した。既定の
  タスク ID（`J-01 ... J-10`）は変更しておらず、`--task-ids` を省略する既存の呼び出し方法
  はそのまま動作する（後方互換）。G-01〜G-10 を採点する場合は
  `--task-ids "G-01 G-02 G-03 G-04 G-05 G-06 G-07 G-08 G-09 G-10"` を指定する。

- PoC-9（`docs/spec/03-poc/ai-first-maintainability/README.md` T-11〜T-15）の可否判定
  正解率 100% は検証者=被験 AI のセルフ実験による自己評価バイアスを排除できていない。
  本スクリプトはタスク設計者・被験 AI・評価者の 3 役分離を機構面で支える評価者役として、
  事前確定したタスクセット（`docs/reports/task-12-4-2-task-definitions.md` の正解ラベル）
  と被験 AI の判定記録を突き合わせ、可否判定正解率・誤判定による破壊件数・判断根拠
  提示割合を機械的に算出する。
- 判定記録・被験 worktree は信頼できない入力として扱い、`## <見出し>` 単位の完全一致
  セクション抽出（`awk`）のみで採点する（`eval`・コマンド置換への再解釈なし、
  `audit-triage.sh`・`feasibility-check.sh` と同一方針）。判定記録は
  `docs/design/feasibility-guardrail.md`・`scripts/feasibility-check.sh --template` と
  同一の見出し形式を前提とする。判定記録の欠落・形式不備は常に不正解・根拠不足側へ
  倒す（fail-closed）。
- `scripts/feasibility-check.sh`（TASK-12.3-2、#84、マージ済み）が存在する場合は不可系
  （不可・要エスカレーション／不可（明確な拒否））の判断根拠提示割合の形式検証をそれへ
  委譲し、存在しない構成でのみ内蔵の最小チェックで代替する。「条件付き可」の判断根拠
  提示割合は `feasibility-check.sh` へ委譲せず、内蔵の新設関数 `check_conditional_fields`
  で判定する（同スクリプトは「ユーザー承認欄が承認済みであること」を要求するが、第三者
  再検証の文脈では被験 AI へ承認を与える人間が介在しないため、正しい記録は逆に未承認の
  ままであるべきで、採点方向が反転してしまうため。
  [`docs/design/gray-zone-feasibility-verification.md`](../docs/design/gray-zone-feasibility-verification.md)
  4 節参照）。
- `--worktrees-dir` 未指定時、誤判定による破壊は PENDING として区別する（0 件と偽らない）。
  破壊計測の対象は「可」以外（不可系・条件付き可）のタスク。「条件付き可」タスクで未承認
  のまま worktree に変更が生じている場合も破壊としてカウントする。
- 終了コードは `0`（採点完了・破壊なし）/ `1`（誤判定による破壊を検知、フェイルクローズ）/
  `2`（引数・入力エラー）。正解率・根拠提示割合自体は情報提示に留め、CI ゲートとしては
  組み込まない（閾値判定は人間レビュー・TASK-12.7 のスコープ）。
- 詳細設計・3 役分離・タスクセット構成・実施手順は
  [`docs/design/third-party-feasibility-verification.md`](../docs/design/third-party-feasibility-verification.md)
  （3 値のみ・「条件付き可」除外の基底プロトコル）・
  [`docs/design/gray-zone-feasibility-verification.md`](../docs/design/gray-zone-feasibility-verification.md)
  （「条件付き可」を含めた拡張プロトコル、TASK-12.6、#47）を参照。TASK-12.6（#47）の
  実測定は本 README 執筆時点で未実施（PENDING）であり、
  [`docs/reports/task-12-6-gray-zone-verification.md`](../docs/reports/task-12-6-gray-zone-verification.md)
  に引き継ぎ事項を記録している。

## `tests/run-third-party-feasibility-tests.sh` — 採点ハーネスのセルフテスト（TASK-12.4-2、#86／TASK-12.6、#47）

```bash
bash scripts/tests/run-third-party-feasibility-tests.sh
```

- ネットワーク・cargo ビルド不要。`scripts/tests/fixtures/feasibility-verify-correct`
  （全件正解）・`feasibility-verify-mixed`（過剰エスカレーション・見落とし・記録欠落・
  形式不備の混在）・`feasibility-verify-gray-correct`/`-gray-self-approval`/
  `-gray-missing-condition`/`-gray-mixed`（TASK-12.6、#47。「条件付き可」の正常系・自己
  承認違反・着手条件欠落・境界誤判定）と、一時的に生成する git リポジトリ（誤判定による
  破壊の検知）で `third-party-feasibility-verify.sh` の採点ロジックを検証する。
- **注意**: 本セルフテストが green であることは「採点ハーネスの算出ロジックが正しく
  動くこと」の確認に過ぎない。独立した被験 AI による実測定が REQ-12 の閾値（80% 以上）
  を達成したことを意味しない。両者を混同しないこと
  （`docs/design/third-party-feasibility-verification.md` 8 節・
  `docs/design/gray-zone-feasibility-verification.md` 9 節）。
