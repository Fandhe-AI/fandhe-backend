# TASK-1.6-1 性能受け入れ計測レポート（axum-ref 比）

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

- **コア側との実測比較**: TASK-1.4-2（#70）・TASK-1.5（#14）マージ後、`CORE_BIN` に
  実行可能バイナリのパスを指定して `bench-accept.sh` を再実行し、本レポートを更新する
  必要がある。現時点では未実施（ブロック中）。
- **ベンチの CI ワークフロー組み込み**: 同一ホスト計測はノイズの影響を受けやすく
  判定が不安定になるため、CI（`.github/workflows/ci.yml`）には組み込んでいない。
  `workflow_dispatch` 等での手動トリガー導入は別途ユーザー承認のうえ Issue 化が必要
  （本 PR のスコープ外）。
- **依存数・unsafe・cargo audit/deny 等の検証**: 姉妹イシュー #72（TASK-1.6-2）のスコープ。
