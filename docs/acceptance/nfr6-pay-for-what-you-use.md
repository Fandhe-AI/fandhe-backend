# NFR-6 受け入れ検証レポート — 拡張の非侵襲性（pay-for-what-you-use、イシュー #282）

## 本レポートの位置づけ

`docs/spec/04-requirements.md` NFR-6（拡張の非侵襲性・pay-for-what-you-use）の実測・
検証はすでに完了しているが、記録が `docs/acceptance/req2-plugin-mechanism.md`・
`req3-openapi-generation.md`・`req4-websocket.md`・`req5-graphql.md`・
`req8-webrtc-attack-surface.md`・`req9-hub-wiring.md` と `scripts/
pay-for-what-you-use-check.sh`（CI 常設ゲート）・`benches/nfr6-exclusive.sh`（専有計測
wrapper）に**分散**しており、NFR-6 の受け入れ基準 2 項目と 1:1 で突合できる専用レポート
が存在しなかった（親 #278 の 2026-07-20 仕様突合で検出）。

先例として **NFR-7 専用レポート `docs/acceptance/nfr7-middleware-async-io.md`（#263）**
があり、「既存実測の転記・出典整理のみ、再実測なし」という位置づけ・構成をそのまま
踏襲する。本レポートも同様に**転記のみで新規実測は行っていない**。実測値・判定区分は
すべて下表の出典からの転記であり、一次記録・詳細解説はそちらを参照する。判定の正は
各出典レポートと `scripts/accept/lib/nfr6-ratio.sh` の `evaluate_nfr6_ratio` にあり、
本レポートは参照集約のみを担う（二重管理による陳腐化・改竄余地を作らない）。

隣接イシュー #283（`req2-plugin-mechanism.md` 基準 5 の記述食い違い整理）は別スコープ。
本レポートは req2 の現行記載をそのまま転記参照するに留め、記述整理は #283 に委ねる。

## 受け入れ基準と実測根拠の対応表

`docs/spec/04-requirements.md` NFR-6 の受け入れ基準（2 項目）に対する判定は次のとおり。

| 受け入れ基準（`04-requirements.md` NFR-6） | 判定 | 出典（名指し） |
|---|---|---|
| 基準 1: プラグイン無効時、当該プラグインの依存・`unsafe`・コードが 0 件でバイナリ・依存ツリーに載らない（PoC-3・PoC-4・PoC-5・PoC-6 で確認済み） | **PASS**（充足） | `scripts/pay-for-what-you-use-check.sh`（全 feature 動的列挙による CI 常設ゲート）+ `req2-plugin-mechanism.md` 基準 2（webrtc-proxy・graphql）+ `req3-openapi-generation.md` 基準 3（openapi）+ `req4-websocket.md`（websocket）+ `req8-webrtc-attack-surface.md`（webrtc）+ `req9-hub-wiring.md`（hub-wiring） |
| 基準 2: パス一致時のみ介入する拡張点（`UpgradeHandler`・パスインターセプト型）は、無関係なパスへの RPS・レイテンシ影響が誤差範囲内（100.3〜100.8%相当）である | **WARN（受容）**（狭義帯未達だが実務許容帯 [95, 105]% 内のため受け入れとして受容） | `req4-websocket.md` 基準 D（WARN〜PASS）・`req5-graphql.md` 基準 C 追補（#216、WARN 確定）・`req8-webrtc-attack-surface.md` 基準 E（2026-07-20 再計測、WARN 確定）・`req9-hub-wiring.md` 基準 D（2026-07-20 再計測、WARN 確定） |

両基準とも「PASS または受け入れとして受容された WARN」であり、`04-requirements.md` の
関連 PoC 欄記載（PoC-3 OK・PoC-4 OK・PoC-5 条件付き OK・PoC-6 OK）と整合する。

## 基準 1 の詳細: feature 別の除外証跡

| feature（プラグイン） | 除外証跡の出典 | 検証内容 |
|---|---|---|
| `webrtc-proxy`・`graphql` | `req2-plugin-mechanism.md` 基準 2b（「再検証（#261）」節） | 両 feature とも `cargo tree` で無効時不出現・有効時出現を確認。配線切れなし |
| `openapi` | `req3-openapi-generation.md` 基準 3 | `cargo tree -p fandhe-backend-core -e normal --no-default-features` で `fandhe-backend-plugin-openapi` 0 件（default 構成でも 0 件）。`--features openapi` でのみ出現 |
| `websocket` | `req4-websocket.md` | 無効時の依存・コード除外検証済み（`pay-for-what-you-use-check.sh` の動的列挙対象） |
| `webrtc`（in-process） | `req8-webrtc-attack-surface.md` NFR-5/NFR-6 節 | 無効時除外検証済み。有効時の依存インパクト補足も同レポートに記録（`cargo tree` で `webrtc` 関連 23 件、`docs/dep-impact/records.md` 参照） |
| `hub-wiring` | `req9-hub-wiring.md` | 無効時除外検証済み |
| 全 feature 横断（機械ゲート） | `scripts/pay-for-what-you-use-check.sh`（`.github/workflows/ci.yml` `pay-for-what-you-use` ジョブ） | cargo tree（無効時不出現・有効時出現）/ cargo geiger / バイナリサイズ・nm シンボル / 全構成ビルドの 5 段検証。feature を動的列挙するため新規 feature 追加時も自動的に検証対象へ含まれる |

基準 1 は個別レポートでの実証に加え、CI 常設ゲート（`pay-for-what-you-use-check.sh`）が
すべての feature 構成で継続的に検証し続けている。

## 基準 2 の詳細と WARN 受容の扱い

### 判定帯の定義（`scripts/accept/lib/nfr6-ratio.sh` `evaluate_nfr6_ratio`）

- **実務許容帯**: RPS 比 [95, 105]%（両側判定）・p95 比 [–, 105]%（片側判定、レイテンシは
  低い方向への乖離を問題にしない）
- **狭義 NFR-6 帯**（`04-requirements.md` 文言どおり）: RPS 比 [100.3, 100.8]%・
  p95 比 [–, 100.8]%
- 実務許容帯外は **FAIL**（フェイルクローズ）、実務許容帯内・狭義帯外は **WARN**
  （受け入れとしては通すが乖離を必ず記録し PASS に丸めない）。RPS・p95 双方の判定の
  うち悪い方（FAIL > WARN > PASS）を総合判定として採用する

### feature 別の実測値・判定

| feature（拡張点型） | 判定区分 | RPS 比 | p95 比 | 実測日・計測環境 |
|---|---|---|---|---|
| `websocket`（`UpgradeHandler` 型） | WARN〜PASS | 実務許容帯内で安定（狭義帯は 3 回中 1 回のみ達成） | 同上 | `req4-websocket.md` 基準 D、詳細は `benches/reports/task-4.4-ws-latency.md` |
| `graphql`（パスインターセプト型） | **WARN**（受容確定、#216） | 98.38% | 101.34% | 2026-07-19 実施、専有計測枠（`benches/nfr6-exclusive.sh`、#178）。`req5-graphql.md` 基準 C 追補（#216）。2026-07-17 の初回振れ幅大実測（最終値 RPS 比 93.72% / p95 比 111.31%、FAIL）は改変せず過去記録として保持 |
| `webrtc`（in-process） | **WARN**（受容確定） | 95.54% | 104.94% | 2026-07-20 実施、専有 Linux ホスト（`RUNS=5 DURATION=15s CONNECTIONS=128`）。`req8-webrtc-attack-surface.md` 基準 E「再計測（2026-07-20・専有 Linux ホスト、基準 E 確定）」節。旧 2026-07-17 実測（RPS 比 94〜95% / p95 比 106〜108%、実務許容帯を僅かに下回り FAIL）は改変せず過去記録として保持 |
| `hub-wiring`（パスインターセプト型） | **WARN**（受容確定） | 99.49% | 99.32% | 2026-07-20 実施、専有 Linux ホスト（`RUNS=5 DURATION=5s CONNECTIONS=32`）。`req9-hub-wiring.md` 基準 D「再計測（2026-07-20・専有 Linux ホスト、基準 D 確定）」節 |
| `openapi`（`Server::openapi()` opt-in 登録型） | （基準 2 単独計測なし） | — | — | 下記「乖離・限界の正直な記録」参照 |

### WARN 受容の運用方針

- `graphql`・`webrtc`・`hub-wiring` はいずれも**狭義帯（100.3〜100.8%）を達成していない**
  が、**実務許容帯（[95, 105]%）内**であるため受け入れとして受容している
- WARN を PASS に丸めない運用（fail-closed）を維持しており、各出典レポートは狭義帯未達を
  隠さず明記したうえで「実務許容帯内のため受け入れとして受容」という判断を記録している
- 過去に記録された FAIL（`graphql` の 2026-07-17 初回実測、`webrtc` の 2026-07-17 実測）は
  改変せず過去記録として保持し、その後の専有計測環境での確定再計測により WARN へ更新
  された経緯を各出典レポートの「再計測」節がそのまま記録している

## 実測日と根拠コミット

各出典レポートの実測日と、その内容が確定したコミットの一覧。

| 出典ファイル | 実測日（レポート記載） | 最終更新コミット |
|---|---|---|
| `docs/acceptance/req2-plugin-mechanism.md` | 2026-07-17（初出）・2026-07-19（#260 再計測）・#261 再検証 | `3b6990b`（#268） |
| `docs/acceptance/req3-openapi-generation.md` | #259・#273 再判定 | `a44c620`（#273） |
| `docs/acceptance/req4-websocket.md` | TASK-4.4（#25） | `2306676`（#209、全 crate 改名時の更新） |
| `docs/acceptance/req5-graphql.md` | 2026-07-17（初回）・2026-07-19（#216 専有計測確定） | `88a0b10`（#223） |
| `docs/acceptance/req8-webrtc-attack-surface.md` | 2026-07-17（初回）・2026-07-20（専有 Linux ホスト確定） | `67f2b88`（#274） |
| `docs/acceptance/req9-hub-wiring.md` | 2026-07-20（専有 Linux ホスト確定） | `67f2b88`（#274） |
| `scripts/pay-for-what-you-use-check.sh` | CI 常設ゲート（継続実行） | `67f2b88`（#274） |
| `scripts/accept/lib/nfr6-ratio.sh`（判定帯定義） | TASK-8.4（#29） | `e3da296`（#146） |
| `benches/reports/task-2.4-plugin-accept.md` | 2026-07-19（#260） | `3b6990b`（#268） |
| `benches/reports/task-8.4-webrtc-nfr6.md` | 2026-07-20 | `b657fbd`（#224） |
| `benches/reports/task-9.5-hub-wiring-performance.md` | 2026-07-20 | `b8ee2e2`（#210、環境変数改名時の更新） |

## 乖離・限界の正直な記録

隠さず記録するフェイルクローズ原則（`.claude/rules/security.md`）に従い、実測の前提
条件の差異・限界を以下に記す。

- **狭義帯（100.3〜100.8%）は現時点でどの feature も達成していない**: `graphql`・
  `webrtc`・`hub-wiring` はいずれも実務許容帯内・狭義帯外の WARN であり、`websocket`
  のみ 3 回中 1 回狭義帯を達成した記録がある（`req4-websocket.md`）。狭義帯の恒常的な
  達成は各出典レポートがそれぞれ「フォローアップ」として残している未解決課題であり、
  本レポートはこれを新たに解決するものではない
- **`openapi` は基準 2 単独の計測記録がない**: `openapi` は `Server::openapi()` の明示
  登録（opt-in）時のみ `GET /openapi.json` を返す設計であり、`req3-openapi-generation.md`
  基準 4 が `GET /health`（無関係パス）への性能影響を計測しているが（RPS +0.58%・
  p95 +1.59%、±5% 以内）、`evaluate_nfr6_ratio` の狭義帯定義に基づく NFR-6 基準 2 単独の
  計測としては記録されていない。基準 4 の実測自体は基準 2 の趣旨（無関係パスへの影響
  誤差範囲内）と整合するが、本レポートはこの差異を改変せず乖離として記録する
- **計測環境の差異**: `graphql`・`webrtc`・`hub-wiring` の確定計測は専有 Linux ホスト
  （`docs/design/nfr6-exclusive-measurement.md`、#178）で実施されたが、`websocket` の
  基準 D は専有計測枠を用いない実測（`benches/reports/task-4.4-ws-latency.md`）である。
  計測条件が feature ごとに異なるため、feature 間の数値を単純比較しない
- **`webrtc` の基準 E・`hub-wiring` の基準 D は初出時 FAIL からの再計測で WARN へ更新**
  された経緯があり、初出時の FAIL 記録自体は各出典レポートが過去記録として保持して
  いる（本レポートもこれを改変せず転記した）
- **転記中に既存記録間の矛盾を発見した場合の扱い**: 転記にあたり各出典の数値・判定
  区分を突合したが、矛盾は検出しなかった。`req2-plugin-mechanism.md` 基準 5（両 feature
  無効時のコア性能維持）の記述に食い違いがあるとの指摘（#283）は別イシューのスコープ
  であり、本レポートは基準 1・基準 2 の対応表にのみ責務を限定しているため影響しない

## 参照

- `docs/spec/04-requirements.md`（NFR-6）
- `docs/acceptance/req2-plugin-mechanism.md`（基準 1・基準 2 系の実証。webrtc-proxy・graphql）
- `docs/acceptance/req3-openapi-generation.md`（基準 3・基準 4。openapi）
- `docs/acceptance/req4-websocket.md`（基準 D。websocket）
- `docs/acceptance/req5-graphql.md`（基準 C・追補 #216。graphql）
- `docs/acceptance/req8-webrtc-attack-surface.md`（基準 E。webrtc in-process）
- `docs/acceptance/req9-hub-wiring.md`（基準 D。hub-wiring）
- `docs/acceptance/nfr7-middleware-async-io.md`（同種の配置不整合是正の先例、#263）
- `scripts/pay-for-what-you-use-check.sh`（基準 1 の CI 常設ゲート）
- `scripts/accept/lib/nfr6-ratio.sh`（`evaluate_nfr6_ratio`、判定帯定義）
- `docs/design/nfr6-exclusive-measurement.md`（専有計測の方法論、#178）
- `benches/nfr6-exclusive.sh`（専有計測 wrapper）
- `benches/reports/task-2.4-plugin-accept.md`・`task-8.4-webrtc-nfr6.md`・
  `task-9.5-hub-wiring-performance.md`（feature 別性能詳細レポート）
- 関連 Issue: #278（親、仕様突合）・#282（本レポート）。参考: #219（req10-tracing.md
  是正）・#236（req11 是正）・#263（NFR-7 是正、先例）・#283（req2 基準 5 記述整理、別スコープ）
