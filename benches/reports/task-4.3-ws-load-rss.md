# TASK-4.3（#24）10,000 同時 WebSocket 接続負荷試験・RSS 再計測レポート

`docs/spec/05-tasks.md` TASK-4.3・Issue #24 の成果物。TASK-4.1（#22）・TASK-4.2（#23）
で確立した WebSocket プラグイン（`crates/plugin-websocket`）と「委譲後の専用タスク
再 spawn + permit 引き継ぎ」最適化の RSS 削減効果を、`benches/bench-ws-load.sh` +
`crates/ws-load-client` で正式に再計測した結果。

## 実施日時・環境

- 実施日時: 2026-07-18（JST）
- OS: Linux 7.0.0-27-generic x86_64（Ubuntu）
- CPU コア数: 12（`nproc`）
- メモリ: 31GiB
- rustc / cargo: 1.96.0（stable, 2026-05-25）
- `ulimit -n`: 524288（要求値を十分に充足）
- `/proc/sys/net/ipv4/ip_local_port_range`: `32768-60999`（幅 28,231。10,000 接続 +
  1,000 の要求 11,000 を充足）
- 計測パラメータ: `RUNS=3 HOLD_SECS=30 RAMP_BATCH=500 RAMP_DELAY_MS=20`
  （既定の `HOLD_SECS=60` より短縮したスモーク〜中規模設定。詳細は「計測の完遂状況」節）

## 計測方法

- fullscratch: `crates/core/examples/ws_echo.rs`（`websocket` feature 有効、
  `MAX_CONNECTIONS=接続数+100`）
- axum: `crates/axum-ref`（`ws` feature 有効、専用 `--target-dir target/ws-bench`）
- 負荷生成: `crates/ws-load-client`（PoC-7 `load-client` の移植・改良）
- 各接続数ティア（1,000 / 5,000 / 10,000）× 各実装で `RUNS` 回試行し、負荷印加中の
  サーバ RSS を 1 秒間隔でサンプリングして中央値を取り、
  `(負荷時 RSS 中央値 − アイドル RSS) / 確立接続数` を「接続あたり RSS 増分」とする
  （`benches/bench-rss.sh` と同じ中央値評価の思想）

## ビルドコマンド

```bash
cargo build --release -p backend-framework-core --features websocket --example ws_echo
cargo build --release -p axum-ref --features ws --target-dir target/ws-bench
cargo build --release -p ws-load-client
```

## 実行コマンド

```bash
CONNECTION_TIERS="1000 5000 10000" HOLD_SECS=30 RUNS=3 RAMP_BATCH=500 RAMP_DELAY_MS=20 \
    RESULT_JSON=/tmp/ws-bench-full-result.json bash benches/bench-ws-load.sh
```

## 計測の完遂状況（重要・数値を偽らず記録する）

本セッションの時間予算の制約により、**fullscratch 側の 1,000/5,000/10,000 接続の
3 ティアは RUNS=3（10,000 接続のみ 2 試行完了・1 試行実行中で中断）まで完遂したが、
axum 側の計測は本レポート作成時点で未実施**。以下はハーネスが実際に取得した
実測値であり、architected な推定・捏造は一切行っていない
（`.claude/rules/security.md` のフェイルクローズ原則）。

したがって、受け入れ条件(2)「接続あたり RSS 増分が axum 実装比 110% 以内」の
**axum 比較そのものは本レポート時点で未判定**。ハーネス自体は
`CONNECTION_TIERS="50"` 等の縮小パラメータで fullscratch/axum 双方の完全な
往復（成立率・RSS 増分・axum 比の算出・PASS/FAIL 判定・`RESULT_JSON` 出力）が
正しく動作することをスモークテストで確認済みであり（下記「ハーネス動作確認
（スモーク）」節）、正式な 10,000 接続本計測は本ハーネスを使って別途実行することで
完了できる状態にある。

## fullscratch（`crates/core/examples/ws_echo.rs`）実測結果

| 接続数 | 試行 | idle RSS (KB) | 負荷時 RSS 中央値 (KB) | 確立接続数 | 確立・維持成功率 | 接続あたり増分 (KB) |
|---|---|---|---|---|---|---|
| 1,000 | 1 | 3600 | 140168 | 1000 | 100.00% | 136.5680 |
| 1,000 | 2 | 3672 | 140164 | 1000 | 100.00% | 136.4920 |
| 1,000 | 3 | 3628 | 141344 | 1000 | 100.00% | 137.7160 |
| **1,000（中央値）** | | | | | **100.00%** | **136.5680** |
| 5,000 | 1 | 3532 | 677940 | 5000 | 100.00% | 134.8816 |
| 5,000 | 2 | 3572 | 677940 | 5000 | 100.00% | 134.8736 |
| 5,000 | 3 | 3620 | 678188 | 5000 | 100.00% | 134.9136 |
| **5,000（中央値）** | | | | | **100.00%** | **134.8816** |
| 10,000 | 1 | 3636 | 1351620 | 10000 | 100.00% | 134.7984 |
| 10,000 | 2（中断時点で速報値のみ） | — | — | 10000 | 100.00% | 参考値: 試行 1 とほぼ同水準（心拍レイテンシ p50=449us・p95=1271us で試行 1 と同等の分布） |
| 10,000（3 試行完遂前に中断） | | | | | | |

### 受け入れ条件(1)・(3) に対する fullscratch 側の暫定評価

- **確立・維持成功率**: 1,000/5,000/10,000 の全ティア・全試行で 100.00%（基準 99% 以上を
  充足）。10,000 接続でも切断・タイムアウトは 1 件も観測されなかった
- **1,000→10,000 の線形性**: 接続あたり RSS 増分は 136.57KB（1,000）→134.88KB
  （5,000）→134.80KB（10,000、試行 1）とほぼ一定であり、**接続数に対して強い線形性
  を示している**（PoC-7 が指摘した「1k→10k のリソース線形性」は本再計測でも
  再現された）
- axum 実装比（受け入れ条件(2)）は前述のとおり本レポート時点で未算出

## axum-ref（`ws` feature 有効）実測結果

本セッションの時間予算内で実施できず、**未実測**。`benches/bench-ws-load.sh` を
そのまま再実行すれば同一手順で取得できる（下記コマンド参照）。

```bash
# fullscratch は既に計測済みのため axum のみ再計測する場合の例
# （bench-ws-load.sh は fullscratch → axum の順に両方実行する仕様のため、
#  axum のみに絞る簡易な env は用意していない。全体を再実行することを推奨）
CONNECTION_TIERS="1000 5000 10000" HOLD_SECS=60 RUNS=3 \
    RESULT_JSON=/tmp/ws-bench-full-result.json bash benches/bench-ws-load.sh
```

## ハーネス動作確認（スモーク）

縮小パラメータ（`CONNECTION_TIERS="30"` `CONNECTION_TIERS="50"` `HOLD_SECS=3〜5`
`RUNS=3`）で fullscratch・axum 双方を含む完全な往復を複数回実行し、以下を確認した:

- 前提検査（`ulimit -n`・エフェメラルポート範囲・バイナリ存在）が正しく機能すること
- 両実装で接続確立・心拍・RSS サンプリング・確立・維持成功率算出が正しく動作すること
- axum 比の算出・PASS/FAIL 判定・`RESULT_JSON` 出力（`jq` で妥当な JSON として
  パース可能）が正しく機能すること

30 接続でのスモーク結果（参考値、判定対象ではない）:

```
impl         connections  rss_increment_kb     success_rate_pct
fullscratch  30           144.6667             100.00
axum         30           154.6667             100.00

axum 比（30 接続時点）: 93.53%
判定(1) RSS 増分 axum 比 110% 以内: PASS
判定(2) 確立成功率 99% 以上（全 impl × 全接続数）: PASS
=== 総合判定: PASS ===
```

50 接続でのスモーク結果（参考値）:

```
impl         connections  rss_increment_kb     success_rate_pct
fullscratch  50           142.4000             100.00
axum         50           148.5600             100.00

接続あたり RSS 増分 axum 比（50 接続時点）: 95.85%（基準: 110% 以内）
=== 総合判定: PASS ===
```

小規模計測（30・50 接続）では axum 比 93〜96% と基準（110% 以内）を大きく下回って
おり、TASK-4.2 の最適化が効果を発揮している可能性を示唆するが、**この参考値は
1,000〜10,000 接続の本計測とは規模が大きく異なり、そのまま外挿すべきではない**
（PoC-7 でも小規模と 10,000 接続本番規模とで傾向が変わる可能性が指摘されている）。

## 判定（現時点）

| 受け入れ条件 | 判定 |
|---|---|
| (1) 10,000 同時接続の確立・維持（成立率 99% 以上） | **fullscratch側は PASS**（100.00%）。axum 側は未実施 |
| (2) 接続あたり RSS 増分が axum 比 110% 以内 | **未判定**（axum 側本計測が未実施のため算出不能） |
| (3) 1,000→10,000 の RSS・CPU 線形性 | **fullscratch 側は PASS 相当**（134.80〜137.72KB/接続で概ね一定）。axum 側は未実施のため相対比較としての線形性検証は未完了 |

**総合判定: 未完了（Conditional Go 条件(1)の正式判定に必要な axum 比較が本
レポート時点で欠落している）。**

## 残作業・次アクション

- `benches/bench-ws-load.sh`（既定パラメータ `HOLD_SECS=60 RUNS=3
  CONNECTION_TIERS="1000 5000 10000"`）をそのまま実行し、axum 側の実測値を
  取得して本レポートを更新する。ハーネス自体はスモークテスト・fullscratch 本計測の
  両方で動作実績があるため、追加実装は不要
- 実測が揃い次第、受け入れ条件(2)（axum 比 110% 以内）・線形性の最終判定を追記する
- 110% 超過が判明した場合は実装計画§8 のコンティンジェンシー（`WebSocketConfig` の
  `read_buffer_size`/`write_buffer_size` 調整、担当 `plugin-builder`）を検討し、
  それでも未達なら数値を偽らず未達と明記して Conditional Go 条件(1)の判断材料として
  報告する
