# REQ-1/NFR-1 性能ベンチの定期実行化（イシュー #285）

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
2. `bench-regression` ラベルで Issue を自動起票する（`ci.yml` dep-audit ジョブの
   `audit-triage` 起票ステップと同一パターン。`.claude/rules/improvement-proposal.md`
   の「自動レイヤ（承認不要）」に該当する、フレームワーク自身の自動監査機構）。
   - FAIL（退行確定）と BLOCKED（計測不能）はタイトルで区別する。計測不能の黙殺も
     「継続検証体制の喪失」であるため、握りつぶさず起票する。
   - 重複起票は `bench-regression` ラベルの既存 open Issue 有無で防止する。
3. Issue 本文には判定結果・実行 URL・計測レポート（`REPORT_MD` の内容）を記載する。

### 起票後の一次対応

1. `FAIL_RETRIES=1 REPORT_MD=... bash benches/bench-accept-exclusive.sh` を手動で
   再実行し、退行の再現を確認する。
2. 再現しない場合は環境ノイズ（host contention 等）と判断し、Issue をクローズする。
3. 再現する場合は `.claude/rules/improvement-proposal.md`（改善提案運用規約）に
   従い対応方針を検討する（性能退行の原因調査・Issue 分解等）。

### BLOCKED の扱い

BLOCKED（専有ロック取得不能・ビルド失敗・静穏未達のいずれか）は計測そのものが
成立しなかったことを意味し、PASS へは丸めない。継続的に BLOCKED になる場合は
self-hosted runner のリソース逼迫（他ジョブとの同時実行等）を疑い、
`QUIESCE_WAIT_SECS` の見直しや runner 増強を検討する（本イシューのスコープ外、
必要であれば別途 Issue 化する）。

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
  固定（dep-audit の cargo-deny/audit 導入ステップと同型）。
- **秘密情報・ログ（A09）**: 使用トークンは `${{ github.token }}` のみ。ベンチ
  出力・環境スナップショット（loadavg・プロセス名）に機密は含まれず、Issue
  本文にも計測結果と run URL のみ記載する。
- **フェイルクローズ**: BLOCKED・起票失敗（gh 障害）でも計測判定（ジョブ失敗）
  自体は変えない（dep-audit と同一原則）。stale PASS 防止（REPORT_MD への結論
  追記契約）は `bench-accept.sh` / `bench-accept-exclusive.sh` 側で既に維持されている。

## スコープ外（out-of-scope-tracking 対象）

- ベンチ結果の履歴蓄積・トレンド可視化（リポジトリへの自動コミットは
  `contents: read` 方針と競合するため今回は行わない）。
- NFR-6（webrtc/graphql/hub）の定期実行化（本イシューは REQ-1/NFR-1 のみ）。
- 退行確定後の自動 bisect・自動修正。
