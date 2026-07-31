# プラグイン境界パターン（TASK-2.1 / #18）

対応: `docs/spec/05-tasks.md` TASK-2.1（REQ-2、`docs/spec/04-requirements.md`）。
MS-1（`docs/spec/06-roadmap.md`）。前提タスク TASK-1.4（#13）完了後に着手。

## 1. 背景

`crates/core` は「最小コア + Cargo feature 駆動プラグイン」を核とする設計だが、
本タスク着手時点では feature による着脱を実例で示した実装が存在しなかった。
一方、プラグインクレート `fandhe-backend-plugin-webrtc-proxy`（#74）はハンドラ単体では
自己完結していたが、コアの接続受理ループへ未配線だった。

本ドキュメントは、`webrtc-proxy` feature を第 1 号として確立した「feature flag
+ `dep:` 構文によるプラグイン境界パターン」を記述し、後続プラグイン
（websocket / graphql / openapi / hub-wiring / tracing）が同パターンを踏襲する
際の指針とする。

## 2. feature 命名規約

プラグインクレート名（`fandhe-backend-plugin-<name>`）から `fandhe-backend-plugin-` 接頭辞を除いた
`<name>` を feature 名とする。

```toml
# crates/core/Cargo.toml
[dependencies]
fandhe-backend-plugin-webrtc-proxy = { path = "../plugin-webrtc-proxy", optional = true }

[features]
default = []
webrtc-proxy = ["dep:fandhe-backend-plugin-webrtc-proxy"]
```

- `optional = true` + `dep:` 構文を使う。`dep:` を使わずに `optional = true`
  だけで feature を作ると、依存クレート名と同名の **implicit feature** が
  暗黙に生えてしまい、公開 feature 名の意図しない増加・利用者からの誤参照を
  招く。`dep:fandhe-backend-plugin-webrtc-proxy` は implicit feature を作らず、
  `webrtc-proxy` という 1 つの feature 名だけを公開 API とする
- `default = []` を維持する。ビルド時に何も選択しなければプラグインは
  依存・コード・`unsafe` を一切バイナリに含まない

## 3. コアループは cfg-free を維持する

`crates/core/src/server.rs::handle_connection`（接続受理・リクエストループ本体）
は `#[cfg(feature = "...")]` を一切持たない（`docs/spec/03-poc` PoC-3 の設計
規約）。プラグインの介入余地は、固定シグネチャの非公開シームヘルパーに閉じる。

feature 分岐が必要になった場合も、コアループ側はヘルパーのシグネチャを
変えずに済み、ヘルパー内部の実装差し替え・分岐追加だけで完結する。

現在 2 種のシームが存在する:

| シーム | 対象パターン | 状態 |
|--------|-------------|------|
| `plugin::try_handle_upgrade`（`crates/core/src/plugin.rs`） | 長時間接続（WebSocket 等）への委譲 | `websocket` feature で配線済み（TASK-4.1 / #22。5 節を参照） |
| `plugin::try_intercept`（`crates/core/src/plugin.rs`） | リクエスト/レスポンス完結型プラグインへのパスインターセプト | `webrtc-proxy`（TASK-2.1）・`webrtc`（TASK-8.1 / #26、in-process 型）で配線済み |

## 4. パスインターセプト型パターン（本タスクで確立、TASK-8.1 / #26 で 2 例目を追加）

`plugin::try_intercept(server: &Server, head: &RequestHead, body: &[u8]) ->
Option<fandhe_backend_http::response::Response>` が固定シグネチャのシーム。

```rust,ignore
pub(crate) async fn try_intercept(
    server: &Server,
    head: &RequestHead,
    body: &[u8],
) -> Option<Response> {
    #[cfg(feature = "webrtc-proxy")]
    {
        if let Some(config) = server.webrtc_proxy_config()
            && let Some(response) =
                fandhe_backend_plugin_webrtc_proxy::try_handle_rtc_offer(head, body, config).await
        {
            return Some(from_plugin_response(response));
        }
    }

    #[cfg(feature = "webrtc")]
    {
        if let Some(config) = server.webrtc_config()
            && let Some(response) = fandhe_backend_plugin_webrtc::try_handle_rtc_offer(head, body, config).await
        {
            return Some(response);
        }
    }

    #[cfg(not(any(feature = "webrtc-proxy", feature = "webrtc")))]
    {
        let _ = (server, head, body);
    }

    None
}
```

TASK-8.1（#26）で `webrtc` feature（in-process 型、`crates/plugin-webrtc`）を
2 例目として追加した。両 feature が `--all-features` で同時有効な場合、
`webrtc-proxy`（別プロセス切り出し型、REQ-8 の MVP 推奨方式）を先に評価する
運用判断とした（実運用では通常どちらか片方のみ `Server` へ登録するため、この
優先順位が問題になるのは意図的に両方登録した場合に限る）。`webrtc` feature
無効時は追加した cfg ブロックが丸ごと消え、`webrtc-proxy` 側の既存挙動には
影響しない。

- `Some(response)`: プラグインが処理を完結させた。呼び出し元（`handle_connection`）
  は既定 `Handler::handle` を呼ばずにこの応答をそのまま送出する
- `None`: 対象パスでない、またはプラグイン自体が無効。呼び出し元は既定
  `Handler::handle`（未登録時 404）へフォールスルーする
- feature 無効時は `server`/`head`/`body` を一切参照しない（`cfg(not(...))`
  ブロックで未使用引数警告のみ抑止し、実処理コードは存在しない）

### 4.1 評価順序

```text
RequestGate → UpgradeHandler → plugin::try_intercept → 既定 Handler
```

`RequestGate` を先に評価するのは、将来の hub TenantGate（TASK-9.1）が
WebSocket アップグレードだけでなくパスインターセプト型プラグインも既定拒否で
ゲートできるようにするため（フェイルクローズ、`docs/spec/04-requirements.md`
REQ-9・`.claude/rules/security.md`）。

### 4.2 プラグイン設定の受け渡し

`Server` ビルダーへ cfg-gated なフィールド・メソッドを追加する。

```rust,ignore
#[cfg(feature = "webrtc-proxy")]
pub fn webrtc_proxy(mut self, config: fandhe_backend_plugin_webrtc_proxy::ProxyConfig) -> Self {
    self.webrtc_proxy_config = Some(config);
    self
}
```

feature 無効時はこのメソッド・対応するフィールドが構造体から完全に消える
（`#[cfg(feature = "...")]` をフィールド定義・`Default` 実装の両方に付与する）。

### 4.3 応答の変換と Content-Type

プラグイン側の中間表現（例: `fandhe_backend_plugin_webrtc_proxy::Response { status,
reason, content_type, body }`）はコアが送出する `fandhe_backend_http::response::Response`
へ変換する。`fandhe_backend_http::response::Response` は任意ヘッダ API を意図的に持たない
（レスポンス分割対策、`crates/http/src/response.rs` の doc）ため、本タスクで
`&'static str` 限定の `Response::with_content_type` を追加した。プラグイン側の
`content_type` フィールドも `&'static str` に限定されているため、変換経路に
外部入力由来の動的文字列が混入する余地はない。

`reason` phrase はプラグイン側の値をそのまま使わず、`fandhe_backend_http::response::Response`
内蔵の固定テーブル（`reason_phrase`）から `status` に基づいて引く。プラグインが
新しいステータスコードを払い出す場合は、このテーブルへのエントリ追加を
忘れないこと（本タスクでは `502 Bad Gateway` / `504 Gateway Timeout` を追加
した。追加を怠ると `HTTP/1.1 502 \r\n` のように reason phrase が空文字へ
劣化する。PoC-9 教訓: ステータスコードのみの検証はこの劣化を見逃す。統合
テストは必ず reason/Content-Type/body まで含めて検証すること）。

**TASK-8.1（#26）での簡素化**: `fandhe-backend-plugin-webrtc-proxy` が独自の中間 `Response`
型（`status`/`reason`/`content_type`/`body`）を持つのは、本パターン確立前
（配線が未確立だった TASK-8.2-2 時点）の歴史的経緯である。`fandhe-backend-plugin-webrtc`
（TASK-8.1）は配線パターンが既に存在する状態で新設したため、この変換層を
省き [`fandhe_backend_http::response::Response`] を直接組み立てて返す（`try_intercept` は
`Some(response)` をそのまま返し、`from_plugin_response` 相当の変換関数を経由
しない）。後続プラグインも、配線パターン確立後に新設する場合はこの簡素化版
（`fandhe_backend_http::response::Response` を直接返す）を優先すること。

## 5. Upgrade 型パターン（TASK-4.1 / #22 で確立）

`fandhe-backend-plugin-websocket`（`crates/plugin-websocket`）が Upgrade 型パターンの
第 1 号実装。`try_intercept` と同型の設計原則を踏襲しつつ、Upgrade 型固有の
差分が 2 点ある（5.1・5.2 節）。

### 5.1 シームのシグネチャ変更（意図的な逸脱）

当初のスタブ `try_handle_upgrade(stream, head, handlers: &[Box<dyn
UpgradeHandler>])` は「シームのシグネチャを変えない」という本ドキュメント
3 節の設計規約に反し、`Vec<u8>`（残余バイト列 `leftover`）+ `&Server`
（設定取得用）を受け取る形に変更した:

```rust,ignore
pub(crate) async fn try_handle_upgrade<S>(
    stream: S,
    head: &RequestHead,
    leftover: Vec<u8>,
    server: &Server,
) -> Option<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    #[cfg(feature = "websocket")]
    {
        if let Some(config) = server.websocket_config()
            && fandhe_backend_plugin_websocket::matches(head, config)
        {
            let _ = fandhe_backend_plugin_websocket::handle_upgrade(stream, head, leftover, config).await;
            return None;
        }
    }

    #[cfg(not(feature = "websocket"))]
    {
        let _ = (head, &leftover, server);
    }

    Some(stream)
}
```

変更理由:

- `leftover`: `handle_connection` はアップグレード委譲前にコア側の読み取り
  バッファを解放する（Conditional Go 条件(1)）が、解放前に
  `RecvBuffer::unread()` で退避した残余バイト列（パイプライン済みの先行
  フレーム等）を委譲先へ引き継ぐ必要がある。`fandhe_backend_plugin_websocket::handle_upgrade`
  はこれを `WebSocketStream::from_partially_read` へそのまま渡し、先行到着
  フレームを取りこぼさない
- `&Server`: 複数 Upgrade 型プラグインが将来増えた場合でも、各プラグインの
  cfg-gated 設定（`server.websocket_config()` 等）へ本シーム経由で
  アクセスできるようにするための一般化。`&[Box<dyn UpgradeHandler>]` では
  「委譲判定のみ」の情報しか持てず、ハンドシェイク詳細検証・フレーミング
  設定に必要な `WebSocketConfig` を渡せなかった

`UpgradeHandler::matches`（同期 API、委譲判定のみの契約）自体は変更して
いない。判定は `WebSocketUpgradeAdapter`（`crates/core/src/server.rs`、
`Server::websocket` が内部登録）が `fandhe_backend_plugin_websocket::matches` へ委譲する
薄いラッパーとして担う。

### 5.2 循環依存の回避

`fandhe-backend-plugin-websocket` は `fandhe-backend-core` に依存しない（`crates/plugin-websocket/src/lib.rs`
の doc を参照）。コア → プラグインの optional 依存（`webrtc-proxy` と同型）
のみを張るため、`UpgradeHandler` trait を実装するアダプタ
（`WebSocketUpgradeAdapter`）はコア側（`crates/core/src/server.rs`）に置く。
プラグイン自体は `matches` / `handle_upgrade` という純関数 + `WebSocketConfig`
のみを公開する。

### 5.3 後続 Upgrade 型プラグインへの適用手順

新規 Upgrade 型プラグインを追加する際は以下を踏襲する:

1. feature 命名規約（2 節）に従い `dep:` 構文で optional 依存を追加する
2. プラグインクレートはコアに依存させない（5.2 節）。`UpgradeHandler`
   アダプタはコア側（`server.rs`）に置く
3. `plugin::try_handle_upgrade` へ cfg-gated 分岐を追加する（複数プラグインが
   並存する場合は `UpgradeHandler::matches` の判定順にそのまま従う）
4. `Server` ビルダーへ cfg-gated な登録メソッドを追加する
5. `scripts/dep-direction-check.sh` の許可リストへ
   `fandhe-backend-core:fandhe-backend-plugin-<name>` を個別追加する（6.1 節と同じ
   方針。`fandhe-backend-plugin-*` への一般化はしない）
6. feature 無効時はコード・依存・`unsafe` が完全に消えることを
   `cargo tree` で確認する

### 5.4 委譲後の専用タスク再 spawn + permit 引き継ぎ（TASK-4.2 / #23【条件(1)】）

PoC-7（`docs/spec/03-poc/high-concurrency-scale/README.md`）実測で、WebSocket
長時間接続の接続あたり RSS が axum 比 155.2%（Conditional Go 条件(1) の成功
基準 110% 未達）となった。原因は、`try_handle_upgrade` が
`fandhe_backend_plugin_websocket::handle_upgrade`（ハンドシェイク + エコーループ）を
`handle_connection` タスクの future 内で**インラインに await** していたこと。
`handle_connection` は `read_request`・応答直列化・keep-alive 制御などを含む
大きな tokio タスクのステートマシンであり、インライン await のままだと WS
接続の生存中ずっとこの大きなステートマシンがメモリ上に残ってしまう。

是正として、マッチ確定時に WS セッション（ハンドシェイク + フレーミング）
だけを載せた専用タスクを `tokio::spawn` し、元の `handle_connection` タスク
は即座に `return` して大きな future を解放する構成へ変更した:

```rust,ignore
pub(crate) async fn try_handle_upgrade<S>(
    stream: S,
    head: &RequestHead,
    leftover: Vec<u8>,
    server: &Server,
    permit: &mut Option<OwnedSemaphorePermit>,
) -> Option<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    #[cfg(feature = "websocket")]
    {
        if let Some(config) = server.websocket_configs().iter()
            .find(|config| fandhe_backend_plugin_websocket::matches(head, config))
        {
            let config = config.clone();
            let head = head.clone();
            let permit = permit.take();          // permit をセッションタスクへ move
            tokio::spawn(async move {
                let _permit = permit;             // セッション終了まで保持
                let _ = fandhe_backend_plugin_websocket::handle_upgrade(stream, &head, leftover, &config).await;
            });
            return None;                          // 元タスクは即 return → 大きな future を解放
        }
    }
    #[cfg(not(feature = "websocket"))]
    { let _ = (head, &leftover, server, &permit); }
    Some(stream)
}
```

**permit 引き継ぎが必須の理由（DoS 対策の維持）**: 同時接続数上限は
`BoundServer::run` が保持する `OwnedSemaphorePermit` で強制する
（`.claude/rules/security.md` のリソース枯渇観点）。素朴に再 spawn すると、
元の `handle_connection` タスクが即座に終了して permit を解放してしまい、
長時間生存する WS セッションが `max_connections` のカウントから漏れる
（リソース枯渇 DoS のリグレッション）。これを避けるため:

- 呼び出し元（`crates/core/src/server.rs` の `handle_connection_with_permit`、
  `pub(crate)`。公開 `handle_connection(server, stream)` はこれを `permit:
  None` で呼ぶ薄いラッパー）は `permit: &mut Option<OwnedSemaphorePermit>`
  を渡す
- `try_handle_upgrade` はマッチ確定時に `permit.take()` で所有権を奪い、新
  タスクへ move する。呼び出し元に残るのは `None`（drop しても no-op）
- 新タスク側は `let _permit = permit;` でセッション終了までローカル変数と
  して保持し、タスクの戻り（セッション終了）と同時に自動 drop される

この結果、`S: AsyncRead + AsyncWrite + Unpin` に加えて `Send + 'static` 境界
が新たに必要になる（`tokio::spawn` の要件）。`handle_connection`／
`handle_connection_with_permit` の型パラメータ `S` にも同じ境界を追加した
（`TcpStream`・`tokio::io::duplex` はいずれも充足する軽微な公開 API 変更）。

観測可能な挙動の検証は `crates/core/tests/websocket_respawn.rs`
（`handle_connection` タスクがハンドシェイク直後に完了すること・
`max_connections(1)` 下で WS セッション生存中は 2 本目の接続が受理されない
こと）を参照。

### 5.5 後続 Upgrade 型プラグインへの適用指針（5.3 節の追補）

5.3 節の手順に加え、コネクション単位の長時間委譲を行うプラグインは以下も
踏襲する:

- `try_handle_upgrade` 相当のシームで委譲確定時は `tokio::spawn` による
  専用タスク再 spawn を検討する（インライン await のまま長時間 await する
  と `handle_connection` の大きなステートマシンが解放されない、5.4 節）
- 同時接続数上限を守るリソース（`OwnedSemaphorePermit` 等）を握っている
  場合は、再 spawn 時に必ずその所有権をセッションタスクへ move する
  （move し忘れは DoS リグレッションになる、5.4 節）

### 5.5.1 セッション内ユーザーハンドラ委譲（Issue #179、`plugin-websocket` 追補）

Upgrade 型パターン確立後もアプリケーションロジックを差し込む手段がなかった
（`crates/plugin-websocket` のセッション処理はエコー専用に固定）。Issue #179
はこれを解消し、セッションループ内で呼ぶメッセージハンドラをユーザーが
差し替え可能にする設計判断を追補する。

- **ハンドラ trait・メッセージ型はプラグイン側で定義する**: 依存方向は
  コア → プラグインの単方向のみ（5.2 節）を維持する制約があるため、
  ハンドラ trait（`WsMessageHandler`）・メッセージ型（`WsMessage` /
  `WsOutcome`）は `crates/plugin-websocket` 側に置く。コアのシグネチャ
  変更は不要で、`WebSocketConfig`（`Server::websocket(config)` で受け渡す
  既存の設定型）の中を旅させるだけで済む
- **`async fn` in trait の型消去**: dyn 互換にするため、`crates/plugin-graphql`
  の `BoxExecuteFn` の先例に倣い、新規依存を追加せず既存依存
  `futures-util`（`std` feature）が提供する `BoxFuture` で手書きする
  （async-trait 等は追加しない、pay-for-what-you-use）
- **tungstenite 型を公開 API に漏らさない**: `WsMessage` は独自表現とし、
  内部依存（`tokio-tungstenite`）のバージョン更新から公開 API を絶縁する
- **呼び出し順序**: セッションループはメッセージごとにハンドラを直列
  `await` する（順序保証・自然なバックプレッシャ。並行処理したい場合は
  ハンドラ内で自前に `tokio::spawn` する建て付け）
- **既存 DoS 上限を後退させない**: `max_message_size` / `max_frame_size` は
  tungstenite 側でハンドラ呼び出し**前**に強制され続ける（上限超過メッセージは
  ハンドラへ到達しない）。ハンドラの `Err` は既存の `WsError` へ
  `Handler(...)` variant として合流させ、コア境界を越えて panic させない
  契約を維持する

## 5.6 Gate 型パターン（依存逆転型、TASK-9.1 / #61 で確立、TASK-9.2 / #62 で
RS256 + JWKS へ差し替え）

`fandhe-backend-plugin-hub-wiring`（`TenantGate`、JWT 検証 → `org_id` 抽出 →
フェイルクローズ）は、4・5 節の 2 パターン（コア → プラグインの optional
依存 + feature ゲート）とは逆に、**プラグイン → コアの一方向依存**（依存
逆転型）を取る第 3 のプラグイン様式である。

### 5.6.1 依存逆転を選べる条件

4・5 節のパターンが依存逆転を採れなかった理由は、パスインターセプト型
（`try_handle_rtc_offer` 等、上流への非同期中継を伴う）・Upgrade 型
（ハンドシェイク検証・101 応答送出という非同期処理を要する）のいずれも、
3 拡張点（`Middleware`/`UpgradeHandler`/`RequestGate`）の同期 API 制約
（dyn 互換性のため、`crates/core/src/extension.rs` 冒頭 doc）に非同期呼び
出しを持ち込めないことにあった（6.1 節）。

`RequestGate` はヘッダ検査のみで完結する既存拡張点であり、`TenantGate` の
判定（`Authorization` ヘッダ抽出 → RS256 署名検証、`kid` による JWKS 内
鍵選択、I/O なし）はこの同期 API 制約に抵触しない。JWKS の取得（HTTP
フェッチ・自動リフレッシュ）自体は非同期 I/O を要するため `check()` 内では
行わず、利用側サービスが取得済み JWKS JSON を注入し
`SharedJwks::set()` で再起動なしローテーションする設計とすることで、
同期 API 制約の中に収めている（TASK-9.2、`crates/plugin-hub-wiring/src/jwks.rs`）。
したがって:

- `crates/core` の `Cargo.toml`・`server.rs`・`plugin.rs` は一切変更しない
  （`optional = true` + `dep:` 構文も、非公開シームへの分岐追加も不要）
- 利用側サービスが `fandhe-backend-plugin-hub-wiring` を依存に加え、
  `Server::gate(TenantGate::new(TenantGateConfig::from_jwks_json(jwks_json)?))`
  （既存の公開 API `Server::gate`、TASK-1.4）で登録するだけで配線が完結する
- `scripts/dep-direction-check.sh` の許可リストには汎用パターン
  `fandhe-backend-plugin-*:fandhe-backend-core`・`fandhe-backend-plugin-*:fandhe-backend-http` が既に存在する
  ため、6.1 節のような個別例外追加は不要（`crates/plugin-hub-wiring/src/lib.rs`
  に依存方向宣言 `server → routes → http::*` を記載するのみでチェック 2 も通過する）

### 5.6.2 pay-for-what-you-use の成立根拠

コア側に `dep:` ゲートを持たないため、feature フラグではなく
「利用側が依存グラフに `fandhe-backend-plugin-hub-wiring` を加えるか否か」で
pay-for-what-you-use が成立する。`cargo tree -p fandhe-backend-core` に
本クレート・その依存（`ring`/`base64`/`serde`/`serde_json`）が一切現れない
ことで機械検証できる（コアが本クレートを依存に持たないため、そもそも現れ
ようがない設計）。

JWT 検証は TASK-9.1（#61）の HS256（HMAC-SHA256、`hmac`/`sha2`）共有秘密鍵
スパイクから、TASK-9.2（#62）で RS256（非対称鍵）+ JWKS へ差し替えた
（HMAC 実装は本番実装に流用せず削除、`docs/spec/05-tasks.md` TASK-9.2）。
署名検証ライブラリは `rsa`（RustCrypto）ではなく `ring` 0.17 を採用する:
`rsa` crate は RUSTSEC-2023-0071（Marvin attack、fix なし）を抱えており
`scripts/dep-audit.sh`（`deny.toml` advisories.ignore = [] のフェイルクローズ
運用）で確実に FAIL する。`ring` は `crates/plugin-webrtc`（`webrtc`
feature 経由）が既に依存グラフへ引き込んでいる実績依存（`deny.toml` の
ライセンス許可リストに ISC 等が既存）であり、`fandhe-backend-plugin-hub-wiring` 追加
による新規のライセンス・advisory 面のリスク増はない。

### 5.6.3 責務境界（`GateOutcome` はクレームを運ばない）

`RequestGate::check` の戻り値 `GateOutcome` は許可/拒否の判定結果のみを運ぶ
契約（`crates/core/src/extension.rs` doc、`docs/spec/03-poc/hub-wiring-middleware`
PoC-6）であり、JWT 検証で抽出した `org_id` 等のクレームはコアへ一切渡らない
（`fandhe-backend-plugin-hub-wiring` 内の `jwt::Claims` に閉じる）。この境界により、
コアは hub 固有シンボル（JWT・`org_id`・JWKS）へ一切依存しないまま、
依存逆転型プラグインからの利用を受け付けられる。

### 5.6.4 後続 Gate 型プラグインへの適用指針

新規プラグインが `RequestGate`/`Middleware` のみを実装し、判定・観測ロジック
が同期 I/O なしで完結する場合は、まず本パターン（依存逆転、コア無変更）を
検討すること。非同期処理・上流中継・コネクション奪取が必要になった時点で
初めて 4・5 節のコア→プラグインパターンへ切り替える。

## 5.7 Middleware 型パターン（TASK-10.1 / #56 で確立）

`crates/plugin-tracing`（可観測性・サンプリング付きトレーシング）は
`Middleware` 拡張点（`crates/core/src/extension.rs`）上に実装した最初の
プラグインである。パスインターセプト型（4 節）・Upgrade 型（5 節）に続く
第 3 の配線パターンとして記録する。

### 5.7.1 パスインターセプト型・Upgrade 型との違い

`Middleware` trait（`on_request` / `on_response`）は元々 dyn 互換の同期 API
として設計されており、`Server` は既に汎用の `middlewares: Vec<Box<dyn
Middleware>>` を保持している。そのためパスインターセプト型・Upgrade 型が
必要とした「専用の非公開シーム（`crate::plugin::try_intercept` /
`try_handle_upgrade`）」は不要で、`Server::tracing(config)` は
`TracingMiddleware`（`crates/core/src/server.rs`）を組み立てて既存の
`middlewares` へ push するだけの薄いビルダーメソッドとして実装できる
（コアループ `handle_connection` 側の変更はゼロ）。

### 5.7.2 依存方向・循環回避

`fandhe-backend-plugin-tracing` は `fandhe-backend-plugin-websocket`（5.2 節）と同一の非循環パターンを
踏襲し、`fandhe-backend-core` に依存しない。`Middleware` trait を実装する
アダプタ（`TracingMiddleware`）はコア側（`crates/core/src/server.rs`、
`tracing` feature 限定）に置く。`Middleware` は dyn 互換のため、原理的には
プラグイン側が core に依存して順方向に `impl Middleware` する設計も選べたが、
`crates/plugin-tracing` を `crates/core` から独立にビルド・テストできる状態を
維持するため、あえて非循環パターンを踏襲した（`crates/plugin-tracing/src/lib.rs`
の doc・6.1 節の許可リスト例外コメントを参照）。

### 5.7.3 サンプリングと記録タイミングの一本化

`Middleware` には request/response を跨いで per-request 状態を運ぶ経路が
ないため、`on_request` と `on_response` で独立にサンプリング判定すると
同一リクエストの記録が対にならない。`TracingMiddleware::on_request` は
no-op とし、判定・記録は `on_response`（`fandhe_backend_plugin_tracing::TracingLayer::
record_response` への委譲）の 1 点に集約する
（`crates/plugin-tracing/src/layer.rs` の doc を参照）。

採択されたリクエストの記録粒度は当初 span 1 つ + 受理・応答の 2 イベント
（PoC-10 代表構成と同粒度）だったが、TASK-10.2（#57）で応答時 1 イベントへ
統合した。span 廃止により採択 1 件あたりの subscriber コールバックが 4 回
（`on_new_span` + enter/exit + イベント 2 件）から 1 回へ減り、TASK-10.4
（性能再検証）の前提となる記録コスト削減を実現する。

### 5.7.4 AGENTS.md「ミドルウェア非同期 I/O 必須化」規約との関係

`TracingLayer::record_response` 内の `tracing` マクロ呼び出し自体は同期だが、
`tracing-subscriber` に登録するレイヤーが `tracing-appender::non_blocking`
writer を使う限り、実際のディスク/ネットワーク I/O は非同期・バッファ済みに
なる（`fandhe_backend_plugin_tracing::init_tracing` が既定でこの構成を組み立てる）。
サンプリング（`fandhe_backend_plugin_tracing::Sampler`、決定的カウンタ方式）は
PoC-10 の知見（非同期 I/O 化だけでは RPS 劣化 31.6% を解消できない）に
対応する追加対策であり、`Sampler::should_sample` が `false` の場合は
`tracing` マクロ呼び出し自体を避けることで有効化コストをサンプリング間隔に
応じて按分する。

## 5.8 パスインターセプト型の静的サービング変種（TASK-2.1 / #256 で確立）

`crates/plugin-openapi`（`openapi` feature）は 4 節のパスインターセプト型と
同じ「コア → プラグインの optional 依存」+ `plugin::try_intercept` 分岐で
配線するが、2 点の変種を持つ。

- **プラグイン側に非同期ハンドラがない**: `webrtc-proxy`/`webrtc`/`graphql`
  は `fandhe_backend_plugin_*::try_handle_*(head, body, config).await` という
  非同期関数へ委譲するが、`fandhe-backend-plugin-openapi` は定数
  `OPENAPI_JSON` / `OPENAPI_YAML`（`include_str!` によるコンパイル時埋め込み、
  `embed.rs`。YAML 対応は #279）またはイシュー #320 で追加した
  `OpenApiDoc`（利用者アプリ独自スキーマ、`custom.rs`）のバイト列を公開
  するのみでハンドラを持たない（`crates/plugin-openapi/src/lib.rs` の
  「拡張点対応: 非該当」宣言はこのため変更していない。実行時拡張点の契約
  ではなくコンパイル時 feature 着脱に閉じる、
  `docs/design/dependency-graph-contract.md` 5 節）。`plugin::try_intercept`
  側は `head.method == "GET" && head.target == "/openapi.json"`（YAML は
  `/openapi.yaml`）とメソッド・パスの完全一致を判定したうえで
  `server.openapi_registration()`（後述の enum）を参照するだけの同期分岐で
  完結し、`.await` を挟まない
- **設定登録型（enum、イシュー #320 で `bool` トグルから移行）**:
  `webrtc_proxy_config`/`graphql_config` と同様の「設定登録型」パターンだが、
  `Server::openapi()` / `Server::openapi_with(doc)` の 2 メソッドが同一の
  非公開 `OpenApiRegistration`（`Disabled` / `Embedded` / `Custom(OpenApiDoc)`）
  へ書き込む。`Disabled`（既定）では feature が有効でも常にフォールスルー
  （404）する点は他の設定登録型プラグイン（`webrtc-proxy`・`graphql`）と
  同じ。`Embedded` はフレームワーク固定スキーマ（`OPENAPI_JSON`/
  `OPENAPI_YAML`）、`Custom` は利用者アプリが `OpenApiDoc::from_json` で
  検証済みの独自スキーマを配信する。API 構造の開示（内部エンドポイント
  構成の露出）を利用者の明示登録なしに既定公開しないため
  （`.claude/rules/security.md` A01/A05 観点、`Server::openapi` /
  `Server::openapi_with` の doc comment を参照）。両メソッドは排他ではなく
  **後勝ち**（最後に呼んだ方の variant が残る、builder パターンの一般的な
  直感に一致。`crates/core/src/server.rs` の `OpenApiRegistration` doc・
  `crates/core/tests/plugin_openapi_boundary.rs` の
  `openapi_with_takes_precedence_over_earlier_openapi_call` /
  `openapi_takes_precedence_over_earlier_openapi_with_call` を参照）。
  `Custom` の JSON 検証（構文妥当性 + トップレベルオブジェクト）は
  `OpenApiDoc::from_json` 構築時（利用者アプリの起動シーケンス内）に一度
  だけ行い、リクエスト処理経路（`try_intercept`）では再検証しない
  （fail-closed、`crates/plugin-openapi/src/custom.rs` の doc を参照）

## 5.9 レスポンス後処理型パターン（イシュー #305 で確立）

`crates/plugin-cors`（`cors` feature）は既存 4 パターン（パスインターセプト型 /
Upgrade 型 / Gate 型 / Middleware 型）のいずれにも載らない、5 番目のプラグイン
境界パターン。

### 5.9.1 なぜ既存の `Middleware` 拡張点で表現できないか

`Middleware::on_response(&self, head, elapsed)` は「観測専用」契約であり、
レスポンスへの参照・可変参照のいずれも持たない（`crates/core/src/extension.rs`
の `Middleware` doc）。CORS ヘッダ付与は応答内容そのものを変更する処理で
あり、この契約に収まらない。5.7 節の Middleware 型パターン（`plugin-tracing`）
はログ出力・カウンタ更新という副作用に閉じていたため問題化しなかったが、
CORS で初めてこの制約に突き当たった。

### 5.9.2 2 層構成での解決

CORS を「プリフライト」と「実リクエストへのヘッダ付与」の 2 つに分解し、
それぞれ既存の異なる仕組みへ配線する:

1. **プリフライト**（`OPTIONS` + `Origin` + `Access-Control-Request-Method`）:
   `fandhe_backend_routes::Router::options_fallback`（イシュー #304 で
   先行整備した opt-in フック）へ、利用者が
   `fandhe_backend_plugin_cors::preflight_response` を直接配線する。
   `Router` 側は対象パスの実登録メソッド一覧（`AllowedMethods`）を渡せる
   ため、`Access-Control-Allow-Methods` の既定値を設定と実体の乖離なく
   導出できる（明示登録 OPTIONS ルートは常に優先される #304 の契約もそのまま
   維持される）
2. **実リクエストへのヘッダ付与**: コア側に固定シグネチャの新シーム
   `crate::plugin::finalize_response(server, head, response) -> Response`
   を新設し、`handle_connection_with_permit`
   （`crates/core/src/server.rs`）が `try_intercept` 応答・既定 `Handler`
   応答のいずれかを確定させた直後、keep-alive 再判定・`serialize` の直前に
   呼ぶ。3 種の既存シーム（`try_intercept`・`try_handle_upgrade`）と同じく
   `#[cfg(feature = "...")]` を本シーム内部に閉じ、`handle_connection` 本体
   の cfg-free 原則（3 節）を維持する

### 5.9.3 `try_intercept` 応答にも既定 `Handler` 応答にも同一適用できる利点

`finalize_response` は `try_intercept`（graphql・openapi 等のパスインター
セプト型プラグイン応答）と既定 `Handler` 応答の**両方**が確定した後の、
単一の合流点で呼ばれる。`Handler` をラップして CORS ヘッダを注入する設計
（`Server::handler` に渡す前にラッパーで包む）だと `try_intercept` が
`Some` を返した経路（既定 `Handler` を呼ばない経路）にはヘッダが乗らず、
graphql/openapi 応答が CORS 対象外になってしまう。「レスポンス確定後の
単一合流点」という設計判断はこの不整合を避けるための必然。

### 5.9.4 `RequestGate` 拒否応答・パースエラー応答を通さない設計判断

`finalize_response` は `handle_connection_with_permit` 内の
`RequestGate` 拒否応答・パースエラー応答（400 等）の送出経路には接続しない
（呼び出し箇所は `try_intercept`/`Handler::handle` の結果確定後のみ）。
拒否応答は最小情報で返すフェイルクローズ方針（`.claude/rules/security.md`）
を CORS ヘッダ付与によって後退させないための意図的な設計であり、5.6 節
（Gate 型パターン）の「`GateOutcome` はクレームを運ばない」という責務境界
の判断と同根。

### 5.9.5 プリフライトとの二重付与防止

`finalize_response` は `fandhe_backend_plugin_cors::is_preflight(head)` が
`true` を返すリクエストには何もしない。プリフライト応答は 1. の
`options_fallback` 経路で既に完結しているため、同一リクエストに実リクエスト
用の CORS ヘッダ付与ロジックを重ねて適用しないための判定。

### 5.9.6 循環依存の回避・依存関係

`crates/plugin-cors` 自体は `fandhe-backend-core` に依存しない
（`fandhe-backend-http` のみに依存する下位層）。`fandhe-backend-plugin-websocket`・
`fandhe-backend-plugin-tracing` と同一の非循環パターンであり、コア側が
`optional = true` + `dep:` 構文で本クレートへ依存する（6.1 節の
`scripts/dep-direction-check.sh` ホワイトリスト例外 4 を参照）。
`fandhe_backend_routes::Router::options_fallback` の型
（`Fn(&RequestHead, &AllowedMethods, &[u8]) -> Response` 互換クロージャ）は
素の関数ポインタで満たせるため、`crates/plugin-cors` は
`fandhe-backend-routes` にも依存しない。

## 5.10 レスポンス後処理型パターンの第 2 インスタンス（イシュー #321、圧縮）

`crates/plugin-compression`（`compression` feature）は 5.9 節が確立した
「レスポンス後処理型」シームの 2 例目。CORS がプリフライト・実リクエスト
ヘッダ付与の 2 層構成を要したのに対し、圧縮は単層（実リクエスト応答の
body 書き換えのみ）で完結する。

### 5.10.1 逐次適用順（CORS → 圧縮固定）

`finalize_response` は複数のレスポンス後処理型プラグインを**逐次適用**
できるよう再構成した（5.9.2 節時点は CORS 単独のため早期 return で足りたが、
2 例目の追加でこの構造は成り立たなくなった）。適用順は CORS → 圧縮の順に
固定する。理由は圧縮が「最終 body を確定させる後処理」であり、以降に
別の後処理型プラグインが body へ触れる余地を残さないため。CORS はヘッダ
のみで body に触れないため本イシュー時点では順序自体が結果へ影響しないが、
将来 body に触れる 3 例目が追加された際に迷わないよう、規約として
明文化する。

### 5.10.2 圧縮判定条件と `Response::header` の新設

CORS 実装時は `Response` に読み取り API（ゲッター）が存在しなくても
（`Origin` ヘッダの有無で分岐するだけで）成立したが、圧縮判定は自身が
下す前の**レスポンス側の状態**（実効 `Content-Type`・既存 `Content-Encoding`
の有無）を読む必要がある。この必要から `crates/http/src/response.rs` に
[`Response::header`](../../crates/http/src/response.rs) を追加した（イシュー
#301 で追加された `with_header` の書き込み系 API 群に対する読み取り系の
初例）。`serialize` の優先順位（専用フィールド `content_type` が
`extra_headers` の同名エントリより優先）と同じ解決順序を維持することで、
圧縮判定が見る値と実際にワイヤへ出る値の乖離を防ぐ。

### 5.10.3 CPU コストと「同期ブロッキング I/O 禁止」規約の非該当性

`Middleware` の同期ブロッキング I/O 禁止規約（`.claude/rules/coding-rust.md`、
PoC-3・5.7.4 節）は I/O 待ちで tokio ワーカスレッドを占有する挙動を対象と
する。gzip 圧縮は同期だが CPU バウンドの処理であり、I/O 待ちを発生させない
（ネットワーク・ディスク双方に触れない、メモリ上の `Vec<u8>` 変換のみ）。
このため本規約の対象外と判断した。ただし CPU 処理自体のコストがゼロに
なるわけではなく、作業量はコア既存のリクエストボディサイズ上限
（`fandhe_backend_http::body::MAX_BODY_BYTES`）・ハンドラの生成物サイズに
より有界という前提の上に成り立つ。巨大応答の圧縮を tokio ワーカから
切り離す `spawn_blocking` 化・チャンク単位のストリーミング圧縮は
スコープ外とし、`.claude/rules/out-of-scope-tracking.md` に従い後続課題
として追跡する（#319 のレスポンス側ストリーミング送信に依存するため、
その完了後に着手可能になる）。

### 5.10.4 BREACH 類似リスクと opt-in 設計の関係

TLS 上の圧縮応答で秘密情報と攻撃者制御入力が混在すると、圧縮後サイズの
観測から秘密が推測されうる（BREACH 類似の攻撃）。本プラグインはこの
リスクを実装で完全には解消できない（HTTP 層の圧縮機構そのものに内在する
特性のため）。他の設定登録型プラグインと同じ「未登録なら feature が
有効でも無効化」という opt-in 設計が、このリスクに対する第一の防御線
として機能する。加えて `CompressionConfig` の対象 `Content-Type`
許可リスト・最小サイズ閾値を利用者が調整できる API を提供し、秘密情報を
含みやすいエンドポイントを個別に対象外化できるようにした
（`crates/plugin-compression/src/lib.rs` の crate doc・`CompressionConfigBuilder::compressible_types`
の doc を参照。具体的な攻撃手順はここに記載しない、
`.claude/rules/feasibility-guardrail.md` の方針）。

### 5.10.5 循環依存の回避・依存関係

`crates/plugin-compression` 自体は `fandhe-backend-core` に依存しない
（`fandhe-backend-http` + `flate2` にのみ依存する下位層）。`crates/plugin-cors`
と同一の非循環パターンであり、コア側が `optional = true` + `dep:` 構文で
本クレートへ依存する（6.1 節の `scripts/dep-direction-check.sh` ホワイト
リスト例外 6 を参照）。

## 5.11 パスインターセプト型の `spawn_blocking` ファイル I/O 変種（イシュー #318 で確立）

`fandhe-backend-plugin-static`（静的ファイル配信プラグイン）は `try_intercept`
（4 節）を使う設定登録型プラグインだが、他の `try_intercept` 実装（GraphQL・
WebRTC 中継）が非同期の上流通信を要するのに対し、本プラグインは同期的な
ファイルシステム I/O（`canonicalize`・`metadata`・`read`）を要する点が異なる。

### 5.11.1 なぜ `Router` ハンドラ（同期シグネチャ）に載せられないか

`fandhe_backend_routes::Router` へ登録するハンドラは
`Fn(&RequestHead, &[u8]) -> Response` という同期シグネチャに固定されている
（`crates/routes` の設計）。ファイル読み込みは（大きなファイル・低速ストレージ
下で）ブロッキング I/O であり、`.claude/rules/coding-rust.md` の「Tokio 上で
ブロッキング処理を await スレッドで実行しない」規約により `spawn_blocking` へ
逃がす必要があるが、同期ハンドラの内部から非同期 `spawn_blocking().await` を
呼ぶことはできない。そのため `Router` 経由ではなく、`try_intercept`
（非同期シーム）へ `Server::static_files(config)` の設定登録型として配線する。

### 5.11.2 `spawn_blocking` の隔離範囲

`fandhe_backend_plugin_static::try_handle_static`（非同期）はメソッド・
マウントプレフィックス判定・字句検証（`.`/`..`/空/NUL/`\`/先頭ドット
セグメント拒否）までを同期で行い、実際のファイルシステム呼び出し（`canonicalize`・
`metadata`・`read`）のみを単一の `tokio::task::spawn_blocking` クロージャへ
まとめて委譲する（`crates/plugin-static/src/lib.rs` の `resolve_and_read`）。
`spawn_blocking` 自体が失敗する（内部で panic した）場合もフェイルクローズで
404 を返し、`try_intercept` 呼び出し元（`handle_connection` の非同期タスク）を
ブロックしない。

### 5.11.3 フェイルクローズ設計（二層防御）

1. **I/O 前の字句検証**: 末尾パスをセグメント分割し、空・`.`・`..`・NUL・
   `\`・先頭が `.` のセグメント（ドットファイル・ドットディレクトリ）を
   含むセグメントを拒否する。パーセントデコードは行わない
   （`crates/routes/src/pattern.rs` の `is_safe_segment_value` と同一の
   「正規化しない」方針を踏襲）。先頭ドット拒否は、公開 root 配下に
   `.env`・`.git/config`・`.htpasswd` 等の機密ファイルが置かれた場合の
   意図しない配信（OWASP A01/A05）を防ぐフェイルクローズ判断で、イシュー
   #318 のレビュー指摘（`.` 始まり通常ファイル名が拒否対象から漏れていた）
   を受けて追加した
2. **`canonicalize` 後の実パス検証**: 正規化済み実パスが正規化済み root
   配下（`starts_with`）であることを確認し、シンボリックリンク経由の
   root 脱出を拒否する

ファイル未検出・検証失敗・権限エラー・サイズ超過（`max_file_bytes`）は
一律 404（存在オラクル・列挙を作らないフェイルクローズ、
`.claude/rules/security.md`）。ディレクトリリスティングは実装しない。

末尾スラッシュ 1 個（`<mount>/dir/`）は「ディレクトリ要求」として受理し
`dir/index.html` を解決する（SSG が生成する `/posts/hello/` 形式の URL
互換、イシュー #418）。除去は 1 個のみに限定し、連続スラッシュ（`//`）は
除去後も空セグメントとして残るため引き続き一律拒否される（上記 1 の
「正規化しない」方針を後退させない）。末尾スラッシュ付き要求が通常
ファイルへ解決された場合も一律 404 とし、存在オラクルを作らない。
301 リダイレクトによる URL 正規化（提案の別案）は、拡張点でレスポンス
改変ができない制約（イシュー #420）と重なるため本クレートのスコープ外。

### 5.11.4 循環依存の回避・依存関係

`crates/plugin-static` 自体は `fandhe-backend-core` に依存しない
（`fandhe-backend-http` のみに依存する下位層）。`fandhe-backend-plugin-websocket`・
`fandhe-backend-plugin-cors` と同一の非循環パターンであり、コア側が
`optional = true` + `dep:` 構文で本クレートへ依存する（6.1 節の
`scripts/dep-direction-check.sh` ホワイトリスト例外 7 を参照）。MIME 推定は
crate 内蔵の静的テーブル（`src/mime.rs`）で行うため、外部 crates.io 依存
（`mime_guess` 等）は追加しない。

### 5.11.5 MIME 解決の 2 段構成（イシュー #423）

内蔵テーブルに `.webmanifest`（`application/manifest+json`）等の PWA/SSG
配信頻出拡張子を追加しつつ、テーブルに存在しない拡張子を利用者が個別に
補える経路を `StaticFilesConfigBuilder::mime(ext, content_type)` として
追加した。解決順序は「利用者オーバーライド（`StaticFilesConfig::
mime_overrides`）→ 内蔵テーブル（`mime::TABLE`）→ 既定値
`application/octet-stream`」の 2 段フォールバックで、未知拡張子は従来どおり
安全側の既定値へ倒れる（フェイルクローズ方針を後退させない）。

`content_type` の型を `&'static str` に限定しているのは、`Response::
with_content_type` が同じ制約を持つ既存設計（5.10 節・`crates/core` の
`plugin.rs` 冒頭コメント）と揃え、リクエスト由来の動的文字列がレスポンス
ヘッダへ流入する経路を型レベルで排除するため。`StaticFilesConfigBuilder::
mime` 自体は失敗せず、拡張子（非空・`.`/`/`/`\`/NUL・制御文字禁止）と
`content_type`（非空・CR/LF 等の制御文字禁止）の検証は `build()` に集約する
（`StaticConfigError::InvalidMimeMapping`）。`Response::with_content_type`
内部の CRLF 検査は `debug_assert!` のみでリリースビルドでは無効化されるため、
`build()` での構築時検証がヘッダインジェクション（OWASP A03、
`.claude/rules/security.md`）を遮断する唯一の確実な防御線になる。

## 5.12 設定登録型プラグインの設定型 core 再エクスポート（イシュー #421）

`static`（5.11 節）・`compression`（5.10 節）のような「設定登録型」プラグインは、
`Server::static_files(config)` / `Server::compression(config)` へ渡す設定型
（`StaticFilesConfig` / `CompressionConfig`）を利用者が構築するために、従来は
プラグインクレート（`fandhe-backend-plugin-static` / `-compression`）への
直接依存が別途必要だった。`crates/core/src/lib.rs` は対象 feature 有効時のみ
プラグインクレートをモジュールとして丸ごと再エクスポートする
（`#[cfg(feature = "static")] pub use fandhe_backend_plugin_static as
plugin_static;` 等）。利用者は `fandhe-backend-core` への依存 + feature 指定
だけで `fandhe_backend_core::plugin_static::StaticFilesConfig` /
`fandhe_backend_core::plugin_compression::CompressionConfig` を構築できる。

- **whole-crate 再エクスポートを採用**（型単位の個別 `pub use` は採用しない）:
  プラグイン側に付随型（`StaticFilesConfigBuilder` / `StaticConfigError` /
  `CompressionConfigBuilder` 等）が増えても本再エクスポートの追随が不要になる。
  型単位の再エクスポートはプラグイン間の名前衝突リスクを生むため見送った
- feature 無効時は再エクスポート自体が `#[cfg(feature = ...)]` で消え、依存も
  `dep:` 構文により `cargo tree` から消える（pay-for-what-you-use は不変）
- ハンドラ本体（`try_handle_static`・`apply_compression` 等）はコア内部の
  非公開 `plugin` モジュール（4.2 節・5.9 節のシーム）から呼ばれる実装詳細で
  あり、本再エクスポート経由での直接利用は想定しない
- 本パターンは `static` / `compression` の 2 feature のみに適用済み。他の
  設定登録型 feature（`websocket` / `graphql` / `cors` / `tracing` /
  `openapi` / `webrtc` 系）への水平展開は本イシューのスコープ外（フォロー
  アップ候補として記録、[[out-of-scope-tracking]]）

## 6. 検証コマンド

| 検証 | コマンド | 期待結果 |
|------|---------|---------|
| 依存除外 | `cargo tree -p fandhe-backend-core` | `fandhe-backend-plugin-webrtc-proxy` が 0 件 |
| 依存有効化 | `cargo tree -p fandhe-backend-core --features webrtc-proxy` | `fandhe-backend-plugin-webrtc-proxy` が出現 |
| 全構成ビルド | `cargo build -p fandhe-backend-core`（無効）／`--features webrtc-proxy`／`cargo build --workspace --all-features` | すべて成功 |
| テスト | `cargo test -p fandhe-backend-core`（無効）／`--features webrtc-proxy`／`cargo test --workspace --all-features` | すべて green（`crates/core/tests/plugin_boundary.rs`・`plugin_boundary_disabled.rs`） |
| lint | `cargo clippy -p fandhe-backend-core --all-targets --no-default-features -- -D warnings`／`--features webrtc-proxy`／`cargo clippy --workspace --all-targets --all-features -- -D warnings` | 警告 0 件 |
| doc | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps` | 警告 0 件 |
| 依存監査 | `scripts/dep-audit.sh` | `webrtc-proxy`・`webrtc`・`websocket` を含む動的列挙構成で違反 0 件（`webrtc` feature 有効化に伴い `deny.toml` の許可ライセンスへ `ISC` を追加済み） |
| pay-for-what-you-use 機械検証 | `scripts/pay-for-what-you-use-check.sh`（TASK-2.2、#19） | cargo tree/geiger・バイナリサイズ・全構成ビルドすべて PASS（`docs/design/pay-for-what-you-use-check.md` 参照） |

`websocket` feature（TASK-4.1 / #22）も同一パターンで検証済み:
`cargo tree -p fandhe-backend-core --features websocket` で
`fandhe-backend-plugin-websocket`・`tokio-tungstenite` が出現し、`webrtc-rs` 系は
出現しない。`crates/core/tests/websocket_upgrade.rs`（feature 有効側）・
`websocket_upgrade_disabled.rs`（feature 無効側）で green。

`openapi` feature（TASK-2.1 / #256）も同一パターンで検証済み:
`cargo tree -p fandhe-backend-core --features openapi` で
`fandhe-backend-plugin-openapi`・`utoipa` 系が出現し、他プラグインは出現しない。
`crates/core/tests/plugin_openapi_boundary.rs`（feature 有効側、未登録
フォールスルー・メソッド不一致・無関係パスも併せて検証）・
`plugin_openapi_boundary_disabled.rs`（feature 無効側）で green。

## 6.1 `scripts/dep-direction-check.sh` ホワイトリストの例外（TASK-1.5 との整合）

`crates/core/Cargo.toml` の `fandhe-backend-plugin-webrtc-proxy` optional 依存
（2 節）は `fandhe-backend-core → fandhe-backend-plugin-webrtc-proxy` という workspace
内 path 依存エッジを生む。`scripts/dep-direction-check.sh`（TASK-1.5、#14）は
本来「コアからのプラグイン依存は禁止」を既定とするホワイトリスト方式だが、
本タスク（TASK-2.1）着手時点でこの前提と `docs/spec/04-requirements.md`
REQ-2 が要求する「feature flag + `dep:` 構文によるコンパイル時プラグイン
機構」が衝突することが判明した（PR #129、Cursor Bugbot 指摘）。

3 拡張点（`Middleware`/`UpgradeHandler`/`RequestGate`）はいずれも dyn
互換性のため同期 API に限定されており（`crates/core/src/extension.rs`
冒頭 doc）、上流への非同期中継を伴うパスインターセプト型プラグイン
（`try_handle_rtc_offer` は `async fn`）を既存拡張点経由の依存逆転
（プラグイン側のみが core に依存する形）で表現できない。このため
`scripts/dep-direction-check.sh` の許可リストへ
`fandhe-backend-core:fandhe-backend-plugin-webrtc-proxy` を明示的な例外として
1 件のみ追加した（`fandhe-backend-plugin-*` への一般化はしない。新規プラグインが
同パターンを踏襲する場合は許可リストへの個別追加とレビューを要求する）。
feature 無効時は本エッジ自体が未解決のまま消えるため pay-for-what-you-use
は維持される（6 節の検証コマンドで確認済み）。詳細な例外根拠・DFS 循環
検出との関係は `scripts/dep-direction-check.sh` の当該コメントを正とする。

TASK-8.1（#26）は同一理由（3 拡張点の同期 API 限定に非同期呼び出しを持ち込め
ない）で `fandhe-backend-core:fandhe-backend-plugin-webrtc` を 2 件目の個別例外として
許可リストへ追加した。チェック 3（プラグイン非依存検査）の除外パターンも
`fandhe_backend_plugin_webrtc\b`（`fandhe_backend_plugin_webrtc_proxy` の部分文字列にならないよう
単語境界付き）・`webrtc_config` を追加して対応済み（`scripts/dep-direction-check.sh`
本体コメント参照）。

TASK-4.1（#22）で `fandhe-backend-core:fandhe-backend-plugin-websocket` を同一方針で
3 件目の例外として追加した（`fandhe-backend-plugin-websocket` 自体は 5.2 節のとおり
`fandhe-backend-core` に依存しないため循環にはならない）。あわせて
チェック 3（プラグイン固有シンボル非依存検査）の例外シンボルパターンにも
`fandhe_backend_plugin_websocket`/`websocket` を追加している。

TASK-10.1（#56）で `fandhe-backend-core:fandhe-backend-plugin-tracing` を 4 件目の
例外として追加した。`Middleware` trait は dyn 互換の同期 API のため、
webrtc-proxy/webrtc（非同期パスインターセプト）とは異なる理由（5.6.2 節）で
非循環パターンを選んだが、生じる workspace 内 path 依存エッジ自体は
websocket と同型（`fandhe-backend-plugin-tracing` → `fandhe-backend-core` の逆依存は
発生しない）。チェック 3 の例外シンボルパターンにも `fandhe_backend_plugin_tracing`/
`TracingMiddleware` を追加している。

イシュー #305 で `fandhe-backend-core:fandhe-backend-plugin-cors` を 5 件目の
例外として追加した。5.9 節のとおり `Middleware` 拡張点では表現できないため
新設したレスポンス後処理型シーム（`crate::plugin::finalize_response`）が
生む workspace 内 path 依存エッジであり、`fandhe-backend-plugin-cors` 自体は
websocket/tracing と同型で `fandhe-backend-core` に依存しない非循環パターン。
チェック 3 の例外シンボルパターンにも `fandhe_backend_plugin_cors` を追加している。

イシュー #321 で `fandhe-backend-core:fandhe-backend-plugin-compression` を 6 件目の
例外として追加した。5.10 節のとおりレスポンス後処理型シームの第 2
インスタンスであり、`fandhe-backend-plugin-compression` 自体は cors/websocket/
tracing と同型で `fandhe-backend-core` に依存しない非循環パターン。
チェック 3 の例外シンボルパターンにも `fandhe_backend_plugin_compression`/
`compression_config` を追加している。

イシュー #318 で `fandhe-backend-core:fandhe-backend-plugin-static` を 7 件目の
例外として追加した。5.11 節のとおりパスインターセプト型 `try_intercept`
（4 節）の `spawn_blocking` ファイル I/O 変種であり、
`fandhe-backend-plugin-static` 自体は cors/compression/websocket/tracing と
同型で `fandhe-backend-core` に依存しない非循環パターン。チェック 3 の
例外シンボルパターンにも `fandhe_backend_plugin_static`/`static_files` を
追加している。

## 7. スコープ外（別タスクで対応）

- `cargo tree`/`cargo geiger`/バイナリサイズ比較の機械的検証スクリプト整備 →
  TASK-2.2（#19）で整備済み（`scripts/pay-for-what-you-use-check.sh`、
  `docs/design/pay-for-what-you-use-check.md` 参照）
- Middleware 非同期 I/O 必須化規約の AGENTS.md 整備 → TASK-2.3（#20）
- 2 プラグイン着脱受け入れテスト・コンパイル時 vs 動的ロードのトレードオフ設計文書 →
  TASK-2.4（#21）で整備済み（`graphql` feature（`crates/plugin-graphql`）+ 既存
  `webrtc-proxy` feature の 2 インスタンスで実証。`docs/design/plugin-loading-tradeoffs.md`・
  `docs/acceptance/req2-plugin-mechanism.md` を参照。TASK-2.4 着手時点で実 WebSocket
  プラグイン（`crates/plugin-websocket`、TASK-4.1 / #22）が別 PR（#137）で並行実装中
  だったため、「WebSocket」の代わりに本モジュール同型のパスインターセプト型第 2
  インスタンスとして GraphQL を選定した経緯は `crates/plugin-graphql` の doc コメントを
  参照。実 WebSocket 自体はその後 #137 のマージにより `websocket` feature として
  別途配線済み。GraphQL 側も TASK-2.4 時点は `POST /graphql` への固定応答スタブに
  留まっていたが、TASK-5.1（#38）で `async-graphql` による実クエリ実行へ差し替え、
  `webrtc-proxy`・`webrtc` と同じ設定登録型パターン（スキーマ未登録時はフォール
  スルー）に揃えた）
- `chunk` バッファのヒープ化・委譲後タスク再 spawn による RSS 最適化 → TASK-4.2
- 10,000 同時接続負荷試験・RSS 再計測 → TASK-4.3
- プラグイン無効時の依存・unsafe・バイナリ 0 件の機械的受け入れテスト → TASK-4.4 / TASK-2.4（#21）
- ユーザー定義 WebSocket メッセージハンドラ API（エコー以外のアプリケーション
  ロジック差し込み）→ 対応 Issue なし（Issue #22 実装計画 8 節、PR 本文で
  新規 Issue 化を提案）
- WebSocket 接続のアイドルタイムアウト・ping/pong ヘルスチェック → 対応 Issue
  なし（DoS 対策の深掘り、PR 本文で提案）
