# イシュー #473: compression 有効構成の並行負荷下 E2E p99 比較 — 実測レポート

対象: `crates/core`（`finalize_response` の `spawn_blocking` オフロード、
PR #471 / イシュー #468）+ `crates/plugin-compression`
（`CompressionConfigBuilder::blocking_threshold`）。

背景: PR #471（イシュー #468）で巨大応答の gzip 圧縮を `spawn_blocking` へ
切り離す際の採否判定はマイクロベンチ（`compress_body` 単体の所要時間 vs
ディスパッチ往復コスト）のみに基づいており、`benches/reports/
issue468-compression-blocking.md` 5 節が明示的にスコープ外としていた
「並行負荷下の E2E p99 比較」を、本イシューで実施した。

## 1. 実行コマンド

```bash
bash benches/compression-e2e-exclusive.sh
```

（内部で `cargo build --release --example compression_e2e_bench
-p fandhe-backend-core --features compression` → 静穏確認 →
`benches/compression-e2e-bench.sh` の順に実行。専有ロック・静穏確認込み。
`RUNS=5 DURATION=15s CONNECTIONS=128 LARGE_CONNECTIONS=32`、既定パラメータ。）

事前に `RUNS=3 DURATION=3s CONNECTIONS=16 LARGE_CONNECTIONS=8` の短縮
スモークで配線を確認済み（構成 A/B 双方で `/large` の `Content-Encoding:
gzip` を目視確認、`compression-e2e-bench.sh` の動作確認）。

## 2. 実行環境（`snapshot_environment` 出力）

```
snapshot_label=before
snapshot_time=2026-08-01T16:05:42Z
snapshot_commit=7d4c9e856a68ebdb64a72ab683ce17528050460b
snapshot_nproc=12
snapshot_loadavg1=0.87
snapshot_busy_processes=none

snapshot_label=after
snapshot_time=2026-08-01T16:08:21Z
snapshot_commit=7d4c9e856a68ebdb64a72ab683ce17528050460b
snapshot_nproc=12
snapshot_loadavg1=14.66
snapshot_busy_processes=none
```

計測開始前は `benches/lib/exclusive.sh` の静穏確認（`LOAD1_MAX=1.0`）を
通過済み。計測後の loadavig 上昇（14.66）は `oha` 自体の負荷生成コスト
（`LARGE_CONNECTIONS=32` + `CONNECTIONS=128` の同時接続）によるもので、
計測中の専有性（他プロセスによる干渉なし）は静穏確認と `flock` 相互排他で
担保済み。

計測対象: `crates/core/examples/compression_e2e_bench.rs`
（`worker_threads = 4` 固定、マイクロベンチ #468 と同一設定）。
`/large` = 256 KiB `application/json`、`/small` = 4 KiB `application/json`
（`min_size`（1024 バイト）以上・`blocking_threshold`（64 KiB）未満に固定、
両構成で常にインライン圧縮）。

## 3. 実測結果（raw 値・中央値）

### 構成 A（offload、既定 `blocking_threshold=64 KiB`）

#### GET /small（背景負荷: GET /large 同時実行中）

| | run1 | run2 | run3 | run4 | run5 | 中央値 |
|---|---|---|---|---|---|---|
| RPS | 83927.5 | 74420.0 | 58142.2 | 68693.0 | 50292.4 | **68693.0** |
| p50 (ms) | 1.434 | 1.509 | 1.834 | 1.615 | 1.949 | 1.615 |
| p95 (ms) | 2.832 | 3.608 | 5.131 | 4.033 | 6.372 | 4.033 |
| p99 (ms) | 3.721 | 5.644 | 8.049 | 6.437 | 9.162 | **6.437** |

#### GET /large（背景負荷本体）

| | run1 | run2 | run3 | run4 | run5 | 中央値 |
|---|---|---|---|---|---|---|
| RPS | 15382.2 | 13683.4 | 11023.3 | 12799.5 | 9617.8 | **12799.5** |
| p50 (ms) | 1.980 | 2.090 | 2.477 | 2.193 | 2.727 | 2.193 |
| p95 (ms) | 3.495 | 4.463 | 6.188 | 4.990 | 7.400 | 4.990 |
| p99 (ms) | 4.500 | 6.736 | 9.393 | 7.680 | 10.356 | **7.680** |

### 構成 B（inline、`blocking_threshold=max`）

#### GET /small（背景負荷: GET /large 同時実行中）

| | run1 | run2 | run3 | run4 | run5 | 中央値 |
|---|---|---|---|---|---|---|
| RPS | 21007.2 | 17011.5 | 30726.2 | 30369.9 | 31743.2 | **30369.9** |
| p50 (ms) | 5.721 | 6.707 | 4.082 | 4.150 | 3.996 | 4.150 |
| p95 (ms) | 11.342 | 15.617 | 7.020 | 7.156 | 6.800 | 7.156 |
| p99 (ms) | 15.490 | 22.589 | 8.810 | 8.601 | 8.017 | **8.810** |

#### GET /large（背景負荷本体）

| | run1 | run2 | run3 | run4 | run5 | 中央値 |
|---|---|---|---|---|---|---|
| RPS | 4895.8 | 3989.3 | 7125.6 | 7005.1 | 7325.0 | **7005.1** |
| p50 (ms) | 6.124 | 7.132 | 4.414 | 4.514 | 4.347 | 4.514 |
| p95 (ms) | 11.922 | 16.489 | 7.287 | 7.439 | 7.070 | 7.439 |
| p99 (ms) | 16.071 | 23.565 | 9.102 | 8.856 | 8.254 | **9.102** |

## 4. 構成 A vs 構成 B 比較表

| 指標 | 構成 A（offload） | 構成 B（inline） | 差分（A が B に対して） |
|------|------|------|------|
| /small RPS 中央値 | 68693.0 | 30369.9 | **+126.2%**（offload が高い） |
| /small p99 中央値 | 6.437 ms | 8.810 ms | **-26.9%**（offload が低い＝良い） |
| /large RPS 中央値 | 12799.5 | 7005.1 | **+82.7%**（offload が高い） |
| /large p99 中央値 | 7.680 ms | 9.102 ms | **-15.6%**（offload が低い＝良い） |

## 5. しきい値判定（`docs/design/plugin-boundary.md` 5.10.7 節「E2E 検証」節と対応）

事前定義した判定基準（実装計画）:

> **しきい値 64 KiB 維持**: 構成 A の /small p99 中央値が構成 B 以下
> （またはノイズ範囲内 +10% 未満の差）であり、かつ構成 A の /large RPS・
> p99 が構成 B 比で大幅悪化（20% 超）していないこと

実測結果は両条件を明確に満たす。加えて事前予想（「/large 自体のディスパッチ
コストによる劣化有無の確認」）に反し、構成 A（offload）は /small だけでなく
**/large 自体の RPS・p99 も構成 B（inline）より優れていた**（RPS +82.7%・p99
-15.6%）。これは、インライン圧縮が tokio ワーカスレッドを長時間占有すること
で `/large` 自身の後続リクエスト処理にも遅延が波及していたためと考えられる
（ワーカスレッドを 4 本に絞った構成での観測。実運用のワーカ数が多い環境では
効果が相対的に小さくなる可能性があるが、方向性が逆転する要因は見当たらない）。

**判定: しきい値 64 KiB を維持する。** 既定値変更は不要と判断した。

## 6. 週次 CI（`bench-schedule.yml`）への組み込み判断

**不採用**。理由（実装計画・`benches/README.md`「compression-e2e-bench.sh /
compression-e2e-exclusive.sh」節と同一）:

- `bench-schedule.yml` の worst-case 予算は既に大きく、E2E 圧縮計測
  （2 構成 × RUNS 回 × 混在負荷、本計測で約 3 分弱）の追加は self-hosted
  runner の負荷抑制方針（`.claude/rules/ci.md`）と衝突する
- compression は opt-in feature であり、しきい値は
  `CompressionConfigBuilder::blocking_threshold` で利用者が調整可能。
  コア性能（REQ-1/NFR-1）のような常時監視対象の要件基準ではない
- 再現手順を `benches/README.md` に常設したため、`crates/plugin-compression` /
  `crates/core` の `finalize_response` 経路に変更が入った際は手動再実行で
  退行確認できる

将来、実測が「常設監視の価値が高い」ことを示す状況になった場合は、別イシューと
して再検討する（本判断は恒久的な決定ではない）。

## 7. 再現手順

```bash
# 動作確認用の短縮スモーク
RUNS=3 DURATION=3s CONNECTIONS=16 LARGE_CONNECTIONS=8 bash benches/compression-e2e-bench.sh

# 本計測（専有実行枠）
bash benches/compression-e2e-exclusive.sh
```

`crates/core/examples/compression_e2e_bench.rs` を編集した場合は本ファイルの
数値を再計測・更新すること。
