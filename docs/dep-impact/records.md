# 依存インパクト記録台帳

`docs/dep-impact/README.md` の運用手順に従い、`crates/plugin-*` 追加・変更時の依存
インパクト計測結果を追記する。エントリは新しい順に追加する。

## 2026-07-17 — `crates/plugin-openapi` 追加（#30、TASK-3.1）

`bf-plugin-openapi`（`utoipa = "5"`（default features）・`serde`（derive）依存、
workspace 内クレートへの依存なし）を追加。本クレートは他クレートから参照されない
独立プラグイン境界であり、`utoipa` 系依存は本クレート自身の構成にのみ現れる
（`.claude/rules/pay-for-what-you-use.md`）。`axum-ref` の bin サイズは workspace
全体のビルドグラフ変化の副作用でわずかに変動しているが、`bf-plugin-openapi` を
参照していないため実質的な影響はない。

計測コマンド: `bash scripts/dep-impact.sh`

### 依存クレート数（workspace メンバー除外・重複バージョンは union で 1 件として計上）

| feature 構成 | 依存クレート数（ベースライン差分） |
|---|---|
| --no-default-features | 54（+5） |
| default | 54（+5） |
| --all-features | 54（+5） |

feature を持つクレートが存在しないため 3 構成とも同一値。差分 5 件は
`utoipa` / `utoipa-gen` / `syn`（version 差分により union 上は別件計上され得る）等、
`bf-plugin-openapi` の直接・推移依存。

### リリースバイナリサイズ

| bin | サイズ (bytes) | ベースライン差分 |
|---|---|---|
| axum-ref | 1356936 | -1360（ビルド環境差によるノイズ。axum-ref は bf-plugin-openapi に依存しない） |

### unsafe 件数

`cargo-geiger` 未導入のため未計測（`scripts/unsafe-triage.sh` によるテキストベース
走査では `bf-plugin-openapi` の unsafe 件数は 0、`scripts/unsafe-baseline.json` の
既定値と一致）。

## 2026-07-16 — ベースライン（#17、TASK-15.2）

`crates/plugin-*` が未着手（TASK-2.1 以降）の現行 workspace（`crates/core` /
`crates/http` / `crates/axum-ref` のみ）で計測したベースライン。以降のプラグイン追加 PR は
このベースラインとの差分を確認する。

計測コマンド: `bash scripts/dep-impact.sh`

### 依存クレート数（workspace メンバー除外）

| feature 構成 | 依存クレート数 |
|---|---|
| --no-default-features | 49 |
| default | 49 |
| --all-features | 49 |

feature を持つクレートが存在しないため 3 構成とも同一値。

### リリースバイナリサイズ

| bin | サイズ (bytes) |
|---|---|
| axum-ref | 1358296 |

`crates/core` は現時点でライブラリのみ（実行可能 bin なし。TASK-1.4-2 / TASK-1.5 以降で
追加予定）。

### unsafe 件数

`cargo-geiger` 未導入のため未計測（導入後に追記する）。
