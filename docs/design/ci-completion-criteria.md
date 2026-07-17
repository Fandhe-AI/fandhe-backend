# CI 完遂判定基準（TASK-14.1、#39、REQ-14）

## 対応する仕様

- `docs/spec/04-requirements.md` REQ-14「AI 改修の検証ゲート」
- `docs/spec/05-tasks.md` TASK-14.1「CI 完遂判定基準実装」

REQ-14 は AI による改修の完遂判定基準を次のように定義する（PoC-9 由来）。

> 完遂の判定基準を「CI（`cargo test` / `clippy -- -D warnings` / `fmt --check`）が全て
> 通過し、追加機能が受け入れ基準を満たすこと」と定義する。

この基準は 2 つの要素からなり、機械判定できるかどうかが異なる。

| 要素 | 機械判定 | 本タスクでの担保方法 |
|------|---------|----------------------|
| CI（`cargo test` / `clippy -D warnings` / `fmt --check`）の全通過 | 可能 | `.github/workflows/ci.yml` の集約ゲートジョブ `ci-complete` + `scripts/setup-required-checks.sh` による required status check |
| 追加機能が受け入れ基準を満たすこと | 不可能（人間の判断を要する） | レビューゲート（TASK-14.3、#41、担当: 人間）。本タスクのスコープ外 |

## 実装: 集約ゲートジョブ `ci-complete`

### 課題

`.github/workflows/ci.yml` には `fmt` / `clippy` / `test` / `doc` / `dep-audit` の各ジョブが
個別に存在するが、「全て通過」を単一の成否として判定する仕組みがなかった。
individual ジョブ名を required status check として個別に登録する運用は、ジョブの追加・改名の
たびに required check 設定を同期させる必要があり、同期漏れで CI 赤のままマージできる状態が
発生しうる（設定腐敗）。

### 設計

`.github/workflows/ci.yml` に `ci-complete` ジョブを追加し、判定対象ジョブ全てを `needs` に
列挙して結果を集約する。

- **fail-closed**: `if: always()` で `needs` のジョブが失敗してもゲート自体は実行し、
  `success` 以外（`failure` / `cancelled` / `skipped`）を一律「未完遂」として扱う。
  GitHub の required status check は「skipped は pass 扱いになりうる」という既知の落とし穴が
  あるため、ゲート側で明示的に弾く。
- **判定対象**: REQ-14 が明記する 3 条件（`fmt` / `clippy` / `test`）に加え、
  - `doc`（TASK-11.2-2、#76: `missing_docs` + rustdoc lint の機械強制）
  - `dep-audit`（TASK-15.2、#17: 依存監査、REQ-15）

  も CI 上の既存の品質ゲートであるため判定に含める。「CI が全て通過」を実質的な意味で
  満たすには、spec が明記した 3 条件だけでなくリポジトリが実際に運用している全ゲートを
  対象にする必要があると判断した。
- **依存ゼロ**: `ci-complete` ジョブ自体は checkout も外部 action も使わず、シェル組み込みの
  みで判定する（サプライチェーン表面ゼロ、pay-for-what-you-use と整合）。
- **schedule 除外**: `schedule` イベント（日次 dep-audit のみ実行）では `fmt`/`clippy`/`test`
  が意図的に `skipped` になる（`doc` ジョブには schedule 除外の `if` がなく実際には実行される
  既存挙動だが、`ci-complete` 自体が schedule 時は丸ごとスキップされるため fail-closed 判定の
  ロジックには影響しない）ため、`ci-complete` 自体も schedule 時は実行しない
  （`if: github.event_name != 'schedule'`）。required status check は `pull_request`/`push`
  イベントのみを対象にすればよい。

### ジョブ追加・改名時の運用

新しい品質ゲートジョブを追加する場合、`ci-complete` の `needs` と判定ステップの `env` /
ループ対象に 1 行追加するだけで判定対象を拡張できる。既存ジョブを改名する場合は
`ci-complete` の `needs` と `scripts/setup-required-checks.sh` の `REQUIRED_CHECK_NAME`
（`ci-complete` 自体は改名しない限り不変）を確認する。`ci-complete` というジョブ名自体を
改名する場合は、本ジョブ名を参照する `scripts/setup-required-checks.sh` の
`REQUIRED_CHECK_NAME` を同時に更新すること。

## 実装: required status check の設定

`scripts/setup-required-checks.sh` が default branch（通常 `main`）の repository ruleset に
`ci-complete` を required status check として設定する。詳細は `scripts/README.md` を参照。

- 本タスクでは required_status_checks のみを設定する。
- PR 必須化・人間承認必須・force push 禁止などの追加ルールは意図的に含めない
  （TASK-14.3、#41、担当: 人間、のスコープ）。
- 実装時点（2026-07-16）で main ブランチは無保護（branch protection 404 / ruleset 0 件）
  だったため、本スクリプトの実行により初めて `ci-complete` が必須化される。
  管理者権限を持つトークンで `gh` にログインした状態でのみ成功する。403 になる場合は
  リポジトリ管理者が手動で実行する必要がある。

## 危険な `unsafe` パターンの機械的ブロックについて

REQ-14 の「危険な `unsafe` パターンをビルド段階で機械的にブロックする多層防御」は
TASK-14.2（#40）で実施済み。ルート `Cargo.toml` の `[workspace.lints.clippy]` に
forbid（`#[allow]` による抑制も不可）/ deny（局所例外可）の 2 層で lint を設定した。
選定根拠・ネガティブ検証の記録は `docs/design/unsafe-deny-lints.md` を参照。
`clippy -- -D warnings` が `ci-complete` の判定対象に含まれるため、当該 lint の違反は
`clippy` ジョブの失敗として自動的に本ゲートに反映される（CI ワークフロー自体の変更は不要）。

## 受け入れ基準との対応

`docs/spec/04-requirements.md` REQ-14 の受け入れ基準（抜粋）:

- [x] AI が生成した変更は、`cargo test` / `clippy -- -D warnings` / `fmt --check` の全通過を
      必須条件としてマージされる → `ci-complete` + `scripts/setup-required-checks.sh`
- [x] 危険な `unsafe` パターンが `cargo clippy` の deny lint で機械的に検出される
      → TASK-14.2（#40）、ルート `Cargo.toml` の `[workspace.lints.clippy]` +
      `docs/design/unsafe-deny-lints.md`
- [ ] 自律実装のマージには、CI 通過に加えてレビューゲート（人間承認または追加レビュー）を
      経る運用が定義されている → TASK-14.3（#41）のスコープ

CI が実際に赤くなるケースでゲートが機能することを確認する受け入れテストは、
TASK-14.3（#41）の受け入れテストの一部として実施する。
