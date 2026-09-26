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
| 1 | PR 経由必須（main への直 push 不可） | 可能 | ruleset `main-protection`（#693 でリポジトリ外運用の実態に合わせて一本化。以前は本タスクで `main-required-checks` に追加した）の `pull_request` ルール |
| 2 | `ci-complete` 全通過（fmt / clippy / test / doc / coverage / dep-audit / unsafe-triage の集約を含む required status check 25 件） | 可能 | 既存 required status check（TASK-14.1、#39。実際の contexts は `scripts/setup-required-checks.sh --print-desired` を正とする） |
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

main の実際の保護は repository ruleset **`main-protection`**（対象: default branch を
`~DEFAULT_BRANCH` で参照、`enforcement: active`）で、リポジトリの外（GitHub UI）で運用
されている。#693 以前は `scripts/setup-required-checks.sh` が存在しない
`main-required-checks` を対象にし、required status check を `ci-complete` の 1 件だけに
絞っていたため、実際の保護とスクリプトの定義が乖離していた。#679（PR #685、fmt/clippy/test
の matrix 化）の際にこの乖離が原因で PR がマージできなくなり ruleset を手作業で修正した
経緯があり、#693 でスクリプトの定義を `main-protection` の現行構成に一致させ、
`scripts/setup-required-checks.sh --check` で以後のずれを検出できるようにした。

`main-protection` の現行構成は次のとおり（正は
`scripts/setup-required-checks.sh --print-desired`）。

| ルール | 設定値 | 根拠 |
|--------|--------|------|
| `pull_request` | `required_approving_review_count: 0`・`required_review_thread_resolution: true`・`allowed_merge_methods: ["squash"]`・その他パラメータは `--print-desired` 参照 | main への直 push を禁止し PR 経由を機械強制する（レビューゲートの土台）。承認数は既存の AI レビュー運用（単独メンテナ + push 前 review + squash merge）を壊さないよう `0` とする |
| `non_fast_forward` | 有効 | main への force push を禁止する（履歴改変によるレビュー済み内容のすり替え防止） |
| `deletion` | 有効 | main ブランチの削除を禁止する |
| `required_status_checks` | `ci-complete` を含む 25 件（matrix 化した fmt/clippy/test の 9 件・codex 系 3 件・Cursor Bugbot 等、`strict_required_status_checks_policy: false`） | TASK-14.1（#39）で `ci-complete` を確立、#679/PR #685 の matrix 化・#693 の一本化を経て現行構成に至る。個別 context の対応は `scripts/setup-required-checks.sh` のコメントを参照 |

`bypass_actors` は空のまま維持する（例外経路を作らない、fail-closed）。PUT ペイロードでは
`bypass_actors: []` を明示的に送る（フィールド省略では既存の bypass_actors がクリアされる
保証がなく、冪等な再実行で例外が残留しうるため）。

### 2.1 ジョブ名を変えるときの運用（#693 で明文化）

required status check の context は ci.yml 等のジョブ名（一部は `matrix.os` を含む文字列）
と厳密一致するため、ジョブ名を改名すると required check が「存在しないジョブ」を待ち続け
マージできなくなる（#679/PR #685 で実際に発生）。ジョブ名を変える PR では次の手順を踏む。

1. `ci.yml`（または `ai-review.yml`）のジョブ名変更と、`scripts/setup-required-checks.sh`
   の required contexts 定義の変更を同じ PR で行う
2. 管理者が `scripts/setup-required-checks.sh --check` を実行し、旧名の削除・新名の追加
   だけが差分に出ることを確認する
3. マージ直前に管理者が `scripts/setup-required-checks.sh`（apply）を実行する（旧名が
   required のままだと当該 PR 自身がマージできなくなるため、マージ前に適用する必要がある）。
   **通常の default branch（`main`）チェックアウトではまだ旧 context 定義のスクリプトしか
   手元になく、解消したいマージ不能状態（新 context が required でない・旧 context が
   required のまま）を解消できない。** 新 context 定義を含む PR head 側のスクリプトを
   明示的に使う（例: `git fetch origin pull/<PR番号>/head && git show
   origin/pull/<PR番号>/head:scripts/setup-required-checks.sh | bash -s --` や、PR ブランチを
   `checkout` してからの実行）
4. マージ後に `--check` が exit 0 になることを確認する（この時点では `main` を checkout
   した状態で実行してよい。マージ後の `main` に新 context 定義が反映済みのため）

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
  `[workspace.lints.rust]` に `unsafe_op_in_unsafe_fn = "deny"` が**行頭からの有効な設定行**
  として存在することを確認する（TASK-14.2 の lint 表が後から弱体化・削除される退行に加え、
  コメントアウトによる無効化も検知する）。
- `.github/workflows/ci.yml` の `ci-complete` ジョブブロックを抽出し、その `needs` 配列の
  要素として判定対象ジョブ（fmt / clippy / test / doc / coverage / dep-audit / unsafe-triage）が
  厳密一致で存在することを確認する（ファイル全体への単純な部分文字列検索ではなく、
  `needs` 配列内の要素照合とすることで、コメントや他ジョブ名への偶然の一致を除外し、
  集約ゲートの判定対象が黙って縮小される退行を確実に検知する）。

### 3.2 フル層（既定モード、受け入れ実施時に手動/任意実行）

- **deny lint 検出テスト**: `git archive HEAD` で scratch 領域へ workspace 全体を複製し、
  `crates/http/src/lib.rs` 相当の複製ファイルへ PoC-9 模擬パターン（`with_capacity` の
  直後に `unsafe { reserve; set_len }`）を注入したうえで `cargo clippy -p fandhe-backend-http --
  -D warnings` を実行し、非 0 終了かつ出力に `uninit_vec` を含むことを確認する。
  さらに同じ関数に `#[allow(clippy::uninit_vec, ...)]` を付与した変種でも、
  `E0453`（forbid lint への `#[allow]` はコンパイルエラー）が発生することを確認する。
  **作業ツリー（このリポジトリのコミット済み内容）は一切変更しない**。複製は
  `mktemp -d` で作成し `trap` で必ず削除する。
- **ruleset 検証テスト**（`gh` 必要）: `scripts/setup-required-checks.sh --check` を実行し、
  `main-protection` の全構成（required status checks・`pull_request` パラメータ・
  `bypass_actors`・`non_fast_forward`・`deletion`・`conditions`）が live と一致すること
  （exit 0）を確認する。さらに `repos/{nwo}/rules/branches/{default_branch}` を用いて、
  `main-protection` の ruleset_id が実際にデフォルトブランチへ適用されていることも確認する
  （内容照合そのものは #693 で `--check` に一本化し、個別 jq 照合は行わない。オフラインの
  ケース網羅は `scripts/tests/run-setup-required-checks-tests.sh` が担う）。

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
  - `uninit_vec` 注入 → `cargo clippy -p fandhe-backend-http -- -D warnings` は非 0 終了、出力に
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

### 4.3 `--check` による live 照合（2026-09-26、#693）

- 背景: main の実際の保護は `main-required-checks` ではなく `main-protection`
  （id 20587666）であり、required status check は `ci-complete` の 1 件ではなく 25 件
  （matrix 化した fmt/clippy/test の 9 件・codex 系 3 件・Cursor Bugbot 等）だった。
  `required_review_thread_resolution: true`・`allowed_merge_methods: ["squash"]` も
  設定されていたが、スクリプト・本ドキュメントには反映されていなかった。#679/PR #685 の
  matrix 化時に ruleset を手作業で更新した経緯もリポジトリに記録されていなかった。
- 対応: `scripts/setup-required-checks.sh` の宛先・定義を `main-protection` の現行構成に
  一致させ、`--check`（読み取り専用）・`--print-desired`（gh 不要）を追加した。§1・§2 の
  記述もこの現行構成に合わせて更新した。
- 実施日: 2026-09-26
- コマンド: `bash scripts/setup-required-checks.sh --check`（読み取りのみ、書き込みは
  一度も実行していない）
- 結果: PASS（`main-protection`（id=20587666）が定義と完全一致、exit 0）。
  `repos/{nwo}/rules/branches/main` にも `ruleset_id=20587666` が含まれることを確認した
  （デフォルトブランチへの実適用確認）。
- `scripts/tests/run-setup-required-checks-tests.sh`（新設、17 ケース）・
  `scripts/tests/run-review-gate-tests.sh`（`--offline` 26 PASS・フル層 33 PASS）も
  同日に実行し全 PASS。

## 5. TASK-14.3 の範囲外（out-of-scope-tracking 対象）

- `pull_request.required_approving_review_count` を 1 以上へ引き上げる実施判断
  （§2「人間判断ダイヤル」参照。本タスクでは文書化のみ）
- `required_status_checks.strict_required_status_checks_policy` を `true` にする実施判断
  （同上）
- PR へのレビュー結果自動投稿の CI 化（現行は `implement-issue-tree` の push 前 review
  運用と PR 本文への手動記載で担保する）
- `cargo geiger` の CI 常設化（TASK-15 系、`docs/dep-impact/` 運用のスコープ）

該当課題が具体化した場合は `.claude/rules/out-of-scope-tracking.md` に従い Issue へ記録する。

## 6. 受け入れ検証レポート

受け入れ検証レポート: `docs/acceptance/req14-verification-gate.md`（#264）。
REQ-14 受け入れ基準 3 項目のうち基準 3（レビューゲート運用の定義）の証跡として
本文書（定義・ruleset ルール・受け入れテスト構成・§4 実施記録）を集約転記している。
