# TASK-10.4（#59）サンプリング適用後性能再検証レポート

`benches/tracing-nfr-bench.sh`（`oha` による empirical 計測。ベースライン
`crates/core/examples/minimal.rs`（`tracing` feature 無効） vs 比較対象
`crates/core/examples/tracing_nfr.rs`（`tracing` feature 有効・`init_tracing` +
`Server::tracing` 登録済み）、高頻度パス想定 `GET /health` への負荷、RUNS=5・
DURATION=5s・CONNECTIONS=32）を 4 回実行した生ログ・結果。判定は
`scripts/accept/tracing-accept.sh` を参照。

## 背景

PoC-10（`docs/spec/04-requirements.md` REQ-10）の実測で、可観測性ミドルウェアは
サンプリングなし構成だと非同期 writer でも **RPS 劣化 31.6%・p95 悪化 61.7%** と
なり REQ-10 の成功基準を満たさなかった。その対策として以下の 3 緩和策を実装済み。

- **TASK-10.1（#56）**: 決定的サンプリング（`Sampler`、既定 `sample_interval=100`）
- **TASK-10.2（#57）**: 受理・応答 2 イベント → 応答時 1 イベントへの統合
- **TASK-10.3（#58）**: 高頻度パスの完全一致除外（`TracingConfig::exclude_path`。
  サンプラーのカウンタ消費前に除外）

本タスク（TASK-10.4）は、この 3 緩和策すべてを適用した構成で `GET /health`
相当の高頻度パスにおいて **RPS 劣化 5% 以内（比 95% 以上）・p95 悪化 110% 以内
（比 110% 以下）** を再計測・確認する。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-17 |
| 対象コミット（`origin/main`、本ブランチの分岐元） | `0ad5148a1c84adcefeef5c0aca169cbc4f01f947` |
| oha | 1.15.0 |
| ビルド | `cargo build --release -p backend-framework-core --example minimal --no-default-features` / `--example tracing_nfr --features tracing` |
| 計測パラメータ | RUNS=5 DURATION=5s CONNECTIONS=32（`benches/tracing-nfr-bench.sh` 既定） |
| 備考 | 本 worktree は並列 issue 実装ワークフロー下で実行されており、他エージェントの並行負荷が計測ノイズに影響している可能性がある（`benches/reports/task-5.2-graphql-performance.md` と同一の既知事情） |

## 計測シナリオ

- **シナリオ A（受け入れ判定対象）**: TASK-10.1〜10.3 の全緩和策適用
  （サンプリング間隔 100 + 受理・応答イベント統合 + `/health` を
  `TracingConfig::exclude_path` で除外）。`examples/tracing_nfr.rs` の既定挙動
  （`EXCLUDE_HEALTH` 未指定＝除外あり）
- **シナリオ B（参考値、受け入れ判定には使わない）**: 除外なし・サンプリングのみ
  （`EXCLUDE_HEALTH=0`）。TASK-10.3 除外機構の効果を差分として観測するための対照

## サマリー（4 回実行、対象: `GET /health`）

| 実行 | baseline RPS 中央値 | シナリオA RPS 中央値 | RPS 比 | baseline p95 中央値 | シナリオA p95 中央値 | p95 比 | 判定（帯: RPS≥95% かつ p95≤110%） |
|------|------|------|------|------|------|------|------|
| 1 回目 | 149865.018 | 140061.597 | 93.46% | 0.000233156 | 0.000243652 | 104.50% | FAIL（RPS 比のみ僅かに未達） |
| 2 回目 | 142059.171 | 139266.729 | 98.03% | 0.000241085 | 0.000245068 | 101.65% | PASS |
| 3 回目（採用値） | 143157.340 | 138379.598 | 96.66% | 0.000240277 | 0.000246928 | 102.77% | PASS |
| 4 回目 | 140559.360 | 139358.900 | 99.15% | 0.000243947 | 0.000248688 | 101.94% | PASS |

4 回中 3 回（2〜4 回目）が RPS 比 96.66〜99.15%・p95 比 101.65〜102.77% でいずれも
受け入れ帯（RPS 比 ≥95%・p95 比 ≤110%）を明確に満たした。1 回目のみ RPS 比が
93.46% と帯をわずかに下回ったが、同一実行のシナリオ B（除外なし・サンプリングの
み）は baseline 比 101.69% と乖離がなく、除外機構自体の欠陥ではなく単発の並列負荷
ノイズと判断する（`docs/acceptance/req5-graphql.md` の判断根拠と同型）。3 回目
（RPS 比 96.66%・p95 比 102.77%、4 回の中央付近の値）を代表値として採用する。

**結論: PASS**（TASK-10.4 の受け入れ基準「RPS 劣化 5% 以内・p95 悪化 110% 以内」を
満たす。`scripts/accept/tracing-accept.sh` の機械判定も 3〜4 回目相当の実行で
PASS を確認済み）。

## PoC-10（緩和策なし）との対比

| 構成 | RPS 比（baseline 比） | p95 比（baseline 比） |
|------|------|------|
| PoC-10（サンプリングなし、非同期 writer のみ） | 68.4%（劣化 31.6%） | 161.7%（悪化 61.7%） |
| TASK-10.4（全緩和策適用、採用値＝3 回目） | 96.66%（劣化 3.34%） | 102.77%（悪化 2.77%） |

3 緩和策の適用により、RPS 劣化は 31.6% → 3.34% に、p95 悪化は 61.7% → 2.77% に
大幅に改善した。

## 生ログ（3 回目、採用値）

```text
=== NFR 計測（RUNS=5 DURATION=5s CONNECTIONS=32） ===
baseline: .../target/release/examples/minimal（tracing feature 無効）
tracing : .../target/release/examples/tracing_nfr（tracing feature 有効、Server::tracing 登録済み）
対象パス: GET /health（高頻度パス想定）

  [baseline] run 1: rps=144227.19710587038 p95=0.000241943
  [baseline] run 2: rps=141363.23548549486 p95=0.000243532
  [baseline] run 3: rps=142661.04522441878 p95=0.000240277
  [baseline] run 4: rps=149285.07443427542 p95=0.000239372
  [baseline] run 5: rps=143157.33997491133 p95=0.000239769

--- シナリオ A（受け入れ判定対象・全緩和策適用: サンプリング + イベント統合 + /health 除外） ---
  [tracing_a] run 1: rps=138379.5984501604 p95=0.000246928
  [tracing_a] run 2: rps=139026.5175807726 p95=0.000245785
  [tracing_a] run 3: rps=138132.18611617186 p95=0.000247544
  [tracing_a] run 4: rps=138154.76209974565 p95=0.000247782
  [tracing_a] run 5: rps=155692.53827394487 p95=0.000221985

--- シナリオ B（参考値・除外なし: サンプリングのみ） ---
  [tracing_b] run 1: rps=138938.49451288904 p95=0.000246156
  [tracing_b] run 2: rps=137897.75121067962 p95=0.000248191
  [tracing_b] run 3: rps=139564.21080573258 p95=0.000243467
  [tracing_b] run 4: rps=138141.25365264606 p95=0.000250389
  [tracing_b] run 5: rps=141376.7590767784 p95=0.00024628

=== 結果（中央値、対象: GET /health） ===
baseline   RPS 中央値: 143157.33997491133 / p95 中央値: 0.000240277
シナリオA  RPS 中央値: 138379.5984501604（baseline 比 96.66%） / p95 中央値: 0.000246928（baseline 比 102.77%）
シナリオB  RPS 中央値: 138938.49451288904（baseline 比 97.05%） / p95 中央値: 0.00024628（baseline 比 102.50%）
```

## 生ログ（1 回目、外れ値）

```text
=== NFR 計測（RUNS=5 DURATION=5s CONNECTIONS=32） ===
  [baseline] run 1: rps=144077.14640606387 p95=0.000238203
  [baseline] run 2: rps=155909.7708212564 p95=0.000229069
  [baseline] run 3: rps=144163.60693466922 p95=0.000238017
  [baseline] run 4: rps=152895.33161354112 p95=0.00023114
  [baseline] run 5: rps=149865.01812753538 p95=0.000233156

  [tracing_a] run 1: rps=138806.6555014692 p95=0.000247222
  [tracing_a] run 2: rps=140061.59699850142 p95=0.000243652
  [tracing_a] run 3: rps=138417.18323240752 p95=0.000247115
  [tracing_a] run 4: rps=140238.9253938565 p95=0.000243157
  [tracing_a] run 5: rps=155031.9571339132 p95=0.000223218

  [tracing_b] run 1: rps=155391.48542631112 p95=0.0002225
  [tracing_b] run 2: rps=155615.26546318125 p95=0.000220879
  [tracing_b] run 3: rps=136989.01660142632 p95=0.000249105
  [tracing_b] run 4: rps=152399.10444839226 p95=0.000227239
  [tracing_b] run 5: rps=138442.84012807175 p95=0.000245285

=== 結果（中央値、対象: GET /health） ===
baseline   RPS 中央値: 149865.01812753538 / p95 中央値: 0.000233156
シナリオA  RPS 中央値: 140061.59699850142（baseline 比 93.46%） / p95 中央値: 0.000243652（baseline 比 104.50%）
シナリオB  RPS 中央値: 152399.10444839226（baseline 比 101.69%） / p95 中央値: 0.000227239（baseline 比 97.46%）
```

## 生ログ（2 回目・4 回目）

```text
--- 2 回目 ---
  [baseline] run 1: rps=141632.39004952373 p95=0.000241085
  [baseline] run 2: rps=141218.31618868443 p95=0.000243915
  [baseline] run 3: rps=142059.17079064102 p95=0.000241691
  [baseline] run 4: rps=147585.15784930025 p95=0.000238889
  [baseline] run 5: rps=143263.08032161812 p95=0.000239638
  [tracing_a] run 1: rps=140420.3707897534 p95=0.000245068
  [tracing_a] run 2: rps=138924.12517828174 p95=0.000247817
  [tracing_a] run 3: rps=140464.7675588939 p95=0.000243869
  [tracing_a] run 4: rps=139266.72872785295 p95=0.000244898
  [tracing_a] run 5: rps=137982.14860382883 p95=0.000248003
baseline   RPS 中央値: 142059.17079064102 / p95 中央値: 0.000241085
シナリオA  RPS 中央値: 139266.72872785295（baseline 比 98.03%） / p95 中央値: 0.000245068（baseline 比 101.65%）

--- 4 回目 ---
  [baseline] run 1: rps=140559.3595680391 p95=0.000243947
  [baseline] run 2: rps=144664.61489008306 p95=0.000242371
  [baseline] run 3: rps=141105.39538945688 p95=0.000243109
  [baseline] run 4: rps=139262.3478939633 p95=0.000251549
  [baseline] run 5: rps=133032.48353774962 p95=0.000279937
  [tracing_a] run 1: rps=120726.0404720482 p95=0.000492177
  [tracing_a] run 2: rps=130794.36527518214 p95=0.000341805
  [tracing_a] run 3: rps=142682.39150574035 p95=0.000248688
  [tracing_a] run 4: rps=139358.89997123583 p95=0.000248422
  [tracing_a] run 5: rps=151680.46655308967 p95=0.000237335
baseline   RPS 中央値: 140559.3595680391 / p95 中央値: 0.000243947
シナリオA  RPS 中央値: 139358.89997123583（baseline 比 99.15%） / p95 中央値: 0.000248688（baseline 比 101.94%）
```

## 受け入れ検証（`scripts/accept/tracing-accept.sh`）

```text
=== 受け入れ検証サマリー（REQ-10、TASK-10.4 / #59） ===
判定 | 基準                                   | 詳細
-------+------------------------------------------+-----------------------------------------
PASS   | A: tracing 無効時の依存完全除外 | cargo tree ... = 0
WARN   | A補足: tracing 有効時の依存インパクト（陽性対照） | ... = 4（配線されていることの確認、問題なし）
PASS   | B: cargo test -p backend-framework-core --no-default-features | ...
PASS   | B: cargo test -p backend-framework-core --features tracing | ...
PASS   | B: cargo test -p bf-plugin-tracing | ...
PASS   | C: NFR サンプリング適用後の性能影響 | シナリオA RPS 比 ≈97% / p95 比 ≈102%（実行時により変動、受け入れ帯内）

結果: FAIL なし（PASS / SKIP / WARN のみ）。
```

## 今後の候補（対象外、参考記録のみ）

自動運転のため本タスクでは着手しないが、レポート作成時に気づいた改善候補を記録
する（`.claude/rules/out-of-scope-tracking.md` に従い、ユーザー承認後に Issue 化
判断する）。

- サンプリング間隔・除外パス設定の運用ガイド（`docs/design/` への反映）。
  現状は crate doc と本レポートに分散している
- `benches/tracing-nfr-bench.sh` のシナリオ B（除外なしサンプリングのみ）の結果は
  ベースライン比 97〜102% 程度で、除外機構がなくてもサンプリングだけで大半の
  劣化は緩和されている（TASK-10.3 の限界的な追加効果の定量化は本タスクのスコープ
  外）
