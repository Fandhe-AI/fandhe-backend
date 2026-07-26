# 拡張点自作ガイド

fandhe-backend のコアは、拡張点を 3 種の trait（`Middleware` / `UpgradeHandler` /
`RequestGate`、定義は `crates/core/src/extension.rs`）に集約する。新機能を追加する
ときは、まずこの 3 種のいずれかに載るかを検討する（[`.claude/rules/coding-rust.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/coding-rust.md)
の設計原則）。[`tutorial.md`](./tutorial.md) は `Middleware` の一例のみを扱うため、
本ページでは 3 種の契約を比較した上で、それぞれの自作方法と守るべき規約を示す。

各 trait の完全な実装例（doc test として `cargo test --doc -p fandhe-backend-core`
で検証される）は `crates/core/src/extension.rs` の doc comment を正とする。
本ページのコード断片は要点の抜粋であり、二重管理をしない（[`README.md`](./README.md)
の原則）。

## 3 拡張点の契約比較

| trait | 呼ばれるタイミング | できること | できないこと |
|-------|-------------------|-----------|-------------|
| `RequestGate` | ルーティング・アップグレード判定より**前** | `GateOutcome::Allow` / `Reject` による早期拒否（認証・認可・同意ゲート等） | レスポンス内容の加工（拒否時の status / body 指定のみ） |
| `UpgradeHandler` | `RequestGate` 通過後、既定 `Handler` より前 | 長時間接続（WebSocket 等）への**委譲判定**（`matches` が `bool` を返す） | フレーミング・接続奪取後の読み書き（プラグイン側の責務） |
| `Middleware` | `on_request`: ヘッド受理後・ルーティング前 / `on_response`: レスポンス送出後 | ロギング・メトリクス等の**観測** | リクエスト・レスポンスの変更（`head` は不変参照のみ） |

登録は `Server` の builder メソッドで行い、いずれも複数登録できる。

| trait | 登録メソッド | 複数登録時の評価 | 実例プラグイン |
|-------|-------------|-----------------|---------------|
| `RequestGate` | `Server::gate` | 登録順に評価し、最初の `Reject` を優先 | `plugin-hub-wiring`（`TenantGate`） |
| `UpgradeHandler` | `Server::upgrade_handler` | 登録順に `matches` を評価 | `plugin-websocket` |
| `Middleware` | `Server::middleware` | 登録順に `on_request` / `on_response` を呼ぶ | `plugin-tracing` |

### 同期契約（3 trait 共通）

3 trait はいずれも**同期 API** である。`async fn` を trait に持ち込むと
`Box<dyn Middleware>` 等の trait object としてコアループが拡張点を保持する構成
（dyn 互換性）が壊れるためである。既定ハンドラ（`Handler::handle`）のみが
async 契約であり、この非対称は意図的な設計である（`crates/core/src/server.rs` の
`Handler` doc・[`docs/design/async-handler.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/async-handler.md) を参照）。

## `RequestGate` を自作する

`check` はリクエストヘッドを検査し、`GateOutcome::Allow`（続行）または
`GateOutcome::Reject { status, body }`（早期拒否）を返す。

```rust,ignore
use fandhe_backend_core::{GateOutcome, RequestGate};
use fandhe_backend_http::request::RequestHead;

/// `X-Api-Key` ヘッダの有無だけを見る例（フェイルクローズ）。
struct ApiKeyGate;

impl RequestGate for ApiKeyGate {
    fn name(&self) -> &'static str {
        "api-key-gate"
    }

    fn check(&self, head: &RequestHead) -> GateOutcome {
        match head.header("x-api-key") {
            Some(_) => GateOutcome::Allow,
            // 判定不能・情報欠落時は必ず Reject（フェイルクローズ）
            None => GateOutcome::Reject {
                status: 401,
                body: Vec::new(),
            },
        }
    }
}

let server = Server::new().handler(router).gate(ApiKeyGate);
```

守るべき契約は次のとおり。

- **フェイルクローズ**: 判定に必要な情報が欠落・不正な場合、あるいは判定不能な
  場合は必ず `Reject` を返し、疑わしきは通過させない（[`.claude/rules/security.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/security.md)
  の認可既定拒否の方針）
- `Reject` の `status` は数値（`u16`）のみを運ぶ。ステータス行の組み立て
  （reason phrase の付与等）はコア側の責務であり、任意文字列をステータス行へ
  書き出せない設計によってレスポンス分割・ヘッダインジェクションを型レベルで
  排除している
- 拒否レスポンス送出後も、登録済み `Middleware` の `on_response` は呼ばれる
  （観測の一貫性）

プロダクション水準の実例は `crates/plugin-hub-wiring` の `TenantGate`
（JWT 検証・テナント境界強制を `RequestGate` だけで実現）を参照する。

## `UpgradeHandler` の役割

`UpgradeHandler` がコアに公開するのは**委譲判定のみ**である。`matches` が
`true` を返すと、コアは当該接続の以降の処理を Upgrade 型プラグイン
（`plugin-websocket` 等）へ委譲する。ハンドシェイク検証・フレーミング・
アップグレード後の読み書きは trait の責務外であり、プラグイン側に閉じる。

```rust,ignore
use fandhe_backend_core::UpgradeHandler;
use fandhe_backend_http::request::RequestHead;

struct WebSocketUpgrade;

impl UpgradeHandler for WebSocketUpgrade {
    fn name(&self) -> &'static str {
        "websocket-upgrade"
    }

    fn matches(&self, head: &RequestHead) -> bool {
        head.header("upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
    }
}
```

自作時の注意は次のとおり。

- 実例である `websocket` feature では、利用者は `UpgradeHandler` を直接書かず
  `Server::websocket(config)` を呼ぶ。コアが内部でアダプタを登録し、委譲成立後の
  処理は `fandhe-backend-plugin-websocket` が担う
- `matches` が `true` を返したのに委譲先の Upgrade 型プラグインが存在しない場合
  （feature 無効・未登録）、コアは黙って落とさず **501 を返して接続を閉じる**。
  自作の `UpgradeHandler` を単独で登録しても長時間接続処理は成立しない点に注意する
- 委譲が成立した接続では `Middleware::on_response` は呼ばれない（委譲時は
  呼ばない契約）。`Middleware` 実装側は「`on_request` が必ず `on_response` を
  伴う」と仮定してはならない

## `Middleware` の非同期 I/O 規約

`Middleware` は同期 API だが、実装内で**同期ブロッキング I/O を行ってはならない**。
`on_request` / `on_response` はコアのリクエストループから直接呼ばれるため、
ここでのブロッキングはスループットに直結する（実測で最大 25% の劣化を確認済み。
`AGENTS.md` の「規約: ミドルウェア非同期 I/O 必須化」を参照）。

ロギング等で I/O が必要な場合は、次のパターンに従う。

- **チャネル送信パターン**: `on_request` / `on_response` では非同期チャネル
  （`tokio::sync::mpsc` 等）への送信・アトミック操作等の非ブロッキング操作に
  留め、実際のファイル・ネットワーク I/O は別タスクで行う
- カウンタ等の軽量な状態は `AtomicUsize` 等の内部可変性で持ち、`&self` の
  不変参照のみで完結させる（ロック保持も避ける）
- `name()` が返す識別名・ログ出力にリクエスト内容（トークン・PII）を含めない
  （[`.claude/rules/security.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/security.md)）

プロダクション実装は `crates/plugin-tracing` を参照する。`tracing-appender` の
non-blocking writer（非同期・バッファ済み I/O）へ記録を委ね、`on_response` の
1 点に記録を集約している（`Middleware` trait には request/response を跨いで
per-request 状態を運ぶ経路がないため）。

## セキュリティ・制約

- 3 trait とも `Send + Sync` 境界が必須である。拡張点の実装は複数ワーカー
  スレッドから共有参照される（この境界を欠くとビルドが通らない）
- `Middleware` は `head` を変更してはならない契約だが、コアはこれを型では
  強制しない。実装者が守る規約として doc に明記されている
- `GateOutcome` は許可/拒否の判定結果のみを運び、JWT クレーム等のプラグイン
  固有データをコアへ持ち込まない（依存方向は常に「プラグイン → コア」の一方向）
- 拡張点の評価順序（`RequestGate` → `UpgradeHandler` → パスインターセプト型
  プラグイン → 既定 `Handler`）は固定であり、利用者側で変更できない

## 関連ドキュメント

- 段階的なチュートリアル（`Middleware` の実装から feature 有効化まで）:
  [`tutorial.md`](./tutorial.md)
- feature 構成別の実行可能サンプル: [`feature-samples.md`](./feature-samples.md)
- プラグイン境界パターンの設計判断:
  [`docs/design/plugin-boundary.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/plugin-boundary.md)
- 既定 `Handler` の async 化（3 拡張点を同期に据え置く判断）:
  [`docs/design/async-handler.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/async-handler.md)
