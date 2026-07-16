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
