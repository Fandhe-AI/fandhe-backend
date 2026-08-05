# 稼働中 `BoundServer` の再バインド（`rebind`）設計判断

イシュー #485（`feat(core): 稼働中 BoundServer の再バインド（listener 差し替え）API`）
対応。`crates/core/src/server.rs` に実装した `BoundServer::rebind_handle` /
`RebindHandle::rebind` の設計判断・根拠・既知の限界を記録する。API・実装の詳細は
同ファイルの doc comment を正とし、本書は「なぜその選択をしたか」を補足する。

## 1. 背景・受け入れ条件

`BoundServer::run_until`（graceful shutdown、イシュー #313、
[`docs/design/graceful-shutdown.md`](graceful-shutdown.md)）は accept ループの
「停止」は実装済みだが、稼働中に listening アドレスを差し替える手段がなかった。
ポート変更・IP バインド先の切り替えを伴うデプロイ手順では、プロセス全体を
再起動せざるを得ず、その間は新規接続を受け付けられない。

承認済み実装計画の受け入れ基準（5 項目）:

1. 再バインド API: 新アドレスへの新規リクエストが成功し、旧アドレスへの新規
   connect は拒否される
2. fail-closed: bind 失敗時は旧 listener・in-flight 接続に一切影響しない
3. 旧 listener の drain: 旧世代の keep-alive 接続は in-flight を完走し、
   `Connection: close` を伴う。grace 超過時は強制クローズする
4. 拡張点引き継ぎ: `Middleware` / `RequestGate` / `UpgradeHandler` /
   `Interceptor` / `Handler` は再登録なしで新アドレスでも動作する
5. 回帰なし: 既存の graceful shutdown・通常経路のテストが無変更で通る。
   `rebind_handle` を呼ばない経路ではチャネルが生成されない

## 2. 公開 API

```rust,ignore
impl BoundServer {
    pub fn rebind_handle(&mut self) -> RebindHandle;
}

#[derive(Clone)]
pub struct RebindHandle { /* ... */ }

impl RebindHandle {
    pub async fn rebind(&self, addr: impl ToSocketAddrs) -> io::Result<SocketAddr>;
}
```

`rebind_handle` は `run_until`（または `run`）を呼ぶ**前**に呼び出す契約。
`BoundServer` は `self` を消費して accept ループへ入るため（`run_until(self, ...)`）、
ハンドルは事前に取り出しておく必要がある。`RebindHandle` 自体は `Clone` 可能で、
複数箇所・複数回の呼び出しに使い回せる。

## 3. 設計方針: rebind コマンドチャネル + ハンドル方式

`run_until` の accept ループへ、shutdown Future と同様の「外部から通知を受ける
チャネル」を追加する構成を選んだ。代替案として検討したもの:

- **`BoundServer` を可変参照で共有し、外部から直接 listener を差し替える**:
  `run_until` は accept ループの内部で `TcpListener` を所有し続ける必要があり、
  外部から直接書き換えるには `Arc<Mutex<TcpListener>>` 等の共有可変状態が要る。
  accept のたびにロックを取る設計は accept ホットパスへ同期プリミティブを
  持ち込むことになり、既存の「accept は lock-free（セマフォの `acquire_owned`
  のみ）」という設計と相性が悪い
- **`run_until` を都度呼び直す（新 `BoundServer` を都度 `bind` し直す）**:
  呼び出し側で「旧 `run_until` を止めて新しい `bind`/`run_until` を始める」を
  自前で組む方式。旧世代の in-flight drain・世代を跨いだ `max_connections` の
  一貫性を呼び出し側が再実装することになり、複雑さが利用者側に漏れる

採用した「コマンドチャネル + ハンドル」方式は、`run_until` 内部の
`race_shutdown_or_accept`（4 節、poll_fn ベース）に 1 分岐を足すだけで済み、
世代管理（5 節）をコア側に閉じ込められる。

### 3.1 ハンドル側で bind する理由（fail-closed が構造的に成立する）

`RebindHandle::rebind(addr)` は次の順で処理する:

1. `TcpListener::bind(addr).await` を実行する
2. 失敗すれば `Err` を返すだけで、`run_until` へは何も送信しない
3. 成功すれば `RebindCommand { listener, reply }` を `mpsc::Sender` へ送る
4. `reply`（`oneshot`）の受信を待ち、差し替え完了を確認してから `local_addr` を返す

bind の失敗判定を `run_until` 側（コマンド受信後）ではなく `RebindHandle::rebind`
側（コマンド送信前）に置いたことで、「bind に失敗したら何も起きない」という
fail-closed 性質が構造的に保証される。`run_until` の accept ループはコマンドを
受信した時点で listener が既に有効であることを前提にでき、エラーハンドリングを
持たない単純な差し替え（`listener = new_listener`）で済む。

### 3.2 チャネル容量とコマンド滞留の有界化

`mpsc::channel(1)`（容量 1）を使う。`RebindHandle::rebind` は bind 済みの
listener を 1 個だけ運ぶコマンドを送るだけであり、`run_until` 側がそれを
処理するまで次のコマンドを受け付ける必要はない。これによりコマンド滞留を
有界化し、`.claude/rules/security.md` のリソース枯渇対策と整合させる。

### 3.3 遅延生成（pay-for-what-you-use）

`rebind_handle` を一度も呼ばなければ `mpsc::channel` は生成されない
（`BoundServer::rebind_tx` / `rebind_rx` は初期値 `None`）。`run_until` は
`rebind_rx: Option<&mut mpsc::Receiver<RebindCommand>>` を 3-way race の
ヘルパへ渡し、`None` の間は当該分岐を常に pending 扱いとしてポーリングしない。
既存の `run()` / `run_until` を素朴に呼ぶだけの利用者は、rebind 機構の
ランタイムコストを一切払わない。

## 4. 3-way race（`tokio::select!` を使わない理由の継承）

`race_shutdown_or_accept`（既存、graceful shutdown で確立）は `tokio::select!`
（`macros` feature、proc-macro 系推移依存を要求）を使わず、`std::future::poll_fn`
+ `std::pin::pin!` で shutdown Future と accept Future を競合させていた
（`crates/core/Cargo.toml` の tokio feature を `rt` / `net` / `io-util` /
`time` / `sync` の 5 つに限定する既存規約、pay-for-what-you-use）。本イシューでも
この方針を継承し、同じヘルパへ rebind コマンドの `mpsc::Receiver::poll_recv` を
1 分岐追加する形で 3-way 化した。

ポーリング優先順位は **shutdown > rebind > accept** の固定順:

- shutdown を最優先するのは既存の理由（shutdown 直後の新規受理を避ける）と同じ
- rebind を accept より先にポーリングするのは、同一 poll で両方 Ready になり
  うる場合に、新規接続を「差し替え前の古い listener」で受理してしまう競合を
  避けるため

`rebind_rx` が `None`（未使用）の間は分岐自体を評価しない。送信側
（`RebindHandle`）が全て drop された場合（利用者がハンドルを破棄した場合）は
`poll_recv` が `Poll::Ready(None)` を返しうるが、これは「今後 rebind コマンドは
来ない」ことを意味するだけで shutdown 相当ではないため無視し、以降は accept の
みをポーリングし続ける（`race_shutdown_or_accept` の doc・実装を参照）。

## 5. 世代別 drain

rebind コマンドを受理すると、`run_until` は次の手順を踏む（`run_until` の doc
「稼働中の再バインド」を正とする）:

1. その時点までの「旧世代」向け `shutdown_flag`（`Arc<AtomicBool>`）を `true`
   にする。既存の graceful shutdown 機構（keep-alive 接続を早期クローズへ
   倒す）にそのまま合流させる
2. listener を新しい `TcpListener`（bind 済み）へ差し替える。以降の accept は
   新アドレスに対してのみ行われる
3. 旧世代のコネクションタスク一式（`CancelSafeJoinSet`）を `std::mem::replace`
   で現行の `JoinSet` から切り離し、独立した背景タスク（`spawn_generation_drain`）
   で `Server::shutdown_grace_period` を上限に drain する（超過分は
   `JoinSet::shutdown` で強制クローズ）。この drain は `run_until` 自体を
   ブロックしない。新世代の accept ループは並行して動き続ける
4. 新世代用の `shutdown_flag`（`Arc<AtomicBool>::new(false)`）を用意する

### 5.1 なぜ `run_until` をブロックしないのか

旧世代の in-flight 接続が grace 期間内に完走しない限り新規 accept を止める
理由はない。稼働中の再バインドという機能の性質上、「新世代は新世代で受理を
続けながら、旧世代は独立に畳んでいく」方が可用性の観点で望ましい。既存の
graceful shutdown（`run_until` 自体の終了）は逆に「新規 accept を止めてから
畳む」必要があるため、両者は意図的に異なる手順を取る。

### 5.2 セマフォ（`connection_limit`）は世代を跨いで単一共有

`connection_limit`（`Arc<Semaphore>`）・`permit_total` はどちらも rebind の
前後で不変であり、世代を跨いで単一のまま使い続ける。旧世代・新世代のどちらの
コネクションも同じセマフォから permit を取得するため、`max_connections` に
よる同時接続数上限は世代を跨いでも正しく維持される。

これは `run_until` 自体の最終 graceful shutdown（`acquire_many_owned(permit_total)`
による in-flight 完了待ち）が、旧世代の背景 drain タスクが握る permit も
含めて正しく待てることを意味する。仮に旧世代分の drain が最終 shutdown より
長引いていても、それは通常の「grace 超過」として扱われ、最終 shutdown 側も
自身の grace 期間で強制クローズへフォールバックする（フェイルクローズ、
`.claude/rules/security.md`）。

### 5.3 拡張点の引き継ぎ

`Middleware` / `RequestGate` / `UpgradeHandler` / `Interceptor` は
`Arc<Server>`（`run_until` が保持、世代を跨いで不変）に登録されているため、
rebind の前後で再登録は一切不要である。`Handler`（`Server` の既定ハンドラ）も
同様。世代交代で変わるのは listener と `shutdown_flag` のみであり、拡張点の
評価順序・契約（`crates/core/src/server.rs` モジュール冒頭 doc の「1 接続あたりの
処理フロー」）は不変。

### 5.4 WebSocket 委譲セッションは `JoinSet` の drain 対象外だが、世代キャンセルは伝播する

5 節の手順 3（旧世代 `JoinSet` の切り離し・背景 drain）が対象とするのは
`run_until` が管理する `JoinSet` に積まれたコネクションタスクのみである。
`UpgradeHandler` の委譲が成立し `handle_connection_with_permit` から
`fandhe_backend_plugin_websocket` 側の専用タスクへ permit ごと `move`
された WebSocket セッションは、この `JoinSet` の外側で独立に走っているため、
rebind 時点で「旧世代」に属していても `spawn_generation_drain` の
grace 超過時 `JoinSet::shutdown` による強制 abort の対象にはならない。

ただしイシュー #490〜#492 で、`spawn_generation_drain` の冒頭にて世代
キャンセル（`crate::plugin::GenerationCancel::fire`）を明示的に発火し、
委譲済みの WS 専用タスクへ伝播する経路を実装済みである。旧世代 WS
セッションは正常な Close ハンドシェイク（close code 1001 Going Away →
`WebSocketConfig::close_grace`（既定 10 秒、イシュー #500 で設定可能化）
上限で応答待ち）で終端し、`JoinSet` 強制 abort
のようなハードクローズには依存しない。permit は共有セマフォ経由のため
`run_until` 自体の最終 graceful shutdown・以降の drain 待ちには反映される。
詳細・統合テストは 6 節「WebSocket 委譲セッションと世代 drain」を参照。

### 5.5 shutdown 確定と rebind チャネルの関係（Bugbot 指摘対応）

`race_shutdown_or_accept` が `Raced::Shutdown` を返した直後（grace drain 開始前）に、
`run_until` は `rebind_rx`（受信側）を明示的に `drop` してチャネルを閉じる。この
タイミングを grace drain の**前**に置くのは、drain の間ずっとチャネルを開けたまま
にすると次の 2 つの問題が生じるため:

1. shutdown 確定後に発行された（または送信済みで reply 待ちだった）
   `RebindHandle::rebind` 呼び出しが、`send` / `reply_rx` 双方で最大
   `Server::shutdown_grace_period` までブロックし続けてしまう
2. 呼び出し側が既に bind 済みの新 `TcpListener` が `RebindCommand` として
   チャネルバッファに滞留したまま、`rebind_rx` が drop されるまでポートを
   保持し続けてしまう（実際には誰にも使われない listener が、無駄にポートを
   専有し続ける）

`rebind_rx` を shutdown 確定時点で即座に drop することで、(a) 以後の
`RebindHandle::rebind` の `send` は即座に失敗し `run_until` 終了済みと同じ
`Err` を fail-fast で返す、(b) 送信済みで `reply_rx` 待ちだったコマンドも
チャネルクローズにより即座にブロックが解消し `Err` を返す、(c) そのコマンドが
保持していた新 `TcpListener` も同時に drop されポートが速やかに解放される。

`RebindHandle::rebind` の doc「# エラー」節にも同様の契約を明記した。回帰テストは
`crates/core/tests/rebind.rs` を参照（shutdown 送出後の `rebind` が grace 期間を
待たず短時間で `Err` になること・rebind で使ったポートが shutdown 後すぐ再利用
可能になることを検証）。

## 6. 既知の限界・スコープ外

- **listener 差し替え瞬間の accept backlog 喪失**: rebind コマンドは 3-way
  race で accept より優先してポーリングされる（4 節）ため、旧 listener を
  差し替える直前の 1 poll で「OS レベルでは 3-way handshake が完了しカーネル
  の accept backlog に滞留しているが、まだ `listener.accept()` を呼んで
  いなかった」接続が存在しうる。差し替え後は旧 `TcpListener` を即座に
  `drop` する設計（1 節・3.1 節、fail-closed の構造的保証と表裏一体）のため、
  この滞留分は `accept()` されないままソケットごと破棄され、クライアントから
  見ると接続確立後に RST を受け取る形になる。これは「旧 listener を即座に
  閉じる」という意図された設計の帰結であり、バグではない。許容している理由は
  次の 2 点: (i) TCP クライアントは接続確立後の RST を通常のネットワーク
  エラーとして扱い、大半の HTTP クライアント実装はリトライ可能である、
  (ii) backlog 滞留自体が極めて短い時間窓（1 poll 分）に限られ、実運用の
  デプロイ手順（新旧アドレスを跨ぐロードバランサ切り替え等）では滞留量が
  無視できるほど小さいと想定される。この滞留を厳密にゼロにするには
  listener 差し替え前に残存 backlog を drain する仕組みが要るが、
  「rebind を accept より優先しないと新 listener 差し替え前の古い listener
  で新規接続を受理してしまう」という 4 節の競合回避方針とトレードオフの
  関係にあり、本イシューのスコープ外とする
- **WebSocket 委譲セッションと世代 drain（解消済み）**:
  `handle_connection_with_permit` から `UpgradeHandler` 経由で WebSocket
  専用タスク（`fandhe_backend_plugin_websocket` 側の `tokio::spawn`）へ
  permit ごと `move` された接続は、rebind 時点の「旧世代」`JoinSet`
  （5 節）の外にあるため、`spawn_generation_drain` の grace 超過時
  `JoinSet::shutdown` による強制 abort の対象には今も含まれない。
  イシュー #490（[`docs/design/ws-cancellation-propagation.md`](ws-cancellation-propagation.md)）
  で設計し、#491（コア配線）・#492（`fandhe_backend_plugin_websocket` 側の
  Close ハンドシェイク実装）で実装済みの世代キャンセル伝播機構により、
  `spawn_generation_drain` の冒頭発火が WS 委譲タスクへ明示的なキャンセル
  シグナルとして伝わり、正常な Close ハンドシェイク（close code 1001
  Going Away → `WebSocketConfig::close_grace`（既定 10 秒）上限で応答待ち）
  で終端する。Close に応答しないクライアントも `close_grace` 有界で強制
  終端され、無期限に生存し続けることはない。permit は共有セマフォ経由の
  ため `run_until` 自体の最終 graceful shutdown・次回以降の rebind の
  drain 待ちには（`close_grace` 経由での解放を含め）反映される（5.2 節）。
  両経路（最終 shutdown・rebind 世代 drain）の end-to-end 検証はイシュー
  #493 の統合テスト（`crates/core/tests/ws_cancellation.rs`）が、居座り
  クライアントの有界終端・rebind 反復での permit 単調消費なしを含めて
  担保する
- **旧世代の背景 drain タスクは detached**: `spawn_generation_drain` が
  `tokio::spawn` するタスクの `JoinHandle` は保持しない。旧世代の permit は
  共有セマフォ経由で最終 shutdown の待ち合わせに含まれる（5.2 節）ため
  実害はないが、`run_until` の呼び出し元がこの背景タスクの完了を明示的に
  待つ手段は現状ない
- **複数回 rebind の連続発行**: `mpsc::channel(1)` の容量制約により、
  `RebindHandle::rebind` の呼び出しは前回のコマンドが `run_until` に消費
  されるまで（`send` が完了するまで）待たされる。通常のデプロイ手順
  （1 回の切り替え）では問題にならないが、短時間に多数回の rebind を
  連続発行する用途は想定していない
- **OS シグナル・設定ファイル監視との統合ヘルパー**: `graceful-shutdown.md`
  4 節と同じ方針で、コアはシグナルハンドラ・設定監視を持たない。呼び出し側が
  任意のトリガーから `RebindHandle::rebind` を呼ぶ設計とする
- **rebind 先アドレスの入力検証**: `RebindHandle::rebind` に渡すアドレスへ、
  HTTP リクエスト由来の値（クエリパラメータ・ヘッダ等の外部入力）を直接
  渡さないこと。信頼できない値を bind 先に使うと、意図しないインターフェース
  への待受につながる（`RebindHandle::rebind` の doc「セキュリティ」節・
  `.claude/rules/security.md` の入力検証観点）

いずれも [[out-of-scope-tracking]] に従い、Issue 起票はユーザー承認を得てから
行う。

## 7. 検証

```bash
cargo build -p fandhe-backend-core                  # feature なし
cargo build -p fandhe-backend-core --all-features   # 全 feature
cargo test -p fandhe-backend-core --test rebind
cargo test -p fandhe-backend-core --all-features
cargo test -p fandhe-backend-core
cargo tree -p fandhe-backend-core -e features        # tokio feature が 5 つのまま
cargo clippy -p fandhe-backend-core --all-targets --all-features -- -D warnings
cargo fmt --check
```
