# TASK-4.4（#25）WebSocket メッセージ往復レイテンシ計測・NFR-6 計測レポート

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` / `bf-plugin-*` 等）
> 表記のまま保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。

`docs/spec/05-tasks.md` TASK-4.4「WebSocket プラグイン受け入れテスト」の成果物。
TASK-4.3（#24、PR #164）で確立した `benches/bench-ws-load.sh` に、維持中の接続の
メッセージ往復レイテンシ（心拍 RTT）percentile 抽出・接続数増による劣化率算出を
追加し、`crates/core/examples/ws_nfr6.rs`・`benches/ws-nfr6-bench.sh` を新設して
NFR-6（無関係パスへの性能影響）を計測した結果。

## 実施日時・環境

- 実施日時: 2026-07-18（JST）
- OS: Linux 7.0.0-27-generic x86_64（Ubuntu）
- CPU コア数: 12（`nproc`）
- rustc / cargo: 1.96.0（stable, 2026-05-25）
- oha: 1.15.0
- `ulimit -n`: 524288（要求値を十分に充足）
- `/proc/sys/net/ipv4/ip_local_port_range`: `32768-60999`（幅 28,231）

## 1. メッセージ往復レイテンシ計測（受け入れ基準: p95 計測記録・劣化度合いの定量化）

### 計測方法

- fullscratch: `crates/core/examples/ws_echo.rs`（`websocket` feature 有効）
- axum: `crates/axum-ref`（`ws` feature 有効、専用 `--target-dir target/ws-bench`）
- 負荷生成: `crates/ws-load-client`。`HEARTBEAT_MS=2000` 間隔の心拍メッセージの
  往復時間（RTT）を全接続・全維持期間にわたり収集し、`p50/p95/p99/max` を算出する
- 各接続数ティア（1,000 / 5,000 / 10,000）× 各実装で `RUNS=3` 回試行し、心拍 RTT
  percentile の中央値を採用値とする

### 実行コマンド

```bash
cargo build --release -p backend-framework-core --features websocket --example ws_echo
cargo build --release -p axum-ref --features ws --target-dir target/ws-bench
cargo build --release -p ws-load-client

CONNECTION_TIERS="1000 5000 10000" HOLD_SECS=30 RUNS=3 \
    RESULT_JSON=/tmp/ws-bench-full.json bash benches/bench-ws-load.sh
```

### 実測結果（RUNS=3 試行の中央値）

| impl | 接続数 | 確立・維持成功率 | 接続あたり RSS 増分 (KB) | 心拍RTT p50 (us) | 心拍RTT p95 (us) | 心拍RTT p99 (us) | 心拍RTT max (us) |
|---|---|---|---|---|---|---|---|
| fullscratch | 1,000 | 100.00% | 135.8520 | 353 | 928 | 1362 | 1735 |
| fullscratch | 5,000 | 100.00% | 134.8840 | 382 | 915 | 1297 | 2489 |
| fullscratch | 10,000 | 100.00% | 134.8200 | 409 | 1024 | 1458 | 3513 |
| axum | 1,000 | 100.00% | 135.1360 | 388 | 855 | 1159 | 1608 |
| axum | 5,000 | 100.00% | 134.0944 | 375 | 935 | 1292 | 2118 |
| axum | 10,000 | 100.00% | 133.9732 | 389 | 982 | 1396 | 4483 |

### 接続数増による劣化度合いの定量化（最小ティア 1,000 → 最大ティア 10,000、p95 基準）

| impl | 1,000 接続 p95 (us) | 10,000 接続 p95 (us) | 劣化率 |
|---|---|---|---|
| fullscratch | 928 | 1024 | **110.34%** |
| axum | 855 | 982 | **114.85%** |

**評価**: 接続数を 1,000→10,000（10 倍）に増やしても、心拍 RTT p95 の劣化率は
fullscratch 110.34%・axum 114.85%（いずれも桁が変わらないマイクロ秒オーダーの
微増）に留まった。p50 も 353〜409us（fullscratch）・375〜389us（axum）と安定して
おり、接続数増加に対する著しいレイテンシ劣化は観測されなかった。「接続数増による
劣化度合いを定量化する」（TASK-4.4 受け入れ基準）を満たす実測データを記録した。

axum との比較では、劣化率自体は axum の方がわずかに大きい（114.85% vs 110.34%）が、
いずれも実務上問題になる水準（例: 2 倍・3 倍化）には遠く、両実装とも良好な線形性を
示している。

### 参考: 接続あたり RSS 増分（TASK-4.3 の再確認、受け入れ基準(1)〜(3)）

TASK-4.3（PR #164）で確立済みの判定に加え、本セッションで 3 impl × 3 tier の完全な
往復を実行し、以下を再確認した:

- 確立・維持成功率: 全 impl × 全接続数で 100.00%（基準 99% 以上を充足）
- 接続あたり RSS 増分 axum 比（10,000 接続時点）: **100.63%**（基準: 110% 以内、PASS）
- 1,000→10,000 の線形性: fullscratch 135.85→134.88→134.82KB、axum 135.14→134.09→
  133.97KB といずれもほぼ一定で、強い線形性を示す

## 2. NFR-6（無関係パスへの RPS・レイテンシ影響）

### 前提修正: `ws_echo.rs` を NFR-6 比較にそのまま流用しない

当初計画（実装計画 §対象ファイル）では NFR-6 比較対象に `examples/ws_echo.rs`
（TASK-4.3 の 10,000 同時接続負荷試験専用、`#[tokio::main(flavor = "multi_thread")]`）
をそのまま流用する想定だった。しかし、ベースライン `examples/minimal.rs`
（`current_thread`）と組み合わせて実測したところ、`GET /health` への RPS 比が
baseline 比 **約190%**（2 回の独立計測で 189.29%・190.34%と再現）という説明の
つかない値になった。

原因を調査した結果、両 example の `#[tokio::main]` フレーバーの違い
（`ws_echo.rs` は `multi_thread` で全 12 コアを使用、`minimal.rs` は
`graphql_nfr6.rs`/`webrtc_nfr6.rs`/`tracing_nfr.rs` と同じく `current_thread` で
単一スレッド）が RPS 差を支配しており、`websocket` feature 自体の処理コストを
計測できていなかったことが判明した。

対処として `crates/core/examples/ws_nfr6.rs`（`graphql_nfr6.rs`/`webrtc_nfr6.rs` と
同型の `current_thread` 専用 NFR-6 計測用 example、待受 `127.0.0.1:3009` 固定）を
新設し、ベースラインとランタイム構成を揃えた。`examples/ws_echo.rs` 自体は
TASK-4.3 の用途（10,000 同時接続負荷試験）に変更なく引き続き使用する。

### 計測方法（修正後）

- baseline: `examples/minimal.rs`（`websocket` feature 無効、`current_thread`）
- 対象: `examples/ws_nfr6.rs`（`websocket` feature 有効・`Server::websocket` 登録済み、
  `current_thread`）
- 計測対象パス: `GET /health`（無関係パス、production の `GET /ws` upgrade 判定は
  1 回のヘッダ確認のみでフォールスルーする）
- `oha`（RUNS=5・DURATION=5s・CONNECTIONS=32）

### 実行コマンド

```bash
cargo build --release -p backend-framework-core --example minimal --no-default-features
cargo build --release -p backend-framework-core --example ws_nfr6 --features websocket

bash benches/ws-nfr6-bench.sh
```

### 実測結果（3 回の独立計測）

| 実行 | RPS 比（ws_nfr6 / baseline） | p95 比（ws_nfr6 / baseline） | `evaluate_nfr6_ratio` |
|------|------|------|------|
| 1 回目 | 98.67% | 101.94% | WARN（実務帯内・狭義帯外） |
| 2 回目 | 100.36% | 99.92% | PASS（狭義帯内） |
| 3 回目（採用値） | 101.49% | 97.71% | WARN（実務帯内・狭義帯外） |

`scripts/accept/lib/nfr6-ratio.sh`（`evaluate_nfr6_ratio`）の判定帯: 実務許容帯
[95%, 105%]（RPS）・[0, 105%]（p95、片側）は FAIL 境界、狭義帯 [100.3%, 100.8%]
（RPS）・[0, 100.8%]（p95）は PASS/WARN 境界。

**判断**: 3 回とも実務許容帯に安定して収まり、FAIL は 1 度も観測されなかった
（`docs/acceptance/req5-graphql.md` が記録したような RPS 比 93〜108%・p95 比
50〜111% という大きな振れ幅は本タスクでは再現されなかった）。狭義帯（PASS）は
2 回目のみ達成し、1・3 回目は WARN（実務帯内・狭義帯外）。総合として無関係パスへの
性能影響は誤差範囲に収まっていると判断する。

## 判定サマリー

| 受け入れ条件 | 判定 |
|---|---|
| メッセージ往復レイテンシ（p95）の計測記録・接続数増による劣化度合いの定量化 | **PASS**（fullscratch 110.34%・axum 114.85%、桁が変わる劣化なし） |
| `websocket` feature 無効時の依存・unsafe・コード完全除外 | **PASS**（`scripts/accept/websocket-accept.sh` 基準 A・A'・pay-for-what-you-use-check.sh、詳細は `docs/acceptance/req4-websocket.md`） |
| NFR-6（無関係パスへの RPS・レイテンシ影響が誤差範囲内） | **PASS〜WARN**（3 回中 FAIL 0 回、実務許容帯内で安定。狭義帯は 1/3 回のみ） |

**総合判定: PASS**（FAIL なし。詳細な受け入れ検証ログは `docs/acceptance/req4-websocket.md` を参照）

## 検証コマンド一覧（再現手順）

```bash
# 事前ビルド
cargo build --release -p backend-framework-core --features websocket --example ws_echo
cargo build --release -p backend-framework-core --example minimal --no-default-features
cargo build --release -p backend-framework-core --features websocket --example ws_nfr6
cargo build --release -p axum-ref --features ws --target-dir target/ws-bench
cargo build --release -p ws-load-client

# レイテンシ計測（本計測は数分〜十数分かかる）
CONNECTION_TIERS="1000 5000 10000" HOLD_SECS=30 RUNS=3 \
    RESULT_JSON=/tmp/ws-bench-full.json bash benches/bench-ws-load.sh

# NFR-6 計測
bash benches/ws-nfr6-bench.sh

# 受け入れ検証オーケストレータ（A〜D をまとめて実行）
WEBSOCKET_ACCEPT_RESULT_JSON=/tmp/ws-bench-full.json bash scripts/accept/websocket-accept.sh

# 判定ロジックのオフライン・セルフテスト
bash scripts/tests/run-websocket-accept-tests.sh
```
