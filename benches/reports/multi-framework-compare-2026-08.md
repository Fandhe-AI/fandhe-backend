# 他フレームワーク横並び比較・axum 比受け入れ再計測レポート（2026-08）

`benches/bench-compare.sh`（本 PR で新設）による axum / fandhe-backend / actix-web / Rocket の
横並び計測と、`benches/bench-accept.sh` による axum 比受け入れ判定（REQ-1 / NFR-1 / NFR-2）の
再計測記録。**横並び比較は判定を持たない情報提供用**であり、受け入れ判定の baseline は
従来どおり axum-ref のみ（`benches/reports/task-1.6-1-performance.md` の後続記録）。

## 実施日時・環境

- 実施日時: 2026-08-26（UTC）
- 対象コミット: `bf8d2b4`（main、v0.4.0 公開後）
- OS: macOS（Darwin 25.6.0 arm64）
- CPU: Apple M4 Max（論理コア 16）、メモリ 64GB
- rustc / cargo: 1.96.0（stable, 2026-05-25）
- oha: 1.14.0
- 計測パラメータ: `RUNS=5 DURATION=15s CONNECTIONS=128`（既定値。各値は 5 回計測の中央値）
- 実行手順: `bench-compare.sh` → 直後に `SKIP_BUILD=1 bench-accept.sh` を同一ホストで順次実行
  （両者とも専有計測 wrapper・静穏確認は未使用の単発 run）

### 計測対象と版

| 名称 | バイナリ | フレームワーク版 | 備考 |
|------|---------|-----------------|------|
| axum | `target/release/axum-ref`（`crates/axum-ref`） | axum 0.8.9 / tokio 1.53.0 | 受け入れ判定の baseline |
| fandhe-backend | `target/release/examples/core-bench`（`crates/core/examples/core-bench.rs`） | fandhe-backend-core 0.4.0（ワークツリー） / tokio 1.53.0 | |
| actix-web | `benches/refs/target/release/actix-ref` | actix-web 4.15.0 | worker ごとの単一スレッドランタイム（既定構成） |
| rocket | `benches/refs/target/release/rocket-ref` | Rocket 0.5.1 | リクエストログを `Critical` に下げて計測 |

4 実装とも同一の 4 エンドポイント・同一のレスポンス body スキーマ・同一の `lto = true`
release プロファイルでビルドした（機能等価性・構成差の詳細は各 `src/main.rs` の doc comment）。

## 横並び比較（`bench-compare.sh`、判定なし）

### GET /health

| フレームワーク | RPS | p50 (ms) | p95 (ms) | p99 (ms) | RPS 比 | p99 比 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| axum | 174299 | 0.726 | 0.825 | 0.906 | 1 | 1 |
| fandhe-backend | 176071 | 0.717 | 0.81 | 0.901 | 1.01 | 1 |
| actix-web | 161598 | 0.779 | 0.875 | 1.143 | 0.93 | 1.26 |
| rocket | 165405 | 0.764 | 0.884 | 0.969 | 0.95 | 1.07 |

### GET /hello/{name}

| フレームワーク | RPS | p50 (ms) | p95 (ms) | p99 (ms) | RPS 比 | p99 比 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| axum | 173672 | 0.73 | 0.816 | 0.906 | 1 | 1 |
| fandhe-backend | 175893 | 0.718 | 0.807 | 0.9 | 1.01 | 0.99 |
| actix-web | 159646 | 0.789 | 0.88 | 1.153 | 0.92 | 1.27 |
| rocket | 165269 | 0.765 | 0.891 | 0.977 | 0.95 | 1.08 |

### GET /users/{id}

| フレームワーク | RPS | p50 (ms) | p95 (ms) | p99 (ms) | RPS 比 | p99 比 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| axum | 168216 | 0.753 | 0.841 | 0.928 | 1 | 1 |
| fandhe-backend | 175590 | 0.719 | 0.819 | 0.904 | 1.04 | 0.97 |
| actix-web | 159632 | 0.791 | 0.865 | 1.157 | 0.95 | 1.25 |
| rocket | 164784 | 0.765 | 0.89 | 0.978 | 0.98 | 1.05 |

### POST /echo

| フレームワーク | RPS | p50 (ms) | p95 (ms) | p99 (ms) | RPS 比 | p99 比 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| axum | 161862 | 0.784 | 0.867 | 0.961 | 1 | 1 |
| fandhe-backend | 175097 | 0.722 | 0.796 | 0.901 | 1.08 | 0.94 |
| actix-web | 157793 | 0.801 | 0.873 | 1.17 | 0.97 | 1.22 |
| rocket | 154475 | 0.819 | 0.94 | 1.034 | 0.95 | 1.08 |

### フットプリント

| フレームワーク | アイドル RSS (KB) | 負荷時 RSS (KB) | バイナリサイズ (bytes) | 起動時間 (ms) | バイナリ比 | 負荷時 RSS 比 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| axum | 2640 | 10496 | 1229584 | 0 | 1 | 1 |
| fandhe-backend | 2448 | 4944 | 957088 | 0 | 0.78 | 0.47 |
| actix-web | 6464 | 11280 | 4118848 | 0 | 3.35 | 1.07 |
| rocket | 4384 | 14192 | 3728464 | 0 | 3.03 | 1.35 |


### 読み方・注意

- RPS は 4 実装とも 15〜18 万 RPS の帯に収まり、fandhe-backend が 4 エンドポイントすべてで
  最大（axum 比 1.01〜1.08 倍、actix-web 比 1.09〜1.11 倍、Rocket 比 1.06〜1.13 倍）。
  ただし同一ホスト・単発 run の相対値であり、差の大半は 10% 以内。**順位を断定する材料では
  なく「同水準」と読む**のが妥当
- p99 は fandhe-backend / axum / Rocket が 0.90〜1.03ms、actix-web のみ 1.14〜1.17ms
  （axum 比 1.22〜1.27 倍）。actix-web の worker 分配モデルとの相性（128 接続 / 16 worker）
  の可能性があるが、本 run では原因分析を行っていない
- フットプリントの差は明確: バイナリサイズは fandhe-backend 0.96MB・axum 1.23MB・
  Rocket 3.73MB・actix-web 4.12MB（fandhe-backend は axum 比 0.78 倍、actix-web 比 0.23 倍）。
  負荷時 RSS は fandhe-backend 4.9MB・axum 10.5MB・actix-web 11.3MB・Rocket 14.2MB
  （fandhe-backend は axum 比 0.47 倍）
- 起動時間は全実装 0ms（`wait_for_health` の 5ms 間隔ポーリングの分解能以下）で差を
  観測できない
- macOS 上の計測であり、既存レポート（Linux x86_64、2026-07-18）とは絶対値を比較しない
  （当時の axum-ref は約 33 万 RPS、本 run は約 17 万 RPS。oha・カーネルの TCP スタック・
  コア数が異なる）

## axum 比受け入れ判定の再計測（`bench-accept.sh`）

`RUNS=5 DURATION=15s CONNECTIONS=128`、既定閾値（RPS 比 0.90 以上・p95/p99 比 1.10 以内・
アイドル RSS 比 1.10 以内・バイナリサイズ比 1.00 以内・起動時間絶対差 20ms 未満）。

| 指標 | baseline(axum) | core | 比率/差 | 基準 | 判定 |
|------|-----------------|------|---------|------|------|
| RPS GET /health | 174293.86440705002 | 176402.850281059 | 1.0121 | >= 0.90 | PASS |
| p95 GET /health | 0.000790166 | 0.000783083 | 0.9910 | <= 1.10 | PASS |
| p99 GET /health | 0.000887417 | 0.000887041 | 0.9996 | <= 1.10 | PASS |
| RPS GET /hello/{name} | 174085.1024971423 | 175707.99847830762 | 1.0093 | >= 0.90 | PASS |
| p95 GET /hello/{name} | 0.000810792 | 0.000792458 | 0.9774 | <= 1.10 | PASS |
| p99 GET /hello/{name} | 0.000906542 | 0.000900292 | 0.9931 | <= 1.10 | PASS |
| RPS GET /users/{id} | 172950.88258208017 | 174688.15849045888 | 1.0100 | >= 0.90 | PASS |
| p95 GET /users/{id} | 0.00083425 | 0.000790583 | 0.9477 | <= 1.10 | PASS |
| p99 GET /users/{id} | 0.000920083 | 0.000905333 | 0.9840 | <= 1.10 | PASS |
| RPS POST /echo | 167735.03549060828 | 174511.9360073188 | 1.0404 | >= 0.90 | PASS |
| p95 POST /echo | 0.000863666 | 0.000795083 | 0.9206 | <= 1.10 | PASS |
| p99 POST /echo | 0.000947334 | 0.000903167 | 0.9534 | <= 1.10 | PASS |
| アイドル RSS | 2672KB | 2432KB | 0.9102 | <= 1.10 | PASS |
| バイナリサイズ | 1229584B | 957088B | 0.7784 | <= 1.00 | PASS |
| 起動時間(ms・絶対差) | 0 | 0 | 0.0000 | <= 20 | PASS |

参考値（判定には使わない）: 負荷時 RSS baseline=10096KB core=4976KB

**総合判定: PASS**（終了コード 0）。全 15 指標が既定閾値を満たした。2026-07-18 の
Linux 計測（同レポート 2 回目・PASS）に続き、v0.4.0 相当のコードでも macOS 上で
axum 比の基準を維持していることを確認した。p95 の 3 帯域判定（`P95_BAND=1`）は
使用していない（既定の 2 値判定）。

## 申し送り（out-of-scope-tracking）

- 本 run は macOS 単発計測。Linux（週次 `bench-schedule.yml` と同環境）での横並び計測は
  未実施。`bench-compare.sh` を週次 CI に組み込む判断は行わない（判定を持たず、対象数に
  比例してジョブ時間が伸びるため）
- actix-web の p99 が他 3 実装より高い原因（worker 分配・接続数との比率）は未分析
- `benches/refs/` の CI ジョブ（ビルド・テスト・`cargo deny`）は本 PR では追加していない。
  `benches/microbench` と同様に ci.yml へ組み込むかは別 Issue で判断する
