---
paths:
  - "lefthook.yml"
  - "lefthook-local.yml"
  - "scripts/commit-msg-check.sh"
description: >
  lefthook.yml・scripts/commit-msg-check.sh を編集する際の方針。フックは staged 限定で一瞬で
  終わる検査とコミット単位でしか意味がない検査に限り、テスト・ビルド・clippy・全体 lint や
  CI と重複する検査は CI（.github/workflows/ci.yml）へ置く。時間予算・判断手順・計測方法を規定する。
---

# Git hooks（lefthook）編集ルール

## 目的

フックはコミットのたびに実行されるため、遅いフックはそのまま開発速度の低下になる。
フックはローカルでの早期検知専用であり、CI の代替ではない。**開発速度を落とさないことを最優先**し、
重い検査・網羅的な検査は CI（`.github/workflows/ci.yml`）に任せる。

## 時間予算

| フック | 予算 | 判定方法 |
| --- | --- | --- |
| pre-commit | 全ジョブ合計 1 秒未満 | `lefthook run pre-commit --all-files` の `summary: (done in N seconds)` |
| commit-msg | 即時（外部コマンド・ネットワーク不可） | 実コミット時に表示される commit-msg の summary（メッセージファイルを引数に取るため `--all-files` では計測しない） |
| pre-push・post-checkout・post-merge 等 | 原則追加しない | 追加する場合は理由と実測値を PR 本文に記載する |

`--all-files` は glob に合う全追跡ファイルを対象にする最悪ケースであり、実際のコミット（staged のみ）は
これより速い。

## フックに置かないもの（→ `ci.yml` へ）

- テスト（`cargo test` / `cargo nextest` / doc test）・ビルド・`cargo clippy`・`cargo doc`
  （`ci.yml` の `test` / `clippy` / `doc` ジョブ）
- 依存監査（`cargo audit` / `cargo deny`、`dep-audit` ジョブ）・`unsafe` トリアージ・
  pay-for-what-you-use 検証などのスクリプト群（`unsafe-triage` / `pay-for-what-you-use` ジョブ等）
- workspace 全体を走査する fmt / lint（`cargo fmt --all --check` を含む）
- 依存クレートの取得・ネットワークアクセス
- `ci.yml` のジョブと同じコマンドを同じ範囲で実行するもの

## フックに置いてよいもの

- `{staged_files}` に限定し、`glob` / `exclude` で対象を絞った 1 秒未満の検査
- CI ではコミット単位で検知できない、または push 後では遅いもの
  - commit-msg（`scripts/commit-msg-check.sh`）: `ci.yml` にはコミットメッセージを検証するジョブがない

### 明示的な例外

- pre-commit の `fmt`（staged の `crates/**/*.rs` に対する `rustfmt --check --edition 2024`）は
  `ci.yml` の `fmt` ジョブ（`cargo fmt --all --check`）と重複するが、staged ファイル限定で
  1 ファイルあたり 0.04〜0.09 秒（`--all-files` の最悪ケースでも 0.37 秒）のため維持する
  （2026-10-11 計測・決定）。従来の `cargo fmt --all --check` は staged ファイル数に関係なく
  workspace 全体を走査し毎回 0.3 秒程度かかっていた
  - 対象範囲は CI と揃える: root workspace のメンバー（`Cargo.toml` の `members = ["crates/*"]`）
    に限定し、同 `exclude` の独立 workspace（`crates/http/fuzz`・`crates/plugin-webrtc/tests-e2e`）を
    除外する。`benches/`・`examples/`・`templates/`・`scripts/tests/fixtures/` も CI の fmt 対象外のため
    glob に含めない。`Cargo.toml` の `members` / `exclude` を変えたら `lefthook.yml` の
    `glob` / `exclude` も追随させる
  - `--edition` は `Cargo.toml` の `[workspace.package] edition` と揃える。メンバー間で edition が
    混在した場合や `rustfmt.toml` を導入した場合は、本方式の前提が崩れるため構成を見直す

## 書き方の規約

- `pre-commit` の `parallel: true` を維持する（直列化しない）
- `{staged_files}` と `glob` / `exclude` で対象を絞る。外部取得物の `.agents/skills/**`・
  `.claude/skills/**` を対象に含む検査を追加する場合はそれらを除外する
- 実体が長くなる場合は `scripts/` 配下へ切り出し、`lefthook.yml` は呼び出しのみにする
  （`scripts/commit-msg-check.sh` と同じ形）
- 条件付き実行が必要な場合は lefthook の `skip` / `only` を使ってよい
  （`.claude/skills/lefthook/references/examples/skip.md`）
- `assert_lefthook_installed: true`（lefthook 不在時にコミットを失敗させる fail-closed 設定）は維持する

## 検査を追加するときの判断手順

1. その検査は `ci.yml` で既に実施されているか
    - 実施済みで、staged 限定にしても 1 秒以上かかる → フックに入れない
2. staged ファイルに限定できるか
    - できない → CI へ置く
3. CI へ置く場合は `ci.yml` にジョブを追加し、集約ジョブ `ci-complete` の `needs` と結果判定にも必ず追加する
    - required status check の定義は `scripts/setup-required-checks.sh`、runner は [[ci]] に従う
4. 編集後に以下を実行し、summary の合計時間をユーザーへ報告する（1 秒以上なら構成を見直す）

```bash
time lefthook run pre-commit --all-files
```

## 手元で CI 相当を確認する

フックで強制しないだけで、[CONTRIBUTING.md](../../CONTRIBUTING.md) の「検証」手順（fmt / clippy / test を
通してから PR を出す）というローカルでの確認の規約自体は変わらない。必要に応じて以下を手動で実行する
（いずれも `Makefile` のターゲット。`make help` で一覧）。

```bash
make lint       # cargo fmt --all --check + cargo clippy（CI の fmt / clippy ジョブと同一コマンド）
make test-all   # cargo test --workspace --all-features（doc test 含む。CI の test ジョブは nextest + doc test）
make audit      # scripts/dep-audit.sh（CI の dep-audit ジョブと同一スクリプト）
```

## バイパス禁止

遅いからといって `--no-verify` や `LEFTHOOK=0` で回避しない（[[conventional-commits]]）。
遅いフックはフック側を直す（検査を CI へ移す・staged 限定にする・対象 glob を絞る）。
