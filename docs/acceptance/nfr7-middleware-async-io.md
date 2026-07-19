# NFR-7 受け入れ検証レポート — ミドルウェア型プラグインの非同期 I/O 必須化（イシュー #263）

## 本レポートの位置づけ

`docs/spec/04-requirements.md` NFR-7（ミドルウェア型プラグインの非同期 I/O 必須化）の
実装・規約明文化はすでに完了しているが、他 REQ/NFR（`req1`〜`req13`・`req15` 等）が従う
`docs/acceptance/<id>-<topic>.md` の命名・配置パターンに対し、NFR-7 単独の受け入れレポート
は存在せず `docs/acceptance/req10-tracing.md` 基準 C（`plugin-tracing` の性能検証）へ
間接依拠している状態だった（仕様照合 #252 で検出。REQ-10 の #219・REQ-11 の #236 と同種の
「配置不整合」是正の系譜）。本レポートは #263 でこの配置不整合を是正するために、既存実測
（PoC-3・`docs/acceptance/req10-tracing.md` 基準 C・`AGENTS.md` 規約）の転記・出典整理により
NFR-7 の受け入れ記録を独立文書化したものである。

**作成時点での再実測は行っていない**。実測値はすべて下表の出典からの転記であり、一次記録・
詳細解説はそちらを参照する。

## 受け入れ基準と実測根拠の対応表

`docs/spec/04-requirements.md` NFR-7 の受け入れ基準（2 項目）に対する判定は次のとおり。

| 受け入れ基準（`04-requirements.md` NFR-7） | 判定 | 出典（名指し） |
|---|---|---|
| 基準 1: 全リクエストに介入する `Middleware` 実装は非同期・バッファ済み I/O で実装される（同期 I/O 実装は不採用とする設計規約が `AGENTS.md` に明記されている） | 充足 | `AGENTS.md`「規約: ミドルウェア非同期 I/O 必須化」節（TASK-2.3、親 Issue #4 系）+ `crates/core/src/extension.rs` の `Middleware` trait doc comment（同旨の契約を記載） |
| 基準 2: 標準提供するミドルウェアの有効化時、コア性能（RPS）の劣化が 5% 以内である | 充足 | `docs/acceptance/req10-tracing.md` 基準 C（シナリオ A: RPS 比 98.59% = 劣化 1.41%、p95 比 102.27%。受け入れ帯 RPS 比 ≥95% かつ p95 比 ≤110%）。一次記録は `docs/reports/task-10-5-acceptance.md`（2026-07-18 実施）、性能詳細は `benches/reports/task-10.4-tracing-performance.md` |

両基準とも充足（PASS）。判定区分は `04-requirements.md` 表の「関連 PoC」欄記載どおり PoC-3（OK）。

## PoC-3 実測根拠（規約の設計根拠）

`AGENTS.md`「規約: ミドルウェア非同期 I/O 必須化」節・出典 `docs/spec/03-poc/plugin-mechanism/README.md`
の性能比較表から、数値を改変せずに転記する。

| 構成 | RPS | 無効時比 |
|---|---|---|
| 全プラグイン無効（コアのみ、基準） | 725,024 | 100%（基準） |
| ロギングミドルウェア（`ACCESS_LOG_QUIET=1`、アトミックカウンタのみ） | 177,549 | 100.5% |
| ロギングミドルウェア（既定、同期 `eprintln!` あり） | 44,108 | **25.0%** |

同一の `Middleware` trait 実装のまま同期 `eprintln!` を止めてアトミックカウンタ更新のみに
切り替える（`ACCESS_LOG_QUIET=1`）と RPS が 100.5% まで回復したことから、劣化要因は
「`Middleware` trait 呼び出し（動的束縛）のコスト自体」ではなく「プラグインが選んだ I/O
実装の質（同期か非同期か）」であると切り分けられた。これが NFR-7 の設計根拠である。

補足として、PoC-10（REQ-10）でも同旨の実測がある。可観測性ミドルウェアを同期 writer で
実装した場合に RPS が 63.0% 劣化し、非同期 writer に切り替えても span/event 生成の CPU
コストにより RPS が 31.6% 劣化する事例が確認されている。サンプリング・イベント数削減・
高頻度パス除外といった追加対策は REQ-10 側のスコープであり、NFR-7 は「同期 I/O の不採用」
という最小限の必須要件を定めるものであるという責務境界を、ここに明記しておく。

## 乖離・限界の正直な記録

隠さず記録するフェイルクローズ原則（`.claude/rules/security.md`）に従い、実測の前提条件の
差異・限界を以下に記す。

- **基準 2 の実証範囲は `plugin-tracing` 1 実装に限られる**: 現時点で `Middleware` 型の
  標準提供プラグインは `plugin-tracing`（TASK-10.1、#56）のみであり、基準 2 の PASS 判定は
  `docs/acceptance/req10-tracing.md` 基準 C のシナリオ A（サンプリング + イベント統合 +
  `/health` 除外構成）の実測 1 件に基づく。将来的に別の `Middleware` 型標準ミドルウェアを
  追加する場合は、同基準での個別の性能検証が別途必要である（過度な一般化はしない）。
- **PoC-3 実測の計測条件**: macOS Apple Silicon 環境・各構成 1 回計測（`docs/spec/03-poc/plugin-mechanism/README.md`）。
  複数回計測・中央値評価という現行の `benches/README.md` の計測規約とは前提が異なる。
- **`docs/acceptance/req10-tracing.md` 実測の計測条件**: 2026-07 の crate・import 一括改名
  （#202）以前の実測記録であり、旧クレート名（`backend-framework-core` 等）表記のまま
  保持されている（同レポート冒頭注記）。実施環境は Linux x86_64（Ubuntu）で PoC-3 の
  macOS 環境とは異なる。実測値本文はそれぞれの一次記録から改変せず転記している。

## 再現手順

基準 2 の実測（`docs/acceptance/req10-tracing.md` 基準 C）は次のスクリプトで再現できる。

```bash
bash scripts/accept/tracing-accept.sh
```

PoC-3 実測自体は PoC フェーズの一次記録（`docs/spec/03-poc/plugin-mechanism/README.md`）
であり、再実行用の常設スクリプトはない。

## 関連リンク

- `docs/spec/04-requirements.md`（NFR-7）
- `AGENTS.md`（「規約: ミドルウェア非同期 I/O 必須化」節）
- `docs/acceptance/req10-tracing.md`（基準 C の実測）
- `docs/reports/task-10-5-acceptance.md`（req10-tracing.md の一次記録）
- `benches/reports/task-10.4-tracing-performance.md`（性能詳細）
- `docs/spec/03-poc/plugin-mechanism/README.md`（PoC-3 性能比較表・発見事項）
- 関連 Issue: #252（仕様照合で検出）・#255・#263（本レポート）。参考: #219（req10-tracing.md 是正）・#236（req11 是正）
