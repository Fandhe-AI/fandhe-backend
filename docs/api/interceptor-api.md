# Interceptor 契約リファレンス

## 1. 目的と位置づけ

本書は `fandhe-backend-core` の `interceptor` モジュールが公開する 4 種目の拡張点
`Interceptor` の全体像・契約・呼び出しタイミングを俯瞰する読み物である。個々の
シグネチャ・doc test を含む一次情報源は rustdoc（`crates/core/src/interceptor.rs`
の doc comment）であり、本書と記述が食い違う場合は rustdoc を正とする。

- 設計原則「拡張点は 4 種 trait に集約」のうち、同期 3 trait（`Middleware` /
  `UpgradeHandler` / `RequestGate`、[extension-api.md](./extension-api.md) 参照）に
  続く 4 種目。既存 3 拡張点では「リダイレクトを返す」「確定済みレスポンスの body を
  差し替える」を表現できないため、`Handler`（[server-api.md](./server-api.md) 参照）
  と同じ「レスポンダ系シーム」ファミリーとして追加された
- `interceptor` モジュールは無条件公開（feature ゲート**不要**）。外部依存ゼロで、
  未登録時は実行時コストもゼロ（pay-for-what-you-use に反しない）
- 登録は `Server::interceptor`（[server-api.md](./server-api.md) 2.1 節参照）
- 自作手順・実装例は [../guide/extension-points.md](../guide/extension-points.md) を参照

## 2. 公開 API 一覧

### 2.1 `Interceptor` trait

| メソッド | シグネチャ概略 | 説明 |
|---------|---------------|------|
| `name` | `fn (&self) -> &'static str` | 診断・ログ表示用の静的識別名 |
| `intercept` | `fn (&self, &RequestHead, &[u8]) -> Option<Response>` | ルーティング・プラグイン評価前のフック。`Some(response)` で応答を確定させる |
| `map_response` | `fn (&self, &RequestHead, Response) -> Response` | 最終応答確定後の改変フック |

両フックとも既定実装は no-op（`intercept` は常に `None`、`map_response` は受け取った
`Response` をそのまま返す）であり、片方のみをオーバーライドして使うこともできる
（`Middleware` の 2 フック 1 trait と同じ流儀）。

### 2.2 登録 API

`Server::interceptor(impl Interceptor + 'static) -> Server` で複数登録できる
（builder パターン、詳細は [server-api.md](./server-api.md) 2.1 節）。

| フック | 複数登録時の評価順序 |
|-------|---------------------|
| `intercept` | 登録順に評価し、最初に `Some` を返した実装が勝つ（以降は呼ばれない） |
| `map_response` | 登録順に逐次適用する（各実装が前段の戻り値を受け取る） |

## 3. 評価順序

コアのリクエスト処理パイプライン（1 リクエストあたり）における評価位置。

| 順序 | ステップ | 備考 |
|------|---------|------|
| 1 | `Middleware::on_request` | 登録順に全件呼び出し |
| 2 | `RequestGate::check` | フェイルクローズ。拒否応答は `Interceptor` を一切通さない |
| 3 | `UpgradeHandler::matches` | マッチしたら接続ごとプラグインへ委譲 |
| 4 | **`Interceptor::intercept`** | 登録順、最初の `Some` が勝つ |
| 5 | パスインターセプト型プラグイン | `intercept` が `Some` を返した場合はスキップ |
| 6 | `Handler::handle` / `handle_streaming` | 4・5 いずれかで確定済みならスキップ |
| 7 | **`Interceptor::map_response`** | 登録順に逐次適用 |
| 8 | レスポンス後処理型プラグイン | 通常応答: `finalize_response`（CORS → 圧縮の順で逐次適用）。ストリーミング応答: `finalize_streaming_head`（CORS ヘッダ付与のみ）の次段で、明示 opt-in 時のみ `prepare_streaming_compression`（チャンク単位の gzip 圧縮、HTTP/1.1 chunked 経路限定）を適用 |
| 9 | レスポンス書き込み → `Middleware::on_response` | 登録順に全件呼び出し |

評価位置の設計判断:

- **`RequestGate` より後**: ゲートの既定拒否（フェイルクローズ）をユーザーコードで
  迂回できないようにする（A01 アクセス制御対策、`RequestGate` と同一の設計判断）
- **`UpgradeHandler` より後**: 確立済みの Upgrade 委譲・permit 引き継ぎ意味論に触れない
- **パスインターセプト型プラグインより前**: 利用者が登録済みプラグイン（例:
  `plugin-static`）の応答を `intercept` で先取りできる（末尾スラッシュ 301 正規化の
  ユースケースが成立する条件）
- **`map_response` は `finalize_response`（CORS → 圧縮）より前**: CORS ヘッダ付与・
  gzip 圧縮は改変後の最終 body に対して適用されるべきため

## 4. 契約・不変条件

1. **同期 API**: `Middleware` 等の同期 3 trait と同じく `async fn` を持たない
   （dyn 互換性の維持のため）
2. **`Send + Sync` 必須**: `Arc<Server>` 経由で複数コネクションタスクから共有参照される
3. **同期ブロッキング I/O 禁止**: 実測でスループットが最大 25% 劣化する。
   カスタム 404 ページ等、レスポンス body に静的コンテンツを使う場合は起動時に
   メモリへプリロードしておく
4. **`name()` に機密を含めない**: リクエスト内容（トークン・PII）を含めてはならない
   （`Middleware::name` と同一契約）
5. **`map_response` を通さない応答（fail-closed）**: `finalize_response` と同一の
   設計判断として、以下の応答は `Interceptor` の対象外
   - `RequestGate` 拒否応答
   - パースエラー応答（400 等、コネクション処理中に確定するもの）
   - Upgrade 委譲失敗時の 501 応答・shutdown 中の 503 応答

## 5. ストリーミング応答への適用

`Handler::handle_streaming` が返す応答は `Response` 型を前提とする通常経路（3 節の
7）を通らないが、`map_response` 自体はコアの `write_streaming_response` が
ヘッド確定時（HTTP/1.0・HTTP/1.1 共通、1 回のみ）に登録順で適用する。ステータス・
`Content-Type`・追加ヘッダの改変はここで反映されるが、ストリーミング応答の実体
（body）は producer タスクが `BodyWriter` 経由で逐次供給し chunked framing はコアが
直接組み立てるため、**`map_response` が返した `Response` の body は反映されず
破棄される**契約である。

`map_response` 適用後のステータスは以降のすべての判定（1xx/204/304 の body 送出
スキップ含む）に一貫して使用される。レスポンス後処理型プラグインはストリーミング
応答では `finalize_streaming_head`（CORS ヘッダ付与のみ）が `map_response` 適用
直後のヘッドへ適用される。body 全体を前提とする一括圧縮（`finalize_response` 経由）
は body 全体のバッファリングが必要でストリーミング設計（バックプレッシャ・打ち切り
クローズ契約）と両立できないため引き続き対象外だが、`CompressionConfigBuilder::
compress_streaming(true)` の明示 opt-in 時のみ、`finalize_streaming_head` の次段で
チャンク単位のストリーミング gzip 圧縮（`prepare_streaming_compression`）が
HTTP/1.1 chunked 経路限定で適用される（既定 OFF、HTTP/1.0 経路は常に identity）。

## 6. セキュリティ観点

- **ゲート最優先・バイパス不可（A01）**: `RequestGate` は `Interceptor` を含む
  すべての応答生成経路より先に評価される。`intercept`/`map_response` を使っても
  ゲートの拒否判定を迂回できない
- **インジェクション耐性（A03）**: `map_response`/`intercept` による改変は
  `Response` の検証済み構築 API（`with_header` の CR/LF/NUL・予約ヘッダ拒否、
  `redirect` の Location 検証）を経由する契約であり、生ヘッダ文字列の直接組み立てを
  必要としない。レスポンス分割・ヘッダインジェクションの新たな経路を作らない
- **fail-closed 除外の維持（A04）**: `RequestGate` 拒否応答・パースエラー応答・
  Upgrade 委譲失敗応答は `Interceptor` の対象外のまま維持される。改変ロジックを
  fail-closed 経路へ差し込む余地を作らない
- **DoS 対策**: 同期契約・ブロッキング I/O 禁止（起動時プリロードパターン）により、
  `Interceptor` 実装がリクエスト処理のスループットを直接劣化させる経路を避ける

## 7. スコープ外・関連ドキュメント

- 自作手順・実装例: [../guide/extension-points.md](../guide/extension-points.md)
- 同期 3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）の契約:
  [extension-api.md](./extension-api.md)
- サーバへの登録 API・リクエスト処理全体の設定: [server-api.md](./server-api.md)
- 設計判断の記録: `docs/design/interceptor-extension-point.md`
  （GitHub: `https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/interceptor-extension-point.md`）
- 最小配線サンプル: `examples/with-interceptor`
  （GitHub: `https://github.com/Fandhe-AI/fandhe-backend/tree/main/examples/with-interceptor`）
- 一次情報源（rustdoc）: `crates/core/src/interceptor.rs`
  （GitHub: `https://github.com/Fandhe-AI/fandhe-backend/blob/main/crates/core/src/interceptor.rs`）
