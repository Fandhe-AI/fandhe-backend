# 接続単位ハンドラと切断通知の API 設計

- **対応イシュー**: [#703](https://github.com/Fandhe-AI/fandhe-backend/issues/703)
  「接続単位ハンドラと切断通知の API を設計して文書化する」（親
  [#702](https://github.com/Fandhe-AI/fandhe-backend/issues/702)「接続単位の
  ハンドラ状態と切断通知に対応する」）
- **ステータス**: ドラフト（自動運転モードでの実装、最終承認は人間レビュー）
- **対応可否判定（feasibility-guardrail）**: **可**。受け入れ基準あり（設計文書の
  作成・案 A/B の比較と採用理由・`CloseReason` の定義・0.4.2/0.5.0 のバージョン方針の
  4 点、いずれも検証可能）・安全性方針と整合（ドキュメント追加のみでコード変更を
  伴わず、既存の DoS 対策・機密混入防止方針を後退させない設計を選ぶ）・影響範囲限定
  （`docs/design/` + 同 README・`CLAUDE.md`・`plugin-boundary.md` の 4 ファイルのみ）
  の 3 軸すべて充足。判定内容は本イシューの実装コミットに記録する
- **本イシューのスコープ**: **設計のみ**。コード変更は行わない。実装は
  [#704](https://github.com/Fandhe-AI/fandhe-backend/issues/704)〜
  [#707](https://github.com/Fandhe-AI/fandhe-backend/issues/707) が担う（9 節参照）

受け入れ基準の対応箇所:

| 受け入れ基準 | 対応節 |
|---|---|
| 設計文書がある | 本文書全体 |
| 案 A/B の比較と採用理由 | 2 節 |
| `CloseReason` の定義 | 4 節 |
| 0.4.2 / 0.5.0 のバージョン方針 | 7 節 |

## 1. 背景・現状の制約

親イシュー #702 は、fandhe-browser の CDP（Chrome DevTools Protocol）互換サーバー
採用に向け、`fandhe-backend-plugin-websocket` の現行 `WsMessageHandler`
（`crates/plugin-websocket/src/handler.rs`）に 4 つの不足があると指摘している。

1. **接続コンテキストが `on_message` に渡らない**: `on_message(&self, msg: WsMessage)`
   はメッセージしか受け取らず、どの接続からかが分からない。接続コンテキスト
   （[`WsOpenContext`]）は `on_open` にしか渡らない（`crates/plugin-websocket/src/lib.rs`
   `handle_upgrade` が 101 応答送出成功後に一度だけ構築して渡す）
2. **ハンドラが接続単位の状態を持てない**: `WebSocketConfig::handler`
   （`crates/plugin-websocket/src/config.rs`）は `Arc<dyn WsMessageHandler>` 1 個を
   全接続が共有する。ハンドラの `self` は不変の共有参照であり、接続ごとの状態を
   自然な形で持てない
3. **切断が `on_close` として通知されない**: 現行 API に `on_close` 相当は存在せず、
   切断は次に `WsSender::send` を呼んだときの失敗（[`WsSendError`]）でしか観測できない
4. **`on_message` 実行中に送信キューが消化されずデッドロックしうる**:
   `crates/plugin-websocket/src/session.rs` `run_session` の受信ループは、
   `race_cancel(cancel.as_mut(), config.handler.on_message(...))` として
   ユーザーハンドラの `Future` を cancel のみと race させ、単独 await する
   （`Message::Text(text)`/`Message::Binary(bin)` の Text/Binary 分岐 2 箇所）。
   この間、`WsSender` の outbound
   チャネル（既定容量 [`DEFAULT_OUTBOUND_CAPACITY`] = 8）を消費する者がいない。
   受信ループ本体（`inbound` と `outbound` の合流、[`race2_alternating`]）は
   `on_message` 呼び出しの**外側**でのみ機能するため、`on_message` 内で容量超の
   `WsSender::send` を呼ぶとデッドロックする

現行コードの該当箇所（読解済み）:

- `crates/plugin-websocket/src/handler.rs`: `WsMessageHandler` trait（`on_message`
  必須・`on_open` 既定実装 provided、イシュー #671）、`WsOpenContext`
  （`#[non_exhaustive]`・非公開フィールド + アクセサ、`sender()`/`param()`/`params()`）、
  `WsSender`（`mpsc::Sender<WsMessage>` の薄いラッパー、`send` のみ公開）
- `crates/plugin-websocket/src/session.rs`: `run_session` が受信ループ本体。
  `Message::Close(_) => break;`／`InboundEvent::Message(None) => break;`（EOF）／
  `InboundEvent::Idle`／`race_cancel` が `None`（shutdown/rebind キャンセル、受信ループ
  先頭・outbound 送出中・on_message 実行中・`apply_outcome` 内部レース由来の複数箇所）／
  `WsOutcome::Close`（[`apply_outcome`] の `SessionFlow::Closed`）／ハンドラ `Err`
  （`outcome?` で即時 `Err(WsError::Handler)` として関数を抜け、Close ハンドシェイクを
  経ない）／`InboundEvent::Outbound` 分岐の `ws.send` 失敗・`apply_outcome` 内部の
  `ws.send`/`ws.close` 失敗（いずれも `Err(WsError::Protocol(_))` として即時関数を抜ける）
  という終了経路が個別の `break`/`return`/`?` に分散している。全経路の網羅表は 4 節を参照
- `crates/plugin-websocket/src/lib.rs` `handle_upgrade`: 101 応答成功後に
  `handler::channel(handler::DEFAULT_OUTBOUND_CAPACITY)` でチャネルを作り、
  `WsOpenContext::new(sender, params)` を構築して `on_open` を一度呼び、
  `session::run_session(stream, leftover, config, cancel, Some(outbound_rx))` へ
  委譲する。`crates/core/src/plugin.rs` の `try_handle_upgrade` は
  `let _ = fandhe_backend_plugin_websocket::handle_upgrade(...)` でエラーを握り捨てて
  おり、本設計・後続実装（#704/#705）ではこの行を変更しない（切断理由はプラグイン
  内部で `on_close` を通じて観測できるため、影響範囲想定から `crates/core` を明示的に
  除外する）
- `docs/design/versioning-policy.md`: pre-1.0（0.x）期は `z` が非破壊追加、trait への
  **必須**メソッド追加のみが破壊的（provided メソッド追加は対象外）。
  `crates/plugin-websocket` の `on_open`/`WsOpenContext`/`with_path_pattern`
  （#671/#675/#676）は **0.4.1 に「BREAKING CHANGE はありません」で lockstep 収録済み**
  （`CHANGELOG.md` で確認済み）という直接の先例がある
- `docs/design/ws-cancellation-propagation.md`: 本ドキュメントの節構成（背景→現状構造→
  方式比較→契約変更要否→対応付け→pay-for-what-you-use→実装指針/受け入れ基準対応表→
  セキュリティ考慮事項→スコープ外）の雛形

## 2. 案 A（`with_handler_factory` による接続単位インスタンス化）と案 B（`on_message_with_ctx` 既定実装併設）の比較

### 案 A の要約

`WebSocketConfig::with_handler_factory(|ctx| ...)` を新設し、接続確立ごとにハンドラの
新規インスタンスを生成する。インスタンスは接続専用なので `self` のフィールドに接続
状態を自然に持てる。

### 案 B の要約

`WsMessageHandler` に `on_message_with_ctx`（既定実装付き、既存の `on_message` へ
委譲）と `on_close`（既定実装 no-op）を**両方 provided** で追加する。ハンドラの
共有シングルトン構造（`Arc<dyn WsMessageHandler>` 1 個）は変えず、接続ごとの状態は
利用者が接続 ID をキーにした自前の `Mutex<HashMap<...>>` 等で管理する。

### 決定打となる制約（採用理由の核）

案 A の「インスタンス生成 = 接続開始、Drop = 切断」という発想は、後続イシュー
#705 が要求する `CloseReason`（クライアント Close・EOF・idle timeout・shutdown
キャンセル・プロトコルエラー・ハンドラ Close・受信上限超過を**区別可能な**形で
通知する）を運べない。Rust の `Drop` はいつ・なぜ呼ばれたかの情報を持たないため、
`on_close(ctx, reason: CloseReason)` という明示コールバックが必要な時点で、Drop
ベースの通知は不採用が確定する。したがって案 A を採用しても `on_close` 相当の明示
コールバックは別途必要になり、案 A が持つはずだった「Drop で足りる」という利点は
消える。

加えて、案 B は既存の `on_message`/`WsOutcome` を使うハンドラ（`EchoHandler`・
`UppercaseHandler`・`examples/with-websocket` の `PingPongEchoHandler`）を**一切
変更せず**コンパイル・動作させられる（`on_message` を必須のまま残し、
`on_message_with_ctx` の既定実装がそれを呼ぶため）。案 A は新しい登録 API
（`with_handler_factory`）と新しいオーナーシップモデル（`Arc<dyn WsMessageHandler>`
共有 → 接続ごとの新規インスタンス）を導入し、`WebSocketConfig` の内部表現変更を
要する分、変更範囲が大きい。

### 結論

**案 B を採用する。** 案 A は「不採用」ではなく、将来的に案 B の上へ糖衣として
追加できる余地がある構成のため、本イシューでは再検討条件として 10 節に記録するに
留め、新規 Issue は起票しない（[[out-of-scope-tracking]] は起票にユーザー承認を
要するため、自動運転モードでは記録のみに留める）。

## 3. 新 API 詳細（案 B の確定形）

### `WsConnId`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WsConnId(u64);

impl fmt::Display for WsConnId { /* "{}" 経由で内部値を表示 */ }
```

`handler.rs` にモジュールスコープの `static NEXT_CONN_ID: AtomicU64 =
AtomicU64::new(1)` を置き、`fetch_add(1, Ordering::Relaxed)` で発行する（プロセス内・
複数 `WebSocketConfig` 登録をまたいで一意。`unsafe` 不使用）。理論上の u64 使い切りは
実用上の懸念なしと明記する（既存文書の同種の注記に揃える）。

### `WsConnContext`（新設）

`#[non_exhaustive]`、非公開フィールド + アクセサ（`WsOpenContext` と同じ設計
パターン）:

- `conn_id() -> WsConnId`
- `sender() -> &WsSender`
- `param(name: &str) -> Option<&str>`
- `params() -> impl Iterator<Item = (&str, &str)>`

`Debug` は `params` の値を出力しない（`WsOpenContext::Debug` と同一のセキュリティ
根拠: URL セグメントは攻撃者制御下の入力）。

`WsOpenContext` とは意図的に別型とする。`on_open` は「一度だけ消費される値」、
`on_message_with_ctx`/`on_close` は「繰り返し参照される値」という用途の違いを型で
分離し、将来どちらかだけにフィールドを追加する場合の型結合を避ける。

**`WsConnContext` は `WsSender` を自身の非公開フィールドとして保持する**（`Arc` や
チャネル送信側そのものを clone して持つ）。これは 5 節で述べる副作用（outbound
チャネルがセッション生存中は閉じなくなる）を伴う。

### `WsOpenContext` の拡張（非破壊）

新規フィールド `conn_id: WsConnId` + アクセサ `conn_id() -> WsConnId` を追加する
（`#[non_exhaustive]`・非公開フィールドのため追加は非破壊）。`pub(crate) fn
new(...)` の引数追加も非公開関数なので破壊的変更にならない。

### `WsMessageHandler` trait への追加

いずれも既定実装（provided）、`on_message` は必須のまま不変:

```rust
fn on_message_with_ctx<'a>(
    &'a self,
    ctx: &'a WsConnContext,
    msg: WsMessage,
) -> BoxFuture<'a, Result<WsOutcome, WsHandlerError>> {
    let _ = ctx;
    self.on_message(msg)
}

fn on_close(&self, ctx: &WsConnContext, reason: CloseReason) {
    let _ = (ctx, reason);
}
```

**計画からの変更点（ライフタイム）**: 当初案は `fn on_message_with_ctx(&self, ctx:
&WsConnContext, msg: WsMessage) -> BoxFuture<'_, ...>` だったが、`&self` が存在する
シグネチャでは省略記法の `'_` は `self` の借用にのみ束縛され、`ctx` を捕捉した
`Future` を返すオーバーライド実装がコンパイルできない。`&'a self` と `ctx: &'a
WsConnContext` を同一の明示ライフタイム `'a` に統一し、返す `BoxFuture<'a, ...>` が
両方の借用を生存させられるようにする。

接続単位の状態・イベント購読を扱う新規ハンドラは `on_message_with_ctx` を
オーバーライドする。この場合トレイトの制約上 `on_message` も何らかの実装を書く
必要がある（`on_message_with_ctx` をオーバーライドしていれば `on_message` は実行時
には呼ばれない）ことをトレードオフとして明記する。`on_message_with_ctx` の既定
実装が `on_message` を呼ぶ構造上、`on_message` を provided にはできない。両方を
provided 化し互いの既定実装で委譲し合う構成にすると、いずれもオーバーライドしない
ハンドラは呼び出しが両者間を無限に往復し、スタックオーバーフローになりうるため
（「何もしない」ではなく、実際には無限再帰による panic に直結する）。

### セッションループの呼び出し変更

`session.rs` の 2 箇所（Text/Binary 受信）を `config.handler.on_message(...)` から
`config.handler.on_message_with_ctx(conn_ctx, ...)` へ変更する（`run_session` の
非公開シグネチャに `conn_ctx: &WsConnContext` を追加。`session` モジュールは非公開
なので破壊的変更に該当しない）。

### `WsSender::closed()` / `is_closed()`

`tokio::sync::mpsc::Sender` が既に提供する `closed().await`（受信側 drop まで待機）／
`is_closed()`（同期判定）へ薄く委譲する（tokio 1.53.1 で利用可能なことを確認済み、
新規依存なし）。

イシュー #727（親 #705）で実装済み（`crates/plugin-websocket/src/handler.rs`。
`session.rs`/`lib.rs` は無変更、`Receiver` drop の既存全終了経路の意味論をそのまま
利用する。統合テストは `crates/plugin-websocket/tests/sender_closed_e2e.rs`）。

## 4. `CloseReason`

```rust
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    ClientClose,
    Eof,
    IdleTimeout,
    Cancelled,
    HandlerClose,
    MessageTooLarge,
    Failed(FailureKind),
    // イシュー #710 で追加。12 節を参照。
    SenderClose,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Io,
    Protocol,
    Handler,
}
```

### 計画からの変更点（`WsError` を運ばない）

当初案は `Failed(WsError)` として実際の `WsError` を `CloseReason` に載せる想定
だったが、次の 3 点は同時に成立しない。

1. `on_close(ctx, reason: CloseReason)` は `reason` を**値渡し**する（`ctx` 越しに
   複数回参照される可能性があるため、参照ではなく値として設計する）
2. `run_session_inner` は `(CloseReason, Result<(), WsError>)` を返し、`Result`
   部分は既存どおり `handle_upgrade` の戻り値（`Result<(), WsError>`）へそのまま
   伝播する（既存テストが `Err(WsError::Protocol(Capacity(_)))` 等を検証している
   契約を変えない）
3. `WsError`（`crates/plugin-websocket/src/error.rs`）は `#[derive(Debug)]` のみで
   **`Clone` を実装しない**（`std::io::Error` / tokio-tungstenite の `Error` を
   ラップするため）

`CloseReason::Failed` が実際の `WsError` を値として運ぶなら、`WsError` を `Clone`
実装するか、`on_close` へ渡す分と `Result` へ渡す分の 2 箇所へ何らかの形で複製する
必要が生じる。前者は `std::io::Error`/tungstenite の `Error` が上流で `Clone` を
実装しない限り不可能、後者は複雑さと `unsafe` 不使用方針に見合わない。

**採用する設計**: `CloseReason::Failed(FailureKind)` として、エラーの**種別**
（`Io` / `Protocol` / `Handler`）のみを運ぶ軽量な `Copy` 値にする。実際の
`WsError`（内部状態・エラー詳細を含む）は `run_session_inner` の戻り値タプルの
`Result<(), WsError>` 側にのみ乗せ、`handle_upgrade` の戻り値へ変更なく伝播する。
`CloseReason` 全体を `Copy + Clone + Debug + PartialEq + Eq` にできるため、
`on_close` 呼び出しと `Result` 返却の両方で同じ判定結果を安価に共有できる。

副次効果として、当初案の「`Failed(WsError)` 経由でエラー内容がハンドラへ渡る」
という経路が構造的に閉じる。`on_close` へはエラーの種別のみが渡り、`WsError` の
`Display`（内部エラーの詳細）はハンドラコードへ一切渡らない。当初案の「`Display`
経由でのみ渡す」というセキュリティ方針より**厳格**な設計になる（8 節）。

**見送った代替案**: `on_close` が `&CloseReason` を参照で受け取り、`Failed(WsError)`
を保持したまま、外側で `Result` へ変換して破壊的に取り出す方式。この場合、
`MessageTooLarge` も同様にエラー内容を保持する必要が生じ（`handle_upgrade` の戻り値
を再構築するため）、`WsError` の participants が増え設計が複雑化する。エラー種別
だけで `on_close` の目的（診断・接続状態管理）は十分満たせるため見送った。

### `MessageTooLarge` を独立 variant にする理由

tungstenite の `Error::Capacity(_)`（`max_message_size`/`max_frame_size` 超過、
tungstenite 0.30 で確認済みの実在 variant）は `Failed(FailureKind::Protocol)` から
**分離**して独立 variant にする（DoS 関連の診断価値が高いため、issue の列挙に明示されて
いる）。分離判定は `WsError::Protocol(tokio_tungstenite::tungstenite::Error::
Capacity(_))` のパターンマッチで行う。

### payload なしの単純 variant

`ClientClose`/`Eof`/`IdleTimeout`/`Cancelled`/`HandlerClose` はいずれも payload
なし（クライアント制御下の Close reason 文字列は一切保持しない。`WsOpenContext::
Debug` と同一のログ・診断への機密混入防止方針）。

### `session.rs` の脱出点対応表

設計文書に表形式で明記し、#705 がそのまま実装できる粒度とする。行番号は
`crates/plugin-websocket/src/session.rs`（v0.4.1、worktree 上で main と無差分）を
実際に読んで確認したもの。`run_session` を包む `loop` 本体・`apply_outcome`
（436-464 行）の両方から抜ける経路をすべて挙げる。

| session.rs の分岐（行番号） | `CloseReason` |
|---|---|
| `Message::Close(_) => break;`（294-296 行） | `ClientClose` |
| `InboundEvent::Message(None) => break;`（250 行、EOF） | `Eof` |
| `InboundEvent::Idle => { ...; return handle_idle_timeout(...); }`（235-238 行） | `IdleTimeout` |
| `race_cancel` が `None` → `handle_cancellation`（215 行・229 行: 受信ループ先頭、outbound 有無で分岐する 2 箇所／244 行: `InboundEvent::Outbound` 分岐内の `ws.send` 送出中／264 行・283 行: `on_message` 実行中（Text/Binary 各分岐）／269-271 行・288-290 行: `apply_outcome` が返した `SessionFlow::Cancelled` を受けて Text/Binary 各分岐から再度 `handle_cancellation` へ分岐） | `Cancelled` |
| `apply_outcome` が返す `SessionFlow::Closed`（`WsOutcome::Close`、268 行・287 行） | `HandlerClose` |
| `outcome?` の `Err(WsHandlerError)`（`on_message` の戻り値、266 行・285 行。現状 `outcome?` で即時 `Err` 化） | `Failed(FailureKind::Handler)` |
| `message?` の `Err`（252 行、tungstenite）: `Error::Capacity(_)` | `MessageTooLarge` |
| `message?` の `Err`（252 行）: `Error::Io(_)` | `Failed(FailureKind::Io)` |
| `message?` の `Err`（252 行）: `Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)`（Close フレームなしの TCP 切断。tokio-tungstenite 0.30 でこの事象が観測される主経路、9 節「#726 実装済みの既知のギャップ」参照） | `Eof`（`Result` は `Err(WsError::Protocol(_))`） |
| `message?` の `Err`（252 行）: `Capacity`/`Io`/`ResetWithoutClosingHandshake` 以外（`ConnectionClosed`/`AlreadyClosed` を含む。次項「`ConnectionClosed`/`AlreadyClosed` の扱い」を参照） | `Failed(FailureKind::Protocol)` |
| `InboundEvent::Outbound` 分岐の `ws.send` 失敗（`Some(Err(err)) => return Err(err.into())`、247 行）: `Error::Io(_)` | `Failed(FailureKind::Io)` |
| `InboundEvent::Outbound` 分岐の `ws.send` 失敗（247 行）: `Io` 以外 | `Failed(FailureKind::Protocol)` |
| `apply_outcome(...).await?` が伝播する `apply_outcome` 内部の `ws.send`/`ws.close` 失敗（`apply_outcome` 内 451 行・459 行の `result?`、呼び出し元の `.await?` 経由。Text 分岐 266 行・Binary 分岐 285 行）: `Error::Io(_)` | `Failed(FailureKind::Io)` |
| 同上: `Io` 以外 | `Failed(FailureKind::Protocol)` |
| `WsSender::close` が enqueue した `OutboundItem::Close`（受信待ち中の `InboundEvent::Outbound` 分岐、またはハンドラ実行中の `run_handler_with_outbound_drain` の 2 経路のいずれかから検出。イシュー #710、12 節参照） | `SenderClose` |

#### `FailureKind::Io` の判別方法（`Error::Io(_)` を明示的に振り分ける）

`FailureKind::Io` は `Protocol` へ埋没させず、`MessageTooLarge`（`Error::
Capacity(_)`）と同じ手法でパターンマッチにより明示的に振り分ける。

`tokio_tungstenite::tungstenite::Error`（tungstenite 0.30）には `Io(std::io::
Error)` variant があり、tungstenite 自身の `error.rs`（38-41 行）が「fatal」と
明記する種別である。接続リセット（`ECONNRESET`）等、実運用で頻発しうる終了経路を
`Protocol` へ一括で丸めると、切断理由の診断価値（本設計の `on_close` 通知の主目的）
が損なわれる。

判定は `message?`（252 行）・`InboundEvent::Outbound` の `ws.send` 失敗（247 行）・
`apply_outcome` 内部の `ws.send`/`ws.close` 失敗（451/459 行、266/285 行の
`.await?` 経由）の 3 箇所すべてで、`?`/`.into()` による早期変換の**前**に
`Result` の `Err` を一度 `match` し、`tungstenite::Error::Io(_)` かどうかを
判定してから `CloseReason` を確定させる（`Capacity(_)` の判定と同一パターン）。
`run_session_inner` が返す `Result<(), WsError>` 側は変更しない
（`From<tungstenite::Error> for WsError`（`crates/plugin-websocket/src/
error.rs`）は無変更のまま常に `WsError::Protocol(_)` を生成し続けてよい。
`MessageTooLarge` も同様に `WsError` 側は `Protocol(Error::Capacity(_))` の
ままであり、`CloseReason` の分類は `WsError` の variant と 1:1 対応しない
既存の設計と整合する）。したがって「`From` 実装を変えない」という制約は
`Io` の判別を妨げない。判別は `run_session_inner` 側のパターンマッチのみで
実現できるため、`error.rs` への変更は不要。

`WsError::Io`（`From<std::io::Error> for WsError`、`crates/plugin-websocket/
src/lib.rs` の `write_racing_cancel` が 101/400/426 応答書き込みで使う、常に
`on_open` 呼び出し以前に発生する経路）とは別物である点に注意する。`run_session_
inner` の脱出点における `Failed(FailureKind::Io)` は、`tungstenite::Error::
Io(_)` の判別結果であり、`WsError::Io` variant 自体が使われるわけではない。

`session.rs` 内で `tungstenite::Error` を `WsError` へ変換する箇所は上記 3 箇所
（252/247/451・459 行）に加えてもう 1 つある。`close_and_drain`（`handle_idle_
timeout`/`handle_cancellation` の共通ヘルパー、569 行付近の `Ok(Err(err)) =>
Err(err.into())`）が、Close 送出後のドレインで 2 次的に失敗した場合の変換
経路である。ここは新たに `Io`/`Protocol` を判別する対象に加えない。本節末尾の
不変条件（「`close_and_drain` 内で発生する二次的なエラー・タイムアウトは、
既に確定した `CloseReason`（`IdleTimeout` または `Cancelled`）を上書きしない」）
により、この経路の `WsError` の内容に関わらず `CloseReason` は既に確定済みの
`IdleTimeout`/`Cancelled` のまま変わらないため、上表への追加行は不要
（`Result<(), WsError>` 側にのみ反映される）。

#### `ConnectionClosed`/`AlreadyClosed` の扱い（Nit 対応）

247 行の `InboundEvent::Outbound` の `ws.send` 失敗が
`tungstenite::Error::ConnectionClosed` / `AlreadyClosed`（相手が既に切断済みの
状態への送信）だった場合も、`Io` ではないため上表の「`Io` 以外」行に従い
`Failed(FailureKind::Protocol)` になる。`ClientClose` へは分類しない。
`ClientClose` は「Close フレームを受信した」（`Message::Close(_) => break;`、
294-296 行）という明示的なプロトコル手続きの完了を表す variant であり、
送信側で「相手が既に閉じていた」ことを検出した経路とは意味が異なるため
（前者はクライアント起点の正常な Close ハンドシェイク、後者はサーバ起点の
push が届け先を失っていたというエラー経路）、混同を避けて別区分に保つ。

### 不変条件（構造で保証、個別 return への散在実装を禁止）

`run_session`（現行 `pub(crate) async fn`）を `run_session_inner`（`(CloseReason,
Result<(), WsError>)` を返す内部関数）へリネームし、上表の全分岐を `break`/早期
`return` ではなく `CloseReason` を伴う値として抜けるよう改修する。新たに
`pub(crate) async fn run_session(..., conn_ctx: &WsConnContext) -> Result<(),
WsError>` を薄い外側ラッパーとして置き、`run_session_inner` の結果から
`on_close(conn_ctx, reason)` を**一度だけ**呼んだ後、`Result` 部分のみを返す。
`handle_upgrade` の呼び出し方は `conn_ctx` 引数追加以外変更しない。

`close_and_drain`（[`handle_idle_timeout`]/[`handle_cancellation`] の共通ヘルパー）
内で発生する二次的なエラー・タイムアウトは、既に確定した `CloseReason`
（トリガとなった条件、`IdleTimeout` または `Cancelled`）を上書きしない。理由は
「トリガ」で確定させ、ドレイン自体の成否は既存どおり `Result<(), WsError>` 側に
のみ反映する（`close_and_drain` は現行どおりタイムアウトを `Ok(())` として吸収する
契約も変えない）。

`on_close` が呼ばれるのは `on_open` が呼ばれた接続に限る（ハンドシェイク失敗・101
送出前キャンセルではどちらも呼ばれない、フェイルクローズの対称性）。呼び出し時点で
outbound の `Receiver` は既に drop 済みのため、ハンドラが `on_close` 内で
`ctx.sender().send(...)` を呼んでも常に `WsSendError` になる契約を明記する。
ランタイム強制終了（プロセス kill 等）でタスクごと drop された場合は保証外
（既知の限界として明記）。同様に、`on_message_with_ctx` 実行中にユーザーハンドラが
panic した場合も `run_session_inner` がアンワインドして `on_close` を呼ぶ前に
関数を抜けるため、この経路も exactly-once 契約の保証外として明記する。

**PR #724 再レビュー指摘対応（P2-1）**: 上記に加えて、`on_open` 自身が panic した
場合も保証外として明記する。`crates/plugin-websocket/src/lib.rs` の
`handle_upgrade` は `config.handler.on_open(handler::WsOpenContext::new(sender,
params))` を（同期呼び出しで、`.await` を挟まず）呼んだ**直後**に
`session::run_session(...)` を呼ぶ。`on_open` の呼び出しは新設の `run_session`
外側ラッパー（`on_close` を呼ぶ側）よりも**前**の段階、すなわち `handle_upgrade`
自身の中で発生するため、`on_open` が panic すると `handle_upgrade` の呼び出し
スタックがそのままアンワインドし、`run_session`（したがって `on_close`）には
到達しない。この経路は「`on_close` が呼ばれるのは `on_open` が呼ばれた接続に
限る」という前提そのものが崩れる（`on_open` の呼び出し自体が完了していない）
ケースであり、`on_message_with_ctx` の panic・プロセス kill と並ぶ第 3 の既知の
限界として明記する。

`on_open` の戻り値は現行 `fn on_open(&self, ctx: WsOpenContext)`
（`crates/plugin-websocket/src/handler.rs`、既定実装は no-op）であり `Result` を
返さない。本設計は `on_open` のシグネチャを変更しない（3 節）ため、`on_open` が
`Err` を返す経路は存在せず、上記の panic 以外に整合させるべき失敗経路はない
（#705 の受け入れ基準を確定する際、この 3 つの既知の限界（`on_message_with_ctx`
panic・プロセス kill・`on_open` panic）を区別して扱う）。

`on_close` 自身が panic した場合は、上記 3 つの既知の限界とは性質が異なる:
`on_close` の呼び出し自体は（`run_session` 外側ラッパーが `on_close(conn_ctx,
reason)` を呼んだ時点で）既に完了しているため、「呼ばれる回数が 1 回以下」という
exactly-once 契約（回数の契約）はこの経路では破れない。panic はその呼び出しの
**戻り**（呼び出し後の後続処理・接続クローズの完遂）に影響するのみであり、
`on_message_with_ctx`/`on_open` の panic（呼び出しそのものが `on_close` へ
到達する前に発生し、呼ばれる回数が 0 になる経路）とは区別する。したがって
`on_close` 自身の panic は既知の限界の第 4 項目としては扱わず、`on_close` 実装が
panic しないことはハンドラ実装者側の責務（`on_message_with_ctx` の panic-safety
契約と同様、`.claude/rules/coding-rust.md` の「panic はライブラリ境界を越えさせ
ない」方針の対象）として明記する。

`crates/core/src/plugin.rs` の `let _ = fandhe_backend_plugin_websocket::
handle_upgrade(...)` は変更不要（切断理由はプラグイン内部で `on_close` を通じて
観測できるため、影響範囲想定から core を明示的に除外する）。

## 5. `WsConnContext` が `WsSender` を保持する副作用

`WsConnContext` は `on_message_with_ctx`/`on_close` の呼び出しにわたって
セッションの生存期間中保持されるため、その内部に持つ `WsSender`（`mpsc::Sender`
のクローン）はセッションが終了するまで decrement されない。

現行 `session.rs` の `InboundEvent` 合流ロジックには「全 `WsSender` クローンが drop
済みでチャネルが閉じた場合はそのイベント源を無効化する」分岐
（`Some(Either::Right(None)) => { drop(outbound.take()); continue; }`）がある。
これは現行では `on_open` がその場で受け取った `WsOpenContext` の `WsSender` を
drop し、他に clone を保持していない場合に到達可能だった。

`run_session`/`run_session_inner` が `conn_ctx: &WsConnContext` を通じて
`WsSender` のクローンをセッション終了まで保持する設計に変わることで、**この分岐は
セッションが走っている間は到達不能になる**（`conn_ctx` 自身が保持するクローンが
常に生きているため、全クローンが drop されることはセッション終了後にしか起こらない）。
`WsSender::closed()`/`is_closed()` の意味論はこの変更の影響を受けない
（受信側 `Receiver` はセッション終了時に drop される点は不変）。

この分岐は削除せず、防御的コードとして維持する（将来 `WsConnContext` の保持方式が
変わった場合の安全網。到達不能であることをコードコメントに明記する）。

## 6. 送信キュー消化方式の設計指針（#706 の前提）

> **実装済み（#704 の PR #725 レビュー指摘対応で前倒し実装）**: 当初は #704
> （本ドキュメント）を設計のみ、実装は #706 に分離する計画だった。しかし
> `on_message_with_ctx` の「正常な使い方（接続自身への push）」が容量超で
> 停止する点が ai-review（codex）から P1 として指摘され、doc への制約明記
> だけでは gate を解除できないと判断されたため、本節の手順・保証をそのまま
> `crates/plugin-websocket/src/session.rs` の
> `run_handler_with_outbound_drain`（新設の非公開ヘルパー）として #704 の
> スコープ内で実装した。以下の「現状」節・手順・保証は設計時点の記述を
> そのまま残すが、コードは既にこの設計を反映済みである（#706 は本節の
> 実装が完了したことをもってクローズ対象になる。単体テストは
> `crates/plugin-websocket/src/session.rs` の
> `on_message_with_ctx_self_send_beyond_capacity_does_not_deadlock`、
> e2e テストは `crates/plugin-websocket/tests/handler_push_ordering_e2e.rs`
> で受け入れ基準 1〜3 を検証する）。

現状（設計時点）: `race_cancel(cancel, config.handler.on_message_with_ctx(ctx, msg))` は単独
await であり、この間 `WsSender` の outbound チャネルを消費するものが誰もいない
（受信ループ先頭の race は次の反復まで戻ってこない）ため、`on_message_with_ctx`
内で容量（既定 8）超の `send` を呼ぶとデッドロックしていた。

**PR #724 レビュー指摘対応（P1/P2、まとめて再構成）**: 当初案は「ハンドラ実行中に
送出された push は構造的に Reply より先にワイヤへ出る」という主張から出発し、
Err・Close・別タスク・スナップショット前後の派生ケースを都度追記して修正を重ねた
結果、記述量が増えるほど食い違いが増える状態になった。本節は追記ではなく、以下の
**手順**を先に確定し、保証はその手順から直接導ける 1 文だけに絞る（対象外の push
の挙動は個別に記述しない）。

### 手順（`on_message_with_ctx` 呼び出しを包む内側ループ、1 反復分）

1. ハンドラ Future が `Poll::Ready(outcome)`（`outcome: Result<WsOutcome,
   WsHandlerError>`）を返すまで、`cancel`（最優先）とハンドラ Future・outbound
   到着を race する既存方針（後述）でポーリングを続ける。到着した push はその都度
   送出し、Close 指示が届いたらハンドラ Future を drop して `SessionFlow::SenderClose`
   で終了する。
2. `outcome` に応じて送信キューを排出する（**#710・PR #736 レビュー指摘対応で改訂**、
   後述の注記参照）。
   - **継続経路**（`Ok(WsOutcome::Reply(_))`、`drain_before_reply`）: 受信側は
     閉じずに、`try_recv()` を最大でチャネル容量（`DEFAULT_OUTBOUND_CAPACITY`。
     `run_handler_with_outbound_drain` が引数で渡す 1 か所のみで、#709 で設定値
     に置き換える）回行い、取り出した push を到着順に `ws.send()` で送出する。
     `Empty` で打ち切り、残りはキューに残したまま外側ループに任せる。Close 指示が
     見つかれば `Reply` を破棄して `SessionFlow::SenderClose` で終了する。容量回を
     使い切った・空になった時点で close が確定済みなら、Close 指示まで取り出し続けて
     同様に終了する（close 確定後は新規 enqueue が `Err` のため件数は容量で有界。
     継続経路では封鎖しないので、確定シグナルは `close()` によるものと判定できる）。
   - **終了経路**（`Ok(WsOutcome::Close)`・`Err(WsHandlerError)`、`flush_outbound`）:
     送信キューを封鎖し（以後の `send`/`close` は `Err`）、受信側を閉じてから、
     `try_recv()` が `Empty`/`Disconnected` を返すまで取り出して到着順に送出する。
     Close 指示が見つかれば `outcome`（`Err` を含む）を破棄して
     `SessionFlow::SenderClose` で終了する。排出全体を `close_grace` で有界化する。
3. `outcome` を反映する。`Ok(WsOutcome::Reply(messages))` なら `messages` を送出して
   外側ループの次の反復へ進む（`SessionFlow::Continue`）。`Ok(WsOutcome::Close)` なら
   Close フレームを送出してセッションを終了する（`SessionFlow::Closed`）。
   `Err(err)` なら `Failed(FailureKind::Handler)` を `CloseReason` として確定し、
   `Err(WsError::Handler(err))` を返して終了する（4 節の対応表の該当行と整合）。

上記手順中のエラー・キャンセルの扱い（各 1 行）:

- 排出中の `ws.send()` 失敗: 以後の項目と Close フレームは送出しない。継続経路・
  `Ok(WsOutcome::Close)` では外側ループの `InboundEvent::Outbound` 分岐と同じ扱い
  （4 節の対応表）。`Err` 経路では元のハンドラエラーを上書きせず
  `Failed(FailureKind::Handler)` のまま終了する。
- 排出中の `cancel` 発火: cancel 最優先を維持し、当該 `Future` を打ち切って
  `handle_cancellation`（`Cancelled`）へ分岐する（どの経路でも同じ）。
- 終了経路の `close_grace` 超過: 残りを諦め、`Ok(WsOutcome::Close)` は Close フレームを
  送らずに終了、`Err` は `Failed(FailureKind::Handler)` で終了する。排出中に Close 指示が
  見つかった場合、その Close ハンドシェイクは排出と同じ期限を引き継ぐ（排出・Close 送出・
  応答待ちの合計が `close_grace` 以内）。
- close 確定後の push・返信の送出: 12 節「close 確定後の有界化」の期限で打ち切り、
  `SenderClose` で終了する（close 未確定時の送出は期限なしのまま）。

> **排出方式の改訂経緯（PR #736）**: 当初は両経路とも `try_recv()` を固定回数
> （容量）行っていたが、満杯キューで `reserve()` 待ちだった `WsSender::close` が排出中に
> 確定すると回数から漏れ、`close()` が `Ok` を返したのに Close が送られなかった
> （Cursor Bugbot）。`Empty` まで無制限に取り出す修正は、送り続ける別タスクがいると
> 終わらず（codex P0）、`Empty` 判定から受信側 drop までの間に確定した Close も失った
> （codex P1）。回数を調整するのではなく、送信キューを封鎖してから排出する終了経路と、
> 閉じずに容量回で打ち切る継続経路に分けることで、両方を構造的に解消した（終了経路が
> 取りこぼさず有界である根拠は 12 節「終了経路の排出（送信キューを封鎖してから排出）」）。

上記手順を反映した outcome 別の対応表（4 節「outcome? の Err」行・
`apply_outcome` 行と整合させたもの）:

| `outcome` | ステップ 2（排出） | ステップ 3（反映） |
|---|---|---|
| `Ok(WsOutcome::Reply(messages))` | 受信側を閉じずに容量回まで（close 確定済みなら Close 指示まで）。Close 指示が見つかれば `SenderClose` で終了 | `messages` を送出、セッション継続 |
| `Ok(WsOutcome::Close)` | 封鎖してから `try_recv()` が `Empty`/`Disconnected` まで。Close 指示が見つかれば `SenderClose` で終了 | Close フレームを送出、セッション終了 |
| `Err(WsHandlerError)` | 同上 | `Failed(FailureKind::Handler)` で終了 |

内側ループがハンドラ完了を待つ間の「cancel（最優先）→ (ハンドラ完了 |
outbound 到着)」の race 自体は既存方針（`race2_alternating` 型の交互化は不要、
ハンドラ Future は 1 回しか完了しない単発イベントのため固定順ポーリングでも
飢餓しない。ただし cancel は最優先を維持）を変えない。outbound チャネルが
閉鎖済み（`recv()` が `None`）の場合の扱い（そのイベント源を無効化しセッション
自体は継続）も既存の外側ループと同じ振る舞いを内側ループに適用する（5 節で
述べたとおり `conn_ctx` がセッション生存中は `WsSender` を保持するため、この
分岐はセッション実行中は到達不能・防御的コードとして維持）。

`idle_deadline` は本内側ループ・排出ステップ中は更新しない（既存の「クライアント
から実際にフレームを受信したときのみ延長」契約を変えない。outbound 送出は
アイドル判定に影響しない）。

### 保証（手順から直接導ける 1 文）

**継続経路では排出開始時点ですでにチャネルへ格納済みだった push が `Reply` より
先に送出され、`Reply` の送出を始める前に close が確定していれば `Reply` は送出されず、
終了経路では封鎖より前に `WsSender::send`/`close` が `Ok` を返した
項目が（`close_grace` 超過・cancel・排出中の送出失敗（以後の項目と Close フレームも
送出されない）で打ち切られない限り）すべて Close 送出・終了より先に処理され、封鎖
より後の呼び出しは `Err` を返す。** ここでの `Reply` に対する「close 確定」は
`close()` が `Ok` を返す時点（確定シグナルの送信）を指す（12 節で使うロック区間での
フラグ更新とは区別する）。

継続経路で排出開始後に格納された push と `Reply` の相対順序、および `Reply` の送出
開始後に確定した close と `Reply` の関係のみ不定とする（後者の `Reply` 送出は close
確定の観測から `close_grace` で打ち切る）。

### #706 への引き渡し事項（#704 の PR #725 で前倒し実装済み）

上記の手順・保証・不定の 3 点に沿って実装し、テストで固定すること:

- 手順（ステップ 1〜3、エラー・キャンセルの扱い）をそのまま実装する
- 保証（排出開始時点で格納済みの push が Reply/Close より先に送出される）を
  実接続で検証する。十分条件のテスト観点として、ハンドラ Future が
  `Poll::Ready` を返す**前**に `WsSender::send(...).await` が完了した push
  （送信元がハンドラ自身か別タスクかを問わない）を用いてよい（この完了時刻
  条件は排出開始時点での格納を成立させる十分条件であり、保証の定義そのもの
  ではない）
- `outcome` が `Err(WsHandlerError)` の場合の排出方針は #710・PR #736 で
  改訂済み（12 節参照）: 排出自体は `outcome` の `Ok`/`Err` に関わらず行い、
  Close 指示が見つからなかった場合に限り `Failed(FailureKind::Handler)` で
  終了することを確認する
- 上記以外（保証の対象外）の push については、順序が不定であることの確認に
  留め、特定の順序を新たに固定しない

> **実装状況**: 上記 4 点はすべて `run_handler_with_outbound_drain`
> （`crates/plugin-websocket/src/session.rs`）として #704 のスコープ内で
> 実装済み。ただし `Failed(FailureKind::Handler)`（4 節の `CloseReason`
> 拡張）自体は #705 のスコープであり未実装のため、現時点ではハンドラ
> エラー時は既存契約どおり即時 `Err(WsError::Handler(_))` で終了する
> （`on_close` 通知はまだ発生しない）。#706 は本節の実装が完了したことを
> もってクローズ対象になる（残作業があれば #706 側で追跡する）。

## 7. バージョン方針

[`versioning-policy.md`](./versioning-policy.md) 2 節「pre-1.0（0.x）期の規則」を
引用: `z` は「非破壊追加・バグ修正」。

本設計の変更点はすべて非破壊:

- `WsMessageHandler` への追加は 2 メソッドとも provided（trait への**必須**メソッド
  追加のみが破壊的、3 節）
- 新規公開型 `WsConnContext`・`WsConnId`・`CloseReason`・`FailureKind` の追加
- 既存 `#[non_exhaustive]` 型 `WsOpenContext` へのフィールド・アクセサ追加
  （非公開フィールドのため非破壊）
- `WsSender` への新規メソッド（`closed()`/`is_closed()`）追加
- `handle_upgrade`・`WebSocketConfig` の公開シグネチャは無変更
- `session.rs`（非公開モジュール）の内部リファクタリング（デッドロック修正、
  6 節・#706）はそもそも公開 API 面の変更ではない

**直接の先例**: `on_open`/`WsOpenContext`/`with_path_pattern`（#671/#675/#676）は
同じ「provided メソッド追加・`#[non_exhaustive]` 型への追加」パターンで、
`CHANGELOG.md` の `## [0.4.1]` に「BREAKING CHANGE はありません」として実際に
収録済み。

**結論: 0.4.2**（非破壊）。0.5.0（破壊的変更）は採らない。

## 8. セキュリティ考慮事項（OWASP Top 10 観点）

- **A04 安全でない設計 / リソース枯渇対策の維持**: `WsConnId` 発行に `unsafe` を
  使わない（`AtomicU64`）。`WsConnContext`/`WsOpenContext` の `Debug` はパスパラ
  メータ（攻撃者制御下の URL セグメント）を出力しない契約を維持・拡張する。
  `CloseReason` はクライアントの Close フレーム reason 文字列（任意 UTF-8、攻撃者
  制御下）を一切保持しない設計とし、ログインジェクション・機密混入経路を新設しない。
  さらに `Failed(FailureKind)` が種別のみを運ぶ設計（4 節）により、`on_close` へ
  渡る情報は当初案より厳格に絞られ、`WsError` の内部状態（`io::Error`・tungstenite
  の `Error` が保持しうる詳細）がハンドラコードへ一切渡らない
- **A05 セキュリティの設定ミス / DoS 対策の後退防止**: `on_message_with_ctx` 実行中
  の outbound 消化は `max_message_size`/`max_frame_size` の既存強制（tungstenite
  側、ハンドラ呼び出し前）を変更しない。`idle_timeout` の非更新契約（outbound 送出
  でリセットしない、Issue #175 由来）を明示的に維持する。送信キュー消化方式の追加
  により新たな無制限バッファ化・無期限ブロックは発生しない（既存の bounded mpsc
  容量 8 のまま）
- **A08 サプライチェーン**: 新規外部依存を追加しない（`AtomicU64` は std、
  `WsSender::closed()`/`is_closed()` は既存 tokio 依存の既存 API に委譲）
- **フェイルクローズ**: `on_close` はハンドシェイク未成立（101 未送出）の接続では
  呼ばれない（`on_open` との対称性）。`Failed` 系 `CloseReason` はエラー種別
  （`FailureKind`）のみを渡し、内部状態やクライアント入力の生データを新たに露出しない

## 9. 引き渡し事項（後続 #704〜#707 向け）

- **#704**: `WsConnId`/`WsConnContext`/`WsOpenContext::conn_id` の実装、`session.rs`
  の呼び出し口変更（`on_message` → `on_message_with_ctx`、ライフタイム注記は 3 節を
  参照）、ユニットテスト。**PR #725 レビュー指摘対応として #706（6 節）の送信キュー
  消化実装も前倒しでスコープに含めた**（`run_handler_with_outbound_drain`）
- **#705**: `CloseReason`/`FailureKind`・`run_session_inner`/`run_session` 分割・
  `on_close` 呼び出し（4 節の脱出点対応表が挙げる全経路で exactly-once。panic・
  プロセス kill 由来の 3 つの既知の限界（4 節「不変条件」参照）は対象外）・
  `WsSender::closed()`/`is_closed()`・各終了経路ごとの実接続テスト。
  `crates/core/src/plugin.rs` は変更不要である旨を明記済みなので影響範囲から
  除外してよい。**#705 はその後 3 分割された**: #726（`CloseReason`/
  `FailureKind` の定義・`run_session_inner` への分割・脱出点対応表の実装。
  **実装済み**、`docs/design/ws-connection-context-and-close.md` 本節参照）・
  #727（`WsSender::closed()`/`is_closed()`）・#729（`on_close` 呼び出し、#726 に
  依存。**実装済み**。`WsMessageHandler::on_close(&self, ctx: &WsConnContext,
  reason: CloseReason)`（既定 no-op）を追加し、`session::run_session`
  ラッパーが `run_session_inner` の戻り値を分解してちょうど 1 回呼ぶ構成
  とした。Issue 本文は `crates/core/src/plugin.rs` も影響範囲に挙げていた
  が、本節の設計方針（コアは変更しない）に従い `crates/core` は無変更のまま
  実装した。ハンドシェイク段階の失敗（400/426・101 送出前キャンセル）は
  `on_open` と対称に `on_close` の対象外とし、失敗の詳細は
  `handle_upgrade` の戻り値からのみ観測できる契約を維持した）
- **#726 実装済みの既知のギャップ（PR #731 レビュー指摘対応で解消済み）**:
  4 節の脱出点対応表は当初 `InboundEvent::Message(None)`（EOF）→ `Eof` の
  みを明記し、tokio-tungstenite 0.30 が Close ハンドシェイクなしの TCP
  切断を返す主経路である `Err(Protocol(ResetWithoutClosingHandshake))` は
  `Failed(FailureKind::Protocol)` へ倒していたため、`Eof` が通常の切断
  経路で実質到達不能になっていた。`CloseReason::Eof` の doc が定義する
  事象（Close ハンドシェイクなしの切断）と `ResetWithoutClosingHandshake`
  が 1:1 対応することから、`SessionFailure::recv`（`session.rs`）で
  `ResetWithoutClosingHandshake` を `Eof` へ分類するよう変更した（`Result`
  側は読み取り失敗を示す `Err(WsError::Protocol(_))` のまま。上記の脱出点
  対応表・4 節参照）。`ws.next()` が実際に `None` を返す経路
  （`ConnectionClosed`/`AlreadyClosed` 到達後の fused 呼び出し等）も
  引き続き `Eof` へ分類され、この場合は `Result` が `Ok(())`。`Eof` は
  1 つの意味論（Close なし切断）に 2 つの到達経路（`Result` が `Ok`/`Err`
  のいずれか）を持つ variant として確定した
- **#706**: 送信キュー消化の内側レース実装・順序契約のテスト固定（6 節、
  **#704 の PR #725 で前倒し実装済み**。e2e テスト
  `crates/plugin-websocket/tests/handler_push_ordering_e2e.rs` を追加し
  受け入れ基準 1〜3 を固定した。クローズ対象）
- **#707**: 2 クライアント同時接続 e2e（前提: #704/#705 完了後。#706 は前倒し
  実装済みのため実質前提済み）。**実装済み**: `crates/plugin-websocket/tests/
  multi_client_e2e.rs` を新設。1 つの `WsMessageHandler` インスタンス（本節が
  示す「案 B」の実装例、`Mutex<HashMap<WsConnId, _>>` で接続単位状態を管理）を
  `/devtools/page/{id}` パターンへ共有 `Arc` として登録し、2 クライアントが
  同時接続した状態で (1) 接続ごとの状態（受信メッセージカウンタ）とサーバー
  起点 push が他方の接続へ混入しないこと（順序バリア + Close ドレイン時の
  厳密検査の 2 段による否定検証。PR #734 レビュー指摘対応で、Close 送出後の
  ドレインも Close フレーム・EOF 以外を受信したら即座に検出する
  `drain_expect_only_close` へ強化し、セッション終了までの全期間を検出範囲に
  含めた）、
  (2) 一方をクライアント Close で切断すると、その接続についてのみ `on_close`
  が `CloseReason::ClientClose` でちょうど 1 回呼ばれ、もう一方は送受信・push
  を継続できること、(3) 同じ検証を EOF（Close ハンドシェイクなしの drop）
  経路でも確認すること、をそれぞれ 1 テストずつ（計 2 テスト）で検証した。
  インメモリ `tokio::io::duplex` のみを使い OS 依存 API がないため 3 OS の
  `ci.yml` test matrix で差異なく通る想定（`#[tokio::test(flavor =
  "multi_thread", worker_threads = 2)]` で 2 セッションを実際に並行実行）

実装着手時に行うこと（各実装イシューへの引き渡し事項）:

- `CHANGELOG.md` に `## [Unreleased]` 節を新設し 0.4.2 として記録すること
- `docs/api/plugin-config-api.md` 2.1 節（`WsMessageHandler`/`WebSocketConfig` の
  API サマリ）を新 API へ追随させること
- `examples/with-websocket` を新 API のショーケースへ更新するか判断すること
  （更新しない場合は既存 `on_message` 実装のままで後方互換確認用として維持する旨を
  明記する）

## 10. スコープ外・再検討条件

- **案 A（`with_handler_factory`）を案 B 上の糖衣として再検討する条件**: 接続数が
  非常に多く、利用者側の `Mutex<HashMap<WsConnId, _>>`（案 B の想定実装パターン）の
  ロック競合が実測でボトルネックになった場合。現時点では新規 Issue を起票しない
  （ユーザー承認前提の [[out-of-scope-tracking]] に従い、記録のみに留める）
- `on_close` へ完全な `WsError` を渡す設計は 4 節で見送った（`FailureKind` による
  種別のみの通知を採用）。将来、診断目的で詳細なエラー内容が必須になった場合は
  再検討する
- `docs/api/plugin-config-api.md` 2.1 節への追随・`CHANGELOG.md` `[Unreleased]`
  節の新設・`examples/with-websocket` の更新判断は 9 節に記載のとおり実装イシュー
  （#704〜#707）のスコープとし、本イシューでは実施しない

## 11. 利用例（CDP 風ハンドラの最小スケッチ、非拘束の説明用）

```rust
struct SessionRegistry {
    sessions: Mutex<HashMap<WsConnId, SessionState>>,
}

impl WsMessageHandler for CdpHandler {
    fn name(&self) -> &'static str {
        "cdp"
    }

    fn on_message(&self, msg: WsMessage) -> BoxFuture<'_, Result<WsOutcome, WsHandlerError>> {
        // on_message_with_ctx をオーバーライドしているため実行時には呼ばれない
        // （3 節のトレードオフ）。フォールバックとして最小実装を置く。
        Box::pin(async move { Ok(WsOutcome::Reply(vec![msg])) })
    }

    fn on_open(&self, ctx: WsOpenContext) {
        self.sessions.lock().unwrap().insert(
            ctx.conn_id(),
            SessionState::new(ctx.param("id").map(str::to_string)),
        );
    }

    fn on_message_with_ctx<'a>(
        &'a self,
        ctx: &'a WsConnContext,
        msg: WsMessage,
    ) -> BoxFuture<'a, Result<WsOutcome, WsHandlerError>> {
        Box::pin(async move {
            // ctx.conn_id() をキーに self.sessions から購読状態を読み書きする
            Ok(WsOutcome::Reply(vec![msg]))
        })
    }

    fn on_close(&self, ctx: &WsConnContext, _reason: CloseReason) {
        self.sessions.lock().unwrap().remove(&ctx.conn_id());
    }
}
```

`Arc<Mutex<HashMap<WsConnId, SessionState>>>` を共有ハンドラに持たせ、`on_open`/
`on_message_with_ctx` で `ctx.conn_id()` をキーに購読状態を登録、`on_close` で
エントリを削除する最小スケッチであり、案 B の運用像を具体化する目的のみに使う
（コンパイル可能であることは保証しない説明用コード）。

## 12. #710: `WsSender::close` によるサーバー起点の Close

親 #708「サーバー起点で任意タイミングに Close を送れる WebSocket API」対応。
`on_message` の戻り値 `WsOutcome::Close`（`HandlerClose`）は `on_message` の
`Future` 内からしか呼べず、`on_open` で spawn したタスク等、ハンドラの外からは
接続を閉じられなかった。`WsSender::close(code, reason)` を追加してこれを解消する。

### 順序保証と競合排除（reserve + 単一 Mutex）

「フラグを確認してから `tx.send().await`」という素朴な実装は、確認と enqueue の
間に別タスクの `close` が割り込むと、送信済みメッセージが Close の後ろへ積まれて
破棄される（`send` は `Ok` を返すのに実際には届かない）静かなデータ欠落を招く。

これを避けるため、`WsSender` は次の構造を取る。

- 送信キューを流れる要素を `OutboundItem { Message(WsMessage), Close { code,
  reason } }` に統一し、`send`/`close` の両方が同一の bounded mpsc へ enqueue
  する（2 本の別チャネルに分けてマージ順序を別途保証するより、単一チャネルの
  FIFO 特性だけで順序保証を得る方が構造的に単純）。
- `closing: Arc<std::sync::Mutex<bool>>` を全 clone で共有し、`commit`（`tx.
  reserve()`/`try_reserve()` で確保した `Permit` を受け取る非公開ヘルパー）が
  「フラグ確認 → （close 呼び出しなら）フラグを立てる → `Permit::send`（同期）」
  の 3 手順を同一ロック区間で行う。ロック保持中に `.await` しない
  （`.claude/rules/coding-rust.md`）ため、`reserve()` の非同期待機はロックの
  **外側**で完了させ、ロック内は同期処理のみに限定する。

この結果、`send` が `Ok` を返したメッセージは必ず Close より前に enqueue され、
Close 確定後に enqueue を試みた `send` は必ず `Err` になる。

### 検証（RFC 6455 7.4 節・5.5 節）

- close code は tungstenite の `CloseCode::from(code).is_allowed()` で検証する
  （`<1000`・`1005`・`1006`・`1015`・予約域 `1016..=2999`・`>=5000` を拒否）。
  tungstenite 自身は送信する Close フレームの code を検証しないため、API 境界
  （`WsSender::close`）での入力検証が必須（`.claude/rules/security.md`）。
- reason は制御フレームの payload 上限 125 バイトから close code 分の 2 バイトを
  引いた 123 バイトまでを許容する。UTF-8 は `&str` の型で保証される。
- いずれかの検証に失敗した場合、close 済みフラグは立てない（不正な要求で以後の
  `send`/`close` を巻き込んで失敗させない）。

### 2 回目以降の `close`・ハンドラ実行中の `close`

- 検証を通過した後、既に close 済み・セッション終了済みであれば
  `WsCloseError::Closed` を返す（`send` の close 後の挙動と一貫させる、
  フェイルクローズ）。
- ハンドラ（`on_message_with_ctx`）実行中に別タスクから `close` が呼ばれた場合、
  `run_handler_with_outbound_drain`（6 節）が outbound 消化中に `OutboundItem::
  Close` を検出し、ハンドラの `Future` を drop して即座に Close ハンドシェイクへ
  分岐する（#499 の中断安全性契約の範囲内。ハンドラの完了を待たない理由は、
  close 後は送信できず待つ意味がないうえ、待てば Close 送出が遅れて有界性を
  損なうため）。ハンドラが返す `WsOutcome::Reply`/`Close` は破棄される（RFC 6455
  5.5.1 節: Close フレームの後にデータフレームを送れない）。
- 戻り値を破棄する条件は「その送出を始める前に close が確定していること」に一本化
  する（ハンドラ実行中か、完了後の送信キュー排出中かを区別しない。PR #736 codex P1
  指摘対応）。排出中に close が確定し、Close 指示が容量回の排出の外に積まれた場合も、
  Reply 送出の直前に確定を確認して Close 指示まで取り出し、Reply を送らない。`Reply`
  の送出を始めた後に確定した場合は `Reply` の残りが送出されうる（close 確定の観測から
  `close_grace` で打ち切る）。

### ハンドラ自身が `close` を呼んだ直後に `Err`/`WsOutcome::Close` を返す場合（PR #736 レビュー指摘対応）

「ハンドラ実行中に別タスクから `close` が呼ばれた場合」（上記）とは別に、
ハンドラ自身が `on_message_with_ctx` の中で `ctx.sender().close(...).await`
を呼び、その直後に（追加の `await` を経ずに）`Err`/`Ok(WsOutcome::Close)` を
返すケースがある。この場合、`close` の enqueue とハンドラ自身の完了が同じ
`handler_fut` の実行内で連続して起こるため、`run_handler_with_outbound_drain`
（6 節）の外側 race が outbound 側（`Right`）より先にハンドラ側（`Left`）を
Ready と観測しうる。

当初の実装は、ハンドラが `Err` を返した場合に排出ステップ自体を丸ごと
省略していたため、この経路では既に enqueue 済みの `OutboundItem::Close`
（および Close より手前に enqueue 済みの push）が排出されずに無言破棄されて
いた（本節冒頭「順序保証と競合排除」の「`send` が `Ok` を返したメッセージは
必ず Close より前に enqueue され」という保証が、enqueue 後にハンドラが
`Err` で終了する経路では「実際に送出される」ところまで届いていなかった）。

`Err` を `WsOutcome::Close` と同じ終了経路として扱い、ハンドラの結果を
反映する**前**に送信キューを排出する（6 節の手順 2。Close が見つかればそこで
`SenderClose` として終了し、見つからなければハンドラエラーで終了する）。
`WsSender::send`/`close` が呼び出し元へ `Ok` を返した時点で確定した「届ける」
契約を、ハンドラ自身の以後の終了結果で覆さない。

### 終了経路の排出（送信キューを封鎖してから排出、PR #736 レビュー指摘対応）

手順（`session.rs` の `flush_outbound`）:

1. `WsSender::seal_for_session` で送信キューを封鎖する。`WsSender::commit` と同じ
   `closing` の `Mutex` のロック区間で封鎖状態を立て、`closed_signal` で満杯キューの
   待機者を解放する。以後の `send`/`close` は `Err` になる。
2. `Receiver::close()` で受信側も閉じる。
3. `try_recv()` が `Empty`/`Disconnected` を返すまで取り出し、`Message` は到着順に
   送出する（送出は cancel と race）。`Close` を見つけたら `SenderClose` で終了する。
4. 手順 3 全体を `close_grace` で有界化する。

**保証**: 手順 1 より前に `send`/`close` が `Ok` を返した項目は、`close_grace` 超過・
cancel・送出失敗（以後の項目と Close フレームも送出されない）で打ち切られない限り
すべて送出（Close 指示は `SenderClose` として処理）され、手順 1 より後の呼び出しは
`Err` を返す。

この保証と有界性は tokio の受信側の起床挙動に依存しない（下流の lockfile が古い
tokio 1.x を解決しても成り立つ）:

- 封鎖と `commit` の `Permit::send` は同じ `Mutex` のロック区間で行われるため、封鎖
  より前に `Ok` を返した項目は手順 3 の開始時点ですでにキューに入っている。permit を
  確保済みでも確定前の送信者は封鎖後に `Err` になり、項目を積まない。したがって
  `try_recv()` の `Empty` で打ち切っても取りこぼさず、`recv().await` で待つ必要がない
  （tokio 1.52.3 以前には、permit を割り当てた直後に受信側を閉じると待機中の受信側が
  起こされない経路があり、待つ実装は `close_grace` まで止まりうる）。
- 封鎖後は新しい項目が積まれないため、取り出す件数はチャネル容量以下で構造的に有界に
  なる（送り続ける別タスクがいても終わる）。
- セッション側は封鎖のロック保持中に `.await` せず、ほかのロックも取らないため、
  `commit` とのデッドロックは起こらない。

### 排出しない終了経路の扱い（PR #736 レビュー指摘対応）

cancel・idle timeout・クライアント Close・EOF・受信/送信エラーの各経路は排出を行わない
（既存契約）が、受信側を drop する前に必ず封鎖する。

手順（`session.rs`）:

1. `run_session_inner` は受信側を `OutboundGuard`（受信側と封鎖用の `WsSender` を
   束ねる）に持たせ、下位の関数へは `&mut OutboundGuard` で渡す。
2. 途中で受信側を手放す箇所は `OutboundGuard::release`（封鎖 → drop）を使う。
3. ガードの `Drop::drop` で封鎖する。`Drop::drop` はフィールドの drop より先に実行
   される（言語仕様）ため、早期 return・future の drop を含むすべての脱出で
   「封鎖 → 受信側の drop」の順になる。

**保証**: 受信側が drop された後に `send`/`close` が `Ok` を返すことはない（permit 確保後・
確定前に drop された場合も、確定は `Err` になる）。

`on_close` の呼び出し回数（ちょうど 1 回）と `CloseReason` の分類は変えない。

### close 確定後の有界化（Cursor Bugbot 指摘対応、PR #736）

`close()` が確定しても、キューには先行する push が残りうる。受信を止めたクライアント
相手ではその送出が止まり、Close ハンドシェイクに到達しなかった。

手順（`session.rs`）:

1. `CloseBound` が `WsSender` の close 確定シグナルを購読し、確定を初めて観測した時刻
   から `close_grace` 後を期限とする。
2. push・返信の送出（`send_bounded`。外側ループ・ハンドラ実行中・継続経路の排出・
   `WsOutcome::Reply`）は、cancel（最優先）・送出・期限の順で race する。期限を過ぎたら
   `ws` に書き込まずに drop し、`close_and_drain` の期限超過と同じく `SenderClose` +
   `Ok(())` で終了する。
3. Close 指示を取り出したら、同じ期限で `close_and_drain` を行う。終了経路の排出中に
   見つかった場合は、排出の期限と早い方を使う。

**保証**: `close()` が `Ok` を返したら、セッションは close 確定の観測（通常は close 確定の
直後）から `close_grace` 以内に Close ハンドシェイクを終えるか接続を打ち切る。その間に
世代キャンセル・idle timeout が先に発火した場合は、その経路の契約（発火時点から
`close_grace`、終了理由 `Cancelled`/`IdleTimeout`）に従う。

close 未確定時の push・返信の送出には期限を設けない（既存の挙動）。

送出中の future を drop しても安全な根拠: tokio-tungstenite 0.30 の `Sink::start_send`
はフレームを丸ごと tungstenite の書き込みバッファへ積み、`poll_flush` がそれを書き出す
だけなので、drop は未投入のフレームを捨てるか投入済みのフレームをバッファに残すかの
どちらかで、`ws` の状態は壊れない（期限超過後は `ws` へ書き込まない）。

### `is_closed()` の意味の変更

`WsSender::is_closed()` は「`closing` フラグ、または受信側 `Receiver` の
drop」のいずれか早い方で `true` になる。`close()` 確定直後は受信側がまだ
drop されていなくても `true` を返しうる点が、`closed()`（受信側 drop まで
完了しない）との違いである。`docs/api/plugin-config-api.md` の記述もこれに
追随させる。

### スコープ外（変更しない）

- 既存の `WsOutcome::Close`（`HandlerClose` 経路）がキュー済みの push を捨てる
  挙動の是正は本イシューの受け入れ基準に含まれない（#711 で是正済み）。
- 送信キューの容量設定・`try_send`（#709）は本イシューの対象外。`OutboundItem`・
  `commit` は `try_reserve()` を使う `try_send` をそのまま載せられる形にしてある。

[`WsOpenContext`]: ../../crates/plugin-websocket/src/handler.rs
[`WsSendError`]: ../../crates/plugin-websocket/src/handler.rs
[`DEFAULT_OUTBOUND_CAPACITY`]: ../../crates/plugin-websocket/src/handler.rs
[`race2_alternating`]: ../../crates/plugin-websocket/src/session.rs
[`apply_outcome`]: ../../crates/plugin-websocket/src/session.rs
[`handle_idle_timeout`]: ../../crates/plugin-websocket/src/session.rs
[`handle_cancellation`]: ../../crates/plugin-websocket/src/session.rs
