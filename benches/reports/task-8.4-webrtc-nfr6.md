# TASK-8.4（#29）NFR-6 計測レポート — WebRTC 無関係パスへの性能影響

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` / `bf-plugin-*` 等）
> 表記のまま保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。

`docs/spec/04-requirements.md` NFR-6「パス一致時のみ介入する拡張点は、無関係なパスへの
RPS・レイテンシ影響が誤差範囲内（100.3〜100.8%相当）である」を、`webrtc` feature
（`crates/plugin-webrtc`、in-process 実装）について empirical に計測した結果。

計画時点の判定方針（`docs/acceptance/req8-webrtc-attack-surface.md` §「基準 E」参照）
どおり、実行可能なコアループ（TASK-1.4-2 #70・TASK-1.5 #14 マージ済み）と `webrtc`
feature の配線（TASK-8.1 #26 マージ済み）が揃っているため、フォールバック（アーキ
テクチャ論拠のみ）ではなく **第一選択の empirical 計測** を実施した。

## 計測方法

- 計測用サーバ:
  - ベースライン: `crates/core/examples/minimal.rs`（`webrtc` feature 無効、
    `GET /` に固定応答を返す `bf_routes::Router` のみ登録）
  - 比較対象: `crates/core/examples/webrtc_nfr6.rs`（TASK-8.4 で追加。`webrtc` feature
    有効、`Server::webrtc(WebRtcConfig::new())` を追加登録した以外はベースラインと
    同一の `GET /` ハンドラ）
- 負荷生成: `oha`（`benches/lib/common.sh` と同じ計測基盤の思想を踏襲。専用スクリプト
  `benches/webrtc-nfr6-bench.sh` を新規追加）
- 対象パス: `GET /`（`webrtc` の対象パス `/rtc/offer` とは無関係。`crate::plugin::
  try_intercept` は `webrtc` feature 有効時、このパスに対しては 1 回のパス比較のみで
  `Handler::handle` へフォールスルーする）
- 各系統 5 回計測し中央値を比較（`benches/README.md` の中央値評価方針を踏襲）

## ビルドコマンド

```bash
cargo build --release -p backend-framework-core --example minimal --no-default-features
cargo build --release -p backend-framework-core --example webrtc_nfr6 --features webrtc
```

## 実行コマンド

```bash
RUNS=5 DURATION=15s CONNECTIONS=128 bash benches/webrtc-nfr6-bench.sh
# 環境変数で調整可能（benches/webrtc-nfr6-bench.sh 自体の既定値は RUNS=5
# DURATION=5s CONNECTIONS=32。本レポートの計測は下記のとおり明示的に
# DURATION=15s・CONNECTIONS=128 へ上書きして実行した）
```

## 計測結果

### バイナリサイズ（参考、`docs/dep-impact/records.md` にも記録）

| バイナリ | 構成 | サイズ (bytes) |
|---|---|---|
| `examples/minimal` | webrtc 無効 | 798,688 |
| `examples/webrtc_nfr6` | webrtc 有効 | 8,846,544 |

比率: 約 11.08 倍。

### RPS・p95 レイテンシ（`GET /` への負荷、中央値、複数回実行）

1 回目の実行（`RUNS=5 DURATION=15s CONNECTIONS=128` を環境変数で明示的に指定。
`benches/webrtc-nfr6-bench.sh` 自体の既定値は `RUNS=5 DURATION=5s CONNECTIONS=32`
であり、上記は既定値ではなく上書き値）:

```
[baseline] run 1: rps=142683.33080126406 p95=0.000928796
[baseline] run 2: rps=143848.57357457106 p95=0.000919057
[baseline] run 3: rps=144352.0953158264  p95=0.000924958
[baseline] run 4: rps=145060.76756489647 p95=0.00091099
[baseline] run 5: rps=144442.7348334524  p95=0.000916788
[webrtc]   run 1: rps=139794.84225165777 p95=0.000997449
[webrtc]   run 2: rps=137462.1515248941  p95=0.000964374
[webrtc]   run 3: rps=136054.88011366123 p95=0.000993638
[webrtc]   run 4: rps=136848.12536326292 p95=0.000979454
[webrtc]   run 5: rps=148274.95531456528 p95=0.000933485

baseline RPS 中央値: 144352.10
webrtc   RPS 中央値: 137462.15（baseline 比 95.23%）
baseline p95 中央値: 0.000919057
webrtc   p95 中央値: 0.000979454（baseline 比 106.57%）
```

2 回目の実行（同一パラメータ、再現性確認）:

```
rps_ratio_pct=93.86
p95_ratio_pct=108.45
```

いずれも RPS 比は 94〜95%（baseline より低下）、p95 比は 106〜108%（baseline より
悪化）で安定しており、単発の外れ値ではない。

### 事前計測（`CONNECTIONS=32 DURATION=5s`、小規模確認）

```
baseline RPS 中央値: 144352.10（run 1-5: 142263, 143663, 143840, 144683, 154567）
webrtc   RPS 中央値: 137120.77（run 1-5: 136734, 137046, 137120, 137196, 137819）
比: 95.34%
```

小規模計測でも同様の傾向（94〜96%）を確認しており、計測パラメータ（接続数・
実行時間）に強く依存しない結果と判断する。

## 判定

NFR-6 の文言上の許容帯（100.3〜100.8%相当）には**収まらなかった**。

判定不能・BLOCKED とはしない（計測用 binary が無いことは阻害要因ではない。
TASK-1.4-2 #70・TASK-1.5 #14 のマージにより実行可能なコアループが存在するため）。
測定結果を捏造・丸めず、実測値をそのまま記録する（`.claude/rules/security.md` の
フェイルクローズ原則）。

## 原因の考察

`crate::plugin::try_intercept`（`crates/core/src/plugin.rs`）は `webrtc` feature 有効時、
対象外パス（本計測の `GET /`）に対して `head.method`・`head.target` の文字列比較 1 回
のみを行いフォールスルーする。この呼び出しコスト自体が数 % のオーダーの RPS 劣化を
説明するとは考えにくい。より妥当な説明は、`webrtc` feature 有効時のバイナリサイズが
約 11 倍（798,688 → 8,846,544 bytes）に達すること（`webrtc-rs` の巨大な依存グラフが
実行ファイルに組み込まれるため）に起因する icache/TLB 圧迫・ページフォールト増等の
マイクロアーキテクチャレベルの影響である。この仮説はコードパス自体の追加検証（本
タスクのスコープ外の性能プロファイリング）なしには確定できないため、断定はせず
「考察」として記録する。

## 対応方針（AGENTS.md に文書化済み）

WebRTC を使うサービスがこの性能影響を許容できない場合、`crates/plugin-webrtc-proxy`
（`webrtc-proxy` feature、`webrtc-rs` 非依存の別プロセス切り出し）を選択することで、
コアプロセスのバイナリサイズ・性能特性への影響を回避できる。詳細は `AGENTS.md`
「WebRTC の攻撃表面と『使う/使わない』サービスの安全性方針」節を参照。

## 追補（#178）: 専有計測環境での再計測

`benches/nfr6-exclusive.sh`（`benches/lib/exclusive.sh` の flock 相互排他 + 静穏確認、
`docs/design/nfr6-exclusive-measurement.md` 参照）を用いて再計測を試行した。

### 生ログ（`LOAD1_MAX=2.0` 緩和試行、既定 `LOAD1_MAX=1.0` では終始 BUSY 判定）

```text
=== NFR-6 計測（RUNS=5 DURATION=5s CONNECTIONS=32） ===
  [baseline] run 1: rps=157598.7016894772 p95=0.000219943
  [baseline] run 2: rps=158705.88494325182 p95=0.000216174
  [baseline] run 3: rps=157898.82749806042 p95=0.000218898
  [baseline] run 4: rps=144488.35198155144 p95=0.000236191
  [baseline] run 5: rps=142807.3219082106 p95=0.000239133
  [webrtc] run 1: rps=138585.62341863848 p95=0.000246621
  [webrtc] run 2: rps=138981.91165037194 p95=0.000246141
  [webrtc] run 3: rps=154100.419739397 p95=0.000223541
  [webrtc] run 4: rps=136471.40954327202 p95=0.000251795
  [webrtc] run 5: rps=138841.9718864645 p95=0.000246442

baseline RPS 中央値: 157598.7016894772
webrtc   RPS 中央値: 138841.9718864645（baseline 比 88.10%）
baseline p95 中央値: 0.000219943
webrtc   p95 中央値: 0.000246442（baseline 比 112.05%）
```

判定: FAIL（`evaluate_nfr6_ratio 88.10 112.05` → FAIL）。既存 FAIL 記録と整合する。

### 判断（追補時点）

本イシュー実装時点は複数 issue の並列実装ワークフロー実行中であり、専有実行枠の静穏
確認は既定閾値（`LOAD1_MAX=1.0`）では一度も成立しなかった（loadavg が終始 1.9〜6 程度で
推移）。上記は `LOAD1_MAX` を暫定的に 2.0 へ緩和した 1 回の参考計測であり、**専有環境の
確定値ではない**。直後の `graphql` 対象は静穏再確認時に loadavg 5.96・`cargo`/`rustc`
稼働中を検知して BLOCKED で正しく停止しており、host contention が継続していたことを
示している。

既定閾値での確定再計測は host が真に静穏な期間に改めて実施する必要がある
（フォローアップとして別途実施、`.claude/rules/out-of-scope-tracking.md`）。本 FAIL
判定は暫定的に維持し、PASS へは丸めない。
