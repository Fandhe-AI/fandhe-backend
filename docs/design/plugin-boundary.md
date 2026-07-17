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
| `try_handle_upgrade`（`server.rs` 内非公開関数） | 長時間接続（WebSocket 等）への委譲 | 実 WebSocket プラグイン（TASK-4.1）配線まではスタブ（常に `Some(stream)`） |
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

## 5. Upgrade 型パターンへの適用指針（後続タスク向け）

本タスクでは `try_handle_upgrade` の実差し替えは行わない（実 WebSocket
プラグインが TASK-4.1 まで未実装のため）。後続タスクが差し替える際は、
`try_intercept` と同型の設計原則を踏襲する:

1. feature 命名規約（2 節）に従い `dep:` 構文で optional 依存を追加する
2. `try_handle_upgrade` のシグネチャを変えずに内部を cfg-gated 分岐へ差し替える
3. `Server` ビルダーへ cfg-gated な登録メソッドを追加する
4. feature 無効時はコード・依存・`unsafe` が完全に消えることを
   `cargo tree` で確認する

## 6. 検証コマンド

| 検証 | コマンド | 期待結果 |
|------|---------|---------|
| 依存除外 | `cargo tree -p backend-framework-core` | `bf-plugin-webrtc-proxy` が 0 件 |
| 依存有効化 | `cargo tree -p backend-framework-core --features webrtc-proxy` | `bf-plugin-webrtc-proxy` が出現 |
| 全構成ビルド | `cargo build -p backend-framework-core`（無効）／`--features webrtc-proxy`／`cargo build --workspace --all-features` | すべて成功 |
| テスト | `cargo test -p backend-framework-core`（無効）／`--features webrtc-proxy`／`cargo test --workspace --all-features` | すべて green（`crates/core/tests/plugin_boundary.rs`・`plugin_boundary_disabled.rs`） |
| lint | `cargo clippy -p backend-framework-core --all-targets --no-default-features -- -D warnings`／`--features webrtc-proxy`／`cargo clippy --workspace --all-targets --all-features -- -D warnings` | 警告 0 件 |
| doc | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps` | 警告 0 件 |
| 依存監査 | `scripts/dep-audit.sh` | `webrtc-proxy` を含む動的列挙構成で違反 0 件 |

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

## 7. スコープ外（別タスクで対応）

- `cargo tree`/`cargo geiger`/バイナリサイズ比較の機械的検証スクリプト整備 → TASK-2.2（#19）
- Middleware 非同期 I/O 必須化規約の AGENTS.md 整備 → TASK-2.3（#20）
- WebSocket・GraphQL の 2 プラグイン着脱受け入れテスト、コンパイル時 vs
  動的ロードのトレードオフ設計文書 → TASK-2.4（#21）
- `try_handle_upgrade` の実プラグイン（plugin-websocket）委譲差し替え → TASK-4.1
