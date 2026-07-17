# レビューゲート運用定義・受け入れテスト（TASK-14.3、#41、REQ-14）

## 対応する仕様

- `docs/spec/04-requirements.md` REQ-14「AI 改修の検証ゲート」
- `docs/spec/05-tasks.md` TASK-14.3「レビューゲート運用定義と受け入れテスト」

REQ-14 の受け入れ基準のうち、TASK-14.1（#39）・TASK-14.2（#40）は次の 2 点を機械化済みである
（詳細は `docs/design/ci-completion-criteria.md`・`docs/design/unsafe-deny-lints.md`）。

- [x] AI が生成した変更は `cargo test` / `clippy -- -D warnings` / `fmt --check` の全通過を
      必須条件としてマージされる（集約ゲート `ci-complete` + `scripts/setup-required-checks.sh`）
- [x] 危険な `unsafe` パターンが `cargo clippy` の deny lint で機械的に検出される
      （`Cargo.toml` の `[workspace.lints.clippy]` 2 層 lint）

残る 1 点が本タスクのスコープである。

- [ ] 自律実装のマージには、CI 通過に加えてレビューゲート（人間承認または追加レビュー）を
      経る運用が定義されている

## 1. レビューゲートの定義

自律実装（AI が生成した変更）を main へ取り込む条件を、次の 3 点の AND として定義する。

| # | 条件 | 機械強制 | 担保方法 |
|---|------|---------|----------|
| 1 | PR 経由必須（main への直 push 不可） | 可能 | ruleset `main-required-checks` の `pull_request` ルール（本タスクで追加） |
| 2 | `ci-complete` 全通過（fmt / clippy / test / doc / dep-audit / unsafe-triage の集約） | 可能 | 既存 required status check（TASK-14.1、#39） |
| 3 | レビューゲート通過（人間承認 **または** 追加の AI レビュー） | 一部（証跡の存在は確認可能だが、レビュー内容の妥当性そのものは機械判定できない） | 下記 §1.1 の運用 |

REQ-14 は「人間または追加の AI レビュー」を明示的に許容している（単独メンテナ体制でも
機械的な CI 通過だけに頼らず、レビューという追加の判断ステップを必ず経る、という趣旨）。
本リポジトリはこれを次の標準運用として具体化する。

### 1.1 標準運用: `implement-issue-tree` の push 前 review

- 実装は `implement-issue-tree` ワークフローの Implement フェーズ（本ドキュメントを含む
  変更もこのフローで生成される）で行う。
- 実装セッションとは **別の reviewer セッション**（`implement-review` スキル、
  `.claude/rules/delegation-impl.md` の実装後フロー）が差分の品質・アーキテクチャ準拠・
  セキュリティ（OWASP Top 10）をレビューする。
- レビューが通過した場合に **のみ** push・PR 作成を行う（push 前 review）。レビューで
  問題が見つかった場合は push せず、実装セッションへ差し戻して修正してから再度レビューする。
- 生成された PR に対しては、必要に応じて `implement-review-pr` スキルで CI ステータス・
  Conventional Commits 準拠を含む追加レビューを実施できる（GitHub PR 経由のレビュー）。
- **証跡の必須化**: PR 本文にレビュー実施の証跡（レビュー結果・確認したセキュリティ観点・
  対象外事項）を残すことを必須とする。証跡が無い PR は本運用の要件を満たさないため、
  マージ前に人間が追記を求めることができる。

人間が直接レビューする場合も同様に、PR の承認（レビューコメントまたは Approve）を
レビューゲート通過の証跡として扱う。

## 2. ruleset の機械強制範囲と人間判断ダイヤル

`scripts/setup-required-checks.sh` が設定する ruleset `main-required-checks`
（対象: default branch、`enforcement: active`）に、本タスクで次のルールを追加する。

| ルール | 設定値 | 根拠 |
|--------|--------|------|
| `pull_request` | `required_approving_review_count: 0` | main への直 push を禁止し PR 経由を機械強制する（レビューゲートの土台）。承認数は既存の AI レビュー運用（単独メンテナ + push 前 review + squash merge）を壊さないよう `0` とする |
| `non_fast_forward` | 有効 | main への force push を禁止する（履歴改変によるレビュー済み内容のすり替え防止） |
| `deletion` | 有効 | main ブランチの削除を禁止する |
| `required_status_checks` | 既存維持（`ci-complete` のみ、`strict_required_status_checks_policy: false`） | TASK-14.1 で確立済み。変更しない |

`bypass_actors` は空のまま維持する（例外経路を作らない、fail-closed）。

### 人間判断ダイヤル（本タスクでは実施判断を行わない項目）

次の 2 点は機械的には設定可能だが、運用体制（単独メンテナ・並列自動実装）への影響が
大きいため、**本タスクでは値を決め打ちせず「人間管理者が判断するダイヤル」として明文化する
に留める**。

1. **`pull_request.required_approving_review_count` を 1 以上へ引き上げるか**
   引き上げると、人間の Approve が無い限りいかなる PR もマージできなくなる
   （AI レビューのみでは通らなくなる）。チーム体制・レビュー担当者の可用性に応じて
   人間管理者が決定する。
2. **`required_status_checks.strict_required_status_checks_policy` を `true` にするか**
   `true` にすると、PR は「マージ先ブランチの最新コミットに対して再実行された CI」の
   通過を要求される（ブランチ追従の強制）。`implement-issue-tree` は複数 Issue を並列に
   worktree で実装し CI を 1 回だけ起動する運用のため、`true`化は並列実装のスループットを
   下げる可能性がある。運用実績を見て人間管理者が判断する。

## 3. 受け入れテストの構成

`scripts/tests/run-review-gate-tests.sh` に 2 層構成で実装する。

### 3.1 オフライン層（`--offline`、CI 常設）

ネットワーク・cargo ビルド不要。`ci.yml` の `unsafe-triage` ジョブに組み込み常時実行する
（既存 `run-triage-tests.sh` と同じ位置づけ）。

- `Cargo.toml` の `[workspace.lints.clippy]` に forbid 11 lint・deny 3 lint、
  `[workspace.lints.rust]` に `unsafe_op_in_unsafe_fn = "deny"` が存在することを確認する
  （TASK-14.2 の lint 表が後から弱体化・削除される退行を検知する）。
- `.github/workflows/ci.yml` に `ci-complete` ジョブと、判定対象ジョブ（fmt / clippy /
  test / doc / dep-audit / unsafe-triage）への `needs` が存在することを確認する
  （集約ゲートの判定対象が黙って縮小される退行を検知する）。

### 3.2 フル層（既定モード、受け入れ実施時に手動/任意実行）

- **deny lint 検出テスト**: `git archive HEAD` で scratch 領域へ workspace 全体を複製し、
  `crates/http/src/lib.rs` 相当の複製ファイルへ PoC-9 模擬パターン（`with_capacity` の
  直後に `unsafe { reserve; set_len }`）を注入したうえで `cargo clippy -p bf-http --
  -D warnings` を実行し、非 0 終了かつ出力に `uninit_vec` を含むことを確認する。
  さらに同じ関数に `#[allow(clippy::uninit_vec, ...)]` を付与した変種でも、
  `E0453`（forbid lint への `#[allow]` はコンパイルエラー）が発生することを確認する。
  **作業ツリー（このリポジトリのコミット済み内容）は一切変更しない**。複製は
  `mktemp -d` で作成し `trap` で必ず削除する。
- **ruleset 検証テスト**（`gh` 必要）: `gh api repos/{nwo}/rulesets` と
  `repos/{nwo}/rules/branches/{default_branch}` を用いて、`main-required-checks` が
  `active` であること、`ci-complete` が required status check に含まれること、
  `pull_request` / `non_fast_forward` / `deletion` の各ルールが有効であること、
  `bypass_actors` が空であることを確認する。

いずれのテストも `set -euo pipefail` の fail-closed とし、期待する失敗（clippy の非 0 終了・
`E0453`）が実際には起きなかった場合を FAIL とする negative test である
（`unsafe-deny-lints.md` のネガティブ検証の考え方を再実行可能な形にスクリプト化したもの）。

## 4. 実施記録

### 4.1 ruleset 更新（`scripts/setup-required-checks.sh`）

- 実施日: 2026-07-17
- コマンド: `bash scripts/setup-required-checks.sh`
- 結果: PASS（既存 ruleset `main-required-checks`（id=19074973）を PUT で更新。
  `pull_request`（`required_approving_review_count: 0`）・`non_fast_forward`・`deletion`
  ルールを追加し、`required_status_checks`（`ci-complete`）は既存設定を維持）
- 実行トークン: `gh auth status` で確認済みの既存認証（`repo` スコープ含む）を使用。
  トークン文字列自体はログ・本ファイルに含めていない。

### 4.2 受け入れテスト（フル層、`scripts/tests/run-review-gate-tests.sh`）

- 実施日: 2026-07-17
- deny lint 検出テスト:
  - `uninit_vec` 注入 → `cargo clippy -p bf-http -- -D warnings` は非 0 終了、出力に
    `clippy::uninit_vec` を含む → PASS
  - `#[allow(clippy::uninit_vec, ...)]` 付与変種 → `error[E0453]` を含む非 0 終了 → PASS
  - 検証用複製は `/tmp` 配下の一時ディレクトリのみに作成し、作業ツリーは変更していない
    ことを `git status` で確認済み → PASS
- ruleset 検証テスト:
  - `main-required-checks` が `active` → PASS
  - `ci-complete` が required status check に含まれる → PASS
  - `pull_request` / `non_fast_forward` / `deletion` ルールが有効 → PASS
  - `bypass_actors` が空 → PASS
- オフライン層（`--offline`）も同日に実行し全 PASS（`ci.yml` の `unsafe-triage` ジョブでも
  以後常時実行される）。

## 5. TASK-14.3 の範囲外（out-of-scope-tracking 対象）

- `pull_request.required_approving_review_count` を 1 以上へ引き上げる実施判断
  （§2「人間判断ダイヤル」参照。本タスクでは文書化のみ）
- `required_status_checks.strict_required_status_checks_policy` を `true` にする実施判断
  （同上）
- PR へのレビュー結果自動投稿の CI 化（現行は `implement-issue-tree` の push 前 review
  運用と PR 本文への手動記載で担保する）
- `cargo geiger` の CI 常設化（TASK-15 系、`docs/dep-impact/` 運用のスコープ）

該当課題が具体化した場合は `.claude/rules/out-of-scope-tracking.md` に従い Issue へ記録する。
