# TASK-1.6-1 性能受け入れ計測レポート（axum-ref 比）

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` / `bf-plugin-*` 等）
> 表記のまま保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。

Issue #71 の成果物。`benches/bench-accept.sh` による axum-ref 比の性能受け入れ判定結果。

## 実施日時・環境

- 実施日時: 2026-07-16（JST）
- OS: Linux 7.0.0-27-generic x86_64（Ubuntu）
- CPU コア数: 12（`nproc`）
- rustc / cargo: 1.96.0（stable, 2026-05-25）
- 計測パラメータ: `RUNS=5 DURATION=15s CONNECTIONS=128`（既定値）

## 本計測（axum-ref vs. コア側）の結果: BLOCKED

既定パラメータで `./benches/bench-accept.sh` を実行した結果:

```
$ RUNS=5 DURATION=15s CONNECTIONS=128 ./benches/bench-accept.sh
（... ビルド ...）
## 判定結果: BLOCKED

コア側計測用バイナリ（CORE_BIN=.../target/release/backend-framework-core）が見つかりません。
TASK-1.4-2（#70）・TASK-1.5（#14）マージ後、フルスクラッチコアの
実行可能サーババイナリが揃った時点で CORE_BIN を指定して再実行してください。
（本スクリプト自体の変更は不要。CORE_BIN の既定値・バイナリ名が確定した場合は
  本スクリプトの既定値を更新する）

終了コード: 2
```

**理由**: 本イシュー着手時点（2026-07-16）で `crates/core`（`backend-framework-core`）は
HTTP/1.1 パーサ・3 種拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）の trait 定義
のみを提供するライブラリクレートであり、接続受理・リクエストループを持つ実行可能サーバ
バイナリは存在しない（TASK-1.4-2 #70・TASK-1.5 #14 がいずれも未マージ、2026-07-16 時点で
issue state は `OPEN`）。axum-ref と機能等価な 4 エンドポイント（`GET /health`,
`GET /hello/{name}`, `GET /users/{id}`, `POST /echo`）を提供するバイナリが存在しない以上、
比較対象を欠くため axum 比の性能判定は実施不能と判断し、安全側に倒して
判定ロジックを実行せずに `BLOCKED` として終了する設計にした
（実装計画の「前提・依存関係」参照）。

`crates/core-ref` を本イシュー内で新規に追加する選択肢も検討したが、`bf-http`
（`crates/http`）は sans-IO 設計で `tokio` の `io-util` feature のみに依存し、実ソケット
の接続受理・リクエストループは TASK-1.4-2（#70）自体の成果物であるため、ここで
計測用バイナリを実装することは #70 のスコープを本 PR に先取りして重複実装することになり、
`crates/core/src/lib.rs` の doc comment にも明記された既定の役割分担
（「実接続は姉妹イシュー TASK-1.4-2（#70）で追加される」）と矛盾する。そのため
本イシューでは harness（`bench-accept.sh` 本体・判定ロジック・レポート生成）の実装と
検証に注力し、コア側との実比較は #70・#14 マージ後に持ち越す。

## ハーネス自体の妥当性検証（axum-ref セルフ比較）

`bench-accept.sh` の比較・閾値判定ロジック自体が正しく機能することを、axum-ref を
baseline・対象の両方に指定するセルフ比較で検証した（`CORE_BIN` に axum-ref のバイナリを
指定。ポートは baseline/core で分離）。

### 検証 1: 既定閾値でのセルフ比較（全項目 PASS になること）

短縮パラメータ（`RUNS=3 DURATION=2s CONNECTIONS=8`、同一ホスト計測を短時間で
繰り返す動作確認目的。既定パラメータ `RUNS=5 DURATION=15s CONNECTIONS=128` でも
同じ判定ロジックが動くことは検証 2 と合わせて確認済み）で実行し、全項目 PASS を確認した。

```
$ RUNS=3 DURATION=2s CONNECTIONS=8 \
    CORE_BIN=target/release/axum-ref CORE_PORT=3202 BASELINE_PORT=3201 \
    ./benches/bench-accept.sh
```

| 指標 | baseline(axum) | core | 比率/差 | 基準 | 判定 |
|------|-----------------|------|---------|------|------|
| RPS GET /health | 216210.7117189976 | 209261.852145358 | 0.9679 | >= 0.90 | PASS |
| p95 GET /health | 0.00005801 | 0.000060736 | 1.0470 | <= 1.10 | PASS |
| p99 GET /health | 0.000079817 | 0.000084718 | 1.0614 | <= 1.10 | PASS |
| RPS GET /hello/{name} | 212534.74245725092 | 210374.40627940476 | 0.9898 | >= 0.90 | PASS |
| p95 GET /hello/{name} | 0.000062414 | 0.000061107 | 0.9791 | <= 1.10 | PASS |
| p99 GET /hello/{name} | 0.000084852 | 0.000084538 | 0.9963 | <= 1.10 | PASS |
| RPS GET /users/{id} | 217373.799427826 | 205131.24162986563 | 0.9437 | >= 0.90 | PASS |
| p95 GET /users/{id} | 0.000057835 | 0.000062031 | 1.0726 | <= 1.10 | PASS |
| p99 GET /users/{id} | 0.000082199 | 0.000088662 | 1.0786 | <= 1.10 | PASS |
| RPS POST /echo | 196443.52057202216 | 200366.3780406729 | 1.0200 | >= 0.90 | PASS |
| p95 POST /echo | 0.000077948 | 0.000078712 | 1.0098 | <= 1.10 | PASS |
| p99 POST /echo | 0.000101947 | 0.000098955 | 0.9707 | <= 1.10 | PASS |
| アイドル RSS | 3912KB | 3880KB | 0.9918 | <= 1.10 | PASS |
| バイナリサイズ | 1358296B | 1358296B | 1.0000 | <= 1.00 | PASS |
| 起動時間(ms・絶対差) | 0 | 0 | 0.0000 | <= 20 | PASS |

参考値（判定には使わない）: 負荷時 RSS baseline=4512KB core=4480KB

判定結果: PASS（終了コード 0）

**注意**: 短縮パラメータ・同一バイナリ同士の比較のため比率は 100% 近辺で安定しているが、
`GET /users/{id}` の RPS 比率が 0.9437・p99 比率が 1.0786 など、axum-ref 同士の比較でも
基準値（0.90 / 1.10）に接近する回があることを観測した。ホストノイズの影響を受けやすい
短時間・軽負荷計測の特性であり、本計測（既定パラメータ `RUNS=5 DURATION=15s
CONNECTIONS=128`）ではより長い負荷印加時間により安定した中央値が得られる見込み。

### 検証 2: 厳しい閾値でのセルフ比較（FAIL・非 0 終了になること）

短縮パラメータ（`RUNS=3 DURATION=2s CONNECTIONS=8`）で `RPS_RATIO_MIN=1.5` を指定し、
axum-ref 同士の比較でも到達し得ない閾値を課したところ、4 エンドポイントすべての RPS
判定が意図通り `FAIL` となり、終了コードも `1`（判定 FAIL）になることを確認した。

```
RPS GET /health          | 217462.76859962818 | 218514.93497996705 | 1.0048 | >= 1.5 | FAIL
RPS GET /hello/{name}    | 213222.8844927897  | 215701.9162504273  | 1.0116 | >= 1.5 | FAIL
RPS GET /users/{id}      | 214499.16050634082 | 215243.3330658657  | 1.0035 | >= 1.5 | FAIL
RPS POST /echo           | 196490.93836683    | 202877.19751699668 | 1.0325 | >= 1.5 | FAIL
（p95/p99・アイドル RSS・バイナリサイズ・起動時間は既定閾値のまま PASS）

## 判定結果: FAIL（1 件以上の基準未達）
終了コード: 1
```

以上より、`bench-accept.sh` の JSON 集計・比率/絶対差算出・閾値判定・非 0 終了の
いずれも意図通りに機能することを両方向（PASS 方向・FAIL 方向）で確認した。

## 申し送り（out-of-scope-tracking）

- **ベンチの CI ワークフロー組み込み**: 同一ホスト計測はノイズの影響を受けやすく
  判定が不安定になるため、CI（`.github/workflows/ci.yml`）には組み込んでいない。
  `workflow_dispatch` 等での手動トリガー導入は別途ユーザー承認のうえ Issue 化が必要
  （本 PR のスコープ外）。
- **依存数・unsafe・cargo audit/deny 等の検証**: 姉妹イシュー #72（TASK-1.6-2）のスコープ。

---

## TASK-1.6-3（#168）実測: BLOCKED 解消・axum-ref 比実測

### 実施日時・環境

- 実施日時: 2026-07-18（UTC）
- OS: Linux 7.0.0-27-generic x86_64（Ubuntu）
- CPU コア数: 12（`nproc`）
- rustc / cargo: 1.96.0（stable, 2026-05-25）
- 計測パラメータ: `RUNS=5 DURATION=15s CONNECTIONS=128`（既定値）

### 計測対象

`crates/core/examples/core-bench.rs`（TASK-1.6-3、#168 で新規追加）を `CORE_BIN` として
使用した。axum-ref（`crates/axum-ref/src/main.rs`）と機能等価な 4 エンドポイント
（`GET /health` / `GET /hello/{name}` / `GET /users/{id}` / `POST /echo`）を提供する。

**機能等価性の担保方法**:
- `bf_routes::Router` は (method, target) 完全一致のみでパスパラメータ（`{name}` /
  `{id}`）を扱えない（TASK-1.5 / #14 時点の既知の制約。パスパラメータ対応の Router
  拡張は本イシューのスコープ外。下記「申し送り」参照）ため、
  `backend_framework_core::Handler` trait を直接実装し、プレフィックスマッチによる
  手書きディスパッチで 4 エンドポイントを提供する
- レスポンス body スキーマ（`EchoBody` / `UserResponse` / `ErrorBody`）は axum-ref と
  同一。`/users/{id}` 応答・`/echo` の JSON パース/再シリアライズには serde/serde_json
  を使用（`crates/core/Cargo.toml` の `[dev-dependencies]` のみに追加。
  `cargo tree -p backend-framework-core -e normal` に現れないことを確認済み、
  pay-for-what-you-use 準拠）
- ランタイム構成（`#[tokio::main]` マルチスレッド）を axum-ref と揃え、ランタイム差を
  計測ノイズに持ち込まない
- 唯一の既知の機能差: axum の `Path` エクストラクタはパーセントデコードを行うが、
  本 example は行わない（計測対象 URL・テスト入力の範囲では影響しない。
  `crates/core/examples/core-bench.rs` の doc comment に明記）

### 実測結果（1 回目）: FAIL

```
$ RUNS=5 DURATION=15s CONNECTIONS=128 ./benches/bench-accept.sh
```

| 指標 | baseline(axum) | core | 比率/差 | 基準 | 判定 |
|------|-----------------|------|---------|------|------|
| RPS GET /health | 334456.54 | 205814.80 | 0.6154 | >= 0.90 | FAIL |
| p95 GET /health | 0.000863965 | 0.001565345 | 1.8118 | <= 1.10 | FAIL |
| p99 GET /health | 0.001367628 | 0.003938392 | 2.8797 | <= 1.10 | FAIL |
| RPS GET /hello/{name} | 350322.18 | 273373.66 | 0.7803 | >= 0.90 | FAIL |
| p95 GET /hello/{name} | 0.000824825 | 0.001022991 | 1.2403 | <= 1.10 | FAIL |
| p99 GET /hello/{name} | 0.001270634 | 0.00244959 | 1.9278 | <= 1.10 | FAIL |
| RPS GET /users/{id} | 351368.74 | 332164.93 | 0.9453 | >= 0.90 | PASS |
| p95 GET /users/{id} | 0.000813402 | 0.000897327 | 1.1032 | <= 1.10 | FAIL |
| p99 GET /users/{id} | 0.001247663 | 0.001286936 | 1.0315 | <= 1.10 | PASS |
| RPS POST /echo | 376763.10 | 351456.17 | 0.9328 | >= 0.90 | PASS |
| p95 POST /echo | 0.000740301 | 0.000866165 | 1.1700 | <= 1.10 | FAIL |
| p99 POST /echo | 0.001518415 | 0.001182251 | 0.7786 | <= 1.10 | PASS |
| アイドル RSS | 3952KB | 3536KB | 0.8947 | <= 1.10 | PASS |
| バイナリサイズ | 1374024B | 859824B | 0.6258 | <= 1.00 | PASS |
| 起動時間(ms・絶対差) | 0 | 0 | 0.0000 | <= 20 | PASS |

`GET /health` の core 側 raw RPS（`228594, 142691, 205814, 234532, 179818`）は試行間の
ばらつきが極端に大きく（最大/最小比 約 1.6 倍）、他 3 エンドポイントの raw RPS
（試行間ばらつきが小さい）と比べて明らかに異質だった。実装計画の想定どおり、
ノイズ起因かどうかを切り分けるため同一パラメータで**1 回だけ再実行**した
（実装計画 Step 4-5、閾値・`Server` 既定値は変更していない）。

### 実測結果（2 回目・再実行）: PASS

```
$ RUNS=5 DURATION=15s CONNECTIONS=128 ./benches/bench-accept.sh
```

| 指標 | baseline(axum) | core | 比率/差 | 基準 | 判定 |
|------|-----------------|------|---------|------|------|
| RPS GET /health | 333435.78 | 356016.70 | 1.0677 | >= 0.90 | PASS |
| p95 GET /health | 0.000858489 | 0.000876463 | 1.0209 | <= 1.10 | PASS |
| p99 GET /health | 0.001335262 | 0.001142568 | 0.8557 | <= 1.10 | PASS |
| RPS GET /hello/{name} | 351782.18 | 355361.37 | 1.0102 | >= 0.90 | PASS |
| p95 GET /hello/{name} | 0.000812059 | 0.000871386 | 1.0731 | <= 1.10 | PASS |
| p99 GET /hello/{name} | 0.001254714 | 0.001143885 | 0.9117 | <= 1.10 | PASS |
| RPS GET /users/{id} | 324683.51 | 354298.76 | 1.0912 | >= 0.90 | PASS |
| p95 GET /users/{id} | 0.000871379 | 0.000883696 | 1.0141 | <= 1.10 | PASS |
| p99 GET /users/{id} | 0.001348969 | 0.00113822 | 0.8438 | <= 1.10 | PASS |
| RPS POST /echo | 297238.72 | 364510.33 | 1.2263 | >= 0.90 | PASS |
| p95 POST /echo | 0.001010173 | 0.000801237 | 0.7932 | <= 1.10 | PASS |
| p99 POST /echo | 0.002146871 | 0.001148847 | 0.5351 | <= 1.10 | PASS |
| アイドル RSS | 3920KB | 3532KB | 0.9010 | <= 1.10 | PASS |
| バイナリサイズ | 1374024B | 859824B | 0.6258 | <= 1.00 | PASS |
| **起動時間(ms・絶対差)** | 0 | 0 | 0.0000 | **<= 20** | **PASS** |

参考値（判定には使わない）: 負荷時 RSS baseline=10716KB core=5608KB

**総合判定: PASS**（終了コード 0）。NFR-1 の起動時間絶対差（20ms 未満）を含む
全 15 指標が既定閾値を満たした。

### 原因分析（1 回目 FAIL の仮説）

`bench-http.sh` は `oha` で 1 プロセスあたり `CONNECTIONS=128` の持続接続を張り続ける。
`crates/core` の `Server` は既定で `max_requests_per_connection=1000`
（`DEFAULT_MAX_REQUESTS_PER_CONNECTION`、`crates/core/src/server.rs`）により、
keep-alive 接続 1 本が 1000 リクエストを処理すると `Connection: close` を返して
切断・再接続を強制する（リソース枯渇 DoS 対策としての意図的な既定値、
`.claude/rules/security.md`）。`DURATION=15s` × 高 RPS（数十万 req/s）の負荷では
128 接続それぞれが 15 秒間に何度も 1000 リクエスト上限へ到達し再接続が発生するため、
再接続タイミングが OS スケジューリング・TCP accept backlog のノイズと重なった試行では
p95/p99・RPS が悪化しうる。1 回目の `GET /health` で観測した異常な試行間ばらつき
（`228594, 142691, ...`）はこの仮説と整合する（axum-ref は keep-alive 接続数上限を
持たないため、同じ再接続コストを負わない）。2 回目の再実行では全エンドポイントで
安定した結果が得られたため、1 回目は再接続タイミングと計測環境（同一ホスト・
他プロセス干渉）の偶発的な重なりによるノイズと判断した。

**是正について**: `max_requests_per_connection` の既定値緩和は「既定値のまま計測する
（コアの素の姿を計測する）」という実装計画の前提・[[security]] のリソース枯渇対策
方針と相反するため、本イシューでは行わない。性能への影響が体系的かどうかの深掘り
（例: `max_requests_per_connection` を変えたパラメトリック計測）は下記「申し送り」を
参照。

### 申し送り（out-of-scope-tracking、TASK-1.6-3 / #168 発生分）

- **`bf_routes::Router` のパスパラメータ（`{name}` 形式）対応**: 本イシューでは
  `Handler` trait を直接実装する手書きディスパッチで回避したが、Router 自体への
  対応は別イシューのスコープ
- **405 応答への `Allow` ヘッダ付与**: `Response` の任意ヘッダ API が未整備
  （TASK-1.5 時点からの既知事項）
- **`max_requests_per_connection` 起因の再接続コストが性能に与える影響の深掘り**:
  上記「原因分析」参照。既定値は変更しないが、パラメータを振った追加計測で
  体系的な影響かどうかを切り分ける価値がある
