# scripts/ — CI・運用スクリプト

`docs/spec/05-tasks.md` TASK-15.2（#17）の成果物。依存監査（`cargo audit` / `cargo deny
check`）と依存インパクト計測を、feature 構成の増減に追従できる形でまとめたスクリプト集。

## スクリプト一覧

| スクリプト | 用途 | CI との対応 |
|-----------|------|-------------|
| `dep-audit.sh` | 全 feature 構成で `cargo audit`・`cargo deny check` を実行する依存監査 | `.github/workflows/ci.yml` の `dep-audit` ジョブから呼ばれる |
| `dep-impact.sh` | feature 構成ごとの依存クレート数・リリースバイナリサイズ・`unsafe` 件数を計測し markdown 表を出力する | CI からは呼ばれない。plugin 追加 PR でのローカル実行を想定（`docs/dep-impact/README.md` 参照） |

## 前提ツール

いずれのスクリプトも前提ツールを自動ダウンロードしない（`.claude/rules/security.md`・
`benches/` と同じ方針）。冒頭で存在検査を行い、見つからない場合は導入コマンドを案内して
終了する。

| ツール | 用途 | 導入コマンド |
|--------|------|-------------|
| `cargo-deny` | ライセンス・出所・重複バージョン監査 | `cargo install --locked cargo-deny@0.19.8` |
| `cargo-audit` | RustSec advisory DB による既知脆弱性検知 | `cargo install --locked cargo-audit@0.22.2` |
| `jq` | `cargo metadata` の JSON 解析 | OS のパッケージマネージャ（例: `apt install jq`） |
| `cargo-geiger`（`dep-impact.sh` のみ、任意） | `unsafe` 件数の計測 | `cargo install --locked cargo-geiger` |

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
