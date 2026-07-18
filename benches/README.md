# benches/ — 負荷生成・計測ハーネス

`docs/spec/05-tasks.md` TASK-1.2 の成果物。`crates/axum-ref`（および将来のフルスクラッチ
コア、TASK-1.6）を対象に RPS・レイテンシ・RSS・起動時間・バイナリサイズを再現手順付きで
計測するためのスクリプト集。TASK-1.6-1（#71）で `bench-accept.sh`（axum-ref との比較・
閾値判定を行う受け入れテスト）を追加した。

## なぜ複数回計測・中央値評価なのか

`docs/spec/03-poc/fullscratch-performance` の PoC-2 では、`POST /echo` の axum 参照実装で
3 回中 1 回だけ p99 が 13.5ms（他 2 回は 4.3ms・1.0ms）という外れ値が観測された。単発計測
では「たまたま外れ値を引いた回」を実力値として誤判定するリスクがある。また PoC-2 の README
には「負荷時 RSS は各実装 1 回のみの単発計測」という環境制約が明記されており、複数回計測が
できていなかった。

本ハーネスはこれらの申し送りを踏まえ、**RPS/レイテンシ・負荷時 RSS の両方について**
複数回計測（既定 RUNS=5、最低 3）を行い、**平均値ではなく中央値**で評価することを標準の
計測手法とする。中央値は外れ値 1 件の影響を受けにくく、PoC-2 のような単発の環境ノイズが
結果全体を歪めるのを防げる。

## 前提ツール

- [`oha`](https://github.com/hatoo/oha)（HTTP 負荷生成。`cargo install oha` で導入）
- `jq`（JSON パース）
- `curl`（`wait_for_health` によるサーバ起動完了検知に使用。全スクリプト共通の前提）
- Linux（`ps -o rss=` を使用するため。Linux 以外では RSS 計測は動作しない）

スクリプトはこれらの前提ツールを自動ダウンロードしない。冒頭で存在検査を行い、
見つからない場合は導入コマンドを案内して終了する（サプライチェーン考慮、
`.claude/rules/security.md`）。

## ビルド手順

```bash
cargo build --release --bin axum-ref
```

## スクリプト一覧

| スクリプト | 計測内容 |
|-----------|---------|
| `bench-http.sh` | RPS・p50・p95・p99（`GET /health`, `GET /hello/{name}`, `GET /users/{id}`, `POST /echo`） |
| `bench-rss.sh` | 負荷時 RSS（試行内複数サンプル × 複数試行の中央値。PoC-2 の単発計測の是正） |
| `bench-footprint.sh` | 起動時間・アイドル RSS・リリースバイナリサイズ |
| `bench-accept.sh` | 上記 3 スクリプトを axum-ref（baseline）・コア側（対象）の順に実行し、比率・絶対差を算出して REQ-1・NFR-1・NFR-2 の基準で判定する受け入れテスト（TASK-1.6-1、#71） |
| `bench-ws-load.sh` | 10,000 同時 WebSocket 接続の確立成功率・接続あたり RSS 増分（fullscratch/axum 比）・線形性を計測する負荷試験ハーネス（TASK-4.3、#24） |

共通関数は `lib/common.sh` に集約している（サーバ起動/停止・前提ツール検査・中央値算出・
`RESULT_JSON` 機械可読出力ヘルパー・数値バリデーション）。

## 実行例

```bash
# 既定パラメータ（RUNS=5 DURATION=15s CONNECTIONS=128）で実行
./benches/bench-http.sh
./benches/bench-rss.sh
./benches/bench-footprint.sh

# 動作確認用に短縮パラメータで素早く回す
RUNS=3 DURATION=3s CONNECTIONS=16 ./benches/bench-http.sh
```

### 計測パラメータ（env で上書き可能）

| 変数 | 既定値 | 意味 |
|------|-------|------|
| `RUNS` | `5` | 計測回数（最低 3。中央値評価の前提を満たすため） |
| `DURATION` | `15s` | oha の負荷印加継続時間 |
| `CONNECTIONS` | `128` | oha の同時接続数 |
| `TARGET_BIN` | `target/release/axum-ref` | 計測対象バイナリ（TASK-1.6 でフルスクラッチコアに差し替え可能） |
| `TARGET_HOST` | `127.0.0.1` | バインド先ホスト（既定でループバックのみ、外部公開しない） |
| `TARGET_PORT` | `3001` | バインド先ポート |
| `SAMPLE_INTERVAL_SEC`（bench-rss.sh のみ） | `1` | 負荷印加中の RSS サンプリング間隔（秒） |
| `RESULT_JSON`（bench-http.sh / bench-rss.sh / bench-footprint.sh） | 未指定 | 指定時、計測結果（中央値・raw 値）を機械可読 JSON として当該パスに書き出す（人間可読 stdout は変更なし）。`bench-accept.sh` が比較判定の入力として使う |

## 出力の読み方（実行結果例）

以下は `RUNS=5 DURATION=15s CONNECTIONS=128` で `crates/axum-ref` を対象に実行した結果例
（実行環境依存の絶対値であり、TASK-1.6 の判定にそのまま使う数値ではない）。

### bench-http.sh

```
# bench-http.sh 結果（RUNS=5 DURATION=15s CONNECTIONS=128）

## GET /health
raw RPS: 451788.8 452448.2 440056.4 440047.3 442650.0
raw p50: 0.000235067 0.000235599 0.000240458 0.00024017 0.000239107
raw p95: 0.000629442 0.000624441 0.000643905 0.000646523 0.000641812
raw p99: 0.000944615 0.000933116 0.000978852 0.000981778 0.000968626
median  RPS=442650.0 p50=0.000239107s p95=0.000641812s p99=0.000968626s

## GET /hello/{name}
median  RPS=436616.5 p50=0.000242017s p95=0.000651322s p99=0.000986527s

## GET /users/{id}
median  RPS=423785.5 p50=0.000247237s p95=0.000676375s p99=0.00102822s

## POST /echo
median  RPS=457081.1 p50=0.000229005s p95=0.000605611s p99=0.001177865s
```

raw 値の並びを見て、PoC-2 のような突出した外れ値がないかを目視確認できるようにしている
（外れ値があっても中央値が採用値になるため判定は歪まないが、傾向把握のため raw も残す）。

### bench-rss.sh

```
試行 1: サンプル数=11 中央値=8192KB
試行 2: サンプル数=12 中央値=9634KB
試行 3: サンプル数=12 中央値=11146KB
試行 4: サンプル数=11 中央値=11352KB
試行 5: サンプル数=11 中央値=12016KB

# bench-rss.sh 結果（RUNS=5 DURATION=10s CONNECTIONS=128）
アイドル RSS: 3896KB
試行別中央値: 8192 9634 11146 11352 12016
負荷時 RSS（試行間中央値）: 11146KB
```

**注意（試行間の RSS 増加傾向について）**: `bench-rss.sh` はサーバプロセスを 1 回だけ起動し、
その同一プロセスに対して RUNS 回の負荷印加試行を繰り返す（試行ごとの再起動はしない）。
上記の実行例のように、tokio/hyper のバッファプール拡張やアロケータが解放済みメモリを OS に
即座に返さない挙動により、試行を重ねるごとに RSS が右肩上がりになることがある。これは
プロセス寿命内の実利用パターンに近い自然な挙動であり、明確なメモリリークとは区別する必要が
ある。継続的な増加が数十試行を超えても収束しない場合はリークを疑い、別途 Issue 化する
（`.claude/rules/out-of-scope-tracking.md`）。

### bench-footprint.sh

```
# bench-footprint.sh 結果（RUNS=5）
raw 起動時間(ms): 0 0 0 0 0
raw アイドル RSS(KB): 3984 3928 3948 3888 4000
中央値 起動時間: 0ms
中央値 アイドル RSS: 3948KB
バイナリサイズ: 1359200 bytes（target/release/axum-ref）
```

**注意（起動時間の計測粒度について）**: 起動時間はプロセス起動から `/health` 初回応答成功
までを 5ms 間隔でポーリングして計測する。axum-ref のような軽量バイナリは 5ms 未満で起動
完了することが多く、その場合は `0ms` と記録される。これはポーリング粒度の限界であり、
REQ-1 の起動時間絶対差基準（20ms 未満）に対しては十分な精度で「基準を満たす」と判定できる。
より高精度な計測が必要になった場合はポーリング間隔の短縮を別途検討する。

## 同一ホスト計測時のノイズ注意

- 計測中に他プロセス（ビルド・ブラウザ等）が同一ホストで CPU/メモリを消費していると
  RPS・レイテンシ・RSS の全指標にノイズが乗る。可能であれば計測中は他の重い処理を止める
- 短時間に繰り返し起動する場合、直前のバインドが `TIME_WAIT` から抜けきらず
  `Address already in use` になることがある。数秒待つか `TARGET_PORT` を変えて再実行する
- `bench-footprint.sh` は `RUNS` 回サーバを起動・停止するため、上記のポートの再利用待ちが
  発生しやすい。連続実行で失敗する場合は `TARGET_PORT` を変更する

## bench-accept.sh — 性能受け入れ判定（TASK-1.6-1）

`bench-http.sh` / `bench-rss.sh` / `bench-footprint.sh` を axum-ref（baseline）→
コア側（対象、`CORE_BIN`）の順に実行し、中央値同士の比率・絶対差を算出して
REQ-1・NFR-1・NFR-2 の基準で判定する受け入れテスト。1 件でも基準未達（FAIL）が
あれば非 0 で終了する。

### 判定基準

| 指標 | 基準 | 既定の env 変数 |
|------|------|----------------|
| RPS（4 エンドポイントすべて） | axum 比 90% 以上 | `RPS_RATIO_MIN=0.90` |
| p95・p99 レイテンシ（4 エンドポイントすべて） | axum 比 110% 以内 | `P95_RATIO_MAX=1.10` / `P99_RATIO_MAX=1.10` |
| アイドル時 RSS | axum 比 110% 以内 | `IDLE_RSS_RATIO_MAX=1.10` |
| リリースバイナリサイズ | axum 比 同等以下 | `BIN_SIZE_RATIO_MAX=1.00` |
| 起動時間 | axum との絶対差 20ms 未満 | `STARTUP_DIFF_MAX_MS=20` |

負荷時 RSS（`bench-rss.sh`）は REQ-1 の必須基準ではないため、判定表とは別に
**参考値としてのみ**出力する。

### 使い方

```bash
# 既定パラメータ（RUNS=5 DURATION=15s CONNECTIONS=128）で実行
# CORE_BIN が指すバイナリが存在しない場合は判定を実施せず BLOCKED（終了コード 2）で終わる
./benches/bench-accept.sh

# コア側バイナリを明示指定して実行（TASK-1.4-2 #70・TASK-1.5 #14 マージ後の想定）
CORE_BIN=target/release/backend-framework-core ./benches/bench-accept.sh

# 判定表を markdown レポートにも追記する
REPORT_MD=benches/reports/task-1.6-1-performance.md ./benches/bench-accept.sh

# ハーネス自体の妥当性検証（axum-ref 同士のセルフ比較。全項目 PASS になるはず）
CORE_BIN=target/release/axum-ref CORE_PORT=3102 ./benches/bench-accept.sh
```

### 終了コード

| コード | 意味 |
|--------|------|
| `0` | 全項目 PASS |
| `1` | 1 件以上 FAIL（性能基準未達） |
| `2` | BLOCKED（`CORE_BIN` が指すバイナリが存在せず判定不能。baseline 側バイナリ欠如も `1` で別途エラー終了） |

### 現状（2026-07-16 時点）

`crates/core` 側はライブラリ（HTTP/1.1 パーサ・3 拡張点）のみで、axum-ref と等価な
エンドポイントを提供する実行可能サーババイナリは TASK-1.4-2（#70）・TASK-1.5（#14）
マージ後に追加される見込み。現時点で既定パラメータのまま実行すると `BLOCKED`
（終了コード 2）で終了する。ハーネス自体の正しさは axum-ref 同士のセルフ比較
（`CORE_BIN` に axum-ref を指定）で検証済み。実測結果は
[`reports/task-1.6-1-performance.md`](reports/task-1.6-1-performance.md) を参照。

`TARGET_BIN` / `TARGET_HOST` / `TARGET_PORT`（`bench-http.sh` 等の単体実行時）や
`BASELINE_*` / `CORE_*`（`bench-accept.sh`）を差し替えることで、`crates/core` 側の
実行可能バイナリが揃った時点で本スクリプトの変更なしにそのまま判定に使える設計にしている。

## webrtc-nfr6-bench.sh / graphql-nfr6-bench.sh — プラグイン feature の NFR 計測

`webrtc`（TASK-8.4、#29）・`graphql`（TASK-5.2、#53）の各 feature を有効化した際、
無関係パス（`GET /`）への RPS・p95 レイテンシ影響が誤差範囲に収まることを検証する
専用ハーネス。共通パターン: ベースライン（`crates/core/examples/minimal.rs`、対象
feature 無効）と、計測対象 feature を有効化した専用 example（`webrtc_nfr6.rs` /
`graphql_nfr6.rs`）へそれぞれ `oha` で負荷をかけ、RPS・p95 の中央値比（`RUNS` 回、
既定 5 回）を算出する。production 配線自体は変更せず、計測専用の example を叩くのみ。

```bash
# 事前ビルド（各スクリプトとも自動ビルドしない）
cargo build --release -p backend-framework-core --example minimal --no-default-features
cargo build --release -p backend-framework-core --example webrtc_nfr6 --features webrtc
cargo build --release -p backend-framework-core --example graphql_nfr6 --features graphql

./benches/webrtc-nfr6-bench.sh
./benches/graphql-nfr6-bench.sh
```

標準出力（stderr）へ実行ログ、標準出力（stdout）へ `rps_ratio_pct=` / `p95_ratio_pct=`
等の machine-readable な結果を出す。`RUNS` / `DURATION` / `CONNECTIONS` を env で
上書き可能（既定 `RUNS=5 DURATION=5s CONNECTIONS=32`）。判定（PASS/WARN/FAIL）は
`scripts/accept/lib/nfr6-ratio.sh`（`evaluate_nfr6_ratio`）を呼ぶ
`scripts/accept/webrtc-accept.sh` / `scripts/accept/graphql-accept.sh` が担う（実務
許容帯 [95%, 105%]・狭義帯 [100.3%, 100.8%]）。実行結果レポートは
`reports/task-8.4-webrtc-nfr6.md` / `reports/task-5.2-graphql-performance.md` を参照。

## hub-nfr6-bench.sh — hub 共通配線プラグインの NFR-6 計測（TASK-9.5、#65）

`bf-plugin-hub-wiring`（依存逆転型プラグイン）をリンクした最小サーバが、無関係
パス（`GET /`）への RPS・p95 レイテンシに与える影響を検証する。`webrtc-nfr6-bench.sh` /
`graphql-nfr6-bench.sh` と同型だが、比較対象は feature 有効化ではなく **クレートの
リンク**（`Server::gate` 未登録＝`BF_HUB_GATE=off`）である点が異なる（依存逆転型
プラグインは Cargo feature ではなく利用側の依存追加で着脱するため）。

比較対象には `examples/hub_link_only.rs`（`examples/minimal.rs` と同一の `GET /` の
みを持つ最小 example）を使う。`examples/hub_service_demo.rs`（PoC-6 相当の
マルチテナント `/items` 系ハンドラ・シードストア・`Authenticator` 呼び出しを持つ
受け入れテスト用ダミーサービス）は使わない。アプリケーション層のオーバーヘッドが
リンクコストの計測値へ混入するため（Cursor Bugbot review 4727552092 指摘1、PR #163）。

```bash
cargo build --release -p backend-framework-core --example minimal --no-default-features
cargo build --release -p bf-plugin-hub-wiring --example hub_link_only

./benches/hub-nfr6-bench.sh
```

`RUNS` / `DURATION` / `CONNECTIONS` を env で上書き可能（既定 `RUNS=5 DURATION=5s
CONNECTIONS=32`）。判定は `scripts/accept/lib/nfr6-ratio.sh`（`evaluate_nfr6_ratio`）を
呼ぶ `scripts/accept/hub-wiring-accept.sh` が担う（実務許容帯 [95%, 105%]・狭義帯
[100.3%, 100.8%]）。実行結果レポートは `reports/task-9.5-hub-wiring-performance.md` を
参照（本環境は専有環境ではなく、実測値が実務許容帯を外れて FAIL 記録されている点・
環境注記を含む）。

## tracing-nfr-bench.sh — サンプリング適用後の可観測性 NFR 再計測

TASK-10.4（#59）: `tracing` feature（TASK-10.1〜10.3 の決定的サンプリング・イベント
統合・高頻度パス除外を適用済み）を有効化した際、高頻度パス想定 `GET /health` への
RPS・p95 レイテンシ影響が REQ-10 の成功基準（RPS 劣化 5% 以内・p95 悪化 110% 以内）に
収まることを検証する。ベースライン（`crates/core/examples/minimal.rs`。TASK-10.4 で
`GET /health` を追加）と、比較対象 `crates/core/examples/tracing_nfr.rs`（`tracing`
feature 有効・`init_tracing` + `Server::tracing` 登録済み）へそれぞれ `oha` で負荷を
かける。`webrtc-nfr6-bench.sh` / `graphql-nfr6-bench.sh` と同型のパターンだが、
以下 2 シナリオを実行する点が異なる:

- **シナリオ A（受け入れ判定対象）**: 全緩和策適用（サンプリング + イベント統合 +
  `/health` を `TracingConfig::exclude_path` で除外）
- **シナリオ B（参考値）**: 除外なし・サンプリングのみ（`EXCLUDE_HEALTH=0`）。
  TASK-10.3 除外機構の追加効果を差分観測するための対照

```bash
# 事前ビルド（自動ビルドしない）
cargo build --release -p backend-framework-core --example minimal --no-default-features
cargo build --release -p backend-framework-core --example tracing_nfr --features tracing

./benches/tracing-nfr-bench.sh
```

標準出力（stderr）へ実行ログ、標準出力（stdout）へ `rps_a_ratio_pct=` /
`p95_a_ratio_pct=`（シナリオ A）・`rps_b_ratio_pct=` / `p95_b_ratio_pct=`（シナリオ
B）等の machine-readable な結果を出す。`RUNS` / `DURATION` / `CONNECTIONS` を env で
上書き可能（既定 `RUNS=5 DURATION=5s CONNECTIONS=32`）。判定（PASS/FAIL）は
`scripts/accept/tracing-accept.sh` が担う（受け入れ帯: RPS 比 ≥95%・p95 比 ≤110%、
REQ-10 の成功基準そのもの。`webrtc`/`graphql` の NFR-6 判定帯とは別の帯）。実行結果
レポートは `reports/task-10.4-tracing-performance.md` を参照。

## bench-ws-load.sh — 10,000 同時 WebSocket 接続負荷試験・RSS 再計測（TASK-4.3、#24）

fullscratch（`crates/core/examples/ws_echo.rs`）と axum-ref（`ws` feature 有効）の
2 実装へ `crates/ws-load-client`（PoC-7 `load-client` の移植・改良）で同一の WebSocket
長時間接続負荷（接続数 1,000/5,000/10,000）を掛け、保持期間中のサーバ RSS を継続
サンプリングして「接続あたり RSS 増分」を算出・比較する。TASK-4.1（#22）・TASK-4.2
（#23）で確立した「委譲後の専用タスク再 spawn + permit 引き継ぎ」最適化の RSS 削減
効果を正式に再計測し、REQ 基準（axum 比 110% 以内・確立成功率 99% 以上・1k→10k の
線形性）を判定する。

`bench-rss.sh`（試行内複数サンプル×複数試行の中央値評価）と同じ計測思想を踏襲するが、
HTTP（oha）ではなく WebSocket 長時間接続（専用クライアント）が対象であるため独立
スクリプトとする。`lib/common.sh` の `median`/`to_json_array`/`write_result_json` の
みを再利用し、`check_dependencies`/`start_server`（oha・単一 TARGET_BIN 前提）は
使わない。

### 前提ツール・前提条件

- `jq`・`curl`（`benches/lib/common.sh` と共通）
- Linux（`ps -o rss=` を使用）
- `ulimit -n` が「最大接続数 + 100」以上（クライアント・サーバは別プロセスのため
  プロセス単位の fd 上限は概ね最大接続数分で足りる。不足時はスクリプトが導入手順を
  案内して終了する。自動引き上げはしない）
- クライアント・サーバがループバック対向のため、`/proc/sys/net/ipv4/ip_local_port_range`
  の幅が「最大接続数 + 1000」以上（PoC-7 で確認されたエフェメラルポート枯渇の再発防止。
  Linux 既定範囲（約 28,000）で 10,000 接続は通常充足する）

### ビルド手順（自動ビルドしない）

```bash
cargo build --release -p backend-framework-core --features websocket --example ws_echo
# axum-ref の ws feature 有効ビルドは既存の target/release/axum-ref（他ベンチの
# baseline）を汚さないよう専用 target-dir へ分離する
cargo build --release -p axum-ref --features ws --target-dir target/ws-bench
cargo build --release -p ws-load-client
```

### 実行例

```bash
# 既定パラメータ（RUNS=3 HOLD_SECS=60 CONNECTION_TIERS="1000 5000 10000"）で実行
bash benches/bench-ws-load.sh

# 動作確認用に縮小パラメータで素早く回す（スモークテスト）
CONNECTION_TIERS="100" HOLD_SECS=5 RUNS=3 bash benches/bench-ws-load.sh

# 機械可読出力
RESULT_JSON=/tmp/ws-load-result.json bash benches/bench-ws-load.sh
```

### 計測パラメータ（env で上書き可能）

| 変数 | 既定値 | 意味 |
|------|-------|------|
| `RUNS` | `3` | 接続数ティアごとの試行回数（最低 3。中央値評価の前提） |
| `HOLD_SECS` | `60` | 接続確立後の維持時間・秒（`crates/core` の `max_connection_lifetime` 既定 300 秒より必ず短くすること。長時間接続は委譲後の専用タスクで生存するため lifetime の影響は受けないが、心拍応答確認のため十分な保持時間を確保する） |
| `CONNECTION_TIERS` | `"1000 5000 10000"` | 計測する接続数（空白区切り） |
| `RAMP_BATCH` / `RAMP_DELAY_MS` | `200` / `50` | `ws-load-client` へ渡すランプアップ速度 |
| `HEARTBEAT_MS` | `2000` | 心拍間隔・ミリ秒 |
| `SAMPLE_INTERVAL_SEC` | `1` | 負荷印加中の RSS サンプリング間隔（秒） |
| `SUCCESS_RATE_MIN_PCT` | `99` | 確立成功率の受け入れ基準（%） |
| `AXUM_RATIO_MAX_PCT` | `110` | 最大接続数時点の接続あたり RSS 増分・axum 比の受け入れ基準（%） |
| `FULLSCRATCH_BIN` / `AXUM_BIN` / `CLIENT_BIN` | `target/release/examples/ws_echo` 等 | 計測対象バイナリの明示指定 |
| `RESULT_JSON` | 未指定 | 指定時、接続数ティア別の接続あたり RSS 増分・成立率を機械可読 JSON として書き出す |

### 判定基準（受け入れ条件、`docs/spec/05-tasks.md` TASK-4.3）

1. 確立成功率 99% 以上（全実装 × 全接続数ティア）
2. 最大接続数（既定 10,000）時点の接続あたり RSS 増分が axum 比 110% 以内
3. 1,000→10,000 の接続あたり RSS 増分の線形性（自動判定はせず、出力表を目視確認する）

実測結果は [`reports/task-4.3-ws-load-rss.md`](reports/task-4.3-ws-load-rss.md) を参照。

## tracing-backpressure-bench.sh — 非同期 writer バックプレッシャー・ログ欠落率計測

TASK-10.6（#90）: `tracing_appender::non_blocking`（既定 lossy=true）の高負荷時
ログ欠落率を負荷段階（イベント総数 × 送出スレッド数）別に計測するハーネス。
`crates/plugin-tracing/examples/backpressure_probe.rs`（既定構成のまま高負荷送出し
`{emitted, written, dropped_lines, drop_rate_pct, events_per_sec}` を JSON 1 行で
出力する計測プローブ）を負荷段階ごとに `RUNS` 回実行し、欠落率・実効イベントレートの
中央値を算出する。

```bash
cargo build --release -p bf-plugin-tracing --example backpressure_probe
RUNS=5 bash benches/tracing-backpressure-bench.sh

# 動作確認用に短縮パラメータ・負荷段階で素早く回す
RUNS=3 STAGES="10000:1 10000:4" bash benches/tracing-backpressure-bench.sh
```

負荷段階は `STAGES`（`"イベント総数:送出スレッド数"` の空白区切りリスト）・1 イベント
あたりの目標バイト長は `LINE_BYTES` で上書き可能。標準出力（stdout）へ負荷段階別の
結果を JSON 配列で出す（`RESULT_JSON` 指定時はファイルにも書き出す、
`benches/lib/common.sh` の規約に準拠）。実測結果・許容基準は
`reports/task-10.6-tracing-backpressure.md` を参照。
