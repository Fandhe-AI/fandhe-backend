# イシュー #616 ホステッドランナー較正レポート

## 0. 結論（先出し）

**固定 ref `calib/616-fixed-base`（コミット `797245a5`）上で、新判定方式
（`P95_BAND=1` + `MAJORITY_TRIALS=3`）下の `bench-schedule.yml` `mode=primary` を
5 回、`mode=pair` を 2 回、いずれも `workflow_dispatch` で逐次実行し、
全 7 ラン（すべて同一コミット）で success・総合 PASS を確認した（9〜10 節）。**

しきい値・試行回数は**すべて現行値のまま正式値として確定する**（値の変更なし・
緩和なし）。ただし「確定」の内訳は 11 節で明示するとおり 2 区分に分かれる。

- **実測確認済み**: 較正ランで当該経路が実際に働き、観測分布が判定余裕の内側に
  収まることを確認できたもの（p95 の帯域内判定・pair の cur/pre 比判定・
  `PAIRS=8` の採用実績）
- **fail-closed 維持**: 較正ラン中に一度も発火せず（発火実績ゼロ）、実測による
  妥当性検証はできていないが、fail-closed 原則により現行値を正式値として採用する
  もの（`MAJORITY_TRIALS` の多数決経路・`PAIR_MIN_PAIRS` 下限・
  `EXT_CPU_MAX_PCT`・`WINDOW_REMEASURE_MAX`・分布逸脱閾値・境界接近トリガ）。
  これらは「実測で確定した」とは記載せず、発火事例の蓄積時に再評価する

受け入れ基準の充足状況（初版の「未達」を実測で更新）:

| 受け入れ基準 | 状態 |
|---|---|
| 1. 新判定方式・同一コミットで 5 回以上の較正ラン | **達成**（9 節。固定 ref `calib/616-fixed-base` = `797245a5` 上の `mode=primary` × 5、全ラン success・総合 PASS。字義どおり同一コミット） |
| 2. 実測ノイズ帯域に基づくしきい値・試行回数の最終値確定 | **達成**（11 節。全パラメータについて現行値を正式値として決定。実測確認済みの範囲と、未発火のため fail-closed 維持とする範囲を区分して明示） |
| 3. 確定しきい値での 1 サイクル green 完走確認 | **達成**（9〜10 節。同一コミットで一次判定 5 サイクル + 二次判定 2 サイクルすべて誤検知なく green 完走。`bench-regression` 自動起票 0 件） |
| 4. ドキュメント追随 | 達成（7 節 + 12 節の確定反映） |

初版（PR #623、コミット c2c7747）は push 不可の実装フェーズ制約により基準 1〜3 を
未達のまま fail-closed 方針（暫定値維持）で記録した。本更新は同レポート 6 節の
再較正条件に従い、push 可能なフェーズで較正ランを実施して基準を実データで
再判定したものである。初版の 1〜8 節は経緯記録として残す（現状の判定は本節と
9 節以降を正とする）。

なお、固定 ref 系列に先立ち `origin/main` HEAD 追従で実施した `mode=primary` × 5
（3 コミットに跨る。9.3 節）は、「同一コミット × 5」の字義を満たさないため
受け入れ基準 1 の根拠には**使わない**。`crates/` ツリーが同一であることを検証済みの
補助データ（ノイズ帯域の追加標本）としてのみ 9.3 節に記録する。

## 1. 較正対象コミット（BASE_SHA）〔初版記録〕

- BASE_SHA: `b0b8745aad12f956ea5454eb3893c367ff799587`（`origin/main` HEAD、
  初版作成時点。新判定方式を導入した #621
  （`a11178f28b341c941e3c3752f84539b36627bc37`、マージ日時
  2026-08-12T14:55:04Z）を含む）
- 初版の作業ブランチ `test/616-calibration-thresholds` は BASE_SHA から作成
- 本更新の較正実施コミットは `797245a5`（9 節。BASE_SHA との差分は
  docs / `.github` / `.agents` のみで `crates/` は同一）

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
ci.yml 導入コミット `b0b8745` 以降の全 PR/push で実行されている。同ジョブは
`benches/microbench/baseline.json` との allocations / bytes **完全一致**を
しきい値ゼロで検証するため、ジョブ green = run 間一致（分散ゼロ）の直接の
証拠である。検証には対象期間の**全 run** を結論付きで列挙し、非 success が
ないことを確認する（12 節の再現手順。success だけを数える方法では間に
failure が挟まっていても検出できないため用いない）。本更新時点の確認では
`b0b8745` 以降の ci.yml run で `microbench` ジョブの失敗は 0 件・green は
10 run 以上（databaseId 31622359156 / 31623275213 / 31624058241 / 31624178077 /
31624740267 / 31625178071 / 31626079321 / 31626938093 / 31627862876 /
31628656309 ほか。再較正条件 3 を充足）。

## 4. 初版時点で較正ラン未実施だった理由（構造的制約）〔初版記録〕

初版の実装は push・PR 作成を行わない実装専任エージェントが担当したため、
`workflow_dispatch` の投入ができなかった。本更新フェーズ（push 可能）で
6 節の再較正条件に従い較正ランを実施した（9〜10 節）。

## 5. しきい値・試行回数の確定（値の変更なし。区分は 11 節）

9〜10 節の実測系列に基づき、以下すべてを**正式値として確定する**。いずれも
初版の暫定値から変更しない（緩和なし）。「実測確認済み」と「fail-closed 維持
（発火実績なし）」の区分と根拠は 11 節。

| パラメータ | 確定値 | 定義箇所 | 区分（11 節） |
|---|---|---|---|
| `P95_MARGIN`（M） | 0.10 | `benches/bench-accept.sh` | 帯域内判定は実測確認済み。判定不能帯（1.10〜1.21）への突入は未観測 |
| `MAJORITY_TRIALS` | 3 | `.github/workflows/bench-schedule.yml` | fail-closed 維持（多数決経路の発火実績なし） |
| `PAIRS` | 8 | `benches/lib/interleave.sh` | 実測確認済み（全系列 8/8 採用） |
| `PAIR_M2` | 0.05 | `benches/lib/interleave.sh` | 実測確認済み（cur/pre 比分布が上限の内側） |
| `PAIR_MIN_PAIRS` | 6 | `benches/lib/interleave.sh` | fail-closed 維持（下限接触なし） |
| `EXT_CPU_MAX_PCT` | 5 | `benches/lib/cpu-probe.sh` | fail-closed 維持（汚染検知の発火実績なし） |
| `WINDOW_REMEASURE_MAX` | 2 | `benches/lib/cpu-probe.sh` | fail-closed 維持（再計測の発生なし） |
| 境界接近しきい値（方式 2 頻度再検討トリガ (ii)） | 余裕 2 ポイント未満 | `docs/design/bench-hosted-runner.md` 5 節 2 項 | fail-closed 維持（現時点で非発火。トリガとして継続監視） |
| 分布逸脱閾値 | 25% | `docs/design/bench-p95-criteria.md` 6 節 | fail-closed 維持（発生 0 件） |

## 6. 再較正条件（初版の引き継ぎ条件と充足結果）

初版が定めた 5 条件と充足状況:

1. 固定 ref・同一コミットでの `mode=primary` 5 回以上の逐次実行と全数記録
   → **充足**（9 節。固定 ref `calib/616-fixed-base` = `797245a5` で 5 回）
2. `mode=pair` 2〜3 回の逐次実行と記録 → **充足**（10 節、2 回。同じく `797245a5`）
3. `microbench` run 間一致 5 run 以上 → **充足**（3 節、10 run 以上・失敗 0 件）
4. 実測に基づく 5 節の再評価（緩和には明示的根拠） → **充足**（11 節。緩和なし。
   未発火パラメータは実測確定と記載せず fail-closed 維持として区分）
5. 確定しきい値での 1 サイクル green 完走 → **充足**（9〜10 節の同一コミット
   7 ラン。`bench-regression` 自動起票 0 件）

今後の再々較正トリガ: ランナー世代交代・toolchain 更新等で 9〜10 節の観測帯域から
明確に逸脱する run が観測された場合、または 11 節の未発火経路（判定不能帯・
多数決・汚染検知）が実運用で発火した場合、本レポートを再更新して 5 節を再確定する。

## 7. ドキュメント追随〔初版記録 + 本更新〕

初版で「#616 で fail-closed 方針により暫定値のまま維持（較正未完了）」へ統一した
以下のファイルの記述を、本更新で「#616 較正ランで確定（値の変更なし。実測確認済み
の範囲と未発火 fail-closed 維持の範囲は本レポート 11 節参照）」へ更新した。

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
- **較正ラン中の `bench-regression` 自動起票**: 全ラン green のため発生ゼロ
  （`gh issue list --label bench-regression --state open` で 0 件を確認済み）
- **未発火防御経路の実測検証**: 11 節の fail-closed 維持パラメータは発火事例が
  蓄積された時点で妥当性を再評価する（人工的に発火させる較正（意図的な退行注入・
  CPU 負荷注入）は本較正のスコープ外とした）

## 9. 較正ラン実施結果（`mode=primary` × 5、固定 ref・同一コミット）

`origin/main` の `797245a5f92d37f3af4446dc86eb4d4a1aaa5917` を指す固定 ref
`calib/616-fixed-base` を作成し、2026-08-12〜13 に
`gh workflow run bench-schedule.yml --ref calib/616-fixed-base -f mode=primary` を
逐次 5 回投入した（前 run の completed を確認してから次を dispatch。並行実行なし。
`concurrency: bench-schedule` により多重実行は構造的にも排除される）。

| # | run ID | headSha | conclusion | 総合判定 | dispatch (UTC) |
|---|---|---|---|---|---|
| f1 | 31645060049 | 797245a5 | success | PASS | 2026-08-12T21:59 |
| f2 | 31646550174 | 797245a5 | success | PASS | 2026-08-12T22:20 |
| f3 | 31648113568 | 797245a5 | success | PASS | 2026-08-12T22:43 |
| f4 | 31649601102 | 797245a5 | success | PASS | 2026-08-12T23:04 |
| f5 | 31650886251 | 797245a5 | success | PASS | 2026-08-12T23:26 |

全 5 ランが 1 試行目で PASS 確定（`MAJORITY_TRIALS=3` の追加試行消費 0、
INCONCLUSIVE / BLOCKED の発生 0、`bench-regression` 自動起票 0 件）。

### 9.1 指標別の実測分布（core/baseline 比、固定 ref 5 ラン）

| 指標 | min | 中央値 | max |
|---|---|---|---|
| RPS GET /health | 1.0058 | 1.0225 | 1.0434 |
| p95 GET /health | 0.9145 | 0.9484 | 1.0269 |
| p99 GET /health | 0.8934 | 0.9851 | 1.0176 |
| RPS GET /hello/{name} | 1.0268 | 1.0310 | 1.0434 |
| p95 GET /hello/{name} | 0.9008 | 0.9432 | 0.9924 |
| p99 GET /hello/{name} | 0.8744 | 0.8922 | 0.9869 |
| RPS GET /users/{id} | 1.0228 | 1.0314 | 1.0896 |
| p95 GET /users/{id} | 0.8218 | 0.9081 | 0.9895 |
| p99 GET /users/{id} | 0.7697 | 0.8822 | 0.9607 |
| RPS POST /echo | 1.0091 | 1.0525 | 1.0537 |
| p95 POST /echo | 0.5969 | 0.7369 | 0.8970 |
| p99 POST /echo | 0.5606 | 0.5628 | 0.7403 |

- RPS 比の全体最小値 1.0058（判定下限 0.90 への最悪余裕 10.6 ポイント）
- p95 比の全体最大値 1.0269（判定しきい値 1.10 への最悪余裕 7.3 ポイント。
  判定不能帯 1.10〜1.21 への突入は 5 ランで 0 回）
- POST /echo の p95/p99 は core が baseline を大きく上回る（比 0.56〜0.90）。
  これは #579 系の最適化効果であり、ノイズではなく系統的差

### 9.2 判定不能帯・多数決経路が未発火であることの含意

固定 ref 5 ランでは p95 比が常に 1.10 未満に収まり、判定不能帯（1.10〜1.21）と
それに続く多数決経路（`MAJORITY_TRIALS`）は一度も働かなかった。これは
「現行構成のホステッドランナーでは通常時のノイズが判定余裕の内側に収まる」
ことの確認であり、**帯域幅 M=0.10・試行回数 3 の妥当性そのものの実測検証では
ない**（発火していない経路は検証できない）。この区別を 11 節・5 節の区分に
反映している。

### 9.3 補助データ: `origin/main` HEAD 追従の先行 5 ラン（3 コミットに跨る）

固定 ref 系列に先立ち、main HEAD 追従で実施した 5 ラン。**受け入れ基準 1 の
根拠には使わない**（同一コミットではないため）。`git diff --stat b0b8745a
1a00563 -- crates/` が空であることは検証済みで、ノイズ帯域の追加標本として記録
する。

| # | run ID | headSha | conclusion | 総合判定 | createdAt (UTC) |
|---|---|---|---|---|---|
| 1 | 31617427362 | b0b8745a | success | PASS | 2026-08-12T16:24:47Z |
| 2 | 31620871009 | b0b8745a | success | PASS | 2026-08-12T17:05:27Z |
| 3 | 31622759268 | c2c77473 | success | PASS | 2026-08-12T17:28:01Z |
| 4 | 31624648014 | c2c77473 | success | PASS | 2026-08-12T17:50:36Z |
| 5 | 31626540246 | 1a005630 | success | PASS | 2026-08-12T18:13:10Z |

この系列の指標分布（RPS 比 0.9911〜1.0568、p95 比 0.6611〜1.0626、いずれも
全ラン 1 試行目 PASS）は固定 ref 系列と整合し、観測ノイズ帯域が計測日・コミット
（docs/CI のみ差分）に対して安定であることを示す。

## 10. 較正ラン実施結果（`mode=pair` × 2、同一コミット）

`gh workflow run bench-schedule.yml -f mode=pair` を逐次 2 回投入した。いずれも
headSha は固定 ref 系列と同一の `797245a5`（dispatch 時点の main HEAD が
`797245a5` であったため、9 節と合わせて全 7 ランが同一コミット）。

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

## 11. しきい値確定の判断根拠（区分別の総括）

### 11.1 実測確認済み（較正ランで当該経路が実際に働いたもの）

| パラメータ | 実測 | 判断 |
|---|---|---|
| `P95_MARGIN=0.10` の帯域内判定 | p95 比の観測最大 1.0269（固定 ref）/ 1.0626（補助系列）。全ラン PASS 側で確定し、通常ノイズがしきい値 1.10 の内側に収まることを確認 | 現行値で確定。ただし判定不能帯（1.10〜1.21）自体の発火は未観測（11.2 参照） |
| `PAIRS=8` | 16/16 系列で 8/8 採用。8 ペアの測定時間内で cur/pre 比が中央値 ±3 ポイント以内に収束 | 現行値で確定 |
| `PAIR_M2=0.05` | 同一バイナリ比較の cur/pre 比が 0.973〜1.023（上限 1.05 へ余裕 2.7 ポイント） | 現行値で確定 |

### 11.2 fail-closed 維持（発火実績ゼロ。実測確定とは記載しない）

| パラメータ | 較正ラン中の状態 | 判断 |
|---|---|---|
| `MAJORITY_TRIALS=3` | 全ラン 1 試行目確定で多数決経路は未発火 | 妥当性は実測検証できていない。fail-closed 原則により現行値を正式値として維持し、判定不能帯の発火事例が蓄積された時点で再評価 |
| `PAIR_MIN_PAIRS=6` | 全系列 8/8 採用で下限に接触せず | 同上 |
| `EXT_CPU_MAX_PCT=5` | 汚染検知 0 件 | 同上 |
| `WINDOW_REMEASURE_MAX=2` | 再計測 0 件 | 同上 |
| 境界接近しきい値（余裕 2 ポイント未満） | 最悪余裕 2.7〜10.6 ポイントで非発火 | 同上（トリガとして継続監視） |
| 分布逸脱閾値 25% | 発生 0 件 | 同上 |

いずれの区分でも**値の変更なし**であり、緩和は行っていない。

## 12. 本更新の検証コマンド（再現手順）

```bash
# 固定 ref が 797245a5 を指すこと
gh api repos/Fandhe-AI/fandhe-backend/branches/calib/616-fixed-base \
  --jq '.commit.sha'

# 較正ランの全数列挙（9〜10 節の run ID・headSha・conclusion と一致すること。
# 結果で絞らず全件を列挙する）
gh run list --workflow=bench-schedule.yml --limit 15 \
  --json databaseId,headSha,event,conclusion,createdAt

# 計測対象ソースの同一性（空出力であること。補助系列 9.3 節の検証）
git diff --stat b0b8745a 1a00563 -- crates/

# microbench の run 間一致（3 節）: 対象期間の ci.yml 全 run を結論付きで列挙し、
# 非 success の run があれば microbench ジョブの結論を個別に確認する
# （success だけを数える方法は間に失敗が挟まっても検出できないため使わない）
gh run list --workflow=ci.yml --limit 20 \
  --json databaseId,headSha,conclusion,createdAt
# 非 success の run があった場合、そのジョブ別結論を確認する:
#   gh run view <databaseId> --json jobs \
#     --jq '.jobs[] | select(.name | contains("microbench")) | .conclusion'

# bench-regression 自動起票がないこと（0 であること）
gh issue list --label bench-regression --state open --json number --jq length
```
