# feature 構成別サンプルガイド

fandhe-backend は「最小コア + Cargo feature 駆動プラグイン」で構成されます。
本文書は feature ごとに、有効化方法・実行可能なサンプル・動作確認手順・
pay-for-what-you-use（[`.claude/rules/pay-for-what-you-use.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/pay-for-what-you-use.md)）の
検証方法を一覧します。実行できる example は `crates/core/examples/*` にある
既存のものを使い、本文書にコード全文は複製しません（[`README.md`](./README.md) の原則）。

`crates/core/examples/*` は最小 example ですが、独立したプロジェクトとして
`cargo run` できる standalone 版が
[`examples/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/examples/)
（`with-<feature>` 命名、1 サンプル = 1 機能）に、複数 feature を同時配線した
実運用形の雛形が
[`templates/app/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/templates/app/)
にあります。重複回避方針の詳細は
[`examples/README.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/examples/README.md)
を参照してください（本文書では再掲しません）。

> 掲載する example の多くは NFR（性能）計測専用として追加されたものです
> （doc comment に「計測専用」と明記されています）。production 配線の書き方
> そのものは各 example のコードと [`getting-started.md`](./getting-started.md) の
> `Server` builder 呼び出し例を参照してください。

## websocket（`fandhe-backend-plugin-websocket`）

RFC 6455 ハンドシェイク検証・101 応答を `UpgradeHandler` 拡張点経由で提供します。

```bash
cargo run --release --example ws_echo -p fandhe-backend-core --features websocket
curl -v http://127.0.0.1:3007/health   # 200 応答
```

`GET /ws`（既定パス）へ WebSocket クライアントで接続するとエコーセッションが
確立します。負荷試験用の派生 example として `ws_nfr6`（`current_thread` ランタイム、
baseline との RPS 比較専用）もあります。

## graphql（`fandhe-backend-plugin-graphql`）

`POST /graphql` をパスインターセプトし、`async-graphql` で実クエリを実行します。

```bash
cargo run --release --example graphql_nfr6 -p fandhe-backend-core --features graphql
curl -v http://127.0.0.1:3003/                                          # 200 応答（無関係パス）
curl -v -X POST http://127.0.0.1:3003/graphql -d '{"query":"{ hello }"}' # クエリ実行
```

`Server::graphql` にスキーマを登録した場合のみ `POST /graphql` を処理し、未登録時は
feature 有効でもフォールスルーします（`crates/plugin-graphql` の doc を参照）。

## webrtc-proxy（`fandhe-backend-plugin-webrtc-proxy`、MVP 推奨）

WebRTC シグナリングを別プロセスに切り出すプロキシ型プラグインです。単体で完結する
runnable example は本ガイド整備時点では未整備です（[8 章「スコープ外」](#スコープ外)
を参照）。有効化・登録手順は次の最小コード断片のとおりです。

```rust,ignore
let server = Server::new()
    .handler(router)
    .webrtc_proxy(fandhe_backend_plugin_webrtc_proxy::ProxyConfig::default());
```

詳細な設計方針（別プロセス切り出し型を選ぶ理由・攻撃表面の考え方）は
[`docs/design/webrtc-process-isolation.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/webrtc-process-isolation.md) を参照してください。

## webrtc（`fandhe-backend-plugin-webrtc`、in-process 型）

`webrtc-rs` に直接依存する in-process 型です。攻撃表面が大きいため、通常は
上記 `webrtc-proxy` を推奨します（[`CLAUDE.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/CLAUDE.md) Repository Structure 参照）。

```bash
cargo build --release --example webrtc_nfr6 -p fandhe-backend-core --features webrtc
```

`POST /rtc/offer` へのシグナリングを扱います。動作確認手順は
`crates/core/examples/webrtc_nfr6.rs` の doc comment を参照してください。

## tracing（`fandhe-backend-plugin-tracing`）

サンプリング付き可観測性を `Middleware` 拡張点経由で提供します。

```bash
cargo run --release --example tracing_nfr -p fandhe-backend-core --features tracing
curl -v http://127.0.0.1:3006/           # 200 応答（無関係パス）
curl -v http://127.0.0.1:3006/health     # 200 応答（計測対象パス）
```

決定的カウンタ方式のサンプリング + 既定で非同期・バッファ済み I/O
（`tracing-appender` の non-blocking writer）により、RPS への影響を抑えています
（[`docs/design/tracing-integration.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/tracing-integration.md) 参照）。

## openapi（`fandhe-backend-plugin-openapi`、`gen-cli` feature）

OpenAPI ドキュメントは `utoipa::path` 定義から `gen-openapi` CLI で生成し、
`crates/plugin-openapi/openapi.json` / `openapi.yaml`（仕様が明記する「json と
同等に yaml も提供」への対応）に静的埋め込みします。`Server::openapi()` を登録すると
`GET /openapi.json` と `GET /openapi.yaml` の両方が同一スキーマ源（`ApiDoc`）から
配信されます。

```bash
# openapi.json / openapi.yaml を再生成する
cargo run -p fandhe-backend-plugin-openapi --bin gen-openapi --features gen-cli

# CI と同じ 2 段階検証（--check → 全 feature ビルド。json/yaml 両方の鮮度を検証）
scripts/openapi-two-stage.sh
```

TypeScript 向け型定義（`ts/src/generated/schema.d.ts`）を連携させる場合は
[`docs/design/openapi-typescript-pipeline.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/openapi-typescript-pipeline.md) を参照してください。

## cors（`fandhe-backend-plugin-cors`）

CORS（Cross-Origin Resource Sharing）を「プリフライト」と「実リクエストへの
ヘッダ付与」の 2 点で配線するプラグインです（[`docs/design/plugin-boundary.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/plugin-boundary.md)
5.9 節「レスポンス後処理型パターン」参照）。

```bash
cargo run --example cors_demo -p fandhe-backend-core --features cors

# プリフライト（204 + Access-Control-Allow-* を確認）
curl -si -X OPTIONS localhost:3004/todos \
  -H 'Origin: https://app.example.com' \
  -H 'Access-Control-Request-Method: POST'

# 実リクエスト（許可オリジン、不許可オリジンでヘッダ有無を比較）
curl -si localhost:3004/todos -H 'Origin: https://app.example.com'
curl -si localhost:3004/todos -H 'Origin: https://evil.example'
```

配線は 2 点のみです（`crates/core/examples/cors_demo.rs` を参照）:

1. `Router::options_fallback(|head, allow, _body| preflight_response(head, allow, &config))`
   でプリフライトを CORS プラグインへ委譲する
2. `Server::new().handler(router).cors(config)` で実リクエスト応答への
   ヘッダ付与を有効化する（未登録なら feature が有効でも完全フォールスルー、
   opt-in）

`CorsConfig::builder()` は許可オリジンの完全一致リスト（既定）・明示 opt-in の
`allow_any_origin()`・`allow_credentials`・`allow_headers`・`max_age` 等を
提供します。`allow_any_origin()` と `allow_credentials(true)` の併用は
`build()` が `Err` を返します（フェイルクローズ、credentials 付き全開放の防止）。

独立プロジェクトとしてそのまま `cargo run` できる standalone 版は
[`examples/with-cors/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/examples/with-cors/)
にあります。

## compression（`fandhe-backend-plugin-compression`）

gzip でレスポンスを圧縮するプラグインです（[`docs/design/plugin-boundary.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/plugin-boundary.md)
5.10 節「レスポンス後処理型パターンの第 2 インスタンス」参照）。CORS と同じ
「レスポンス後処理型」シームで配線し、複数登録時は CORS → 圧縮の順に適用
されます。

```bash
cargo run --example compression_demo -p fandhe-backend-core --features compression

# 閾値以上の text/plain・Accept-Encoding: gzip → Content-Encoding: gzip
curl -si localhost:3008/large -H 'Accept-Encoding: gzip' | head -20

# Accept-Encoding なし → 無圧縮のまま
curl -si localhost:3008/large

# 閾値未満の応答（既定 1024 バイト未満）→ 無圧縮のまま
curl -si localhost:3008/small -H 'Accept-Encoding: gzip'
```

配線は 1 点のみです（`crates/core/examples/compression_demo.rs` を参照）:

`Server::new().handler(router).compression(config)` で登録すると、ステータス・
`Content-Type`・body サイズ・`Accept-Encoding` の判定基準を満たすレスポンスを
gzip 圧縮します（未登録なら feature が有効でも完全フォールスルー、opt-in）。

`CompressionConfig::builder()` は最小圧縮対象サイズ `min_size`（既定 1024
バイト）・圧縮対象 `Content-Type` リスト `compressible_types`
（既定 `text/*`・`application/json` 等）を提供します。秘密情報を含みやすい
レスポンスは BREACH 類似の情報漏洩リスクがあるため、対象 `Content-Type` から
除外することを推奨します（`crates/plugin-compression/src/lib.rs` の crate
doc を参照）。

`fandhe-backend-plugin-compression` へ直接依存しなくても、`compression`
feature を有効化した `fandhe-backend-core` から
`fandhe_backend_core::plugin_compression::CompressionConfig` として同じ型を
参照できます（次回の crates.io リリース以降に反映）。

## static（`fandhe-backend-plugin-static`）

SPA フロントエンド等の静的ファイルを配信するプラグインです
（[`docs/design/plugin-boundary.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/plugin-boundary.md)
5.11 節「パスインターセプト型の `spawn_blocking` ファイル I/O 変種」参照）。

```bash
cargo run --example static_demo -p fandhe-backend-core --features static

# index.html（mount そのまま）
curl -si localhost:3005/static

# 通常ファイル（Content-Type 推定 + X-Content-Type-Options: nosniff を確認）
curl -si localhost:3005/static/app.js

# パストラバーサル試行（404 を確認）
curl -si --path-as-is localhost:3005/static/../Cargo.toml
```

配線は `Server::new().static_files(config)` の 1 点のみです（未登録なら feature が
有効でも完全フォールスルー、opt-in）。`StaticFilesConfig::builder(mount, root)` は
`root` を構築時に `canonicalize` し、不在・非ディレクトリを `Err` で早期拒否します。

- パストラバーサル対策は二層防御（I/O 前の字句検証 + `canonicalize` 後の実パスが
  正規化済み root 配下であることの確認）で行い、シンボリックリンク経由の脱出も
  拒否します
- 字句検証では先頭が `.` のセグメント（ドットファイル・ドットディレクトリ）も
  一律拒否します。`root` 配下に `.env`・`.git/config` 等の機密ファイルが誤って
  置かれていても配信されません
- ファイル未検出・検証失敗・サイズ超過（`max_file_bytes`、既定 8 MiB）は一律 404
  （存在オラクルを作らないフェイルクローズ）
- ディレクトリはインデックス（`index.html`）を試行し、それ以外はディレクトリ
  リスティングを実装しません
- ファイル I/O は `tokio::task::spawn_blocking` に閉じ、非同期ランタイム
  スレッドをブロックしません

`fandhe-backend-plugin-static` へ直接依存しなくても、`static` feature を
有効化した `fandhe-backend-core` から
`fandhe_backend_core::plugin_static::StaticFilesConfig` として同じ型を
参照できます（次回の crates.io リリース以降に反映）。

## hub-wiring（`fandhe-backend-plugin-hub-wiring`）

マルチテナント JWT 検証（RS256 / JWKS）・テナント境界強制を `RequestGate` 拡張点
（`TenantGate`）だけで実現するプラグインです。

```bash
cargo run --release -p fandhe-backend-plugin-hub-wiring --example hub_service_demo
```

`GET /items`・`GET /items/{id}`・`POST /items` を持つダミー hub サービスが起動します。
配線コードの範囲は `crates/plugin-hub-wiring/examples/hub_service_demo.rs` の
`// --- wiring:begin ---` 〜 `// --- wiring:end ---` を参照してください。

## pay-for-what-you-use の検証

各 feature を無効化した状態で、当該プラグインの依存が依存グラフから完全に消えることを
確認できます。

```bash
# 既定（feature なし）構成で plugin-* 依存が一切出ないことを確認する
cargo tree -p fandhe-backend-core

# 個別 feature を有効化した場合のみ対応する依存が現れることを確認する
cargo tree -p fandhe-backend-core --features websocket
```

## スコープ外

- `webrtc-proxy` feature 単体で完結する runnable example の新設は本ガイド整備の
  対象外です。現時点ではコード断片 + 設計ドキュメント参照で代替しています。
