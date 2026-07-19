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
| cargo-geiger | 0.13.0（`--manifest-path` を絶対パスで指定すれば実行可能。TASK-8.1 時点の「実行失敗」記録は誤った呼び出し方によるもので、本タスクで訂正した） |

## 判定サマリー（`scripts/accept/webrtc-accept.sh`）

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A: webrtc 無効時の依存完全除外 | `cargo tree -p backend-framework-core \| grep -c webrtc` = 0 |
| WARN | A補足: webrtc 有効時の依存インパクト | `cargo tree -p backend-framework-core --features webrtc \| grep -c webrtc` = 23（`docs/dep-impact/records.md` 参照） |
| PASS | B: plugin-webrtc 自コード unsafe 0件 | `crates/plugin-webrtc/src` に unsafe 0 件（テキストベース走査） |
| WARN | B補足: 依存側 unsafe 増分（webrtc-rs、cargo geiger 実測） | baseline（webrtc 無効）: Functions 69/170 / webrtc 有効: Functions 304/592（約 4.4 倍。PoC-5 実測「約 2.2 倍」より大きい。`docs/dep-impact/records.md` 参照） |
| PASS | C: cargo audit / deny 0件 | `scripts/dep-audit.sh`（全 feature 構成）が正常終了（advisories ok, bans ok, licenses ok, sources ok） |
| PASS | D: webrtc/webrtc-proxy 2 feature の存在 | `backend-framework-core` に `webrtc`（in-process）・`webrtc-proxy`（別プロセス切り出し）の両 feature が存在 |
| PASS | D補足: in-process/proxy のクレート境界分離 | `crates/plugin-webrtc` と `crates/plugin-webrtc-proxy` は相互依存なし |
| FAIL | E: NFR-6 無関係パス影響 | RPS 比 94〜95% / p95 比 106〜108%（webrtc 有効 / ベースライン、`GET /` への負荷計測。狭義の NFR-6 帯 100.3〜100.8% には収まらない。詳細下記） |

**終了コード: 1（FAIL あり、基準 E）**

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
  別途実施する。上記基準 E の FAIL 判定は維持する。

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
