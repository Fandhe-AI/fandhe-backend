# プラグイン境界パターン（TASK-2.1 / #18）

対応: `docs/spec/05-tasks.md` TASK-2.1（REQ-2、`docs/spec/04-requirements.md`）。
MS-1（`docs/spec/06-roadmap.md`）。前提タスク TASK-1.4（#13）完了後に着手。

## 1. 背景

`crates/core` は「最小コア + Cargo feature 駆動プラグイン」を核とする設計だが、
本タスク着手時点では feature による着脱を実例で示した実装が存在しなかった。
一方、プラグインクレート `bf-plugin-webrtc-proxy`（#74）はハンドラ単体では
自己完結していたが、コアの接続受理ループへ未配線だった。

本ドキュメントは、`webrtc-proxy` feature を第 1 号として確立した「feature flag
+ `dep:` 構文によるプラグイン境界パターン」を記述し、後続プラグイン
（websocket / graphql / openapi / hub-wiring / tracing）が同パターンを踏襲する
際の指針とする。

## 2. feature 命名規約

プラグインクレート名（`bf-plugin-<name>`）から `bf-plugin-` 接頭辞を除いた
`<name>` を feature 名とする。

```toml
# crates/core/Cargo.toml
[dependencies]
bf-plugin-webrtc-proxy = { path = "../plugin-webrtc-proxy", optional = true }

[features]
default = []
webrtc-proxy = ["dep:bf-plugin-webrtc-proxy"]
```

- `optional = true` + `dep:` 構文を使う。`dep:` を使わずに `optional = true`
  だけで feature を作ると、依存クレート名と同名の **implicit feature** が
  暗黙に生えてしまい、公開 feature 名の意図しない増加・利用者からの誤参照を
  招く。`dep:bf-plugin-webrtc-proxy` は implicit feature を作らず、
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
| `plugin::try_intercept`（`crates/core/src/plugin.rs`） | リクエスト/レスポンス完結型プラグインへのパスインターセプト | `webrtc-proxy` で配線済み（本タスク） |

## 4. パスインターセプト型パターン（本タスクで確立）

`plugin::try_intercept(server: &Server, head: &RequestHead, body: &[u8]) ->
Option<bf_http::response::Response>` が固定シグネチャのシーム。

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
                bf_plugin_webrtc_proxy::try_handle_rtc_offer(head, body, config).await
        {
            return Some(from_plugin_response(response));
        }
    }

    #[cfg(not(feature = "webrtc-proxy"))]
    {
        let _ = (server, head, body);
    }

    None
}
```

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
pub fn webrtc_proxy(mut self, config: bf_plugin_webrtc_proxy::ProxyConfig) -> Self {
    self.webrtc_proxy_config = Some(config);
    self
}
```

feature 無効時はこのメソッド・対応するフィールドが構造体から完全に消える
（`#[cfg(feature = "...")]` をフィールド定義・`Default` 実装の両方に付与する）。

### 4.3 応答の変換と Content-Type

プラグイン側の中間表現（例: `bf_plugin_webrtc_proxy::Response { status,
reason, content_type, body }`）はコアが送出する `bf_http::response::Response`
へ変換する。`bf_http::response::Response` は任意ヘッダ API を意図的に持たない
（レスポンス分割対策、`crates/http/src/response.rs` の doc）ため、本タスクで
`&'static str` 限定の `Response::with_content_type` を追加した。プラグイン側の
`content_type` フィールドも `&'static str` に限定されているため、変換経路に
外部入力由来の動的文字列が混入する余地はない。

`reason` phrase はプラグイン側の値をそのまま使わず、`bf_http::response::Response`
内蔵の固定テーブル（`reason_phrase`）から `status` に基づいて引く。プラグインが
新しいステータスコードを払い出す場合は、このテーブルへのエントリ追加を
忘れないこと（本タスクでは `502 Bad Gateway` / `504 Gateway Timeout` を追加
した。追加を怠ると `HTTP/1.1 502 \r\n` のように reason phrase が空文字へ
劣化する。PoC-9 教訓: ステータスコードのみの検証はこの劣化を見逃す。統合
テストは必ず reason/Content-Type/body まで含めて検証すること）。

## 5. Upgrade 型パターン（TASK-4.1 / #22 で確立）

`bf-plugin-websocket`（`crates/plugin-websocket`）が Upgrade 型パターンの
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
            && bf_plugin_websocket::matches(head, config)
        {
            let _ = bf_plugin_websocket::handle_upgrade(stream, head, leftover, config).await;
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
  フレーム等）を委譲先へ引き継ぐ必要がある。`bf_plugin_websocket::handle_upgrade`
  はこれを `WebSocketStream::from_partially_read` へそのまま渡し、先行到着
  フレームを取りこぼさない
- `&Server`: 複数 Upgrade 型プラグインが将来増えた場合でも、各プラグインの
  cfg-gated 設定（`server.websocket_config()` 等）へ本シーム経由で
  アクセスできるようにするための一般化。`&[Box<dyn UpgradeHandler>]` では
  「委譲判定のみ」の情報しか持てず、ハンドシェイク詳細検証・フレーミング
  設定に必要な `WebSocketConfig` を渡せなかった

`UpgradeHandler::matches`（同期 API、委譲判定のみの契約）自体は変更して
いない。判定は `WebSocketUpgradeAdapter`（`crates/core/src/server.rs`、
`Server::websocket` が内部登録）が `bf_plugin_websocket::matches` へ委譲する
薄いラッパーとして担う。

### 5.2 循環依存の回避

`bf-plugin-websocket` は `backend-framework-core` に依存しない（`crates/plugin-websocket/src/lib.rs`
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
   `backend-framework-core:bf-plugin-<name>` を個別追加する（6.1 節と同じ
   方針。`bf-plugin-*` への一般化はしない）
6. feature 無効時はコード・依存・`unsafe` が完全に消えることを
   `cargo tree` で確認する

### 5.4 委譲後の専用タスク再 spawn + permit 引き継ぎ（TASK-4.2 / #23【条件(1)】）

PoC-7（`docs/spec/03-poc/high-concurrency-scale/README.md`）実測で、WebSocket
長時間接続の接続あたり RSS が axum 比 155.2%（Conditional Go 条件(1) の成功
基準 110% 未達）となった。原因は、`try_handle_upgrade` が
`bf_plugin_websocket::handle_upgrade`（ハンドシェイク + エコーループ）を
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
            .find(|config| bf_plugin_websocket::matches(head, config))
        {
            let config = config.clone();
            let head = head.clone();
            let permit = permit.take();          // permit をセッションタスクへ move
            tokio::spawn(async move {
                let _permit = permit;             // セッション終了まで保持
                let _ = bf_plugin_websocket::handle_upgrade(stream, &head, leftover, &config).await;
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

## 6. 検証コマンド

| 検証 | コマンド | 期待結果 |
|------|---------|---------|
| 依存除外 | `cargo tree -p backend-framework-core` | `bf-plugin-webrtc-proxy` が 0 件 |
| 依存有効化 | `cargo tree -p backend-framework-core --features webrtc-proxy` | `bf-plugin-webrtc-proxy` が出現 |
| 全構成ビルド | `cargo build -p backend-framework-core`（無効）／`--features webrtc-proxy`／`cargo build --workspace --all-features` | すべて成功 |
| テスト | `cargo test -p backend-framework-core`（無効）／`--features webrtc-proxy`／`cargo test --workspace --all-features` | すべて green（`crates/core/tests/plugin_boundary.rs`・`plugin_boundary_disabled.rs`） |
| lint | `cargo clippy -p backend-framework-core --all-targets --no-default-features -- -D warnings`／`--features webrtc-proxy`／`cargo clippy --workspace --all-targets --all-features -- -D warnings` | 警告 0 件 |
| doc | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps` | 警告 0 件 |
| 依存監査 | `scripts/dep-audit.sh` | `webrtc-proxy`・`websocket` を含む動的列挙構成で違反 0 件 |
| pay-for-what-you-use 機械検証 | `scripts/pay-for-what-you-use-check.sh`（TASK-2.2、#19） | cargo tree/geiger・バイナリサイズ・全構成ビルドすべて PASS（`docs/design/pay-for-what-you-use-check.md` 参照） |

`websocket` feature（TASK-4.1 / #22）も同一パターンで検証済み:
`cargo tree -p backend-framework-core --features websocket` で
`bf-plugin-websocket`・`tokio-tungstenite` が出現し、`webrtc-rs` 系は
出現しない。`crates/core/tests/websocket_upgrade.rs`（feature 有効側）・
`websocket_upgrade_disabled.rs`（feature 無効側）で green。

## 6.1 `scripts/dep-direction-check.sh` ホワイトリストの例外（TASK-1.5 との整合）

`crates/core/Cargo.toml` の `bf-plugin-webrtc-proxy` optional 依存
（2 節）は `backend-framework-core → bf-plugin-webrtc-proxy` という workspace
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
`backend-framework-core:bf-plugin-webrtc-proxy` を明示的な例外として
1 件のみ追加した（`bf-plugin-*` への一般化はしない。新規プラグインが
同パターンを踏襲する場合は許可リストへの個別追加とレビューを要求する）。
feature 無効時は本エッジ自体が未解決のまま消えるため pay-for-what-you-use
は維持される（6 節の検証コマンドで確認済み）。詳細な例外根拠・DFS 循環
検出との関係は `scripts/dep-direction-check.sh` の当該コメントを正とする。

TASK-4.1（#22）で `backend-framework-core:bf-plugin-websocket` を同一方針で
2 件目の例外として追加した（`bf-plugin-websocket` 自体は 5.2 節のとおり
`backend-framework-core` に依存しないため循環にはならない）。あわせて
チェック 3（プラグイン固有シンボル非依存検査）の例外シンボルパターンにも
`bf_plugin_websocket`/`websocket` を追加している。

## 7. スコープ外（別タスクで対応）

- `cargo tree`/`cargo geiger`/バイナリサイズ比較の機械的検証スクリプト整備 →
  TASK-2.2（#19）で整備済み（`scripts/pay-for-what-you-use-check.sh`、
  `docs/design/pay-for-what-you-use-check.md` 参照）
- Middleware 非同期 I/O 必須化規約の AGENTS.md 整備 → TASK-2.3（#20）
- WebSocket・GraphQL の 2 プラグイン着脱受け入れテスト、コンパイル時 vs
  動的ロードのトレードオフ設計文書 → TASK-2.4（#21）
- `chunk` バッファのヒープ化・委譲後タスク再 spawn による RSS 最適化 → TASK-4.2
- 10,000 同時接続負荷試験・RSS 再計測 → TASK-4.3
- プラグイン無効時の依存・unsafe・バイナリ 0 件の機械的受け入れテスト → TASK-4.4 / TASK-2.4（#21）
- ユーザー定義 WebSocket メッセージハンドラ API（エコー以外のアプリケーション
  ロジック差し込み）→ 対応 Issue なし（Issue #22 実装計画 8 節、PR 本文で
  新規 Issue 化を提案）
- WebSocket 接続のアイドルタイムアウト・ping/pong ヘルスチェック → 対応 Issue
  なし（DoS 対策の深掘り、PR 本文で提案）
