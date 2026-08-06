# graceful shutdown ガイド

fandhe-backend は `BoundServer::run_until(shutdown)` により graceful shutdown を
提供する。シャットダウンシグナルを受けると新規接続の受理を
止め、処理中（in-flight）のリクエスト・接続の完了を上限時間まで待ってから
終了する。デプロイ更新のたびに in-flight リクエストが強制切断される問題への
対処である。API は `crates/core/src/server.rs`、設計判断の記録は
[`docs/design/graceful-shutdown.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/graceful-shutdown.md) を参照する。

## 公開 API

| API | 役割 |
|-----|------|
| `BoundServer::run_until(shutdown)` | `shutdown` Future が完了するまで accept ループを回し、完了後に graceful shutdown シーケンスを実行する |
| `Server::shutdown_grace_period(grace)` | in-flight 完了待ちの上限時間を設定する（既定 30 秒） |
| `BoundServer::run()` | 従来 API。`run_until(std::future::pending::<()>())` への薄い委譲となり、挙動・シグネチャとも後方互換を維持する |

`shutdown` は `Future<Output = ()>` であれば何でもよい。シグナル源（Ctrl-C・
SIGTERM・管理エンドポイント等）はコアで扱わず、利用者が任意の Future として
渡す設計である（`tokio` の `signal` feature をコアの依存に持ち込まないための
pay-for-what-you-use。[`.claude/rules/pay-for-what-you-use.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/pay-for-what-you-use.md)）。

## shutdown シーケンス

`shutdown` Future が完了すると、次の順序で処理する。

1. **accept 停止**: shutdown フラグを立て、リスニングソケットを明示的に drop
   する。以降の新規接続は OS レベルで拒否される
2. **in-flight 完了待ち**: `Server::shutdown_grace_period`（既定 30 秒）を上限に、
   全 in-flight 接続の完了を待つ。処理中のリクエストは完走させつつ、以後の応答には
   `Connection: close` を付けて keep-alive 接続も早期に閉じる
3. **上限超過時は強制クローズ**: 上限内に完了しない接続は警告ログを 1 行出した上で
   強制クローズする（ハング防止のフェイルクローズ）

どちらの経路でも `run_until` は `shutdown_grace_period` + ε 以内に必ず
`Ok(())` で戻る。

## 使い方: `tokio::signal::ctrl_c` と組み合わせる

実行可能な完全例は `crates/core/examples/graceful_shutdown.rs` を正とする
（[`README.md`](./README.md) の二重管理をしない原則）。

```bash
cargo run --example graceful_shutdown -p fandhe-backend-core
curl -v http://127.0.0.1:3001/    # 200 応答
# Ctrl-C を送ると新規接続の受理を止め、in-flight 完了を待って終了する
```

構成の要点は次のとおり（コード断片。全文は example を参照）。

```rust,ignore
use fandhe_backend_core::Server;

let server = Server::new()
    .handler(router)
    .shutdown_grace_period(std::time::Duration::from_secs(10));
let bound = server.bind("127.0.0.1:3001").await?;

bound
    .run_until(async {
        tokio::signal::ctrl_c()
            .await
            .expect("Ctrl-C シグナルハンドラの登録に失敗しました");
        println!("シャットダウンシグナルを受信しました");
    })
    .await
```

利用側アプリで `tokio::signal` を使う場合は、自分の `Cargo.toml` で tokio の
`signal` feature を有効にする（fandhe-backend 側では dev-dependencies 限定であり、
コアの依存グラフには現れない）。SIGTERM（Kubernetes 等のコンテナ環境の停止
シグナル）と組み合わせる場合も同様に、`tokio::signal::unix::signal` で作った
Future を `shutdown` として渡せばよい。

## `run()` との使い分けと後方互換

| 呼び出し | 停止手段 | 用途 |
|---------|---------|------|
| `run()` | なし（プロセス kill のみ） | ベンチ・使い捨てのローカル実行・従来コードの無変更維持 |
| `run_until(shutdown)` | `shutdown` Future の完了 | 本番運用・デプロイ更新を伴う長期稼働 |

`run()` は `run_until` への薄い委譲として残っており、既存の `run()` 利用箇所は
無変更のまま動作する（後方互換のため）。新規コードでは
`run_until` の利用を推奨する。

なお `run_until` が返す Future 自体を呼び出し側の `tokio::select!` 等で外部
キャンセルした場合、in-flight 接続は abort されず独立タスクとして完走する
（従来の detached spawn 時代の挙動を維持）。ただしこの経路では grace 上限に
よる強制クローズも働かないため、確実に片付けたい場合はキャンセルではなく
`shutdown` Future の完了で止めること。

## 稼働中の listener 差し替え（rebind）

`BoundServer::run_until` の accept ループを止めずに listening アドレスを
差し替えたい場合は `RebindHandle` を使う。

| API | 役割 |
|-----|------|
| `BoundServer::rebind_handle()` | `run_until` へ move する前の `BoundServer` から `RebindHandle` を取得する |
| `RebindHandle::rebind(addr)` | `addr` へ新規 `TcpListener` を bind してから、稼働中の accept ループへ listener の差し替えを依頼する |

`rebind` は呼び出し側で新規 bind を完了させてから差し替えを依頼する構造の
ため、`addr` への bind に失敗した場合は旧 listener・処理中の接続に一切影響
しない（fail-closed）。差し替え成功後は新規接続のみ新アドレスで受理され、
旧アドレスの listener は即座に閉じられる。差し替え直前までに旧アドレス経由
で確立済みだった「旧世代」接続は `Server::shutdown_grace_period` を上限に
**背景タスクで** drain され（`run_until` 自体・新世代の accept ループは
ブロックされない）、超過分は強制クローズする（最終 graceful shutdown と
同じ仕組みを世代ごとに独立適用したもの、詳細は
[`docs/design/rebind.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/rebind.md)）。

旧世代の WebSocket 委譲セッションへは世代キャンセルが伝播し、正常な Close
ハンドシェイク（close code 1001 Going Away → `WebSocketConfig::close_grace`
上限のドレイン）で切断される（
[`docs/design/ws-cancellation-propagation.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/ws-cancellation-propagation.md)）。

`Server::shutdown_grace_period` に基づく背景 shutdown が確定した以降に呼んだ
（または呼び出し中だった）`rebind` は、grace 期間の終了を待たず速やかに
`Err` を返す。

**副作用（`Server::webrtc` 登録時）**: `Server::webrtc` にスキーマ登録済みの
場合、`rebind` 呼び出しのたびに `WebRtcConfig::registry` 上のアクティブな
`RTCPeerConnection` が**新旧世代を区別せず全件**強制切断される（WS 委譲
セッションのような `shutdown_grace_period` の猶予は適用されない）。単なる
リスニングポート切り替えのつもりで `rebind` を呼んでも、rebind と無関係な
進行中の WebRTC 通話まで切断されうる点に注意する（`RebindHandle::rebind`
doc・上記 `docs/design/ws-cancellation-propagation.md` 10 節を参照）。

```rust,ignore
use fandhe_backend_core::Server;

let mut bound = Server::new().handler(router).bind("127.0.0.1:3001").await?;
let rebind = bound.rebind_handle();
tokio::spawn(async move { bound.run().await });

// 新アドレスへ bind してから差し替えを依頼する。
let new_addr = rebind.rebind("127.0.0.1:3002").await?;
```

**セキュリティ**: `rebind` に渡すアドレスへ、HTTP リクエスト由来の値
（クエリパラメータ・ヘッダ等の外部入力）を直接渡さないこと。運用者が制御
する設定値・環境変数からのみ呼び出す（`.claude/rules/security.md` の入力
検証観点、`RebindHandle` の doc に準拠）。

## drain 中の idle keep-alive 接続の扱い

drain（`run_until` の最終 graceful shutdown・`RebindHandle::rebind` の旧世代
drain のいずれも同一機構）開始時点で、リクエスト待ちの idle 状態にある
keep-alive 接続をどう扱うかは、次の 4 点を**公開契約**（後方互換性の対象。
`docs/design/versioning-policy.md` の破壊的変更手続きを経ない限り変更しない）
として保証する。

1. drain 開始を理由として idle keep-alive 接続を即座に閉じない
2. grace 期限内に処理が完了する範囲で、通常のリクエストは拒否されず
   受理・完走する。保証されるのは「到着」ではなく「到着かつ grace 期限内の
   完了」であり、grace 直前に到着してもハンドラの処理が grace 期限をまたぐ
   場合は下記 4 の強制クローズが優先し、処理途中でも abort されうる。
   grace 期限内に完了した場合、応答には `Connection: close` が付与され、
   以降その接続では次のリクエストを受け付けない（接続あたり drain 後に
   処理する後続リクエストは最大 1 件）
3. WebSocket 等の Upgrade リクエストは上記「セキュリティ・制約」節の
   とおり 503 で拒否される（2 の例外）
4. 後続リクエストが来ない場合を含め、接続は `shutdown_grace_period` + ε
   以内に必ず閉じる（強制クローズによる有界性。フェイルセーフ）

一方、後続リクエストが来ない場合に接続が具体的にいつ閉じるか（read
タイムアウト到達か grace 超過強制クローズかの別）は**実装詳細**であり
保証しない。

この契約により、drain 期間中も既存の keep-alive 接続でアプリケーションの
最後のリクエストを 1 件処理させてから終了させる、といった利用側の実装が
安全に成立する。詳細な判断根拠・回帰テストは
[`docs/design/graceful-shutdown.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/graceful-shutdown.md)
7.2 節、rebind 経由の同一契約は
[`docs/design/rebind.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/rebind.md)
5.6 節を参照。

## セキュリティ・制約

- **フェイルクローズ**: grace 超過時は残存接続を強制クローズし、`run_until` が
  無期限にハングしない（[`.claude/rules/security.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/security.md)
  のリソース枯渇・可用性観点）
- shutdown フラグ受信後に到着した WebSocket 等の Upgrade リクエストは委譲せず
  **503 で拒否**する（grace 強制クローズの管理外となる detached セッションを
  shutdown 後に増やさないため）
- shutdown 前に委譲済みの WebSocket セッションは grace 超過時の強制 abort の
  対象外である（既知の限界）。ただし in-flight 完了待ちはタイムアウトで実装されて
  いるため、セッションが生き続けても `run_until` 自体は grace + ε 以内に必ず戻る
- accept エラー（`ECONNABORTED`・fd 枯渇等）は一過性として扱い、`run_until` を
  終了させず短い待機の後に accept を再試行する（1 件のエラーでリスナー全体が
  停止しない可用性設計）

## 関連ドキュメント

- 最小サーバの起動と `Server` builder の全体像: [`getting-started.md`](./getting-started.md)
- ストリーミング応答と shutdown の関係（producer タスクの完走待ち）:
  [`streaming.md`](./streaming.md)
- 設計判断の記録（`tokio::select!` を使わない理由・セマフォによる in-flight
  検知等）: [`docs/design/graceful-shutdown.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/graceful-shutdown.md)
- 実行可能な完全例: `crates/core/examples/graceful_shutdown.rs`
