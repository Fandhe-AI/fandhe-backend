# `tracing` エコシステム連携方式設計（TASK-10.5、#60、REQ-10）

`crates/plugin-tracing`（TASK-10.1〜10.4、#56〜#59）は既に実装済み。本文書は
「`tracing` エコシステムとどう連携するか」の設計判断を、実装済みコード（`crates/
plugin-tracing/src/*.rs` の doc comment）を正として整理し、依存インパクトの要約・
サンプリング設定と記録粒度の切り替え方法・拡張時の不変条件を一箇所にまとめる。

## 1. 背景・立ち位置

PoC-10（`docs/spec/04-requirements.md` REQ-10）は、可観測性ミドルウェアをサンプリング
なしで有効化すると RPS が最大 63.0% 劣化することを実測した。この実測を受け、REQ-10 は
可観測性機能を「サンプリング前提のオプトイン」として再定義している。本設計文書は
その再定義を踏まえ、`crates/plugin-tracing` が `tracing` / `tracing-subscriber` /
`tracing-appender` という既存 Rust エコシステムとどう連携するかを記述する。

対応タスク: TASK-10.5（#60）。前提タスク: TASK-10.1（#56、サンプリング機構）・
TASK-10.2（#57、イベント統合）・TASK-10.3（#58、高頻度パス除外）・TASK-10.4（#59、
性能再検証）・TASK-10.6（#90、バックプレッシャー実測）はすべてマージ済み。

## 2. レイヤ構成

```
アプリ層（crates/core/examples/tracing_nfr.rs 等の利用者コード）
    │  init_tracing(TracingOutput::Stdout) を起動時に 1 回呼び、
    │  戻り値の WorkerGuard をプロセス終了まで保持する
    ▼
fandhe-backend-plugin-tracing::init                       tracing_subscriber::fmt() + non_blocking writer
    │  registry ではなく tracing_subscriber::fmt() のビルダを直接使う
    │  （TASK-10.1 時点でカスタムレイヤ合成は不要と判断。将来外部レイヤを
    │    足す場合は 6 節の拡張指針を参照）
    ▼
fandhe-backend-plugin-tracing::layer::TracingLayer         除外照合 → サンプリング判定 → 記録実行
    │  crates/core 側の TracingMiddleware（tracing feature 限定 API）から
    │  Middleware 拡張点経由で呼ばれる（拡張点定義: crates/core/src/extension.rs）
    ▼
tracing::event!(...)                            tracing クレートのマクロ経由でグローバル
                                                 サブスクライバへディスパッチ
    ▼
tracing-appender::non_blocking writer           バッファ済み・非同期フラッシュ（lossy）
    ▼
標準出力（TracingOutput::Stdout、既定）
```

`fandhe-backend-plugin-tracing` は `crates/core` に依存しない（`fandhe-backend-plugin-websocket` と同一の
非循環パターン）。`Middleware` trait を実装するアダプタ（`TracingMiddleware`）は
コア側（`crates/core/src/server.rs`、`tracing` feature 限定）に置き、本クレートは
`fandhe-backend-http::request::RequestHead` の参照 + `tracing` 系クレートへの委譲のみを提供する
（`crates/plugin-tracing/src/lib.rs` 冒頭 doc、接続契約節）。

### `WorkerGuard` 保持契約

`init_tracing` の戻り値 `WorkerGuard` は**プロセス終了までスコープを保持し続ける**
必要がある。drop されると non-blocking writer のバックグラウンドフラッシュスレッド
が停止し、以降の `tracing` 呼び出しがログを出力しなくなる（`tracing-appender` 自体の
契約、`crates/plugin-tracing/src/init.rs` の doc）。呼び出し例は `init.rs` の doc test
（`init_tracing` 呼び出し側で `let _guard = ...` としてローカル変数に保持）を参照。

### non_blocking writer の lossy 特性

`init_tracing` が使う non-blocking writer は**バックプレッシャ時にイベントを破棄する
（lossy）**。有界チャネルが満杯の場合、`tracing` イベントは黙って失われる
（`tracing-appender::non_blocking` の既定動作）。この挙動は TASK-10.6（#90）の
決定的統合テスト（`crates/plugin-tracing/tests/backpressure.rs`）で実測済みであり、
高負荷時の欠落率実測・許容基準は `benches/reports/task-10.6-tracing-backpressure.md`
を参照すること。セキュリティ監査イベント等、欠落を許容できないログは既定構成
（lossy）の対象外とし、同レポート「許容基準」節が示す代替設計（ブロッキング経路・
同期書き込み）を検討する必要がある。本設計文書はこの制約を「既定構成の前提」として
そのまま引き継ぐ（矛盾する記述をしない）。

## 3. サンプリング設定の切り替え方法

[`TracingConfig`][] が保持する `sample_interval: NonZeroU64` で切り替える。

```rust
use fandhe_backend_plugin_tracing::TracingConfig;
use std::num::NonZeroU64;

// 既定（100 リクエストに 1 回記録）
let default_config = TracingConfig::default();

// 全件記録（sample_interval = 1）
let full_config = TracingConfig::new(NonZeroU64::new(1).unwrap());
```

- 既定値 `100`（REQ-10 の例示値をそのまま採用、`crates/plugin-tracing/src/config.rs`）。
  具体値の妥当性そのものは TASK-10.1 のスコープ外とされ、性能再検証（TASK-10.4）で
  この既定値のまま RPS 劣化 5% 以内・p95 悪化 110% 以内（REQ-10 成功基準）を満たす
  ことを確認済み（実測: RPS 劣化 3.34%・p95 悪化 2.77%、`benches/reports/
  task-10.4-tracing-performance.md`）
- サンプリング判定は [`Sampler`][] の `AtomicU64` カウンタによる決定的カウンタ方式
  （疑似乱数ではなく「N 件に 1 件」を厳密に満たす）
- サンプリング間隔を変更したい場合は `Server::tracing(TracingConfig { sample_interval:
  ..., ..Default::default() })` として `crates/core` 側から渡す（本クレート自体は
  `crates/core` を知らないため、配線は必ずコア側の `tracing` feature 限定 API 経由）

### 高頻度パス除外による記録粒度の一次調整

[`TracingConfig::exclude_path`][] にパスを登録すると、そのパスへのリクエストは
サンプラーのカウンタ判定より**前**に除外され、記録コストだけでなくサンプリング
周期の消費自体も回避する（[`TracingLayer::record_response`][] の doc）。ヘルス
チェック等の高頻度パスをここに登録することが、記録粒度を「下げる」最も低コストな
第一の手段である。

- 照合セマンティクスはバイト単位の完全一致（`/health` と `/health/` は別パス扱い）。
  プレフィックス一致・glob は意図的に非対応（ログ抑制範囲の意図しない拡大＝
  可観測性の穴を防ぐ安全側の設計、`.claude/rules/security.md`）
- TASK-10.3（#58）で追加。TASK-10.4 の性能再検証（RPS 劣化 5% 以内）の前提となる
  緩和策の 1 つ

## 4. 記録粒度の切り替え方法（拡張指針）

### 4.1 既定の記録フィールド（不変条件）

[`TracingLayer::record_response`][] が記録するフィールドは **method・path・
elapsed_ms の 3 つに限定**する。ヘッダ値（`Authorization` / `Cookie` 等）・ボディ・
クエリ文字列は一切記録しない契約（`crates/plugin-tracing/src/lib.rs` セキュリティ節、
OWASP Top 10「可観測性」観点、`.claude/rules/security.md`）。

**この契約は記録粒度を上げる拡張であっても破ってはならない不変条件**とする。
粒度を上げる場合は次節の指針に従い、この 3 フィールドの外側（別イベント種別・
別レイヤ）として追加する。

### 4.2 記録粒度を上げる場合の拡張指針

現状 `TracingLayer` は応答時 1 イベントへ統合済み（TASK-10.2、#57 で受理・応答の
2 イベントから統合）。将来、より詳細な記録（リクエストボディサイズ・ルーティング
マッチ結果等の追加フィールド、または段階別の複数イベント）が必要になった場合の
拡張パスは以下の 2 通りを想定する。

1. **`TracingLayer` 自体にフィールドを追加**: `record_response` のシグネチャ・
   記録内容を拡張する。ただし 4.1 の不変条件（method/path/elapsed_ms 限定・PII/
   ヘッダ/ボディ非記録）は必ず維持し、新フィールドもこの契約に反しないことを
   実装前に確認する。既存の性能特性（サンプリング判定 → 除外照合 → 1 イベント
   記録という順序、TASK-10.4 で実測済みの RPS/p95 特性）を崩さないよう、追加
   フィールドのコスト（シリアライズ・アロケーション）を性能再検証（新規 NFR
   計測）で確認してから適用する
2. **外部レイヤの合成（`tracing_subscriber::registry` への移行）**: 現行実装は
   `tracing_subscriber::fmt()` を直接使う単一レイヤ構成（2 節参照）。利用者側が
   独自の `tracing_subscriber::Layer` 実装（例: OpenTelemetry エクスポータ・
   構造化ログの別フォーマッタ）を追加合成したい場合は、`init_tracing` を
   `registry().with(fmt_layer).with(custom_layer)` 形式に拡張する必要がある。
   これは `init_tracing` のシグネチャ変更（または新規関数追加）を伴うため、
   TASK-10.5 のスコープ外の実装変更となる。必要が生じた場合は
   `.claude/rules/out-of-scope-tracking.md` に従い Issue 化してから着手する

いずれの拡張も、`crates/plugin-tracing` が `crates/core` に依存しないという非循環
パターン（1 節）・`Middleware` 拡張点 1 本による配線（`crates/core/src/server.rs`
の `tracing` feature 限定）は変更しない前提とする。

## 5. 依存インパクトの要約

実測詳細は `docs/dep-impact/records.md`「2026-07-18 — `crates/plugin-tracing`
依存インパクト記録（#60、TASK-10.5）」エントリを参照。要約:

| 指標 | `tracing` feature 無効 | `tracing` feature 有効 | 増分 | PoC-10 実測 |
|---|---|---|---|---|
| 依存クレート数（union 展開） | 9 | 33 | +24 | +26 |
| release バイナリサイズ（`examples/minimal` vs `tracing_nfr`） | 799,144 bytes | 1,059,800 bytes | +32.6% | +57.6% |
| RSS（アイドル） | 2,980 KB | 7,312 KB | +145.4% | - |
| RSS（負荷時中央値） | 3,240 KB | 7,520 KB | +132.1% | +301.4% |

`tracing` feature 無効時は `fandhe-backend-plugin-tracing` / `tracing` / `tracing-subscriber` /
`tracing-appender` のいずれも `cargo tree -p fandhe-backend-core` に現れず、
pay-for-what-you-use の完全除外を満たす（`scripts/accept/tracing-accept.sh` A/D/E
チェックで機械検証、6 節参照）。

TASK-10.1〜10.3 の緩和策（サンプリング・イベント統合・高頻度パス除外）適用後は
PoC-10 実測ほどの相対増分（RSS +301.4%）にはならないことを本タスクで確認した
（RSS +132.1%〜+145.4%）。これは REQ-10 の DoS 耐性（記録コスト抑制）にも寄与する
（`.claude/rules/security.md`「リソース枯渇（DoS）」観点、記録コストが低いほど
高頻度アクセス時のリソース消費増分を抑えられる）。

## 6. TASK-10.5 受け入れ基準との対応

| 受け入れ基準 | 対応箇所 |
|---|---|
| `tracing` feature 有効時の依存クレート数・バイナリサイズ・RSS の増分を実測記録する | `docs/dep-impact/records.md` 該当エントリ（5 節に要約） |
| `tracing` feature 無効時に依存が一切現れないことを確認する | `docs/dep-impact/records.md` 該当エントリ + `scripts/accept/tracing-accept.sh` A（既存）/D/E（本タスクで追加） |
| `tracing` エコシステムとの連携方式（サンプリング設定・記録粒度の切り替え方法）を設計文書化する | 本文書 2〜4 節 |
| 受け入れテストスクリプトと実行結果を記録する | `scripts/accept/tracing-accept.sh`（D/E 追加）+ `docs/reports/task-10-5-acceptance.md` |

[`TracingConfig`]: ../../crates/plugin-tracing/src/config.rs
[`TracingConfig::exclude_path`]: ../../crates/plugin-tracing/src/config.rs
[`TracingLayer`]: ../../crates/plugin-tracing/src/layer.rs
[`TracingLayer::record_response`]: ../../crates/plugin-tracing/src/layer.rs
[`Sampler`]: ../../crates/plugin-tracing/src/sampler.rs
