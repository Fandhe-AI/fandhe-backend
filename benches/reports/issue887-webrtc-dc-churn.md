# イシュー #887: 拒否したデータチャネルの大量開閉と RSS（長時間検証）

## 結論

**有界ではない。** `with_max_data_channels_per_connection` で拒否されるチャネルを 1 接続で
開閉し続けると、server プロセスの RSS は、その接続が生きている間、開閉の累計回数に
ほぼ比例して増え続ける。1 回あたりの増分は、攻撃者が指定する **label の長さ**にほぼ等しい。
同時本数の上限（#877）は、この累計の増加を抑えない。

- 8 バイトの label でも 15 分（約 38 万サイクル）で RSS が約 3 MB から約 110 MB に増えた。
- label が 1 KiB なら 10 分で約 300 MB、16 KiB なら 5 分で約 2 GB に達した。
- churn を行わない対照（同条件 10 分）の傾きは 3.9 KiB/分で、増加は churn による。

## 目的と受け入れ基準との対応

| 受け入れ基準 | 対応 |
|---|---|
| 大量開閉を再現する手順がある | 本書「再現手順」。example `crates/plugin-webrtc/examples/datachannel_churn.rs`、`benches/webrtc-dc-churn.sh`、CI 向け回帰テスト `rejected_channel_churn_keeps_connection_and_does_not_leak_permits` |
| 長時間の RSS 推移を記録し、有界かの結論を書く | 本書「結果」「結論」（変種 A は 15 分、B・D は 10 分、C は 5 分） |
| 有界でない場合の原因と対処方針を記録する | 本書「原因」「対処方針」 |

## 再現手順

```bash
# 15 分の本計測（切り離して起動し、完了を有界に待つ）
DURATION_SEC=900 bash benches/webrtc-dc-churn.sh --detach     # OUT_DIR=... を表示
bash benches/webrtc-dc-churn.sh --wait <OUT_DIR> 540          # 完了するまで繰り返す

# label 長の影響 / 対照（churn なし）
LABEL_BYTES=1024 DURATION_SEC=600 POST_CLOSE_IDLE_SEC=180 bash benches/webrtc-dc-churn.sh --detach
LABEL_BYTES=16384 CONCURRENCY=2 DURATION_SEC=300 POST_CLOSE_IDLE_SEC=150 bash benches/webrtc-dc-churn.sh --detach
CONCURRENCY=0 DURATION_SEC=600 bash benches/webrtc-dc-churn.sh --detach

# CI でも回る短時間の回帰確認（枠の漏れと接続維持のみ。メモリは見ない）
cargo test -p fandhe-backend-plugin-webrtc --test webrtc_datachannel rejected_channel
```

構成: server 役（上限 1、`127.0.0.1` のみ）を起動し、その RSS を `ps` で 5 秒ごとに採取する。
client 役が 1 本目のチャネルを維持したまま、上限を超えるチャネルを並列度 `CONCURRENCY` で
「作成 → server に拒否され `OnClose` → drop」と繰り返す。1 本目のエコー、接続状態、
client 側 `get_stats()` の DataChannel 統計エントリ数も 10 秒ごとに記録する。

## 計測環境

| 項目 | 値 |
|---|---|
| OS / CPU | Darwin 27.0.0 / arm64 |
| rustc | 1.98.1（release ビルド） |
| webrtc / rtc | 0.21.1 / 0.21.1 |
| ホスト負荷 | 実行開始時のロードアベレージ 約 2.5〜5（他の作業と共有。単一ホスト） |
| 共通パラメータ | server の同時チャネル上限 1、RSS 採取間隔 5 秒、churn 前の待機 15 秒 |

## 結果

### 変種ごとの要約

| 変種 | label | 並列度 | 時間 | 累計サイクル | RSS 開始 → 終了 | 後半の傾き | client 統計エントリ数（最終） |
|---|---|---|---|---|---|---|---|
| A | 8 B | 4 | 900 s | 381,015 | 6.0 MB → 110 MB | 約 6.1 MB/分 | 251,692 |
| B | 1 KiB | 4 | 600 s | 328,473 | 6.1 MB → 308 MB | 約 14.8 MB/分 | 216,427 |
| C | 16 KiB | 2 | 300 s | 194,746 | 6.2 MB → 約 2.0 GB（最大） | 下記注参照 | 125,283 |
| D（対照） | 8 B | 0 | 600 s | 0 | 6.1 MB → 7.0 MB | 3.9 KiB/分 | 1 |

待機時（churn 前）の RSS は約 3 MB。全変種で `create_data_channel` のエラーとサイクルの
タイムアウトは 0 件、1 本目のエコーは常に成功、接続は `Connected` のまま維持された。
つまり攻撃は接続を切らず、サービス影響は RSS の増加として現れる。

注（変種 C）: 16 KiB では macOS のメモリ圧縮により `ps` の RSS が大きく上下した
（最大 2.09 GB、途中の採取に 0.3 GB や 0.7 GB がある）。後半の傾きの数値は信頼できないので
表には載せず、最大値と下の突き合わせで評価する。

### 変種 A の推移（churn 開始からの経過秒）

| 経過 | 0 | 120 | 241 | 361 | 481 | 602 | 722 | 842 |
|---|---|---|---|---|---|---|---|---|
| server RSS (MB) | 6.0 | 49 | 80 | 83 | 76 | 82 | 113 | 109 |
| 累計サイクル | - | 155,427 | 218,218 | 251,658 | 281,409 | 323,569 | 356,021 | 373,596 |
| client 統計エントリ数 | - | 102,496 | 144,027 | 166,147 | 185,815 | 213,687 | 235,154 | 246,768 |

増加は一定の速さではなく段階的で、サイクルの速さも落ちていく（最初の 10 秒で約 5.4 万、
その後は 1 分あたり数千〜2 万）。ただし RSS は減らず、統計エントリ数は単調に増える。

### label 長との関係（原因の識別）

サイクル数がほぼ同じ（A: 約 38 万、B: 約 33 万）で label だけを 8 B → 1 KiB にすると、RSS の
増分が約 100 MB → 約 300 MB に増えた。差の約 200 MB を統計エントリ数（約 21 万）で割ると
1 エントリあたり約 0.9〜1 KiB で、label 長とほぼ一致する。変種 C はさらに明確で、
`16,384 B x 125,283 エントリ = 約 2.05 GB` が観測した最大 RSS（約 2.09 GB）と一致する。
同じ rtc コードで動く client の `get_stats()` が返す DataChannel エントリ数は、変種
A・B・C のいずれでも累計サイクルの約 65% で、時間とともに単調に増え続ける。

### close 後の RSS

変種 A・B では client が pc を close した後も 60〜180 秒にわたり RSS は下がらなかった
（B は 308 → 288 MB で約 6%、A は微増）。ただし、これは「接続の終了で解放されない」ことの
証拠ではない。(1) アロケータは解放したメモリを OS へ返さず保持しうる、(2) server が相手の
消失を ICE の失敗として検知するまでに 30 秒前後かかる、(3) macOS の `ps` の RSS は
圧縮の影響を受ける、ため、本計測では「寿命の終わりに回収されるか」を判定できていない。
回収の有無はソース上の後始末（下記）からの推測にとどまる。

## 原因

事前のソース読解（webrtc 0.21.1 / rtc 0.21.1）では、チャネルごとの状態の多くは close 時に
除去される（rtc の `DataChannelRegistry`、SCTP の `Association::streams`）。一方で、
次の箇所だけは**チャネルを open するたびにエントリが追加され、close では削除されない**。

- rtc の統計集計器 `StatsAccumulator::data_channels`
  （`rtc-0.21.1/src/statistics/accumulator/mod.rs`）。open のたびに
  `get_or_create_data_channel(handle, label, protocol)`（`handler/datachannel.rs`）で追加され、
  キーは再利用されない単調増加のハンドル。close 時は状態を更新するだけで削除せず、
  削除は pc の close 時の `clear()` だけである。

観測はこの読解と整合する: (1) client 側の統計エントリ数が累計サイクルとともに単調に増える、
(2) 増分が相手の決める label 長に比例する（エントリが label と protocol を抱えるため）、
(3) 同時本数の上限を超えて拒否される経路でも増える（チャネルは `OnOpen` の直前または直後まで
進むため）。ただし server 側の集計器は外から見えず、client 側の同じコードを代理指標として
いる。「server 側の増加の主因がこの集計器である」ことは、label 比例性と数量の一致による
強い状況証拠であり、ヒーププロファイラによる直接確認ではない。

なお、候補としていた `data_channel_events_tx`（`Drop` 時の `try_lock` 失敗による取り残し）は、
本計測の範囲では独立した増加要因として切り分けていない（label 比例の傾きで説明がつくため）。

## 対処方針

本イシューは検証が目的のため、`src/` の挙動は変えていない。緩和は別の変更として扱う。

1. **本クレートでの緩和（推奨候補）**: 接続ごとに「寿命中に受け付けたデータチャネル open の
   累計数」に上限を設け、超えたら接続を close する（フェイルクローズ。攻撃側に再接続と
   シグナリングのコストを払わせ、1 接続で積める量に天井を作る）。既定値、設定 API
   （`with_max_data_channels_per_connection` と並ぶ設定）、テスト、CHANGELOG を伴うため別 PR にする。
   既定値は正当な利用（同時 16 本以内の通常の開閉）を壊さない水準を選ぶ必要がある。
2. **label・protocol の長さ制限**: 増分は label 長に比例するため、受け入れる label の長さに
   上限を置けば 1 サイクルあたりの量を抑えられる（案 1 と併用可能）。
3. **上流への報告**: close 済みチャネルの統計エントリの削除（または件数の上限）を
   webrtc-rs / rtc に報告する。#888（上流がストリーム数の設定 API を公開したら再検討）と関連する。

別イシューの起票は、`.claude/rules/out-of-scope-tracking.md` に従いユーザーの承認が必要なため、
本書では行っていない（要承認）。

## 既知の限界

- server 内部は代理指標（client 側の統計エントリ数）と RSS でしか見ていない。
- 単一ホスト（macOS、他作業と共有）での計測で、RSS はアロケータの保持と OS の圧縮の影響を受ける。
  絶対値より、label 比例性と対照との差を根拠にした。
- close 後の回収は未確認（上記）。接続を閉じれば回収される、という前提で緩和策を設計する場合は、
  Linux でヒープ統計（例: 回収後の `malloc_trim` 後の RSS）を取り直すこと。
- 並列度 4 で立ち上がりに約 5,000 サイクル/秒を出す負荷であり、実運用の攻撃者の到達速度はネットワーク
  条件に左右される。
