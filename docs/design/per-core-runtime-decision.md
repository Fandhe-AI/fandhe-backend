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
| tier(2) | hyper 素実装 | 35.1 万 | tokio マルチスレッド work-stealing ランタイム、単一 `TcpListener` を単一 accept タスクが処理し、受理後の接続タスクはワーカースレッド群へ work-stealing で分散されうる |
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
  したコアに固定され、他コアへ work-stealing で移動しない。コアへの固定自体は
  `Send` 契約とは独立した性質で、ハンドラタスクが `Send` のままでも成立する
  （固定されたコア上のマルチスレッドランタイム 1 ワーカーへ `tokio::spawn` すれば
  タスクは事実上そのコアに留まる。コア間でのタスク移動（cache line のコア間
  バウンス）が減るキャッシュ局所性の改善は、この「コア固定」という性質自体から
  生じるものであり、後述の `!Send` 化には依存しない）。
- **`!Send` 化は per-core accept の必須条件ではなく、独立した追加の最適化選択肢**
  である。1 接続が 1 コアに固定される構造の上でなら、ハンドラタスクを `!Send`
  （`Rc`/`RefCell` 等）で実装してマルチスレッド共有のための `Arc`/`Mutex`/
  アトミック操作を省略することも*できる*が、これは「コア固定」の上に積める
  任意の最適化であって、accept 並列化そのものが要求する性質ではない。
  「`!Send` を許容するとデータ競合検証が弱まる」という主張も不正確である:
  `Rc` が `Send` でないこと自体はコンパイラが強制し続けるため、スレッドをまたいだ
  データ競合は `!Send` 化後も引き続きコンパイル時に排除される。実際に生じうる
  リスクは、`RefCell` の借用規約違反が（`Mutex` のようなブロッキングではなく）
  実行時 panic になる点であり、これは「panic をライブラリ境界を越えさせない」
  という既存方針（`.claude/rules/coding-rust.md`）との整合を別途要する話であって、
  「データ競合の検証」の話ではない。

これが tier(1) が tier(2) を大きく上回る構造的理由であり、hyper/tokio 本体はこの構造を
既定にしていない。hyper はプロトコル実装ライブラリであり、ランタイム・accept 戦略の
選択をアプリケーション側に委ねる汎用設計を意図的に採る（tokio のワークスティーリング
マルチスレッドランタイムは大半のワークロードで扱いやすさと性能のバランスが良く、
per-core 特化は「1 プロセス =CPU 全コアを使い切る HTTP サーバ」という限定用途向けの
最適化という位置づけ）。actix-web/ntex はこの限定用途に対して意図的に構造ごと作り込む
ことで tier(1) 性能を得ている。

本書は以降、accept 並列化（`SO_REUSEPORT` + コアごとの listener/ランタイム、`Send`
契約を維持する）を**軸 A**、ハンドラタスクの `!Send` 化（`Rc`/`RefCell` 許容）を
**軸 B** として分離して評価する（3 節・4 節）。両者は独立した設計軸であり、軸 A の
採否は軸 B の採否を含意しない。

## 3. 現行アーキテクチャとの整合性分析

`explorer` 相当の棚卸しにより、per-core モデル採用時に影響しうる公開契約・並行機構を
軸 A（accept 並列化、`Send` 契約維持）・軸 B（`!Send` ハンドラ許容）に分けて洗い出す
（ファイル・行は本イシュー着手時点の実在参照）。2 節のとおり軸 A は軸 B を必須と
しないため、軸 A 単独での影響と軸 B を伴う場合の追加影響を分離して示す。

### 3.1 `Send + Sync` を要求する公開契約（軸 A では不変）

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
静的検証（コンパイル時アサーション）があり、4 拡張点の `Send + Sync` 境界は「複数
ワーカースレッドから呼ばれる」ことを前提にコード上で強制されている（`interceptor.rs:236-238`
の `Interceptor` も同様）。**軸 A（accept 並列化のみ）を採用してもこれらの契約は変更を
要しない**: 各コアの `current_thread` ランタイムがハンドラタスクを `Send` のまま
実行すればよく、trait 定義・`_assert_send_sync` を含めて現状維持できる。上表が
`!Send` 化（軸 B）を伴う場合にのみ再検討対象になる。

### 3.2 現行 accept ループの実装（軸 A が置き換える対象）

現行の `crates/core/src/server.rs` の accept 経路は、単一の `TcpListener` に対して
**単一の accept タスク**が `listener.accept().await` を繰り返し呼び出し、受理した
接続ごとの処理を `CancelSafeJoinSet`（`JoinSet` ラッパー、`tokio::spawn` 相当）へ
spawn する構造である。複数ワーカーが同一 listener に対して並列に `accept()` を
呼び合う構造ではない。spawn 後の接続タスク自体は、マルチスレッドランタイムの
work-stealing により起点コア以外のワーカースレッドへ移動しうる（「1 つの accept
タスク + spawn 後は work-stealing され得る接続タスク」が現行実装の正確な記述）。

軸 A（`SO_REUSEPORT` で listener を N 分割し、コアごとに専用の accept 経路を持たせる）
は、この「単一 accept タスク」という構造を「コアごとの accept タスク（+ コア固定
ランタイムまたはコア固定 spawn）」へ置き換える変更であり、次の機構が影響を受ける
（`Send` 契約は 3.1 のとおり不変のまま、accept・drain・キャンセル伝播の調整範囲が
「単一ランタイム内」から「コアをまたいだ調整」へ変わる点が変更の中心）。

| 機構 | 所在 | 軸 A 採用時の変更点 |
|------|------|------|
| accept ループ | `crates/core/src/server.rs`（`listener.accept()` 周辺） | `SO_REUSEPORT` bind ×N・コアごとの accept タスク起動・接続のコア固定方針の決定 |
| graceful shutdown | `crates/core/src/server.rs`（`BoundServer::run_until`、#313） | 「accept 停止 → in-flight 完了待ち → grace 超過強制クローズ」をコアごとに独立させるか集約するかの再設計 |
| rebind 世代 drain | `crates/core/src/server.rs`（`RebindHandle::rebind`、#485/#488） | 新 listener 差し替え・旧世代 drain をコアごとに独立実行できる形へ再設計 |
| WS 世代キャンセル | `crates/core/src/lib.rs` / `crates/core/src/plugin.rs` / `crates/core/src/server.rs`（`GenerationCancel`/`UpgradeCancel`、#489〜#499） | `tokio::sync::watch` 自体はマルチスレッドランタイム内でスレッドをまたいで機能するため技術的には流用可能。「コアごとに独立した世代管理が必要か」の設計判断が要る |
| `SessionDrain`（WebRTC） | `crates/core/src/plugin.rs`、`crates/plugin-webrtc/src/drain.rs` | 同上 |

`spawn_blocking` 圧縮オフロード（#468）はマルチスレッドランタイムのブロッキング
スレッドプールを前提とするが、軸 A 単独（ハンドラは `Send` のまま・`current_thread`
ランタイムでもブロッキングプールは持てる）であればプール構成の見直しに留まり、
拡張点契約・ハンドラ型には影響しない。

## 4. 採用した場合の影響範囲と概算工数

3 節の棚卸しを踏まえた機構単位の概算（Rough Order of Magnitude、実測ではなく設計上の
見積り。1 = 数日、大 = 数週間規模を目安とする定性区分）を軸 A・軸 B で分けて示す。

### 4.1 軸 A（accept 並列化、`Send` 契約維持）の影響範囲

| 変更対象 | 内容 | 規模目安 |
|---------|------|---------|
| accept ループ・`BoundServer::run_until` | `SO_REUSEPORT` bind・コアごとの accept タスク起動・接続のコア固定方針決定（`Send` 契約は不変） | 大 |
| graceful shutdown（#313） | 「in-flight 完了待ち」をコアごとに独立して行う設計へ作り直し | 中〜大 |
| rebind（#485/#488） | 世代 drain・listener 差し替えをコアごとに独立実行できる形へ再設計 | 中〜大 |
| WS 世代キャンセル（#489〜#499） | `tokio::sync::watch` はマルチスレッドランタイム内であればスレッドをまたいで機能するため per-core でも技術的には使えるが、「コアごとに独立した世代管理が必要か」の設計判断・全経路の再検証が要る | 中 |
| `SessionDrain`（WebRTC、#498） | 同上 | 中 |
| `spawn_blocking` 圧縮オフロード（#468） | `current_thread` ランタイムはブロッキングスレッドプールを持てるが、コアあたり 1 ランタイムだと専用プール構成の見直しが要る | 小〜中 |
| 既存並行設計ドキュメント | `graceful-shutdown.md` / `rebind.md` / `ws-cancellation-propagation.md` / `plugin-boundary.md` の accept・drain 経路に関する記述改訂 | 中 |
| テスト・ベンチ・受け入れ検証 | per-core accept 経路の統合テスト新設、既存の graceful shutdown・rebind・WS キャンセル・`SessionDrain` の回帰テストをコアごとの独立性を含めて再検証、REQ-1/NFR-1 系ベンチの再計測（`benches/bench-accept-exclusive.sh` 系） | 大 |

軸 A 単独では 4 拡張点 trait（`Middleware`/`UpgradeHandler`/`RequestGate`/
`Interceptor`）・`crates/routes` のハンドラ型（`RouteHandler`/`ParamRouteHandler`/
`HandlerFuture`）・streaming API（`BodyWriter`）は 3.1 のとおり変更不要であり、
13 公開クレートの breaking change・利用者の外部クレート（`sqlx` 等）互換性再検証も
発生しない。軸 A 単独の影響範囲は accept/bind 層と graceful shutdown・rebind・WS
世代キャンセル・`SessionDrain` という 4 つの並行調整機構、およびそれらのテスト・
ベンチに限定でき、**個別に列挙可能**である。

### 4.2 軸 B（`!Send` ハンドラ許容）を追加した場合の影響範囲

軸 A に加えて軸 B（`!Send` タスク・`Rc`/`RefCell` の許容）を採る場合、次が追加で
影響を受ける。

| 変更対象 | 内容 | 規模目安 |
|---------|------|---------|
| 4 拡張点 trait の `Send` 境界 | `Send` 除去（`!Send` タスク許容）または `Send`/`?Send` 二重系統の併設。前者は既存ユーザー実装の破壊的変更、後者は API 表面の倍増 | 大（breaking change、13 公開クレート lockstep のメジャーバンプ級） |
| `Router` ハンドラ型（`RouteHandler`/`ParamRouteHandler`/`HandlerFuture`） | 同上 | 大 |
| streaming API（`BodyWriter`） | `mpsc::Sender` の `Send` 前提を含め producer タスクの生成方式ごと見直し | 中 |
| tokio エコシステム互換 | `sqlx` 等、ハンドラ内で `.await` される外部クレートの多くは `Send` future を前提に設計されている（`async-handler.md`、イシュー #314/#315）。ハンドラを `!Send` 化すると、こうした外部クレートとの共存可否をクレートごとに再検証する必要がある | 大（利用者依存クレートに波及、fandhe-backend 側で制御不能） |

総括すると、軸 A（accept 並列化）は影響範囲を accept/bind 層と 4 並行機構に限定
できる一方、**軸 B（`!Send` 化）を伴う場合にのみ** 4 拡張点 trait の公開契約・
`crates/routes` のハンドラ型・streaming API・13 公開クレートのバージョニング・
利用者が持ち込む外部クレート（`sqlx` 等）との互換性にまで影響が波及し、「限定
できない」規模になる。5 節の不採用判断は、軸 A・軸 B を合わせて採用する案
（現時点で採否判断の主対象とする、6 節参照）を対象とする。

## 5. 採否の結論: 不採用（現時点）

**結論: per-core accept モデル（軸 A + 軸 B、`!Send` ハンドラ許容込みの一般形。
2 節・4 節参照）の採用は現時点で不採用とする。**

**本判断は `Send + Sync` 契約を維持したまま accept を並列化する案（軸 A 単独）を
否定しない。** 軸 A 単独は 4.1 節のとおり影響範囲を accept/bind 層と graceful
shutdown・rebind・WS 世代キャンセル・`SessionDrain` という 4 つの並行調整機構に
限定でき、4 拡張点 trait・`crates/routes` ハンドラ型・streaming API・13 公開クレート
のバージョニング・外部クレート（`sqlx` 等）互換性には影響しない。軸 A 単独案は
6 節「再検討条件 (b)」の第一候補として扱い、本書の不採用結論を根拠に将来の軸 A 単独
提案を P1 指摘対象としない。

根拠（軸 A + 軸 B の一般形、`!Send` ハンドラ許容を含む案について）:

1. **`!Send` 化に伴う panic 境界の再整備コスト**: 2 節のとおり、`!Send` 化自体は
   `Rc` が `Send` でないことをコンパイラが強制し続けるためデータ競合の検証を弱め
   ない。一方 `RefCell` の借用規約違反は実行時 panic になり、「panic をライブラリ
   境界を越えさせない」という既存方針（`.claude/rules/coding-rust.md`）との整合を
   別途設計する必要がある。この整備コストは 4.2 節の影響範囲（4 拡張点 trait・
   `crates/routes` ハンドラ型・streaming API の breaking change）と併せて負う必要が
   あり、判断がつかない境界を保守側（不採用）へ倒す fail-closed 原則
   （`.claude/rules/feasibility-guardrail.md`）に照らし現時点では不採用側へ倒す。
2. **hyper が同種の判断を意図的に取らないことと同根**: 2 節のとおり、hyper/tokio 本体は
   per-core 特化をライブラリ既定にしていない。tier(2) の hyper 素実装自体が tier(1) の
   65% 程度で頭打ちになっているのは、hyper が汎用性・エコシステム互換を per-core 特化
   より優先した結果である。fandhe-backend が hyper 上に構築されている以上、hyper が
   選ばなかった構造を土台の上に無理に積み増すコストは、hyper のライブラリ設計判断を
   フレームワーク側で覆すコストに等しい。
3. **軸 B を伴う一般形は影響範囲が全並行機構・全公開契約に波及し限定できない**
   （feasibility-guardrail の 3 軸: 実施可能か・安全か・**影響範囲が許容内か** の
   うち、影響範囲が不充足）: 4.2 節のとおり、軸 B を採る場合 4 拡張点 trait・
   `crates/routes` のハンドラ型・streaming API・13 公開クレートの lockstep
   バージョニング・利用者が持ち込む外部クレート（`sqlx` 等）との互換性にまで影響が
   波及する。軸 A 単独（4.1 節）に閉じた変更としては影響範囲を列挙・限定できるが、
   軸 B を伴う一般形はそれができないため、`finalize-seam-public-api.md`（#462）5 節と
   同じ論法（判断がつかない境界は保守側へ倒す fail-closed）で不採用側へ倒す。
4. **性能目標に対して緊急性がない**: #579 の Phase 目標は「Phase 1 + Phase 3（P1 実装）で
   hyper 素実装同等（約 35 万 RPS、+8〜15%）」であり、per-core なしで到達見込みが立って
   いる。tier(1) との残差 30〜40% を今すぐ詰める必要がある具体的な事業要件は本イシュー
   起票時点で提示されていない。この緊急性のなさは軸 A 単独案にも及ぶ: 軸 A のみでは
   tier(1) 帯（約 54 万 RPS）到達には通常至らない（tier(1) は軸 A・軸 B 双方を組み
   合わせて実現している構造のため）ため、軸 A 単独を今すぐ採用する動機も同様に薄い。

受け入れ基準の「採用する場合の設計要件」は不採用のため非該当。ただし再検討時に
再調査なしで再開できるよう、設計要件スケッチを 6 節に残す。

## 6. 再検討条件

以下のいずれかが生じた場合に本判断を再検討する。

- **(a) 実ワークロードでの tier(2) 上限の未達が実測で示されたとき**: 実運用の
  ワークロードで hyper 系 tier（約 35 万 RPS 相当）の上限が事業要件を下回ることが
  ベンチ・本番計測で具体的に示されたとき。
- **(b) opt-in feature としての限定導入案が両立性を示せたとき**: 既定はマルチスレッド
  ランタイム（現行構造）を維持し、`per-core` のような Cargo feature を明示的に有効化
  した場合のみ別 accept 経路（`SO_REUSEPORT` + コアごとの accept 経路）を使う設計。
  **第一候補は軸 A 単独（`Send` 契約維持）である**: 5 節のとおり軸 A 単独は 4 拡張点
  trait・ハンドラ型・13 公開クレートのバージョニングに影響しないため、`Send` 契約の
  二重化コストを負わずに成立しうる。成立条件は次の 2 点を両立できると設計段階で
  示せること:
  - **pay-for-what-you-use**（`.claude/rules/pay-for-what-you-use.md`）: feature 無効時に
    コード・依存（`socket2` 等の `SO_REUSEPORT` 用クレート）・`unsafe` の増分がゼロで
    あること。
  - accept/bind 層・drain/キャンセル系 4 機構（4.1 節）を feature ゲート付きで
    分岐させる設計が、既存の非 per-core 経路の可読性・保守性を大きく損なわないこと。
  軸 B（`!Send` ハンドラ許容）まで含める場合のみ、追加で「`Send` 契約の二重化コスト:
  拡張点 trait・ハンドラ型を `Send` 版 / `?Send` 版で併設する場合の API 表面倍増・
  保守コストが、pay-for-what-you-use の恩恵（未使用時ゼロコスト）と見合うこと」を
  示す必要がある。

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
- **拡張点 trait の `?Send` 化パターン**（軸 B を採る場合）: 既存 4 拡張点
  （`Middleware`/`UpgradeHandler`/`RequestGate`/`Interceptor`）を `Send` 必須のまま
  維持しつつ、per-core 専用の第 5 の拡張点（例: `LocalHandler`、`?Send` 許容）を feature
  ゲート付きで新設する案が、既存 4 拡張点の破壊的変更を避けられる点で有力候補となる。
- **並行機構の per-core 対応順**（軸 A に共通）: graceful shutdown（#313）→
  rebind（#485/#488）→ WS キャンセル（#489〜#499）→ `SessionDrain`（#498）の順に、
  依存が薄いものから段階的に再設計する（4.1 節の一覧を優先度順の着手リストとして
  使う）。

再検討条件が満たされ実装に着手する場合は、`.claude/rules/out-of-scope-tracking.md` に
従いユーザー承認のうえイシュー化する。本書では再検討条件の記録に留める。

## 7. セキュリティ考慮事項（OWASP Top 10 観点）

本イシューの成果物はドキュメントのみでコード挙動は不変だが、「不採用」という判断
自体のセキュリティ根拠を記録する（`finalize-seam-public-api.md` 7 節と同型）。

- **A04 安全でない設計**: `!Send` 化（軸 B）自体は `Rc` が `Send` でないことを
  コンパイラが強制し続けるためデータ競合の検証を弱めない。一方 `RefCell` の借用
  規約違反は実行時 panic になり、「panic をライブラリ境界を越えさせない」という
  既存方針（`.claude/rules/coding-rust.md`）との整合を別途要する。この整備コストを
  4.2 節の breaking change 規模の影響範囲と併せて負う必要があるという判断が、判断が
  つかない境界を保守側（不採用）へ倒す fail-closed 原則
  （`.claude/rules/feasibility-guardrail.md`）に整合することを 5 節の根拠 1 として
  明記した（本項目は軸 B を伴う一般形にのみ適用され、`Send` 契約を維持する軸 A
  単独案には適用されない）。
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
