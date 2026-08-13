# REQ-1/NFR-1 性能ベンチの定期実行化（イシュー #285、#614 で新判定方式へ更新）

## 追補（イシュー #614）

Phase 1（#611・#612）・Phase 2 ハーネス実装（#613）を受け、本 workflow を新判定方式へ
接続した。本節は追補であり、以下の本文（当初 #285 設計）は歴史的経緯として残す。

- **判定 4 値化**: 一次判定（axum 比）を PASS/FAIL/BLOCKED の 3 値から
  PASS/FAIL/BLOCKED/INCONCLUSIVE（判定不能）の 4 値へ拡張した
  （`docs/design/bench-p95-criteria.md`）。p95 のみ `P95_BAND=1` で 3 帯域判定
  （PASS/INCONCLUSIVE/FAIL、判定不能帯の上限は spec 基準値 1.10 ×
  `(1 + P95_MARGIN)`。`P95_MARGIN` 暫定値 0.10 は #616 の較正ランで確定）を
  有効化する。RPS・p99・RSS・バイナリサイズ・起動時間は従来どおり単一しきい値の
  2 値判定のまま
- **多数決化**: 旧 `FAIL_RETRIES=1`（FAIL のみ 1 回再試行、2 連続 FAIL で退行確定）を
  `MAJORITY_TRIALS=3`（`benches/lib/exclusive.sh` の `nfr6_run_with_majority`、
  最大 3 試行の多数決。初回 PASS/BLOCKED は即確定、割れる場合は INCONCLUSIVE へ丸める）
  へ置き換えた
- **退行帰属のための追撃**: `bench-accept` ジョブは FAIL または INCONCLUSIVE 確定時、
  直近の成功 run のコミットを pre として `benches/bench-pair.sh`（交互ペア測定、#613）
  による二次判定を実行し、結果を Issue 本文へ添付する。二次判定の結果は一次判定・
  ジョブの成否を変更しない（証跡のみ）。参照コミットが解決できない場合（初回実行・
  現 SHA と同一）は追撃を実行せずその旨を明記する（silent skip 禁止）
- **月次無条件二次判定（新設ジョブ `bench-pair-monthly`）**: 一次判定の構造的弱点
  （baseline・core が同方向に悪化する run を検知できない、`docs/design/
  bench-hosted-runner.md` 7 節 (iii)）への暫定対応。恒久策（run 系列の統計的
  監視）は #616 の較正ラン標本蓄積が前提のため本イシューでは実装せず、
  「#616 が標本を追加するまで自動検知は機能せず、その間は暫定月次実行が主手段」
  という `bench-hosted-runner.md` の規定どおり、標本蓄積までの繋ぎとして毎月 1 回
  （`0 4 1 * *`）無条件に交互ペア測定を実行する
- **cron 追加**: `0 4 1 * *`（毎月 1 日 04:00 UTC）を追加。日曜と重なる月初でも
  `concurrency: group: bench-schedule, cancel-in-progress: false` の直列化により
  同時実行しない（両ジョブとも同一 concurrency group を使う既存設定を維持）
- **timeout-minutes 再設定**: 週次 `bench-accept` ジョブは 240→300 分（多数決最大
  3 試行 + 追撃分のビルド・計測を worst-case へ加算）。月次 `bench-pair-monthly`
  ジョブは新設で 90 分（専有実行枠を経由しないぶん週次より短い worst-case）。
  実測に基づく最終確定は本 PR の push 前フェーズでは実施していない（後述の
  「引き渡し事項」参照）
- **`actions/cache` 不使用の維持**: 計測系ジョブは差分要因を持ち込まないクリーン
  ビルドを維持する既存方針（下記本文「`CARGO_TARGET_DIR` 隔離」節参照）を変更しない。
  月次ジョブも同方針に従う

### 引き渡し事項（本 PR に含まれない、push 前フェーズの制約）

本実装は「push・PR 作成を行わない」タスクとして実施したため、以下は本 PR に
含まれない（実施は後続の Review フェーズ・別 Issue に委ねる）。

- `workflow_dispatch`（mode=primary / mode=pair）の実行検証・擬似退行注入による
  fail-closed 経路（Issue 自動起票 → クローズ）の実地確認
- 上記の実測所要時間に基づく `timeout-minutes` の最終確定（現在値は worst-case の
  机上積算のみ）
- トリガ (iii)（baseline・core 同方向悪化）の監視ロジック実装（#616 の較正ラン
  標本蓄積が前提のため保留。#616 側で追跡）
- #612 6 節の外れ値機械除外条件（分布逸脱 25% + プローブ証拠）の一次判定 RUNS 内への
  組み込み（#613/#614 いずれの実装にも含まれない残課題。ツリー #607 のレビュー
  ゲートで扱いを判断）
- しきい値暫定値（`P95_MARGIN=0.10`・`PAIR_M2=0.05`・`PAIR_MIN_PAIRS=6`・
  `EXT_CPU_MAX_PCT=5`）の実測較正（#616 のスコープ）

## 追補（イシュー #616）

#616 の較正ランにより、5 節・引き渡し事項に記載の
しきい値（`P95_MARGIN=0.10`・`PAIR_M2=0.05`・`PAIR_MIN_PAIRS=6`・`EXT_CPU_MAX_PCT=5`）は
すべて確定値として確定した（値の変更なし・緩和なし。実測確認済みの範囲と未発火
fail-closed 維持の範囲の区分は `benches/reports/issue616-hosted-runner-calibration.md`
11 節参照）。新判定方式での較正ラン（固定 ref・同一コミット 797245a5 で 5 回、
全ラン success・総合 PASS）+ 二次判定 2 回を実施し、実測ノイズ帯域は
現行しきい値の判定余裕の内側に収まることを確認した。詳細・受け入れ基準の充足状況・
申し送りは同レポートを参照。

---

## 背景・目的

REQ-1/NFR-1（`docs/spec/04-requirements.md`。RPS axum 比 90% 以上・p95/p99 110% 以内）
の判定は `benches/bench-accept.sh` / `benches/bench-accept-exclusive.sh`
（TASK-1.6-1・#71、REQ-2 基準 5 再計測・#260）として整備済みだったが、**手動実行前提で
CI に常設されておらず**、2026-07-18 の再計測 PASS（`benches/reports/task-1.6-1-performance.md`）
以降の性能退行を継続検知する体制がなかった（親イシュー #278 の仕様突合で再確認）。

加えて、初回計測が keep-alive 再接続ノイズ等で **FAIL → 再実行 PASS と振れた実績**が
あり（同レポート「実測結果（1 回目）: FAIL」）、単発 FAIL を退行と誤認しない頑健化
（限定再試行の規約化）が必要だった。

## 制約

`.claude/rules/ci.md` の self-hosted runner 規約:

- 全ジョブ `runs-on: self-hosted` + `timeout-minutes` 必須
- **日次 schedule は dep-audit のみ**（ビルドを伴うジョブは `if:
  github.event_name != 'schedule'` で除外）
- schedule 系 workflow 同士は cron をずらして負荷を分散する
- workflow の `permissions` は最小権限（`contents: read`）、fork PR へシークレットを
  露出するトリガーを追加しない

## 方式設計（比較と採用案）

| 案 | 内容 | 評価 |
|----|------|------|
| A. `ci.yml` に schedule ジョブを追加 | 既存 cron `30 0 * * *` に相乗りし、`github.event.schedule` で分岐 | 「日次 schedule は dep-audit のみ」方針と衝突する。cron 文字列分岐は壊れやすく、ジョブ追加のたびに分岐が複雑化する。**不採用** |
| B. **週次 schedule の別 workflow（採用）** | `bench-schedule.yml` を新設し、週 1 回 + `workflow_dispatch` | `update-external.yml` と同型の前例あり。cron 分散・負荷抑制・関心分離が明快。ビルドを伴う重いジョブを日次方針から完全に切り離せる |
| C. `workflow_dispatch` + 外部定期起動 wrapper | 定期起動をリポジトリ外（cron 等）に持つ | 起動主体がリポジトリ外に漏れて追跡不能になる。**不採用**（`workflow_dispatch` は B の手動実行用として併設する） |

採用した設計（案 B）の骨子:

- **週次**（日次にしない）で self-hosted runner の負荷を抑制する。
- cron は既存の `update-external.yml`（00:00 UTC 日次）・`ci.yml`（00:30 UTC 日次）と
  ずらし **`0 2 * * 0`（日曜 02:00 UTC = 11:00 JST）** とする。
- 計測は **`benches/bench-accept-exclusive.sh` を流用**する（flock 相互排他 + 静穏
  確認 + 環境スナップショット）。ホスト負荷（並列 issue 実装ワークフロー等）による
  誤 FAIL を回避する。判定ロジック（RUNS=5・中央値評価）は `bench-accept.sh` に
  既存のまま委譲する。
- **判定の頑健化**: FAIL（終了コード 1）時に限り、同一専有ロック保持中に静穏確認を
  やり直して 1 回だけ再計測する（`FAIL_RETRIES`、既定 0 で従来挙動維持・
  `bench-schedule.yml` からは 1 を指定）。BLOCKED（終了コード 2）は再試行せず
  フェイルクローズする。詳細規約は `benches/README.md`「定期実行（bench-schedule.yml）」
  節を参照。

## 対象ファイル・変更箇所

| パス | 変更 | 内容 |
|------|------|------|
| `.github/workflows/bench-schedule.yml` | 新規 | 週次 schedule + `workflow_dispatch`。単一ジョブ `bench-accept`（`runs-on: self-hosted`・`timeout-minutes: 240`（PR #291 Bugbot 指摘対応で 180 から延長）・workflow `permissions: contents: read`・ジョブのみ `issues: write` 追加・`concurrency: group: bench-schedule, cancel-in-progress: false`） |
| `benches/lib/exclusive.sh` | 変更 | 再試行判定関数 `nfr6_run_with_fail_retry` を追加（終了コード 1 のときのみ `wait_for_quiescence` → `snapshot_environment` → 再実行、0/2 は即返す） |
| `benches/bench-accept-exclusive.sh` | 変更 | `FAIL_RETRIES`（既定 0）を導入し、`bench-accept.sh` 呼び出しを `nfr6_run_with_fail_retry` 経由に変更 |
| `scripts/tests/run-nfr6-exclusive-tests.sh` | 変更 | 再試行関数のケース追加（初回 PASS・1→0・1→1・BLOCKED 非再試行・FAIL_RETRIES=0 の 5 ケース） |
| `benches/README.md` | 変更 | 「定期実行（bench-schedule.yml）」節を追加 |
| `docs/design/bench-scheduled-run.md` | 新規 | 本ファイル |
| `.claude/rules/ci.md` | 変更 | 週次ベンチ workflow の位置づけ（日次 dep-audit のみ方針の例外ではなく、別 workflow・週次・cron 分散で両立）を追記 |
| `CLAUDE.md` | 変更（軽微） | `benches/` の説明に定期実行 workflow への言及を追記 |

`crates/**` の変更はなし。

## 退行検知時の扱い（通知・起票フロー）

`bench-accept-exclusive.sh` が非 0（FAIL または BLOCKED）で終了した場合:

1. `bench-accept` ジョブ自体を失敗（赤）として終了する（フェイルクローズ、CI の
   通知機構でユーザーに可視化される）。
2. `bench-regression` ラベルで Issue を自動起票する（`.claude/rules/
   improvement-proposal.md` の「自動レイヤ（承認不要）」に該当する、フレームワーク
   自身の自動監査機構）。ラベル冪等作成・重複判定・起票は共通部品
   `Fandhe-AI/actions/idempotent-issue`（SHA 固定）呼び出しへ委譲する
   （イシュー #539。本 workflow 側はタイトル・本文の組み立てのみ担う 2 ステップ
   構成）。`ci.yml` dep-audit ジョブの `audit-triage` 起票は advisory ID ごとの
   可変回数ループを伴うため同置換の対象外（`ci.yml` 側に現行シェル実装を残置）。
   - FAIL（退行確定）と BLOCKED（計測不能）はタイトルで区別する。計測不能の黙殺も
     「継続検証体制の喪失」であるため、握りつぶさず起票する。
   - 重複起票は `bench-regression` ラベルの既存 open Issue 有無で防止する
     （`idempotent-issue` の `search-query` 検索）。
3. Issue 本文には判定結果・実行 URL・計測レポート（`REPORT_MD` の内容）を記載する。

### 起票後の一次対応

1. `FAIL_RETRIES=1 REPORT_MD=... bash benches/bench-accept-exclusive.sh` を手動で
   再実行し、退行の再現を確認する。
2. 再現しない場合は環境ノイズ（host contention 等）と判断し、Issue をクローズする。
3. 再現する場合は `.claude/rules/improvement-proposal.md`（改善提案運用規約）に
   従い対応方針を検討する（性能退行の原因調査・Issue 分解等）。

### BLOCKED の扱い

BLOCKED（専有ロック取得不能・ビルド失敗・静穏未達・baseline / `CORE_BIN`
バイナリ未整備のいずれか）は計測そのものが成立しなかったことを意味し、PASS へは
丸めない（イシュー #478 で baseline バイナリ欠如も本 BLOCKED 契約へ統一し、
「性能退行 FAIL」との誤起票を解消した）。継続的に BLOCKED になる場合は
self-hosted runner のリソース逼迫（他ジョブとの同時実行等）を疑い、
`QUIESCE_WAIT_SECS` の見直しや runner 増強を検討する（本イシューのスコープ外、
必要であれば別途 Issue 化する）。

`nfr6_run_with_fail_retry`（`benches/lib/exclusive.sh`）は呼び出し対象コマンドに
「決定論的な環境失敗は終了コード 1（FAIL）で返さず BLOCKED（終了コード 2）で
返す」契約を課している。この契約が守られないと、決定論的失敗をノイズ起因の
FAIL と誤認して無意味な再試行（静穏待機込み）が発生し、`bench-accept.sh` の
追記型レポート生成が複数回呼ばれて同一文言の「## 結論」セクションが
REPORT_MD へ重複追記される（イシュー #476 で実証、#478 で baseline 欠如の
exit コードを 1 → 2 へ統一して実害を解消済み、#479 で契約自体を
`nfr6_run_with_fail_retry` の doc comment・`benches/README.md` へ明文化）。

## `CARGO_TARGET_DIR` 隔離と実効パス導出（イシュー #480）

2026-08-02 の週次実行（run 30729910081）で、`cargo build --release` 成功
直後に `target/release/axum-ref` が見つからず BLOCKED になる事象が発生した
（詳細な一次証拠・タイムライン・確度の内訳は
`benches/reports/issue480-target-dir-investigation.md` 参照）。

**根本原因（高確度）**: self-hosted runner フリート（org: Fandhe-AI）は、
同一 org の他リポジトリ（fandhe-frontend）の CI が明示的に前提とするとおり、
ホスト共有の `CARGO_TARGET_DIR=/cargo-target` をジョブへ注入する構成である
可能性が高い。この場合 `cargo build` の成果物はリポジトリ直下の `target/`
ではなく `/cargo-target` 配下に生成され、`benches/lib/common.sh` 等が決め
打ちしていた `${WORKSPACE_ROOT}/target/release/...` には最初から存在しない
（「ビルド成功直後に消失した」のではなく「そのパスには最初から生成されて
いない」が実態）。実機（runner）への直接アクセスができない開発環境からの
調査のため「確証」ではなく「高確度の推定」に留まる点、および対応が
この推定の正否に依存しない設計にしてある点は上記レポートの 4・5 節を参照。

**対応（2 層防御）**:

1. **ジョブローカル `CARGO_TARGET_DIR` の設定**: `bench-accept` ジョブへ
   `env: CARGO_TARGET_DIR: ${{ github.workspace }}/target` を設定し、
   ホスト共有 `/cargo-target` から意図的に隔離する。これは
   `benches/lib/common.sh` 側の対応がなくても単独で症状を解消する
   （fandhe-frontend #1192 で実証済みの「共有 target 上の成果物汚染」
   リスクの回避も兼ねる）。
2. **`BENCH_TARGET_DIR` による実効パス導出**: `benches/lib/common.sh` に
   実効 target ディレクトリを導出するヘルパーを追加した。優先順位は
   (a) `CARGO_TARGET_DIR` env（非空なら最優先、相対パスは workspace 基準で
   絶対化）、(b) `cargo metadata --no-deps` の `target_directory`
   （`.cargo/config.toml` の `build.target-dir` も正しく反映する cargo
   自身の権威値。ただし `Cargo.lock` が gitignore 対象でネットワーク
   アクセスを伴いうるため 専有ロック保持中の呼び出し元では (a) で
   短絡させる）、(c) 従来どおり `${WORKSPACE_ROOT}/target`。ベンチ各
   スクリプトの `TARGET_BIN` / `BASELINE_BIN` / `CORE_BIN` 等の既定値は
   すべてこの `BENCH_TARGET_DIR` を基準にする。2 層目を持つ理由は、
   1 層目（ジョブローカル env）を将来のリファクタで見落としても実効パスの
   決め打ち依存が残らないようにするため。
3. **ビルド直後の fail-fast**: `bench-accept-exclusive.sh` は静穏確認
   （最大 30 分待機）に入る前にバイナリの実在を検査し、欠如時は
   `FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE`（2）で即 BLOCKED 終了する。
   欠如は静穏待機を待っても解消しないため、待機を浪費せず早期に検出する
   （`bench-accept.sh` 側の FAIL/BLOCKED 判定〔#478 の担当範囲〕とは独立）。

## セキュリティ考慮（OWASP Top 10 観点）

- **最小権限（A01/A05）**: workflow 全体は `permissions: contents: read`。
  `issues: write` は起票を行うジョブにのみ付与する（dep-audit ジョブと同一
  パターン）。`pull_request_target` 等 fork へシークレットを露出するトリガーは
  追加しない（`schedule` / `workflow_dispatch` のみ）。
- **インジェクション（A03）**: Issue 本文は `--body-file` でファイル渡しし、ベンチ
  出力・レポート内容をシェル再解釈やコマンド置換に埋め込まない。`gh issue list`
  の検索はラベル指定（固定値）のみで外部入力を含めない。
- **リソース枯渇 / runner 占有（DoS、NFR-10）**: `timeout-minutes: 240`
  （専有ロック取得待ち・計測前静穏待機・再試行前静穏待機の 3 待機フェーズ
  （各最大 1800s = 30 分、合計 90 分）+ ビルド + 計測 2 試行の実処理時間を包含。
  PR #291 Bugbot 指摘対応で 180 分から延長。旧 180 分は上記 3 待機フェーズの
  うちロック取得待ちと再試行前静穏待機を予算に含めておらず、ホスト競合時に
  worst-case がジョブタイムアウトを超えて PASS/FAIL/BLOCKED 判定確定前にジョブが
  キャンセルされうる不備があった）・`concurrency` で同時実行を抑止・flock で
  ホスト上の他計測との相互排他・週次実行で負荷を抑制する。
- **サプライチェーン（A06/A08）**: `actions/checkout` は既存 workflow と同一
  コミット SHA に固定する。`oha` 導入は `cargo install --locked` + バージョン
  固定（dep-audit の cargo-deny/audit 導入ステップと同型）。ジョブローカル
  `CARGO_TARGET_DIR`（イシュー #480）は、ホスト共有 `/cargo-target` 上で
  他リポジトリ・他 runner ジョブの成果物と混ざる・上書きされるリスク
  （fandhe-frontend #1192 で実証済み）を排除し、計測対象バイナリの完全性を
  保証する。
- **秘密情報・ログ（A09）**: 使用トークンは `${{ github.token }}` のみ。ベンチ
  出力・環境スナップショット（loadavg・プロセス名）に機密は含まれず、Issue
  本文にも計測結果と run URL のみ記載する。
- **フェイルクローズ**: BLOCKED・起票失敗（`idempotent-issue` action の失敗）でも
  計測判定（ジョブ失敗）自体は変えない（判定は「判定結果でジョブを終了」ステップが
  `if: always()` で独立に決めるため）。イシュー #539 で共通部品へ置換したことにより、
  重複検索（`search-query`）失敗時の挙動は旧実装の `|| echo "0"`（フェイルオープン。
  gh 障害を「既存なし」と誤判断し重複起票しうる）からフェイルクローズ（異常終了し
  重複起票を防ぐ）へ改善された。stale PASS 防止（REPORT_MD への結論追記契約）は
  `bench-accept.sh` / `bench-accept-exclusive.sh` 側で既に維持されている。

## スコープ外（out-of-scope-tracking 対象）

- ベンチ結果の履歴蓄積・トレンド可視化（リポジトリへの自動コミットは
  `contents: read` 方針と競合するため今回は行わない）。
- NFR-6（webrtc/graphql/hub）の定期実行化（本イシューは REQ-1/NFR-1 のみ）。
- 退行確定後の自動 bisect・自動修正。
