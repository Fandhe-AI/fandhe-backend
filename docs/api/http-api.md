# HTTP API リファレンス（fandhe-backend-http）

## 1. 目的と位置づけ

- 一次情報源は rustdoc（`cargo doc -p fandhe-backend-http`）である。本書は `fandhe-backend-http` クレートの公開 API の全体像・モジュール間の契約・DoS 上限を 1 ページで俯瞰するための索引であり、個別 API の詳細仕様・doc test は rustdoc 側を正とする
- `fandhe-backend-http` は workspace 依存グラフ（`server → routes → http`）の末端に位置する最小 HTTP コアである。実行時依存は tokio の `io-util` のみ（feature `net` 有効時に tokio `net` が追加され `socket` モジュールが公開される）
- パーサ群（`request` / `chunked` / `query` / `form` / `cookie` / `percent` / `body`）はすべて sans-IO 純関数・状態機械として実装され、ソケット I/O を持たない。ルーティング層の使い方は [Router API](./router-api.md)、サーバ組み立ては [Server API](./server-api.md) を参照

## 2. モジュール別公開 API 一覧

### 2.1 `request` — リクエストヘッドの sans-IO パーサ

| API | 種別 | 概要 |
|-----|------|------|
| `parse_request_head(buf)` | fn | `&[u8]` からリクエストライン + ヘッダを解析する純関数。`Result<ParseOutcome, ParseError>` |
| `ParseOutcome` | enum | `Complete { head, consumed }`（ヘッダ終端までの消費バイト数付き）/ `Incomplete`（追い読み後に再試行） |
| `ParseError` | enum | `HeaderSectionTooLarge` / `TooManyHeaders` / `InvalidRequestLine` / `UnsupportedVersion` / `InvalidHeader` |
| `RequestHead` | struct | `method` / `target` / `version` は公開フィールド、ヘッダ列は非公開（アクセサ経由のみ） |
| `RequestHead::header(name)` | method | 大小文字無視で先頭一致の 1 件を返す。同名複数時は最初のみ |
| `RequestHead::headers()` | method | 全ヘッダを出現順に走査するイテレータ。重複検査は呼び出し元の責務 |
| `RequestHead::path()` | method | `target` の最初の `?` より前。無正規化・非デコードのまま返す |
| `RequestHead::query()` | method | 最初の `?` より後の生文字列。`?` なしは `None`、`/x?` は `Some("")` |
| `RequestHead::cookies()` | method | 全 `Cookie` ヘッダを結合し cookie-pair 列へ分解。累積 DoS 上限を適用 |
| `HttpVersion` | enum | `Http10` / `Http11` のみ受理。他バージョンは `UnsupportedVersion` |

### 2.2 `response` — レスポンス直列化

| API | 種別 | 概要 |
|-----|------|------|
| `Response::new(status, body)` / `empty(status)` | fn | `status: u16` + 生バイト列 body から構築 |
| `Response::with_content_type(&'static str)` | method | `Content-Type` 設定。`&'static str` 限定の型レベル制約 |
| `Response::with_allow(AllowedMethods)` | method | 405 応答用 `Allow` ヘッダ。構築時検証済み専用型のみ受理 |
| `Response::with_header(name, value)` | method | 検証付き任意ヘッダ追加（追記セマンティクス）。`Result<Self, HeaderError>` |
| `Response::header(name)` | method | 設定済みヘッダの読み取り（大小文字無視、後処理型プラグイン用） |
| `Response::with_set_cookie(SetCookie)` | method | 検証済み `Set-Cookie` を infallible に追加。複数回呼び出しで複数行 |
| `Response::redirect(status, location)` | fn | 301/302/303/307/308 限定の 3xx 構築。`Result<Self, RedirectError>` |
| `Response::serialize(keep_alive)` | method | `Content-Length` 付き一括送信のワイヤ直列化 |
| `Response::serialize_chunked_head(keep_alive)` | method | chunked ストリーミングのヘッド部のみ（`Transfer-Encoding: chunked`、body 非送出） |
| `Response::serialize_streaming_head_http10()` | method | HTTP/1.0 向け EOF 終端ストリーミングヘッド（常に `Connection: close`） |
| `Response::is_bodyless_status(status)` | fn | RFC 9112 §6.3 の body を持ち得ない応答（1xx・204・304）判定 |
| `AllowedMethods::from_methods(...)` | fn | tchar 検証 + ソート + 重複排除。1 件でも不正なら `None` |
| `HeaderError` | enum | `InvalidName` / `InvalidValue` / `ReservedName` |
| `RedirectError` | enum | `UnsupportedStatus` / `EmptyLocation` / `InvalidLocation(HeaderError)` |

### 2.3 `cookie` — 読み取りパーサ + 構築時検証済み書き込み型

| API | 種別 | 概要 |
|-----|------|------|
| `parse_cookie_header(value)` | fn | 単一 `Cookie` ヘッダ値を RFC 6265 cookie-pair 列へ分解（ゼロコピー借用） |
| `SetCookie::new(name, value)` | fn | cookie-name（tchar）/ cookie-value（cookie-octet）を構築時検証 |
| `SetCookie::http_only()` / `secure()` / `same_site(SameSite)` / `path(p)` / `max_age(secs)` | builder | 属性付与。`path` のみ fallible（path-value 検証 + `/` 開始必須）。`SameSite::None` 指定時は `Secure` を自動付与 |
| `SetCookie::to_header_value()` | method | `Path` → `Max-Age` → `SameSite` → `Secure` → `HttpOnly` の固定順で直列化 |
| `SameSite` | enum | `Strict` / `Lax` / `None`（`None` は属性値であり未設定の意味ではない） |
| `CookieError` | enum | `CookieStringTooLarge` / `TooManyCookies` / `InvalidCookiePair` / `InvalidName` / `InvalidValue` / `InvalidPath` |

`Domain` / `Expires` / `Partitioned` 属性は最小サブセット方針によりスコープ外である。

### 2.4 `query` — クエリ文字列 key-value 分解

| API | 種別 | 概要 |
|-----|------|------|
| `parse_query(query)` | fn | `RequestHead::query()` の生文字列を `&`/`=` で分解。分解前に全長・組数の 2 上限を検査 |
| `QueryPairs<'a>` | struct | `Iterator<Item = (&str, &str)>` のゼロコピーイテレータ。追加割り当てなし |
| `QueryError` | enum | `QueryTooLong` / `TooManyPairs` |

分解セマンティクス: 重複キーは出現順にすべて返す。`a` → `("a", "")`、`=v` → `("", "v")`、空セグメント（`&&`）はスキップ、2 個目以降の `=` は値の一部。percent-decode・`+` → 空白変換は行わない。

### 2.5 `form` — `application/x-www-form-urlencoded` ボディパーサ

| API | 種別 | 概要 |
|-----|------|------|
| `parse_form(body)` | fn | 生 body `&[u8]` を `Vec<(String, String)>` へ。UTF-8 検証 → `parse_query` 委譲 → `+` → 空白置換 → percent-decode の順で適用 |
| `is_form_content_type(ct)` | fn | media-type 部分（`;` より前）を OWS trim + 大小文字無視で厳密一致判定。前置一致は `false` |
| `FormError` | enum | `BodyTooLong` / `TooManyPairs` / `InvalidUtf8Body` / `Decode(PercentDecodeError)` |

`+` → 空白置換を percent-decode より**先**に適用する順序は不変条件である（逆順だと `%2B` が誤って空白化される）。呼び出し元は `is_form_content_type` で Content-Type を確認した後にのみ `parse_form` を呼ぶ契約。

### 2.6 `percent` — percent-decode（opt-in ヘルパ）

| API | 種別 | 概要 |
|-----|------|------|
| `decode_bytes(input)` | fn | `%XX` → 1 バイト復元。UTF-8 検証なし（`%FF` 等のバイナリ値も `Ok`）。`+` は変換しない |
| `decode_str(input)` | fn | `decode_bytes` + `String::from_utf8` 厳密検証（lossy 変換なし） |
| `PercentDecodeError` | enum | `TruncatedEscape { at }` / `InvalidHexDigit { at }` / `InvalidUtf8`。不正シーケンスは U+FFFD へ黙殺せず必ず `Err` |

### 2.7 `error` — エラーレスポンス共通化

| API | 種別 | 概要 |
|-----|------|------|
| `IntoResponse` | trait | `Response` への変換契約。`Response` / `HttpError` / `Result<T, E>`（両辺が `IntoResponse`）に実装済み |
| `HttpError::new(status, &'static str)` | fn | ステータス + ユーザー提示メッセージの標準形エラー。`?` で伝播可能 |
| `error_response(status, &'static str)` | fn | JSON 標準形 `{"error":"..."}` の `Response` を構築。serde 非依存の手実装エスケープ（RFC 8259 準拠） |

`message` を `&'static str` に限定するのは情報漏えい対策である。ソースコード上に静的に書かれたリテラルしか渡せないため、実行時エラーの `Display` / `Debug` 出力（DB エラー詳細・ファイルパス・スタックトレース）がエラーボディへ流出する経路が型レベルで存在しない。

### 2.8 `body` — body フレーミングの意味決定

| API | 種別 | 概要 |
|-----|------|------|
| `body_length(head)` | fn | ヘッダ列から「body を何バイト読むべきか」を決定（上限は `MAX_BODY_BYTES`） |
| `body_length_with_limit(head, max)` | fn | 上限引数化版。`Server::max_body_bytes` 上書き時に使用。`max == 0` は body 付きリクエスト一律拒否 |
| `BodyLength` | enum | `None` / `Fixed(u64)` / `Chunked` |
| `BodyError` | enum | `TransferEncodingUnsupported` / `ContentLengthWithChunked` / `DuplicateContentLength` / `InvalidContentLength` / `BodyTooLarge` |

`Transfer-Encoding` は HTTP/1.1 かつ単独の `chunked` のみ受理する。`Content-Length` との共存・重複 `Content-Length`・`chunked, chunked` 等の多重指定は一律拒否（RFC 9112 §6.3 のリクエストスマグリング対策）。

### 2.9 `chunked` — chunked transfer-coding のデコーダ / エンコーダ

| API | 種別 | 概要 |
|-----|------|------|
| `ChunkedDecoder::new()` / `with_max_body_bytes(max)` | fn | sans-IO インクリメンタルデコーダ（状態機械）を構築 |
| `ChunkedDecoder::decode(input, out)` | method | 可能な限りデコードし `out` へ追記。`DecodeOutcome::{Complete, Incomplete}`（各 `consumed` 付き） |
| `encode_chunk(data, out)` | fn | `<hex-size>\r\n<data>\r\n` を追記。空データは無出力（`0\r\n\r\n` の誤終端を構造的に防止） |
| `encode_terminator(out)` | fn | 終端 `0\r\n\r\n` を追記（trailer は出力しない） |
| `ChunkedError` | enum | `InvalidChunkSize` / `ChunkLineTooLong` / `TooManyChunks` / `BodyTooLarge` / `TrailerUnsupported` / `InvalidLineTerminator` |

構文は CRLF 厳格（bare LF 拒否）。非空 trailer は受理しない（安全側デフォルト）。エンコーダ側の利用文脈は[ストリーミングガイド](../guide/streaming.md)を参照。

### 2.10 `buffer` — 接続単位の読み取りバッファ

| API | 種別 | 概要 |
|-----|------|------|
| `RecvBuffer::new()` | fn | 空の読み取りバッファを構築。1 コネクションにつき 1 つ保持する契約 |
| `RecvBuffer::unread()` | method | 未消費バイト列（パイプライン残余含む）。`parse_request_head` へそのまま渡せる |
| `RecvBuffer::capacity()` | method | 内部容量の観測用（縮小ポリシーのテスト・監視補助） |

遅延コンパクション + ゼロ埋め回避で memmove コストを削減し、処理完了時に容量を 64 KiB（内部定数 `MAX_RETAINED_CAPACITY`）へ有界化して keep-alive 接続のメモリ滞留を防ぐ。

## 3. 契約・不変条件

| 契約 | 内容 |
|------|------|
| sans-IO | パーサ・エンコーダはソケット I/O を持たない純関数 / 状態機械。入力不足は `Incomplete` を返し、呼び出し元が追い読みして再入力する |
| 非デコード・無正規化 | `RequestHead::path` / `query` / `parse_query` / `parse_cookie_header` は percent-decode を行わず生のまま返す。ルート照合とパーサでデコード有無が食い違う正規化バイパス（OWASP A01）を防ぐ。デコードは照合確定後にハンドラが `percent` / `form` で明示的に行う |
| 二重デコード禁止 | `decode_bytes` / `decode_str` / `parse_form` は 1 値につき 1 回だけ適用する。デコード結果（`%00`・制御文字・`../` を含みうる）の再検証は呼び出し元の責務 |
| fail-closed | 上限超過・構文違反時は部分結果を一切返さず `Err`。cookie の不正 pair も黙ってスキップしない |
| ヘッダ送出経路の限定 | `Response` は無検証の任意ヘッダ送出 API を持たない。経路は `with_content_type`（`&'static str` 限定）・`with_allow` / `with_set_cookie`（構築時検証済み専用型）・`with_header`（構築時検証 + `Result`）の 4 つのみで、CRLF がワイヤに出る経路を型レベルで排除する |
| フレーミングの一元管理 | `Content-Length` / `Connection` / `Transfer-Encoding` は `with_header` で上書き不可（`HeaderError::ReservedName`）。`serialize`（Content-Length のみ）と `serialize_chunked_head`（Transfer-Encoding のみ）は経路分離され、両ヘッダが共存する応答は構造的に生成できない |
| bodyless ステータス | 1xx・204・304 は `serialize_chunked_head` がフレーミングヘッダを出力せず、呼び出し元も chunked body・終端チャンクを送出しない対の契約（キープアライブ上のレスポンス分割防止） |
| consumed 前進 | `ParseOutcome::Complete` / `DecodeOutcome` の `consumed` 分だけ呼び出し元がバッファを前進させ、残余（パイプライン済み次リクエスト・body 先頭）を保持する |

## 4. セキュリティ観点

### 4.1 DoS 上限定数一覧（全モジュール横断）

| 定数 | 値 | モジュール | 超過時のエラー |
|------|-----|-----------|----------------|
| `MAX_HEADER_BYTES` | 16 KiB | `request` | `ParseError::HeaderSectionTooLarge` |
| `MAX_HEADER_COUNT` | 100 | `request` | `ParseError::TooManyHeaders` |
| `MAX_BODY_BYTES` | 1 MiB（既定、`Server::max_body_bytes` で上書き可） | `body` | `BodyError::BodyTooLarge` / `ChunkedError::BodyTooLarge` |
| `MAX_CHUNK_COUNT` | 16,384 | `chunked` | `ChunkedError::TooManyChunks` |
| `MAX_CHUNK_LINE_BYTES` | 256 | `chunked` | `ChunkedError::ChunkLineTooLong` |
| `MAX_QUERY_BYTES` | 8 KiB | `query` | `QueryError::QueryTooLong` |
| `MAX_QUERY_PAIRS` | 256 | `query` | `QueryError::TooManyPairs` |
| `MAX_FORM_BYTES` | 8 KiB（`MAX_QUERY_BYTES` と同値固定） | `form` | `FormError::BodyTooLong` |
| `MAX_FORM_PAIRS` | 256（`MAX_QUERY_PAIRS` と同値固定） | `form` | `FormError::TooManyPairs` |
| `MAX_COOKIE_COUNT` | 100 | `cookie` | `CookieError::TooManyCookies` |
| `MAX_COOKIE_STRING_BYTES` | 8 KiB | `cookie` | `CookieError::CookieStringTooLarge` |

- いずれも**バッファ確保前に検査**し、超過時は部分結果を返さない（fail-closed）
- `RequestHead::cookies()` は `MAX_COOKIE_COUNT` / `MAX_COOKIE_STRING_BYTES` を複数 `Cookie` ヘッダに跨る**累積値**へ適用し、ヘッダ分割による上限迂回を防ぐ
- `percent` は追加上限を持たない（出力長 ≦ 入力長・`O(n)` 1 パスで、入力自体が上流の `MAX_HEADER_BYTES` 等で有界のため）

### 4.2 その他の防御

| 観点 | 内容 |
|------|------|
| レスポンス分割・ヘッダインジェクション | ヘッダ値の CR/LF/NUL + 制御文字（HTAB 除く）拒否、ヘッダ名の tchar 検証、reason phrase の固定テーブル化により CRLF 混入経路を排除 |
| リクエストスマグリング | CRLF 厳格構文（bare LF 拒否・obs-fold 拒否）、`Content-Length` 重複 / TE 共存 / 多重 TE の一律拒否、非空 trailer 拒否 |
| 情報漏えい | `HttpError` / `error_response` の `&'static str` 限定、エラー `Display` に拒否対象の値・機密を含めない方針 |
| JSON インジェクション | `error_response` は RFC 8259 準拠エスケープで常に妥当な JSON を出力 |
| ReDoS | パーサはすべて線形走査・バックトラックなし（`request` の部分列探索含む） |
| Cookie セキュリティ | `SetCookie` は `HttpOnly` / `Secure` / `SameSite` を builder で付与可能（セッション ID には `HttpOnly` を強く推奨）。`SameSite=None` は `Secure` を自動付与 |
| オープンリダイレクト | `Response::redirect` はワイヤ妥当性のみ検証する。外部入力由来の `location` の許可リスト検証は呼び出し元の責務 |

## 5. スコープ外・関連ドキュメント

本書が扱わない範囲:

- `connection`（ソケット読み取りループ）・`socket`（`TCP_NODELAY`、feature `net`）: サーバ内部実装であり、利用者はサーバ経由で間接利用する。詳細は `crates/http/src/connection.rs` / `crates/http/src/socket.rs` の rustdoc を参照
- ルーティング（`Router` / パスパラメータ / fallback）: [Router API](./router-api.md)
- `Server` の組み立て・graceful shutdown: [Server API](./server-api.md) と[グレースフルシャットダウンガイド](../guide/graceful-shutdown.md)
- 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）: [Extension API](./extension-api.md) と[拡張点ガイド](../guide/extension-points.md)
- プラグインの設定型: [Plugin Config API](./plugin-config-api.md)
- ストリーミング送信の使い方（`Handler::handle_streaming` / `StreamingResponse`）: [ストリーミングガイド](../guide/streaming.md)
- 入門・チュートリアル: [ガイド目次](../guide/README.md)・[Getting Started](../guide/getting-started.md)・[feature 構成別サンプル](../guide/feature-samples.md)・[チュートリアル](../guide/tutorial.md)
- 設計判断の経緯: `docs/design/` 配下（例: `docs/design/async-handler.md`・`docs/design/graceful-shutdown.md`）
