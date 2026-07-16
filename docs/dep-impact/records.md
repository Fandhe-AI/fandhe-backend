# 依存インパクト記録台帳

`docs/dep-impact/README.md` の運用手順に従い、`crates/plugin-*` 追加・変更時の依存
インパクト計測結果を追記する。エントリは新しい順に追加する。

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
