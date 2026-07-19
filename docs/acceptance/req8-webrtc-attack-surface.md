# REQ-8 受け入れ検証レポート — WebRTC 攻撃表面評価（TASK-8.4、#29）

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` / `bf-plugin-*` 等）
> 表記のまま保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。

`docs/spec/04-requirements.md` REQ-8（WebRTC）・NFR-5・NFR-6 の受け入れ基準のうち
TASK-8.4 が担う「依存・バイナリ・unsafe を再評価し audit / deny を確認する」を
`scripts/accept/webrtc-accept.sh` で検証した結果。TASK-8.1（#26、in-process 実装）・
TASK-8.2（#27、別プロセスプロキシ）・TASK-8.3（#28、`webrtc-rs` バージョン戦略）は
前提タスクとしてマージ済み（本レポートはそれらの変更を前提とし、production コードの
追加変更は行っていない）。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-17 |
| 対象コミット（作業ブランチ起点、`origin/main`） | `8bb5494b9eddd80e76b15520b17b88e8ac8ba14a` |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| cargo-audit | 0.22.2 |
| cargo-deny | 0.19.8 |
| oha | 1.15.0 |
| cargo-geiger | 0.13.0（`--manifest-path` を絶対パスで指定すれば実行可能。TASK-8.1 時点の「実行失敗」記録は誤った呼び出し方によるもので、本タスクで訂正した。**2026-07 再実行（#220）で解消継続を確認、下記「2026-07 再実行」節参照**） |

## 判定サマリー（`scripts/accept/webrtc-accept.sh`）

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A: webrtc 無効時の依存完全除外 | `cargo tree -p backend-framework-core \| grep -c webrtc` = 0 |
| WARN | A補足: webrtc 有効時の依存インパクト | `cargo tree -p backend-framework-core --features webrtc \| grep -c webrtc` = 23（`docs/dep-impact/records.md` 参照） |
| PASS | B: plugin-webrtc 自コード unsafe 0件 | `crates/plugin-webrtc/src` に unsafe 0 件（テキストベース走査） |
| **受容 WARN**（#242 で確定、下記「REQ-8 webrtc unsafe 増分の削減策評価とリスク受容判断確定」節参照） | B補足: 依存側 unsafe 増分（webrtc-rs、cargo geiger 実測） | baseline（webrtc 無効）: Functions 69/170 / webrtc 有効: Functions 304/592（約 4.4 倍。PoC-5 実測「約 2.2 倍」より大きい。乖離要因は #183 で特定済み・ベースライン圧縮効果が主因で実害なし。削減策（バージョン更新・feature 絞り込み）は評価のうえ不適用、残余リスクは受容判断済み（PR レビュー承認で最終確定）。`docs/dep-impact/records.md` 参照） |
| PASS | C: cargo audit / deny 0件 | `scripts/dep-audit.sh`（全 feature 構成）が正常終了（advisories ok, bans ok, licenses ok, sources ok） |
| PASS | D: webrtc/webrtc-proxy 2 feature の存在 | `backend-framework-core` に `webrtc`（in-process）・`webrtc-proxy`（別プロセス切り出し）の両 feature が存在 |
| PASS | D補足: in-process/proxy のクレート境界分離 | `crates/plugin-webrtc` と `crates/plugin-webrtc-proxy` は相互依存なし |
| FAIL（旧判定を維持、参考実測は下記参照） | E: NFR-6 無関係パス影響 | 当時: RPS 比 94〜95% / p95 比 106〜108%（狭義の NFR-6 帯 100.3〜100.8% には収まらない）。**2026-07 再実行（#220）で参考実測（別パラメータ）を追加したが、同一パラメータでの確定比較が済んでいないため FAIL 判定は維持する。下記「2026-07 再実行」節参照** |

**終了コード（当時）: 1（FAIL あり、基準 E）**。**2026-07 再実行でも基準 E の FAIL 判定は維持しており、本レポート上の公式判定は FAIL のまま据え置く。ただし `webrtc-accept.sh` は実行のたびに実測される RPS・p95 比に応じて動的に FAIL/WARN/PASS を判定する（判定ロジックは `scripts/accept/lib/nfr6-ratio.sh` の `evaluate_nfr6_ratio` に従う）ため、専有計測 BLOCKED 下の参考実測（既定 `CONNECTIONS=32 DURATION=5s`）で偶然 WARN 相当の値が出た実行では終了コード 0 となることがある。旧確定計測（`CONNECTIONS=128 DURATION=15s`）とは単純比較できないため、この 1 回の実行結果を唯一の確定判断とはしない（詳細は下記「2026-07 再実行」節）。

## 基準 E（NFR-6）の詳細と判断

`benches/webrtc-nfr6-bench.sh`（`oha` による empirical 計測。計測用バイナリ:
`crates/core/examples/minimal.rs` = ベースライン、`crates/core/examples/webrtc_nfr6.rs` =
`webrtc` feature 有効・`Server::webrtc` 登録済み）を複数回実行した結果:

| 実行 | RPS 比（webrtc / baseline） | p95 比（webrtc / baseline） |
|------|------|------|
| 1 回目（RUNS=5, DURATION=5s, CONNECTIONS=128） | 95.23% | 106.57% |
| 2 回目（RUNS=5, DURATION=5s, CONNECTIONS=128） | 93.86% | 108.45% |

詳細な生ログは `benches/reports/task-8.4-webrtc-nfr6.md` を参照。

**判断**: NFR-6 の文言上の許容帯（100.3〜100.8%相当）には収まらなかった。この帯は
GraphQL（PoC-3、依存インパクトが軽微なパスインターセプト型）由来の実測に基づくもので
あり、`plugin::try_intercept` 自体は対象外パスに対して 1 回のパス比較のみでフォール
スルーする（`crates/core/src/plugin.rs`）ため拡張点の呼び出しコスト自体は無視できる
はずだが、in-process WebRTC はバイナリサイズが約 11 倍に達する（本レポート内、
`docs/dep-impact/records.md` 参照）ため、icache/TLB 圧迫等バイナリサイズに起因する
実測ノイズ・性能影響が生じていると考えられる。`webrtc-accept.sh`（`evaluate_nfr6_ratio`、
`scripts/accept/lib/nfr6-ratio.sh`）はこの実測値を NFR-7（ミドルウェア型、RPS 劣化 5%
以内）の先例を踏まえた実務的許容帯 [95%, 105%] と RPS 比・p95 比の両方について照合し、
悪い方の判定を総合判定として採用する（NFR-6 が RPS・レイテンシ双方を要求範囲に含む
ため）。2 回目の計測（RPS 比 93.86%）はこの実務帯を RPS 単独でも下回り、さらに両回とも
p95 比（106.57%・108.45%）が実務帯 105% を上回るため、いずれの観点からも FAIL とした
（判定を PASS に丸めない。捏造しない・フェイルクローズ、`.claude/rules/security.md`）。

**この FAIL を production コードの不具合として扱わない理由**: TASK-8.4 は test スコープ
であり、production 側の対応（性能最適化）は本タスクの範囲外。REQ-8 は最初から
「WebRTC を要するサービスは別プロセス・別サービスへ切り出す」ことを MVP 推奨設計と
しており（`crates/plugin-webrtc-proxy`）、in-process 版の性能影響が大きいこと自体は
既知の設計上のトレードオフ（PoC-5「条件付き OK」）と整合する。本 FAIL は「in-process
WebRTC を有効化したサービスは無関係パスの性能へも無視できない影響を受ける」という
実測事実を隠さず記録するためのものであり、対応方針（別プロセス切り出しの推奨）は
`AGENTS.md`「WebRTC の攻撃表面と『使う/使わない』サービスの安全性方針」に明記した。

## BLOCKED / フォローアップ

- **PoC-5「約 2.2 倍」と本タスク実測「約 4.4 倍」の乖離原因特定**: **#183 で解決済み**
  （`docs/dep-impact/records.md` の「`webrtc` feature の unsafe 増分乖離（PoC-5 比 2.2
  倍→実測 4.4 倍）の原因特定（#183）」エントリ参照）。`webrtc-rs` は両時点とも 0.17.1 で
  同一（バージョン変化は要因から棄却）。乖離の主因は (1) 計測対象範囲差
  （PoC-5 の `pluggable-core` は常時依存が重く baseline 110〜111、
  `backend-framework-core` は pay-for-what-you-use 徹底で baseline 69 と小さいため、
  同程度の絶対増分でも比率が拡大するベースライン圧縮効果）、副次的に (2) cargo-geiger
  の到達可能性（used）判定がバージョン・環境に依存し完全な再現性を持たないこと
  （同一ソース・同一 `Cargo.lock` の再実測でも used が 247 → 306 とずれた）と特定した。
  依存側 unsafe の絶対量（feature 有効時 total 592〜594）は両時点でほぼ不変であり、
  新規の危険 unsafe パターンも確認されなかった（実害なし）。
- **NFR-6 の狭義帯（100.3〜100.8%）達成**: in-process WebRTC のバイナリサイズ削減
  （不要な `webrtc-rs` feature の絞り込み等）は性能最適化の深掘りであり本タスクの
  スコープ外。対応方針は上記の通り別プロセス切り出し推奨として文書化済み。
- **専有計測環境（#178）**: `benches/nfr6-exclusive.sh`（flock 相互排他 + 静穏確認、
  `docs/design/nfr6-exclusive-measurement.md`）を整備した。並列 issue 実装ワークフロー
  下では既定の静穏閾値が成立せず、緩和閾値での参考計測は FAIL（RPS 比 88.10%・p95 比
  112.05%、既存 FAIL 記録と整合）となった（`benches/reports/task-8.4-webrtc-nfr6.md`
  追補節）。既定閾値での確定再計測は host が真に静穏な期間にフォローアップとして
  別途実施する。**2026-07 再実行（#220）でも専有計測は BLOCKED、参考実測は割れており、
  いずれも旧確定 FAIL を覆す根拠にならないため、基準 E の FAIL 判定は維持している。
  下記「2026-07 再実行」節参照。**

## 2 クレートの区別（再確認）

`crates/plugin-webrtc`（in-process、`webrtc` feature）と `crates/plugin-webrtc-proxy`
（別プロセス切り出し、`webrtc-proxy` feature）はクレート境界で完全に分離しており、
相互に path 依存しないことを機械検証済み（基準 D補足）。両 feature は
`--all-features` で共存コンパイル可能（`crate::plugin::try_intercept` が
`webrtc-proxy` を優先評価）。

## 検証コマンド一覧（再現手順）

```bash
# A・B・C・D・E をまとめて実行
bash scripts/accept/webrtc-accept.sh

# 判定ロジックのオフライン・セルフテスト（cargo 非依存）
bash scripts/tests/run-webrtc-accept-tests.sh

# NFR-6 計測用バイナリのビルド（E の前提）
cargo build --release -p backend-framework-core --example minimal --no-default-features
cargo build --release -p backend-framework-core --example webrtc_nfr6 --features webrtc

# 攻撃表面の契約テスト（black-box、公開 API 経由）
cargo test -p bf-plugin-webrtc --test attack_surface

# 依存インパクトの個別確認
cargo tree -p backend-framework-core | grep -c webrtc                    # 0
cargo tree -p backend-framework-core --features webrtc | grep -c webrtc  # 23
```

## 2026-07 再実行（#220）

イシュー #220 は、本レポート 69 行目付近（基準 E の「production コードの不具合として
扱わない理由」段落）の FAIL 記載を「cargo-geiger の呼び出し方に起因する FAIL」と記述して
起票された。本節冒頭でまず前提を整理し、そのうえで受け入れ基準（該当検証項目の再実行と
実測結果での更新、ツール起因で解消不能な場合の区分変更）を実施した結果を記録する。

### 前提の整理（イシュー記載との乖離）

再調査の結果、本レポートに現存する FAIL は**基準 E（NFR-6 無関係パス影響）の 1 件のみ**で
あり、これは `oha` による性能実測起因であって cargo-geiger とは無関係と確認した。
cargo-geiger に関する記載は 2 箇所のみ:

- 実行環境表の cargo-geiger 行（25 行目）: 「TASK-8.1 時点の『実行失敗』記録は誤った
  呼び出し方によるもので、本タスクで訂正した」という**解消済みの注記**
- 基準 B補足（34 行目）: **WARN・実測成功済み**（baseline 69/170 → webrtc 有効
  304/592）

「cargo-geiger 呼び出し起因の FAIL」は現行レポートに存在しない。起票者はこの B補足注記
と、直後の基準 E の FAIL（NFR-6・oha 実測起因）を混同したと判断する。この整理は自動運転
モードでの実装のため差し戻さず、イシューの実質的要求「レポートに残る FAIL 記載を
再実行・実測で更新し、確定不能ならば区分と理由を改める」+「geiger 起因と誤読されうる
記載の明確化」として忠実に実施した。

### cargo-geiger 再実測（基準 B補足、呼び出し方起因の失敗が再発しないことの実証）

`webrtc-accept.sh` と同一の絶対パス `--manifest-path` 呼び出しで再実行した。

```
$ cargo geiger --output-format Ascii --manifest-path "$(pwd)/crates/core/Cargo.toml" --no-default-features
...
69/170     3812/6464    119/168 4/4     143/299
```

```
$ cargo geiger --output-format Ascii --manifest-path "$(pwd)/crates/core/Cargo.toml" --features webrtc
...
304/592    24791/33700  613/716 83/87   904/1265
```

Functions 列（used/total）は baseline 69/170・webrtc 有効 304/592 で、レポート初出時
（34 行目）と完全に一致した。`cargo geiger` 自体の終了コードは 205 件の依存側 build
warning により非 0（`error: Found 205 warnings`）になるが、Ascii レポートは正常に出力
されており、これは geiger 実行の失敗ではない（`webrtc-accept.sh` も標準出力の Functions
行のみを解析するため、この warning 終了コードの影響を受けない）。**呼び出し方起因の
失敗は再発しておらず、25 行目の訂正記録は解消継続を確認した。**

### 基準 A〜D・受け入れスクリプト全体の再実行

環境: 実行日時 2026-07-19、対象コミット `c26090c695b4a5c1bd4bb2bddae023e4a0a178d9`
（`origin/main`）、rustc/cargo 1.96.0・cargo-audit 0.22.2・cargo-deny 0.19.8・oha 1.15.0
（実行環境表と同一）。`bash scripts/accept/webrtc-accept.sh` を実行し、A〜D は全項目
再確認して PASS/WARN を維持した（依存構成・unsafe 自コード 0 件・audit/deny・2 feature
分離のいずれも初出時から変化なし）。

### 基準 E（NFR-6）の再計測

**専有計測（`benches/nfr6-exclusive.sh`）は BLOCKED（判定不能）**。並列 issue 実装
ワークフロー下で 90 秒待っても静穏（loadavg 1 分値 ≦ 1.0）が得られず
（`snapshot_loadavg1=1.33`、既定の静穏待機上限 1800 秒を待っても解消する見込みが薄いと
判断し早期に区分を確定した。静穏閾値 `LOAD1_MAX=1.0` は緩和していない）、
`FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE`（2）で終了した。これは #178 が既に文書化した
「並列 issue 実行環境下では NFR-6 の確定計測が成立しない」問題の再現であり、host
contention は本イシュー実装環境自体（同時に複数イシューが並列実行されている）に起因
する。

専有枠なし（host contention あり）の参考実測を 3 回実施した結果は次の通り
（`RUNS=5 DURATION=5s CONNECTIONS=32`、`benches/webrtc-nfr6-bench.sh` / 3 回目のみ
生ログを直接実行、1・2 回目は `webrtc-accept.sh` 経由）:

| 実行 | RPS 比（webrtc / baseline） | p95 比（webrtc / baseline） | `evaluate_nfr6_ratio` |
|------|------|------|------|
| 1 回目（`webrtc-accept.sh` 経由） | 96.14% | 103.03% | WARN（実務帯内・狭義帯外） |
| 2 回目（直接実行） | 94.64% | 104.17% | FAIL（RPS 比が実務下限 95% を僅かに下回る） |
| 3 回目（直接実行） | 96.08% | 104.36% | WARN（実務帯内・狭義帯外） |

3 回中 2 回は実務許容帯 [95%, 105%] 内（WARN 相当）、1 回は実務下限を 0.36 ポイント
下回り FAIL 相当となった。3 回とも p95 比は狭義帯 100.8% を超えるが実務帯 105% 内。

**注意（初出時実測との単純比較はできない）**: 初出時の確定 FAIL 記録（RPS 比
93.86〜95.23%・p95 比 106.57〜108.45%）は `CONNECTIONS=128 DURATION=15s` での計測で
あるのに対し、本節の 3 回はいずれも `benches/webrtc-nfr6-bench.sh` の既定値
`CONNECTIONS=32 DURATION=5s`（初出時レポートの「事前計測（小規模確認）」節と同一
パラメータ）である。RPS 比は両パラメータでおおむね重なるが、p95 比は本節の 3 回
（103〜104%）の方が初出時の確定計測（106〜108%）より一貫して良好であり、これは
接続数の差（128 vs 32）に起因する可能性が高く、host contention による測定ノイズ
のみで説明できるとは言い切れない。したがって「初出時 FAIL がノイズの範囲内だった」
と断定はせず、**同一パラメータ・真に静穏な環境での確定比較がまだ行われていない
ため判定不能**、という消極的な結論にとどめる（捏造しない・フェイルクローズ）。

**判断（旧 FAIL 判定を維持、区分変更は行わない）**: 専有計測が BLOCKED で確定できず、
参考実測も WARN 相当と FAIL 相当の間で割れている。加えて、参考実測は旧確定 FAIL 計測
（`CONNECTIONS=128 DURATION=15s`）とは異なるパラメータ（既定値 `CONNECTIONS=32
DURATION=5s`）で行っており、下記「注意」のとおり p95 比の改善が接続数の差に起因する
可能性を否定できないため、これを根拠に旧 FAIL を覆すことはできない。したがって基準 E
は次のとおり**旧 FAIL 判定を維持**し、参考実測は判定を補強する追加情報としてのみ
併記する。

> **E（現状維持）: FAIL** — 旧確定計測（`CONNECTIONS=128 DURATION=15s`、RPS 比
> 93.86〜95.23%・p95 比 106.57〜108.45%）を正とし、判定を維持する。専有計測
> （`benches/nfr6-exclusive.sh`）は並列 issue 実行下の host contention（#178 既知問題）
> により BLOCKED（判定不能）で、旧 FAIL を覆す・確定させるいずれの根拠にもならない。
> 参考実測 3 回（別パラメータ・host contention あり）は 2 回が実務許容帯 [95%, 105%]
> 内（WARN 相当）、1 回が僅かに下回り FAIL 相当、狭義帯（100.3〜100.8%）には 3 回とも
> 収まらない。**この参考実測のみを根拠に FAIL→WARN へ区分を緩めることはしない**
> （同一パラメータ・真に静穏な環境での確定再計測が済むまでは旧 FAIL 判定を維持する、
> 捏造しない・フェイルクローズ）。真に静穏な環境での同一パラメータ確定再計測を
> フォローアップとして別途実施する必要がある。旧 FAIL 実測値（RPS 比
> 93.86〜95.23%・p95 比 106.57〜108.45%）は履歴として保持し、改変・削除しない。

イシュー #220 が想定した受け入れ条件「ツール起因で解消不能な場合の SKIP/WARN への
区分変更・理由明記」は、本件には**適用しない**。本件の未確定理由は cargo-geiger では
なく **NFR-6 計測の host contention**（#178 既知問題）であり、かつ根拠となる参考実測が
旧確定計測と比較不能なパラメータ（接続数 128 vs 32）での実測であるため、区分変更の
正当化としては不十分と判断した（cargo-geiger 自体は上記のとおり正常動作を再確認済み。
この対応関係の詳細な整理は PR 本文にも明記する）。

`webrtc-accept.sh` は実行のたびに `evaluate_nfr6_ratio` が実測値から動的に判定する
ため、1 回目実測（WARN 判定）を用いた実行では総合終了コードが `0`（FAIL なし）と
なることがある。しかし上記のとおり参考実測にはばらつきがあり（3 回中 1 回は FAIL
相当）、本レポートはこの 1 回の実行結果を確定判断とはしない。本レポート上の基準 E の
**公式判定は FAIL のまま据え置く**（捏造しない・フェイルクローズ、
`.claude/rules/security.md`）。

### まとめ

- cargo-geiger 呼び出し起因の FAIL は現行レポートに存在せず、25 行目の訂正記録どおり
  解消が継続していることを再実測で確認した（イシュー起票の前提誤認を訂正）。
- 唯一の FAIL 記載（基準 E、NFR-6）は cargo-geiger と無関係な `oha` 実測起因である。
  専有計測は BLOCKED、参考実測（別パラメータ・host contention あり）は割れており、
  いずれも旧確定 FAIL を覆す根拠にならないため、**基準 E の判定は FAIL のまま維持する**
  （区分変更は行わない）。実測値は改変せず、旧 FAIL 記録・新規参考実測をすべて保持する。
- 残課題（真に静穏な環境・同一パラメータでの基準 E 確定再計測）は既存の
  「BLOCKED / フォローアップ」節および #178 のスコープに含まれ、新規 Issue 化は不要と
  判断した（既存フォローアップの再確認のため）。

再実行時の生ログは `benches/reports/task-8.4-webrtc-nfr6.md` の追補節を参照。

## REQ-8 webrtc unsafe 増分の削減策評価とリスク受容判断確定（#242）

**スコープの明確化**: 本節は基準 B補足（依存側 unsafe 増分、WARN）のみを対象とする。
本節の直前まで論じてきた基準 E（NFR-6 無関係パス性能影響、FAIL）は host contention
（#178）に起因する別課題であり、本節では扱わない（再論しない）。

親トラッキング #235 の Conditional Go 条件(2)「WebRTC 別プロセス切り出し・攻撃表面評価」
が「条件付き解消」のまま残っていた要因である基準 B補足の WARN について、(1) 悪化要因の
特定記録の確定、(2) 削減策（バージョン更新・feature 絞り込み）の評価結果と適用/不適用の
根拠、(3) 削減不能な残余リスクの受容判断、の 3 点を確定させた。詳細な調査記録・根拠データは
`docs/dep-impact/records.md` の「2026-07-19 — REQ-8 webrtc unsafe 増分の削減策評価と
リスク受容判断確定（#242）」エントリに記録した。本節ではその結論のみを要約する。

### 1. 悪化要因（結論の最終確定）

見かけの「約 2.2 倍→約 4.4 倍」の乖離は #183 で既に特定済みであり（主因: pay-for-what-
you-use 徹底によるベースライン圧縮効果、副次要因: cargo-geiger の到達可能性判定の
非決定性）、2026-07-19 の再実測（本レポート「2026-07 再実行（#220）」節）でも
Functions 69/170・304/592 が完全一致することを確認済み。依存側 unsafe の絶対量
（feature 有効時 total 592〜594）は不変で新規の危険パターンもなく、**見かけの比率悪化は
実害の増加ではない**ことを最終結論として確定する。

### 2. 削減策の評価

- **バージョン更新**: `docs/design/webrtc-rs-version-strategy.md` の 2026-07-17 再確認
  （crates.io 最新版は引き続き v0.17.1、Sans-I/O 系 `rtc` は安定版未成立）を根拠に
  **不適用**。本セッションはネットワークアクセス権限のない subagent 実行環境のため
  crates.io への再照会は実施できず（`curl` 実行不能を確認済み）、2 日前の直近確定記録に
  依拠した旨を明記する（捏造しない・断定と推測の区別）。
- **feature 絞り込み**: `webrtc` 0.17 系は SDP/ICE/SCTP/DataChannel が単一 feature 構成で
  不可分に結合しており、`crates/plugin-webrtc` の調査範囲では安全に除去できる default-on
  feature を特定できなかったため**現時点では不適用**（機能除去不可能と断定したわけではなく
  調査不足による保留であることを明記）。安全な除去候補が将来的に見つかった場合は
  production 変更（`crates/plugin-webrtc/Cargo.toml`）を伴うため、
  `.claude/rules/out-of-scope-tracking.md` に従い別 Issue として切り出す。

### 3. 残余リスクの受容判断

削減策がいずれも不適用のため、`webrtc` feature を有効化した in-process 利用時の依存側
unsafe（大きな絶対量）は削減されず残る。この残余リスクは、`AGENTS.md`「WebRTC の攻撃表面と
『使う/使わない』サービスの安全性方針」・`crates/plugin-webrtc/Cargo.toml` の既存フレーミング
（「明示的に非推奨の in-process プラグインを opt-in したサービスにのみ顕在化する」）に
依拠した**リスク受容案（提案）**として記録する。既定構成・MVP 推奨の別プロセス切り出し版
（`crates/plugin-webrtc-proxy`、webrtc-rs 依存 0 件）を利用するサービスは本リスクの
影響を受けない。

**承認フローの扱い（自動運転モード）**: 本判断はユーザー承認を待たずに記録するが、
**最終承認は本タスク（#242）の PR レビュー（人間承認ゲート）で行う**
（`webrtc-rs-version-strategy.md`・`.claude/rules/feature-modification.md` の既存前例と
同一原則）。

### 基準 B補足の確定扱い

上記を根拠に、基準 B補足の判定を「**受容 WARN（削減不能・残余リスク受容済み、PR レビュー
承認をもって確定）**」として確定する。PASS への丸め込みは行わず、WARN のまま実測値・
判断根拠を保持する（捏造しない・フェイルクローズ、`.claude/rules/security.md`）。
