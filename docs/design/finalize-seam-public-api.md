# `finalize_response` 系シームの公開 API 化を検討する（イシュー #462）

## 1. 背景

「レスポンス後処理型」シーム（`crates/core/src/plugin.rs` の `finalize_response`、
イシュー #305 で確立・イシュー #321 で第 2 インスタンス（圧縮）を追加）、およびその
ストリーミング応答版 `finalize_streaming_head`（イシュー #451 で追加）は、いずれも
`pub(crate)` の非公開シームであり、利用できるのは同梱プラグイン（`plugin-cors` /
`plugin-compression`）のみである。

PR #458（イシュー #451、`finalize_streaming_head` 追加）のレビュー時、「第 3 の
レスポンス後処理型プラグインを外部ユーザーが書けるようにするには公開 API 化が必要に
なる」という論点がスコープ外として切り出された。本書はイシュー #462 として、その
公開 API 化の**採否を判断し記録する**ドキュメントである。

参照:
- [`plugin-boundary.md`](./plugin-boundary.md) 5.9 節（レスポンス後処理型パターンの
  確立経緯・5.9.7 節のストリーミング適用範囲）・5.10 節（第 2 インスタンス、圧縮）
- [`interceptor-extension-point.md`](./interceptor-extension-point.md)（ユーザー向け
  公開拡張点 `Interceptor` の設計判断）
- [`dependency-graph-contract.md`](./dependency-graph-contract.md)（`finalize_response`
  を固定シームとして掲載する契約一覧）

## 2. 現状の非公開シーム 2 つの契約

| シーム | 適用対象 | 適用内容 | 除外対象 |
|--------|---------|---------|---------|
| `finalize_response` | 一括応答（`try_intercept` 応答・既定 `Handler::handle` 応答の合流点） | CORS → 圧縮の順で逐次適用（`cors`/`compression` feature 有効時のみ、`Server::cors`/`Server::compression` 登録時のみ） | `RequestGate` 拒否応答・パースエラー応答（fail-closed、`plugin-boundary.md` 5.9.4 節） |
| `finalize_streaming_head` | ストリーミング応答ヘッド（`Handler::handle_streaming` opt-in 経路、`write_streaming_response` がヘッド確定時に 1 回呼ぶ） | CORS のみ（`cors` feature 有効時のみ、`Server::cors` 登録時のみ） | 圧縮（body 全体を確定させる後処理であり chunked framing の直接書き出しループと両立できないため意図的に対象外、`plugin-boundary.md` 5.9.7 節） |

両シームとも CORS 判定条件は共通ヘルパ `apply_cors`（`cors` feature 時のみコンパイル）
に単一情報源化されており（イシュー #451）、プリフライトリクエストには適用しない
（二重付与防止、5.9.5 節）。

いずれも固定シグネチャ `fn(&Server, &RequestHead, Response) -> Response` の
`pub(crate)` 関数で、`crates/core` 内部の呼び出し箇所（`handle_connection_with_permit` /
`write_streaming_response`）からのみ呼ばれる。

## 3. `Interceptor::map_response` との棲み分け

ユーザー向けのレスポンス改変は、イシュー #420 で追加した公開 trait
`Interceptor::map_response`（`crates/core/src/interceptor.rs`）が既に提供している。
両者を比較する。

| 観点 | `finalize_response` / `finalize_streaming_head` | `Interceptor::map_response` |
|------|--------------------------------------------------|------------------------------|
| 公開範囲 | `pub(crate)`（非公開、同梱プラグイン専用） | `pub`（外部クレートから実装可能） |
| feature ゲート | `cors` / `compression` feature 前提 | なし（外部依存ゼロ、pay-for-what-you-use に整合） |
| 登録方法 | コンパイル時 `#[cfg(feature = "...")]` で `crates/core` 内に配線済み | 実行時 `Server::interceptor` で複数登録・登録順評価 |
| 適用対象（一括応答） | `try_intercept` 応答・既定 `Handler` 応答の合流点 | 同一の合流点（評価順序: `map_response` が先） |
| 適用対象（ストリーミング） | ヘッド（ステータス・`Content-Type`・追加ヘッダ）のみ、CORS のみ（イシュー #451） | ヘッド（ステータス・`Content-Type`・追加ヘッダ）のみ（イシュー #434。body は chunked framing がコアの直接書き出しループを経由するため反映されず破棄する契約、両者共通） |
| `RequestGate` 拒否応答・パースエラー応答 | 適用しない（fail-closed） | 適用しない（同一理由。`interceptor-extension-point.md` 4 節） |
| 評価順序（一括応答） | `map_response` の**後**（`docs/design/interceptor-extension-point.md` 処理フロー参照） | `finalize_response` より**前** |

「単一合流点で `try_intercept` 応答・既定 `Handler` 応答の両方に同一適用できる」という
finalize 系の設計上の利点（5.9.3 節）は、`map_response` も同型の合流点で同等に備えて
いる。ストリーミング応答への適用範囲も、`finalize_streaming_head`（CORS ヘッダ付与の
みに限定）と `map_response`（ヘッド全体、ただし body は対象外）は実質同等以上であり、
機能面での差はほぼ解消している。

## 4. ギャップ分析

上記比較から、`map_response` で表現できず finalize 系でのみ表現できるのは次の 1 点の
みである。

- **CORS ヘッダ付与後・gzip 圧縮後の最終応答への介入**（例: 圧縮後 body に対する
  応答署名・チェックサム付与、圧縮後サイズに基づくヘッダ調整）

この用途は評価順序上 `finalize_response`（`map_response` の後）でしか実現できない。
ただし、現時点でこの用途を必要とする具体的なユースケース・利用者フィードバックは
提示されていない。

## 5. 採否: 不採用

**結論: 公開 API 化は現時点で不採用とする。**

根拠:

1. **機能重複**: 2 節・3 節の比較のとおり、ユーザー向けレスポンス改変は
   `Interceptor::map_response` が既に公開提供しており、finalize 系の設計上の利点を
   実質的に包含している。
2. **非公開シームの存在理由は同梱プラグイン固有の要請**: (a) Cargo feature ゲートに
   よる pay-for-what-you-use（外部クレートは `crates/core` の `#[cfg(feature = "...")]`
   に参加できないため、公開化は必然的に実行時登録 API となり `Interceptor` と同型に
   帰着する）、(b) CORS → 圧縮の順序固定（圧縮は body を確定させる後処理のため必ず
   最後、5.10.1 節）、(c) プリフライト二重付与防止という CORS 固有判定（5.9.5 節）。
   いずれも外部プラグイン向けの汎用契約として一般化する必然性がない、同梱プラグイン
   固有の実装詳細である。
3. **REQ-2 原則との整合**: 「既存拡張点で表現できない場合にのみ新規 trait 追加を検討
   する」という原則（`docs/spec` 追随済み、イシュー #432）に照らすと、4 節のギャップ
   （圧縮後の最終応答への介入）は具体的需要が未提示であり、現時点で公開 trait を新設
   するのは原則違反にあたる。
4. **fail-closed**: 4 節の残ギャップを外部コードへ開放すると、serialize 直前
   （keep-alive 判定後）での Content-Length / Transfer-Encoding 矛盾（レスポンス分割・
   スマグリングの余地）、圧縮済み body への任意改変、`RequestGate` 拒否応答除外契約の
   希薄化など、攻撃表面が拡大する（7 節参照）。具体的需要のない現時点では判断がつかない
   境界を保守側（不採用）へ倒す（`.claude/rules/feasibility-guardrail.md` の fail-closed
   原則と同根）。

受け入れ基準の「採用する場合は公開 trait / 登録 API・評価順序・テスト（feature 無効時
の陰性対照含む）が揃うこと」は、不採用のため非該当。

## 6. 再検討条件

以下のいずれかが生じた場合に本判断を再検討する。

- 「圧縮・CORS 適用後の最終応答への介入」を必要とする具体的ユースケース（crates.io
  利用者フィードバック等）が提示されたとき
- 第 3 の同梱レスポンス後処理型プラグインの追加時に、外部化可能な共通契約が自然に
  抽出できると判明したとき

再検討時に採用へ転じる場合の設計要件スケッチ（再調査なしで再開できるよう記録する）:

- 評価順序: `map_response` → CORS → ユーザーシーム → 圧縮、もしくは圧縮後専用の
  別フックとして建てる（既存の CORS → 圧縮固定順を崩さない）
- `RequestGate` 拒否応答・パースエラー応答の fail-closed 除外は維持する
- feature 無効時の陰性対照テスト（当該 trait 実装が存在しない構成でコード・依存が
  残らないこと、pay-for-what-you-use 検証）を必須とする
- 公開化する場合は実行時登録 API（`Interceptor` と同型の `Server::` 登録メソッド）に
  なる旨（5 節根拠 2 と同一）を設計の前提とする

再検討条件が満たされ実装に着手する場合は、`.claude/rules/out-of-scope-tracking.md` に
従いユーザー承認のうえイシュー化する。本書では再検討条件の記録に留める。

## 7. セキュリティ考慮事項（OWASP Top 10 観点）

本イシューの成果物はドキュメントのみでコード挙動は不変だが、「不採用」という判断
自体のセキュリティ根拠を記録する。

- **A01 アクセス制御の不備**: 公開 API 化しないことで、`RequestGate` 拒否応答・
  パースエラー応答へ外部コードが触れる新経路を作らない。finalize 系がこれらを除外
  する fail-closed 契約（`plugin-boundary.md` 5.9.4 節）を外部拡張により希薄化させ
  ない。
- **A03 インジェクション（レスポンス分割・スマグリング）**: serialize 直前（keep-alive
  判定後）の最終応答へ外部コードが介入できるようになると、Content-Length /
  Transfer-Encoding と body の矛盾を作る余地が生じる。既存の公開経路
  `Interceptor::map_response` は `Response` の検証済み API（予約ヘッダ拒否・CR/LF/NUL
  拒否）に閉じており、この保証を維持する。
- **A04 安全でない設計**: 圧縮後 body への外部介入を認めないことで、BREACH 類似リスク
  面（`plugin-boundary.md` 5.10.4 節）を拡大しない。判断がつかない境界は保守側へ倒す
  fail-closed 原則（`.claude/rules/feasibility-guardrail.md`）に整合する。
- **A05 セキュリティ設定ミス**: CORS 判定ロジックの単一情報源化（`apply_cors`
  ヘルパ、イシュー #451）を維持し、外部実装による判定乖離（CORS 設定不備）の余地を
  作らない。
- **シークレット**: 本書に鍵・トークン・PII の混入要素はない。

## 8. スコープ外（現イシューに混ぜない）

- 第 3 のレスポンス後処理型プラグインの追加
- チャンク単位のストリーミング gzip 圧縮（`plugin-boundary.md` 5.10.3 節の既存後続
  課題）
- 「圧縮・CORS 適用後の最終応答への介入」フックの実装（6 節の再検討条件が満たされた
  時点でユーザー承認のうえイシュー化する）
