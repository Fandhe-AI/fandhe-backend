# レスポンスストリーミングガイド

fandhe-backend は、レスポンス body を一括ではなく逐次送信する chunked
ストリーミング送信を opt-in で提供する（イシュー #319）。SSE
（`text/event-stream`）・大きなファイルの逐次生成・進捗つき長時間処理の応答などに
使う。入口は既定ハンドラ trait の opt-in 既定メソッド
`Handler::handle_streaming`（`crates/core/src/server.rs`）と、
`fandhe_backend_core::streaming` モジュールの `StreamingResponse` / `BodyWriter`
（`crates/core/src/streaming.rs`）である。

既存の `Handler::handle` のみを実装した型は本機能を一切意識せずに動作し続ける
（`handle_streaming` の既定実装は常に `None` を返し、従来どおり `Content-Length`
一括応答になる。後方互換）。

## 基本形: `handle_streaming` + producer タスク

`handle_streaming` が `Some(StreamingResponse)` を返すと、コアの書き出しループは
通常の一括応答経路の代わりに chunked framing で逐次送信する。典型パターンは
`StreamingResponse::channel` で得た `BodyWriter` を `tokio::spawn` した producer
タスクへ move し、producer がデータ生成の都合に合わせて `send` / `finish` を呼ぶ
構成である（完全な実行例は `crates/core/src/server.rs` の
`Handler::handle_streaming` doc test を正とする）。

```rust,ignore
use fandhe_backend_core::server::Handler;
use fandhe_backend_core::streaming::StreamingResponse;
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;

struct StreamingHandler;

impl Handler for StreamingHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        Box::pin(async { Response::empty(404) })
    }

    fn handle_streaming(&self, _head: &RequestHead, _body: &[u8]) -> Option<StreamingResponse> {
        // status / Content-Type / チャネル容量（bounded mpsc）を指定して構築する
        let (response, writer) = StreamingResponse::channel(200, Some("text/plain"), 4);
        tokio::spawn(async move {
            writer.send(b"hello ".to_vec()).await.ok();
            writer.send(b"world".to_vec()).await.ok();
            writer.finish().await.ok();
        });
        Some(response)
    }
}
```

`Content-Type` が不要なら既定容量（8）の `StreamingResponse::new(status)` も使える。

## チャネル構築とバックプレッシャ

`StreamingResponse::channel(status, content_type, capacity)` の `capacity` は
bounded mpsc チャネルの容量である。

| 項目 | 挙動 |
|------|------|
| チャネル満杯時の `send` | 受信側（コアの書き出しループ）がソケットへ書き出して空きができるまで `.await` で待機する（バックプレッシャ） |
| サーバ側バッファ上限 | 高々「`capacity` × 1 チャンク分」に有界。producer がソケットの処理速度を追い越して無制限にメモリを積み上げることはない（DoS 対策） |
| `capacity = 0` | `1` に切り上げる（panic しないフェイルセーフ） |
| 既定容量（`new`） | 8（数チャンク分のパイプライン化を許しつつ上限を小さく保つ妥協値） |
| `BodyWriter` の clone | 可。`mpsc::Sender` のセマンティクスを継承し、複数タスクから送信できる |

## `send` / `finish` の契約

| 操作 | 契約 |
|------|------|
| `send(data)` | 1 チャンク分を送る。空 `Vec` も成功するが、ワイヤへは無出力（誤終端防止） |
| `finish()` | 正常終端。受信側は終端チャンク（`0\r\n\r\n`）を送出し、応答を完全なものとして扱う（keep-alive 継続も許される）。`self` を消費するため「`finish` 後の `send`」は型レベルで書けない |
| `finish` を呼ばずに drop | **打ち切り**。受信側は終端チャンクを送出せず接続をクローズする |
| 戻り値 `Err(StreamClosed)` | 受信側が既に終了（クライアント切断等）した後の送信。producer は以降の送信を止めてタスクを終了してよい |

`finish` 省略時に終端チャンクを送らないのは fail-closed の応答完全性維持である。
打ち切られた応答をクライアント・キャッシュに「完全な応答」と誤認させない
（RFC 9112 の length 整合性。`crates/core/src/streaming.rs` のモジュール doc を参照）。

### チャンク間隔の制約（30 秒以内）

producer からの次チャンク待ちには書き込みタイムアウト（30 秒、固定値）が適用され、
超過すると正常に稼働している producer でも接続が強制クローズされる
（スロープロデューサ対策）。SSE のハートビートや long-poll のようにイベント発生が
まばらな場合は、30 秒未満の間隔で `writer.send(Vec::new())` を呼んで待ち時間を
リセットするとよい（空チャンクはワイヤへ無出力のため、クライアントに余計な
バイトを見せずに内部キープアライブとして使える）。

## HTTP バージョン別の挙動

| クライアント | framing | 接続 |
|-------------|---------|------|
| HTTP/1.1 | `Transfer-Encoding: chunked` ヘッド + チャンク列 + 終端チャンク | `finish` で正常終端すれば keep-alive 継続可 |
| HTTP/1.0 | framing なしの生データを EOF（接続クローズ）で終端。常に `Connection: close` | 応答完了 = 接続クローズ（keep-alive は常に無効） |

HTTP/1.0 は `Transfer-Encoding: chunked` を理解しない前提のクライアントが残るため、
コアが自動でフォールバックする。ハンドラ側の実装を分ける必要はない。

また RFC 9112 §6.3 に従い、1xx・204・304 を `handle_streaming` から返した場合は
body 送出ループへ入らず、ヘッド送出のみで応答を完了させる。

## 通常 `handle` との使い分け

| 観点 | `handle`（一括応答） | `handle_streaming` |
|------|---------------------|--------------------|
| body | 応答全体を組み立ててから `Content-Length` 付きで送信 | チャンク単位で逐次送信（全体サイズ不要） |
| 契約 | async（`HandlerFuture` を返す） | 同期のまま（チャネルを組み立てて即座に返り、非同期 I/O は producer タスクが担う） |
| 適する用途 | 通常の API 応答・小さな body | SSE・大きな逐次生成 body・長時間処理の進捗通知 |
| レスポンス後処理型プラグイン（CORS ヘッダ付与・gzip 圧縮） | 適用される | **適用されない**（`Response` 型前提のシームのため） |

評価順序にも注意する。パスインターセプト型プラグイン（graphql / openapi /
static 等）が処理を完結させなかった場合にのみ `handle_streaming` が確認され、
`Some` ならこのリクエストに対して `handle` は呼ばれない。`None` なら従来どおり
`handle` の一括応答経路に入る。

## セキュリティ・制約

- bounded チャネルのバックプレッシャにより、producer 起因のメモリ積み上げは
  「`capacity` × 1 チャンク分」に有界（[`.claude/rules/security.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/.claude/rules/security.md)
  のリソース枯渇 DoS 対策）
- チャンク待ち・実書き込みの双方に 30 秒のタイムアウトと接続生存期間上限
  （`Server::max_connection_lifetime`）の短い方が適用され、超過時は接続を
  強制クローズする（フェイルクローズ）
- タイムアウト・書き込みエラー・producer 打ち切りの場合、`Middleware::on_response`
  は呼ばれない（「完走した応答のみ観測する」契約）
- CORS ヘッダ付与・gzip 圧縮（レスポンス後処理型プラグイン）はストリーミング
  応答には適用されない。必要なヘッダはハンドラ・構成側で別途手当てする

## 関連ドキュメント

- 拡張点の全体像（`Handler` と 3 拡張点の関係）: [`extension-points.md`](./extension-points.md)
- graceful shutdown との組み合わせ（in-flight 接続の完了待ち）:
  [`graceful-shutdown.md`](./graceful-shutdown.md)
- API・契約の正とする doc comment: `crates/core/src/streaming.rs`・
  `crates/core/src/server.rs`（`Handler::handle_streaming`）
- sans-IO chunked エンコーダ: `crates/http/src/chunked.rs`（`encode_chunk` /
  `encode_terminator`）
