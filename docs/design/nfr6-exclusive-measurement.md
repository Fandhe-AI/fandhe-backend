# NFR-6 専有計測環境の設計（#178）

対応: イシュー #178「test(bench): NFR-6（無関係パスへの性能影響）専有計測環境の整備と再計測」。
関連: TASK-5.2（#53、GraphQL）・TASK-8.4（#29、WebRTC）・TASK-9.5（#65、hub-wiring）。

## 1. 背景

GraphQL・WebRTC・hub-wiring の NFR-6（無関係パスへの性能影響）受け入れ計測は、並列
issue 実装ワークフロー下の host contention により以下の状態で確定できていなかった。

| 対象 | 受け入れ文書・基準 | 現状判定 |
|------|------|------|
| GraphQL | `docs/acceptance/req5-graphql.md` 基準 C | FAIL（RPS 比 93.72〜108.51% と振れ幅大） |
| WebRTC | `docs/acceptance/req8-webrtc-attack-surface.md` 基準 E | FAIL（RPS 比 94〜95% / p95 比 106〜108%） |
| hub-wiring | `docs/acceptance/req9-hub-wiring.md` 基準 D | WARN（専有環境での確定再計測が未了と明記） |

`benches/reports/task-9.5-hub-wiring-performance.md` の「診断: ポート固有の変動」節では、
**同一バイナリ・同一マシンでも計測タイミング・ポート使用履歴だけで RPS が約 4.4 倍変動する**
直接証拠が記録されており、環境ノイズが判定を支配していることが疑われていた。

## 2. 専用 self-hosted runner 案との比較

`.claude/rules/ci.md` は「self-hosted runner の schedule 実行は軽量に保つ」
「ビルドを伴うジョブは schedule から除外する」ことを求めており、oha 実計測を CI
常設ジョブへ足すことはこの負荷抑制方針と衝突する。専用ラベル付き bench 専用 runner の
新設も検討したが、以下の理由で見送り、**手動実行の専有実行枠プロトコル**を採用した。

- 新規 runner の調達・ラベル運用は本イシューのスコープ（計測手順の整備・再計測）を
  超える運用変更を伴う
- 専有実行枠プロトコル（flock + 静穏確認）であれば、既存の self-hosted runner 群を
  変更せず、手動実行時のみ他ジョブとの競合を検知・回避できる
- 受け入れ条件は「専用 self-hosted runner **または**専有実行枠」の後者で足りる

## 3. 専有実行枠プロトコル

`benches/lib/exclusive.sh`（関数定義のみ、副作用なし。`benches/lib/common.sh` と
同じ「単体実行しない」方針）が以下を提供する。

### 3.1 相互排他（flock）

`BF_NFR6_LOCK`（既定 `/tmp/backend-framework-nfr6-bench.lock`）に対する `flock` で
ホストグローバルな相互排他を行う。並列 worktree の同一スクリプト同士が同時に計測しない
ことを保証する。

- ロックパスが symlink の場合は使用を拒否する（world-writable な `/tmp` 配下での
  symlink squat 対策、`.claude/rules/security.md`）
- ロックファイルへデータを書き込むことはない（`exec N>path` で空ファイルを開くのみ。
  squat されても flock 待ちになるだけで情報漏えい・上書き破壊は起きない）
- 取得できない場合は `QUIESCE_WAIT_SECS` を上限に待機し、超過時は BLOCKED
  （`BF_NFR6_BLOCKED_EXIT_CODE`、既定 2）として終了する。バイパス用の強制フラグは
  設けない（フェイルクローズ）

### 3.2 静穏確認（quiescence check）

計測直前に以下を確認する。

- 1 分 loadavg が `LOAD1_MAX`（既定 1.0）以下
- 自プロセス以外に `cargo` / `rustc` / `oha` が稼働していない

いずれかを満たさない場合は `QUIESCE_POLL_INTERVAL_SECS`（既定 30s）間隔で
`QUIESCE_WAIT_SECS`（既定 1800s）まで再試行し、超過時は BLOCKED として終了する
（PASS へ丸めない）。

### 3.3 環境スナップショット

実行日時・対象コミット・`nproc`・loadavg（計測前後）・他プロセス検出結果を
machine-readable な `key=value` 行で出力する。**プロセスは名前のみ記録し、
コマンドライン引数は記録しない**（argv にトークン等が混入していても記録しない、
情報漏えい対策）。

### 3.4 ポート動的採番について（見送り）

計画時点ではポートの動的採番も検討した（`benches/reports/task-9.5-hub-wiring-performance.md`
の「診断: ポート固有の変動」への直接対処のため）。しかし対象 example
（`crates/core/examples/{minimal,webrtc_nfr6,graphql_nfr6}.rs`・
`crates/plugin-hub-wiring/examples/hub_link_only.rs`）はいずれも
`server.bind("127.0.0.1:<port>")` を Rust コード中にハードコードしており、env 経由の
ポート上書きに対応していない。本イシューは `crates/**` を変更しない方針のため、
ポート動的採番機能は実装せず、**flock による直列化（同時に 1 計測のみ）と静穏確認**を
専有性の担保手段とした。ポート固定に起因する変動要因は「同時に他の計測・ビルドが
走っていないこと」が保証されれば影響しないため、直列化で代替できると判断する。

対象 example にポート env 上書きを追加する対応は、`crates/**` の変更を伴うため別
イシューとして切り出す候補とする（本設計判断は「実装しない」ではなく「本イシューの
制約下では実装できない」ことの記録）。

## 4. wrapper（`benches/nfr6-exclusive.sh`）

専有実行枠を取得したうえで、`webrtc` / `graphql` / `hub`（`TARGETS` で選択可）の
既存 nfr6 bench を**順次**実行し、`scripts/accept/lib/nfr6-ratio.sh` の
`evaluate_nfr6_ratio` で PASS/WARN/FAIL を算出する。各対象の計測直前に静穏を
再確認する（前対象の計測完了直後・他ジョブの割り込み開始を検知するため）。

FAIL が残っても wrapper 自体は非 0 で終了しない（判定確定こそが wrapper の責務であり、
FAIL の是非判断は人間レビューへ委ねる）。BLOCKED のみ非 0（`BF_NFR6_BLOCKED_EXIT_CODE`）
で終了し、フェイルクローズを徹底する。

## 5. 再計測の実施結果（#178 時点）

本イシュー実装時点は複数 issue の並列実装ワークフロー実行中であり、専有実行枠の
静穏確認は既定閾値（`LOAD1_MAX=1.0`）では一度も成立しなかった（loadavg が終始 1.9〜6
程度で推移）。`LOAD1_MAX=2.0` へ緩和した 1 回の試行では webrtc 対象の計測は完了したが
（結果は下記）、直後の graphql 対象は静穏再確認時に loadavg 5.96・`cargo`/`rustc` 稼働中を
検知して **BLOCKED** で正しく停止した。これは専有実行枠が「host contention 下では
計測を進めない」という設計どおりに機能した実例であり、判定を丸めていない。

| 対象 | 結果 |
|------|------|
| webrtc（`LOAD1_MAX=2.0` 緩和試行、開始時 loadavg 1.90） | RPS 比 88.10%・p95 比 112.05%（FAIL。既存 FAIL 記録と整合、専有環境の確定値ではなく参考値） |
| graphql | BLOCKED（静穏再取得できず。loadavg 5.96、`cargo`/`rustc` 稼働中） |
| hub | 未実施（graphql の BLOCKED により wrapper が停止） |

既定閾値（`LOAD1_MAX=1.0`）での確定再計測は、他 issue の並列実装が完了し host が
真に静穏な期間に改めて実行する必要がある。3 対象すべての判定は
`docs/acceptance/req5-graphql.md`・`docs/acceptance/req8-webrtc-attack-surface.md`・
`docs/acceptance/req9-hub-wiring.md` の既存記録（GraphQL/WebRTC: FAIL、hub-wiring: WARN）
を暫定的に維持する。確定再計測は本イシューのフォローアップとして別途実施する
（`.claude/rules/out-of-scope-tracking.md` に従い記録）。

## 6. セルフテスト

`scripts/tests/run-nfr6-exclusive-tests.sh` が `benches/lib/exclusive.sh` のみを
source し、`get_loadavg1` / `list_busy_process_names` をモック化して静穏判定・
ロック相互排他・symlink 拒否・BLOCKED 相当の待機超過を cargo/oha/ネットワーク
非依存で検証する。CI への常設組み込みは行わない（兄弟の accept セルフテストと
同じ手動実行、`.claude/rules/ci.md` の schedule 負荷抑制と整合）。
