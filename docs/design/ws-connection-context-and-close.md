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
| `message?` の `Err`（252 行）: `Capacity`/`Io` 以外（`ConnectionClosed`/`AlreadyClosed` を含む。次項「`ConnectionClosed`/`AlreadyClosed` の扱い」を参照） | `Failed(FailureKind::Protocol)` |
| `InboundEvent::Outbound` 分岐の `ws.send` 失敗（`Some(Err(err)) => return Err(err.into())`、247 行）: `Error::Io(_)` | `Failed(FailureKind::Io)` |
| `InboundEvent::Outbound` 分岐の `ws.send` 失敗（247 行）: `Io` 以外 | `Failed(FailureKind::Protocol)` |
| `apply_outcome(...).await?` が伝播する `apply_outcome` 内部の `ws.send`/`ws.close` 失敗（`apply_outcome` 内 451 行・459 行の `result?`、呼び出し元の `.await?` 経由。Text 分岐 266 行・Binary 分岐 285 行）: `Error::Io(_)` | `Failed(FailureKind::Io)` |
| 同上: `Io` 以外 | `Failed(FailureKind::Protocol)` |

#### `FailureKind::Io` の判別方法（`Error::Io(_)` を明示的に振り分ける）

`FailureKind::Io` は `Protocol` へ埋没させず、`MessageTooLarge`（`Error::
Capacity(_)`）と同じ手法でパターンマッチにより明示的に振り分ける。

`tokio_tungstenite::tungstenite::Error`（tungstenite 0.30）には `Io(std::io::
Error)` variant があり、tungstenite 自身の `error.rs`（37-41 行）が「fatal」と
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

現状: `race_cancel(cancel, config.handler.on_message_with_ctx(ctx, msg))` は単独
await であり、この間 `WsSender` の outbound チャネルを消費するものが誰もいない
（受信ループ先頭の race は次の反復まで戻ってこない）ため、`on_message_with_ctx`
内で容量（既定 8）超の `send` を呼ぶとデッドロックする。

**方針**: ハンドラ呼び出しをハンドラ Future を `pin!` した内側ループで包み、各
反復で「cancel（最優先）→ (ハンドラ完了 | outbound 到着)」を race する。outbound が
先着した場合は即座に `ws.send()` で送出し、ハンドラ Future は **drop せず再ポーリ
ング**してループを継続する（[`race2_alternating`] のような優先順位交互化は不要と
判定: ハンドラ Future は 1 回しか完了しない単発イベントであり、`ws.next()` のよう
に継続的に Ready を生成し続ける入力ではないため、固定順のポーリングでもハンドラ
完了検知が飢餓することはない。ただし cancel は既存どおり最優先を維持する）。

outbound チャネルが閉鎖済み（`recv()` が `None`）の場合は既存の外側ループと同じ
振る舞い（そのイベント源を無効化し、ビジーループ化を防ぎつつセッション自体は継続）
を内側ループにも適用する。5 節で述べたとおり、`conn_ctx` がセッション生存中は
`WsSender` を保持するため、この分岐はセッション実行中は到達不能である（防御的
コードとして維持）。

**PR #724 レビュー指摘対応（P1）**: 当初案は「ハンドラ実行中に送出された push は
構造的に Reply より先にワイヤへ出る」としていたが誤り。内側ループは「cancel →
(ハンドラ完了 | outbound 到着)」を 1 反復ごとに race するだけであり、ハンドラ
Future 自身が `WsSender::send(...).await` を完了させてから（await 中に中断
されなければ同一 `poll()` 内で）続きを実行し `Poll::Ready(outcome)` を返す場合、
その反復では「ハンドラ完了」の分岐が「outbound 到着」の分岐より先に（あるいは
同時に、ポーリング順序に関わらず）選ばれうる。この場合、ハンドラ自身が直前に
`send` を完了させた push がチャネル内に残っていても、内側ループはそれを検出
する前にハンドラ完了へ分岐し、`apply_outcome` が `WsOutcome::Reply`/`Close` を
先に送出してしまう。race の対象が「push の到着」と「ハンドラの完了」という
2 つの独立な非同期イベントである以上、片方が他方の**内部で**起きたことを race の
結果だけから復元することはできない（構造的に保証されない）。

**確定した設計（排出ステップを追加）**: ハンドラ Future が `Poll::Ready(outcome)`
を返した時点で、まず `outcome`（`Result<WsOutcome, WsHandlerError>`）の
`Ok`/`Err` を判定する。排出ステップを行うのは `outcome` が `Ok` の場合のみで、
`Err` の場合は行わない。

**PR #724 再レビュー指摘対応（P1・2 件目）**: 当初案は「ハンドラ完了を検知したら
（`outcome` の内容に関わらず）排出してから `apply_outcome` を呼ぶ」としていたが
誤り。`outcome` が `Err(WsHandlerError)` の場合、現行 `session.rs`（252 行の
`message?` と同様の早期 `?` パターン、266 行・285 行の `outcome?`）は排出も
`apply_outcome` 呼び出しも行わず即座に `Err(WsError::Handler(_))` を返す。この
順序を維持しないと、(a) ハンドラが失敗した接続へ push を送ってしまう（失敗した
ハンドラが確定した意味的な状態と矛盾する応答をクライアントに見せる恐れがある）、
(b) 排出中の `ws.send()` が失敗した場合、その `Failed(FailureKind::Io/Protocol)`
が本来の `Failed(FailureKind::Handler)` を上書きしてしまい、`on_close` の
呼び出しコード（`run_session` 外側ラッパー）が確定させる `CloseReason` が
ハンドラ失敗という一次要因を隠す、という 2 つの問題が生じる。したがって
`outcome` の判定を排出より**先**に行う順序を設計として確定する:

1. `outcome` が `Err(err)` の場合: 排出を行わず、直ちに `Failed(FailureKind::
   Handler)` を `CloseReason` として確定し、`Err(WsError::Handler(err))` を
   返す（現行 `outcome?` と同じ即時終了、4 節の対応表の該当行と整合）。
2. `outcome` が `Ok(outcome)` の場合: 排出ステップへ進む（`outcome` が
   `WsOutcome::Reply`/`Close` のいずれであっても、以下の排出手順自体は共通）。

| `outcome` | 排出を行うか | 送出順序 |
|---|---|---|
| `Err(WsHandlerError)` | 行わない | 何も送出せず `Failed(FailureKind::Handler)` を確定して即座に抜ける |
| `Ok(WsOutcome::Reply(messages))`（`messages` が空でも同様） | 行う | 排出した push（到着順）→ `messages`（到着順） |
| `Ok(WsOutcome::Close)` | 行う | 排出した push（到着順）→ Close フレーム |

排出ステップの内容（`try_recv()` による同期排出・`ws.send()` での直列送出）は
`Ok` の 2 ケースで共通、以下に続ける。

排出は有界にする。上限は「排出を開始する時点の `Receiver` の長さ
（`tokio::sync::mpsc::Receiver::len()` のスナップショット）」とし、排出ループは
このスナップショット件数に達したら（それ以上 `try_recv()` を呼ばずに）停止して
Reply/Close の送出へ進む。他タスクが排出中も継続して push し続けても、その分は
次回の外側ループ反復（`InboundEvent::Outbound` 分岐）で処理され、本排出ステップ
自体は有限回で終わる（Reply が無期限に遅延しない）。`len()` が実装依存で不正確に
なる可能性（同期メソッドのため通常は正確だが、将来のチャネル実装変更を想定）に
備え、フォールバック上限として既定容量 `DEFAULT_OUTBOUND_CAPACITY`（= 8）を
採用してもよい（`min(len(), DEFAULT_OUTBOUND_CAPACITY)` 等、実装イシュー側の
判断に委ねる。本文書は「有界であること」を不変条件として課すのみで、具体的な
上限式の選択までは固定しない）。

**保証する順序契約（doc・テストで固定する対象、再定義）**: 「**送信元がハンドラ
自身か別タスクかに関わらず**、ハンドラの `Future` が完了する（`Poll::Ready` を
返す）**前**に `WsSender::send(...).await` が完了した push は、そのハンドラが
返す `WsOutcome::Reply`/`Close` の送出より**先に**ワイヤへ出る」。排出は
`try_recv()` でチャネルに格納済みの値を送信元を区別せず取り出すため、この保証は
実装と一致する（送信元で区別する制約は課さない）。

**PR #724 再レビュー指摘対応（P2・3 件目）**: 以前の版は上記の保証を宣言した
直後に「別タスクから並行して送られる push は一律対象外」という完了時刻と無関係な
除外を追加しており、両者が矛盾していた（別タスクの `send` がハンドラ完了前に
完了した場合、保証の対象になるはずなのに除外規定に該当してしまう）。除外条件を
**完了タイミングのみ**で定義し直し、送信元による区別を削除する。以下は明示的に
順序保証の対象**外**とする:

- **ハンドラ完了後**に `send(...).await` が完了した push（送信元がハンドラ自身か
  別タスクかを問わない）。この push が実際にいつ送出されるかは `outcome` に
  よって異なる（直前の「outcome ごとの排出表」と整合させる）:
  - `Ok(WsOutcome::Reply(_))` の場合: セッションは継続する（`SessionFlow::
    Continue`）ため、次回の外側ループ反復で通常の `InboundEvent::Outbound`
    経路として処理される（送出される）
  - `Ok(WsOutcome::Close)` の場合: `apply_outcome` が Close フレームを送出した
    時点でセッションは終了する（`SessionFlow::Closed => break;`）。外側ループの
    次回反復は発生しないため、この push は送出されずセッション終了時に
    `Receiver` が drop されて破棄される（`Err(WsHandlerError)` の場合と同様に
    そもそも送出されない事象として扱う）
- **ハンドラ完了と同時刻**（同一の排出開始時点）に `send(...).await` が実行中
  だった push（送信元を問わない。チャネルが満杯で送信待機中のものを含む）。
  `try_recv()` はチャネルに既に格納済みの値のみを取り出すため、送信途中の
  ものは排出対象にならない
- 排出上限（本節で定めたスナップショット件数）を超えた分。上限に達した時点で
  排出ループは停止するため、超過分は次回の外側ループ反復で処理される

`idle_deadline` は本内側ループ・排出ステップ中は更新しない（既存の「クライアント
から実際にフレームを受信したときのみ延長」契約を変えない。outbound 送出は
アイドル判定に影響しない）。

内側ループ中の `ws.send()` エラー（排出ステップでの送出を含む）は外側ループの
`InboundEvent::Outbound` 分岐と同じ扱いにする（4 節の対応表を参照。`tungstenite::
Error::Io(_)` なら `Failed(FailureKind::Io)`、それ以外なら `Failed(FailureKind::
Protocol)` へ振り分ける）。

このエラーは `run_session_inner` にとって一次的な脱出（そのまま `CloseReason`
として確定し `Err(WsError)` を返す）であり、`close_and_drain`（4 節「不変条件」
節を参照）が既に確定した `IdleTimeout`/`Cancelled` トリガを上書きしないという
契約とは無関係である。両者は別の場面で発生する別のエラー処理: `close_and_drain`
の上書き禁止契約は「`IdleTimeout`/`Cancelled` で切断を決めた**後**の Close
ハンドシェイク送出・ドレイン中に生じる二次的なエラー」を対象とし、本排出
ステップのエラーは「まだ `CloseReason` が確定していない受信ループの途中」で
生じる一次的なエラーである。したがって排出ステップの送出失敗がそのまま
`CloseReason` になる（上書きの概念自体が発生しない）。

排出ステップ中に cancel が発火した場合も既存の優先順位（cancel 最優先）を
維持する: 排出ループの各 `ws.send()` は他の送出箇所と同様に `cancel` と race
させ（`race_cancel`）、発火時は当該送出を打ち切って `handle_cancellation` へ
分岐する（`Cancelled`）。排出予定だった残りの push は送出されないままセッションが
終了する（Close ハンドシェイクへ移るため。これは既存の「cancel 発火時は
outbound を明示的に drop し満杯チャネルでの送出待ちを即座に解放する」契約と
整合する）。

**#706 への引き渡し事項**: 本節の排出ステップ（`try_recv()` による有界排出・
上限の具体的な算出方法・4 節対応表と同じ `Io`/`Protocol` 振り分け・排出中
cancel の扱い・`outcome` が `Err(WsHandlerError)` の場合は排出を行わず即座に
`Failed(FailureKind::Handler)` を確定する順序）の実装、および上記「保証する
順序契約」（送信元に関わらず完了タイミングのみで判定する版）をテストで固定
すること（(1) ハンドラ自身が `send().await` 完了後に `Ok(Reply)` を返すケースを
実接続で検証し、push が Reply より先にワイヤへ出ることを確認する、(2) ハンドラ
とは**別タスク**から、ハンドラ完了**前**に `send().await` を完了させた push が
Reply より先に届くことを検証する（送信元による区別がないことの確認）、(3) 別
タスクからの `send().await` がハンドラ完了と同時刻・完了後に完了するケースは
順序保証の対象外であることを確認する（フレーク回避のため、タイミング制御が
難しい場合はこのケースを厳密な e2e ではなく設計注記の確認に留めてもよい）、
(4) ハンドラが `Err` を返すケースで、事前に `send().await` 済みの push があっても
排出されずに `Failed(FailureKind::Handler)` で終了することを確認する）を #706
の受け入れ基準に追加する。

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
- `session.rs`（非公開モジュール）の内部リファクタリング（#706 のデッドロック修正）
  はそもそも公開 API 面の変更ではない

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
  参照）、ユニットテスト
- **#705**: `CloseReason`/`FailureKind`・`run_session_inner`/`run_session` 分割・
  `on_close` 呼び出し（全終了経路で exactly-once）・`WsSender::closed()`/
  `is_closed()`・各終了経路ごとの実接続テスト。`crates/core/src/plugin.rs` は変更
  不要である旨を明記済みなので影響範囲から除外してよい
- **#706**: 送信キュー消化の内側レース実装・順序契約のテスト固定（前提: #704 完了後）
- **#707**: 2 クライアント同時接続 e2e（前提: #704/#705/#706 完了後）

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

[`WsOpenContext`]: ../../crates/plugin-websocket/src/handler.rs
[`WsSendError`]: ../../crates/plugin-websocket/src/handler.rs
[`DEFAULT_OUTBOUND_CAPACITY`]: ../../crates/plugin-websocket/src/handler.rs
[`race2_alternating`]: ../../crates/plugin-websocket/src/session.rs
[`apply_outcome`]: ../../crates/plugin-websocket/src/session.rs
[`handle_idle_timeout`]: ../../crates/plugin-websocket/src/session.rs
[`handle_cancellation`]: ../../crates/plugin-websocket/src/session.rs
