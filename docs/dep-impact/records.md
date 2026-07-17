# 依存インパクト記録台帳

`docs/dep-impact/README.md` の運用手順に従い、`crates/plugin-*` 追加・変更時の依存
インパクト計測結果を追記する。エントリは新しい順に追加する。

## 2026-07-17 — `crates/plugin-graphql` を実 GraphQL 実行へ実装（#38、TASK-5.1）

TASK-2.4（#21）のスタブ（`bf-http` のみ依存、外部依存 0 件）に `async-graphql = "7"`
（`default-features = false`）・`serde`（derive）・`serde_json` を追加し、実クエリ実行を
実装。事前見積もり（実装計画、PoC 実測）は「+95 クレート、バイナリ +1.51MB」だったが、
実測は以下のとおり（`default-features = false` により playground/graphiql 等の
開発 UI 系依存を含めていないため見積もりより少ない）。

`bf-plugin-graphql` 自身が引き込む新規クレート数（`bf-http`・自身を除く）:

```
$ cargo tree -p bf-plugin-graphql -e normal --prefix none | sed 's/ (\*)$//' | sort -u \
    | grep -v '^bf-http \|^bf-plugin-graphql ' | wc -l
76
```

pay-for-what-you-use の受け入れ条件（`backend-framework-core` が `graphql` feature
無効時に `async-graphql`/`bf-plugin-graphql` 系依存を一切解決しないこと）は
`bash scripts/pay-for-what-you-use-check.sh` の全項目（cargo tree 陰性/陽性・
cargo geiger・バイナリサイズ・全構成ビルド）で PASS を確認済み:

```
[PASS] b: cargo tree 検証（無効構成） — 全プラグインクレートが依存グラフから 0 件
[PASS] b: cargo tree 検証（有効構成 graphql） — bf-plugin-graphql のみが出現し他プラグインの混入なし
[PASS] c: cargo geiger 検証 — 無効構成の依存グラフにプラグインクレートは 0 件（unsafe 計上対象なし）
[PASS] d: バイナリサイズ計測 — 無効構成 798680 bytes <= 有効構成 9106696 bytes（差分 8308016 bytes、
    `--all-features` ビルドとの比較のため他プラグイン分を含む）
```

計測コマンド: `bash scripts/dep-impact.sh`（workspace 全体の参考値）

### 依存クレート数（workspace メンバー除外・重複バージョンは union で 1 件として計上）

| feature 構成 | 依存クレート数（直前エントリ差分） |
|---|---|
| --no-default-features | 265（+37） |
| default | 265（+37） |
| --all-features | 265（+37） |

`crates/plugin-*` を含む全 workspace メンバーをそれぞれ自身の既定 feature で計測する
方式であり、`crates/core` 単体の pay-for-what-you-use 遵守を測るものではない点は
既存エントリと同様（`bf-plugin-graphql` 自身の新規依存が上記 76 件、workspace 全体の
union 差分が +37 なのは `async-graphql`/`serde`/`serde_json`/`thiserror` 系の一部が
既に他プラグイン（`plugin-webrtc`・`plugin-openapi` 等）経由で workspace の依存
グラフに存在済みで union 上重複計上されないため）。

`crates/core` の dev-dependencies（テスト専用、`async-graphql`（`dynamic-schema`
feature））はリリースビルドに含まれないため pay-for-what-you-use の対象外
（`.claude/rules/pay-for-what-you-use.md`）。

### リリースバイナリサイズ

| bin | サイズ (bytes) |
|---|---|
| axum-ref | 1374024 |

`axum-ref` は `bf-plugin-graphql` に依存しないため、直前エントリとの差分は
ビルド環境ノイズ（既存エントリと同様の注記）。

### unsafe 件数

`cargo-geiger` は `dep-impact.sh` 標準実行（`cargo geiger --all-features`）では
webrtc 系の巨大依存グラフ解決に失敗したため未計測。pay-for-what-you-use-check.sh の
`c: cargo geiger 検証` で「`graphql` feature 無効構成に対象クレート 0 件（unsafe
計上対象なし）」は個別に確認済み（上記参照）。

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

## 2026-07-17 — `crates/plugin-websocket` 追加（#22、TASK-4.1）

`bf-plugin-websocket`（`tokio-tungstenite = "0.30"`（`default-features = false`,
`handshake` feature のみ）・`futures-util`（`sink`/`std` feature のみ）依存、
workspace 内クレートへの依存は `bf-http` のみ）を追加。`tokio-tungstenite` の
`handshake` feature は推移的に `tungstenite`（`sha1`/`data-encoding`/`http`/
`httparse`/`rand` 等）を引き込むため、workspace 全体の依存クレート数（本測定は
`crates/plugin-*` を含む全 workspace メンバーをそれぞれ自身の既定 feature で
計測する方式であり、`crates/core` 単体の pay-for-what-you-use 遵守を測るもの
ではない点に注意）が増加している。

`crates/core` 側の pay-for-what-you-use は個別に確認済み: `cargo tree -p
backend-framework-core`（既定構成）に `bf-plugin-websocket`・`tokio-tungstenite`
は一切現れず、`cargo tree -p backend-framework-core --features websocket` で
初めて出現する（`docs/design/plugin-boundary.md` 6 節）。TLS 系（`native-tls`・
`rustls`）・`connect`（クライアント接続関数群）feature は無効のままであり、
これらの重い依存は一切増えていない。

計測コマンド: `bash scripts/dep-impact.sh`

### 依存クレート数（workspace メンバー除外・重複バージョンは union で 1 件として計上）

| feature 構成 | 依存クレート数（直前エントリ差分） |
|---|---|
| --no-default-features | 73（+19） |
| default | 73（+19） |
| --all-features | 73（+19） |

feature を持つ workspace メンバーが `crates/core`（`webrtc-proxy`/`websocket`）
のみのため、`crates/plugin-websocket` 自身は常に自身の既定 feature（`handshake`
のみ、`default-features = false`）でツリーに現れ、3 構成とも同一値になる。
直前エントリ（54）比 +19 は `tokio-tungstenite`/`tungstenite` とその推移依存
（`sha1`・`digest`・`data-encoding`・`http`・`httparse`・`rand` 系等）。

### リリースバイナリサイズ

| bin | サイズ (bytes) | 直前エントリ差分 |
|---|---|---|
| axum-ref | 1356936 | 0（`axum-ref` は `bf-plugin-websocket` に依存しない） |

### unsafe 件数

`cargo-geiger` 実行が環境要因で失敗（`scripts/unsafe-triage.sh` の対象は
workspace 自身のコードのみで、`crates/plugin-websocket` は 0 件、
`scripts/unsafe-baseline.json` の既定値と一致。依存 crate 側の unsafe 増減は
本記録の対象外）。

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
