# REQ-11 受け入れ検証レポート — AI ファーストな保守性の構造規約（TASK-11.1〜11.5）

## 本レポートの位置づけ

`docs/spec/04-requirements.md` REQ-11（AI ファーストな保守性の構造規約）の受け入れ記録は、
他 REQ（`req1`〜`req10`・`req12`・`req13`・`req15`）が従う `docs/acceptance/req<N>-<topic>.md`
の命名・配置パターンに対し `docs/reports/task-11-5-acceptance.md`（TASK-11.5、#37）にのみ
存在し、配置が不整合だった（イシュー #236。REQ-10 の同種の不整合は #219 で是正済み）。

本レポートは REQ-11 の受け入れ基準 5 項目それぞれについて、**2026-07-19 時点の実測・
機械検証結果**と一次記録への参照を集約する。REQ-10 のレポート（転記のみ）と異なり、
カバレッジ・依存方向の 2 項目は本レポート作成時に再実測している（下記「実測」列に明記）。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実施日 | 2026-07-19 |
| OS | Linux 7.0.0-27-generic x86_64（Ubuntu） |
| rustc / cargo | 1.96.0（stable, 2026-05-25） |
| 前提コミット | `54e87a7`（`origin/main`、PR #244 マージ済み） |

## 判定サマリー

| 判定 | 受け入れ基準 | 検証方法 | 結果 |
|------|------------|---------|------|
| PASS | コア全体の自動テスト行カバレッジ 80% 以上 | `scripts/coverage.sh --fail-under-lines 80`（本レポート作成時に再実測） | コア対象（`fandhe-backend-core` / `-http` / `-routes` / `ws-load-client`）の行カバレッジ **90.20%**（Regions 91.17% / Functions 91.96%）。workspace 全体では行 91.32% |
| PASS | 全公開 API の doc コメント網羅率 100%・doc test を伴う | `Cargo.toml` workspace lint `missing_docs = "warn"` + CI `cargo clippy -- -D warnings` による機械強制、CI `doc` ジョブ | 未記載の公開 API があれば `warn` が `-D warnings` で昇格し CI 失敗。`ci-complete` green をもって網羅率 100% を機械的に担保 |
| PASS | `AGENTS.md` にモジュール境界・変更手順・判定基準・エスカレーション基準・アサーション網羅性が明記 | 目視確認（節構成） | 「AI エージェント向け変更ガイド」節配下に 5 項目すべてが対応する小節として存在（下記「証跡 3」） |
| PASS | CI にテスト単位のタイムアウトが設定されている | `.config/nextest.toml` + `.github/workflows/ci.yml` | nextest の `slow-timeout = { period = "60s", terminate-after = 2 }`（120 秒で強制終了・TIMEOUT 失敗扱い）。加えて CI 全 16 ジョブに `timeout-minutes` を設定（多層防御） |
| PASS | 依存方向が一方向（循環依存なし）であることをコード上で確認できる | `scripts/dep-direction-check.sh`（本レポート作成時に再実行） | 3 検証すべて PASS（終了コード 0、下記「証跡 5」） |

**FAIL 0 件・WARN 0 件**。5 基準すべて PASS。

## 証跡

### 1. カバレッジ（コア対象 行 90.20%）

`scripts/coverage.sh --fail-under-lines 80` の再実測結果（抜粋）。

```
==> コア対象パッケージ: fandhe-backend-core fandhe-backend-http fandhe-backend-routes ws-load-client
core/src/server.rs      1056  56  94.70%  ...  746  51  93.16%
http/src/connection.rs   653  15  97.70%  ...  362   4  98.90%
http/src/request.rs      566   7  98.76%  ...  327   3  99.08%
routes/src/lib.rs        349  17  95.13%  ...  180  14  92.22%
TOTAL                   4972 439  91.17%  ... 3032 297  90.20%
==> coverage.sh: コア対象の行カバレッジが 80% 以上であることを確認しました
```

- コア対象の行カバレッジ **90.20%**（閾値 80% に対し +10.20 ポイント）
- 参考: workspace 全体（プラグイン含む）は行 91.32%
- PoC-9 実測（94.29%〜95.99%）より低いが、これは計測対象が PoC スケルトンから
  実装フェーズの全コア（負荷生成クライアント `ws-load-client` の行 37.13% を含む）へ
  拡大したためであり、受け入れ基準（80% 以上）は充足している
- CI では `coverage` ジョブが同一スクリプト・同一閾値で実行され、`ci-complete` の
  判定対象に含まれる（機械強制。`.github/workflows/ci.yml`）

### 2. doc コメント網羅率 100%・doc test

- `Cargo.toml`（workspace lints）に `missing_docs = "warn"` を設定
- CI の `clippy` ジョブが `cargo clippy -- -D warnings` を実行するため、`warn` は
  エラーに昇格する。公開 API に doc コメントが欠けた時点で CI が失敗する
- ローカル反復開発の利便性のため lint レベル自体は `warn` を維持し、CI 側で
  `-D warnings` により強制する二段構え（`Cargo.toml` の該当コメント参照）
- doc test は CI `test` ジョブ（`cargo test`）が実行し、`doc` ジョブが rustdoc lint を検証する
- 一次記録: `docs/reports/task-11-5-acceptance.md`（TASK-11.5、#37）

> 注記: 「網羅率 100%」は機械強制の結果として担保される性質のものであり、本レポートでは
> 独立した計測値を別途算出していない（`missing_docs` が 1 件でも検出されれば CI が
> 失敗するため、`ci-complete` green が網羅率 100% と同値）。

### 3. AGENTS.md の記載事項

`AGENTS.md`「AI エージェント向け変更ガイド」節（TASK-11.3、#35）配下の小節構成:

| 受け入れ基準の要求項目 | 対応する節 |
|---------------------|-----------|
| モジュール境界 | 「モジュール境界」 |
| 変更手順（新規エンドポイント追加手順を含む） | 「変更手順」／「新規エンドポイント追加手順」 |
| 変更完了の判定基準（`cargo test` / `clippy -D warnings` / `fmt --check`） | 「変更完了の判定基準」 |
| アサーションの網羅性要求 | 「アサーション網羅性」 |
| 安全性方針 | 「安全性方針」 |
| エスカレーション基準 | 「エスカレーション基準」 |

### 4. CI テスト単位タイムアウト

`.config/nextest.toml`:

```toml
# 60 秒経過で「slow」警告を出し、以後 60 秒ごとに追加の period を消費する。
# 2 period（= 120 秒）経過したテストは強制終了（SIGTERM）して失敗（TIMEOUT）扱いにする。
slow-timeout = { period = "60s", terminate-after = 2 }
```

- PoC-9 で確認された「オフバイワンのバグがテスト失敗ではなく `cargo test` のハングとして
  現れる」事例（REQ-11 詳細）に対応するテスト単位タイムアウト
- 加えて `.github/workflows/ci.yml` の全ジョブ（16 箇所）に `timeout-minutes` を設定し、
  ハングしたジョブが self-hosted runner を無期限占有することを防ぐ多層防御
  （`.claude/rules/ci.md`・TASK-11.4、#36）

### 5. 依存方向の一方向性

`scripts/dep-direction-check.sh` の再実行結果:

```
[PASS] 1: 依存エッジホワイトリスト照合 — 循環なし・全エッジが許可リストに合致
[PASS] 2: エントリポイント依存方向宣言 — crates 直下 12 クレート全てのエントリポイントに統一形式の宣言あり
[PASS] 3: プラグイン非依存（core/routes/http） — crates/core・crates/http・crates/routes にプラグイン固有シンボル・依存を検出せず
=== 依存方向一方向性検証: PASS ===
```

- 依存エッジは `core → {http, routes, plugin-*}`、`routes → http`、`plugin-* → http`、
  `plugin-hub-wiring → {core, http}` の一方向のみで循環なし
- 各クレートのエントリポイント（`lib.rs` / `main.rs`）の doc コメントに依存方向宣言があり、
  機械可読な形で確認できる（REQ-11 詳細「`lib.rs` の doc コメントに明記」を充足）
- CI では `pay-for-what-you-use` ジョブ等と併せて検証される（TASK-11.1、#33 / #14）

## 一次記録・関連文書

| 文書 | 内容 |
|------|------|
| `docs/reports/task-11-5-acceptance.md` | TASK-11.5（#37）網羅的自動テスト実装と受け入れテストの一次記録 |
| `AGENTS.md` | モジュール境界・変更手順・判定基準・エスカレーション基準・アサーション網羅性の規約本体 |
| `.config/nextest.toml` / `.github/workflows/ci.yml` | テスト単位タイムアウト・ジョブタイムアウトの設定 |
| `scripts/coverage.sh` / `scripts/dep-direction-check.sh` | 本レポートの再実測に用いた検証スクリプト |
| `docs/design/ci-completion-criteria.md` | `ci-complete` 集約ゲートの判定基準 |

## NFR-8 との関係

REQ-11 に対応する非機能要件 NFR-8（AI 保守容易性の測定可能化）のうち、カバレッジ 80% 以上・
doc 網羅率 100% は本レポートで確認した。自動修正成功率 70% 以上・注入リグレッション検知率
90% 以上は別途 `docs/reports/task-12-7-acceptance.md`（自動修正成功率 80%）および
イシュー #238 の確定検証で記録されている。

## 補足: 本レポートの限界

- doc コメント網羅率 100% は機械強制の帰結として担保しており、独立した計測値は算出していない
  （「証跡 2」の注記参照）
- カバレッジは 2026-07-19 時点・コミット `54e87a7` の実測であり、以降の変更で変動する。
  継続的な担保は CI `coverage` ジョブ（同一閾値）が行う
