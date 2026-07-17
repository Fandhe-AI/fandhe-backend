# AGENTS.md

## 文書の位置づけ

本リポジトリで作業するすべての AI エージェント・開発者が従う設計規約集。
全体の運用ガイドは `CLAUDE.md`、Rust コーディング規約の詳細は `.claude/rules/`
（特に [coding-rust.md](.claude/rules/coding-rust.md)）を参照し、本書は
`CLAUDE.md` / `.claude/rules/` と内容を重複させず、実装コード（`crates/**`）から
直接参照される横断的な設計規約のみを記載する。

## 規約: ミドルウェア非同期 I/O 必須化

TASK-2.3（`docs/spec/05-tasks.md`、Phase 1 / MS-1、親 Issue #4、前提 TASK-2.1 #18）
対応。`docs/spec/04-requirements.md` REQ-2 受け入れ基準・NFR-7 を満たす規約文書。

### 規約本文

全リクエストに介入する `Middleware` 実装（`crates/core/src/extension.rs` の
`Middleware` trait、`on_request` / `on_response`）は**非同期・バッファ済み I/O を
既定**とする。同期ブロッキング I/O 実装（同期 `eprintln!`・同期ファイル書き込み・
`std::net` 直接利用等）は**不採用**とする。

`Middleware` trait 自体は dyn 互換性（`Box<dyn Middleware>` としてコアループが
拡張点を保持する構成）を保つため `async fn` を持たない同期 API として定義される
（`crates/core/src/extension.rs` モジュール doc「非同期・I/O に関する規約」節）。
本規約はこの同期 API の**制約下で守るべき実装契約**であり、trait のシグネチャ変更
を求めるものではない。

### 実装パターン

I/O が必要な実装は、フック（`on_request` / `on_response`）内では非同期チャネルへの
送信、またはアトミックカウンタの更新等の**非ブロッキング操作に留め**、実際の I/O
（ファイル書き込み・ネットワーク送信等）は別タスク（バックグラウンドタスク・
`tracing-appender` の non-blocking writer 等）に委譲する。

### 根拠（PoC-3 実測、`docs/spec/03-poc/plugin-mechanism/README.md`）

全リクエストに介入するミドルウェア型プラグイン（ロギング）を素朴な同期 I/O
（リクエストごとの同期 `eprintln!`）で実装すると、`/health` の RPS が
**725,024 → 44,108 RPS（無効時比 25.0%）** まで劣化した。同一の `Middleware`
trait 実装のまま I/O を停止し、アトミックカウンタの更新のみに切り替えて計測
（`ACCESS_LOG_QUIET=1`）すると **177,549 RPS（無効時比 100.5%）** まで回復した。

この切り分けにより、劣化要因は「`Middleware` trait 呼び出し（動的束縛）のコスト
自体」ではなく「プラグインが選んだ I/O 実装の質（同期か非同期か）」であることが
実証された。

補足として、PoC-10（`docs/spec/04-requirements.md` REQ-10）でも同旨の実測がある。
可観測性ミドルウェアを同期 writer で実装した場合に RPS が **63.0% 劣化**すること
に加え、非同期 writer に切り替えても span/event 生成の CPU コストにより RPS が
31.6% 劣化する事例が確認されており、**非同期 I/O 化だけでは pay-for-what-you-use
の性能目標を満たさない場合がある**（サンプリング・イベント数削減・高頻度パス除外
等の追加対策は REQ-10 側のスコープであり、本規約は「同期 I/O の不採用」という
最小限の必須要件を定めるものである）。

### 出典リンク

- `docs/spec/03-poc/plugin-mechanism/README.md`（PoC-3 性能比較表・発見事項）
- `docs/spec/02-poc-plan.md`（PoC-3 計画）
- `docs/spec/04-requirements.md`（REQ-2・NFR-7、参考: REQ-10・PoC-10）
- `docs/spec/05-tasks.md`（TASK-2.3）
- `crates/core/src/extension.rs`（`Middleware` trait 定義・同旨の契約を doc comment に記載）

### 適用範囲と検証責務

標準提供ミドルウェア有効化時のコア RPS 劣化は 5% 以内を維持する（NFR-7 受け入れ
基準）。レビュー時の本規約準拠確認は `reviewer` / `plugin-builder`、性能検証は
`bench-builder` が担う（[delegation-impl.md](.claude/rules/delegation-impl.md)）。

### 可用性・可観測性に関する注記

- **リソース枯渇（DoS）耐性**: 全リクエストのホットパスに載るミドルウェアが同期
  I/O を行うと、スロー I/O（ディスク詰まり・パイプブロック等）発生時にワーカー
  スレッドが枯渇し、サービス全体が応答不能に陥りうる。本規約はこのリスクを構造的
  に排除する（[security.md](.claude/rules/security.md) の「リソース枯渇（DoS）」
  観点）。
- **ログ欠落の許容可否**: 非同期・バッファ済みログはバックプレッシャ時にイベント
  欠落（drop）が起こりうる。セキュリティ監査イベント等、欠落を許容できないログの
  扱いは、標準ロギング／トレーシング実装（REQ-10・`plugin-tracing` 系タスク）側
  の設計事項として別途定める。本規約はこの論点を暗黙に決定しない。
