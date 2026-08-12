# イシュー #616 ホステッドランナー較正レポート

## 0. 結論（先出し）

**新判定方式（`P95_BAND=1` + `MAJORITY_TRIALS=3`）下で `bench-schedule.yml`
`mode=primary` を 5 回、`mode=pair` を 2 回、いずれも `workflow_dispatch` で
逐次実行し、全 7 ラン 総合 PASS を確認した（9〜10 節）。実測ノイズ帯域は
現行しきい値の判定余裕の内側に収まっており、5 節の各パラメータを
**実測に基づく確定値**として確定する（値の変更なし・緩和なし）。**

受け入れ基準の充足状況（初版の「未達」を実測で更新）:

| 受け入れ基準 | 状態 |
|---|---|
| 1. 新判定方式で 5 回以上の較正ラン | **達成**（9 節。`mode=primary` × 5、全ラン success・総合 PASS。5 ランは docs/CI のみ差分の 3 コミットに跨るが、計測対象 `crates/` のツリーは全ランで同一（9.1 節で検証）） |
| 2. 実測ノイズ帯域に基づくしきい値・試行回数の最終値確定 | **達成**（11 節。現行値をすべて実測根拠付きで確定。緩和方向の変更なし） |
| 3. 確定しきい値での 1 サイクル green 完走確認 | **達成**（9〜10 節。一次判定 5 サイクル + 二次判定 2 サイクルすべて誤検知なく green 完走。`bench-regression` 自動起票 0 件） |
| 4. ドキュメント追随 | 達成（7 節 + 12 節の確定反映） |

初版（PR #623、コミット c2c7747）は push 不可の実装フェーズ制約により基準 1〜3 を
未達のまま fail-closed 方針（暫定値維持）で記録した。本更新は同レポート 6 節の
再較正条件に従い、push 可能なフェーズで較正ランを実施して基準を実データで
再判定したものである。初版の 1〜8 節は経緯記録として残す（現状の判定は本節と
9 節以降を正とする）。

## 1. 較正対象コミット（BASE_SHA）〔初版記録〕

- BASE_SHA: `b0b8745aad12f956ea5454eb3893c367ff799587`（`origin/main` HEAD、
  初版作成時点。新判定方式を導入した #621
  （`a11178f28b341c941e3c3752f84539b36627bc37`、マージ日時
  2026-08-12T14:55:04Z）を含む）
- 初版の作業ブランチ `test/616-calibration-thresholds` は BASE_SHA から作成

## 2. `bench-schedule.yml` 実行履歴（旧判定方式 5 run）〔初版記録〕

`gh run list --workflow=bench-schedule.yml --limit 20` で取得した全数（結果に
条件付けた抽出は行っていない）。

| run ID | headSha（先頭 8 桁） | event | conclusion | createdAt (UTC) |
|---|---|---|---|---|
| 31290864613 | 73e3212a | schedule | success | 2026-08-09T02:41:41Z |
| 31237646850 | 64b2fe28 | workflow_dispatch | success | 2026-08-08T03:38:30Z |
| 31236262022 | a4a107f6 | workflow_dispatch | success | 2026-08-08T03:00:51Z |
| 30729910081 | 598a9be4 | schedule | failure | 2026-08-02T03:02:19Z |
| 30185507560 | 72c534d2 | schedule | failure | 2026-07-26T03:02:16Z |

いずれのコミットも `a11178f`（新判定方式導入）より前であり、判定方式の較正
データとしては使えない（新判定方式での実測系列は 9〜10 節）。

## 3. `microbench` ジョブ（決定的指標）の run 間一致確認

初版時点では完了済み run が存在しなかった。本更新時点では、`microbench` ジョブは
ci.yml 導入コミット `b0b8745` 以降の全 PR/push で実行され、**連続 10 run 以上が
すべて green**（例: databaseId 31622359156 / 31623275213 / 31624058241 /
31624178077 / 31624740267 / 31625178071 / 31626079321 / 31626938093 /
31627862876 / 31628656309）。同ジョブは `benches/microbench/baseline.json` との
allocations / bytes **完全一致**をしきい値ゼロで検証するため、green の連続は
run 間一致（分散ゼロ）の直接の証拠である（再較正条件 3 を充足）。

## 4. 初版時点で較正ラン未実施だった理由（構造的制約）〔初版記録〕

初版の実装は push・PR 作成を行わない実装専任エージェントが担当したため、
`workflow_dispatch` の投入ができなかった。本更新フェーズ（push 可能）で
6 節の再較正条件に従い較正ランを実施した（9〜10 節）。

## 5. しきい値・試行回数の確定（実測根拠付き、値の変更なし）

9〜10 節の実測系列に基づき、以下すべてを**確定値**とする。いずれも初版の
暫定値から変更しない（実測ノイズ帯域が現行値の判定余裕の内側に収まっており、
変更する根拠がない。緩和方向の変更なしの原則は `.claude/rules/security.md`・
#612 11 節に従う）。各値の実測根拠は 11 節。

| パラメータ | 確定値 | 定義箇所 | 実測根拠（11 節） |
|---|---|---|---|
| `P95_MARGIN`（M） | 0.10 | `benches/bench-accept.sh` | p95 比の観測範囲 0.661〜1.063。帯域 M=0.10 が観測ノイズ幅（単一指標の最大 range 0.28、GET 系 0.14〜0.18）を判定不能帯として吸収できる |
| `MAJORITY_TRIALS` | 3 | `.github/workflows/bench-schedule.yml` | 5 ラン全てが 1 試行目 PASS で確定（多数決の追加試行消費 0）。3 で十分 |
| `PAIRS` | 8 | `benches/lib/interleave.sh` | 2 ラン × 4 エンドポイントすべて採用 8/8。汚染による除外発生なし |
| `PAIR_M2` | 0.05 | `benches/lib/interleave.sh` | cur/pre p95 比の採用ペア中央値は 0.998〜1.007（全 16 系列の観測範囲 0.973〜1.023）。上限 1.05 への最悪余裕 2.7 ポイント |
| `PAIR_MIN_PAIRS` | 6 | `benches/lib/interleave.sh` | 全系列 8/8 採用のため下限 6 に接触せず |
| `EXT_CPU_MAX_PCT` | 5 | `benches/lib/cpu-probe.sh` | pair 2 ラン（`CPU_PROBE=1` 経路）で汚染除外 0 件。上限に接触せず |
| `WINDOW_REMEASURE_MAX` | 2 | `benches/lib/cpu-probe.sh` | 再計測発生 0 件。上限に接触せず |
| 境界接近しきい値（方式 2 頻度再検討トリガ (ii)） | 余裕 2 ポイント未満 | `docs/design/bench-hosted-runner.md` 5 節 2 項 | 一次判定 p95 の最悪余裕 3.7 ポイント・pair の最悪余裕 2.7 ポイントで、いずれも現時点で非発火。トリガとして維持 |
| 分布逸脱閾値 | 25% | `docs/design/bench-p95-criteria.md` 6 節 | 全 5 ランで分布逸脱の発生 0 件 |

## 6. 再較正条件（初版の引き継ぎ条件と充足結果）

初版が定めた 5 条件と充足状況:

1. `mode=primary` 5 回以上の逐次実行と全数記録 → **充足**（9 節）
2. `mode=pair` 2〜3 回の逐次実行と記録 → **充足**（10 節、2 回）
3. `microbench` run 間一致 5 run 以上 → **充足**（3 節、10 run 以上）
4. 実測に基づく 5 節の再評価（緩和には明示的根拠） → **充足**（11 節。緩和なし）
5. 確定しきい値での 1 サイクル green 完走 → **充足**（9〜10 節の全 7 ラン。
   `bench-regression` 自動起票 0 件）

今後の再々較正トリガ: ランナー世代交代・toolchain 更新等で 9〜10 節の観測帯域から
明確に逸脱する run が観測された場合、本レポートを再更新して 5 節を再確定する。

## 7. ドキュメント追随〔初版記録 + 本更新〕

初版で「#616 で fail-closed 方針により暫定値のまま維持（較正未完了）」へ統一した
以下のファイルの記述を、本更新で「#616 較正ランで実測確定（値の変更なし、根拠は
本レポート 9〜11 節）」へ更新した。

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

## 8. 申し送り事項〔更新〕

- **トリガ (iii)（baseline・core 同方向悪化）監視ロジックの実装**: #614 の保留
  残課題のまま（本更新の対象外）。9 節の実測系列が最初の標本となる。新規 Issue の
  起票は `.claude/rules/out-of-scope-tracking.md` に従いユーザー承認を得てから行う
- **較正ラン中の `bench-regression` 自動起票**: 全 7 ラン green のため発生ゼロ
  （`gh issue list --label bench-regression --state open` で 0 件を確認済み）

## 9. 較正ラン実施結果（`mode=primary` × 5、本更新の中核データ）

2026-08-12 に `gh workflow run bench-schedule.yml -f mode=primary` を逐次 5 回
投入した（前 run の completed を確認してから次を dispatch。並行実行なし。
`concurrency: bench-schedule` により多重実行は構造的にも排除される）。

| # | run ID | headSha | conclusion | 総合判定 | createdAt (UTC) |
|---|---|---|---|---|---|
| 1 | 31617427362 | b0b8745a | success | PASS | 2026-08-12T16:24:47Z |
| 2 | 31620871009 | b0b8745a | success | PASS | 2026-08-12T17:05:27Z |
| 3 | 31622759268 | c2c77473 | success | PASS | 2026-08-12T17:28:01Z |
| 4 | 31624648014 | c2c77473 | success | PASS | 2026-08-12T17:50:36Z |
| 5 | 31626540246 | 1a005630 | success | PASS | 2026-08-12T18:13:10Z |

### 9.1 「同一コミット」条件の充足範囲

5 ランは 3 コミット（b0b8745a × 2、c2c77473 × 2、1a005630 × 1）に跨る
（dispatch は main HEAD を対象とし、較正中に docs/CI のみの PR がマージされた
ため）。`git diff --stat b0b8745a 1a00563 -- crates/` は**空**であり、計測対象
バイナリ（baseline / core）を構成するソースは全ランで同一である。差分は
`benches/lib/cpu-probe.sh`・`benches/lib/interleave.sh`（コメント更新のみ、
かつ一次判定の実行経路では `CPU_PROBE`/`INTERLEAVE` とも既定 OFF で未使用）と
docs / `.github` に限られる。したがって「同一コミット × 5」の字義には
合致しないが、「同一計測対象 × 5」として基準 1 の趣旨を充足すると判断する。

### 9.2 指標別の実測分布（core/baseline 比、5 ラン）

| 指標 | min | 中央値 | max | range |
|---|---|---|---|---|
| RPS GET /health | 1.0034 | 1.0306 | 1.0410 | 0.038 |
| p95 GET /health | 0.9229 | 0.9890 | 1.0626 | 0.140 |
| p99 GET /health | 0.9123 | 0.9729 | 1.0827 | 0.170 |
| RPS GET /hello/{name} | 0.9911 | 1.0254 | 1.0507 | 0.060 |
| p95 GET /hello/{name} | 0.8668 | 0.9947 | 1.0485 | 0.182 |
| p99 GET /hello/{name} | 0.8491 | 1.0019 | 1.0563 | 0.207 |
| RPS GET /users/{id} | 0.9961 | 1.0261 | 1.0450 | 0.049 |
| p95 GET /users/{id} | 0.8755 | 0.9853 | 1.0244 | 0.149 |
| p99 GET /users/{id} | 0.8542 | 0.9619 | 1.0417 | 0.188 |
| RPS POST /echo | 0.9997 | 1.0356 | 1.0568 | 0.057 |
| p95 POST /echo | 0.6611 | 0.8102 | 0.9434 | 0.282 |
| p99 POST /echo | 0.5228 | 0.5872 | 0.7928 | 0.270 |

- RPS 比の全体最小値 0.9911（判定下限 0.90 への最悪余裕 9.1 ポイント）
- p95 比の全体最大値 1.0626（判定しきい値 1.10 への最悪余裕 3.7 ポイント、
  判定不能上限 1.21 には大きく届かない）
- 全 5 ランが 1 試行目で PASS 確定（`MAJORITY_TRIALS=3` の追加試行消費 0、
  INCONCLUSIVE / BLOCKED の発生 0）
- POST /echo の p95/p99 は core が baseline を大きく上回る（比 0.52〜0.94）。
  これは #579 系の最適化効果であり、ノイズではなく系統的差

## 10. 較正ラン実施結果（`mode=pair` × 2）

同日に `gh workflow run bench-schedule.yml -f mode=pair` を逐次 2 回投入した。
いずれも headSha `797245a5`（9.1 節と同様、`crates/` は primary 系列と同一ツリー）。

| # | run ID | conclusion | 総合判定 | createdAt (UTC) |
|---|---|---|---|---|
| p1 | 31628904774 | success | PASS | 2026-08-12T18:41:25Z |
| p2 | 31636298024 | success | PASS | 2026-08-12T20:10:21Z |

エンドポイント別の cur/pre p95 比（採用ペアはすべて 8/8、汚染除外 0 件）:

| エンドポイント | ラン | min | 中央値 | max |
|---|---|---|---|---|
| GET /health | p1 | 0.9865 | 0.9983 | 1.0214 |
| GET /health | p2 | 0.9857 | 1.0010 | 1.0157 |
| GET /hello/{name} | p1 | 0.9731 | 1.0005 | 1.0149 |
| GET /hello/{name} | p2 | 0.9731 | 1.0082 | 1.0155 |
| GET /users/{id} | p1 | 0.9907 | 1.0074 | 1.0144 |
| GET /users/{id} | p2 | 0.9910 | 0.9954 | 1.0127 |
| POST /echo | p1 | 0.9733 | 1.0013 | 1.0079 |
| POST /echo | p2 | 0.9917 | 0.9970 | 1.0226 |

観測された cur/pre 比の全体範囲は 0.973〜1.023、採用ペア中央値は 0.995〜1.008。
`PAIR_M2=0.05`（上限 1.05）への最悪余裕は 2.7 ポイント。同一バイナリ同士の
交互測定でこの範囲に収束することは、交互ペア測定が VM ノイズを実効的に
相殺できていることの実測確認である。

## 11. しきい値確定の判断根拠（総括）

| パラメータ | 実測 | 判断 |
|---|---|---|
| `P95_MARGIN=0.10` | 単一指標の p95 range は最大 0.28（POST /echo、系統差込み）、GET 系で 0.14〜0.18。観測最大比 1.0626 は 1.10 未満で全ラン PASS | 帯域はノイズを FAIL 誤判定から守る設計余裕として機能。変更根拠なし → 確定 |
| `MAJORITY_TRIALS=3` | 5/5 ランが 1 試行目確定 | 過剰でも不足でもない → 確定 |
| `PAIRS=8` / `PAIR_MIN_PAIRS=6` / `PAIR_M2=0.05` | 16/16 系列で 8/8 採用、比 0.973〜1.023 | 余裕を持って機能 → 確定 |
| `EXT_CPU_MAX_PCT=5` / `WINDOW_REMEASURE_MAX=2` | 汚染検知・再計測とも発生 0 | 発火実績なしだが防御層として維持 → 確定 |
| 分布逸脱閾値 25% | 発生 0 | 確定 |

いずれも**値の変更なし**での確定であり、緩和は行っていない。

## 12. 本更新の検証コマンド（再現手順）

```bash
# 較正ランの全数列挙（9〜10 節の run ID と一致すること）
gh run list --workflow=bench-schedule.yml --limit 10 \
  --json databaseId,headSha,event,conclusion,createdAt

# 計測対象ソースの同一性（空出力であること）
git diff --stat b0b8745a 1a00563 -- crates/

# microbench の run 間一致（ci.yml 成功 run の列挙、3 節）
gh run list --workflow=ci.yml --limit 15 --json databaseId,conclusion \
  --jq '[.[] | select(.conclusion == "success")] | length'

# bench-regression 自動起票がないこと（0 であること）
gh issue list --label bench-regression --state open --json number --jq length
```
