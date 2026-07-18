# TASK-9.5（#65）hub 共通配線 NFR-6 計測レポート

`benches/hub-nfr6-bench.sh`（`oha` による empirical 計測。ベースライン
`crates/core/examples/minimal.rs`（`bf-plugin-hub-wiring` 未リンク） vs 比較対象
`crates/plugin-hub-wiring/examples/hub_service_demo.rs`（`bf-plugin-hub-wiring`
リンク済み・`BF_HUB_GATE=off` で `TenantGate` 未登録）、無関係パス `GET /` への負荷、
RUNS=5・DURATION=5s・CONNECTIONS=32）を計 4 回実行した生ログ・結果。判定は
`scripts/accept/hub-wiring-accept.sh`（`docs/acceptance/req9-hub-wiring.md`）を参照。

## レビュー指摘と是正（測定手法の不備）

初期実装では `hub_service_demo.rs` に `GET /` ルートを一切登録しておらず、無関係パス
計測が実際には「ベースライン: `GET /` → 200」対「hub: `GET /` → 404（未登録ルート）」
という**異なる応答形状**を比較してしまっていた（advisor レビューで指摘）。これは
「無関係パスへの影響」ではなく応答パス自体の違いを測る不正な比較であり、初回の
RPS 比 80〜83% という測定値はこの不備を含んだまま記録されていた。

**是正**: `hub_service_demo.rs::build_router` に `crates/core/examples/minimal.rs` と
同一形状（200・同等サイズの body）を返す `GET /` ハンドラを追加し、以後の測定は
是正後のバイナリで実施した。本レポートの生ログはすべて是正後のもの。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-18 |
| 対象コミット（`origin/main`） | `01b7f1c49eae1e1f99471cb77152b3cb41519e75` |
| oha | 1.15.0 |
| rustc/cargo | 1.96.0 |
| 備考 | **専有環境（PoC-2 同等）ではない**。本 worktree は他イシューの並列実装
  ワークフロー下（`.claude/worktrees/` 配下の複数 worktree が同時に cargo
  build・cargo test 等を実行し得る共有環境）で計測している。下記「診断: ポート
  固有の変動」節の通り、同一環境内でも測定対象ポートの計測タイミングにより
  絶対 RPS が 4〜5 倍変動することを確認しており、環境負荷の変動が支配的な
  要因であることを示す直接的な証拠を得た |

## サマリー（是正後、4 回実行）

| 実行 | baseline RPS 中央値 | hub RPS 中央値 | RPS 比 | baseline p95 中央値 | hub p95 中央値 | p95 比 |
|------|------|------|------|------|------|------|
| 1 回目 | 143855.963 | 122394.147 | 85.08% | 0.000237584 | 0.000492432 | 207.27% |
| 2 回目 | 145463.144 | 119799.288 | 82.36% | 0.000241577 | 0.000506863 | 209.81% |
| 3 回目 | 146055.388 | 119977.466 | 82.15% | 0.000237605 | 0.000507479 | 213.58% |
| 4 回目 | 143768.833 | 80169.944 | 55.76% | 0.000239741 | 0.000882084 | 367.93% |

4 回目は `hub` 側の 5 サブランが 53,054〜121,905 rps と単一実行内で大きく振れており
（下記生ログ参照）、静的なバイナリ差ではなく実行時点の環境負荷変動が支配的である
ことを示している。

## 診断: ポート固有の変動（コントロール実験）

`baseline`（`examples/minimal`）を全く同一のバイナリのまま、ポートだけを変えて
連続計測したところ、片方（本タスクで既に多数回使い回していたポート）は
143K rps 台、もう片方（本タスクで初めて使うポート）は 624K rps 台と、**同一バイナリ・
同一マシンで約 4.4 倍の差**が出た。さらに `hub_service_demo`（`BF_HUB_GATE=off`）を
未使用ポートで `baseline` と直接比較したところ RPS 比 99.23%（621240 vs 626083）と、
狭義帯には届かないものの実務許容帯にほぼ収まる結果を得た。

これは「`bf-plugin-hub-wiring` をリンクしたことによる固有の性能劣化」ではなく、
**本 worktree・本セッションが同一ポートで計測プロセスの起動・停止を多数回繰り返した
ことによる環境側の変動**（ポート単位の負荷・接続状態の蓄積、または並列実行中の
他 worktree の CPU 負荷が特定の測定タイミングに重なったこと）が支配的要因である
可能性が高いことを示す直接的な証拠である。本スクリプト自体（`benches/hub-nfr6-bench.sh`）
の測定手法（是正後）は妥当だが、本セッションの実行環境固有の事情により絶対値が
不安定であり、専有環境での再計測なしに数値を断定できない。

## 生ログ（1 回目）

```text
=== NFR-6 計測（RUNS=5 DURATION=5s CONNECTIONS=32） ===
baseline: .../target/release/examples/minimal（bf-plugin-hub-wiring 未リンク）
hub     : .../target/release/examples/hub_service_demo（bf-plugin-hub-wiring リンク済み・BF_HUB_GATE=off で TenantGate 未登録、GET / は無関係パス）

  [baseline] run 1: rps=143855.96335208393 p95=0.000237553
  [baseline] run 2: rps=142968.66547076957 p95=0.000238729
  [baseline] run 3: rps=143618.60044897677 p95=0.000237644
  [baseline] run 4: rps=144110.2226106941 p95=0.000237584
  [baseline] run 5: rps=143938.07469175154 p95=0.000237143
  [hub(gate off)] run 1: rps=122124.63721637015 p95=0.000492432
  [hub(gate off)] run 2: rps=123044.23441996462 p95=0.000487485
  [hub(gate off)] run 3: rps=122394.86048550722 p95=0.000491
  [hub(gate off)] run 4: rps=122394.14674991502 p95=0.000496528
  [hub(gate off)] run 5: rps=122235.1527346015 p95=0.000495375

=== 結果（中央値、対象: GET / 無関係パス） ===
baseline RPS 中央値: 143855.96335208393
hub      RPS 中央値: 122394.14674991502（baseline 比 85.08%）
baseline p95 中央値: 0.000237584
hub      p95 中央値: 0.000492432（baseline 比 207.27%）
```

## 生ログ（2 回目）

```text
=== NFR-6 計測（RUNS=5 DURATION=5s CONNECTIONS=32） ===
  [baseline] run 1: rps=145252.29786949488 p95=0.000241577
  [baseline] run 2: rps=145463.14393004027 p95=0.000242022
  [baseline] run 3: rps=147871.02495279975 p95=0.000240286
  [baseline] run 4: rps=146903.5034436941 p95=0.000240254
  [baseline] run 5: rps=143543.2473586067 p95=0.000245224
  [hub(gate off)] run 1: rps=125428.07569291725 p95=0.000472324
  [hub(gate off)] run 2: rps=121885.04914781151 p95=0.000491667
  [hub(gate off)] run 3: rps=119799.28802363561 p95=0.000506863
  [hub(gate off)] run 4: rps=119071.58244455997 p95=0.000512802
  [hub(gate off)] run 5: rps=118438.46685884887 p95=0.000518449

=== 結果（中央値、対象: GET / 無関係パス） ===
baseline RPS 中央値: 145463.14393004027
hub      RPS 中央値: 119799.28802363561（baseline 比 82.36%）
baseline p95 中央値: 0.000241577
hub      p95 中央値: 0.000506863（baseline 比 209.81%）
```

## 生ログ（3 回目）

```text
=== NFR-6 計測（RUNS=5 DURATION=5s CONNECTIONS=32） ===
  [baseline] run 1: rps=145587.61010886985 p95=0.00023922
  [baseline] run 2: rps=146055.38797276776 p95=0.000235495
  [baseline] run 3: rps=146539.1193749469 p95=0.000237605
  [baseline] run 4: rps=143477.64474752027 p95=0.000240517
  [baseline] run 5: rps=146109.98828501021 p95=0.000234442
  [hub(gate off)] run 1: rps=119988.63258517228 p95=0.000504555
  [hub(gate off)] run 2: rps=119715.41093257973 p95=0.000508102
  [hub(gate off)] run 3: rps=120023.80970468723 p95=0.000504551
  [hub(gate off)] run 4: rps=119275.46979168251 p95=0.000508363
  [hub(gate off)] run 5: rps=119977.46573124423 p95=0.000507479

=== 結果（中央値、対象: GET / 無関係パス） ===
baseline RPS 中央値: 146055.38797276776
hub      RPS 中央値: 119977.46573124423（baseline 比 82.15%）
baseline p95 中央値: 0.000237605
hub      p95 中央値: 0.000507479（baseline 比 213.58%）
```

## 生ログ（4 回目、単一実行内の振れが顕著）

```text
=== NFR-6 計測（RUNS=5 DURATION=5s CONNECTIONS=32） ===
  [baseline] run 1: rps=146845.2316992093 p95=0.000235197
  [baseline] run 2: rps=147432.76998781154 p95=0.000239741
  [baseline] run 3: rps=142358.77870447366 p95=0.000242163
  [baseline] run 4: rps=143768.83326715525 p95=0.000238407
  [baseline] run 5: rps=142013.30948742112 p95=0.000241424
  [hub(gate off)] run 1: rps=121904.70214029957 p95=0.000491188
  [hub(gate off)] run 2: rps=53054.09718441786 p95=0.001489988
  [hub(gate off)] run 3: rps=64632.54331990965 p95=0.001221967
  [hub(gate off)] run 4: rps=91126.71208079312 p95=0.000702669
  [hub(gate off)] run 5: rps=80169.94448195961 p95=0.000882084

=== 結果（中央値、対象: GET / 無関係パス） ===
baseline RPS 中央値: 143768.83326715525
hub      RPS 中央値: 80169.94448195961（baseline 比 55.76%）
baseline p95 中央値: 0.000239741
hub      p95 中央値: 0.000882084（baseline 比 367.93%）
```

## コントロール実験の生ログ（診断用、参考）

同一バイナリ（`examples/minimal`）を「本セッションで既に多数回使ったポート」と
「本セッションで初めて使うポート」でそれぞれ計測（`oha -z 5s -c 32`、RUNS=5）:

```text
slot1 (既出ポート): rps 中央値 = 143093.53
slot2 (未使用ポート): rps 中央値 = 623924.43
ratio = 436.03%
```

`hub_service_demo`（`BF_HUB_GATE=off`）を未使用ポートで測定順序を入れ替えて比較:

```text
hub(先に計測、未使用ポート)     median rps = 621240.30
baseline(後に計測、未使用ポート) median rps = 626082.01
ratio (hub/baseline) = 99.23%
```

## 参考値: opt-in コスト（ゲート有効時のスループット）

本スクリプトは baseline / gate-off の 2 系統のみを自動計測する（PASS/FAIL 判定対象は
無関係パスへの影響のみのため）。ゲート有効 + 有効トークン時の `/items` 系スループット
（opt-in コスト）は本タスクでは自動計測していない（未計測）。手動計測する場合の手順:

```bash
cargo run --release -p bf-plugin-hub-wiring --example hub_service_demo
# 起動時に標準出力へ表示される curl コマンド例のトークンをそのまま使い、
# oha -z 5s -c 32 -H "Authorization: Bearer <token>" http://127.0.0.1:3100/items
```

## 判断

`docs/acceptance/req9-hub-wiring.md` 「基準 D（NFR-6）の詳細と判断」を参照。是正後も
RPS 比 55.76〜85.08%・p95 比 207.27〜367.93% と実務許容帯（RPS [95%,105%]・p95
[0,105%]）を安定して下回るため FAIL として記録する（フェイルクローズ、
`.claude/rules/security.md`、数値を丸めない）。

ただし上記コントロール実験により、同一バイナリ・同一マシンでも計測対象ポートの
使用履歴・計測タイミングだけで RPS が 4 倍以上変動すること、および未使用ポートで
測定すると `hub_service_demo` と `minimal` の比が 99.23% とほぼ同等になることを
直接確認した。この事実は「`bf-plugin-hub-wiring` のリンク・拡張点登録が無関係パスへ
真の性能劣化を及ぼしている」という仮説よりも、「本セッションの実行環境（並列
worktree・繰り返し計測によるポート単位の負荷蓄積）が測定値を支配している」という
仮説を強く支持する。`crates/plugin-hub-wiring/src/**`・`crates/core/src/server.rs` の
production コードは本タスクで変更していない。

## 専有環境での再現手順（フォローアップ）

1. 他の cargo プロセス・worktree が動作していない専有環境を用意する
2. `cargo build --release -p backend-framework-core --example minimal --no-default-features`
3. `cargo build --release -p bf-plugin-hub-wiring --example hub_service_demo`
4. `bash benches/hub-nfr6-bench.sh` を複数回実行し、本レポートと同じ形式で記録する
   （実行のたびに新規プロセスを起動するため、本レポートで観測したポート単位の
   負荷蓄積は専有環境では再現しない見込み）
5. 狭義帯（100.3〜100.8%）またはせめて実務許容帯（95〜105%）に収まれば、本 FAIL は
   本セッション固有の環境要因に起因していたと確定できる。収まらない場合は
   `RequestGate` 拡張点の評価コスト（`crates/core/src/server.rs` の拡張点走査順）を
   見直す別課題として起票する（out-of-scope-tracking、[[out-of-scope-tracking]]）
