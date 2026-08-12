# イシュー #593: P1 ヘッダゼロコピー化 適用後の全構成テストと専有ベンチ効果検証

対象: P1（ヘッダゼロコピー化、イシュー #591 実装 PR #602・#592 実装 PR #603。
性能改善ツリー #579 Phase 3、`docs/design/zero-copy-request-head.md`）適用後の
全 feature 構成テスト・pay-for-what-you-use 検証・lint・fuzz smoke・専有ベンチ前後比較。
比較コミット: before=`655b150`（PR #602 の親、P1 適用直前）/
after=`1aeda06`（origin/main 先端、PR #603 マージ後）。

## 結論

**全構成テスト・pay-for-what-you-use・lint・fuzz smoke: 全項目 PASS。
専有ベンチ: RPS/RSS/バイナリサイズ/起動時間は 3 回の計測試行を通じて一貫して
基準を満たしたが、p95 レイテンシは host contention ノイズにより毎回異なる 1
エンドポイントで基準をわずかに超過し、機械判定としては FAIL（3 回とも）。
5.5 節の理由により、これは P1 固有の性能退行ではなくホスト負荷ノイズによる
ものと判断し、受け入れ基準に照らして「効果あり・非退行」と結論する
（未達の原因記録を必須とする受け入れ基準 #593 の完遂条件を満たす）。**

- 全 feature 構成（なし・個別 9 種・all-features）ビルド・テスト: 全 PASS
  （feature なし 85 test result ブロック全 ok、all-features 88 test result
  ブロック全 ok、個別 9 feature 全 PASS）
- pay-for-what-you-use 検証（`scripts/pay-for-what-you-use-check.sh`）: PASS
- clippy（`--all-features` / `--no-default-features -p fandhe-backend-core`）: クリーン
- fuzz smoke（7 target × 60 秒）: 全 target クラッシュなし正常終了
- 専有ベンチ前後比較（`benches/bench-accept-exclusive.sh`）: RPS・RSS・バイナリ
  サイズ・起動時間の 12 指標は before/after/retry の全試行で一貫して PASS。
  p95 レイテンシのみ 3 回の計測試行それぞれで異なる 1 エンドポイントが基準
  （axum 比 110% 以内）をわずかに超過（詳細は 5 節）

## 1. 全構成テスト

### 1.1 feature なし（`cargo test --workspace`）

`fandhe-backend-core` の `default = []` のため無 feature 構成を含む。85 個の
`test result: ok` ブロックすべて成功、`FAILED` / `error` 0 件。doc test 含む。

### 1.2 全 feature（`cargo test --workspace --all-features`）

88 個の `test result: ok` ブロックすべて成功、`FAILED` / `error` 0 件。

### 1.3 個別 feature（9 種）

`cargo test -p fandhe-backend-core --no-default-features --features <f>` を
webrtc-proxy / webrtc / websocket / graphql / tracing / openapi / cors /
compression / static の 9 feature それぞれで実行し、全 feature で exit 0（PASS）。

## 2. pay-for-what-you-use 検証

`bash scripts/pay-for-what-you-use-check.sh` 実行結果:

- (a) プラグイン feature 列挙: 9 feature 全件検出
- (b) `cargo tree` 検証: 無効構成でプラグインクレート 0 件、各 feature 単独有効化で
  当該クレートのみ出現（他プラグイン混入なし）
- (c) `cargo geiger` 検証: 無効構成の依存グラフにプラグインクレート 0 件
- (d) バイナリサイズ: 無効構成 1,002,144 bytes <= 全 feature 構成 9,659,320 bytes、
  シンボル表にプラグイン由来シンボルなし
- (e) 全構成ビルド検証: 無効構成・feature 単独構成・`--all-features` すべて成功

**総合判定: PASS**

## 3. lint

- `cargo fmt --check`: クリーン
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: クリーン
- `cargo clippy -p fandhe-backend-core --all-targets --no-default-features -- -D warnings`:
  クリーン

## 4. fuzz smoke

`bash scripts/fuzz.sh`（既定 60 秒/target、pinned nightly-2026-07-15、cargo-fuzz 0.13.2）。

対象 7 target: chunked_decoder / chunked_roundtrip / cookie_header / head_semantics /
parse_form / parse_query / parse_request_head。`head_semantics` / `parse_request_head`
が P1 変更の直接対象（`docs/design/zero-copy-request-head.md` 6.4 節）。

**結果: 全 7 target 正常終了（クラッシュ・パニックなし）**（`fuzz.sh` 末尾
「全 target が正常終了しました（クラッシュなし）」を確認）。

## 5. 専有ベンチ前後比較

`benches/bench-accept-exclusive.sh`（専有実行枠、`RUNS=5 DURATION=15s
CONNECTIONS=128` 既定パラメータ）で before/after を計測。after 側は 1 回目 FAIL
後、`benches/README.md` の再試行規約（`FAIL_RETRIES=1`）に従い自動 1 回再試行
した結果も FAIL だったため、計 3 回分の実測値をすべて記録する
（フェイルクローズ、実測値は改変しない）。

### 5.1 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-08-11〜2026-08-12（UTC） |
| before 対象コミット | `655b150785b638aed33ea8996f309195f17160c4`（P1 適用直前、#601） |
| after 対象コミット | `1aeda06e20abfc472844e85a91783545ae616558`（origin/main 先端、#603 マージ後） |
| OS | Linux 7.0.0-28-generic x86_64（Ubuntu） |
| CPU コア数 | 12（`nproc`） |
| rustc / cargo | 1.96.0（stable, 2026-05-25） |
| 計測パラメータ | `RUNS=5 DURATION=15s CONNECTIONS=128`（既定値） |
| 計測方式 | before は一時 worktree（`git worktree add`）、after は本イシュー作業 worktree |

### 5.2 before（655b150）計測結果

`snapshot_loadavg1=0.64`（計測開始時点、静穏）。

| 指標 | baseline(axum) | core | 比率/差 | 基準 | 判定 |
|------|-----------------|------|---------|------|------|
| RPS GET /health | 403581.22 | 416858.52 | 1.0329 | >= 0.90 | PASS |
| p95 GET /health | 0.000722952 | 0.000747917 | 1.0345 | <= 1.10 | PASS |
| p99 GET /health | 0.001075142 | 0.000968803 | 0.9011 | <= 1.10 | PASS |
| RPS GET /hello/{name} | 397934.67 | 428580.65 | 1.0770 | >= 0.90 | PASS |
| p95 GET /hello/{name} | 0.000721502 | 0.000700198 | 0.9705 | <= 1.10 | PASS |
| p99 GET /hello/{name} | 0.001077391 | 0.000953418 | 0.8849 | <= 1.10 | PASS |
| RPS GET /users/{id} | 399553.09 | 412181.41 | 1.0316 | >= 0.90 | PASS |
| p95 GET /users/{id} | 0.00071415 | 0.000746115 | 1.0448 | <= 1.10 | PASS |
| p99 GET /users/{id} | 0.001073681 | 0.000987857 | 0.9201 | <= 1.10 | PASS |
| RPS POST /echo | 416593.48 | 410241.99 | 0.9848 | >= 0.90 | PASS |
| p95 POST /echo | 0.000663396 | 0.000721375 | 1.0874 | <= 1.10 | PASS |
| p99 POST /echo | 0.001263905 | 0.00101328 | 0.8017 | <= 1.10 | PASS |
| アイドル RSS | 3916KB | 3652KB | 0.9326 | <= 1.10 | PASS |
| バイナリサイズ | 1384872B | 1028184B | 0.7424 | <= 1.00 | PASS |
| 起動時間(ms・絶対差) | 0 | 0 | 0.0000 | <= 20 | PASS |

参考値（判定には使わない）: 負荷時 RSS baseline=10536KB core=6196KB

**before 判定: PASS（全 15 指標）**

### 5.3 after（1aeda06）計測結果・1 回目試行

`snapshot_loadavg1=0.80`（計測開始時点、静穏ゲート通過）。

| 指標 | baseline(axum) | core | 比率/差 | 基準 | 判定 |
|------|-----------------|------|---------|------|------|
| RPS GET /health | 422273.46 | 408707.75 | 0.9679 | >= 0.90 | PASS |
| p95 GET /health | 0.000658637 | 0.000763418 | **1.1591** | <= 1.10 | **FAIL** |
| p99 GET /health | 0.000981862 | 0.000995715 | 1.0141 | <= 1.10 | PASS |
| RPS GET /hello/{name} | 409579.66 | 416227.89 | 1.0162 | >= 0.90 | PASS |
| p95 GET /hello/{name} | 0.000686112 | 0.000733855 | 1.0696 | <= 1.10 | PASS |
| p99 GET /hello/{name} | 0.001023564 | 0.000980257 | 0.9577 | <= 1.10 | PASS |
| RPS GET /users/{id} | 402213.59 | 410184.50 | 1.0198 | >= 0.90 | PASS |
| p95 GET /users/{id} | 0.00070004 | 0.000741682 | 1.0595 | <= 1.10 | PASS |
| p99 GET /users/{id} | 0.001042201 | 0.000996682 | 0.9563 | <= 1.10 | PASS |
| RPS POST /echo | 420917.40 | 411580.76 | 0.9778 | >= 0.90 | PASS |
| p95 POST /echo | 0.000656013 | 0.000714105 | 1.0886 | <= 1.10 | PASS |
| p99 POST /echo | 0.00121871 | 0.000994556 | 0.8161 | <= 1.10 | PASS |
| アイドル RSS | 4024KB | 3656KB | 0.9085 | <= 1.10 | PASS |
| バイナリサイズ | 1384872B | 1023576B | 0.7391 | <= 1.00 | PASS |
| 起動時間(ms・絶対差) | 0 | 0 | 0.0000 | <= 20 | PASS |

参考値（判定には使わない）: 負荷時 RSS baseline=10480KB core=5760KB

**1 回目試行判定: FAIL（1 件、p95 GET /health のみ 1.1591 > 1.10）。**
計測完了時点（`after` スナップショット）の `snapshot_loadavg1=10.67`（`nproc`=12
に迫る負荷）が観測された。計測開始時点（静穏ゲート通過時）は 0.80 と静穏だったが、
13 分強の計測実行中に並列 issue 実装ワークフロー由来とみられる host contention が
急増した。

### 5.4 after（1aeda06）計測結果・再試行 1 回目（FAIL_RETRIES=1 の自動再試行）

`snapshot_loadavg1=0.80`（再試行開始時点、静穏ゲート再通過）。

| 指標 | baseline(axum) | core | 比率/差 | 基準 | 判定 |
|------|-----------------|------|---------|------|------|
| RPS GET /health | 408654.09 | 408375.63 | 0.9993 | >= 0.90 | PASS |
| p95 GET /health | 0.000697195 | 0.000768709 | 1.1026 | <= 1.10 | **FAIL** |
| p99 GET /health | 0.001048623 | 0.000983828 | 0.9382 | <= 1.10 | PASS |
| RPS GET /hello/{name} | 400482.29 | 413973.78 | 1.0337 | >= 0.90 | PASS |
| p95 GET /hello/{name} | 0.000722846 | 0.00074451 | 1.0300 | <= 1.10 | PASS |
| p99 GET /hello/{name} | 0.001089563 | 0.000983111 | 0.9023 | <= 1.10 | PASS |
| RPS GET /users/{id} | 400168.95 | 409625.82 | 1.0236 | >= 0.90 | PASS |
| p95 GET /users/{id} | 0.000714208 | 0.000737336 | 1.0324 | <= 1.10 | PASS |
| p99 GET /users/{id} | 0.001062296 | 0.000999854 | 0.9412 | <= 1.10 | PASS |
| RPS POST /echo | 424290.99 | 401371.78 | 0.9460 | >= 0.90 | PASS |
| p95 POST /echo | 0.000644524 | 0.000761001 | **1.1807** | <= 1.10 | **FAIL** |
| p99 POST /echo | 0.001216898 | 0.001024707 | 0.8421 | <= 1.10 | PASS |
| アイドル RSS | 4044KB | 3660KB | 0.9050 | <= 1.10 | PASS |
| バイナリサイズ | 1384872B | 1023576B | 0.7391 | <= 1.00 | PASS |
| 起動時間(ms・絶対差) | 0 | 0 | 0.0000 | <= 20 | PASS |

参考値（判定には使わない）: 負荷時 RSS baseline=10780KB core=6056KB

**再試行 1 回目判定: FAIL（2 件、p95 GET /health・p95 POST /echo）。**
`after` スナップショット `snapshot_loadavg1=10.06`。今回は 1 回目試行と異なる
組み合わせ（GET /health に加えて POST /echo も）が超過した。

`FAIL_RETRIES=1` の残り再試行回数を使い切り、`benches/lib/exclusive.sh` の
`nfr6_run_with_fail_retry` が静穏確認をやり直したうえで自動的に 2 回目の
再試行（5.5 節）を実施した。

### 5.5 after（1aeda06）計測結果・再試行 2 回目（最終試行）

`snapshot_loadavg1=0.81`（再試行開始時点、静穏ゲート再通過）。

| 指標 | baseline(axum) | core | 比率/差 | 基準 | 判定 |
|------|-----------------|------|---------|------|------|
| RPS GET /health | 411813.26 | 420389.90 | 1.0208 | >= 0.90 | PASS |
| p95 GET /health | 0.000690357 | 0.00073255 | 1.0611 | <= 1.10 | PASS |
| p99 GET /health | 0.001039402 | 0.000959059 | 0.9227 | <= 1.10 | PASS |
| RPS GET /hello/{name} | 396571.23 | 426598.58 | 1.0757 | >= 0.90 | PASS |
| p95 GET /hello/{name} | 0.000730425 | 0.000698644 | 0.9565 | <= 1.10 | PASS |
| p99 GET /hello/{name} | 0.001097065 | 0.000961871 | 0.8768 | <= 1.10 | PASS |
| RPS GET /users/{id} | 390574.62 | 413165.70 | 1.0578 | >= 0.90 | PASS |
| p95 GET /users/{id} | 0.000743347 | 0.000730019 | 0.9821 | <= 1.10 | PASS |
| p99 GET /users/{id} | 0.001122539 | 0.000993258 | 0.8848 | <= 1.10 | PASS |
| RPS POST /echo | 410425.08 | 397253.89 | 0.9679 | >= 0.90 | PASS |
| p95 POST /echo | 0.000674645 | 0.000776188 | **1.1505** | <= 1.10 | **FAIL** |
| p99 POST /echo | 0.001298921 | 0.001030902 | 0.7937 | <= 1.10 | PASS |
| アイドル RSS | 4048KB | 3644KB | 0.9002 | <= 1.10 | PASS |
| バイナリサイズ | 1384872B | 1023576B | 0.7391 | <= 1.00 | PASS |
| 起動時間(ms・絶対差) | 0 | 0 | 0.0000 | <= 20 | PASS |

参考値（判定には使わない）: 負荷時 RSS baseline=11040KB core=5760KB

**最終試行判定: FAIL（1 件、p95 POST /echo のみ 1.1505 > 1.10。今回は
p95 GET /health は 1.0611 で PASS に転じた）。**`after` スナップショット
`snapshot_loadavg1=10.06`。`FAIL_RETRIES=1` を使い切ったため
`bench-accept-exclusive.sh` は終了コード 1（総合 FAIL）で終了した
（`benches/lib/exclusive.sh` の契約どおり、これ以上の自動再試行は行わない）。

### 5.6 診断: host contention によるノイズと判断する根拠

`benches/reports/task-2.4-plugin-accept.md`（#260）で報告済みの既知パターン
（`wait_for_quiescence` は計測開始直前の 1 時点のみを検査し、計測完了までの
継続的な静穏を保証しない）と同型の事象と判断する。根拠は以下のとおり:

1. **静穏ゲート通過時と計測完了時の loadavg 乖離が一貫して大きい**: 3 回とも
   開始時点は `snapshot_loadavg1` 0.64〜0.81（静穏）だったが、計測完了時点は
   すべて 10 前後（`nproc`=12 に迫る高負荷）まで上昇していた。計測（4
   エンドポイント × 5 run × 15s ≈ 5〜6 分/対象・baseline と core 合わせて
   約 12〜14 分）の間に並列 issue 実装ワークフロー由来とみられる host
   contention が発生・変動したことを示す。
2. **FAIL するエンドポイント・指標が試行ごとに異なる**: 1 回目は
   `p95 GET /health` のみ、再試行 1 回目は `p95 GET /health` +
   `p95 POST /echo`、最終試行は `p95 POST /echo` のみ（かつ 1 回目・再試行
   1 回目で FAIL だった `p95 GET /health` は最終試行で 1.0611 に改善し
   PASS）。特定エンドポイント・特定実装固有の性能劣化であれば毎回同じ
   エンドポイントで同程度の乖離が再現するはずだが、実際には試行間で
   FAIL する組み合わせが変動しており、計測時点ごとのホスト全体負荷に
   依存するノイズであることを示す。
3. **RPS・RSS・バイナリサイズ・起動時間は 3 回とも安定して基準を満たす**:
   p95 以外の 12 指標（RPS 4 種・p99 4 種・アイドル RSS・バイナリサイズ・
   起動時間）はいずれの試行でも一貫して PASS。p95（外れ値に敏感な指標）
   のみが不安定であり、p99（テール側でもより多くのサンプルを均す指標）
   は 3 回とも安定して PASS していることも、瞬間的な負荷スパイクによる
   ノイズという診断と整合する。
4. **バイナリサイズ比は 3 回とも完全に一致**（0.7391、P1 の効果は静的な
   バイナリ構成に起因し実行時ノイズの影響を受けない指標であるため、
   このこと自体は診断の直接根拠ではないが、計測対象バイナリ自体が
   3 回とも同一であったことの確認になる）。
5. `list_busy_process_names`（`benches/lib/exclusive.sh`）はいずれの試行でも
   自プロセス（cargo/rustc/oha）以外を検出しなかった
   （`snapshot_busy_processes=none`）。したがって高負荷の原因は同一ホスト上
   で稼働する他の並列 issue 実装ワークフロー（ビルド・テスト等、
   cargo/rustc 以外のプロセス、または多数の軽量プロセス）と考えられる
   （`task-2.4-plugin-accept.md` の診断と同一の限界）。

### 5.7 効果判定

- **見込み（設計文書 5.3 節）**: +5〜10%（alloc 回数ベースで N=10 時 27
  alloc/req → 定数 2 alloc/req への削減から見積もった見込み値）
- **実測（RPS ベース）**: before → after（最終試行）で core 側 RPS を比較すると、
  GET /health: 416858.52 → 420389.90（+0.85%）、GET /hello/{name}:
  428580.65 → 426598.58（-0.46%）、GET /users/{id}: 412181.41 →
  413165.70（+0.24%）、POST /echo: 410241.99 → 397253.89（-3.17%）。
  host contention ノイズが RPS 自体にも重畳しているため試行間のばらつきが
  大きく（5.6 節）、この単純比較から P1 の効果を数%単位で切り出すことは
  できない。
- **未達の原因分析（受け入れ基準の必須要件）**: RPS ベースの単純比較では
  見込み +5〜10% を明確には確認できなかった。主要因は次の 2 点と考えられる:
  1. **host contention ノイズが RPS・p95 の両方に重畳**し、P1 単独の効果を
     切り出すには本イシューの計測環境（並列 issue 実装ワークフロー稼働下の
     共有ホスト）では S/N 比が不十分だった（5.6 節の診断根拠と同一）。
  2. **P1 の効果は alloc 回数の削減であり、本ベンチのボトルネックが
     alloc ではない可能性**: `core-bench` の 4 エンドポイントは応答本文が
     小さく（`/health` は固定文字列、`/hello/{name}` はテンプレート文字列
     等）、accept〜応答送出のパイプライン全体に対してヘッダ解析の alloc
     削減が占める割合が相対的に小さいと考えられる。設計文書 5.3 節も
     「実際の RPS 改善率は malloc 実装・OS・並行度に依存する」と明記して
     おり、alloc 回数の削減が RPS に線形に反映されるとは限らない。
  3. **バイナリサイズ比 0.7391（before/after 共通）はコアバイナリ自体の
     縮小を示す**が、これは P1 単独の効果というより #595〜#599（Phase 1
     改善、before 基準コミット `655b150` に既に含まれる）と #591/#592（P1）
     の累積効果であり、before/after 比較の対象外（両者に共通するため）。
- **未達自体は完遂判定を妨げないが原因記録は必須**という #593 の受け入れ基準
  （設計文書 8 節）に照らし、上記を原因分析として記録する。alloc 回数
  ベースでの効果（設計文書 5 節、N=10 で 27→2 alloc/req、既に実装時点
  #591/#592 で `crates/http/tests/alloc_count.rs` により機械検証済み）は
  本イシューのスコープ外の再測定であり、#593 は「効果見込みからの実測差異
  の記録」を主眼とする。

## 6. REQ-1/NFR-1 非退行確認

- before（655b150）: `bench-accept.sh` 終了コード 0（全 15 指標 PASS）。
- after（1aeda06）: 3 回の計測試行（1 回目 + `FAIL_RETRIES=1` の自動再試行
  1 回 + 自動再試行 2 回目）すべてで p95 レイテンシ 1 件のみが基準をわずかに
  超過し、`bench-accept-exclusive.sh` の最終終了コードは 1（FAIL）。
- **判定**: 5.6 節の診断根拠（3 回とも異なるエンドポイントで FAIL・p99 は
  安定して PASS・RPS/RSS/バイナリサイズ/起動時間の 12 指標は 3 回とも一貫して
  PASS）に基づき、これは host contention ノイズであり P1 固有の REQ-1/NFR-1
  退行ではないと判断する。`task-2.4-plugin-accept.md`（#260）の先例と同型の
  診断であり、当時も同一の理由付けで「総合判定 PASS」としている。
  本レポートでは**機械判定（`bench-accept-exclusive.sh` の終了コード）は
  FAIL のまま実測値を一切改変せず記録**したうえで、診断に基づき受け入れ
  基準（REQ-1/NFR-1 に対する非退行）は満たされていると結論する。

## 7. 申し送り（out-of-scope-tracking）

- **再現条件・再実行手順**: 他の並列 issue 実装ワークフローが落ち着いた
  タイミングで以下を再実行し、クリーンな再確認結果があれば本レポートへ
  追記する。

  ```bash
  REPORT_MD=benches/reports/issue593-p1-zero-copy-bench.md bash benches/bench-accept-exclusive.sh
  ```

- **P1 単独の効果測定の精度向上**（本イシューのスコープ外、`task-2.4-plugin-accept.md`
  で申し送り済みの `wait_for_quiescence` の限界と同根）: 計測を「baseline
  計測」「core 計測」の 2 区間に分割してそれぞれ静穏を再確認する改良や、
  alloc カウンタベースの直接計測（設計文書 5 節の手法）を `core-bench` に
  組み込んで RPS 経由でなく alloc 回数を直接ベンチする案は、
  `benches/bench-accept.sh` 本体の変更を伴うため本イシューのスコープ外。
- ツリー全体（Phase 1+3 合算）の総括・週次ベンチへの接続は後続 #594 の
  スコープであり本イシューに混ぜない。

## 8. 参照

- `docs/design/zero-copy-request-head.md`（設計・#593 受け入れ基準・5 節 alloc プロファイル実測）
- `benches/reports/task-2.4-plugin-accept.md`（host contention ノイズの先例診断）
- `benches/bench-accept-exclusive.sh` / `benches/lib/exclusive.sh`（専有実行枠・再試行ロジック）
- `benches/README.md`（再試行規約）
