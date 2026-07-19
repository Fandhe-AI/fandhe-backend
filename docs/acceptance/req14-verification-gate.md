# REQ-14 受け入れ検証レポート — AI 改修の検証ゲート（TASK-14.1〜14.3）

## 本レポートの位置づけ

`docs/spec/04-requirements.md` REQ-14（AI 改修の検証ゲート）は TASK-14.1（#39）・
TASK-14.2（#40）・TASK-14.3（#41）で全受け入れ基準 PASS 済みだが、他 REQ（`req1`〜
`req13`・`req15` 等）が従う `docs/acceptance/req<N>-<topic>.md` の命名・配置パターンに
対し、証跡が `docs/design/ci-completion-criteria.md`・`docs/design/unsafe-deny-lints.md`・
`docs/design/review-gate.md`（§4 実施記録）に分散同居したまま `docs/acceptance/` 側に
レポートが存在しなかった（仕様照合 #252 で検出）。REQ-10 の同種不整合（#219）・REQ-11 の
同種不整合（#236 → PR #249、コミット `e8aba16`）に続く是正である。

本レポートは REQ-10 レポート（`docs/acceptance/req10-tracing.md`）と同型であり、**新規
検証は行わず**、上記 3 設計文書（TASK-14.3 実施記録は 2026-07-17 実施）に既に記録された
判定・証跡を `docs/acceptance/` の命名パターンへ集約転記したものである。実測値・実施記録の
詳細ログはすべて転記元の各設計文書を正本として参照する。

## 実施記録の環境情報

本レポートは新規実測を行っていないため、転記元の実施記録に基づく情報のみを示す。

| 項目 | 値 |
|------|-----|
| 転記元の実施日 | 2026-07-17（TASK-14.3 §4 ruleset 更新・受け入れテスト） |
| 転記元の実施記録の所在 | `docs/design/review-gate.md` §4「実施記録」 |
| TASK-14.2 ネガティブ検証の実施記録の所在 | `docs/design/unsafe-deny-lints.md`「ネガティブ検証（実施記録）」節 |
| 本レポート作成日 | 2026-07-19（イシュー #264、転記のみ） |

## 判定サマリー

| 判定 | 受け入れ基準 | 検証方法 | 結果概要 |
|------|------------|---------|---------|
| PASS | AI が生成した変更は `cargo test` / `clippy -- -D warnings` / `fmt --check` の全通過を必須条件としてマージされる | `.github/workflows/ci.yml` の集約ゲートジョブ `ci-complete` + `scripts/setup-required-checks.sh` による required status check | `ci-complete` が fmt/clippy/test/doc/dep-audit/coverage/unsafe-triage 等の実ジョブを `needs` 集約し `if: always()` の fail-closed 判定で単一の成否に集約。ruleset `main-required-checks` に required status check として設定済み（2026-07-17 実施） |
| PASS | 危険な `unsafe` パターンが `cargo clippy` の deny lint で機械的に検出される | ルート `Cargo.toml` の `[workspace.lints.clippy]`（forbid 11 lint・deny 3 lint の 2 層）+ ネガティブ検証 | `uninit_vec` 注入パターンが `clippy::uninit_vec` エラーで検出、`#[allow]` による抑制試行は forbid 層により `error[E0453]` でブロック（deny 層のみ局所例外を許容） |
| PASS | 自律実装のマージには CI 通過に加えてレビューゲート（人間承認または追加の AI レビュー）を経る運用が定義されている | `docs/design/review-gate.md`（レビューゲート定義・ruleset ルール・受け入れテスト） | `implement-issue-tree` の push 前 review 標準運用を定義。ruleset に `pull_request`・`non_fast_forward`・`deletion` ルールを追加（`bypass_actors` 空）。フル層受け入れテスト 2026-07-17 実施で全 PASS、オフライン層は CI `unsafe-triage` ジョブで常設継続 |

**3 基準すべて PASS**（FAIL 0 件）。

## 証跡（基準別）

### 基準 1: CI 全通過の必須化（TASK-14.1、#39）

- **集約ゲートジョブ `ci-complete`**（`.github/workflows/ci.yml`）: `fmt` / `clippy` /
  `test` / `doc` / `dep-audit` に加え、リポジトリが実際に運用する全品質ゲート
  （`coverage` / `unsafe-triage` / `pay-for-what-you-use` / `fuzz-smoke` /
  `openapi-two-stage` / `openapi-ts` / `actionlint`）を `needs` に列挙して結果を集約する。
  `if: always()` により `needs` のジョブが失敗してもゲート自体は実行され、`success` 以外
  （`failure` / `cancelled` / `skipped`）を一律「未完遂」として扱う fail-closed 設計
  （GitHub の「skipped が pass 扱いになりうる」既知の落とし穴への対処）。
  `ci-complete` ジョブ自体は checkout も外部 action も使わずシェル組み込みのみで判定する
  （サプライチェーン表面ゼロ）。`schedule` イベント時は `ci-complete` 自体が丸ごとスキップ
  される（`if: github.event_name != 'schedule'`）。
- **required status check の設定**（`scripts/setup-required-checks.sh`）: default branch の
  repository ruleset に `ci-complete` を required status check として設定するスクリプト。
  TASK-14.1 時点（2026-07-16）で main ブランチは無保護（branch protection 404 / ruleset
  0 件）だったため、本スクリプトの実行により初めて `ci-complete` が必須化された。
- 一次記録: `docs/design/ci-completion-criteria.md`

### 基準 2: 危険 `unsafe` パターンの機械的検出（TASK-14.2、#40）

- **2 層 lint テーブル**（ルート `Cargo.toml` の `[workspace.lints.clippy]`）:
  - 第 1 層 forbid（`#[allow]` による抑制自体を禁止、11 lint）: `uninit_vec` /
    `uninit_assumed_init` / `mem_replace_with_uninit` / `transmuting_null` /
    `wrong_transmute` / `unsound_collection_transmute` / `eager_transmute` /
    `cast_slice_different_sizes` / `zst_offset` / `out_of_bounds_indexing` /
    `not_unsafe_ptr_arg_deref`
  - 第 2 層 deny（局所 `#[allow]` による例外化が可能、3 lint）: `undocumented_unsafe_blocks` /
    `unnecessary_safety_comment` / `multiple_unsafe_ops_per_block`
  - `[workspace.lints.rust]` に `unsafe_op_in_unsafe_fn = "deny"` も設定
- **ネガティブ検証（実施記録）**: `crates/http/src/lib.rs` 末尾に PoC-9 の模擬パターン
  （`Vec::with_capacity` 直後の `unsafe { reserve; set_len }`）を一時注入して実証（検証後に
  revert 済み、コミットには含まれない）。
  1. 注入パターンで `cargo clippy -p fandhe-backend-http -- -D warnings` を実行 →
     `clippy::uninit_vec` および `clippy::undocumented_unsafe_blocks` の 2 件でエラー
  2. `#[allow(dead_code, clippy::uninit_vec, clippy::undocumented_unsafe_blocks)]` を付与して
     再実行 → `error[E0453]: allow(clippy::uninit_vec) incompatible with previous forbid` で
     それでもエラー（forbid 層が `#[allow]` による抑制を許さないことを実証。
     `undocumented_unsafe_blocks` は deny 層のため `#[allow]` で抑制でき、この lint のみは
     通過 = 設計どおりの挙動）
  3. 注入コードを revert し、クリーンツリーで `cargo clippy --workspace --all-targets
     --all-features -- -D warnings` が全通過することを再確認
- **受け入れテストのスクリプト化**: `scripts/tests/run-review-gate-tests.sh` のフル層
  「deny lint 検出テスト」が上記ネガティブ検証を再実行可能な形にスクリプト化している
  （`git archive HEAD` で scratch 領域へ複製し、作業ツリーは変更しない）
- 一次記録: `docs/design/unsafe-deny-lints.md`

### 基準 3: レビューゲート運用の定義（TASK-14.3、#41）

- **レビューゲートの定義**（`docs/design/review-gate.md` §1）: 自律実装を main へ取り込む
  条件を (1) PR 経由必須 (2) `ci-complete` 全通過 (3) レビューゲート通過（人間承認または
  追加の AI レビュー）の AND として定義。標準運用は `implement-issue-tree` の Implement
  フェーズとは別セッションの reviewer（`implement-review` スキル）が差分をレビューし、
  通過した場合にのみ push・PR 作成を行う「push 前 review」。PR 本文にレビュー実施の証跡を
  必須化する。
- **ruleset ルール**（`docs/design/review-gate.md` §2、`scripts/setup-required-checks.sh`）:
  `main-required-checks` ruleset に `pull_request`（`required_approving_review_count: 0`）・
  `non_fast_forward`・`deletion` を追加。`required_status_checks`（`ci-complete`）は既存維持。
  `bypass_actors` は空のまま維持（例外経路を作らない fail-closed）。承認数引き上げや
  `strict_required_status_checks_policy` の `true` 化は「人間判断ダイヤル」として明文化に
  留め、本タスクでは値を決め打ちしない。
- **受け入れテストの 2 層構成**（`scripts/tests/run-review-gate-tests.sh`）:
  - オフライン層（`--offline`、CI 常設）: ネットワーク・cargo ビルド不要。lint テーブルの
    行頭有効性・`ci-complete` の `needs` 配列要素の厳密一致を確認。`ci.yml` の
    `unsafe-triage` ジョブに組み込み常時実行される
  - フル層（既定モード）: deny lint 検出テスト（上記基準 2 のネガティブ検証を
    スクリプト化）+ ruleset 検証テスト（`gh api` で `main-required-checks` の
    `active` 状態・required status check・各ルール・`bypass_actors` を確認）
- **実施記録**（`docs/design/review-gate.md` §4、2026-07-17）:
  - ruleset 更新: `bash scripts/setup-required-checks.sh` を実行し PASS
    （既存 ruleset `main-required-checks`、id=19074973、を PUT で更新。`pull_request` /
    `non_fast_forward` / `deletion` ルールを追加、`required_status_checks` は既存維持）
  - フル層受け入れテスト: deny lint 検出テスト（`uninit_vec` 注入検出・`#[allow]` 抑制不可
    `E0453`・作業ツリー非改変）と ruleset 検証テスト（`active` / `ci-complete` 含有 /
    `pull_request` `non_fast_forward` `deletion` 有効 / `bypass_actors` 空）の全項目が PASS
  - オフライン層も同日実行で全 PASS。以後 `ci.yml` の `unsafe-triage` ジョブで常時実行継続
- 一次記録: `docs/design/review-gate.md`

## 一次記録・関連文書表

| 文書・スクリプト | 役割 |
|------|------|
| `docs/design/ci-completion-criteria.md` | 基準 1 の一次記録（`ci-complete` 設計・受け入れ基準対応表） |
| `docs/design/unsafe-deny-lints.md` | 基準 2 の一次記録（2 層 lint テーブル設計・ネガティブ検証） |
| `docs/design/review-gate.md` | 基準 3 の一次記録（レビューゲート定義・ruleset 設計・受け入れテスト・実施記録） |
| `scripts/setup-required-checks.sh` | required status check・ruleset ルールの設定スクリプト |
| `scripts/tests/run-review-gate-tests.sh` | レビューゲート受け入れテスト（オフライン層・フル層） |
| ルート `Cargo.toml`（`[workspace.lints.clippy]` / `[workspace.lints.rust]`） | 2 層 deny lint テーブルの実体 |
| `.github/workflows/ci.yml`（`ci-complete` / `unsafe-triage` ジョブ） | 集約ゲート・オフライン層受け入れテストの実行基盤 |

## 補足: 本レポートの限界

- **再実測なし**: 本レポートは 2026-07-17 時点の一次記録（`docs/design/review-gate.md` §4・
  `docs/design/unsafe-deny-lints.md` ネガティブ検証）からの転記であり、作成時点
  （2026-07-19）で `cargo clippy` やネガティブ検証コードの再注入・ruleset API 呼び出しを
  再実行していない。実測値の正本は各一次記録側にある。
- **ruleset は GitHub 側設定**: `main-required-checks` ruleset はリポジトリ内のスナップショット
  ではなく GitHub 側の設定であるため、本レポート・一次記録の記載時点以降に手動変更が
  加われば乖離しうる。現状値の確認は `gh api repos/{nwo}/rulesets` を都度実行する必要がある。
- **継続担保はオフライン層が担う**: ruleset のフル層検証・deny lint 検出テストは手動/任意
  実行のため、リポジトリ内容側の退行（lint テーブルの弱体化・`ci-complete` の判定対象縮小）
  に対する継続的な機械検知は `ci.yml` の `unsafe-triage` ジョブに組み込まれたオフライン層
  （`scripts/tests/run-review-gate-tests.sh --offline`）が担う。

## 関連

- 転記元 1（基準 1）: `docs/design/ci-completion-criteria.md`
- 転記元 2（基準 2）: `docs/design/unsafe-deny-lints.md`
- 転記元 3（基準 3・実施記録）: `docs/design/review-gate.md`
- 同種の配置是正の先行事例: `docs/acceptance/req10-tracing.md`（#219）・
  `docs/acceptance/req11-ai-first-maintainability.md`（#236 → PR #249）
- 関連 Issue: #39（TASK-14.1）・#40（TASK-14.2）・#41（TASK-14.3）・#252（配置不整合検出）・
  #264（本レポート新設）
