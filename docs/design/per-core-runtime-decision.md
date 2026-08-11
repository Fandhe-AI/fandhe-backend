# P5 per-core accept モデルの採否判断を記録する（イシュー #589）

## 1. 背景

イシュー #579（2026-08-11 実施の 15 フレームワーク実測ベンチ比較・原因調査、親トラック
イシュー）の結果、fandhe-backend は `/health` エンドポイントで 31.7 万 RPS を記録した。
これは axum（32.6 万）・poem・salvo 等 hyper 系 tier とほぼ互角だが、hyper 素実装
（35.1 万）比で −10%、actix-web/ntex（約 54 万）比で 57% にとどまる。

原因調査で確認された階層構造（#579 本文が一次ソース）:

| tier | 代表実装 | 実測 RPS（目安） | 構造上の特徴 |
|------|---------|------------------|-------------|
| tier(1) | actix-web / ntex | 約 54 万 | `SO_REUSEPORT` + コアごとの `current_thread` ランタイム（per-core accept モデル） |
| tier(2) | hyper 素実装 | 35.1 万 | tokio マルチスレッド work-stealing ランタイム、accept は単一 listener 共有 |
| tier(2) 相当 | fandhe-backend / axum / poem / salvo | 31.7 万〜32.6 万 | hyper 素実装と同じ土台（マルチスレッド work-stealing）、フレームワーク層のオーバーヘッド分だけ hyper 素実装を下回る |

hyper 素実装自体が tier(1) の 65% 程度で頭打ちになっている事実は、fandhe-backend と
tier(1) の差が「フレームワーク層の実装品質」ではなく「hyper + tokio マルチスレッド
work-stealing」という**土台の構造差**であることを示している。Phase 1（#580、P2〜P4 の
低リスク最適化）+ Phase 3（#582、P1 ヘッダゼロコピー化）を適用しても、目標は
hyper 素実装同等（約 35 万 RPS、+8〜15%）までであり、tier(1) への到達には別の構造が
要る。

本書はその構造である **per-core accept モデル（P5）** について、実装せずに採否判断と
根拠を記録するイシュー #589（親 #581 Phase 2 設計判断、ルート #579）の成果物である。
先行する設計判断ドキュメント [`finalize-seam-public-api.md`](./finalize-seam-public-api.md)
（イシュー #462）と同形式（棚卸し → 影響範囲分析 → 不採用根拠 → 再検討条件）に揃える。

参照:
- [`graceful-shutdown.md`](./graceful-shutdown.md)（最終 graceful shutdown、イシュー #313）
- [`rebind.md`](./rebind.md)（rebind 世代 drain、イシュー #485/#488）
- [`ws-cancellation-propagation.md`](./ws-cancellation-propagation.md)（WS 委譲タスクへの
  キャンセル伝播、イシュー #490〜#499）
- [`plugin-boundary.md`](./plugin-boundary.md)（プラグイン境界パターン一覧）

## 2. per-core accept モデルの説明

actix-web / ntex（tier(1)）が採用する構造は概ね次のとおり:

- `SO_REUSEPORT` ソケットオプションで、同一アドレス・ポートに対して複数の listener を
  OS レベルで並列 bind する。カーネルが接続を listener 群へ分散するため、userland での
  accept 競合（単一 listener を全ワーカーが奪い合う構造）が生じない。
- ワーカー数（通常は論理コア数）だけ `current_thread`（シングルスレッド）の tokio
  ランタイムを起動し、各ランタイムが自分専用の listener を占有する。1 接続は accept
  したコアに固定され、他コアへ work-stealing で移動しない。
- 接続がコアに固定されるため、ハンドラタスクは `Send` である必要がなく `!Send`
  （`Rc`/`RefCell` 等の非同期化不要な単純参照カウント・可変参照）で実装でき、
  マルチスレッド共有のための `Arc`/`Mutex`/アトミック操作を要所で省略できる。
  加えてコア間でのタスク移動（cache line のコア間バウンス）がなくなるため、
  キャッシュ局所性が改善する。

これが tier(1) が tier(2) を大きく上回る構造的理由であり、hyper/tokio 本体はこの構造を
既定にしていない。hyper はプロトコル実装ライブラリであり、ランタイム・accept 戦略の
選択をアプリケーション側に委ねる汎用設計を意図的に採る（tokio のワークスティーリング
マルチスレッドランタイムは大半のワークロードで扱いやすさと性能のバランスが良く、
per-core 特化は「1 プロセス =CPU 全コアを使い切る HTTP サーバ」という限定用途向けの
最適化という位置づけ）。actix-web/ntex はこの限定用途に対して意図的に構造ごと作り込む
ことで tier(1) 性能を得ている。

## 3. 現行アーキテクチャとの整合性分析

`explorer` 相当の棚卸しにより、per-core モデル採用時に再設計が必要になる公開契約・
並行機構を洗い出す（ファイル・行は本イシュー着手時点の実在参照）。

### 3.1 `Send + Sync` を要求する公開契約

| 契約 | 所在 | 内容 |
|------|------|------|
| `Middleware` | `crates/core/src/extension.rs:86` | `pub trait Middleware: Send + Sync` |
| `UpgradeHandler` | `crates/core/src/extension.rs:135` | `pub trait UpgradeHandler: Send + Sync` |
| `RequestGate` | `crates/core/src/extension.rs:357` | `pub trait RequestGate: Send + Sync` |
| `Interceptor` | `crates/core/src/interceptor.rs:191` | `pub trait Interceptor: Send + Sync` |
| `RouteHandler` / `ParamRouteHandler` | `crates/routes/src/lib.rs:120,130` | `Box<dyn Fn(...) -> HandlerFuture + Send + Sync>` |
| `HandlerFuture` | `crates/routes/src/lib.rs:108` | `Pin<Box<dyn Future<Output = Response> + Send>>` |
| `BodyWriter` | `crates/core/src/streaming.rs` | `tokio::spawn` した producer タスクへ move する `Send + 'static` 契約（`mpsc::Sender` 保持） |
| `WsMessageHandler` | `crates/plugin-websocket` | ユーザー定義メッセージハンドラ、`on_message` が返す `Future` を `race_cancel` で扱う |

`crates/core/src/extension.rs:384-394` には `_assert_send_sync::<dyn Middleware>()` 等の
静的検証（コンパイル時アサーション）があり、3 拡張点の `Send + Sync` 境界は「複数
ワーカースレッドから呼ばれる」ことを前提にコード上で強制されている（`interceptor.rs:236-238`
の `Interceptor` も同様）。

### 3.2 `tokio::spawn` / マルチスレッド前提の並行機構

| 機構 | 所在 | 前提 |
|------|------|------|
| accept ループ | `crates/core/src/server.rs`（`listener.accept()` / `poll_accept` 周辺） | 単一 `TcpListener` をマルチスレッドランタイムの複数ワーカーが共有し `tokio::spawn` で接続ごとにタスク生成 |
| graceful shutdown | `crates/core/src/server.rs`（`BoundServer::run_until`、#313） | accept 停止 → in-flight（他コアで動く可能性のある）タスク完了待ち → grace 超過強制クローズ |
| rebind 世代 drain | `crates/core/src/server.rs`（`RebindHandle::rebind`、#485/#488） | 新 listener 差し替え後、旧世代接続をランタイム全体で背景 drain |
| WS 世代キャンセル | `crates/core/src/lib.rs` / `crates/core/src/plugin.rs` / `crates/core/src/server.rs`（`GenerationCancel`/`UpgradeCancel`、#489〜#499） | `tokio::sync::watch` チャネルでランタイム全体のワーカーへキャンセル信号を broadcast |
| `SessionDrain`（WebRTC） | `crates/core/src/plugin.rs`、`crates/plugin-webrtc/src/drain.rs` | 最終 graceful shutdown・rebind 両経路からレジストリ全件へ有界 close を伝播 |
| `spawn_blocking` 圧縮オフロード | `crates/core`（#468） | マルチスレッドランタイムのブロッキングスレッドプールへオフロード |

これらはいずれも「1 つの共有 `TcpListener` + マルチスレッドランタイムの work-stealing」
という前提の上に組み上がっている。per-core 化（`SO_REUSEPORT` で listener を N 分割し、
各コアに `current_thread` ランタイムを割り当てる）は、この前提そのものを置き換える
変更であり、上表の全機構が「単一ランタイム内で完結する調整」から「コアをまたいだ
調整」へ作り直しを要する。

## 4. 採用した場合の影響範囲と概算工数

3 節の棚卸しを踏まえた機構単位の概算（Rough Order of Magnitude、実測ではなく設計上の
見積り。1 = 数日、大 = 数週間規模を目安とする定性区分）:

| 変更対象 | 内容 | 規模目安 |
|---------|------|---------|
| 3 拡張点 trait の `Send` 境界 | `Send` 除去（`!Send` タスク許容）または `Send`/`?Send` 二重系統の併設。前者は既存ユーザー実装の破壊的変更、後者は API 表面の倍増 | 大（breaking change、13 公開クレート lockstep のメジャーバンプ級） |
| `Router` ハンドラ型（`RouteHandler`/`ParamRouteHandler`/`HandlerFuture`） | 同上 | 大 |
| streaming API（`BodyWriter`） | `mpsc::Sender` の `Send` 前提を含め producer タスクの生成方式ごと見直し | 中 |
| accept ループ・`BoundServer::run_until` | `SO_REUSEPORT` bind・`current_thread` ランタイム ×N 起動・接続のコア固定 | 大 |
| graceful shutdown（#313） | 「in-flight 完了待ち」をコアごとに独立して行う設計へ作り直し | 中〜大 |
| rebind（#485/#488） | 世代 drain・listener 差し替えをコアごとに独立実行できる形へ再設計 | 中〜大 |
| WS 世代キャンセル（#489〜#499） | `tokio::sync::watch` はマルチスレッドランタイム内であればスレッドをまたいで機能するため per-core でも技術的には使えるが、「コアごとに独立した世代管理が必要か」の設計判断・全経路の再検証が要る | 中 |
| `SessionDrain`（WebRTC、#498） | 同上 | 中 |
| `spawn_blocking` 圧縮オフロード（#468） | `current_thread` ランタイムはブロッキングスレッドプールを持てるが、コアあたり 1 ランタイムだと専用プール構成の見直しが要る | 小〜中 |
| tokio エコシステム互換 | `sqlx` 等、ハンドラ内で `.await` される外部クレートの多くは `Send` future を前提に設計されている（`async-handler.md`、イシュー #314/#315）。ハンドラを `!Send` 化すると、こうした外部クレートとの共存可否をクレートごとに再検証する必要がある | 大（利用者依存クレートに波及、fandhe-backend 側で制御不能） |
| 既存並行設計ドキュメント | `graceful-shutdown.md` / `rebind.md` / `ws-cancellation-propagation.md` / `plugin-boundary.md` の全面改訂 | 中 |
| テスト・ベンチ・受け入れ検証 | per-core 経路の統合テスト新設、既存の graceful shutdown・rebind・WS キャンセル・`SessionDrain` の回帰テストをコアごとの独立性を含めて再検証、REQ-1/NFR-1 系ベンチの再計測（`benches/bench-accept-exclusive.sh` 系） | 大 |

総括すると、影響は「一部モジュールの局所改修」に収まらず、**4 拡張点 trait の公開
契約・`crates/routes` のハンドラ型・streaming API・accept ループ・graceful shutdown /
rebind / WS キャンセル / `SessionDrain` の全並行機構・13 公開クレートのバージョニング・
利用者が持ち込む外部クレート（`sqlx` 等）との互換性」にまたがる。個別工数の積算では
なく、影響範囲そのものが「限定できない」規模であることが 5 節の不採用根拠の中心となる。

## 5. 採否の結論: 不採用（現時点）

**結論: per-core accept モデルの採用は現時点で不採用とする。**

根拠:

1. **安全性・AI ファースト保守性優先の設計原則との衝突**: イシュー #579 の方針
   「安全性・AI ファースト保守性を優先した上で改善する」に対し、per-core 化が要求する
   `!Send` タスク・`Rc`/`RefCell` 系構造（3 節）は、データ競合をコンパイラが検証する
   前提（`Send`/`Sync` 境界）を弱める方向の変更である。「AI によるセキュリティ脆弱性
   発見リスクに備える」というフレームワークの出発点（`.claude/rules/security.md`）に
   照らし、判断がつかない境界を保守側（不採用）へ倒す fail-closed 原則
   （`.claude/rules/feasibility-guardrail.md`）に整合させる。
2. **hyper が同種の判断を意図的に取らないことと同根**: 2 節のとおり、hyper/tokio 本体は
   per-core 特化をライブラリ既定にしていない。tier(2) の hyper 素実装自体が tier(1) の
   65% 程度で頭打ちになっているのは、hyper が汎用性・エコシステム互換を per-core 特化
   より優先した結果である。fandhe-backend が hyper 上に構築されている以上、hyper が
   選ばなかった構造を土台の上に無理に積み増すコストは、hyper のライブラリ設計判断を
   フレームワーク側で覆すコストに等しい。
3. **影響範囲が全並行機構・全公開契約に波及し限定できない**（feasibility-guardrail の
   3 軸: 実施可能か・安全か・**影響範囲が許容内か** のうち、影響範囲が不充足）:
   4 節の棚卸しのとおり、4 拡張点 trait・`crates/routes` のハンドラ型・streaming API・
   accept ループ・graceful shutdown・rebind・WS キャンセル・`SessionDrain` の全機構・
   13 公開クレートの lockstep バージョニング・利用者が持ち込む外部クレート（`sqlx` 等）
   との互換性にまたがる。1 モジュール・1 クレートに閉じた変更として見積れず、
   `finalize-seam-public-api.md`（#462）5 節と同じ論法（判断がつかない境界は保守側へ倒す
   fail-closed）で不採用側へ倒す。
4. **性能目標に対して緊急性がない**: #579 の Phase 目標は「Phase 1 + Phase 3（P1 実装）で
   hyper 素実装同等（約 35 万 RPS、+8〜15%）」であり、per-core なしで到達見込みが立って
   いる。tier(1) との残差 30〜40% を今すぐ詰める必要がある具体的な事業要件は本イシュー
   起票時点で提示されていない。

受け入れ基準の「採用する場合の設計要件」は不採用のため非該当。ただし再検討時に
再調査なしで再開できるよう、設計要件スケッチを 6 節に残す。

## 6. 再検討条件

以下のいずれかが生じた場合に本判断を再検討する。

- **(a) 実ワークロードでの tier(2) 上限の未達が実測で示されたとき**: 実運用の
  ワークロードで hyper 系 tier（約 35 万 RPS 相当）の上限が事業要件を下回ることが
  ベンチ・本番計測で具体的に示されたとき。
- **(b) opt-in feature としての限定導入案が両立性を示せたとき**: 既定はマルチスレッド
  ランタイム（現行構造）を維持し、`per-core` のような Cargo feature を明示的に有効化
  した場合のみ別 accept 経路（`SO_REUSEPORT` + `current_thread` ×N）を使う設計。
  成立条件は次の 2 点を両立できると設計段階で示せること:
  - **pay-for-what-you-use**（`.claude/rules/pay-for-what-you-use.md`）: feature 無効時に
    コード・依存（`socket2` 等の `SO_REUSEPORT` 用クレート）・`unsafe` の増分がゼロで
    あること。
  - `Send` 契約の二重化コスト: 拡張点 trait・ハンドラ型を `Send` 版 / `?Send` 版で
    併設する場合の API 表面倍増・保守コストが、pay-for-what-you-use の恩恵（未使用時
    ゼロコスト）と見合うこと。

再検討時に採用へ転じる場合の設計要件スケッチ（再調査なしで再開できるよう記録する）:

- **`SO_REUSEPORT` の OS 依存性**: Linux（`SO_REUSEPORT`、カーネル 3.9+）と
  BSD 系/macOS（`SO_REUSEPORT_LB` 等、意味論が異なる場合がある）でプラットフォーム
  分岐が必要になりうる。対応 OS 範囲をあらかじめ確定する。
- **`tokio::task::LocalSet` 案**: `!Send` タスクをマルチスレッドランタイム上の
  `current_thread` 相当の枠内で動かす `tokio::task::LocalSet` を使えば、accept
  ループ自体は既存のマルチスレッドランタイムのまま、ハンドラ実行だけをコア（ワーカー
  スレッド）に固定する折衷案が取れる可能性がある。真の per-core（`current_thread`
  ランタイム ×N + `SO_REUSEPORT`）より変更範囲を小さくできるかどうかを再検討時に
  最初に評価する。
- **拡張点 trait の `?Send` 化パターン**: 3 拡張点 + `Interceptor` を `Send` 必須のまま
  維持しつつ、per-core 専用の第 5 の拡張点（例: `LocalHandler`、`?Send` 許容）を feature
  ゲート付きで新設する案が、既存 4 拡張点の破壊的変更を避けられる点で有力候補となる。
- **並行機構の per-core 対応順**: graceful shutdown（#313）→ rebind（#485/#488）→
  WS キャンセル（#489〜#499）→ `SessionDrain`（#498）の順に、依存が薄いものから
  段階的に再設計する（3 節の一覧を優先度順の着手リストとして使う）。

再検討条件が満たされ実装に着手する場合は、`.claude/rules/out-of-scope-tracking.md` に
従いユーザー承認のうえイシュー化する。本書では再検討条件の記録に留める。

## 7. セキュリティ考慮事項（OWASP Top 10 観点）

本イシューの成果物はドキュメントのみでコード挙動は不変だが、「不採用」という判断
自体のセキュリティ根拠を記録する（`finalize-seam-public-api.md` 7 節と同型）。

- **A04 安全でない設計**: per-core 化は `!Send` タスク・`Rc`/`RefCell`・スレッド固定
  共有状態など、データ競合をコンパイラが検証する前提（`Send`/`Sync` 境界）を弱める
  構造を持ち込む。「AI によるセキュリティ脆弱性発見リスクに備える」という本フレーム
  ワークの出発点（`.claude/rules/security.md`）に照らし、判断がつかない境界を保守側
  （不採用）へ倒す fail-closed 原則（`.claude/rules/feasibility-guardrail.md`）に整合
  することを 5 節の根拠 1 として明記した。
- **A05 セキュリティ設定ミス / リソース枯渇（DoS）**: 既存の graceful shutdown・
  rebind drain・WS キャンセル・`SessionDrain` は接続リソースの有界解放（grace 超過
  強制クローズ）を保証する多層防御であり、per-core 化はこれら全機構の再実装を要する
  （3 節・4 節）。再設計時の回帰（drain 漏れ・接続リーク・grace 判定の per-core 分断
  による取りこぼし）リスクを不採用根拠の一部とする。
- **A06 脆弱な依存**: 不採用により `socket2` 等の `SO_REUSEPORT` 用依存・プラット
  フォーム分岐・追加 `unsafe` を導入しない（pay-for-what-you-use・攻撃表面最小化の
  維持）。
- **A09 ログと監視**: 本書にベンチ数値・イシュー番号以外の環境情報（内部ホスト名・
  認証情報）を含めない。
- **シークレット**: 鍵・トークン・PII の混入要素はない。

## 8. スコープ外（現イシューに混ぜない）

- per-core accept モデルの実装・PoC（6 節の再検討条件が満たされた時点でユーザー承認の
  うえイシュー化する）
- P1〜P4 個別最適化（#580 Phase 1、#582/#590 Phase 3 系）
- 性能改善ツリー全体の専有ベンチ再計測（#594）
