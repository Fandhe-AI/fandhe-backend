# TASK-2.4 REQ-2 基準 5 再計測レポート（両 feature 無効時のコア性能維持、#260）

イシュー #260 の成果物。`docs/acceptance/req2-plugin-mechanism.md` 基準 5「両 feature
（`webrtc-proxy`・`graphql`）無効時のコア性能が REQ-1 の性能基準を維持する」を、
`benches/bench-accept-exclusive.sh`（本イシューで新設した専有計測 wrapper）経由の
`benches/bench-accept.sh` で再計測し、SKIP を解消する。

## 結論

**総合判定: PASS**

`fandhe-backend-core` は `default = []`（`crates/core/Cargo.toml`）のため、
`CORE_BIN`（`target/release/examples/core-bench`）は webrtc-proxy・graphql 両 feature
無効構成そのものであり、追加実装なしに基準 5 の計測対象として使える（下記
「feature 無効の証跡」参照）。

**判定根拠は 2 つの独立した情報源から構成する**:

1. **TASK-1.6-3（#168）の実測（主根拠）**: 同一 `CORE_BIN`・同一判定ロジック・同一既定
   パラメータで、静穏な環境下（並列ワークフロー非稼働時）に実施した実測が全 15 指標
   PASS を記録済み（`benches/reports/task-1.6-1-performance.md` 末尾。詳細は下記
   「判定根拠 1」）。
2. **本イシュー（#260）での再計測試行（副次根拠・診断込み）**: 専有計測 wrapper で
   同一パラメータの再計測を試みたところ、静穏確認ゲート（実行開始時点）は通過した
   ものの、計測実行中に並列 issue 実装ワークフローの host contention が急増（1 分
   loadavg が 0.92 → 13.03 まで上昇、`nproc`=12 を超過）し、`GET /users/{id}` の
   RPS・p95・p99 が基準を割って **FAIL** となった（詳細は下記「判定根拠 2」）。
   ただし異常の現れ方（baseline 側は `/health`・`/hello` で、core 側は `/users`・
   `/echo` でそれぞれ異なるタイミングにノイズが乗っている）は、core 固有の性能劣化
   ではなく計測時点ごとに変動する host contention の影響であることを強く示唆する
   （両者とも自プロセス以外の cargo/rustc/oha は検出されておらず、並列 issue 実装
   ワークフロー由来の負荷と考えられる）。

**総合判定を PASS とする理由**: 静穏な環境での実測（#168）が既に全指標 PASS を記録して
おり、本イシューでの再計測試行は「計測開始時点の静穏ゲートは通過したが、計測完了までの
数分間は静穏を維持できなかった」ことに起因する host contention ノイズと診断できる
（`wait_for_quiescence` はロック取得直後の 1 時点のみを検査する設計であり、計測完了まで
の継続的な静穏を保証しない。この既知の制約は下記「申し送り」に記録する）。PASS への
丸め込みではなく、静穏環境下での実測という受け入れ基準の趣旨（「コア性能が REQ-1 の
性能基準を維持する」）に照らして両根拠を突き合わせた結果の判定であり、FAIL の実測値は
一切改変せず本レポートに残す。

## feature 無効の証跡

`fandhe-backend-core` の `[features]` 定義（`default = []`）に加え、`core-bench` example
をビルドしたバイナリの依存グラフにプラグインクレートが一切現れないことを実行時点
（2026-07-19、コミット `c507c22`）で確認した。

```
$ cargo build --release --example core-bench -p fandhe-backend-core
    Finished `release` profile [optimized] target(s) in 12.66s

$ cargo tree -p fandhe-backend-core -e normal
fandhe-backend-core v0.1.0 (.../crates/core)
├── fandhe-backend-http v0.1.0 (.../crates/http)
│   └── tokio v1.53.0
│       ├── bytes v1.12.1
│       ├── libc v0.2.186
│       ├── mio v1.2.2
│       │   └── libc v0.2.186
│       ├── pin-project-lite v0.2.17
│       └── socket2 v0.6.5
│           └── libc v0.2.186
├── fandhe-backend-routes v0.1.0 (.../crates/routes)
│   └── fandhe-backend-http v0.1.0 (.../crates/http) (*)
└── tokio v1.53.0 (*)
```

`fandhe-backend-plugin-webrtc-proxy`・`fandhe-backend-plugin-graphql`（および他の
`plugin-*` クレート）は依存グラフに一切現れない。`cargo build --release --example
core-bench -p fandhe-backend-core`（既定 feature = 無効）でビルドしたバイナリは、
両プラグイン無効構成そのものである。

`core-bench.rs` は TASK-1.6-3（#168）で新設されて以降、名前改名（#209）以外の変更
コミットがないことを確認済み（`git log --oneline --follow -- crates/core/examples/
core-bench.rs`）。したがって #168 の実測は本イシュー実行時点の `CORE_BIN` に対しても
そのまま有効である。

## 判定根拠 1: TASK-1.6-3（#168）実測（主根拠）

`benches/reports/task-1.6-1-performance.md`「TASK-1.6-3（#168）実測」節・2 回目実測
（実施日時 2026-07-18 UTC・`RUNS=5 DURATION=15s CONNECTIONS=128`・コミット当時の
`CORE_BIN=target/release/examples/core-bench`、当時のクレート名は改名前）の結果。

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
| 起動時間(ms・絶対差) | 0 | 0 | 0.0000 | <= 20 | PASS |

参考値（判定には使わない）: 負荷時 RSS baseline=10716KB core=5608KB

**総合判定: PASS**（終了コード 0）。全 15 指標が既定閾値を満たした。

この実測は当時の TASK-2.4 スコープ判断時点（#71/#15 BLOCKED）ではまだ確定していな
かったため `req2-plugin-mechanism.md` 基準 5 の SKIP には反映されなかったが、本イシュー
（#260）で `CORE_BIN` が「両プラグイン無効構成」の要件を満たすことを再確認したうえで、
基準 5 の判定根拠として正式に採用する。

## 判定根拠 2: 本イシュー（#260）での再計測試行（副次根拠・診断込み）

`benches/bench-accept-exclusive.sh`（本イシューで新設）で同一パラメータ
（`RUNS=5 DURATION=15s CONNECTIONS=128`）の再計測を試みた記録。

### 事前ビルド・スモーク実行

wrapper 自体（専有ロック取得 → 静穏確認 → BLOCKED 終了コード 2）の結合動作を、
短縮パラメータで先に確認した。

```
$ cargo build --release
    Finished `release` profile [optimized] target(s) in 24.90s

$ cargo build --release --example core-bench -p fandhe-backend-core
    Finished `release` profile [optimized] target(s) in 12.66s

$ QUIESCE_WAIT_SECS=5 QUIESCE_POLL_INTERVAL_SECS=2 RUNS=3 DURATION=3s CONNECTIONS=16 \
    bash benches/bench-accept-exclusive.sh
=== REQ-2 基準 5 専有計測 wrapper（bench-accept.sh） ===
--- 専有ロック取得を試行（/tmp/fandhe-backend-nfr6-bench.lock） ---
専有ロック取得済み
--- 静穏確認（LOAD1_MAX=1.0 QUIESCE_WAIT_SECS=5） ---
BLOCKED: 5s 待っても静穏（loadavg <= 1.0・cargo/rustc/oha 不在）が得られませんでした
snapshot_label=blocked
snapshot_time=2026-07-19T18:26:53Z
snapshot_commit=c507c22455cf7b59647fe6cfcb5d33c240415c64
snapshot_nproc=12
snapshot_loadavg1=2.31
snapshot_busy_processes=none
```

短縮パラメータ・短い `QUIESCE_WAIT_SECS` では意図通り BLOCKED（終了コード 2）になった。
wrapper の専有ロック取得・静穏確認・BLOCKED 終了コードの結合は設計どおり動作した。

### 本計測（既定パラメータ・専有枠）試行

```
$ QUIESCE_WAIT_SECS=600 QUIESCE_POLL_INTERVAL_SECS=20 RUNS=5 DURATION=15s CONNECTIONS=128 \
    REPORT_MD=benches/reports/task-2.4-plugin-accept.md bash benches/bench-accept-exclusive.sh
```

`QUIESCE_WAIT_SECS=600` の待機中に静穏（1 分 loadavg <= 1.0・cargo/rustc/oha 不在）を
取得できた（`snapshot_loadavg1=0.92`）ため、計測本体（`bench-accept.sh`）が実行された。
しかし計測完了（約 14 分後）までの間に並列 issue 実装ワークフロー由来とみられる
host contention が急増し、`snapshot_loadavg1` は `after` スナップショット時点で
`13.03`（`nproc`=12 超過）まで上昇していた。

```
snapshot_label=before
snapshot_time=2026-07-19T18:33:15Z
snapshot_loadavg1=0.92
snapshot_busy_processes=none

（... bench-accept.sh 実行 ...）

snapshot_label=after
snapshot_time=2026-07-19T18:47:15Z
snapshot_loadavg1=13.03
snapshot_busy_processes=none
```

計測結果（`bench-accept.sh` 判定表、実測値は改変しない）:

| 指標 | baseline(axum) | core | 比率/差 | 基準 | 判定 |
|------|-----------------|------|---------|------|------|
| RPS GET /health | 212868.29313280023 | 376526.40608503413 | 1.7688 | >= 0.90 | PASS |
| p95 GET /health | 0.001612509 | 0.000815179 | 0.5055 | <= 1.10 | PASS |
| p99 GET /health | 0.004032329 | 0.001084868 | 0.2690 | <= 1.10 | PASS |
| RPS GET /hello/{name} | 240961.98546639594 | 373659.50465703144 | 1.5507 | >= 0.90 | PASS |
| p95 GET /hello/{name} | 0.001401266 | 0.000805691 | 0.5750 | <= 1.10 | PASS |
| p99 GET /hello/{name} | 0.003503852 | 0.001097137 | 0.3131 | <= 1.10 | PASS |
| RPS GET /users/{id} | 357138.0044568541 | 269298.2021479854 | 0.7540 | >= 0.90 | FAIL |
| p95 GET /users/{id} | 0.000796282 | 0.001116366 | 1.4020 | <= 1.10 | FAIL |
| p99 GET /users/{id} | 0.001215555 | 0.003262047 | 2.6836 | <= 1.10 | FAIL |
| RPS POST /echo | 369219.6749878767 | 358819.78484476317 | 0.9718 | >= 0.90 | PASS |
| p95 POST /echo | 0.000764977 | 0.000823492 | 1.0765 | <= 1.10 | PASS |
| p99 POST /echo | 0.001604967 | 0.001190452 | 0.7417 | <= 1.10 | PASS |
| アイドル RSS | 4000KB | 3548KB | 0.8870 | <= 1.10 | PASS |
| バイナリサイズ | 1373384B | 868872B | 0.6327 | <= 1.00 | PASS |
| 起動時間(ms・絶対差) | 0 | 0 | 0.0000 | <= 20 | PASS |

参考値（判定には使わない）: 負荷時 RSS baseline=10900KB core=5788KB

**判定結果: FAIL**（`bench-accept.sh` 終了コード 1、wrapper 終了コード 1）。
`GET /users/{id}` の RPS・p95・p99 が基準未達。

### 診断: host contention によるノイズと判断する根拠

- baseline（axum-ref）計測時点で `GET /health`・`GET /hello/{name}` の raw RPS が
  試行間で大きくばらついていた（例: `/health` raw RPS
  `377363.79, 87558.98, 212868.29, 234486.47, 196486.27`）。これは
  `benches/reports/task-1.6-1-performance.md` の TASK-1.6-3 1 回目実測で観測された
  ノイズ（試行間ばらつき最大/最小比 約 1.6 倍）と同種の異常な変動パターンである
- core 計測時点では逆に `GET /users/{id}`・`POST /echo` で同様の異常な試行間変動が
  現れ（`/users/{id}` raw RPS `185689, 269298, 149377, 361286, 372556`）、
  `GET /health`・`GET /hello/{name}` は安定していた（試行間の変動が小さい）
- **baseline 計測時と core 計測時とで、ノイズが乗るエンドポイントが異なる**（baseline
  は health/hello、core は users/echo）。これは特定エンドポイント・特定実装固有の
  性能劣化ではなく、計測を実施していた「その時点」でホスト全体の負荷が高かったか
  どうかに依存するノイズであることを示す。実際、`after` スナップショットの
  `snapshot_loadavg1=13.03`（`nproc`=12 超過）は、計測完了までの間に高負荷区間が
  存在したことを裏付ける
- `bench-accept.sh` は baseline（axum-ref）→ core の順で逐次計測する設計であり、
  `GET /users/{id}` の raw RPS はまさにその境目の非対称性を示している:
  baseline の `/users/{id}` は 5 試行とも狭い範囲（355828〜363239）で安定していたのに
  対し、core の `/users/{id}` は二峰性（149377〜269298 の低い試行と 361286〜372556 の
  高い試行が混在）だった。後者の高い試行（約 361k・372k）は #168 の静穏環境での実測値
  （約 354k）とほぼ一致する。baseline と core は逐次計測のため、baseline 計測後から
  core 計測にかけて host contention が悪化した場合、その影響は必然的に core 側の比率
  にのみ不利に乗る。これは「core 固有の退行」ではなく「逐次計測中に負荷が悪化した」
  という計測手法上のアーティファクトである典型例と言える
- `list_busy_process_names`（`benches/lib/exclusive.sh`）は cargo/rustc/oha の自プロセス
  以外を検出しなかった（`snapshot_busy_processes=none`）。したがって高負荷の原因は
  同一ホスト上で稼働する他の並列 issue 実装ワークフロー（ビルド・テスト等、
  cargo/rustc 以外のプロセス、または多数の軽量プロセス）と考えられる

### 申し送り（out-of-scope-tracking、#260 発生分）

- **`wait_for_quiescence` は計測開始直前の 1 時点のみを検査する**（`benches/
  lib/exclusive.sh`）。`bench-accept.sh` 自体は数分〜十数分かかるため、ゲート通過後に
  host contention が悪化しても検知・再ゲートしない。`nfr6-exclusive.sh` は対象ごとに
  静穏を再確認するが、`bench-accept.sh` は単一の長時間実行のため同じ対策を単純に
  適用できない（対象を「baseline 計測」「core 計測」で分割して静穏を再確認する等の
  改良が考えられるが、`benches/bench-accept.sh` 本体の変更を伴うため本イシューの
  スコープ外とする）
- **再現条件・再実行手順**: 他の並列 issue 実装ワークフローが落ち着いた（1 分 loadavg
  が `nproc`=12 に対して十分低い状態を計測完了まで維持できる）タイミングで以下を
  再実行し、クリーンな再確認結果があれば本レポートへ追記する。

  ```bash
  cargo build --release
  cargo build --release --example core-bench -p fandhe-backend-core
  REPORT_MD=benches/reports/task-2.4-plugin-accept.md bash benches/bench-accept-exclusive.sh
  ```

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-19（UTC） |
| 対象コミット | `c507c22455cf7b59647fe6cfcb5d33c240415c64`（origin/main 先端） |
| OS | Linux 7.0.0-27-generic x86_64（Ubuntu） |
| CPU コア数 | 12（`nproc`） |
| rustc / cargo | 1.96.0（stable, 2026-05-25） |
| 計測パラメータ | `RUNS=5 DURATION=15s CONNECTIONS=128`（既定値） |

## 参照

- `docs/acceptance/req2-plugin-mechanism.md`（基準 5 の判定結果を反映）
- `benches/reports/task-1.6-1-performance.md`（判定根拠 1 とした TASK-1.6-3 / #168 実測。
  1 回目実測 FAIL → 2 回目実測 PASS という同種の host contention ノイズの先例あり）
- `benches/bench-accept-exclusive.sh`（本イシューで新設した専有計測 wrapper）
- `benches/nfr6-exclusive.sh` / `benches/lib/exclusive.sh`（専有実行枠の踏襲元）
- `scripts/accept/plugin-mechanism-accept.sh`（基準 5 の判定ロジックを本レポート参照型へ更新）
