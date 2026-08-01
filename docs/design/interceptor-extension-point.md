# ユーザー向けインターセプト・レスポンス改変拡張点 `Interceptor`（イシュー #420）

## 背景

crates.io 利用者（`fandhe-backend-core` v0.1.0）からの実測フィードバック。静的配信
サイトで「末尾スラッシュの 301 正規化」「カスタム 404 ページ」を実装しようとしたが、
既存の 3 拡張点（`crates/core/src/extension.rs`）のいずれにも実現経路がなかった:

- [`Middleware`](../../crates/core/src/extension.rs) — 観測専用契約。`on_request` は
  `&RequestHead` しか受け取らず、`on_response` もレスポンスへの参照を持たない
- `RequestGate` — `GateOutcome::Allow` / `Reject { status, body }` の二択で、`Reject`
  はヘッダを運べないため 301 + `Location` を表現できない
- `UpgradeHandler` — 長時間接続への委譲判定専用

`Router::fallback`（#316）は `Handler` 到達後にしか効かず、`plugin-static` は
`try_intercept` 段階で `Response::empty(404)` を確定させるため（
`crates/plugin-static/src/lib.rs`）、静的配信 mount 配下のカスタム 404 は
Router 側でも実現不能だった。

## イシューの 2 提案と採否

イシューは (a) `Option<Response>` を返す軽量インターセプト拡張点、(b) `Server` への
エラーレスポンス差し替えフック、を「または」で提案していた。分析の結果:

- (a) 単独では **plugin-static が確定させる 404 の差し替えができない**（インターセプト
  はプラグイン評価より前に走るため、404 になるかを知り得ない）
- (b) 単独では **成功パスのリダイレクトなど、応答確定前の介入ができない**

そこで両者を **1 つの trait `Interceptor`（2 フック・既定 no-op）** として統合実装
した。これは既存の `Middleware`（`on_request`/`on_response` の 2 フック 1 trait）と
同じ流儀であり、API 表面積を最小に保つ。

## 「拡張点は 3 種類に限定」原則との整合

`docs/spec/04-requirements.md` REQ-2 受け入れ基準は「既存 3 拡張点で表現できない場合
にのみ新規 trait の追加を検討する」と定める。本件は上記のとおり 3 拡張点のいずれでも
表現不能であることを確認済み（`Middleware` はレスポンス参照なし・`RequestGate` は
ヘッダなし・`UpgradeHandler` は接続委譲専用）。

また `Handler`（`crates/core/src/server.rs`）が既に「3 拡張点の対象外の既定レスポンダ
差し込み口」という前例を確立している。`Interceptor` もこれと同じ「レスポンダ系シーム」
ファミリーとして位置づける。

**spec への追随の要否**: `docs/spec/04-requirements.md` は別リポジトリ
（`Fandhe-AI/fandhe-backend-spec`）の submodule であり、本実装からは直接編集しない。
「3 拡張点で表現できない場合にのみ新規 trait を追加する」という受け入れ基準の運用上の
例外事例（`Handler` に続く 2 例目）として、spec リポジトリ側への追随提案は
out-of-scope-tracking（`.claude/rules/out-of-scope-tracking.md`）に従いイシュー #432
として起票済み。**追随完了**: `Fandhe-AI/fandhe-backend-spec` へ
[PR #4](https://github.com/Fandhe-AI/fandhe-backend-spec/pull/4) で前提条件・REQ-1・
REQ-2・REQ-13・制約事項の「拡張点は 3 種類に限定」記述を 4 種類（`Interceptor` 追加）
へ更新し、マージ済み（`bccb876`）。本リポジトリの `docs/spec` submodule 参照もこの
コミットへ更新した（イシュー #432）。PoC 記録・完了タスク本文・ロードマップは当時の
事実の歴史的記録として据え置いている。

## 設計: trait `Interceptor`（`crates/core/src/interceptor.rs`）

```rust
pub trait Interceptor: Send + Sync {
    fn name(&self) -> &'static str;
    fn intercept(&self, _head: &RequestHead, _body: &[u8]) -> Option<Response> { None }
    fn map_response(&self, _head: &RequestHead, response: Response) -> Response { response }
}
```

- **同期 API**（3 拡張点と同じ dyn 互換設計）。`Middleware` と同一の「同期ブロッキング
  I/O 禁止」契約（PoC-3 実測でスループット最大 25% 劣化）。カスタム 404 ページ等は
  起動時にメモリへプリロードして返す使い方を doc test で示す
- 登録: `Server::interceptor(impl Interceptor + 'static)`（builder、
  `Vec<Box<dyn Interceptor>>` 保持、複数登録可・登録順評価）
- **feature ゲート不要**: 外部依存ゼロの純コア機能であり、`Handler`・3 拡張点と同じく
  「実装ゼロなら実行時コストもゼロ」（未登録時は空 `Vec` の走査のみ）。
  pay-for-what-you-use（`.claude/rules/pay-for-what-you-use.md`）に反しない

## 処理フローへの組み込み（`handle_connection_with_permit`）

```text
1. Middleware::on_request
2. RequestGate::check（変更なし・最優先のまま）
3. UpgradeHandler::matches（変更なし）
3.5. Interceptor::intercept（新規。登録順、最初の Some が勝つ）
4. plugin::try_intercept（intercept が Some なら skip）
5. Handler::handle / handle_streaming（同上 skip）
5.4. Interceptor::map_response（新規。登録順に逐次適用。`handle_streaming` 経路では
     `write_streaming_response` のヘッド確定時に同じく登録順で適用、イシュー #434）
5.5. plugin::finalize_response（CORS → 圧縮。map_response の後。ストリーミング応答には
     未適用のまま。ストリーミング応答には代わりに plugin::finalize_streaming_head
     （CORS のみ）+ plugin::prepare_streaming_compression（チャンク単位の
     ストリーミング gzip 圧縮、イシュー #461）が `write_streaming_response` の
     ヘッド確定時に適用される、イシュー #451）
6. レスポンス書き込み → Middleware::on_response
```

### 評価位置の設計判断

- **`RequestGate` より後**: ゲートの既定拒否（フェイルクローズ）をユーザーコードで
  迂回不能にする（既存の「ゲートが最優先」原則を維持。OWASP A01 対策）
- **`UpgradeHandler` より後**: 確立済みの Upgrade 委譲・permit 引き継ぎ意味論
  （TASK-4.2）に触れない
- **`plugin::try_intercept` より前**: plugin-static 等の登録済みプラグインをユーザー
  コードが明示的に先取りできる（末尾スラッシュ 301 正規化のユースケース成立条件）
- **`map_response` は `finalize_response` より前**: CORS ヘッダ付与・gzip 圧縮は
  改変後の最終 body に対して適用されるべきため（圧縮は「body を確定させる後処理として
  必ず最後」の既存規約を維持）

### `map_response` を通さない応答（fail-closed 維持、`finalize_response` と同一の設計判断）

- `RequestGate` 拒否応答
- パースエラー応答（400 等）
- Upgrade 委譲失敗 501 / shutdown 503

ストリーミング応答（`handle_streaming`）は当初（本設計時点）#319 と同一理由（`Response`
型前提のシームであるため）でスコープ外としていたが、イシュー #434 で「ステータス・
ヘッダのみ」の限定的な適用へ変更した。詳細は次節を参照。

## イシュー #434: ストリーミング応答への `map_response` 適用

### 適用範囲: ステータス + ヘッダのみ。body 改変はスコープ外（破棄）

ストリーミング応答の実データは producer タスクが `crate::streaming::BodyWriter` 経由で
逐次供給し、chunked framing は `write_streaming_response`（`crates/core/src/server.rs`）が
`crate::streaming::{encode_chunk, encode_terminator}` で直接組み立てる。`Response::body` を
経由しないため、`map_response` が返した `Response` の **body は反映されず破棄する**
（`head_response.body = Vec::new()` を直列化前に明示実行）。

- **不採用案 1: body 差し替えを許す**。バックプレッシャ（bounded mpsc）・応答完全性
  契約（`finish` 省略時は終端チャンクなしで打ち切りクローズ、イシュー #319）と両立
  できず、body 全体のバッファリングが必要になり #319 のストリーミング設計そのものを
  破壊するため不採用
- **不採用案 2: ストリーミング専用の新規フックを追加する**（例: `map_streaming_head`）。
  「拡張点は既存 3 種 + レスポンダ系シームで表現できない場合にのみ追加検討」という
  REQ-2 の原則（本文書冒頭を参照）に対し、既存の `map_response` シグネチャ
  （`&RequestHead, Response -> Response`）で「ステータス・ヘッダのみ反映・body 破棄」
  という契約を明文化するだけで表現可能なため、API 表面積を増やさない現行案を採用

### 適用位置: `write_streaming_response` 冒頭で 1 回、HTTP/1.0・HTTP/1.1 両経路共通

従来 HTTP/1.0 経路・HTTP/1.1 経路それぞれで重複していた `head_response`
（`Response::empty(streaming.status)` + `content_type`）の構築を関数冒頭（バージョン
分岐前）へ統合し、直後に `server.interceptors` を登録順に fold して `map_response` を
適用する。**mapped 後の `status`（`head_response.status`）を以降のすべての判定に一貫
使用する**。特に `Response::is_bodyless_status` 判定（1xx・204・304 で body 送出・
終端チャンクをスキップ）を mapped status へ切り替えたことで、インターセプタが例えば
200 → 204 へ書き換えた場合に「ヘッダ側の `Transfer-Encoding` 抑制」と「body 送出・
終端チャンクのスキップ」が対で成立し、レスポンス分割類の脅威（`serialize_chunked_head`
doc の RFC 9112 §6.3 コメントと同一脅威）を構造的に防ぐ。`map_response` の呼び出しは
ヘッド確定時の 1 回のみで、チャンクごとには呼ばない（同期・非ブロッキング契約は
PoC-3 実測根拠のまま変更なし）。

### スコープ外の明確化

- `plugin::finalize_response`（`Response` 型前提の通常応答経路専用シーム、body
  全体を前提とする `apply_compression` を含む CORS → 圧縮の逐次適用）自体は
  ストリーミング応答には引き続き適用しない。イシュー #451 で
  `plugin::finalize_streaming_head`（`finalize_response` の第 4 のシーム）を新設し、
  `write_streaming_response` のヘッド確定時（`map_response` 適用の直後）に **CORS
  ヘッダ付与**を適用するようになった。圧縮は gzip がストリーム body 全体を
  確定させる後処理であり `apply_compression` をそのまま持ち込めないため、
  イシュー #461 で body を保持しない専用エンコーダによる**別の第 5 のシーム**
  （`plugin::prepare_streaming_compression`）を新設し、チャンク単位の
  ストリーミング圧縮を接続した（`crates/core/src/plugin.rs` の
  `finalize_streaming_head`・`prepare_streaming_compression` の doc・
  `docs/design/plugin-boundary.md` 5.9.7 節・5.10.6 節を参照）
- `RequestGate` 拒否応答・パースエラー応答・Upgrade 失敗 501 / shutdown 503 の除外
  （fail-closed）は変更しない

## ユースケース充足の確認

- **301 正規化**: `intercept` で `head.path()` を検査し `Response::redirect(301, ...)`
  （#301/#302 で実装済みの検証付き API）を返す。plugin-static 登録時もインターセプト
  が先行する（`crates/core/tests/plugin_static_boundary.rs` の
  `interceptor_intercept_takes_priority_over_static_mount` で検証）
- **カスタム 404**: `map_response` で `response.status == 404` のとき起動時プリロード
  済みの 404 ページ body へ差し替える。plugin-static の一律 404・Handler 未登録 404・
  Router 404 のすべてに一箇所で効く（同ファイルの
  `interceptor_map_response_rewrites_static_404_body` で検証）

## 受け入れ基準

イシューは feature-request テンプレート外で受け入れ基準の明記がないため、
`.claude/rules/feature-modification.md` に従い本設計文書で以下を定義する:

1. 利用者が `Server::interceptor` 登録のみで「特定条件のリクエストへ 301 リダイレクト
   を返す」を実装でき、`Server::static_files` 登録時もリダイレクトが優先されること
2. 利用者が `map_response` で plugin-static の 404 応答（`Response::empty(404)` 固定）
   の body を差し替えられること
3. `Interceptor` 未登録時、既存の全挙動・依存ツリー・バイナリが変化しないこと
   （後方互換）
4. `RequestGate` 拒否応答・パースエラー応答は `Interceptor` で改変不能であること
   （フェイルクローズ維持）
5. 公開 API に doc comment + doc test が付き、全 feature 構成（なし・個別・全）で
   CI 緑
6.（イシュー #434）`Handler::handle_streaming` によるストリーミング応答のヘッド
   （ステータス・`Content-Type`・追加ヘッダ）に登録済み `Interceptor::map_response` が
   HTTP/1.0・HTTP/1.1 両経路で適用され、`map_response` が返した body は反映されず
   ワイヤに現れないこと。`Interceptor` 未登録時のストリーミング応答は従来と同一
   （後方互換）であること

## セキュリティ考慮事項（OWASP Top 10 観点）

- **A01 アクセス制御の不備**: `Interceptor` は `RequestGate` より**後**に評価する
  ため、認証・認可・同意ゲートの既定拒否をユーザーコードで迂回できない
  （`crates/core/tests/interceptor.rs` の
  `request_gate_rejection_bypasses_interceptor_entirely` で固定）
- **A03 インジェクション**: intercept/map_response が返す応答は `Response` の検証済み
  API（`with_header` の CR/LF/NUL 検証・`redirect` の Location 検証、#301）経由でしか
  組み立てられず、レスポンス分割・ヘッダインジェクションの新経路を作らない
- **A04 安全でない設計（fail-closed）**: `RequestGate` 拒否応答・パースエラー応答は
  改変対象外とし、最小情報応答のフェイルクローズ方針を維持（`finalize_response` と
  同一の設計判断）。既定実装は完全 no-op で、未登録時の挙動変化ゼロ
- **A05 セキュリティ設定ミス**: opt-in 登録型。plugin-static の「未検出・検証失敗・
  サイズ超過は一律 404」という情報非開示特性は保たれる（`map_response` は一律 404
  のみを観測し、失敗原因を区別できない）
- **リソース枯渇（DoS）**: 同期契約・ブロッキング I/O 禁止を `Middleware` と同一規約で
  doc に明記（PoC-3 実測根拠）。カスタム 404 ページは起動時プリロードを doc test で
  提示。コア側にバッファ・タイマー等の新規リソースは追加しない
- **シークレット**: `name()` にリクエスト内容（トークン・PII）を含めない契約を
  `Middleware::name` と同一文言で明記
- **A03 インジェクション / レスポンス分割・スマグリング（イシュー #434 追加）**:
  ストリーミング応答でもヘッダ改変経路は通常応答と同じ `Response::with_header`
  （`Content-Length` / `Transfer-Encoding` / `Connection` は `HeaderError::ReservedName`
  で構築時拒否、CR/LF/NUL も拒否）に限られ、framing 矛盾ヘッダを注入する経路は型
  レベルで存在しない。mapped status を `is_bodyless_status` 判定・framing 判定へ
  一貫使用することで「ヘッダのみ抑制して終端チャンクを送る」型のレスポンス分割を
  防ぐ（`streaming_map_response_status_rewrite_to_bodyless_suppresses_framing` で
  固定）。body 破棄により chunked framing の完全性（producer のチャンクのみが
  ワイヤに乗ること）も維持する（`streaming_map_response_body_is_discarded` で固定）

## 参照

- 実装: `crates/core/src/interceptor.rs`（trait 定義・doc test・unit test）、
  `crates/core/src/server.rs`（`Server::interceptor`・処理フロー組み込み）
- 統合テスト: `crates/core/tests/interceptor.rs`、
  `crates/core/tests/plugin_static_boundary.rs`（static との交差）、
  `crates/core/tests/plugin_compression_boundary.rs`（圧縮との交差）
- 拡張点全体の設計原則: `.claude/rules/coding-rust.md`・`AGENTS.md`
  「AI エージェント向け変更ガイド」節
- pay-for-what-you-use: `.claude/rules/pay-for-what-you-use.md`
