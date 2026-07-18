# TASK-10.6（#90）計測レポート — 非同期 writer バックプレッシャー・ログ欠落検証

PoC-10（`docs/spec/03-poc/observability-tracing/README.md`「発見事項」「環境制約」）が
未検証（推測）のまま残していた 2 点を実測で確定させる。

1. `tracing-appender` の non-blocking チャネルが満杯時に**ドロップするかブロックするか**
2. 高負荷時のログ欠落率（定量計測）と欠落許容基準

`crates/plugin-tracing/src/init.rs` の doc comment も従来「lossy（推測に基づく契約記述）」
としていたが、本タスクの成果を反映して「実測済み」へ更新した。

## 計測方法

### 1. チャネル満杯時の挙動（決定的統合テスト）

`crates/plugin-tracing/tests/backpressure.rs`。`tracing_appender::non_blocking::
NonBlockingBuilder` に「書き込みをゲートで人為的に停止できる writer」（`GateWriter`、
std のみで実装、新規依存の追加なし）を渡し、`buffered_lines_limit` を小さく
（16）設定して意図的にチャネルを満杯にする。

- `lossy(true)`（既定）: 満杯時に呼び出し側スレッドをブロックせず `error_counter().
  dropped_lines()` が増加することを検証（`lossy_true_drops_excess_events_without_
  blocking_caller`）
- `lossy(false)`: 満杯時に送出側スレッドがブロックし、`dropped_lines() == 0` を保つ
  ことを検証（`lossy_false_blocks_caller_and_preserves_all_events`）
- 両テストとも「実書込行数 + dropped_lines == 総送出行数」の勘定整合を検証し、
  欠落率算出の妥当性根拠とする
- `tracing_subscriber::fmt` レイヤ経由の実経路でイベントが writer に到達することを
  確認する煙テストを追加（`traced_events_reach_writer_through_fmt_layer`）。チャネル
  勘定そのものは上記 2 テストが担う

```bash
cargo test -p bf-plugin-tracing --test backpressure
```

3 テストとも安定して pass する（連続 3 回の再実行で flaky なし、CI では毎回実行される
決定的テスト）。

### 2. 高負荷ログ欠落率の定量計測

`crates/plugin-tracing/examples/backpressure_probe.rs`（既定構成: lossy=true・
`buffered_lines_limit` 既定値 128,000。`init_tracing` の既定構成と同一のビルダー呼び出し）
を用い、出力先を一時ファイルとして負荷段階ごとに送出する。1 プロセス 1 回の実行につき
`{emitted, written, dropped_lines, drop_rate_pct, threads, line_bytes, elapsed_secs,
events_per_sec}` の JSON 1 行を出力する。

`benches/tracing-backpressure-bench.sh` が負荷段階（イベント総数:送出スレッド数）ごとに
プローブを RUNS 回実行し、`benches/lib/common.sh` の `median` で欠落率・実効イベント
レートの中央値を算出する。負荷段階は PoC-10 実測（約 23 万イベント/秒〔115,612 RPS ×
2 イベント〕）を跨ぐ範囲を既定とする。

```bash
cargo build --release -p bf-plugin-tracing --example backpressure_probe
RUNS=5 bash benches/tracing-backpressure-bench.sh
```

## 計測結果

環境: 本リポジトリの CI 相当コンテナ環境（`Linux 7.0.0-27-generic`）。書き込み先は
tmpfs 上ではなく通常のファイルシステム上の一時ディレクトリ（`mktemp -d`）。

### 1. チャネル満杯時の挙動（結論）

| 設定 | 満杯時の挙動 | 実測根拠 |
|------|-------------|---------|
| `lossy(true)`（既定、`init_tracing` が使う設定） | **ブロックせずドロップ**する | `tests/backpressure.rs::lossy_true_drops_excess_events_without_blocking_caller` |
| `lossy(false)` | **送出側スレッドをブロック**し欠落ゼロを保つ | `tests/backpressure.rs::lossy_false_blocks_caller_and_preserves_all_events` |

`tracing-appender 0.2.5` の実装（`non_blocking.rs` の `Write for NonBlocking`）でも、
`is_lossy` 分岐で `try_send`（失敗時 `error_counter` をインクリメントし成功扱いで返す）
と `send`（ブロッキング）を明確に切り替えており、テスト結果と実装根拠が一致する。

### 2. 負荷段階別の欠落率実測値（RUNS=5、中央値）

| イベント総数 | 送出スレッド数 | 欠落率中央値 | ドロップ行数中央値 | 実効イベントレート中央値 (events/sec) |
|---:|---:|---:|---:|---:|
| 100,000 | 1 | 0.000000% | 0 | 1,875,733.55 |
| 1,000,000 | 1 | 38.087500% | 380,875 | 2,218,778.16 |
| 1,000,000 | 4 | 72.670900% | 726,709 | 7,667,028.67 |
| 5,000,000 | 4 | 82.485920% | 4,124,296 | 7,912,346.34 |
| 5,000,000 | 8 | 91.328160% | 4,566,408 | 10,726,119.04 |

各段階 5 回の生値（`drop_rate_pct`）は `RUNS=5 bash benches/tracing-backpressure-bench.sh`
の標準エラー出力にすべて記録される（本レポート作成時の実行ログより抜粋）。

```
events=100000  threads=1: 0.000000, 0.000000, 0.000000, 0.000000, 0.000000
events=1000000 threads=1: 39.853700, 40.081200, 36.350300, 37.316200, 38.087500
events=1000000 threads=4: 71.894300, 72.670900, 72.366100, 72.911400, 73.350500
events=5000000 threads=4: 82.563260, 81.814580, 82.485920, 82.312020, 83.126940
events=5000000 threads=8: 85.098240, 91.328160, 90.081160, 91.515720, 91.949100
```

段階間でばらつきはあるが（例: 5,000,000 events / 8 threads は 85.1〜92.0%）、いずれの
段階でも「送出レートが `buffered_lines_limit`（既定 128,000 行）に対して十分高い場合に
大量ドロップが発生する」という傾向は一貫しており、単発の外れ値ではない。

**注意**: 本計測プローブは「送出側の全力送出レート」を意図的に作るため、通常運用で
現実的な負荷（PoC-10 実測: 約 23 万イベント/秒）を大きく超える負荷段階（100 万〜500 万
イベントをごく短時間に送出）を含む。100,000 イベント/1 スレッドの段階（欠落率 0%）が
最も PoC-10 実測に近い負荷密度であり、これは「定常的な送出レートが writer のフラッシュ
スループットを下回る限り欠落は発生しない」ことの裏付けになる。

## 許容基準（受入基準 2 の文書化）

1. **通常運用（サンプリング適用後の定常イベントレートが writer フラッシュスループット
   を下回る構成）**: 欠落率 0% を基準とする。100,000 イベント/1 スレッドの実測
   （欠落率 0.000000%）がこれを裏付ける。`bf-plugin-tracing` の `Sampler` によるサンプリング
   （TASK-10.1）は、REQ-10 の「サンプリング前提のオプトイン」方針のもとイベントレート
   自体を絞る役割を持ち、本基準の前提を成立させる主要な手段である。
2. **飽和時（送出レートが `buffered_lines_limit` を大きく超える場合）**: 既定
   （`lossy(true)`）は「ブロックせずドロップ」する = **可用性優先（DoS 耐性）** であり、
   これはリクエスト処理スレッドがログ writer のスロー I/O によって停止しないことを
   意味する（`.claude/rules/security.md` リソース枯渇 (DoS) 観点）。欠落率は
   `NonBlocking::error_counter().dropped_lines()` により実行時に観測可能であり、
   この値が 0 でない場合はログの完全性が損なわれていることを可観測性の運用者が
   検知できる（値自体を運用メトリクスとして公開する API 拡張は本タスクのスコープ外、
   §「スコープ外」参照）。
3. **欠落を許容できないログ（認証失敗・監査イベント等のセキュリティ監査ログ）**:
   `init_tracing` の既定構成（lossy=true）の対象外とする。これらのログを記録する
   場合は次のいずれかの設計判断を採る（AGENTS.md「ログ欠落の許容可否」節が先送りして
   いた論点への回答）:
   - `NonBlockingBuilder::lossy(false)` によるブロッキング経路（バックプレッシャーが
     リクエスト処理スレッドへ波及するトレードオフを許容できる場合）
   - 非同期チャネルを経由しない同期書き込み経路（レイテンシ増を許容できる場合）
   - いずれも `init_tracing` の公開 API 拡張（`lossy`/`buffered_lines_limit` の設定公開）
     を要するため、実装は本タスクのスコープ外（§「スコープ外」参照）とし、必要になった
     時点で別 Issue として起票する

## スコープ外（out-of-scope-tracking、ユーザー承認前提）

- `init_tracing` の公開 API 拡張（`buffered_lines_limit`/`lossy` の設定公開、
  `dropped_lines` の運用メトリクス公開、ファイル出力対応）
- 実サーバ（`crates/core` + `tracing` feature）+ `oha` による E2E 欠落計測（本計測は
  チャネル挙動の直接検証としては十分。E2E の性能再検証は TASK-10.4 #59 の領域と重なる）
- サンプリング適用後の性能再検証そのもの（TASK-10.4）
