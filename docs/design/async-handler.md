# async ハンドラ対応の設計判断（イシュー #314）

`docs/spec/04-requirements.md` REQ-1 の最小コア・拡張点契約と、コアループ自体は
既に async である事実（`crates/core/src/server.rs` の接続受理・リクエストループ）
を踏まえ、ハンドラ契約を async 化する際の設計判断を記録する。本文書は **設計判断の
記録のみ**であり、実装は本文書 6 節の分解方針に従って後続イシューで行う。

## 1. 結論

**候補 (c) 「拡張点は同期のまま、ハンドラのみ async 化」を採用する。**
`Handler::handle` / `RouteHandler` / `ParamRouteHandler` を
`Pin<Box<dyn Future<Output = Response> + Send>>` を返す boxed-future 契約へ移行し、
既存の同期クロージャは `std::future::ready` 相当のアダプタで受け入れて後方互換を
確保する。3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`、
`crates/core/src/extension.rs`）の同期契約・`_assert_object_safe` は変更しない。
型消去には新規依存を追加せず、`std::future::Future` + `Pin<Box<dyn ...>>` を
手書きする（`crates/plugin-websocket`・`crates/plugin-graphql` の先例、5 節参照）。

## 2. 現状の同期契約の棚卸し

ハンドラ呼び出し点は現在すべて同期である。

| シンボル | 定義 | シグネチャ |
|---------|------|-----------|
| `RouteHandler` | `crates/routes/src/lib.rs:83` | `Box<dyn Fn(&RequestHead, &[u8]) -> Response + Send + Sync>` |
| `ParamRouteHandler` | `crates/routes/src/lib.rs:92-93` | 上記 + `PathParams` 引数（同型） |
| `Handler` trait | `crates/core/src/server.rs:161-164` | `fn handle(&self, head: &RequestHead, body: &[u8]) -> Response` |
| 呼び出し箇所 | `crates/core/src/server.rs:996` | `handler.handle(&request.head, &request.body)`（`handle_connection` 内、同期呼び出し） |

一方、コアループ本体（`handle_connection`）自体は既に `async fn` であり、
`crates/core/src/plugin.rs` のプラグインインターセプト経路（`try_intercept`
`try_handle_upgrade`、いずれも `async fn`）は既に async 化済みである。**async 化が
未達なのはユーザー定義ハンドラの呼び出し点のみ**であり、これがハンドラから
sqlx 等の非同期 DB クライアントを構造的に使えない分水嶺になっている。

3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`、
`crates/core/src/extension.rs:29-37`）は「dyn 互換維持のため意図的に同期」と
既に規約明文化されており（`_assert_object_safe` テストでコンパイル時検証）、
本判断はこの契約を変更しない。ハンドラは拡張点 3 種とは別の呼び出し経路
（`Handler::handle`）であり、拡張点契約への影響なしに単独で async 化できる。

## 3. 候補比較表

比較軸: dyn 互換性・性能影響（REQ-1 の axum 級維持）・後方互換・拡張点契約への
影響・実装コスト。

| 候補 | dyn 互換性 | 性能影響 | 後方互換 | 拡張点契約への影響 | 実装コスト |
|------|-----------|---------|---------|------------------|-----------|
| (a) async trait 化（`Handler::handle` を丸ごと `Pin<Box<dyn Future<...>>>` 返却へ変更） | 維持できる（boxed future は object safe） | リクエスト毎 Box アロケーション 1 回 | **破壊的**。全既存ハンドラ実装（同期クロージャ含む）に変更を強制 | なし（`Handler` は拡張点 3 種と別 trait） | 中（型は単純だが呼び出し側の書き換えが広範） |
| (b) async ルータ別型併設（`AsyncRouter` / `AsyncHandler` を別型で追加） | 維持できる | 同期経路は無変化、async 経路のみ (a) と同等コスト | **完全**。既存同期系は無変更 | なし | 高（ルーティング・ディスパッチ・doc test を二重に保守） |
| (c) 拡張点は同期のまま、ハンドラのみ async 化（**採用**） | 維持できる | (a) と同じ Box アロケーション 1 回。呼び出し点が 1 箇所（`server.rs:996`）に閉じるため影響範囲が最小 | アダプタ（同期クロージャ → `std::future::ready` でラップ）で吸収。**呼び出し側 API は非破壊**、`Fn` トレイト境界の型だけ変わる | なし | 低〜中（型定義変更 + アダプタ + 呼び出し点 1 箇所の書き換え） |
| (d) 現状維持 + `spawn_blocking` ガイダンス（対照案） | 該当なし | ブロッキングスレッドプール圧迫。イシュー背景に明記のとおりアンチパターン | 完全（変更なし） | なし | 実装コストゼロだが根本課題を解決しない |

**(d) を棄却する根拠**: `block_in_place` + `block_on` はブロッキングスレッド
プールの圧迫であり、`.claude/rules/coding-rust.md`「Tokio 上でブロッキング処理を
await スレッドで実行しない（`spawn_blocking` を使う）」という既存規約の精神に反する
回避策に過ぎない。非同期 DB クライアントを本来の非同期経路で使えないという
根本課題を解決しない。

**(b) を棄却する根拠**: 後方互換は完全だが、ルーティング・ディスパッチ・
doc test・examples を同期/非同期の二系統で保守するコストが継続的にかかる。
既存ハンドラの大半が最終的に async 化を要する見込み（sqlx 等の利用が前提）である
以上、二重管理の期間が長期化しやすく、AI ファースト保守性（CLAUDE.md の核 2 原則）
の観点でも複雑度が高い。

**(a) を棄却する根拠**: (c) と性能特性・dyn 互換性は同等でありながら、既存の
同期クロージャ登録コード（examples・doc test 含む）すべてに破壊的変更を強制する。
(c) のアダプタ方式で同じ結果を非破壊的に達成できるため、(a) を選ぶ理由がない。

**(c) を採用する根拠**: コアループが既に async であるため呼び出し側の変更が
`server.rs:996` の 1 箇所に閉じる。3 拡張点の契約（`extension.rs:29-37`）を
一切変更しない。既存の同期ハンドラ登録コードはアダプタで無変更のまま動作する。

## 4. 性能影響予測とベンチ検証方法

### 4.1 予測

- **リクエスト毎の追加コスト**: `Pin<Box<dyn Future<Output = Response> + Send>>`
  を返す契約に変更すると、ハンドラ呼び出し毎に Box アロケーション 1 回と、
  対応する非同期状態機械の poll オーバーヘッドが追加される。同期ハンドラを
  `std::future::ready` でラップした場合も、返却値自体は即座に `Poll::Ready` に
  なるため poll は 1 回で完了し、追加コストは実質 Box アロケーション 1 回に限定
  される。
- **REQ-1 の性能閾値との突き合わせ**: `benches/bench-accept.sh` 冒頭コメントに
  明記されたとおり REQ-1・NFR-1・NFR-2 の基準は「RPS が axum 比 90% 以上、p95・p99
  レイテンシが axum 比 110% 以内」である。axum 自体が `Handler` を boxed future
  ベースで実装しており同種のオーバーヘッドを既に負っていることから、同オーダーの
  コスト（リクエスト毎ヒープ確保 1 回）であれば基準内に収まる可能性が高いと見立てる。
  ただし実測値による裏付けは行っていないため、本文書の見立ては「実装イシューでの
  ベンチ実測を必須とする」前提を崩すものではない。
- **リスクが高い経路**: hot path（`GET /health` 相当の単純応答）では相対的な
  オーバーヘッド比率が最も大きくなりうるため、ベンチシナリオに単純応答エンドポイント
  を必ず含める。

### 4.2 検証方法

- 実装イシューにおいて、`benches/bench-accept-exclusive.sh`（専有計測、
  イシュー #178・#260 の規約に従う相互排他・静穏確認込みの wrapper）で変更前後の
  RPS・p95・p99 を比較し、REQ-1 の閾値（RPS 比 0.90 以上・p95/p99 比 1.10 以内）を
  満たすことを回帰判定として実施する。
- 週次 `bench-schedule.yml`（イシュー #285）による継続監視で、マージ後の性能退行を
  検知する（`benches/bench-accept-exclusive.sh` を週次専有実行枠で自動実行する
  既存の仕組みをそのまま利用でき、本変更のための新規 CI 追加は不要）。
- FAIL が出た場合は `FAIL_RETRIES`（既定 0）による単発 FAIL の限定再試行を許容しつつ、
  再現する劣化は実装を見直す（アダプタ経路の追加コストが許容できない場合は、
  同期ハンドラ専用の高速経路を型分岐で残すことを再検討する。ただし本文書の時点では
  そこまでの複雑化は見送り、まず (c) のシンプルな実装で計測することを優先する）。

## 5. 後方互換・移行方針

- **同期クロージャの受け入れ維持**: `Router::route` 等の既存公開シグネチャは
  ジェネリクス境界を「戻り値が `Response` の `Fn`」から「戻り値が
  `Future<Output = Response>` の `Fn`」へ広げ、既存の同期クロージャは
  `move |head, body| std::future::ready(f(head, body))` 相当のアダプタで内部的に
  ラップして登録する。呼び出し元のコードは変更不要（doc test・examples が現状のまま
  コンパイル・パスすることを実装イシューの受け入れ条件に含める）。
- **型消去は新規依存を追加しない**: `Pin<Box<dyn Future<Output = Response> + Send>>`
  は `std::future` + `std::pin` + `std::boxed` のみで表現でき、`futures-util` の
  `BoxFuture` 型エイリアスすら不要である（`futures-util` はコア
  `crates/core`・`crates/routes` の現行依存に含まれない。pay-for-what-you-use
  上、コアへ新規依存を持ち込まない選択を優先する）。この点は
  `crates/plugin-websocket`（`WsMessageHandler::on_message` が既存依存
  `futures-util` の `BoxFuture` で型消去、`docs/design/plugin-boundary.md` 5.5.1 節）
  ・`crates/plugin-graphql`（`BoxExecuteFn`、`crates/plugin-graphql/src/lib.rs:134`）
  という「async fn in trait を手書き boxed-future で型消去し async-trait 等の
  crate を追加しない」先例と方針を一致させたものであり、コア側は既存依存すら
  増やさずに同じパターンを踏襲する。
- **doc test・examples への影響**: `Handler` 実装・`Router::route` 登録コードの
  doc test は新シグネチャに合わせて更新が必要（型が変わるため）。実装イシューの
  スコープに examples・doc test の追随を含める（`.claude/rules/
  feature-modification.md` のドキュメント追随チェックリストに従う）。

## 6. 採用案と実装イシュー分解方針

採用案（(c)）を次の単位に分解して後続イシューへ切り出す。依存順に列挙する。

1. **core `Handler` async 化**: `crates/core/src/server.rs` の `Handler` trait を
   boxed-future 契約へ変更し、`handle_connection` の呼び出し点
   （`server.rs:996`）を `.await` へ更新する。
2. **routes ハンドラ型 async 化**: `crates/routes/src/lib.rs` の `RouteHandler` /
   `ParamRouteHandler` を boxed-future 契約へ変更し、同期クロージャ受け入れ
   アダプタ（5 節）を追加する。ステップ 1 に依存。
3. **examples・doc test 追随**: `crates/core/examples/core-bench.rs` 等の既存
   example・doc test を新シグネチャに追随させる。ステップ 1・2 に依存。
4. **ベンチ回帰判定の実施**: `benches/bench-accept-exclusive.sh` による変更前後
   比較を実施し、REQ-1 閾値充足を記録する（4.2 節の検証方法）。ステップ 1〜3 完了後。
5. **ドキュメント追随**: `AGENTS.md`・`crates/core/src/extension.rs` の doc
   comment（「同期 API 規約」の記述に「ハンドラは対象外」である旨を追記）・
   `docs/design/plugin-boundary.md` 等、拡張点契約とハンドラ契約の違いを
   誤解なく参照できるよう更新する。ステップ 1〜4 と並行可能。

各イシューは `.claude/rules/feature-modification.md` の完遂判定 3 条件
（`ci-complete` 緑・受け入れ基準充足・ドキュメント追随完了）に従う。

## 7. DoS・安全性考慮

- **長時間 pending との相互作用**: async ハンドラが長時間 `.await` で止まる場合でも、
  既存の接続生存期間上限（`crates/core/src/server.rs` の
  `Server::max_connection_lifetime`）・keep-alive 中の最大リクエスト数上限
  （`Server::max_requests_per_connection`）・読み取りタイムアウト
  （`DEFAULT_READ_TIMEOUT` = 30 秒、`server.rs:111`）は**ハンドラ呼び出し前後の
  I/O 待ちに対する上限であり、ハンドラ本体の実行時間そのものには及ばない**。
  この事実は現行の同期 `Handler::handle` でも同様（同期呼び出し中はスレッドを
  占有し続ける）であり、async 化によって新たに生じる問題ではないが、実装イシューの
  受け入れ条件として「ハンドラ実行時間そのものに対する上限は本変更のスコープ外で
  あり、必要であれば別途 `tokio::time::timeout` 等をハンドラ実装側の責務とする」
  ことを明記する（コア側にハンドラ実行タイムアウトを追加するかどうかは本文書では
  判断せず、必要性が具体化した時点で別途 out-of-scope-tracking 規約に従い
  Issue 化する）。
- **フェイルクローズ維持**: `RequestGate` の早期拒否・404/405 のデフォルト拒否
  経路はハンドラ呼び出し前の別経路（`crates/core/src/plugin.rs` の
  `try_intercept` 等、拡張点 3 種の呼び出し）であり、本変更（ハンドラ契約のみの
  async 化）による影響を受けない。3 節の比較表のとおり拡張点契約は不変であるため、
  この保証は設計上自明である。
- **panic 境界**: 同期 `Handler::handle` 呼び出し時と同様、async 化後もハンドラの
  panic がコアループ・他コネクションへ波及しない契約を維持する。boxed future の
  `.await` 中に panic した場合、Rust の async ランタイム（Tokio）は panic を
  当該タスク（接続単位で spawn されているタスク）内に閉じ込めるため、既存の
  タスク分離（接続単位 spawn）構造と整合する。実装イシューでこの契約を
  維持することをテスト（panic するハンドラを登録した統合テスト）で検証する。
- **秘密情報・攻撃手順の不記載**: 本文書はトークン・接続文字列等の秘密や実行可能な
  攻撃コードを含まない。

## 8. 再評価の条件・参照

- **Rust の async fn in trait（AFIT / RPITIT）**: 2026-07 時点で `async fn` を
  trait に直接書けるようになった（edition 2021 以降）が、`impl Trait` を返す
  ため dyn 非互換（`Box<dyn Trait>` として扱えない）という制約が残る。本フレーム
  ワークは 3 拡張点・`Handler` のいずれも `Box<dyn ...>` として保持する設計
  （`crates/core/src/server.rs` のハンドラ登録、`extension.rs` の拡張点保持）
  のため、native AFIT がそのままでは採用できない。将来 Rust が dyn 互換な
  native async trait（例: `dyn*` や関連の言語機能）を安定化した場合は、
  手書き boxed-future アダプタ（5 節）を置き換える再評価の余地がある。
- **axum の `Handler`**: axum は generics ベースの `Handler` trait と
  boxed future を組み合わせた設計を採用しており、本文書の候補 (c) と同種の
  トレードオフ（dyn 互換性のための boxed future、リクエスト毎の Box アロケーション）
  を負っている。axum が REQ-1 の性能基準比較対象であること自体が、(c) の性能
  コストが「axum 級」の許容範囲に収まりうる根拠の一つになる。
- **関連する既存規約・先例**:
  - `.claude/rules/coding-rust.md`「拡張点は 3 種 trait に集約」
    「Tokio 上でブロッキング処理を await スレッドで実行しない」
  - `crates/core/src/extension.rs:29-37`（3 拡張点の同期契約規約）
  - `docs/design/plugin-boundary.md` 5.5.1 節（`WsMessageHandler` の
    boxed-future 型消去、Issue #179）
  - `crates/plugin-graphql/src/lib.rs:134`（`BoxExecuteFn`）
  - `benches/bench-accept.sh`（REQ-1・NFR-1・NFR-2 の axum 比性能基準）
  - `docs/spec/04-requirements.md` REQ-1・REQ-2
- **関連イシュー**: #314（本イシュー、設計判断の記録）。実装イシューは
  6 節の分解方針に従い本イシューのクローズ後に起票する。
