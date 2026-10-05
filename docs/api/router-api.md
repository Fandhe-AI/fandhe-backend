# ルーティング API リファレンス（`fandhe-backend-routes`）

## 1. 目的と位置づけ

- 本ページは `fandhe-backend-routes` クレートが提供するルーティング API（`Router` と関連型）の契約を一覧化するリファレンスである
- 一次情報源は rustdoc（`crates/routes/src/lib.rs` / `crates/routes/src/pattern.rs` の doc comment・doc test。`Router::merge` はモジュール doc「ルータ合成」節、変更履歴は `CHANGELOG.md` の `[0.4.2]`）であり、本ページは全体像の把握と横断的な契約・セキュリティ観点の整理を担う
- `Router` はコアの既定ハンドラとして動作する。依存方向は `server → routes → http::*` の一方向であり、本クレートはソケット I/O・接続ライフサイクルを扱わない（それらは `crates/core` の責務。[サーバ API](./server-api.md) 参照）
- リクエスト・レスポンスのプリミティブ（`RequestHead` / `Response`）は `fandhe-backend-http` が提供する（[HTTP API](./http-api.md) 参照）

## 2. 公開 API 一覧

### 2.1 `Router` メソッド

| メソッド | シグネチャ（要約） | 戻り値 | 役割 |
|---------|-------------------|--------|------|
| `Router::new` | `() -> Self` | `Router` | 空のルータを作る（ルート未登録時は全リクエスト 404） |
| `Router::route` | `(method, path, handler: Fn(&RequestHead, &[u8]) -> Response)` | `Self` | 静的ルート（method + path 完全一致）の同期ハンドラ登録 |
| `Router::route_async` | `(method, path, handler: Fn(&RequestHead, &[u8]) -> Fut)` | `Self` | 静的ルートの async ハンドラ登録（`Fut: Future<Output = Response> + Send + 'static`） |
| `Router::route_param` | `(method, pattern, handler: Fn(&RequestHead, &PathParams<'_>, &[u8]) -> Response)` | `Result<Self, RoutePatternError>` | `{name}` / 末尾 `{*name}` を含むパターンルートの同期ハンドラ登録 |
| `Router::route_param_async` | `(method, pattern, handler: Fn(&RequestHead, &PathParams<'_>, &[u8]) -> Fut)` | `Result<Self, RoutePatternError>` | パターンルートの async ハンドラ登録 |
| `Router::options_fallback` | `(handler: Fn(&RequestHead, &AllowedMethods, &[u8]) -> Response)` | `Self` | OPTIONS プリフライトの opt-in フォールバック登録 |
| `Router::fallback` | `(handler: Fn(&RequestHead, &[u8]) -> Response)` | `Self` | 未マッチ共通処理の登録（ポリシーは既定 `FallbackPolicy::NotFoundOnly`） |
| `Router::fallback_with` | `(policy: FallbackPolicy, handler)` | `Self` | ポリシー明示版の fallback 登録 |
| `Router::merge` | `(self, other: Router)` | `Result<Self, RouterMergeError>` | 独立に組み立てた 2 つの `Router` を 1 つへ合成する（重複登録はフェイルクローズでエラー。`other` は所有権ごとムーブされる） |
| `Router::dispatch` | `(&self, head: &RequestHead, body: &[u8])` | `HandlerFuture` | ルート解決とハンドラ委譲（解決は同期・ハンドラ実行のみ非同期） |

### 2.2 関連型

| 型 | 種別 | 役割 |
|----|------|------|
| `HandlerFuture` | `type` = `Pin<Box<dyn Future<Output = Response> + Send>>` | ハンドラ実行の戻り値となる boxed future。常に `'static`（借用を持ち越さない契約） |
| `RouteHandler` | `type`（boxed `Fn`） | 静的ルートの内部ハンドラ型（`&RequestHead, &[u8]` → `HandlerFuture`） |
| `ParamRouteHandler` | `type`（boxed `Fn`） | パターンルートの内部ハンドラ型（`PathParams` を追加で受け取る） |
| `OptionsFallbackHandler` | `type`（boxed `Fn`） | OPTIONS フォールバックのハンドラ型（`AllowedMethods` を受け取る同期契約） |
| `FallbackPolicy` | `enum`（`NotFoundOnly` / `IncludeMethodNotAllowed`） | fallback が 405（メソッド不一致）も引き受けるかの選択。`Default` は `NotFoundOnly` |
| `PathParams<'a>` | `struct`（`get` / `iter` / `len` / `is_empty`） | `{name}` / `{*name}` の束縛値への読み取りアクセス |
| `RoutePatternError` | `enum` | パターン登録時エラー（`NoParamSegment` / `WildcardNotLast` 等） |
| `RouterMergeError` | `enum`（`#[non_exhaustive]`） | `Router::merge` が検出する合成時の衝突（`DuplicateRoute` / `DuplicateParamRoute` / `ConflictingFallback` / `ConflictingOptionsFallback`） |
| `ParamRoute` / `Segment` | `struct` / `enum` | パターンルートの内部表現（`pattern` モジュールから再エクスポート） |

## 3. 契約・不変条件

### 3.1 パスパターン仕様

| パターン | 一致対象 | 備考 |
|---------|---------|------|
| 静的パス（`/health` 等） | method + path のバイト完全一致 | `HashMap` ルックアップ。同一 `(method, path)` の再登録は後勝ち（上書き） |
| `{name}` | 非空の 1 セグメント | `.` / `..` と一致するセグメント、`?` / `#` を含むセグメントには一致しない（404 側へフェイルクローズ） |
| `{*name}`（末尾のみ） | 残りパス全体（`/` を含む、**1 個以上**のセグメント） | 0 セグメントは不一致。吸収する全セグメントに `{name}` と同じ検証を個別適用（`/static/../etc/passwd` は 404） |

- パターンは先頭 `/` で始まり、各セグメントは「リテラル」「`{name}`」「最終セグメントに限り `{*name}`」のいずれか（`a{b}` のような混在セグメントは不可）
- `{name}` / `{*name}` を 1 つも含まないパターンは `RoutePatternError::NoParamSegment`（完全一致は `route` を使う責務分界）
- `{*name}` を最終セグメント以外に配置すると `RoutePatternError::WildcardNotLast`（登録時にフェイルクローズ）

マッチング例:

| 登録パターン | リクエストパス | 結果 |
|-------------|---------------|------|
| `/hello/{name}` | `/hello/alice` | 一致（`name = "alice"`） |
| `/hello/{name}` | `/hello/` | 不一致（`{name}` は非空セグメントのみ） |
| `/hello/{name}` | `/hello/a/b` | 不一致（`{name}` は 1 セグメントのみ） |
| `/static/{*path}` | `/static/css/app.css` | 一致（`path = "css/app.css"`） |
| `/static/{*path}` | `/static/` | 不一致（`{*name}` は 1 セグメント以上） |
| `/static/{*path}` | `/static/../etc/passwd` | 不一致（`..` セグメント拒否 → 404） |

### 3.2 `PathParams` の読み取り API

| メソッド | 戻り値 | 役割 |
|---------|--------|------|
| `get(name)` | `Option<&str>` | 束縛値の名前引き（未束縛は `None`） |
| `iter()` | `impl Iterator<Item = (&str, &str)>` | (名前, 値) の走査 |
| `len()` / `is_empty()` | `usize` / `bool` | 束縛数の確認 |

- 束縛値はリクエストの `path()` からの借用（`PathParams<'a>`）であり、`route_param_async` の `async move` へ持ち込む場合は同期部で `to_string()` 等により所有値へ変換する

### 3.3 `dispatch` の解決優先順位

1. 静的ルート（完全一致）。パラメータルートを追加してもこの経路の性能・挙動は不変（後方互換）
2. 静的ルートが miss した場合のみ、パラメータルートを**登録順**に線形走査し最初の一致へ委譲
3. いずれにも一致しない場合、`fallback` / `fallback_with` 登録済みならポリシーに従い委譲。未登録なら 404 / 405 + `Allow`

- `Router::merge` 後もこの優先順位がそのまま適用される（パラメータルートの登録順は self が先、other が後。本ページ 3.8 節参照）

### 3.4 同期登録 API と `HandlerFuture` の内部アダプタ関係

- 既定ハンドラ契約は boxed future（`HandlerFuture`）返却へ移行済みだが、`route` / `route_param` の同期登録 API は**非破壊のまま維持**される
- 同期ハンドラは内部アダプタで `Box::pin(std::future::ready(response))` に包まれる。借用（`head` / `body`）は同期部で消費され future へ持ち越されないため、`HandlerFuture` はライフタイムパラメータを持たない（常に `'static`）
- `route_async` / `route_param_async` の利用者は `Fut: 'static` 契約を負う。引数の借用を `async` ブロックへ持ち越せないため、必要な値は同期部で `clone` してから `async move` へ渡す（axum / warp と同系のトレードオフ）
- 型消去は std のみで行い `async-trait` 等の外部依存を追加しない。3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）は意図的に同期のまま据え置き（[拡張 API](./extension-api.md) 参照）
- `dispatch` 自体は同期関数であり、ルーティング解決（優先順位判定・404/405/`Allow` 集約・各フォールバック判定）はすべて同期で行われる。非同期なのはハンドラ本体の実行のみ

### 3.5 `FallbackPolicy` と 405 + `Allow` の挙動

| 状況 | fallback 未登録 | `NotFoundOnly`（既定） | `IncludeMethodNotAllowed` |
|------|----------------|------------------------|---------------------------|
| 未登録パス（404） | `404`（空 body） | fallback へ委譲 | fallback へ委譲 |
| パス一致・メソッド不一致（405） | `405` + `Allow` | `405` + `Allow`（委譲しない） | fallback へ委譲（`Allow` は付与されない） |

- `Allow` には対象パスに登録済みの全 method（静的 + 形状一致したパラメータルート）をソート済み・重複排除で列挙する
- 既定が `NotFoundOnly` なのは安全側の選択: `Allow` による登録 method 開示という既存挙動を維持し、405 の意味論を暗黙に変えない
- fallback ハンドラには `PathParams` は渡されない（未マッチのため束縛が存在しない）。また `OPTIONS *`（asterisk-form）等の非 origin-form も fallback に到達しうるため、ハンドラは「先頭 `/`」を前提にしてはならない

### 3.6 `options_fallback` と CORS プリフライト

- OPTIONS リクエストが静的・パラメータいずれのルートにも解決できず、かつ対象パスに 1 件以上のルートが登録されている場合のみ、従来の 405 + `Allow` の代わりに委譲される。ハンドラは対象パスの登録済み method 一覧（`AllowedMethods`、tchar 検証済み）を受け取る
- 明示登録された `route("OPTIONS", ...)` / `route_param("OPTIONS", ...)` は常にこのフォールバックより優先される（利用者定義のプリフライト処理を横取りしない）
- 対象パスが未登録なら発火せず 404 のまま（パス列挙攻撃表面を拡大しない）
- CORS プラグイン利用時は、`fandhe_backend_plugin_cors::preflight_response` をここへ直接配線するのが標準構成。具体的な配線例は [feature 構成別サンプル](../guide/feature-samples.md) の cors 節を参照
- `Router::fallback` の `IncludeMethodNotAllowed` より `options_fallback` が優先される（OPTIONS 専用の挙動を横取りしない）

### 3.7 クエリ文字列の分離

- パス照合は `RequestHead::path()`（`target` 中の最初の `?` より前）に対して行い、クエリ文字列はハンドラが `RequestHead::query()` で参照する
- 静的ルート照合・パラメータルート照合・405 の `Allow` 集約の 3 経路すべてが同一の `path()` を参照し、経路間でパース結果が食い違わない
- `route` の `path` 引数に `?` を含めて登録したルートは、リクエスト側が常に `path()` で分離されるため到達不能になる（登録時に `?` を含めないこと）

### 3.8 `merge` によるルータ合成

- 想定用途は、クレートをまたいで独立に組み立てた `Router` を 1 つにまとめ、`Server::handler` へ登録する構成である。`other` は所有権ごとムーブ消費され、`route_param` と同じ `Result` 返却のビルダー形式のため `.merge(a)?.merge(b)?` と `?` で連鎖できる
- 衝突は self（`merge` を呼ぶ側）と other の間でのみ判定する。同一 `Router` 内の既存意味論（`route` は後勝ち、`route_param` は登録順の先勝ち）は変えない

| 衝突種別 | 判定条件 | 返却バリアント |
|---------|---------|---------------|
| 静的ルート | `(method, path)` の完全一致（method の大文字小文字は区別） | `DuplicateRoute { method, path }` |
| パラメータルート | method 一致かつセグメント形状が等価（`Literal` は文字列一致、`{name}` 同士・`{*name}` 同士は名前を問わず種別一致、セグメント数も一致。`/a/{id}` と `/a/{name}` は衝突） | `DuplicateParamRoute { method, pattern }`（`pattern` は self 側の表記を `{name}` / `{*name}` 形式で復元したもの） |
| `fallback` / `fallback_with` | 両方に登録されている | `ConflictingFallback` |
| `options_fallback` | 両方に登録されている | `ConflictingOptionsFallback` |

- 全件を検査してから統合する 2 段階方式で、最初に見つかった衝突を `Err` で返す。`Err` の場合は self・other のどちらも破棄される
- 衝突としないもの: `/a/{x}` と `/a/{*rest}`（種別が異なる）、静的 `/a/b` とパラメータ `/a/{x}`、method が異なる同一パス。これらは既存の優先順位（静的 → パラメータ、登録順）で解決される
- 引き継ぎの意味論:
  - 静的ルートは path ごとに method を合併し、405 の `Allow` にも両方の method が集約される
  - パラメータルートは self が先、other が後（`Vec::extend`）
  - `fallback` / `options_fallback` は片方にのみ登録されていればそれを引き継ぐ（`FallbackPolicy` もそのまま）。どちらにもなければ `None` のままで既定の 404 / 405 + `Allow` を維持する
  - `fallback` は接頭辞に限定されず、合成後のルータ全体に適用される（サブルータに付けた fallback は他のサブルータの未マッチにも効くため、合成後の最上位ルータで登録することを推奨）

```rust
use fandhe_backend_routes::{Router, RouterMergeError};
use fandhe_backend_http::response::Response;

let todos = Router::new().route("GET", "/todos", |_h, _b| {
    Response::new(200, b"todos".to_vec())
});
let users = Router::new().route("GET", "/users", |_h, _b| {
    Response::new(200, b"users".to_vec())
});
let router = todos.merge(users).unwrap();

// 重複するルート登録はエラーになる
let a = Router::new().route("GET", "/x", |_h, _b| Response::empty(200));
let b = Router::new().route("GET", "/x", |_h, _b| Response::empty(200));
let err = match a.merge(b) {
    Ok(_) => panic!("expected merge to fail"),
    Err(e) => e,
};
assert!(matches!(
    err,
    RouterMergeError::DuplicateRoute { ref method, ref path }
        if method == "GET" && path == "/x"
));
```

## 4. セキュリティ観点

| 観点 | 契約 |
|------|------|
| フェイルクローズ | デフォルト許可の経路は存在しない。未登録は 404、メソッド不一致は 405。fallback 登録済みでも既定ポリシーは 405 を委譲しない安全側 |
| 正規化非実施 | % デコード・末尾スラッシュ正規化を一切行わず、パーサが渡したバイト列をそのまま比較する（正規化差異によるアクセス制御バイパス、OWASP A01 対策）。`%2e%2e` はリテラルとして通過するため、デコード後の再検証は利用側の責務 |
| パス走査対策 | `{name}` / `{*name}` は `.` / `..` セグメント、`?` / `#` を含むセグメントに一致しない（不一致 = 404 側へ倒す） |
| ヘッダインジェクション | `Allow` は `AllowedMethods` の構築時 tchar 検証により CRLF インジェクションを型レベルで排除。不正 token の登録 method は `Allow` から除外され、全滅時は `Allow` なし 405 にフォールバック |
| method の大文字小文字 | RFC 9110 に従い区別する（`get` は `GET` に一致しない）。独自の正規化を持ち込まない |
| DoS 耐性 | ルートは起動時登録のみ（実行時追加・削除 API なし）。`dispatch` は登録数に対して予測可能なコストで応答する |
| 合成時の黙示的上書き | `Router::merge` は重複ルート・両方への fallback / options_fallback 登録を `RouterMergeError` で検出し、意図しないハンドラ差し替えを防ぐ（OWASP A01）。エラー内の `method` / `path` / `pattern` は起動時に開発者が登録した値でありリクエスト由来ではない |
| 情報開示の非拡大 | `options_fallback` は未登録パスでは発火せず 404 のまま。既存の 405 + `Allow` で開示済みの情報以上を新規開示しない |

## 5. スコープ外・関連ドキュメント

- **スコープ外**: ソケット I/O・接続管理・graceful shutdown（`crates/core`。[サーバ API](./server-api.md)）、HTTP パース・`Response` 直列化・cookie / query / form パーサ（`fandhe-backend-http`。[HTTP API](./http-api.md)）、`OPTIONS *`（asterisk-form）の解決、% デコード後のパス再検証（利用側責務）
- 3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）: [拡張 API](./extension-api.md)、[拡張点ガイド](../guide/extension-points.md)
- プラグインの登録 API: [プラグイン設定 API](./plugin-config-api.md)、使い方は [feature 構成別サンプル](../guide/feature-samples.md)
- 入門・チュートリアル: [Getting Started](../guide/getting-started.md)、[チュートリアル](../guide/tutorial.md)
- async ハンドラ移行の設計判断: `docs/design/async-handler.md`（<https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/async-handler.md>）
- 実装本体: `crates/routes/src/lib.rs`・`crates/routes/src/pattern.rs`（doc test 付き rustdoc が一次情報源）
