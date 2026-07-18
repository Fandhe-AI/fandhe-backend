# TASK-5.2（#53）GraphQL NFR 計測レポート

`benches/graphql-nfr6-bench.sh`（`oha` による empirical 計測。ベースライン
`crates/core/examples/minimal.rs`（`graphql` feature 無効） vs 比較対象
`crates/core/examples/graphql_nfr6.rs`（`graphql` feature 有効・`Server::graphql`
登録済み）、無関係パス `GET /` への負荷、RUNS=5・DURATION=5s・CONNECTIONS=32）を
5 回実行した生ログ・結果。判定は `scripts/accept/graphql-accept.sh`（`docs/acceptance/
req5-graphql.md`）を参照。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-17 |
| 対象コミット（`origin/main`） | `e3da2960baeaf230cbd84c15d7b6f403742d008d` |
| oha | 1.15.0 |
| 備考 | 本 worktree は並列 issue 実装ワークフロー下で実行されており、他エージェントの並行負荷が計測ノイズに影響している可能性がある（`docs/acceptance/req5-graphql.md` の判断参照） |

## サマリー（5 回実行）

| 実行 | baseline RPS 中央値 | graphql RPS 中央値 | RPS 比 | baseline p95 中央値 | graphql p95 中央値 | p95 比 |
|------|------|------|------|------|------|------|
| 1 回目 | （生ログ未保存、標準出力のみ確認） | | 108.51% | | | 50.25% |
| 2 回目 | （生ログ未保存、標準出力のみ確認） | | 95.74% | | | 103.34% |
| 3 回目 | 146063.898 | 137266.072 | 93.98% | 0.000242882 | 0.000252817 | 104.09% |
| 4 回目（採用値） | 144075.972 | 135023.614 | 93.72% | 0.000242485 | 0.000269905 | 111.31% |
| 5 回目 | 130919.721 | 138086.276 | 105.47% | 0.000451228 | 0.000255348 | 56.59% |

1・2 回目は `scripts/accept/graphql-accept.sh`（`benches/graphql-nfr6-bench.sh` を内部
呼び出し）を直接実行した際の標準出力上でのみ確認し、生ログファイルとしては保存し
損ねている（比率値は `graphql-accept.sh` の PASS/WARN/FAIL 記録・本レポート作成時の
実行記録から転記。捏造ではなく実行時に確認した値だが、再現性のため 3〜5 回目は
生ログを保持した）。

## 生ログ（3 回目）

```text
=== NFR 計測（RUNS=5 DURATION=5s CONNECTIONS=32） ===
baseline: .../target/release/examples/minimal（graphql feature 無効）
graphql : .../target/release/examples/graphql_nfr6（graphql feature 有効、Server::graphql 登録済み）

  [baseline] run 1: rps=142265.09700175456 p95=0.000247523
  [baseline] run 2: rps=146063.8980229493 p95=0.000242882
  [baseline] run 3: rps=142009.28363994643 p95=0.00024459
  [baseline] run 4: rps=148499.42786643188 p95=0.000240784
  [baseline] run 5: rps=157199.7617813137 p95=0.000222627
  [graphql] run 1: rps=153142.47647309632 p95=0.000227651
  [graphql] run 2: rps=136786.54231359533 p95=0.000252817
  [graphql] run 3: rps=139937.34426090124 p95=0.000250451
  [graphql] run 4: rps=136678.93411555467 p95=0.00025297
  [graphql] run 5: rps=137266.07178768603 p95=0.000255128

=== 結果（中央値、対象: GET / 無関係パス） ===
baseline RPS 中央値: 146063.8980229493
graphql  RPS 中央値: 137266.07178768603（baseline 比 93.98%）
baseline p95 中央値: 0.000242882
graphql  p95 中央値: 0.000252817（baseline 比 104.09%）
```

## 生ログ（4 回目、採用値）

```text
=== NFR 計測（RUNS=5 DURATION=5s CONNECTIONS=32） ===
baseline: .../target/release/examples/minimal（graphql feature 無効）
graphql : .../target/release/examples/graphql_nfr6（graphql feature 有効、Server::graphql 登録済み）

  [baseline] run 1: rps=107961.07300186066 p95=0.000649252
  [baseline] run 2: rps=125329.10565235312 p95=0.000557121
  [baseline] run 3: rps=148745.5327910385 p95=0.000242485
  [baseline] run 4: rps=149642.12670650028 p95=0.000235886
  [baseline] run 5: rps=144075.971817528 p95=0.0002382
  [graphql] run 1: rps=138555.19204132882 p95=0.000250647
  [graphql] run 2: rps=129936.87184812396 p95=0.000288193
  [graphql] run 3: rps=135023.6141288493 p95=0.000269905
  [graphql] run 4: rps=127009.48306007539 p95=0.000467158
  [graphql] run 5: rps=135589.4086107412 p95=0.000260435

=== 結果（中央値、対象: GET / 無関係パス） ===
baseline RPS 中央値: 144075.971817528
graphql  RPS 中央値: 135023.6141288493（baseline 比 93.72%）
baseline p95 中央値: 0.000242485
graphql  p95 中央値: 0.000269905（baseline 比 111.31%）
```

## 生ログ（5 回目）

```text
=== NFR 計測（RUNS=5 DURATION=5s CONNECTIONS=32） ===
baseline: .../target/release/examples/minimal（graphql feature 無効）
graphql : .../target/release/examples/graphql_nfr6（graphql feature 有効、Server::graphql 登録済み）

  [baseline] run 3: rps=130919.72094744524 p95=0.000451228
  [baseline] run 4: rps=135146.5595565596 p95=0.000289025
  [baseline] run 5: rps=136176.12340159915 p95=0.000277756
  [graphql] run 1: rps=122388.53105574886 p95=0.000494995
  [graphql] run 2: rps=138086.27612454776 p95=0.000259054
  [graphql] run 3: rps=139097.20598288384 p95=0.000250944
  [graphql] run 4: rps=137153.92186569938 p95=0.000255348
  [graphql] run 5: rps=138122.66471051608 p95=0.000253322

=== 結果（中央値、対象: GET / 無関係パス） ===
baseline RPS 中央値: 130919.72094744524
graphql  RPS 中央値: 138086.27612454776（baseline 比 105.47%）
baseline p95 中央値: 0.000451228
graphql  p95 中央値: 0.000255348（baseline 比 56.59%）
```

（1・2 回目は baseline/graphql run 1〜5 の生ログ行を保存し損ねたため、5 回目の生ログは
baseline run 1・2 の出力が端末バッファの都合で欠落している。結果行の中央値・比率は
実行時に確認済みの値をそのまま転記している。）

## 判断

`docs/acceptance/req5-graphql.md` 「基準 C（NFR）の詳細と判断」を参照。RPS 比
93.72〜108.51%・p95 比 50.25〜111.31% と振れ幅が大きく、狭義帯（100.3〜100.8%相当）は
いずれの実行でも達成せず、実務許容帯（RPS [95%,105%]・p95 [0,105%]）にも安定して
収まらない。最終実行（4 回目）を採用値とし FAIL として記録する（フェイルクローズ、
`.claude/rules/security.md`）。原因は本 worktree が並列 issue 実装ワークフロー下で
実行されているための環境ノイズである可能性が高く、production コード
（`crates/core/src/plugin.rs`・`crates/plugin-graphql/src/lib.rs`）は本タスクで変更して
いない。専有環境での再計測が望ましい（`docs/acceptance/req5-graphql.md` の
BLOCKED / フォローアップ節を参照）。

## 追補（#178）: 専有計測環境での再計測試行

`benches/nfr6-exclusive.sh`（`docs/design/nfr6-exclusive-measurement.md` 参照）で
専有環境での確定再計測を試行した。本イシュー実装時点は複数 issue の並列実装
ワークフロー実行中であり、静穏確認（既定 `LOAD1_MAX=1.0`）は一度も成立しなかった。

wrapper を `LOAD1_MAX=2.0`（緩和・参考値）で実行したところ、webrtc 対象の計測完了後、
graphql 対象の計測直前の静穏再確認で loadavg 5.96・`cargo`/`rustc` 稼働中を検知し
**BLOCKED** で正しく停止した（graphql の再計測は実施できず）。これは専有実行枠が
host contention を検知して計測を進めない設計どおりに機能した実例である
（判定を丸めていない）。

したがって本レポートの GraphQL FAIL 記録は変更しない。既定閾値での確定再計測は
host が真に静穏な期間に改めて実施する必要があり、フォローアップとして別途実施する
（`.claude/rules/out-of-scope-tracking.md`）。
