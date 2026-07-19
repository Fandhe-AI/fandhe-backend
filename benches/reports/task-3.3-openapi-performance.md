# TASK-3.3 性能受け入れ計測レポート — OpenAPI 生成有無での `GET /health` 性能有意差

> 注記: 本レポートの初回記録（2026-07-17）は 2026-07 の crate・import 一括改名（#202）
> 以前の実測記録であり、旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` /
> `bf-plugin-*` 等）表記のまま保持している。実測値本文は改変しない
> （`docs/design/framework-naming.md` 7 節）。
>
> **最終判定は末尾の「再計測（#259）」節を参照（PASS）。** 初回の BLOCKED 判定は
> `openapi` feature 配線（#256）・5 エンドポイント実サービング（#257）の完了後、
> 2026-07-19 の再計測で解消した。以下の初回記録は経緯としてそのまま保持する。

Issue #32 の成果物。REQ-3 受け入れ基準「OpenAPI 生成の有無で `GET /health` 相当の
ランタイム性能指標（RPS・p95）に有意差がない（±5% 以内）」の検証結果。

## 実施日時・環境

- 実施日時: 2026-07-17（JST）
- OS: Linux 7.0.0-27-generic x86_64（Ubuntu）
- CPU コア数: 12（`nproc`）
- rustc / cargo: 1.96.0（stable, 2026-05-25）
- 計測ツール: `oha`（導入済み、`which oha` で確認済み）

## 判定結果: BLOCKED

`crates/core` にサーバ側 `openapi` feature（`openapi = ["dep:bf-plugin-openapi"]`
相当、`GET /openapi.json` 静的サービングハンドラ）が本イシュー着手時点で存在しない
ため、「OpenAPI 生成有効／無効」の 2 構成を A/B 比較する計測そのものが実施不能である。

```
$ cargo metadata --format-version 1 --no-deps | \
    jq -r '.packages[] | select(.name == "backend-framework-core") | .features | keys[]'
webrtc-proxy
websocket
graphql
default
```

`openapi` feature が一覧に存在しない。`crates/plugin-openapi/src/lib.rs`・
`src/embed.rs` の doc comment は当該配線を TASK-2.1（#18）のスコープとして委ねて
いるが、TASK-2.1 は `webrtc-proxy` feature のみを確立してクローズしており、
`openapi` の配線・後継 Issue のいずれも存在しない（詳細な経緯は
`docs/acceptance/req3-openapi-generation.md` を参照）。

**理由**: `openapi` feature が存在しない以上、「OpenAPI 生成有効（`--features
openapi` でビルドし `GET /openapi.json` を実サービングする構成）」を作れず、
比較対象の片方を欠く。加えて `crates/core/examples/minimal.rs` は現状 `GET /` の
みを登録しており、`GET /health` 自体も未登録である（対象パスの計測用エンドポイント
も別途 #32 スコープ内で追加が必要）。両者が揃わない状態で「有意差なし」を実測せずに
PASS と記録することは、`.claude/rules/security.md` のフェイルクローズ原則（判定不能を
PASS と偽らない）に反するため、`task-1.6-1-performance.md`（#71）の前例に倣い
BLOCKED として正直に記録する。

## 再計測手順（`openapi` feature 配線後）

フォローアップ Issue（`docs/acceptance/req3-openapi-generation.md` 「フォローアップ」
節 1）で `crates/core` に `openapi` feature が配線された後、以下の手順で再計測する:

```bash
# baseline: openapi feature 無効
cargo build --release --example minimal -p backend-framework-core --no-default-features
cp target/release/examples/minimal /tmp/minimal-baseline

# 対象: openapi feature 有効
cargo build --release --example minimal -p backend-framework-core --features openapi
cp target/release/examples/minimal /tmp/minimal-openapi

# 各構成で GET /health を RUNS=5 中央値方式で計測し RPS・p95 の相対差を確認する
# （benches/lib/common.sh の中央値算出関数を再利用する想定。ポートは分離する）
RUNS=5 /tmp/minimal-baseline &
oha -z 15s -c 128 --no-tui --output-format json http://127.0.0.1:3000/health

RUNS=5 /tmp/minimal-openapi &
oha -z 15s -c 128 --no-tui --output-format json http://127.0.0.1:3000/health
```

RPS・p95 の相対差が ±5% 以内であることを確認し、本レポートを更新して判定結果を
BLOCKED から PASS/FAIL に更新すること。`OPENAPI_JSON` は `include_str!` によるコンパ
イル時埋め込み定数の参照のみで実行時コストがない設計（`embed.rs` の doc comment）の
ため、`GET /openapi.json` のハンドラが `GET /health` のリクエスト処理経路に一切関与
しない限り有意差はほぼゼロになる見込みだが、実測せずに結論づけない。

---

## 再計測（#259、2026-07-19）

### 判定結果（再計測、#259）: PASS

RPS・p95 とも中央値の相対差が ±5% 以内（RPS +0.58%、p95 +1.59%）。

### 計測環境・条件

- 実施日時: 2026-07-19（UTC）、対象コミット: `12dbdc3`（origin/main 先端）
- OS: Linux 7.0.0-27-generic x86_64 / CPU 12 コア / rustc・cargo 1.96.0（stable）
- 計測ツール: `oha -z 15s -c 128 --no-tui --output-format json`（各 run 前に 2s の
  ウォームアップを実施・記録対象外）
- 計測対象: `crates/core/examples/openapi_endpoints.rs`（#257 の 5 エンドポイント実
  サービング。#259 で `#[cfg(feature = "openapi")]` の `Server::openapi()` 登録を追加）
  - baseline: `cargo build --release --example openapi_endpoints -p fandhe-backend-core`
    （feature なし。`GET /openapi.json` は 404。事前 curl で確認）
  - openapi: 同 `--features openapi`（`GET /openapi.json` が 200 で実サービング。
    事前 curl で確認。バイナリサイズ差 +8,816 bytes = openapi.json 埋め込み分）
- 測定エンドポイント: `GET /health`（`text/plain`、両構成で同一ハンドラ）
- 専有性: `benches/lib/exclusive.sh` の `acquire_exclusive_lock`（flock）+
  `wait_for_quiescence`（loadavg1 ≤ 1.0・他 cargo/rustc/oha 不在）を計測開始前に確認。
  計測開始時スナップショット: loadavg1=0.89
- 方式: RUNS=5、host contention によるドリフト対策として baseline/openapi を run ごと
  に交互（ペア）で計測し、RPS・p95 それぞれの中央値で比較（`benches/README.md` の
  複数回計測・中央値評価規約）

### 実測値

| run | baseline RPS | baseline p95 (s) | openapi RPS | openapi p95 (s) |
|-----|-------------|------------------|-------------|-----------------|
| 1 | （無効。注参照） | — | 149330.85 | 0.000916 |
| 2 | 143142.89 | 0.000925 | 143623.06 | 0.000923 |
| 3 | 150566.95 | 0.000906 | 141813.10 | 0.000935 |
| 4 | 153566.00 | 0.000908 | 143973.18 | 0.000929 |
| 5 | 142048.89 | 0.000931 | 144021.66 | 0.000917 |
| **中央値** | **143142.89** | **0.000908** | **143973.18** | **0.000923** |

- 相対差（openapi − baseline / baseline、中央値）: **RPS +0.58%、p95 +1.59%**
  → いずれも ±5% 以内で **PASS**
- 注（baseline run 1 の無効化）: 計測開始前の残存プロセス整理（先行計測の二重起動
  停止）が run 1 の baseline サーバプロセスに波及し、当該 run の計測値を取得できなかった。
  baseline は有効 4 run・openapi は 5 run で中央値を算出した（`benches/lib/common.sh` の
  規約が要求する最低 3 run は両構成とも充足）。判定を左右しない旨を含め、事実のまま記録
  する（フェイルクローズ、`.claude/rules/security.md`）
- 生ログ・実行手順は #259 の PR 参照（計測ドライバは exclusive.sh の関数を source する
  一時スクリプト。本レポートの条件・上表の値がそのまま再現手順になる）
