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
| `RequestGate` | ルーティング・アップグレード判定より**前** | `GateOutcome::Allow` / `Reject` による早期拒否（認証・認可・同意ゲート等、拒否応答は `Retry-After` 等ヘッダ付きも可） | 判定根拠データ（JWT クレーム等）をコアへ持ち出すこと |
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
`GateOutcome::Reject { response }`（早期拒否、`response` は検証済み
[`Response`]）を返す。`Retry-After` 等ヘッダ付き拒否応答を返せるよう、
検証済み `Response` を直接運ぶ設計になっている。

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
            // 判定不能・情報欠落時は必ず Reject（フェイルクローズ）。
            // ヘッダ不要な最小構成は `GateOutcome::reject` ヘルパで足りる。
            None => GateOutcome::reject(401, Vec::new()),
        }
    }
}

let server = Server::new().handler(router).gate(ApiKeyGate);
```

`Retry-After` 等のヘッダを付与したい場合は、`Response` の検証済み構築 API
（`with_header` / `with_content_type`）で組み立ててから `Reject` へ渡す。

```rust,ignore
use fandhe_backend_core::{GateOutcome, RequestGate};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;

struct RateLimitGate;

impl RequestGate for RateLimitGate {
    fn name(&self) -> &'static str {
        "rate-limit-gate"
    }

    fn check(&self, _head: &RequestHead) -> GateOutcome {
        let response = Response::new(429, b"{\"error\":\"rate limited\"}".to_vec())
            .with_content_type("application/json")
            .with_header("Retry-After", "30")
            .expect("リテラル値は構築時検証を通る");
        GateOutcome::Reject { response }
    }
}
```

守るべき契約は次のとおり。

- **フェイルクローズ**: 判定に必要な情報が欠落・不正な場合、あるいは判定不能な
  場合は必ず `Reject` を返し、疑わしきは通過させない（[`.claude/rules/security.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/security.md)
  の認可既定拒否の方針）
- `Reject` が運ぶ `response` は `Response` の構築時検証（CR/LF/NUL 拒否・
  `Content-Length`/`Connection`/`Transfer-Encoding` の予約名拒否）を経た値の
  みで、任意文字列を無検証でヘッダ・ステータス行へ書き出す経路は存在しない
  （レスポンス分割・ヘッダインジェクション対策）
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

## `Interceptor`（ユーザー向けインターセプト・レスポンス改変）

上記 3 拡張点はいずれも「リクエストを弾く」「観測する」「長時間接続へ委譲する」だけで、
**リダイレクトを返す**・**確定済みレスポンスの body を差し替える**ことができない。
この 2 用途向けに `Interceptor` trait（`crates/core/src/interceptor.rs`）を追加した。
3 拡張点の対象外だが、`Handler` と同じ「レスポンダ系シーム」として feature ゲートなしで
常時利用できる（詳細な設計判断は
[`docs/design/interceptor-extension-point.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/interceptor-extension-point.md)）。

```rust,ignore
use fandhe_backend_core::interceptor::Interceptor;
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;

/// `/old` を `/new` へ 301 で正規化する例。
struct RedirectOld;

impl Interceptor for RedirectOld {
    fn name(&self) -> &'static str {
        "redirect-old"
    }

    fn intercept(&self, head: &RequestHead, _body: &[u8]) -> Option<Response> {
        if head.path() == "/old" {
            Response::redirect(301, "/new").ok()
        } else {
            None
        }
    }
}
```

登録は `Server::interceptor(...)`（複数登録可、`RequestGate`/`Middleware` と同じ
builder パターン）。

| フック | 呼ばれるタイミング | できること |
|-------|-------------------|-----------|
| `intercept` | `UpgradeHandler` 通過後・`plugin::try_intercept`（static 等）より前 | `Some(response)` で応答を確定させ、以降のプラグイン評価・`Handler` をスキップ |
| `map_response` | 最終応答（`intercept`/プラグイン/`Handler` いずれか）確定後・CORS/圧縮より前 | 確定済み `Response` を任意に書き換えて返す |

- 複数 `Interceptor` を登録した場合、`intercept` は登録順に評価し最初の `Some`
  が勝つ（以降は呼ばれない）。`map_response` は登録順に**逐次適用**する
  （各実装が前段の戻り値を受け取る）
- `intercept` が `plugin::try_intercept`（`static`/`graphql` 等の設定登録型プラグイン）
  より**前**に評価されるため、利用者は登録済みプラグインの応答をインターセプトで
  先取りできる（末尾スラッシュ 301 正規化のユースケース）
- `map_response` は CORS ヘッダ付与・gzip 圧縮より**前**に適用されるため、書き換え後の
  body に対して圧縮・ヘッダ付与が効く
- `RequestGate` 拒否応答・パースエラー応答・Upgrade 委譲失敗応答・ストリーミング応答
  （`Handler::handle_streaming`）には適用されない（fail-closed。既存の
  `finalize_response` と同一の除外方針）
- `intercept`/`map_response` とも `Middleware` と同じ同期契約（同期ブロッキング I/O
  禁止）。カスタム 404 ページ等の静的コンテンツは起動時にメモリへプリロードしておく

## セキュリティ・制約

- 3 trait とも `Send + Sync` 境界が必須である。拡張点の実装は複数ワーカー
  スレッドから共有参照される（この境界を欠くとビルドが通らない）
- `Middleware` は `head` を変更してはならない契約だが、コアはこれを型では
  強制しない。実装者が守る規約として doc に明記されている
- `GateOutcome` は許可/拒否の判定結果のみを運び、JWT クレーム等のプラグイン
  固有データをコアへ持ち込まない（依存方向は常に「プラグイン → コア」の一方向）
- 拡張点の評価順序（`RequestGate` → `UpgradeHandler` → `Interceptor::intercept` →
  パスインターセプト型プラグイン → 既定 `Handler` → `Interceptor::map_response` →
  レスポンス後処理型プラグイン）は固定であり、利用者側で変更できない

## 関連ドキュメント

- 段階的なチュートリアル（`Middleware` の実装から feature 有効化まで）:
  [`tutorial.md`](./tutorial.md)
- feature 構成別の実行可能サンプル: [`feature-samples.md`](./feature-samples.md)
- プラグイン境界パターンの設計判断:
  [`docs/design/plugin-boundary.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/plugin-boundary.md)
- `Interceptor`（3 拡張点で表現できないリダイレクト・レスポンス改変）の設計判断:
  [`docs/design/interceptor-extension-point.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/interceptor-extension-point.md)
- 既定 `Handler` の async 化（3 拡張点を同期に据え置く判断）:
  [`docs/design/async-handler.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/async-handler.md)
