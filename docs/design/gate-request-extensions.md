# ゲートの検証結果をリクエスト単位でハンドラへ渡す経路（イシュー #924）

WebSocket の接続コンテキストから同じ値を読む拡張は 14 節（イシュー #933、親 #932）で扱う。

## 1. 背景・目的

`RequestGate` が得た検証結果（例: `plugin-jwt` が検証した Claims）を後段のハンドラへ渡す、
リクエスト単位の経路がコアになかった。

- `GateOutcome::Allow` は値を運ばない。
- `GateContext` は peer address を読むだけで、接続ごとに 1 回作られる（`handle_connection_with_permit`
  のループの外）。`Copy` でもある。
- DPoP 有効時は `jti` を 1 要求につき 1 回だけ登録するため、ハンドラが `authenticate` を呼び直せない
  （`docs/design/jwt-verify-plugin.md` 9.3 節は、ゲートが得た Claims を後段へ渡す経路があることを前提にする）。
- この経路を `plugin-jwt` の内部だけで作ろうとした案は、共有状態では「正当な要求」と「同じ proof の
  再送」を区別できず、ゲート通過の証明が現在のリクエストに束縛されないという欠陥（認可迂回）を
  指摘された。リクエストへの束縛はコアの構造で保証する必要がある。

## 2. 制約（候補を分ける条件）

- 「ゲートがどう書くか」と「ハンドラがどう読むか」の 2 つのシームが要る。
- 値の寿命はリクエスト単位でなければならない。接続単位の器（`GateContext`）に置くと keep-alive の
  後続リクエストへ漏れる。
- 依存方向は `server(core) → routes → http`。`RouteHandler = Fn(&RequestHead, &[u8])` は core の型を
  名指しできないため、routes の API を変えずに読ませるには、器の型を `crates/http` に置き
  `RequestHead` にぶら下げる必要がある。
- `GateOutcome` は `Debug, Clone, PartialEq, Eq` を derive する。型消去した値（`dyn Any`）は
  `PartialEq` を満たせない。具体型にすればコアがプラグインの型へ依存する。
  `plugin-boundary.md` 5.6.3 / PoC-6 には「`GateOutcome` は判定結果だけを運ぶ」という既定の判断もある。
- `RequestHead: Clone` は WebSocket 委譲経路（`crates/core/src/plugin.rs`）で実際に使われる。
  値は `Arc` で共有し、`T: Clone` を要求しない。

## 3. 候補の比較

| 案 | 内容 | 評価 |
|---|---|---|
| A | `GateOutcome::Allow` に型付きの値を運ばせる | `PartialEq/Eq` と両立しない。網羅 match を壊す。受けた後にハンドラへ届ける器が別に要る。PoC-6 に反する。不採用 |
| B | `GateContext` に書き込み枠を足す | 接続単位で作られ `Copy` のため、リクエスト間で漏れる。直すには `Copy` を外しループ内で作り直す BREAKING が要り、接続スコープとリクエストスコープが混ざる。不採用 |
| C | プラグイン内の共有状態 | 正当な要求と再送を区別できない。不採用 |
| D | ハンドラへコンテキスト引数を新設 | `Handler::handle`・`RouteHandler` など公開 API を広く壊す。不採用 |
| E-1 | `check(&self, head: &mut RequestHead, ctx)` | `RequestHead::version` が `pub` で、ゲートが書き換えられる。head を変更しない観測契約とも整合しない。不採用 |
| E-2 | 既定メソッド `check_with_extensions` を足す | #486（`gate-peer-addr.md` 3.1 案 B）で退けた形。契約が二重になる。不採用 |
| **E-3（採用）** | `crates/http` に `RequestExtensions` を新設して `RequestHead` に持たせ、`check` に `extensions: &mut RequestExtensions` を足す | 器は 1 リクエストの `RequestHead` と同じ寿命で、束縛が構造で保証される。`GateOutcome`・`GateContext` は不変。routes の API も不変。正準メソッドは 1 本 |

`GateContext` や、プロセスを通した共有状態を器の候補として採らない。将来の選択肢としても残さない
（A01 アクセス制御の不備につながるため）。

## 4. 採用案の公開 API

`crates/http`（`extensions` モジュール）:

- `RequestExtensions`: `Vec<(TypeId, Arc<dyn Any + Send + Sync>)>`。空ならヒープを確保しない。
  - `new` / `insert<T>(&mut self, T) -> Result<(), T>`（同じ型があれば上書きせず `Err`）/
    `get<T>() -> Option<&T>` / `get_shared<T>() -> Option<Arc<T>>`（`'static` future を返す非同期ハンドラへ
    move するため）/ `contains` / `len` / `is_empty`
  - `remove`・`get_mut` は提供しない。
  - `Clone` は `Arc` の複製だけ。`Debug` は件数だけで値を出さない。`unsafe` なし、新規依存なし。
- `RequestHead::extensions()` / `extensions_mut()`。`parse_request_head` は常に空で構築する。
  `PartialEq` の比較には含めない（ワイヤ上の意味的な値だけを比較する既存契約を維持）。

`crates/core`:

- `RequestGate::check(&self, head: &RequestHead, ctx: &GateContext, extensions: &mut RequestExtensions) -> GateOutcome`
- `RequestExtensions` を `fandhe_backend_core::extension` と crate ルートで再エクスポートする。

## 5. コアの配線

`handle_connection_with_permit` は、リクエストごとに空の `RequestExtensions` を作り、
`first_rejection` へ渡す。全ゲートが `Allow` を返し、器が空でなければ、`request.head.extensions_mut()`
へ移す。以降（Upgrade 判定・`Interceptor`・`try_intercept`・`Handler`・`map_response`・
`on_response_with_status`）は同じ head を参照する。`Reject` の経路では器を drop し、拒否応答と
`on_response_with_status` は空の head のままになる。

## 6. 可視性の契約

| 段階 | 読めるか | 書けるか |
|---|---|---|
| `Middleware::on_request`（ゲート前） | 空 | 不可 |
| `RequestGate::check` | 進行中の器（先行ゲートの値） | 可（唯一の書き込み口） |
| `UpgradeHandler::matches`・Upgrade 委譲先 | 値あり（WebSocket では `WsHandshakeContext::head()` から読める。`WsOpenContext` / `WsConnContext` からは 14 節の実装後に読める） | 不可（`&RequestHead`） |
| `Interceptor::intercept` / `map_response` | 値あり | 不可 |
| `try_intercept`・`Handler`・Router のハンドラ | 値あり | 不可 |
| `Middleware::on_response_with_status` | 通過時は値あり、拒否時は空 | 不可 |
| 同じ接続の次のリクエスト | 空（新しい `RequestHead`） | — |

## 7. BREAKING 判定と移行

- BREAKING: `RequestGate::check` のシグネチャ変更（`versioning-policy.md` 3 節「拡張点 3 trait のシグネチャ
  変更」）。0.x では次の minor 相当。版上げはリリース手順側で行い、本変更では Cargo の version を変えない。
- 非推奨期間を挟まない理由: 必須メソッドの引数追加は `#[deprecated]` で段階移行できない。二重メソッドの
  暫定期間は #486 と同じ理由で退ける。
- 非破壊: `RequestHead` へのフィールド追加（`version` 以外は非公開）、新しい型、新しいアクセサ。
- 移行手順は `CHANGELOG.md` の `[Unreleased]` を参照。既存の `impl RequestGate` は第 3 引数
  `_extensions: &mut RequestExtensions` を足すだけで、挙動は変わらない。

## 8. 既存の設計判断との整合

- `plugin-boundary.md` 5.6.3: `GateOutcome` は引き続き判定だけを運ぶ。判定根拠データはリクエスト単位の
  `RequestExtensions` を経由し、コアは型消去した値を中継するだけで、プラグインの型へ依存しない。
  5.6.3 の文言はこれに合わせて改めた。
- `jwt-verify-plugin.md` 9.3 節の「後段への受け渡し経路」は本書の `RequestExtensions` を指す。9.3 節の
  文言更新は `plugin-jwt` が実際に使う #904 で行う。

## 9. セキュリティ（OWASP 観点）

- A01 / A07: ゲート通過の証明を、それを得たリクエストの `RequestHead` と同じ寿命の器に閉じ込める。
  書き込み口は `check` だけで、後段は `&RequestHead` しか持たず書けない。拒否時は器を破棄する。同じ型の
  二重 insert は上書きせず `Err` にし、後のゲートが先の検証結果を差し替えることを防ぐ（ゲートは拒否を返す）。
- なりすまし: 値は型（`TypeId`）で引き、文字列キーは使わない。クライアントが送るバイト列から器へ値が入る
  経路はない（パーサは常に空）。プラグイン側は外部から構築できない newtype（非公開コンストラクタ）を入れると、
  他のゲートや利用者コードによるなりすましも防げる。`plugin-jwt` への申し送り（#904）。
- A09: `Debug` は件数だけで、`RequestHead` の `Debug` がログに出ても Claims は露出しない。
- DoS: 書くのは登録済みのゲート（信頼コード）だけで、件数はゲート数で有界。空の器はヒープを確保しない。
- メモリ安全: `unsafe` なし。A06: 新規依存なし。

## 10. pay-for-what-you-use

feature ゲートは不要（外部依存ゼロ）。ゲート未使用の構成では、`RequestHead` が空の `Vec`（ヒープ確保なし）を
1 つ持ち、接続ループがリクエストごとに空の器を作るだけで、確保は増えない。
`crates/http/tests/alloc_count.rs`（`parse_request_head` の alloc が定数）と `benches/microbench.sh --check`
（ラチェット）で確認した。

## 11. テストと受け入れ基準の対応

| 基準 | 検証 |
|---|---|
| 1 設計判断の記録 | 本書 |
| 2 実経路で同じリクエストのハンドラだけが読める | `crates/core/tests/gate_request_extensions.rs` の `handler_reads_value_inserted_by_gate_for_same_request`（実 TCP） |
| 3 別リクエスト・ゲート未通過からは読めない | 同 `value_does_not_leak_to_next_request_on_same_connection`・`concurrent_connections_do_not_cross_values`・`no_gate_means_no_value`・`rejected_request_does_not_reach_handler_nor_leak` |
| 3 可視性の契約・fail-closed | 同 `visibility_contract_for_middleware_hooks`・`duplicate_insert_fails_closed`・`interceptor_sees_gate_value` |
| 4 使わない構成で依存・性能の後退なし | `cargo tree` に新規依存なし、`alloc_count`、`microbench.sh --check` |
| 5 BREAKING の移行手順 | `CHANGELOG.md` |

## 12. スコープ外

- `plugin-jwt` の `JwtGate` が検証済み Claims を `GateVerified` として入れる件、および
  `jwt-verify-plugin.md` 9.3 節の文言更新: #904（実装済み）。
- `plugin-hub-wiring` の `TenantGate` が `TenantVerified` を同じ経路で渡す件: #905（実装済み、
  `TenantGate::verified`）。
- `plugin-websocket` の `WsOpenContext` / `WsConnContext` から値を読めるようにする件: 14 節で設計する。
  実装は本ツリー（#932）の #934、実 TCP テスト・文書は #935。
- 0.6.0 への版上げとリリース作業（#930 / #931 で完了済み）。
- `&RequestExtensions` を受ける `verified` 系の補助関数（hub-wiring / plugin-jwt）。
- セッション中の `exp` 再検証やセッション寿命の上限を設ける機構。

## 13. 拡張点閉包ゲート（`extension-closure-gate.sh`）への理由記載

本変更は `RequestGate::check` のシグネチャ変更（`crates/core/src/extension.rs`）を含むため、
`dependency-graph-contract.md` 4 節の運用に従い、E（閉包違反候補）と判定されるファイルの理由を記す
（4.3 節・4.4 節と同形式）。

1. **対象**: イシュー #924 / PR #925
2. **E ファイルパス**:
   - `crates/http/src/extensions.rs`（新規）
   - `crates/http/src/request.rs`
   - `crates/http/src/lib.rs`
   - `crates/core/src/extension.rs`
   - `CHANGELOG.md`
3. **閉じない理由**: `extension-closure-check.sh` の分類規則は中間層クレート `crates/http` と
   `CHANGELOG.md` を A〜D に含めない（4.3 節〜4.9 節と同一の運用上のギャップ）。加えて、ゲートが
   入れる付帯データの器 `RequestExtensions` はリクエスト 1 件と寿命を共にする必要があり、
   `RequestHead` を定義する `crates/http` の型に属する。プラグイン（A）内にも、コアの許容シーム（B）内にも
   置けない（3 節 E-3 の採用理由）。
4. **正当性根拠**: 追加は外部依存ゼロの加法的な型と、`RequestGate::check` への引数追加のみである。
   依存方向（`server → routes → http`）と拡張点の種類数は変わらない。移行手順は `CHANGELOG.md` に記す。

## 14. WebSocket の接続コンテキストから読む（イシュー #933、親 #932）

### 14.1 経路の事実

- コアは全ゲートが `Allow` のあと、Upgrade 判定より前に器を `request.head` へ移す。`Reject` は Upgrade
  の分岐より前に応答して抜けるので、拒否された upgrade は `handle_upgrade*` に届かない。`on_open` は
  呼ばれず 101 も返らない（fail-closed）。
- `plugin::try_handle_upgrade` は head を clone して `handle_upgrade_with_peer_addr` へ渡す。
  `RequestExtensions` の clone は `Arc` の参照カウントを増やすだけである。
- plugin-websocket では `ConnRequestInfo::from_head` が head を消費する唯一の地点で、101 応答の送出に
  成功したあと 1 回だけ構築し、`Arc` で両コンテキストに共有する（`ws-connection-context-and-close.md`
  17 節）。許可リストの 5 項目だけを抜き出して head を捨てるため、値はそこで失われる。
- 現状の読み口: `WsHandshakeContext::head().extensions()` は `WsHandshakeCheck` のフック内（101 応答の
  前）で既に読める。`WsOpenContext` / `WsConnContext` からは読めない。

### 14.2 採用案

- `ConnRequestInfo` に `extensions: RequestExtensions` を足し、`from_head` で `head.extensions().clone()`
  を保持する。
- `WsOpenContext::extensions(&self) -> &RequestExtensions` と
  `WsConnContext::extensions(&self) -> &RequestExtensions` を追加する。書き込み口は持たない。
- `RequestExtensions` は `Clone` と `Default` を実装しているので、`ConnRequestInfo` の既存の derive と
  テストにある `ConnRequestInfo::default()` はそのままコンパイルできる。`Arc<dyn Any + Send + Sync>`
  を持つため、両コンテキストの `Send` / `Sync` も変わらない。

### 14.3 退けた案

- **`RequestHead` を丸ごと保持する**: 17 節の許可リスト方式と、保持量の上限という論拠に反する。
- **`handle_upgrade*` のシグネチャに引数を足す**: BREAKING であり、head が既に運んでいるので不要。
- **`on_open` などからゲートや認証を呼び直す**: DPoP の `jti` は 1 回しか使えない（1 節と同じ理由）。
- **plugin-websocket が plugin-jwt / hub-wiring の具体型に依存する**: plugin から plugin への依存を
  増やさない方針に反する。型消去した器を中継するだけにする。

### 14.4 許可リスト方針との整合

- 許可リストが抑えているのは、クライアントが制御するバイト列（任意ヘッダの取得口と保持量）である。
- `RequestExtensions` はパーサが常に空で構築し、書けるのは登録済みのゲート（信頼コード）の `check`
  だけである。件数はゲートの数で上限が決まり、任意ヘッダの汎用取得口は増えない。
- 差分は正直に記す。保持量の上限は、byte 定数（`MAX_CONTEXT_*`）からゲート実装が決める大きさへ移る。
  ゲートは検証済みの小さな値（Claims など）だけを入れる前提とする。

### 14.5 公開 API への影響と版

- 追加するのは、`#[non_exhaustive]` で非公開フィールドの型へのメソッドだけである。
  `crates/core`・`handle_upgrade*`・拡張点の trait は変えない。新規依存なし、`unsafe` なし。
- 非破壊の追加として扱う（7 節の BREAKING とは区別する）。版上げはリリース手順側で行う。

### 14.6 利用者の読み方

- `TenantGate::verified` と `JwtGate::verified` は `&RequestHead` を取るので、WebSocket のコンテキスト
  からは呼べない。
- WebSocket では `ctx.extensions().get::<TenantVerified>()` から `TenantVerified::tenant`、または
  `get::<GateVerified<E>>()` から `GateVerified::claims` を読む。`E` はゲート登録時と同じ型でないと
  `TypeId` が一致せず `None` になる。`None` は拒否側に倒す（fail-closed）。
- `&RequestExtensions` を受ける補助関数は 12 節のとおりスコープ外とする。

### 14.7 既知の限界

- 値は WebSocket セッションが続く間ずっと保持される。
- JWT の `exp` はハンドシェイク時点で検証するだけで、セッション中には再検証しない。鍵のローテーション
  や失効も、張ったあとのセッションには反映されない。
- アプリ側の緩和策: `JwtGate` を使う場合は `on_open` で `Claims::exp()` を読み、`WsSender::close` を
  期限に予約する。ping と idle timeout も併用する。
- `TenantClaims` は `org_id()` だけを公開し `exp` を出さない。`TenantGate` 単独ではアプリが期限を
  読めない。
- コアの `max_connection_lifetime` が upgrade 後の委譲セッションにも効くかは本節では断定しない。
  #935 の文書化の前に `crates/core/src/server.rs` で確かめる。

### 14.8 `Debug` とセキュリティ（OWASP）

- `Debug` には 2 層で値を出さない。`RequestExtensions` の `Debug` は件数だけを出し、両コンテキストの
  `Debug` は `info` 自体を出さない（17 節）。#934 でもこの出力項目を変えない（A09）。
- A01 / A07: リクエストへの束縛は 9 節と同じで、ゲートを通った head に限る。WebSocket 側は読み取り専用。
- DoS: 件数はゲート数で上限が決まる。セッション中の常駐は 14.7 に記した。A06: 新規依存なし。
  メモリ安全: `unsafe` なし。

### 14.9 #934 / #935 への申し送り

- #934: 両アクセサと doc test。`on_open` / `on_message_with_ctx` / `on_close` で値を読めること、
  ゲートがなければ空であること、別の接続に現れないことをユニットテストで確かめる。`Debug` の出力は
  不変とする。
- #935: 実 TCP の統合テスト（`Server::gate` と `Server::websocket`）で、上記 3 箇所と
  `WsHandshakeContext::head().extensions()` から読めること、ゲートが拒否した upgrade では `on_open` も
  101 もないこと、続けて張った別の接続に値が漏れないことを確かめる。あわせて CHANGELOG・ガイド
  （`exp` の限界を含む）・CLAUDE.md を更新し、`ws-connection-context-and-close.md` 17 節から逆参照を張る。
