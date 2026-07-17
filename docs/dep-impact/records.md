# 依存インパクト記録台帳

`docs/dep-impact/README.md` の運用手順に従い、`crates/plugin-*` 追加・変更時の依存
インパクト計測結果を追記する。エントリは新しい順に追加する。

## 2026-07-17 — `crates/plugin-webrtc` 追加（#26、TASK-8.1）

`bf-plugin-webrtc`（`webrtc = "0.17"`（0.17.1 系）・`serde_json`・`tokio`（`time`
のみ）依存、workspace 内クレートへの依存は `bf-http` のみ）を追加。PoC-5 の実測どおり
`webrtc` は依存 +189 クレート級のインパクトを持つ。

`scripts/dep-impact.sh` は `cargo metadata` がワークスペース全体の依存グラフを解決する
（`--features`/`--no-default-features` を渡してもワークスペースメンバー自体の
manifest 解決には影響しない）という既知の制約により、`bf-plugin-webrtc` を workspace
メンバーとして追加した時点で 3 構成すべてに `webrtc` 系依存が計上され、
「feature 無効時の依存数が変化しないこと」を本スクリプト単体では区別できない
（`bf-plugin-webrtc-proxy` 追加時も同様の制約を受けていた）。

pay-for-what-you-use の受け入れ条件（`backend-framework-core` が `webrtc` feature
無効時に `webrtc` 系依存を一切解決しないこと）は、より正確な
`cargo tree -p backend-framework-core` で機械検証した:

```
$ cargo tree -p backend-framework-core | grep -c webrtc
0
$ cargo tree -p backend-framework-core --features webrtc | grep -c webrtc
23
```

計測コマンド: `bash scripts/dep-impact.sh`（workspace 全体の参考値。上記 `cargo tree`
差分検証と併読すること）

### 依存クレート数（workspace メンバー除外・重複バージョンは union で 1 件として計上）

| feature 構成 | 依存クレート数（ベースライン差分） |
|---|---|
| --no-default-features | 228（+174） |
| default | 228（+174） |
| --all-features | 228（+174） |

上記の理由により 3 構成とも同一値（スクリプトの既知の制約。`webrtc` feature 単体の
実インパクトは `cargo tree -p backend-framework-core --features webrtc` の差分
（0 → 23 件、推移依存込みの重複除外前カウント）で判断すること）。

### リリースバイナリサイズ

| bin | サイズ (bytes) | ベースライン差分 |
|---|---|---|
| axum-ref | 1373104 | +14808（`bf-plugin-webrtc` に依存しないが、workspace 全体の
  `Cargo.lock` 更新に伴うビルド環境差によるノイズ） |

### unsafe 件数

`cargo geiger` の実行が本環境で失敗したため未計測（`scripts/unsafe-triage.sh` による
テキストベース走査では自コード（`crates/plugin-webrtc`）の `unsafe` は 0 件。
依存側 `unsafe` 増分（`webrtc-rs` 由来）は PoC-5 実測（約 2.2 倍）を参照し、恒久的な
計測は TASK-8.4（#29、攻撃表面評価）のスコープとする）。

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
