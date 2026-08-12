# イシュー #616 ホステッドランナー較正レポート

## 0. 結論（先出し）

**新判定方式（`P95_BAND=1` + `MAJORITY_TRIALS=3`）が有効化されたコミット上での
同一コミット × 5 回以上の較正ラン、および確定しきい値での 1 サイクル green 確認
（受け入れ基準 1・3）は、本レポート作成時点では実施できていない。**

理由: 本イシューの実装は push・PR 作成を行わない自動運転エージェントが担当し、
`workflow_dispatch` の dispatch にはリモートブランチへの push が前提となるため、
このフェーズでは新規の較正ラン投入ができない（詳細は 4 節）。

この制約下で、以下の fail-closed 方針を採用する。

- **しきい値・試行回数はすべて現状の値（暫定値）を維持し、「#616 較正で確定」と
  文書へ記載しない**。値を動かさないことが最も安全側であり、根拠なき変更（特に
  緩和方向）を行わないという #612 11 節・`.claude/rules/security.md` の
  fail-closed 原則に従う
- 較正ランの実測系列が未収集であることを設計文書・スクリプトコメントへ明記し、
  「#616 で確定（未了）」という誤った既完了表示を残さない
- 読み取り専用で取得可能な既存データ（旧判定方式での `bench-schedule.yml` 実行
  履歴、`microbench` ジョブの run 間一致確認）のみを本レポートに整理し、次フェーズ
  （push 可能なエージェント、または人間による `workflow_dispatch` 投入）が較正を
  再開できるよう申し送る

受け入れ基準の充足状況:

| 受け入れ基準 | 状態 |
|---|---|
| 1. 新判定方式・同一コミットで 5 回以上の較正ラン | **未達**（4 節）。次フェーズへ引き継ぎ |
| 2. 実測ノイズ帯域に基づくしきい値・試行回数の最終値確定 | **未達**（fail-closed 方針により暫定値を維持・較正未完了。5〜6 節） |
| 3. 確定しきい値での 1 サイクル green 完走確認 | **未達**（4 節、基準 1 が前提のため） |
| 4. ドキュメント追随 | 達成（本レポート + 7 節記載の追随箇所） |

## 1. 較正対象コミット（BASE_SHA）

- BASE_SHA: `b0b8745aad12f956ea5454eb3893c367ff799587`（`origin/main` HEAD、
  作成時点。新判定方式を導入した #621
  （`a11178f28b341c941e3c3752f84539b36627bc37`、マージ日時
  2026-08-12T14:55:04Z）を含む）
- 本イシューの作業ブランチ `test/616-calibration-thresholds` は BASE_SHA から作成

## 2. `bench-schedule.yml` 実行履歴（既存 5 run、すべて旧判定方式コミット）

`gh run list --workflow=bench-schedule.yml --limit 20` で取得した全数（結果に
条件付けた抽出は行っていない）。

| run ID | headSha（先頭 8 桁） | event | conclusion | createdAt (UTC) |
|---|---|---|---|---|
| 31290864613 | 73e3212a | schedule | success | 2026-08-09T02:41:41Z |
| 31237646850 | 64b2fe28 | workflow_dispatch | success | 2026-08-08T03:38:30Z |
| 31236262022 | a4a107f6 | workflow_dispatch | success | 2026-08-08T03:00:51Z |
| 30729910081 | 598a9be4 | schedule | failure | 2026-08-02T03:02:19Z |
| 30185507560 | 72c534d2 | schedule | failure | 2026-07-26T03:02:16Z |

いずれのコミットも `a11178f`（新判定方式導入）より前であり、`P95_BAND=1` +
`MAJORITY_TRIALS=3` の判定ロジックを経ていない。個々の run の RPS 比・p95 比・
p99 比の実測値そのものはノイズ分布の参考情報として無価値ではないが、**判定方式
（帯域・多数決）の較正データとしては使えない**ため、本レポートでは数値の転記を
見送り、`docs/design/bench-hosted-runner.md` 2 節（既存 n=3 実測、2026-08-08〜09
に集中）の記載を参照するに留める。新判定方式導入後は 0 run（本レポート作成時点で
新判定方式コミット上での `bench-schedule.yml` 実行実績はゼロ）。

## 3. `microbench` ジョブ（決定的指標）の run 間一致確認

`microbench` ジョブ（ci.yml、alloc カウンタ方式、イシュー #619/#615 系）は
BASE_SHA の push（`gh run list --workflow=ci.yml` で確認した databaseId
31612527085、event=push、headSha=b0b8745、作成時点で in_progress）が最初の
導入 run であり、**本レポート作成時点で完了済みの `microbench` ジョブ実行実績が
存在しない**。設計上ノイズゼロが期待値（`benches/microbench/baseline.json` との
allocations / bytes 完全一致を `cargo test` が検証する決定的テスト、
`docs/design/deterministic-microbench.md` 参照）であり、複数 run にわたる分散
確認は本イシューでは実施できず、以降の PR/push で自然蓄積する run を次フェーズが
確認することを申し送る。

## 4. 較正ラン未実施の理由（構造的制約）

本イシューは push・PR 作成を行わない実装専任エージェントが担当した。計画の
Step 0〜2・Step 9 は次を要求する。

- 較正用固定ブランチ（`issue-616-calibration-base`）の作成と `git push`
- `gh workflow run bench-schedule.yml --ref <branch> -f mode=primary/pair`
  による `workflow_dispatch` の投入（対象 ref がリモートに存在することが前提）
- 確定しきい値での 1 サイクル green 確認のための作業ブランチ push 後の dispatch

いずれもリモートへの push を前提とするため、本フェーズでは実行できない。
push・PR 作成が行われる後続フェーズ（レビュー通過後）で、本レポートの 1〜4 節を
引き継ぎ、計画の Step 0〜2・Step 9 を実施することを推奨する（8 節参照）。

## 5. しきい値・試行回数の暫定値維持（fail-closed: 較正未完了）

新方式・同一コミットでの実測系列が存在しないため、以下すべて**現状の暫定値を
そのまま維持し、較正未完了のまま次フェーズへ引き継ぐ**（確定値ではない）。
値を動かさないことが最も安全側であるという
計画 Step 4 の判断基準、および `.claude/rules/security.md` のフェイルクローズ
原則に従う。

| パラメータ | 現状値 | 定義箇所 | 判断 |
|---|---|---|---|
| `P95_MARGIN`（M） | 0.10 | `benches/bench-accept.sh` | 現状値を維持。実測系列収集後に再判断 |
| `MAJORITY_TRIALS` | 3 | `.github/workflows/bench-schedule.yml` | 現状値を維持 |
| `PAIRS` | 8 | `benches/lib/interleave.sh` | 現状値を維持 |
| `PAIR_M2` | 0.05 | `benches/lib/interleave.sh` | 現状値を維持 |
| `PAIR_MIN_PAIRS` | 6 | `benches/lib/interleave.sh` | 現状値を維持 |
| `EXT_CPU_MAX_PCT` | 5 | `benches/lib/cpu-probe.sh` | 現状値を維持 |
| `WINDOW_REMEASURE_MAX` | 2 | `benches/lib/cpu-probe.sh` | 現状値を維持 |
| 境界接近しきい値（方式 2 頻度再検討トリガ (ii)） | 余裕 2 ポイント未満 | `docs/design/bench-hosted-runner.md` 5 節 2 項 | 現状値を維持 |
| 分布逸脱閾値 | 25% | `docs/design/bench-p95-criteria.md` 6 節 | 現状値を維持 |

いずれも緩和（しきい値を広げる・試行回数を減らす）方向の変更は行っていない。

## 6. 再較正条件（次フェーズへの引き継ぎ条件）

以下がすべて揃った時点で、本レポートを更新し、5 節の値を実測に基づき再確定する。

1. BASE_SHA（または較正実施時点の `origin/main` HEAD）を固定した専用ブランチで、
   新判定方式（`P95_BAND=1` + `MAJORITY_TRIALS=3`）下の `bench-schedule.yml`
   `mode=primary` を 5 回以上逐次実行し、RPS 比・p95 比・p99 比・判定結果・
   試行消費数を全数記録する（2 節の空欄を埋める）
2. 同ブランチで `mode=pair` を 2〜3 回逐次実行し、cur/pre 比分布・採用/除外ペア数・
   外部 CPU 占有率分布を記録する
3. `microbench` ジョブの run 間一致（allocations / bytes 完全一致）を 5 run 以上で
   確認する
4. 1〜3 の結果をもとに 5 節の各パラメータを再評価し、緩和する場合は実測分布の
   明示的根拠を付す（`.claude/rules/security.md`・#612 11 節）
5. 確定後、新しきい値での `bench-schedule.yml` 1 サイクルが誤検知なく green で
   完走することを確認する（受け入れ基準 3）

## 7. ドキュメント追随

本レポート作成に合わせ、以下のファイルの「#616 で確定」という既完了を示唆する
記述を、「#616 で fail-closed 方針により暫定値のまま維持（新方式同一コミット
系列の実測較正は未収集・較正未完了、再較正条件は本レポート参照）」という趣旨の
記述へ統一した。恒久策（トリガ (iii) 監視ロジック実装等）は #614 由来の保留のまま変更
していない。

- `benches/bench-accept.sh`
- `benches/lib/interleave.sh`
- `benches/lib/cpu-probe.sh`
- `.github/workflows/bench-schedule.yml`
- `benches/README.md`
- `docs/design/bench-hosted-runner.md`
- `docs/design/bench-p95-criteria.md`
- `docs/design/bench-scheduled-run.md`
- `docs/design/README.md`
- `CLAUDE.md`

## 8. 申し送り事項

- **受け入れ基準 1・3 の較正ラン本体**: 6 節の再較正条件に従い、push 可能な
  フェーズ（レビュー通過後の後続エージェント、または人間の `workflow_dispatch`
  投入）で実施する。無料枠ホステッドランナー（public リポジトリ）のため追加コスト
  上の制約はない
- **トリガ (iii)（baseline・core 同方向悪化）監視ロジックの実装**: #614 の保留
  残課題。6 節の実測系列が蓄積された後に実装再開が可能になる。新規 Issue の起票は
  `.claude/rules/out-of-scope-tracking.md` に従いユーザー承認を得てから行う（本
  フェーズでは提案に留め、自動起票しない）
- **較正ラン中に自動起票されうる `bench-regression` Issue の一次対応**:
  次フェーズで較正ランを実施する際、FAIL/INCONCLUSIVE/BLOCKED が発生し
  `bench-regression` ラベルの Issue が自動起票された場合は、ノイズ起因と確定
  できる場合に限り本レポート（更新後）を根拠にコメントしてクローズする
  （idempotent-issue のため重複起票はしない）
