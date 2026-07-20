# graceful shutdown（`BoundServer::run_until`）設計判断

イシュー #313（`feat(core): graceful shutdown を実装する`）対応。
`crates/core/src/server.rs` に実装した `BoundServer::run_until` の設計判断・
根拠・既知の限界を記録する。API・実装の詳細は同ファイルの doc comment を
正とし、本書は「なぜその選択をしたか」を補足する。

## 1. 背景・受け入れ条件

従来の `BoundServer::run()` は無限 accept ループであり、停止手段がプロセス
kill しかなかった。デプロイ更新のたびに処理中（in-flight）のリクエスト・
接続が強制切断される問題があった。

イシュー #313 の受け入れ条件（4 項目）:

1. 既存 `run()` の後方互換を維持（新 API の追加とする）
2. in-flight 完了待ちに上限時間を設け、超過時は強制クローズ（フェイルクローズ）
3. シグナル受信後に新規接続を受け付けないことを統合テストで担保
4. tokio への追加 feature 依存が最小（pay-for-what-you-use）

## 2. 公開 API

```rust,ignore
impl Server {
    pub fn shutdown_grace_period(mut self, grace: Duration) -> Self;
}

impl BoundServer {
    pub async fn run_until<F>(self, shutdown: F) -> io::Result<()>
    where
        F: Future<Output = ()>;
}
```

`run()` は `run_until(std::future::pending::<()>()).await` へ委譲する薄い
ラッパーへ置き換えた。挙動・シグネチャとも完全な後方互換を維持する
（受け入れ条件 1）。既存の `run()` 利用箇所（examples・統合テスト）は無変更
のまま動作する。

シグナル源（Ctrl-C 等）はコアで扱わない。利用者が任意の `Future` を
`shutdown` として渡す設計とし、`tokio::signal` feature をコアの
`[dependencies]` に追加しない（受け入れ条件 4、下記 4 節）。利用例は
`crates/core/examples/graceful_shutdown.rs` を参照。

## 3. shutdown シーケンス

1. **accept 停止**: shutdown Future 完了を検知したら、shutdown フラグ
   （`Arc<AtomicBool>`）を立て、リスニングソケットを明示的に `drop` する。
   以降の新規接続は OS レベルで拒否される（受け入れ条件 3）
2. **in-flight 完了待ち**: `Server::shutdown_grace_period`（既定
   `DEFAULT_SHUTDOWN_GRACE_PERIOD` = 30 秒）を上限に、`connection_limit`
   セマフォの全 permit（`permit_total` 個）が解放されるのを待つ
3. **上限超過時は強制クローズ**: 上限内に全 permit が解放されなければ、
   警告ログを 1 行出した上で残存コネクションタスクを `JoinSet::shutdown`
   で abort する（受け入れ条件 2）

どちらの経路でも `run_until` は `shutdown_grace_period` + ε 以内に必ず
`Ok(())` で戻る。

## 4. `tokio::select!` を使わない理由（`macros` feature 非追加）

`tokio::select!` は `macros` feature（proc-macro 系推移依存を伴う）を要求
する。`crates/core/Cargo.toml` の tokio feature は `rt` / `net` / `io-util` /
`time` / `sync` の 5 つに限定する既存規約（pay-for-what-you-use）があり、
本イシューでもこれを増やさない方針とした。

代わりに `std::future::poll_fn` + `std::pin::pin!` で「shutdown Future と
accept Future を競合させる」私有ヘルパー（`race_shutdown_or_accept`、
`crates/core/src/server.rs`）を実装した。shutdown Future はループの外で
1 度だけ pin し、反復をまたいで poll し続ける（各反復で pin し直すと
Future の内部状態が失われる）。accept 側の Future は反復ごとに新規生成し、
cancel-safe（shutdown 側が先に完了して drop されても、取得済み permit は
自動解放されるだけで接続を取りこぼさない）。

`race_shutdown_or_accept` は shutdown を先にポーリングする実装とし、
shutdown・accept が同一 poll で同時に Ready になりうる場合でも
「shutdown 優先」を保証する（shutdown 直後に新規接続を受理してしまう
競合を避ける）。

## 5. in-flight 完了待ちを「セマフォの全 permit 回収」で実現する理由

`bind()` で確定する同時接続数上限（`max_connections.max(1)`）を
`permit_total: u32` として `BoundServer` に保持する。shutdown 後は
`tokio::time::timeout(grace, connection_limit.acquire_many_owned(permit_total))`
で待つ。

`permit_total` は `Semaphore::new` に渡す値と**同一の式**（`bind()` 内で
1 回だけ計算）から導出しており、二重計算による乖離を防いでいる。値が
異なると次のいずれかの不整合が生じる:

- `permit_total` が実際より小さい → drain が早期に完了したと誤判定し、
  in-flight 接続を残したまま次のフェーズへ進んでしまう
- `permit_total` が実際より大きい → drain が絶対に完了せず、常に grace
  タイムアウトへ落ちる

WebSocket 委譲で専用タスクへ move された permit も同じセマフォで解放
されるため、WS セッションを含む全 in-flight を漏れなく待てる
（`crate::plugin::try_handle_upgrade` の doc「permit の契約」を参照）。

## 6. 強制クローズを `tokio::task::JoinSet` で実現する理由

accept ループはコネクションタスクを `JoinSet`（`rt` feature のみで利用可、
新規依存なし）へ spawn する。各反復で `while join_set.try_join_next().is_some() {}`
により完了済みタスクを全件回収する（1 件のみの回収だと accept 待ちが続く
間に完了タスクが溜まり続けるため）。

grace 超過時は `join_set.shutdown().await` で残タスクを abort する。abort
により保持中の `TcpStream` が drop され、ソケットは即時クローズされる
（half-open 残留・fd リークなし）。

### 6.1 `run_until` 自体の外部キャンセルからの保護（PR #336 レビュー是正）

`JoinSet::drop` は保持中の未完了タスクを全 abort する。`run_until` が返す
`Future` 自体が呼び出し側の `tokio::select!` 等で外部キャンセルされ
Future が drop されるケース（一般的な shutdown パターン）では、素の
`JoinSet` だと accept 済みの in-flight 接続まで即座に abort されてしまい、
「`run()` の cancel は accept 停止のみで処理中のリクエストは継続する」と
いう従来（detached `tokio::spawn` 時代）の挙動から退行する（Bugbot 指摘、
review comment 3615287445, PR #336）。

`CancelSafeJoinSet`（`server.rs` 内のラッパー型）で `JoinSet` を包み、
`Drop` を `abort_all` ではなく `detach_all`（タスクを追跡から外すだけで
abort しない）に差し替えて是正した。内部の grace 超過時強制クローズは
`join_set.shutdown().await` を明示的に呼ぶ経路であり `Drop` を経由しない
ため、この変更後も強制クローズの挙動自体は変わらない。

## 7. keep-alive 接続の早期クローズ

shutdown シグナル受信を `Arc<AtomicBool>` で `handle_connection_with_permit`
へ伝える。既存の keep-alive 判定（`should_keep_alive(&request.head) && ...`）
に `&& !shutdown_flag` を加えることで、処理中のリクエストは完走させつつ
応答に `Connection: close` を付与して接続を閉じる既存機構へ自然に合流する。

`keep_alive` はリクエスト受信直後（`on_request` 直後）に一度算出するが、
`try_intercept` / `Handler::handle` の呼び出しが非同期に長引く間に
shutdown が入ると、算出時点の値のまま古い判定で応答してしまう
（`max_connection_lifetime` の再チェックと同型の問題）。これを防ぐため、
応答送信直前にも `!shutdown_flag` を再チェックし、`Connection: close` を
確実に付与する（Bugbot 指摘、review comment 3615144800, PR #336
"Stale keep-alive after shutdown" の是正）。

公開 API の `handle_connection(&Server, S)` はシグネチャ不変（内部で
「シャットダウンなし」の固定 `false` フラグを渡す）。`pub(crate)` の
`handle_connection_with_permit` のみ引数を追加した。

### 7.1 Upgrade 分岐への shutdown_flag 適用（PR #336 レビュー是正）

`shutdown_flag` は元々 HTTP の keep-alive 判定にしか影響せず、Upgrade
（WebSocket 等）分岐はこれを一切参照していなかった。shutdown 後に
Upgrade を許すと、その permit は `crate::plugin::try_handle_upgrade`
内で `JoinSet` 外の detached セッションタスクへ move され、grace
force-close を過ぎても動き続けうる（Bugbot 指摘、review comment
3615144815, PR #336 "Upgrade ignores shutdown flag" の是正）。

`UpgradeHandler` がマッチした直後・`try_handle_upgrade` 呼び出し前に
`shutdown_flag` をチェックし、`true` なら 503 で明示的に拒否して
`Connection: close` で接続を閉じるよう是正した（shutdown_flag 受信前に
既に委譲済みのセッションは対象外、8 節参照）。

## 8. 既知の限界・スコープ外

- **アイドル keep-alive 接続への即時 wakeup**: read 待ち中のアイドル
  keep-alive 接続はこのフラグに即応しない（次の read タイムアウト、または
  grace 超過の強制クローズで確実に閉じる）。`tokio::sync::Notify` 等による
  即時中断は本イシューのスコープ外
- **WebSocket 専用タスクへのキャンセル伝播**: shutdown_flag 受信前に
  既に Upgrade へ委譲済みの WebSocket 専用タスク（`fandhe_backend_plugin_websocket`
  側の `tokio::spawn`）は `run_until` が管理する `JoinSet` の外にあるため、
  grace 超過時の強制 abort 対象にはならない（shutdown_flag 受信後の
  「新規」Upgrade は 7.1 節の是正で 503 拒否されるようになったが、既存の
  委譲済みセッションには遡及しない）。ただし in-flight 完了待ちは permit
  回収のタイムアウトで実装されており、WS セッションが permit を握った
  まま生き続けても `run_until` 自体は grace + ε 以内に必ず戻る
  （`BoundServer::run_until` の doc「既知の限界」を参照）
- **OS シグナル（SIGTERM/SIGINT）ヘルパーのコア提供**: 現状は利用者側で
  Future を用意する設計とし、コアはシグナルハンドラを持たない（4 節）

いずれも [[out-of-scope-tracking]] に従い、Issue 起票はユーザー承認を得て
から行う。

## 9. 検証

```bash
cargo build -p fandhe-backend-core                  # feature なし
cargo build -p fandhe-backend-core --all-features   # 全 feature
cargo test -p fandhe-backend-core --test graceful_shutdown
cargo test -p fandhe-backend-core --all-features
cargo tree -p fandhe-backend-core -e normal          # tokio feature が 5 つのまま
cargo clippy -p fandhe-backend-core --all-targets --all-features -- -D warnings
```
