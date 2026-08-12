# P1 ヘッダゼロコピー化（`RequestHead` の Range 保持）設計検討

- **対応イシュー**: [#588](https://github.com/Fandhe-AI/fandhe-backend/issues/588)
  「P1 ヘッダゼロコピー化（`RequestHead` の Range 保持）の設計検討」
- **出典**: 性能改善ツリー [#579](https://github.com/Fandhe-AI/fandhe-backend/issues/579)
  （2026-08-11、15 フレームワーク横断ベンチ起点）Phase 2 の設計判断イシュー
- **ステータス**: ドラフト（自動運転モードでの実装であるため、本ドキュメントは安全側の
  保守的判断として作成したドラフトであり、**最終承認は人間レビュー（本イシューの PR
  レビュー）で行う**。Phase 3 実装（#590〜#593）は本承認を経てから着手する
  ［feasibility-guardrail 準拠、`.claude/rules/feasibility-guardrail.md`］）
- **対応可否判定（feasibility-guardrail）**: **可**。受け入れ基準あり（設計文書に検討・
  採用案・不採用案の根拠を記録し、Phase 3 issue の受け入れ基準を確定させる。検証可能）・
  安全性方針と整合（ドキュメント追加のみ、コード変更なし。既存の DoS 上限・入力検証契約を
  後退させない設計を条件化する）・影響範囲限定（`docs/design/` + `CLAUDE.md` の該当箇所の
  み。Phase 3 のコード変更は別イシューでレビューゲートを経る）の 3 軸すべて充足

## 0. 結論（先出し）

- **採用案**: 案 B（所有ヘッドバッファ + `Range<usize>` 列。ライフタイムパラメータを
  `RequestHead` に持ち込まない）
- **移行方式**: 一括 breaking change として Phase 3（#590〜#593）で実装する。アクセサ
  先行導入 2 段階案は本件では見送る（3.2 節で理由を記録）
- **バージョニング**: `docs/design/versioning-policy.md` の pre-1.0 規則に従い `0.y` の
  `y` を 1 つ上げる（現行 0.3.0 → 0.4.0 が有力候補、正式決定は Phase 3 実装 PR で行う）
- **効果見込み**: 5 節の実測に基づき、ヘッダ本数 N に対して現状 `5 + 2N` alloc/req 前後
  （後述の実測値参照）が **2 alloc/req 前後（N に非依存）** へ削減できる見込み

## 1. 背景

### 1.1 ベンチ根拠

性能改善ツリー #579（2026-08-11 実測）で `/health` エンドポイントが 31.7 万 RPS、
hyper 素実装（`axum-ref` 相当の最小構成）が 35.1 万 RPS。fandhe-backend はこれに対し
約 −10% であり、その主成分がリクエストあたりのヒープアロケーション過多にあると分析
されている（#579 Phase 2 設計判断イシュー群の起点）。

### 1.2 alloc 内訳（実コード確認済み、行番号は本イシュー着手時点の `crates/http/src/request.rs`）

| 箇所 | 行 | 内容 | alloc 数（1 リクエストあたり） |
|------|-----|------|------|
| `parse_request_line` | request.rs:509-510 | `method` / `target` を `String::from_utf8(x.to_vec())` で所有化 | 2 |
| `parse_header_line` | request.rs:559-560 | ヘッダ 1 本につき name / value を `String::from_utf8(x.to_vec())` で所有化 | 2N（N = ヘッダ本数） |
| `parse_request_head` | request.rs:373 | `headers: Vec<(String, String)>` の格納先 `Vec` | 1（+ 容量不足時の再確保） |

該当箇所は現行実装（`RequestHead`）:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    pub target: String,
    pub version: HttpVersion,
    headers: Vec<(String, String)>,
}
```

`method` / `target` は `pub` フィールド、`headers` は非公開で `header()` /
`headers()` アクセサ経由（`&str` を返す）。

## 2. 公開 API 影響の全量調査

`RequestHead` を参照する `.rs` ファイルは `grep -rl "RequestHead" --include="*.rs" .`
で **72 ファイル**（2026-08-11 時点、workspace + `templates/` + `examples/`）。内訳:

| 領域 | ファイル数（概算） | 主な参照形態 |
|------|------|------|
| `crates/http`（本体・fuzz・tests） | 6 | 定義本体、`req.head.method` / `req.head.target` 直接比較（`connection.rs`・`http_flow.rs`）、`RequestHead::path()` 内部で `self.target` 参照（request.rs:136-163） |
| `crates/core`（拡張点・plugin シーム・tests） | 25 | `Middleware::on_request/on_response`・`UpgradeHandler`・`RequestGate::check`・`Interceptor::intercept/map_response`・`Handler::handle/handle_streaming` の全シグネチャが `&RequestHead` を引数に取る（`crates/core/src/extension.rs:92,96,135,365`、`server.rs:249,326`）。`plugin.rs:369,383` が `head.method == "GET" && head.target == "/openapi.json"` の完全一致判定に使用 |
| `crates/routes` | 7 | `Router::dispatch(&self, head: &RequestHead, body: &[u8])`（`routes/src/lib.rs:708`）。静的ルート照合 `head.method.as_str()`（lib.rs:718）、パラメータルート照合 `param_route.method == head.method`（lib.rs:738）、405 応答生成時の `param_route.method.clone()`（lib.rs:741、`RouteEntry.method: String` 側の clone であり `RequestHead` 側には現れない）|
| `crates/plugin-*`（websocket / graphql / openapi / webrtc / webrtc-proxy / cors / compression / static / tracing / hub-wiring） | 27 | いずれも `head.method` / `head.target` の完全一致・前方一致判定（例: `plugin-websocket/src/handshake.rs:32,57`、`plugin-graphql/src/lib.rs:278`、`plugin-cors/src/lib.rs:285` の `eq_ignore_ascii_case`）。`plugin-tracing/src/layer.rs:105` はログ出力に `%head.method` を埋め込む（`Display` 前提、`&str` ビューでも問題なし） |
| `templates/app` / `examples/*` | 3 | ユーザー向けサンプルコードとしての参照（ハンドラ登録・ログ出力） |

### 2.1 特に影響が大きい参照パターン

- **`pub` フィールド直接アクセス**（`head.method` / `head.target`）: 上記 2 節の表の
  ほぼ全域。案 B（3.2 節）を採用する場合、フィールドを非公開化しアクセサ
  `method()` / `target()`（`&str` 返却）へ統一する必要がある。既存の `header()` /
  `headers()` と同型のアクセサパターンであり、置換自体は機械的（`sed` 相当）に行える。
  **`head.version`（`pub version: HttpVersion`）はこの非公開化の対象に含めない**
  （3.2 節）。`HttpVersion` は `Copy` な enum で alloc 削減の動機がなく、`workspace`
  内の `crates/plugin-websocket/src/handshake.rs:60`・`crates/http/src/body.rs:206`・
  `crates/http/src/request.rs:51,339,519,608,637`・`crates/http/src/connection.rs:133`・
  `crates/http/tests/http_flow.rs:47,96,138,152`・`crates/core/src/server.rs:2727` の
  `head.version` 直接参照は本設計変更後も無変更で動作する（Codex レビュー #600 指摘 1
  対応。当初の設計文書では `version` フィールドの扱いが構造体スニペット上で非公開に
  見える書き方になっており、本文と Phase 3 受け入れ基準（8 節）が `method`/`target`
  のみを breaking として扱う前提と整合していなかったため、明記して解消した）
- **`Clone` / `PartialEq` / `Eq` derive**: `#[derive(Debug, Clone, PartialEq, Eq)]`
  （request.rs:372）。`Request { head, body }`（`connection.rs:32-37`）も `Clone` を
  要求する箇所がないか要確認（現状 `Request` 自体は `Clone` を derive していない。
  `RequestHead` 単体の `Clone` は主にテストヘルパでの複製用途）。案 B（所有バッファ）
  なら `Clone` は「バッファの deep copy」相当のコストで維持でき、意味論は変わらない
- **`routes/src/lib.rs:690-712` のルーティングキー照合**: イシュー #583 で導入された
  ネスト map 照合（`self.routes.get(head.path()).and_then(|m| m.get(head.method.as_str()))`）
  は `&str` 借用のみで完結しており、`String` 化・clone は発生しない設計にすでになって
  いる（コメント既記載）。`method()` アクセサへの置換後もこの無 alloc 性質は保たれる
- **`Handler::handle` 等 4 拡張点のシグネチャ**: 全て `head: &RequestHead` の**共有参照**
  で受け取る（`fn handle(&self, head: &RequestHead, body: &[u8])` 等、
  `crates/core/src/server.rs:249,326`、`extension.rs:92,96,135,365`、
  `interceptor.rs`）。**戻り値・引数のどこにも `RequestHead` の所有権移動がない**ため、
  `RequestHead` 自体にライフタイムパラメータを導入しない限り、これら 4 trait のシグネ
  チャは無変更のまま維持できる（3.2 節の案 B が trait 波及を避けられる根拠）

## 3. ライフタイム設計

### 3.1 案 A: `RequestHead<'buf>`（`RecvBuffer` 借用 + Range 保持の完全ゼロコピー）

`RequestHead` が `RecvBuffer` の内部バイト列を直接借用し、method/target/header の
各 Range から `&'buf str` ビューを都度算出する構造。

**問題点**（`crates/http/src/buffer.rs` の契約と衝突）:

- `RecvBuffer::consume`（buffer.rs 内、カーソル前進のみで消費を表現）・次回読み取り時の
  遅延コンパクションは、`RequestHead` が同じ `RecvBuffer` の借用を保持している間は
  `&mut RecvBuffer` を取れず**呼び出せない**。body 読み取り（`Content-Length` 分の
  追加 recv、`connection.rs` の `read_request_with_limit`）は head パース後に同一
  バッファへ追加読み取りを行う設計であり、head の借用が生きている間はこの追加読み取り
  ループ自体が借用チェッカ上不可能になる
- 回避には `RequestHead<'buf>` を body 読み取り完了前に確実に破棄させる呼び出し順序
  制御が必要になるが、`Request { head, body }`（`connection.rs:32-37`）が両者を同時に
  保持する現行構造と根本的に相容れない（`head` を借用のまま `Request` に同梱できない）
- ライフタイムパラメータは `RequestHead<'buf>` から `Handler::handle(&RequestHead<'?>, ...)`
  ・`Middleware::on_request(&RequestHead<'?>)` 等、2 節で列挙した**4 拡張点 trait 全て**
  へジェネリックライフタイムとして波及する。`dyn Middleware` のような trait object 化
  （`Server::middleware` 等が `Box<dyn Middleware>` で複数登録を許す設計、`server.rs`）
  とライフタイムパラメータは相性が悪く（`for<'a> dyn Middleware<'a>` 相当の HRTB が
  必要になり type-erasure が困難）、拡張点の実用性を大きく損なう

**判定**: 不採用。alloc 削減効果は最大だが、コア設計（`RecvBuffer` 再利用契約・4 拡張点
の trait object 化）との構造的衝突が大きすぎる。

### 3.2 案 B: 所有ヘッドバッファ + Range 保持（採用）

`RequestHead` がヘッド部（リクエストライン + ヘッダ + 終端空行、`MAX_HEADER_BYTES`
= 16 KiB 以下）のバイト列を **1 回のコピーで所有**し、method / target / 各ヘッダ
name・value を `Range<usize>` で保持する。公開 API はアクセサ経由の `&str` ビューの
みとする。

```rust
pub struct RequestHead {
    buf: Box<str>,                 // ヘッド部バイト列（所有、1 回コピー。UTF-8 検証済み、6.3 節）
    method: Range<usize>,
    target: Range<usize>,
    pub version: HttpVersion,      // 現行どおり pub のまま維持（Copy 型・alloc 対象外、2.1 節）
    headers: Vec<(Range<usize>, Range<usize>)>,
}

impl RequestHead {
    pub fn method(&self) -> &str { &self.buf[self.method.clone()] }
    pub fn target(&self) -> &str { &self.buf[self.target.clone()] }
    // header() / headers() は既存アクセサをそのまま維持（内部実装のみ Range 経由に変更）
}
```

`version: HttpVersion` は `method` / `target` と異なり alloc 削減の対象ではない
（`HttpVersion` は `Copy` な enum で所有コストがなく、`buf` へ切り出す動機がない）。
現行実装で `pub` であるフィールドを本設計変更で非公開化する対象は `method` /
`target` の 2 フィールドのみとし、`version` は breaking change に含めない
（Codex レビュー #600 指摘 1 対応。2.1 節・8 節の #591/#592 受け入れ基準に反映）。

**構造上の利点**:

- `RequestHead` はライフタイムパラメータを持たない **`'static` な所有型のまま**。
  4 拡張点 trait のシグネチャ（`&RequestHead` の共有参照渡し）は 2.1 節の分析どおり
  **無変更**で成立する。`dyn Middleware` 等の trait object 化にも影響しない
  （これが案 A に対する採用の主因）
- `RecvBuffer` との関係が単純になる: head パース完了時点で `RequestHead::buf` へ
  ヘッド部を 1 回コピーし、その後は `RecvBuffer` 側のカーソル前進・コンパクション・
  次回読み取りと `RequestHead` の生存期間が完全に独立する（借用の衝突が原理的に
  発生しない）
- alloc 数: `buf: Box<str>` で 1 alloc（ヘッド部全体を 1 回のメモリコピー + 構築時
  UTF-8 検証 1 回で確保。`Box<[u8]>` ではなく `Box<str>` を採用する理由は 3.2 節
  トレードオフ・6.3 節参照）+ `headers: Vec<(Range, Range)>` で 1 alloc（+ 必要なら
  再確保。事前に `MAX_HEADER_COUNT` を上限に `Vec::with_capacity` することで再確保も
  回避可能）。method/target は `Range` のみで alloc なし。**ヘッダ本数 N に依存しない
  定数個の alloc**（2 個前後）に削減できる

**トレードオフ**:

- ヘッド部の生バイト列を 1 回コピーするコスト自体は残る（案 A ほどの完全ゼロコピー
  ではない）。ただし現状実装も `find_subslice` によるヘッダ部切り出し
  （`&buf[..header_end]`）を経ており、コピー回数の絶対値としては「N+1 回の
  `to_vec()`」（現状）→「1 回の `Box<str>` 化」（案 B）への削減であり、alloc 回数
  ・コピー総バイト数のいずれも改善する
- UTF-8 検証はアクセサ呼び出しのたびに行わない。`buf` を `Box<[u8]>` ではなく
  `Box<str>` として保持し、**構築時（ヘッド部コピー直後）に 1 回だけ**
  `str::from_utf8(&raw_bytes)` を実行して検証する。検証に失敗した場合は
  `parse_request_head` の既存エラー型（`ParseOutcome`/`ParseError`、6.1 節）で
  `Result` として呼び出し元へ伝播し、`.unwrap()` / `.expect()` は使わない
  （`.claude/rules/coding-rust.md` のライブラリコード方針。Codex レビュー #600
  指摘 2 対応）。構築後は `self.buf` が妥当な `&str` であることが型で保証されるため、
  `method()` / `target()` アクセサは `&self.buf[self.method.clone()]` という
  非 fallible な `str` のバイト範囲インデックスで実装できる（`unsafe` 不使用、
  `from_utf8_unchecked` も不要）。この範囲インデックスが UTF-8 文字境界からずれて
  panic する余地がないことは、Range の生成元（tchar・CRLF 等の区切りバイトは常に
  ASCII、すなわち 1 バイト目が `0x00`〜`0x7F`）と UTF-8 の性質（マルチバイト文字の
  継続バイトは常に `0x80` 以上）から導かれる不変条件として 6.2 節・6.3 節に明記する

### 3.3 案 C: `Cow` ハイブリッド

`method` / `target` / ヘッダ値を `Cow<'buf, str>`（借用 or 所有）で保持し、通常経路は
借用、テストヘルパ等の複製が必要な経路のみ所有化するハイブリッド。

**判定**: 不採用。`Cow<'buf, str>` は結局 `'buf` ライフタイムパラメータを
`RequestHead<'buf>` に持ち込むため、案 A と同じ trait object 化の問題を抱える。
かつ borrowed/owned の分岐が呼び出し元コードに漏れ出し（`match head.method() { Cow::Borrowed(s) => .., Cow::Owned(s) => .. }`
のような分岐は生じないにせよ、内部実装・テストの分岐コストが増える）、API の単純さ
を損なう。案 B が同等以上の alloc 削減効果をライフタイムパラメータなしで達成できる
ため、C を積極採用する理由がない。

## 4. 段階移行案の比較

### (a) 一括 breaking change（採用）

Phase 3（#590〜#593）で `crates/http` の内部構造変更 → workspace 全域の追随 → 検証
を通しで実施し、`0.y` の `y` を 1 つ上げてリリースする。

### (b) アクセサ先行導入 + pub フィールド deprecate → 内部差し替えの 2 段階

第 1 段で `method()` / `target()` アクセサを追加し `pub method: String` /
`pub target: String` を `#[deprecated]` にする（内部構造は `String` のまま）。
第 2 段で内部構造を案 B へ差し替える。

**判定**: 不採用。理由:

- 第 1 段時点では alloc 削減効果がゼロ（内部構造が `String` のまま）。効果を得るには
  結局第 2 段（= 内部構造の破壊的変更）が必要であり、2 段階に分けても「アクセサ経由
  でしか触れない」という利用者側の**ソースコード**互換性緩和効果しか得られない
- `pub` フィールドは Rust の semver では**フィールドの型・可視性変更そのものが
  breaking**（`docs/design/versioning-policy.md` 3 節）。deprecate 期間を挟んでも
  最終的に `pub method: String` を除去する時点で breaking change 自体は避けられず、
  2 段階に分けることで「2 回の breaking リリース」を経由するコストの方が大きい
- 72 ファイルの参照はいずれも同一リポジトリ内（workspace + templates/examples）で
  あり、外部利用者向けの段階的移行を要する事情（crates.io 公開先の広範な既存利用者
  への配慮）は現時点で強くない（0.3.0 時点でまだ pre-1.0・利用実績が薄い）。一括
  breaking の方が変更差分を追跡しやすく、レビューコストも 1 回で完結する

### (c) `Cow` 型ハイブリッド移行

3.3 節の案 C をそのまま移行方式として採用する案。

**判定**: 不採用。3.3 節で述べたとおり案 C 自体を設計として不採用としたため、移行方式
としても採用しない。

### 結論

(a) 一括 breaking を採用する。`.standalone-crates-io-skip`（`scripts/standalone-crates-io-check.sh`
運用、`docs/design/crates-io-release.md` 8 節）は Phase 3 実装 PR がマージされ
crates.io へ再公開されるまでの間、`templates/app` / `examples/*` に暫定配置する
（新 API 依存を理由とする既存パターンの踏襲、#592 の受け入れ基準に反映済み）。

## 5. 効果見込みの裏取り（alloc プロファイル実測）

### 5.1 手法

外部依存を増やさず、`#[global_allocator]` にカウンティングラッパー
（`System` を委譲先とし `alloc` 呼び出し回数・バイト数を `AtomicUsize` で集計）を
実装したスクラッチバイナリを一時的に作成し、`fandhe-backend-http`（本リポジトリの
`crates/http`）に対して `path` 依存で `parse_request_head` を直接呼び出して計測した
（本バイナリはコミットしていない。設計文書に手法とコマンド・結果のみを転記する運用は
本イシュー計画に明記済み）。

計測対象: ヘッダ本数 N ∈ {0, 1, 5, 10, 30} のリクエスト（`GET /items?x=1 HTTP/1.1` +
`Host` ヘッダ 1 本 + `X-Custom-Header-{i}: value-{i}` を N 本）に対して
`parse_request_head` を 1 回呼び出し、呼び出し前後の alloc 回数・alloc バイト数の
差分を記録（1 回のウォームアップ呼び出しでページフォルト等の外乱を除外）。

### 5.2 結果（`cargo run --release`、opt-level=1）

| ヘッダ本数 N | alloc 回数（現状実装） | alloc バイト数（現状実装） |
|------|------|------|
| 0 | 5 | 220 |
| 1 | 7 | 244 |
| 5 | 16 | 724 |
| 10 | 27 | 1,612 |
| 30 | 68 | 3,668 |

回帰的にはおおむね `alloc回数 ≈ 5 + 2.1N` 前後（`Vec<(String, String)>` の push に伴う
容量再確保が加わる分、単純な `5 + 2N` よりわずかに多い）。N=10（実運用で典型的な
ヘッダ本数）では **27 alloc/req**。

### 5.3 採用案（案 B）での見込み

案 B は `buf: Box<str>`（1 alloc）+ `headers: Vec<(Range, Range)>`
（`Vec::with_capacity(実ヘッダ数)` で確保すれば再確保なしの 1 alloc）の
**構造上定数 2 alloc/req（N に非依存）** になる見込み。N=10 の実測 27 alloc/req から
2 alloc/req への削減は、#579 起点の効果見込み「+5〜10%」の根拠として妥当なオーダーで
ある（alloc 回数ベースで 1 桁近い削減。実際の RPS 改善率は malloc 実装・OS・並行度に
依存するため、確定値は Phase 3 実装後に #593 の専有ベンチで再測定する）。

## 6. パーサ契約・fuzz への影響

### 6.1 `parse_request_head` の戻り値契約変更点

- 戻り値 `ParseOutcome::Complete { head: RequestHead, consumed: usize }` の型自体は
  不変。`RequestHead` の内部構造のみ変更する（3.2 節）
- `method()` / `target()` アクセサ追加、`pub method: String` / `pub target: String`
  フィールドの削除（breaking）。**`pub version: HttpVersion` は非公開化しない**
  （3.2 節。breaking change の対象は `method` / `target` の 2 フィールドのみ）
- `buf: Box<str>` への UTF-8 検証（3.2 節・6.3 節）が失敗しうる新しい失敗点として
  追加されるため、既存の `ParseError`（型は不変）に検証失敗を委譲する。呼び出し元
  ・エラー型のバリアント追加要否は #591 実装時に確定するが、`.unwrap()` /
  `.expect()` によるパニックは選択肢に含めない（6.3 節）

### 6.2 不変条件として維持するもの（後退させない）

Phase 3 実装（#591 が主に担当）は以下を維持することを受け入れ基準の前提とする
（8 節・OWASP 観点は 9 節）。

- **UTF-8 検証**: 現状の `String::from_utf8(...)` による厳密検証と同等の保証を、
  `buf: Box<str>` 構築時（ヘッド部コピー直後）の `str::from_utf8(&raw_bytes)` **1 回**
  で維持する（6.3 節）。検証失敗は `Result` で呼び出し元へ伝播し、`.unwrap()` /
  `.expect()` は使用しない。構築後は `buf` が妥当な `&str` であることが型で保証される
  ため、`method()` / `target()` アクセサ側での再検証は不要（かつ `unsafe { str::
  from_utf8_unchecked(...) }` も使用しない）
- **tchar / 制御文字拒否**: `is_tchar` / `is_forbidden_ctl`（request.rs）による
  method・ヘッダ名の token 検証、値の制御文字拒否ロジックはバイトスライス上でそのまま
  動作し、Range 化による影響を受けない
- **obs-fold 拒否**: `split_by_crlf` による行分割方式（bare LF/CR の非分割）は
  Range 化と無関係に維持される
- **DoS 上限**: `MAX_HEADER_BYTES`（16 KiB）・`MAX_HEADER_COUNT`（100）は不変。
  `buf: Box<str>` は `MAX_HEADER_BYTES` 以下に有界化されており、`headers` の
  `Vec<(Range, Range)>` も `MAX_HEADER_COUNT` で有界（Range 自体は `usize` 2 個分の
  スタックサイズであり、String のヒープ確保がなくなる分メモリ効率はむしろ向上する）
- **`consumed` 境界**: `header_end + TERMINATOR.len()` の算出ロジックは不変。
  リクエストスマグリング対策（`\r\n\r\n` 終端のみでの完了判定、パイプライン残余は
  次回呼び出しへ持ち越し）に変更を加えない

### 6.3 `unsafe` 不使用・`.expect()` 不使用の方針

案 B は `Box<str>` へのヘッド部コピー・構築時 1 回の UTF-8 検証・Range 切り出しのみで
実装可能であり、`from_utf8_unchecked` 等の `unsafe` を要求しない。`.claude/rules/
coding-rust.md`（`unsafe` 最小限方針）・`docs/design/unsafe-deny-lints.md`
（`cargo geiger` ラチェット）と整合させるため、**Phase 3 実装は `unsafe` を使わずに
実現することを採用条件とする**。

さらに `.claude/rules/coding-rust.md`「`.unwrap()` / `.expect()` はライブラリコードで
避け、`Result` / `?` でエラーを伝播する」方針に従い、UTF-8 検証は次の 2 段構成とする
（Codex レビュー #600 指摘 2 対応。当初案の「アクセサ側で `str::from_utf8(...)
.expect(...)`」は不採用とし、構築時 1 回検証 + 非 fallible アクセサへ改める）。

1. **構築時（fallible）**: ヘッド部バイト列を `buf: Box<str>` へコピーする際、
   `str::from_utf8(&raw_bytes)` の検証結果を `Result` として扱い、失敗時は
   `parse_request_head` の既存エラー型（型は不変、6.1 節）で呼び出し元へ伝播する。
   外部入力（リクエストバイト列）に対する検証はこの 1 箇所に閉じる
2. **アクセサ（非 fallible）**: `method()` / `target()` / `header()` は検証済みの
   `buf: Box<str>` から `Range<usize>` でバイト範囲インデックスするのみで、
   `str::from_utf8` の再検証・`.expect()` を行わない。この範囲インデックスが UTF-8
   文字境界からずれて panic することがないのは、Range の生成元（method・target・
   ヘッダ name/value の境界はいずれも tchar 検証・`:`・空白・CRLF 等 ASCII バイト
   （`0x00`〜`0x7F`）で区切られる、request.rs のパース仕様）と UTF-8 の性質
   （マルチバイト文字の継続バイトは常に `0x80` 以上で ASCII バイトと重複しない）
   から導かれる不変条件であり、外部入力の形状に依存しない。この不変条件は #591 の
   受け入れ基準（8 節）でテストにより担保する

アクセサでの UTF-8 再検証を構築時 1 回へ集約したことで、N 本のヘッダに対する
追加走査コストも構築時の 1 回のみに削減される（5 節の alloc 削減効果に加え、
CPU コストの面でも従来の「アクセサ呼び出しのたびに再検証」案より有利）。

### 6.4 fuzz target への影響

- `crates/http/fuzz/fuzz_targets/parse_request_head.rs`: `parse_request_head` の
  入力・エラー型は不変のため、fuzz ハーネス自体の変更は不要。ビルドは
  `RequestHead` の内部構造変更に伴い `crates/http` の再コンパイルが必要なだけで、
  fuzz target のソース変更は想定しない
- `crates/http/fuzz/fuzz_targets/head_semantics.rs`: `head.method` / `head.target`
  の直接参照箇所があれば `method()` / `target()` アクセサへの置換が必要
  （2 節の一括 breaking 方針に従う）
- 両 fuzz target とも #591 の受け入れ基準「fuzz target が新契約でビルド・短時間実行で
  問題なし」で確認する

## 7. 採用案と不採用案の根拠まとめ

| 軸 | 採用 | 不採用 | 主な理由 |
|------|------|------|------|
| ライフタイム設計 | 案 B（所有バッファ + Range） | 案 A（借用 + Range）、案 C（Cow） | 案 A/C は `RequestHead<'buf>` ライフタイムが 4 拡張点 trait・`dyn Middleware` 等の trait object 化に波及し、`RecvBuffer` の consume/コンパクション契約とも衝突する（3.1・3.3 節） |
| 移行方式 | (a) 一括 breaking | (b) 2 段階アクセサ先行、(c) Cow ハイブリッド | (b) は効果ゼロの段階を挟むだけで breaking 自体は避けられず二度手間。(c) は 3.3 節で不採用の案 C を前提とするため不採用（4 節） |
| `unsafe` 使用 | 不使用 | `from_utf8_unchecked` によるチェック省略 | pay-for-what-you-use・security 方針（`unsafe` 最小限・cargo geiger ラチェット）を優先し、性能より安全性を優先する安全側判断（6.3 節） |

判断がつかない軸（例: バージョン番号を 0.4.0 とするか 0.5.0 の一部として他の変更と
まとめるか）は Phase 3 実装 PR 側での確定に委ね、本文書では pre-1.0 の `y` を 1 つ
上げるという規則のみを確定させる（安全側判断、`versioning-policy.md` 2 節と整合）。

## 8. Phase 3 実装 issue（#590〜#593）の受け入れ基準確定

本節は #590〜#593 の既存受け入れ基準を置き換えず、本設計文書の採用案（3.2・4・6 節）
に沿って**具体化**する（マージ後に `gh issue edit` で各 issue 本文へ反映する）。

### #591（`crates/http` 内部構造変更）

- `RequestHead` を 3.2 節の構造（`buf: Box<str>` + `method`/`target`: `Range<usize>` +
  `headers: Vec<(Range<usize>, Range<usize>)>` + `pub version: HttpVersion` は
  引き続き公開フィールドのまま）へ変更し、`method()` / `target()` アクセサ
  （`&str` 返却）を追加、`pub method: String` / `pub target: String` フィールドを
  削除する。**`pub version: HttpVersion` は削除・非公開化しない**（3.2 節。
  Codex レビュー #600 指摘 1 対応）
- 6.2 節の不変条件（UTF-8 検証・tchar/制御文字拒否・obs-fold 拒否・DoS 上限・
  `consumed` 境界）を後退させないことをテストで担保する（既存テストの意味的等価な
  移植 + 新規: 不正 UTF-8 境界ケースの追加）
- UTF-8 検証は `buf: Box<str>` 構築時の 1 回に閉じ、`method()` / `target()` /
  `header()` アクセサは `.unwrap()` / `.expect()` を使わない非 fallible な実装と
  する（6.3 節。Codex レビュー #600 指摘 2 対応）。構築時検証の失敗が既存
  `ParseError`（型不変）へ正しく伝播することをテストで担保する
- alloc カウンタ（5 節の手法）または同等のテストで、ヘッダ本数 N に対して
  alloc 回数が定数（N に非依存）であることを確認する
- `unsafe` を使用しない（6.3 節）
- fuzz target 2 種がビルド・短時間実行で問題ないことを確認する（6.4 節）

### #592（core/routes/plugin 追随）

- 2 節の表で列挙した全参照箇所のうち、`head.method` / `head.target` の直接フィールド
  アクセスを `head.method()` / `head.target()` アクセサ呼び出しへ機械的に置換する。
  `head.version` は `pub` フィールドのまま維持されるため置換対象に含まれない
  （Codex レビュー #600 指摘 1 で列挙された `crates/plugin-websocket/src/
  handshake.rs:60`・`crates/http/src/body.rs:206`・`crates/http/src/request.rs:51,
  339,519,608,637`・`crates/http/src/connection.rs:133`・`crates/http/tests/
  http_flow.rs:47,96,138,152`・`crates/core/src/server.rs:2727` の `head.version`
  参照は無変更のまま動作することを実装時に確認する）
- 4 拡張点 trait（`Middleware` / `UpgradeHandler` / `RequestGate` / `Interceptor`）・
  `Handler::handle` / `handle_streaming` のシグネチャは 2.1 節の分析どおり**無変更**
  であることを維持する（万一シグネチャ変更が必要になった場合は設計文書の前提が崩れる
  ため、実装前に本文書の再検討へエスカレーションする）
- CHANGELOG に BREAKING CHANGE セクションを追加し、`pub method`/`pub target` →
  `method()`/`target()` への移行手順を明記する（`pub version` は不変である旨も
  誤解防止のため明記する。#486 の記載パターンに倣う）
- `.standalone-crates-io-skip`（理由: 「#590 系 breaking change、crates.io 未再公開の
  新 API 依存」）を `templates/app` / 対象 `examples/*` に配置する

### #593（検証・効果測定）

- 全 feature 構成（なし・個別・全）でビルド・テスト通過
- fuzz smoke 実行で問題なし
- `benches/bench-accept-exclusive.sh`（専有実行枠）で前後比較を実施し、
  `benches/reports/` に記録する。5 節の見込み（+5〜10%）に対する実測値との差異が
  あれば原因分析を添付する（未達自体は完遂判定を妨げないが、原因の記録を必須とする
  ── 実装計画の完了条件どおり）
- REQ-1/NFR-1 の既存受け入れ基準（`docs/acceptance/`）に対して非退行であることを
  確認する

## 9. セキュリティ考慮事項（OWASP Top 10 観点）

Phase 3 実装（#590〜#593）が拘束される不変条件として、`.claude/rules/security.md` の
観点別に整理する。

- **入力検証の非後退**（A03: Injection 系・A04: Insecure Design）: tchar 検証・制御
  文字拒否・obs-fold 拒否・UTF-8 厳密検証は現行実装のパース時点の判定ロジックを
  そのまま流用し、Range 切り出し後のアクセサでも UTF-8 検証（`str::from_utf8` の
  `Result` チェック）を省略しない。検証済みであることを「型で保証する」現行契約
  （`RequestHead` のフィールドは常に妥当な `&str` として取り出せる）を維持する
- **リソース枯渇 DoS**（A04/A05 系）: `MAX_HEADER_BYTES`（16 KiB）・
  `MAX_HEADER_COUNT`（100）の上限を変更しない。所有バッファ案（案 B）はヘッド部
  （≤16 KiB）以外のバイト列を保持しない契約とし、body 全体や `RecvBuffer` の
  未消費領域を誤って参照し続けることがないようにする（6.2 節）
- **リクエストスマグリング**（A04 系）: `consumed` 境界の算出（`\r\n\r\n` 終端の
  みでの判定）・パイプライン残余の扱いを変更しない（6.2 節）。fuzz target
  （`parse_request_head.rs`・`head_semantics.rs`）による回帰検証を Phase 3 で維持する
- **バッファ再利用による情報漏洩**（A01: Broken Access Control 系、旧リクエストの
  取り違え）: `RecvBuffer` は接続単位で再利用され、`consume` はカーソル前進のみで
  実バイトを消去しない（`buffer.rs`）。案 B は head パース完了時点でヘッド部を
  `RequestHead::buf`（別メモリ領域）へコピーして所有権を分離するため、その後
  `RecvBuffer` 側が次リクエストの読み取りで同じ領域を上書きしても
  `RequestHead::buf` には影響しない（案 A の借用方式で懸念された「Range ずれ・
  use-after-consume によるバイト列露出」は、案 B ではコピーによる所有権分離が
  型レベルで遮断する）
- **`unsafe` 不使用**（メモリ安全性）: 6.3 節のとおり、ゼロコピー化を `unsafe`
  （`from_utf8_unchecked` 等）なしで実現することを採用条件とする。`cargo geiger`
  ラチェット（`docs/design/unsafe-deny-lints.md`）に抵触しない
- **文書自体の安全性**: 本文書は攻撃再現手順・具体的なエクスプロイトコードを含まない
  （feasibility-guardrail「明確な拒否」章と同一原則）。シークレット・環境固有情報も
  含まない

## 10. スコープ外

- **P5 per-core 最適化**: 性能改善ツリー #579 の別イシュー（#589 想定）のスコープ。
  本文書では扱わない
- **Phase 3 の実コード変更**: 本イシューの成果物は設計文書のみ。#590〜#593 の実装は
  ユーザー承認後に着手する（feasibility-guardrail 準拠、0 節）
- **`webrtc-rs` バージョン戦略との混同回避**: `docs/design/webrtc-rs-version-strategy.md`
  はフレームワーク本体の semver とは別軸（`versioning-policy.md` 0 節と同一の注意書き）
- **HTTP/2 対応**: 本フレームワークは v1 スコープで HTTP/1.1・HTTP/1.0 のみを扱う
  （`docs/design/v1-scope-tls-multipart.md` と同様、HTTP/2 は範囲外）

## 11. extension-closure-gate 理由記載（`crates/http/tests/alloc_count.rs`）

`docs/design/dependency-graph-contract.md` 4 節の運用に基づく、Phase 3 実装 PR（#602）
での E（閉包違反候補）ファイルの理由記載。

1. **対象コミット/PR**: PR #602（イシュー #591、性能改善ツリー #579 Phase 3）
2. **E ファイルパス**: `crates/http/tests/alloc_count.rs`
3. **閉じない理由**: `extension-closure-check.sh` の分類規則は C（テスト）を
   `crates/core/tests/**`・`crates/plugin-*/tests/**` のみに限定しており、中間層
   クレート `crates/http` 配下のテスト（`crates/http/tests/**`）は走査対象に
   含めていない（4.9 節「本節の運用上のギャップ」と同種）。本ファイルは
   `RequestHead` の内部表現変更（本文書 3〜5 節）に伴う alloc プロファイル退行
   検知用の常設テストであり、`crates/http` 配下に新設したため機械的に E 判定と
   なった
4. **正当性根拠**: 本ファイルは `parse_request_head` の 1 リクエストあたり
   ヒープアロケーション回数を計測するテストダブル（`GlobalAlloc` を実装する
   dev-dependency `stats_alloc` を使用、本クレート自体には `unsafe` を導入しない。
   PR #602 レビュー指摘 P0 対応で自前の `unsafe impl GlobalAlloc` から置き換え済み）
   であり、3 拡張点 trait（`Middleware`/`UpgradeHandler`/`RequestGate`）・
   `try_intercept` 固定シームの契約・シグネチャ・実装ロジックはいずれも変更しない。
   依存方向（`server → routes → http::*`、1 節）にも影響しない。プラグイン実装
   ロジックの拡張点外への漏出ではなく、`extension-closure-check.sh` の C 分類が
   中間層クレートのテストディレクトリを想定していないことに起因する運用上の
   ギャップである。分類規則自体の見直し（`crates/http/tests/**`・
   `crates/routes/tests/**` の C への追加）は 4.9 節と同一の別 Issue 対象として
   据え置く（`.claude/rules/out-of-scope-tracking.md`）

## 12. extension-closure-gate 理由記載（`crates/http/tests/dos_header_count_capacity.rs`）

`docs/design/dependency-graph-contract.md` 4 節の運用に基づく、PR #602 での
E（閉包違反候補）ファイルの理由記載（11 節と同一 PR・同一運用ギャップに属する
2 件目）。

1. **対象コミット/PR**: PR #602（イシュー #591、性能改善ツリー #579 Phase 3）
2. **E ファイルパス**: `crates/http/tests/dos_header_count_capacity.rs`
3. **閉じない理由**: 11 節と同一。`extension-closure-check.sh` の分類規則は
   C（テスト）を `crates/core/tests/**`・`crates/plugin-*/tests/**` のみに
   限定しており、中間層クレート `crates/http` 配下のテスト
   （`crates/http/tests/**`）は走査対象に含めていない（4.9 節「本節の運用上の
   ギャップ」と同種）。本ファイルは `RequestHead` の内部表現変更（本文書 3〜5 節）
   に伴い `crates/http` 配下に新設したため機械的に E 判定となった
4. **正当性根拠**: 本ファイルは `parse_request_head` がヘッダ本数の事前確保
   （`Vec::with_capacity`）を `MAX_HEADER_COUNT`（100）以下にクランプすることを
   固定する常設テスト（PR #602 レビュー指摘 P0 対応。`header_count` は最大
   16 KiB の未信頼なヘッダ部から得られる値であり、上限検査前に
   `Vec::with_capacity` へそのまま渡すと並行接続によるメモリ枯渇 DoS を増幅
   しうるため、クランプ済みであることをアロケーションバイト数の実測で検証する）
   であり、`alloc_count.rs`（11 節）と同じく計測対象は `parse_request_head`
   （`crates/http`）の既存実装のみで、3 拡張点 trait（`Middleware`/
   `UpgradeHandler`/`RequestGate`）・`try_intercept` 固定シームの契約・
   シグネチャ・実装ロジックはいずれも変更しない。依存方向（`server → routes →
   http::*`、1 節）にも影響しない。プラグイン実装ロジックの拡張点外への漏出
   ではなく、`extension-closure-check.sh` の C 分類が中間層クレートのテスト
   ディレクトリを想定していないことに起因する運用上のギャップである
   （分類規則自体の見直しは 11 節と同一で 4.9 節と同一の別 Issue 対象として
   据え置く、`.claude/rules/out-of-scope-tracking.md`）

## 13. #592 実施記録（core/routes/plugin 追随・移行手順記載）

イシュー [#592](https://github.com/Fandhe-AI/fandhe-backend/issues/592)
「RequestHead 変更への core/routes/plugin 追随と移行手順記載」の実施記録。前提として
#591 実装（PR [#602](https://github.com/Fandhe-AI/fandhe-backend/pull/602)）が
workspace 内全クレートの追随・`CHANGELOG.md` の移行手順記載・`CLAUDE.md` の反映を
先行実施済みであり、#592 は残存事項の棚卸しと受け入れ基準の検証確定に絞って実施した。

### 13.1 workspace 全クレートの参照置換

2 節の表で列挙した全参照箇所のうち `head.method` / `head.target` の直接フィールド
アクセスは PR #602 で置換済み（本イシューでの追加コード変更は 0 件）。本イシューでの
追加調査で、置換漏れとしてコメント・doc 内の表記のみ 2 件検出し修正した:

- `crates/core/src/plugin.rs`（363 行付近）: コメント内 `` `head.target` `` 表記を
  `` `head.target()` `` へ更新（コメントのみ、挙動変更なし）
- `docs/design/plugin-boundary.md`（517 行付近）: `head.method == "GET" &&
  head.target == "/openapi.json"` の記述を `head.method() == "GET" &&
  head.target() == "/openapi.json"` へ更新（実コードは #602 で既にアクセサ化済みで、
  doc の記述だけが旧表記のまま残っていた）

### 13.2 4 拡張点 trait のシグネチャ不変確認

`Middleware` / `UpgradeHandler` / `RequestGate` / `Interceptor` の 4 拡張点 trait、
および `Handler::handle` / `Handler::handle_streaming` は、いずれも引数が
`&RequestHead`（共有参照）のままでシグネチャ変更がないことを実装ソース
（`crates/core/src/extension.rs`・`crates/core/src/interceptor.rs`）で確認した。
これらの trait を実装するプラグイン・利用者コード側の移行は、実装本体内で
`head.method` / `head.target` を直接参照している箇所のみをアクセサ呼び出しへ
書き換えれば完了する。本確認結果は `CHANGELOG.md` の `[Unreleased]` エントリへ
正式に追記した。

### 13.3 全 feature 構成ビルド検証

`bash scripts/pay-for-what-you-use-check.sh` を実行し、(a) プラグイン feature 列挙・
(b) 個別 feature 構成での `cargo tree` 検証（他プラグイン混入なし）・(c) `cargo
geiger`（無効構成で対象 unsafe 計上ゼロ）・(d) バイナリサイズ差分・(e) 無効構成/個別
構成/`--all-features` の全構成ビルド、すべて PASS を確認した。加えて
`cargo clippy -p fandhe-backend-core --all-targets --no-default-features -- -D
warnings` もクリーンであることを確認した（CI clippy ジョブと同一コマンド）。

### 13.4 standalone workspace 5 件の検証と `.standalone-crates-io-skip` 配置要否判定

`templates/app`・`examples/with-cors`・`examples/with-graphql`・
`examples/with-websocket`・`examples/with-interceptor` の 5 standalone workspace
（root workspace 非メンバー）について、次の 2 段階で検証した。

1. **ローカル HEAD（path 依存、新 API）でのビルド・テスト**: 5 クレートすべてで
   `cargo build` / `cargo test` が成功（各クレートのテストスイート全件 PASS）。
   事前の `git grep` 全量調査で、5 クレートの `RequestHead` 参照は
   `parse_request_head` / `ParseOutcome` 分配束縛 / `head.path()` / `head.query()`
   のみで、廃止された `pub method` / `pub target` フィールドへの直接参照は 0 件
   だったことと整合する結果
2. **crates.io 公開版（0.3.0）のみでのビルド・テスト（受け入れ基準 4）**:
   `bash scripts/standalone-crates-io-check.sh`（ネットワーク要）を実行し、
   `== 集計: PASS 5 / SKIP 0 / FAIL 0 / 全 5 クレート ==` を確認した。5 クレート
   すべてが crates.io 公開版 0.3.0 のみで build/test 通過したため、
   `.standalone-crates-io-skip` マーカーは**配置不要**と判定した（`RequestHead` の
   フィールド直接アクセスへの依存が元々なく、0.3.0・HEAD いずれの API でも
   コンパイル可能なコードだったため）

### 13.5 バージョン bump 見送り判断

本イシューでは 13 クレートの lockstep バージョンバンプ（0.3.0 → 0.4.0 候補、0 節
参照）を実施しない。確立済み運用（v0.2.0 = #437、v0.3.0 = #509/#506）では、lockstep
バンプは breaking change マージ時点ではなく**リリース準備イシュー**（`docs/design/
crates-io-release.md` 7.1〜7.3 節）で実施する。#592 の受け入れ基準にバンプは含まれず、
今バンプすると standalone workspace 5 件の依存 `version` を未公開の "0.4.0" へ揃える
必要が生じ、`standalone-crates-io-check.sh` の「全クレート SKIP かつ PASS 0 件は
exit 1」という fail-closed 判定（週次常設検証 `standalone-crates-io.yml`）を恒常
FAIL させてしまう。次回 crates.io 再公開時に別途起票するリリース準備イシューで、
7.2 節の手順に従って実施する。

### 13.6 受け入れ基準との対応まとめ

| 受け入れ基準 | 結果 |
|------|------|
| workspace 全クレート + templates/app + examples/* がビルド・テスト通過 | PASS（13.1〜13.4） |
| 全 feature 構成（なし・個別・全）でビルド確認 | PASS（13.3） |
| CHANGELOG に BREAKING CHANGE と移行手順を記載 | PASS（#602 で先行記載済み、13.2 の追補で拡張点シグネチャ不変を明記） |
| `.standalone-crates-io-skip` を必要に応じて配置 | 配置不要と判定・根拠記録済み（13.4） |

## 14. #593 実施記録（検証・効果測定）

イシュー [#593](https://github.com/Fandhe-AI/fandhe-backend/issues/593)
「P1 適用後の全構成テストと専有ベンチ効果検証」の実施記録。詳細は
`benches/reports/issue593-p1-zero-copy-bench.md` を参照し、本節は受け入れ基準
（8 節「#593」）への対応まとめに絞る。

### 14.1 全構成テスト・fuzz smoke

- feature なし（`cargo test --workspace`）: 85 個の `test result: ok` ブロック
  すべて成功
- 全 feature（`cargo test --workspace --all-features`）: 88 個の
  `test result: ok` ブロックすべて成功
- 個別 feature 9 種（webrtc-proxy / webrtc / websocket / graphql / tracing /
  openapi / cors / compression / static）: 全 feature で exit 0（PASS）
- pay-for-what-you-use（`scripts/pay-for-what-you-use-check.sh`）: PASS
  （プラグイン feature 列挙・`cargo tree`・`cargo geiger`・バイナリサイズ・
  全構成ビルドの 5 段すべて成功）
- lint（`cargo fmt --check` / `cargo clippy --all-features` /
  `cargo clippy --no-default-features -p fandhe-backend-core`）: クリーン
- fuzz smoke（`scripts/fuzz.sh`、7 target × 60 秒、`head_semantics` /
  `parse_request_head` が P1 変更の直接対象）: 全 target 正常終了・クラッシュなし

### 14.2 専有ベンチ前後比較

`benches/bench-accept-exclusive.sh` で before（`655b150`、P1 適用直前）/
after（`1aeda06`、origin/main 先端）を比較した。

- **before**: 全 15 指標 PASS
- **after**: 1 回目試行 + `FAIL_RETRIES=1` の自動再試行 2 回（計 3 回）を
  実施したが、いずれも p95 レイテンシ 1 件のみが基準をわずかに超過し機械判定は
  FAIL（3 回とも異なるエンドポイントで超過、p99・RPS・RSS・バイナリサイズ・
  起動時間の 12 指標は 3 回とも一貫して PASS）
- 診断根拠（詳細は `issue593-p1-zero-copy-bench.md` 5.6 節）により、これは
  `benches/reports/task-2.4-plugin-accept.md`（#260）で報告済みの host
  contention ノイズと同型の事象であり、P1 固有の性能退行ではないと判断した

### 14.3 効果見込みとの差異・原因分析

設計文書 5.3 節の見込み（+5〜10%）に対し、host contention ノイズの重畳により
RPS ベースの単純比較で明確な改善率を確認できなかった。原因分析（詳細は
`issue593-p1-zero-copy-bench.md` 5.7 節）:

1. host contention ノイズが RPS・p95 の両方に重畳し、本イシューの計測環境
   （並列 issue 実装ワークフロー稼働下の共有ホスト）では S/N 比が不十分だった
2. P1 の効果は alloc 回数の削減であり、`core-bench` の 4 エンドポイントは
   応答本文が小さく、ヘッダ解析の alloc 削減が accept〜応答送出パイプライン
   全体の RPS に占める寄与が相対的に小さい可能性がある
3. バイナリサイズ比（0.7391、before/after 共通）は #591/#592（P1）と
   #595〜#599（Phase 1 改善、before 基準コミットに既に含まれる）の累積効果

alloc 回数ベースでの効果自体は #591/#592 実装時点で
`crates/http/tests/alloc_count.rs`（N=1/N=30 の alloc 差分を直接計測する
常設テスト）により機械検証済みであり、本節はその実測値の RPS への反映度を
別途確認する試みとして位置づける。

### 14.4 受け入れ基準対応表

| 受け入れ基準 | 結果 |
|------|------|
| 全 feature 構成（なし・個別・全）でビルド・テスト通過 | PASS（14.1） |
| fuzz smoke 実行で問題なし | PASS（14.1、7 target 全正常終了） |
| 専有ベンチで前後比較を実施し `benches/reports/` に記録。見込みとの差異があれば原因分析を添付 | 実施・記録済み（14.2・14.3、`issue593-p1-zero-copy-bench.md`）。機械判定は host contention ノイズにより FAIL だが、原因分析を添付した上で診断に基づき非退行と結論（未達自体は完遂判定を妨げない） |
| REQ-1/NFR-1 の既存受け入れ基準に非退行 | 診断根拠に基づき非退行と判断（`issue593-p1-zero-copy-bench.md` 6 節） |
