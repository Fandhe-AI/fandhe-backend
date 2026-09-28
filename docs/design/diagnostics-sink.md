# ライブラリ内部診断シンク `Server::diagnostics` 設計判断

イシュー #720（`feat(core): ライブラリ内の eprintln! 出力を利用側で差し替え
られるようにする`）対応。`crates/core/src/diagnostics.rs` に実装した
[`Diagnostics`] trait・[`Server::diagnostics`] 登録口の設計判断・根拠・
将来案を記録する。API・実装の詳細は同ファイル・`crates/core/src/server.rs`
の doc comment を正とし、本書は「なぜその選択をしたか」を補足する。

## 1. 背景・課題

`fandhe-backend-core` はライブラリであるにもかかわらず、実行時診断 4 箇所
（`crates/core/src/server.rs`）で `eprintln!` により固定の日本語文言を
stderr へ直接出力していた。

| 箇所 | 発生条件 |
|------|---------|
| `BoundServer::run_until` の主 accept ループ | `listener.accept()` が失敗した（バックオフ後に再試行） |
| `configure_accepted_stream` | accept 直後のソケットへの TCP_NODELAY 設定が失敗した（フェイルオープンで継続） |
| `BoundServer::run_until` の最終 graceful shutdown | in-flight 完了待ちが `shutdown_grace_period` を超過した |
| `spawn_generation_drain`（rebind 旧世代 drain） | 旧世代接続の drain が `shutdown_grace_period` を超過した |

利用側（CLI・ログ集約基盤等）がこの出力先・書式・抑止を一切制御できないのは
以下の点で望ましくない:

- **pay-for-what-you-use / AI ファースト保守性**: フレームワーク利用者が
  自前のログ基盤（`tracing` 等）へ統一したくても、コアの出力だけが例外的に
  stderr へ漏れる
- **可観測性**（`.claude/rules/security.md`）: 抑止も転送もできないため、
  運用上不要なノイズを除去できない

## 2. 比較した 2 案

Issue は 2 案を提示していた。

### 案 A: `tracing` / `log` へコアが直接依存する feature を追加する

コアに `tracing` feature を新設し、有効時は `tracing::warn!` 等で出力する
（無効時は現状の `eprintln!` を維持）。

- **不採用理由**: コアの `tracing` feature は既に
  `Server::tracing(config)` → `crates/plugin-tracing` への配線専用
  （`Middleware` 拡張点経由のプラグイン境界パターン）として確立している。
  コア本体が `tracing` crate へ直接依存する構成を同じ feature 名の下に
  混在させると、「`tracing` feature = プラグイン配線」という既存の意味論が
  壊れ、pay-for-what-you-use の境界（プラグインは `crates/plugin-*` に
  閉じる）とも整合しない。新しい feature 名を割り当てる案も検討したが、
  「新規外部依存を増やさず解決できる」案 B が優先される。

### 案 B（採用）: 利用側が実装する診断シンクの登録口を追加する

[`Diagnostics`] trait（`crate::extension` の 3 拡張点・`Interceptor` と同型の
「外部依存ゼロの純コア機能」）を新設し、[`Server::diagnostics`] で登録する。
未登録時（既定）は [`StderrDiagnostics`] が現行の `eprintln!` 出力と完全
互換の文言・接頭辞・出力先を維持する。

- **新規依存ゼロ**: `tracing` crate 等への直接依存を増やさない
- **利用側が案 A も実現できる**: シンク実装内で `tracing::warn!` へ転送すれば
  事実上 `tracing` 連携も成立する（本書 6 節の将来案を参照）。案 A の
  スーパーセットとして機能する
- **`Interceptor`（イシュー #420）と同じ位置づけ**: 3 拡張点で表現できない
  ユースケースに対し、feature ゲート不要の追加シームを設ける前例に倣う

両案を比較し、**案 B を採用**した。

## 3. 公開 API

```rust,ignore
pub trait Diagnostics: Send + Sync + 'static {
    fn report(&self, event: &DiagnosticEvent<'_>);
}

#[non_exhaustive]
pub enum DiagnosticEvent<'a> {
    AcceptFailed { error: &'a io::Error },
    TcpNodelayFailed { error: &'a io::Error },
    ShutdownGraceExceeded { grace: Duration },
    RebindDrainGraceExceeded { grace: Duration },
}

impl fmt::Display for DiagnosticEvent<'_> { /* 接頭辞なしの本文 */ }

pub struct StderrDiagnostics; // 既定シンク、`Diagnostics` を実装

impl Server {
    pub fn diagnostics(mut self, sink: impl Diagnostics) -> Self;
}
```

- `Diagnostics` は同期 trait（dyn 互換性のため、3 拡張点と同じ設計判断）
- `DiagnosticEvent` は `#[non_exhaustive]` とし、将来イベントを追加しても
  breaking change にしない
- `impl<F: Fn(&DiagnosticEvent<'_>) + Send + Sync + 'static> Diagnostics for F`
  の blanket impl により、クロージャをそのまま登録できる
  （`Server::new().diagnostics(|_event: &DiagnosticEvent<'_>| {})` で出力
  抑止が 1 行で書ける。引数の型注釈は必須で、`|_| {}` のみでは HRTB が絡み
  型推論に失敗しコンパイルが通らない）
- `Server` は `Arc<dyn Diagnostics>` としてシンクを保持する。`Box` ではなく
  `Arc` にしたのは、`spawn_generation_drain`（detached `tokio::spawn` タスク）
  へ `Server` 全体ではなくシンクだけを安価に `clone` して渡すため

## 4. 接頭辞の扱い（既定シンクのみが付与する）

`DiagnosticEvent::Display` は現行 `eprintln!` 引数から接頭辞
（`fandhe_backend_core::server: `）を除いた本文をそのまま返す。接頭辞は
`StderrDiagnostics::report` 内部でのみ付与する。これにより:

- 独自シンクへは「ライブラリ内部のどのコンポーネントの診断か」を利用者が
  自分のログ書式（`tracing` のターゲット名等）で表現できる
- 既定シンクの出力は現行 `eprintln!` とバイト単位で一致する
  （`crates/core/tests/diagnostics.rs` の子プロセス方式で検証、本書 7 節）

## 5. panic 境界（多層防御、フェイルクローズしない前提を明記）

`Diagnostics::report` は「panic しない」契約だが、利用者実装の誤りに備え、
非公開ヘルパ `emit` が `std::panic::catch_unwind` で `report` 呼び出しを包む
（`.claude/rules/coding-rust.md` 「panic はライブラリ境界を越えさせない」）。

- accept ループ（`BoundServer::run_until`）や rebind の背景 drain タスク
  （`spawn_generation_drain`）へ panic を伝播させない
- **`panic = "abort"` ビルドでは `catch_unwind` が捕捉できない**ため、本防御は
  `unwind` パニック戦略限定の多層防御であり、`Diagnostics::report` の
  「panic しない」契約を代替しない（trait doc に明記）

## 6. 通知順序の変更（grace 超過 2 箇所、既定互換に影響しない挙動変更）

最終 graceful shutdown・rebind 旧世代 drain の grace 超過強制クローズ 2 箇所
は、従来「`eprintln!` → `JoinSet::shutdown().await`」の順だったが、本変更で
「`JoinSet::shutdown().await` → 診断シンクへの通知」の順に入れ替えた。

- **理由**: 強制クローズ（フェイルクローズ、有界時間でのクローズ）の完了を
  確定させてから通知することで、利用側シンクの異常・遅延（`emit` の
  `catch_unwind` で panic は防げても、ブロッキング I/O 等の遅延までは防げない）
  が強制クローズの確定を妨げないようにする
- **既定シンクへの影響**: `StderrDiagnostics::report` は同期の
  `eprintln!` 1 行のみであり、**stderr が詰まらない限り**実質的な遅延がなく、
  出力順序が反転しても外部から観測可能な違い（stderr の 1 行が出るタイミングが
  強制クローズ完了の直後へわずかに後ろへずれる）はテストで区別できない程度に
  留まる（`docs/design/graceful-shutdown.md` 3 節・`CHANGELOG.md` に記載）。
  なお本節の通知順序の入れ替えにより、grace 超過 2 箇所は stderr が詰まって
  `eprintln!` がブロックしても**強制クローズ自体の完了は妨げられない**
  （遅延は診断ログ 1 行の出力タイミングに限られる）。7 節も参照

## 7. 既定シンクとブロッキング I/O 禁止契約の関係（PR #748 レビュー対応）

[`Diagnostics::report`] は「ブロッキング I/O を行わない」契約だが（3 節・
`crates/core/src/diagnostics.rs` モジュール doc）、この契約は
[`Server::diagnostics`] で**利用者が登録するシンク実装**に対するものであり、
既定シンク [`StderrDiagnostics`] は 1 節の背景（現行 `eprintln!` 出力との
完全互換を最優先）により、この契約の**意図的な例外**として同期 `eprintln!`
をそのまま使う。

- **なぜ `Middleware`（`AGENTS.md`「規約: ミドルウェア非同期 I/O 必須化」）と
  同列に扱わないか**: 同規約の根拠である PoC-3・PoC-10 実測は「全リクエストに
  介入するミドルウェアが同期 I/O を行うと RPS が著しく劣化する」という
  per-request ホットパスの問題である。`DiagnosticEvent` の 4 種
  （`AcceptFailed` / `TcpNodelayFailed` / `ShutdownGraceExceeded` /
  `RebindDrainGraceExceeded`）はいずれも accept 失敗・grace 超過等の低頻度な
  エラー・シャットダウン経路限定のイベントであり、per-request では発火しない
  ため PoC-3/10 の性能劣化根拠はそのまま適用されない。問題になりうるのは
  スループットではなく、**stderr が詰まった場合にブロックしうる**という点
  （後述）
- **前提とする配置**: stderr が端末・ファイル、または受信側が生きている
  パイプであること（一般的な運用環境）
- **前提が崩れた場合の影響**（イベントごと。`crates/core/src/server.rs` の
  呼び出し位置に基づく）:
  - `AcceptFailed`: 主 accept ループでバックオフ前に呼ばれるため、
    ブロックすると次回 accept 再試行が遅延する
  - `TcpNodelayFailed`: 該当 1 接続の accept 処理が遅延する
    （フェイルオープン方針自体は不変）
  - `ShutdownGraceExceeded` / `RebindDrainGraceExceeded`: 6 節の通知順序
    変更により**強制クローズの完了を確定させた後**に呼ばれるため、
    ブロックしても強制クローズ自体の完了は妨げられない。遅延は診断ログ
    1 行の出力タイミングに限られる
- **緩和策**: 上記の影響を許容できない場合、利用者は [`Server::diagnostics`]
  で非ブロッキングな独自シンク（有界チャネルへ `try_send` し、別スレッド・
  タスクが実際の書き込みを行う。`plugin-tracing` が使う
  `tracing-appender` の non-blocking writer へ転送する実装も同様に有効）を
  登録できる
- **再検討トリガ**: 将来 `DiagnosticEvent` に per-request 相当の高頻度
  イベントが追加される場合、または実運用で stderr 詰まりによる停止が
  観測された場合は、既定シンクの非ブロッキング化（10 節）を再検討する

この整理はドキュメント（doc comment・本書）の明確化のみであり、既定シンクの
実装・挙動（現行 `eprintln!` 出力との完全互換）は変更しない。

## 8. テスト方針

- **単体テスト**（`crates/core/src/diagnostics.rs`）: 4 種イベントの
  `Display` が現行 `eprintln!` 引数と完全一致すること・`StderrDiagnostics`
  の整形結果（接頭辞付き）・panic するシンクを `emit` へ渡しても呼び出し元へ
  伝播しないこと
- **統合テスト**（`crates/core/tests/diagnostics.rs`）:
  - 最終 graceful shutdown・rebind 旧世代 drain それぞれの grace 超過シナリオ
    （`graceful_shutdown.rs` / `rebind.rs` の既存シナリオを再利用）で、
    登録した記録シンクへ対応するイベントが届くこと
  - **stderr の実バイト列検証は子プロセス方式で行う**: libtest の出力捕捉は
    スレッドローカルであり、tokio ワーカー・detached タスク（`tokio::spawn`）
    からの `eprintln!` を捕捉できないため、in-process のアサーションでは
    「出力されないこと」を検証できない。`std::process::Command::new(
    std::env::current_exe())` で自分自身を `--exact <test> --nocapture
    --test-threads=1` 指定・環境変数付きで再起動し、子プロセスの stderr を
    親プロセス側で読み取って判定する。独自シンク（出力抑止クロージャ）登録時に
    既定の接頭辞付き出力が **一切含まれない**ことと、未登録時に既定文言が
    含まれることの両方を確認する（陰性・陽性対照のペア）

## 9. `ShutdownGraceExceeded` 通知の detached 化（PR #748 レビュー P1 対応）

6 節・7 節は「`join_set.shutdown().await` → 通知」の順に入れ替えたことで
**強制クローズ自体の完了**は利用側シンクの遅延に妨げられないと説明したが、
最終 graceful shutdown の `run_until` 側（`ShutdownGraceExceeded`）は通知
（`crate::diagnostics::emit`）自体を `run_until` の返却経路上で**同期的に**
呼んでいたため、契約違反のシンク（`report` がブロッキング I/O を行う、
または停止する）が登録されていた場合、`emit` の呼び出しが完了するまで
`run_until` 自体が返らない可能性が残っていた。これは
`docs/design/graceful-shutdown.md`・`docs/design/rebind.md` が明記する
「`shutdown_grace_period + ε` 以内に必ず戻る」という `run_until` 自体の
公開契約に抵触しうる（`catch_unwind` は panic のみ捕捉し、ブロッキング・
ハングは防げない）。

- **対応（初版）**: `ShutdownGraceExceeded` の通知を `tokio::spawn` した
  detached タスクへ切り離し、`run_until` はこのタスクの完了を待たずに
  `Ok(())` を返す。強制クローズ自体（`join_set.shutdown().await`）は従来
  どおり `run_until` 側で同期的に完了を確定させてから通知タスクを起動する
  ため、6 節が述べた「強制クローズの完了は妨げられない」という性質は不変
- **対応（訂正、PR #748 Bugbot 指摘対応）**: 上記の `tokio::spawn` には
  別の欠落があった。detached タスクは tokio ランタイムが以後もそれを
  ポーリングして初めて実行されるが、`run_until` を `current_thread`
  ランタイムの `block_on` へ渡す**最後の await**として呼び出す典型的な
  使い方（公式 `graceful_shutdown` サンプルを含む）では、`run_until` が
  `Ok(())` を返した時点で `block_on` 自体が完了し即座に戻るため、以後
  ランタイムは何もポーリングしない。この場合 detached タスクは 1 度も
  実行されずに破棄され、通知が確実に失われる（regression テスト
  `crates/core/tests/diagnostics.rs::
  shutdown_grace_exceeded_notified_even_when_run_until_is_last_await_on_current_thread_runtime`
  で再現・検証済み）。`emit` は同期関数で `.await` 点を持たないため tokio
  タスクとして実行する必要はなく、切り離し先を `tokio::spawn` ではなく
  `std::thread::spawn`（OS スレッド）へ変更した。OS スレッドは tokio
  ランタイムの継続ポーリングに一切依存せず独立に実行されるため、上記の
  欠落を避けられる（実行完了を待たない fire-and-forget である点・強制
  クローズの完了は妨げられないという性質は不変）
- **`RebindDrainGraceExceeded` は元から対象外**: rebind 旧世代 drain の
  通知（`spawn_generation_drain` 内の `emit` 呼び出し）は、
  `spawn_generation_drain` 関数自体が呼び出し時点で `tokio::spawn` して
  返る設計（`run_until` の `Raced::Rebind` 分岐はこの spawn 呼び出しを
  待機しない）のため、今回の変更前から既に `run_until` の返却経路とは
  独立しており、同種の問題を抱えていなかった
- **既定シンクへの影響なし**: `StderrDiagnostics::report` は同期
  `eprintln!` 1 行のみで、detached タスクの中で呼ばれても文言・接頭辞・
  出力先は不変。`crates/core/tests/diagnostics.rs` の子プロセス方式による
  検証（8 節）は、通知が detached タスクから行われる前提を既に踏まえている
  ため追加変更は不要

## 10. スコープ外・将来案

- **accept 失敗・TCP_NODELAY 失敗のエンドツーエンド統合テストは追加しない**:
  `EMFILE` 等の環境依存条件でしか再現できないため、単体テスト（`Display` の
  文言一致）でのみカバーする
- **`tracing` feature 有効時に使える tracing 転送シンクの同梱**: 利用者が
  自分でシンクを書けば実現できるため（本書 2 節）、今回のスコープには
  含めない。需要が具体化した時点で別イシューとして起票を検討する
  （`.claude/rules/out-of-scope-tracking.md`）
- **既定シンクの非ブロッキング化**: `StderrDiagnostics` を有界チャネル +
  背景スレッドで書き込む非ブロッキング実装へ置き換える案。7 節の
  再検討トリガに該当する事情が生じるまでは、複雑さ（背景スレッドの
  生存期間管理、プロセス終了時に未フラッシュのメッセージが失われうる点、
  `crates/core/tests/diagnostics.rs` の子プロセス方式によるバイト単位
  一致検証への影響）に見合わないため見送る。需要が具体化した時点で
  別イシューとして起票を検討する（`.claude/rules/out-of-scope-tracking.md`）
- `gen-openapi` CLI（`crates/plugin-openapi/src/bin/gen-openapi.rs`）の
  `eprintln!` は CLI バイナリとして正当な出力であり、本設計の対象外
