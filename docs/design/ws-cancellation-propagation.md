# WS 委譲タスクへのキャンセル伝播機構の設計

イシュー #490（`feat(core): WS 委譲タスクへのキャンセル伝播機構の設計`）対応。
`UpgradeHandler` 経由で WebSocket へ委譲された接続タスクへ、最終 graceful
shutdown（#313）・rebind 世代 drain（#485/#488）双方のキャンセルを伝播する
機構を設計する。**本イシューは設計のみを成果物とし、コード実装は行わない**。
コア配線は #491、`plugin-websocket` 側の Close frame 送信は #492、統合
テスト・`run_until` doc「既知の限界」更新は #493 が担う。

対応する要件・タスクは
[`docs/spec/04-requirements.md`](../spec/04-requirements.md) REQ-4
（WebSocket プラグイン。「大量の長時間接続を低リソースでさばけるように
する」ユーザーストーリー）および同 PoC-7 の Conditional Go 条件 (1)
（「長時間接続へ処理を委譲する際のコア側リソース解放」）である。本設計は
この条件が要求する「委譲後のリソース解放」を、恒常的な解放漏れ（PoC-7 が
検出したバッファ未解放）ではなく、shutdown/rebind 時の能動的なキャンセル
伝播という形で補完する。

## 1. 背景・現状の制約

`crate::plugin::try_handle_upgrade`（`crates/core/src/plugin.rs`）は
`UpgradeHandler` の委譲判定成立後、semaphore permit を `Option::take` で
奪って `tokio::spawn` した専用タスクへ **permit ごと move** する。この
専用タスクは `BoundServer::run_until`（`crates/core/src/server.rs`）が
管理する `CancelSafeJoinSet` の**外側**で走るため、次の 2 経路の grace
超過強制クローズ（`JoinSet::shutdown`）の対象に含まれない。

1. **最終 graceful shutdown**（#313）: `run_until` 末尾の
   `acquire_many_owned(permit_total)` timeout 超過時
   （`docs/design/graceful-shutdown.md` 8 節）
2. **rebind 世代 drain**（#485/#488）: `spawn_generation_drain` の grace
   超過時（`docs/design/rebind.md` 5.4 節・6 節）

いずれも in-flight 完了待ち自体は permit 回収の timeout で実装されており
「`run_until` は grace + ε 以内に必ず戻る」という既存保証は崩れていない
（フェイルセーフは既に成立している）。本イシューが解決するのは「WS
セッションそのものへ明示的なキャンセルが伝播せず、grace 超過後も接続が
生き続ける」という運用上の残課題である。

## 2. 現状構造の整理

| 構成要素 | 所在 | 要点 |
|---------|-----|------|
| `UpgradeHandler` trait | `crates/core/src/extension.rs` | `name()` + `matches(&RequestHead) -> bool` のみ。**委譲判定のみ**の同期契約で、接続処理・キャンセルは責務外 |
| 委譲シーム | `crates/core/src/plugin.rs` `try_handle_upgrade`（`pub(crate)`） | permit を take → `tokio::spawn` で detached 化 → `fandhe_backend_plugin_websocket::handle_upgrade` へ完全委譲 |
| 呼び出し元 | `crates/core/src/server.rs` `handle_connection_with_permit` | shutdown_flag 受信後の新規 Upgrade は 503 拒否済み（既委譲分が本イシューの対象） |
| 世代管理 | 同上 `run_until` | 世代 = `shutdown_flag`（`Arc<AtomicBool>`）+ `CancelSafeJoinSet` のペア。rebind 時に旧世代の flag を true → `mem::replace` で JoinSet 切り離し → `spawn_generation_drain` |
| WS セッション受信ループ | `crates/plugin-websocket/src/session.rs` `run_session` | `idle_timeout` 有効時は各受信待ちを `tokio::time::timeout` で監視。`handle_idle_timeout` が Close ハンドシェイク（Close frame 送信 → 相手の Close 応答を `WebSocketConfig::close_grace`（既定 10 秒、イシュー #500 で設定可能化）上限で待機）を既に実装済み |
| コアの tokio feature | `crates/core/Cargo.toml` | `rt` / `net` / `io-util` / `time` / `sync` の 5 つに限定 |
| plugin-websocket の tokio feature | `crates/plugin-websocket/Cargo.toml` | `io-util` / `time` のみ（**`sync` を持たない**）。`crates/core` に依存しない設計（`docs/design/plugin-boundary.md` 6.1 節、循環依存回避） |

## 3. キャンセルシグナルの方式比較

比較軸: 実現コスト／伝播レイテンシ／pay-for-what-you-use 適合／
plugin-websocket 側の追加要件／cancel-safety（取りこぼしの有無）。

| 案 | 概要 | 実現コスト | 伝播レイテンシ | pay-for-what-you-use | plugin-websocket 追加要件 | cancel-safety |
|----|------|-----------|---------------|----------------------|---------------------------|---------------|
| **A. 世代別 `tokio::sync::watch`** | 世代ごとに `watch::Sender<bool>` を `shutdown_flag` と対で生成し、委譲時に `Receiver` を専用タスクへ渡す | 小（コアの既存 `sync` feature 内で完結） | 即時（`wait_for` で通知、3.1 節「消費側の必須実装」参照） | 適合（未使用時は生成しない遅延生成が可能） | `Receiver` を直接渡す場合のみ `sync` feature が要る（4 節参照） | 現在値を先に確認してから待つ実装（`wait_for`）を必須とすれば取りこぼしなし（3.1 節） |
| B. `Notify` + 既存 `AtomicBool` 併用 | `notify_waiters` で起床させつつ、実際の判定は flag 併読に頼る | 中（flag 併読が必須契約になり誤用余地大） | 即時（ただし未待機タスクには届かない） | 適合 | 同上 | **取りこぼしうる**（`notify_waiters` は呼び出し時点で待機中のタスクにしか届かない。flag 併読を必ずセットで実装する必要があり誤用しやすい） |
| C. 既存 `Arc<AtomicBool>` 直渡し + ポーリング | `run_session` の受信待ちを短い上限の `timeout` に載せ替え、ループ毎に flag を確認 | 最小（plugin-websocket の Cargo 変更ゼロ） | ポーリング間隔に律速（周期 wakeup コストが常時発生） | 適合だが常時 wakeup コストが乗る | **不要** | 取りこぼしなし（毎周回で確認するため） |
| D（棄却）. `AbortHandle` 登録による強制 abort | WS タスクの `AbortHandle` を世代別レジストリで共有し grace 超過時に abort | 中〜大（`Mutex<Vec<AbortHandle>>` 等の共有可変レジストリが必要） | 即時 | 共有可変状態の追加コストが pay-for-what-you-use と相性が悪い | 不要（コア側で完結） | 取りこぼしなしだが **ハードキャンセル**。#492 が要求する「Close frame を送信して切断する」という正常クローズ手順を実行する猶予がない |

### 3.1 採用案: A（世代別 `watch`）

- 案 D は grace 超過時に TCP を即座に切るだけで、WS プロトコルレベルの
  正常な Close ハンドシェイク（#492 が担う予定の Close frame 送信）を
  実行する機会を奪う。`session.rs` の `handle_idle_timeout` が既に
  「Close frame 送信 → 相手の応答を `WebSocketConfig::close_grace`（既定
  10 秒）上限で待つ」という
  正常クローズパターンを実装済みであり、キャンセル伝播もこれと同じ
  「まず正常終了を試み、それ自体にも上限を設ける」設計に揃えるべきである。
  よって D は棄却する。
- 案 C はコード変更が最小である一方、無通信時でも周期的な wakeup
  コストが常時発生し、かつキャンセル反映が「次のポーリング周期まで
  遅延する」。長時間 idle な WS 接続では反映までの遅延がポーリング間隔
  分そのまま乗る。
- 案 B は `Notify::notify_waiters` の性質上、発火とタスクの subscribe に
  TOCTOU が生じうる（`session.rs` の受信ループが `ws.next()` の
  `Future` を pending 中でない一瞬に発火した場合、通知を取りこぼす）。
  flag 併読を必須契約にすれば救えるが、これは事実上「B と C の合成」で
  あり実装が複雑化するだけで A に対する優位性がない。
- **案 A は世代ごとに 1 個の `watch::Sender` を持ち、値そのものは常に
  最新の状態を保持し続ける**ため、B のような通知の完全な取りこぼしは
  起きない。ただし後述のとおり **`watch` の「変更通知」自体はレベル
  トリガではない**ため、消費側の実装を誤ると同種の TOCTOU を再導入
  しうる（次項「消費側の必須実装」）。

  **消費側の必須実装（TOCTOU 回避の要件）**: `tokio::sync::watch::
  Receiver::changed()` は「このレシーバがまだ観測していない、直近の
  `send` より新しい値」を待つ API であり、レシーバ生成時点の値は
  「既に観測済み」として扱われる。`Sender::subscribe()` で得た新規
  レシーバは、生成時点で「観測済み」初期化される実装のため、
  **`send(true)` が先に起きてから `subscribe()` した場合、
  そのレシーバの `changed()` は（次の `send` が来るまで）永久に
  解決しない**。これは 3 節冒頭で評価軸に挙げた「委譲確定と発火の
  競合」そのものであり、`changed()` を素朴に使うだけでは案 B と同じ
  取りこぼしを作り込んでしまう。

  この失敗を避けるため、**消費側は必ず「現在値を先に確認し、条件を
  満たしていなければ変更を待つ」実装（`Receiver::wait_for(|&v| v)`
  相当。現在値のチェックを内包し、既に `true` ならその場で即解決する）
  を使う**ことを設計上の必須要件として確定する。3.2 節のキャンセル
  `Future` はこの `wait_for` ベースの実装で構築することとし、単純な
  `changed()` の使用は誤り（バグ）として扱う。この要件を満たせば、
  「発火 → 委譲確定（レシーバ取得）」「委譲確定 → 発火」いずれの順序
  でも取りこぼしなく検出でき、8 節が要求するレベルトリガ相当の安全性が
  実際に成立する。

### 3.2 委譲境界での受け渡し型

`plugin-websocket` は `crates/core` に依存できない設計（2 節参照）ため、
案 A の `watch::Receiver<bool>` をどう境界越しに渡すかが独立の論点になる。

| 選択肢 | 概要 | Cargo 変更 | 受け入れ条件4「新規依存・feature 追加なし」との整合 |
|-------|------|-----------|----------------------------------------------------|
| **(i) キャンセル `Future` として渡す** | コアが `watch::Receiver` から構築した `Pin<Box<dyn Future<Output = ()> + Send>>` を `handle_upgrade` へ渡す | ゼロ（plugin-websocket は既存依存の `futures-util` で `select` 相当の race が可能） | 完全に整合（依存グラフ・Cargo 記述とも不変） |
| (ii) `watch::Receiver` を直接渡す | plugin-websocket の tokio features へ `sync` を追加 | `Cargo.toml` の記述変更を要する | `websocket` feature 有効時、コア（`crates/core`）が既に tokio `sync` を有効化しているため feature unification 上の**最終依存グラフは不変**。ただし plugin-websocket 単体の `Cargo.toml` 記述は変わり、`cargo tree -p fandhe-backend-plugin-websocket`（コア抜きの単体ビルド）でも `sync` が要求されるようになる |
| (iii) `Arc<AtomicBool>` を渡す | 案 C 相当を境界越しに持ち込む | ゼロ | 整合するが 3.1 節で棄却した案 C の限界（ポーリング律速）をそのまま引き継ぐ |

**採用: (i)**。理由:

- 受け入れ条件 4「新規依存・tokio feature 追加なしで実現可能」を、
  plugin-websocket クレート単体の `Cargo.toml` 記述レベルで**完全に**
  満たす。(ii) は最終ビルド成果物のバイナリには影響しないという主張が
  成立しうるが、「plugin-websocket が単体で（コアに依存せず）`sync`
  feature を要求するようになる」という記述変更自体は起こり、
  `docs/design/plugin-boundary.md` 6.1 節が明記する「本クレートは
  純関数 + 設定型のみを公開し、コアに依存しない」という既存の依存方向
  規約とも接触面が増える。(i) はこの接触面をゼロにできる
- `handle_upgrade` のシグネチャに `impl Future<Output = ()> + Send +
  'static`（または `Pin<Box<dyn Future<...>>>`）を 1 引数追加するだけで
  済み、`futures-util` は plugin-websocket の既存直接依存（`sink` /
  `std` feature）にトレイト境界を 1 つ増やす程度の変更に収まる
- コア側は `watch::Receiver::wait_for(|&v| v)`（3.1 節「消費側の必須
  実装」で確定した TOCTOU 回避パターン。現在値を先に確認してから待つ
  ため `changed()` 単体とは異なり取りこぼしが起きない）を
  `async move { let _ = rx.wait_for(|&v| v).await; }` で `Future` に
  包むだけであり、`try_handle_upgrade`（`pub(crate)`、自由に変更可）の
  内部実装として完結する

## 4. `UpgradeHandler` 契約変更の要否・後方互換

### 4.1 trait シグネチャ変更は不要

現行の委譲経路は 3 層構造になっている:

1. **`UpgradeHandler` trait**（`crates/core/src/extension.rs`、公開・
   安定契約）: `matches(&RequestHead) -> bool` による**委譲判定のみ**
2. **`try_handle_upgrade`**（`crates/core/src/plugin.rs`、`pub(crate)`・
   非公開シーム）: 判定成立後の実際の spawn・permit move を担う
3. **`fandhe_backend_plugin_websocket::handle_upgrade`**
   （`crates/plugin-websocket/src/lib.rs`、公開 API）: 実際のセッション
   処理

キャンセル伝播は「委譲判定」ではなく「委譲後の接続処理」に属する関心事
であり、責務は層 2・層 3 に閉じる。**層 1（`UpgradeHandler` trait）の
シグネチャ変更は不要**。この 3 層分離自体が、キャンセル機構の追加が
利用者向け公開契約（`UpgradeHandler` を実装する既存プラグイン・利用者
コード）に一切影響しないことを構造的に保証する。

### 4.2 後方互換戦略

変更が及ぶのは非公開シーム（層 2、自由に変更可）と公開 API（層 3）。
層 3 の変更方針を次のとおり確定する。

- **`fandhe_backend_plugin_websocket::handle_upgrade` は breaking change
  として扱う**（シグネチャへキャンセル `Future` 引数を追加する）。
  理由: (a) 現行 0.2.0 は crates.io 未公開でありイシュー #437 の
  breaking change 2 件に本件を追加するコストが小さい（`CHANGELOG.md`
  への追記は #491 の実装時に行う）、(b) 追加 API（例:
  `handle_upgrade_with_cancel`）による非破壊温存は、無期限キャンセル
  可能な `Future`（`std::future::pending()`）を渡す旧 API を残置する
  ことになり、`session.rs` 側で 2 経路の cancel-safety を維持し続ける
  保守コストが生じる。単一の必須引数に統一する方が
  `.claude/rules/coding-rust.md` の「AI ファースト保守性」（1 経路の
  方が誤用余地が小さい）に沿う
- 実際の呼び出し元を `grep -rn "handle_upgrade" --include='*.rs'
  crates/ examples/ templates/` で確認した結果、本番コードでの呼び出し
  元は `crates/core/src/plugin.rs` の `try_handle_upgrade`
  （非公開シーム、自由に変更可）**1 箇所のみ**である。一方、
  `crates/plugin-websocket/tests/`（`handshake_e2e.rs` /
  `handler_e2e.rs` / `idle_timeout.rs`）が `handle_upgrade` を直接
  駆動しており、これらは breaking change の実影響範囲に含まれる
  （#492 実装時にキャンセル `Future` 引数を追加した新シグネチャへ
  追随させる必要がある）。`crates/core/tests/websocket_upgrade.rs` 等
  コア側のテストは `try_handle_upgrade` 経由（層 2 まで）の統合テスト
  であり `handle_upgrade` を直接呼ばないため、この一覧には含まれない。
  breaking change の実影響範囲は「コア内部の呼び出し箇所 1 件 +
  plugin-websocket 自身の統合テスト 3 ファイル」に限定される
  （利用者向け公開 API・他プラグインからの呼び出しは存在しない）
- lockstep バージョニング（`docs/design/crates-io-release.md` 7.2 節）
  により、`fandhe-backend-plugin-websocket` の破壊的変更は次回
  publish 時に全 13 クレートへ同時反映される。#491 実装時に
  `CHANGELOG.md` の breaking change 一覧へ追記する

## 5. 世代との対応付け・発火タイミング

### 5.1 世代構造の拡張

`run_until` は現行、世代を「`shutdown_flag: Arc<AtomicBool>` +
`CancelSafeJoinSet`」の対として管理している。本設計では世代の構成要素へ
`watch::Sender<bool>`（初期値 `false`、キャンセル発火時に `true` を
`send`）を追加し、「`shutdown_flag` + `CancelSafeJoinSet` + キャンセル
`watch::Sender`」の三つ組を 1 世代として扱う。

- **最終 shutdown**（`run_until` 末尾）・**rebind 世代 drain**
  （`spawn_generation_drain`）の**両経路が同一の世代構造体（同一
  `watch::Sender`）を発火源として共有する**。`spawn_generation_drain`
  は現行 `old_join_set: CancelSafeJoinSet` のみを引数に取るが、これへ
  世代のキャンセル `watch::Sender`（またはそれを包む世代構造体）を
  追加で受け取るようシグネチャを拡張する
- WS 委譲タスクは spawn 時点でその世代の `watch::Receiver`（3.2 節の
  (i) によりキャンセル `Future` へ変換済み）を受け取り、
  `fandhe_backend_plugin_websocket::handle_upgrade` へ渡す

### 5.2 発火タイミングの意味論

3 案を比較する。

- (a) **grace 超過時のみ発火**: 5.4 節（`rebind.md`）の「grace 超過
  強制クローズへの包含」という #489 由来の要求に最も忠実。grace 期間中
  の WS セッションは通常どおり生存を継続する
- (b) drain 開始時に発火: grace 期間の頭から正常 Close を促すため、
  クライアントとの正常切断が grace 内で完了しやすくなる
- (c) 2 段階（drain 開始時に「正常 Close を試みよ」を通知しつつ、
  grace 超過時に強制打ち切り）

**採用: (c) の簡略形として、drain 開始時に 1 回だけ発火する（実質 (b)
と同じタイミング）が、WS セッション側の応答は「正常 Close を試みて
`WebSocketConfig::close_grace`（既定 10 秒）上限で打ち切る」という #492 が実装予定の有界動作に委ねる**。
理由:

- (a)（grace 超過時のみ発火）は、grace 期間中の待機を丸ごと「何もせず
  待つだけ」に費やしてしまう。WS セッション側が正常 Close を試みる
  猶予が実質ゼロ（grace 超過とほぼ同時に強制クローズが来る）になり、
  「Close frame を送信して切断する」という #492 の要件を満たす時間的
  余裕がない
- drain 開始時点で発火すれば、WS セッション側は `Server::
  shutdown_grace_period` の期間をまるごと正常 Close の試行に使える。
  `session.rs` の既存 `handle_idle_timeout` パターン（Close frame 送信
  → `WebSocketConfig::close_grace`（既定 10 秒）上限で応答待ち）をキャンセル経路にもそのまま適用でき、
  実装の一貫性が高い
- 「grace 超過時の強制クローズへの包含」という既存の不変条件は、
  `run_until` 自体の待ち合わせ（`docs/design/rebind.md` 5.2 節
  「セマフォ（`connection_limit`）は世代を跨いで単一共有」: permit は
  共有セマフォ経由のため grace 超過時は既存の `JoinSet::shutdown` 相当
  の強制クローズではなく **permit 回収 timeout** で担保される）で維持
  されるため後退しない。本設計が追加するのは「セッションに能動的な
  終了機会を与える」ことであり、「grace 超過後も必ず終わる」という
  既存フェイルセーフを置き換えるものではない（8 節参照）
- **rebind でも旧世代 WS セッションは正常 Close させる。opt-out は
  設けない**（発火タイミングを drain 開始時に統一する以上、rebind も
  最終 shutdown と同じ扱いとする設計判断。長時間生存する WS
  アプリケーションにとっては rebind のたびに接続が切られる挙動変化と
  なるが、これは「rebind は世代を切り替える操作である」という既存
  意味論（`docs/design/rebind.md` 1 節）に沿った振る舞いであり、
  接続を維持したいアプリケーションは rebind の実行タイミングを
  自身の運用側で制御することを前提とする）

### 5.3 最終 shutdown 経路での追加待機の要否

`run_until` の既存保証「`Server::shutdown_grace_period` + ε 以内に必ず
`Ok(())` で戻る」を本設計は壊さない。理由:

- `run_until` の grace 待ち自体は既存どおり `acquire_many_owned(
  permit_total)` の timeout で実装され続ける。WS セッションへの
  キャンセル発火は「permit 解放を早める」ための追加シグナルであり、
  `run_until` 側が WS タスクの終了を明示的に待つ新たな待機ステップを
  追加するわけではない
- キャンセル発火のタイミングを「shutdown_flag を true にする」直後
  （既存の 1 節 手順と同じ箇所）に揃えることで、`run_until` の制御
  フロー自体には分岐が増えない（既存の `shutdown_flag.store(true, ...)`
  の直後に `watch::Sender::send(true)` を追加するだけ）
- rebind の世代 drain も同様に `spawn_generation_drain` 内部で発火を
  追加するだけで、`run_until` 本体のブロッキング挙動は変わらない
  （`rebind.md` 5.1 節「`spawn_generation_drain` は `run_until` 自体を
  ブロックしない」を維持）

## 6. pay-for-what-you-use・依存検証

### 6.1 `websocket` feature 無効時

キャンセル配線のコード全体（世代構造体への `watch::Sender` 追加・
`try_handle_upgrade` でのキャンセル `Future` 構築・受け渡し）は、既存の
`#[cfg(feature = "websocket")]` シームに閉じ込める。`rebind.md` 3.3 節の
「遅延生成」パターンと同型で、`watch::channel` の生成自体も
`websocket` feature 有効時のみ行う（無効時は世代構造体からキャンセル
チャネル用フィールドを `cfg` で除外し、`run_until` の世代管理コードにも
`websocket` feature 無効時の分岐コストが一切乗らないようにする）。

### 6.2 `websocket` feature 有効だが未使用時

`Server::websocket` へ設定登録がない場合、`try_handle_upgrade` の
`websocket` 分岐は既存どおり `matches` が常に不成立で早期リターンする
ため、キャンセル `Future` の構築コード自体が実行されない。世代ごとの
`watch::channel` 生成は「feature 有効時は常に発生する軽量コスト」
（`watch::channel` はヒープ確保 1 回程度の低コスト）として許容する
（`websocket` config が 1 件も登録されていない場合でもチャネル自体は
世代ごとに生成されるが、これは既存の `shutdown_flag: Arc<AtomicBool>`
生成と同等のコストであり、既存設計が許容している水準を超えない）。

### 6.3 新規依存・tokio feature 追加なしの確認

- `tokio::sync::watch` はコアの既存 `sync` feature に含まれる
  （`tokio::sync::Semaphore` と同一 feature フラグ、`crates/core/
  Cargo.toml` の既存コメント参照）。**コア側の追加 feature・追加依存は
  ゼロ**
- plugin-websocket 側は 3.2 節で確定した (i) 案（キャンセル `Future`
  として受け渡す）により、既存依存の `futures-util`（`sink` / `std`
  feature、`WebSocketStream` の `StreamExt`/`SinkExt` 駆動に既に使用中）
  の範囲で完結する。`tokio` の `sync` feature を新規追加しない
- 検証コマンド（#491 実装時に実行、本イシューでは実行環境を確認する
  ステップとして記録のみ）:

```bash
# コア: websocket feature 無効時に watch 関連コードが一切現れないことを
# コードパス・依存双方で確認する（tokio 自体は既に共通依存のため
# `cargo tree` 単体では差分が出ない点に注意。#491 で cfg 分岐の
# コードレビューにより確認する）
cargo tree -p fandhe-backend-core                      # feature なし
cargo tree -p fandhe-backend-core --features websocket  # tokio の sync が
                                                          # 既存のまま（新規追加なし）

# plugin-websocket: tokio の sync feature が要求されないままであることを確認
cargo tree -p fandhe-backend-plugin-websocket -e features
```

## 7. 実装指針・受け入れ基準対応表（後続イシュー向け）

| 後続イシュー | 実装対象 | 本設計の対応節 |
|-------------|---------|---------------|
| #491（コア配線） | 世代構造体へキャンセル `watch::Sender` を追加、`try_handle_upgrade` でキャンセル `Future`（3.2 節 (i)）を構築して `handle_upgrade` へ渡す、`spawn_generation_drain` シグネチャ拡張、最終 shutdown・rebind 両経路での発火配線 | 5 節・6 節 |
| #492（Close frame 送信） | `fandhe_backend_plugin_websocket::handle_upgrade` へキャンセル `Future` 引数を追加（breaking change）、`run_session` の受信待ちとキャンセル `Future` を race させ、発火時は `handle_idle_timeout` と同型の Close ハンドシェイク（Close frame 送信 → `WebSocketConfig::close_grace`（既定 10 秒、イシュー #500 で設定可能化）上限で応答待ち）を実行 | 3 節・4.2 節・5.2 節 |
| #493（統合テスト・doc 更新） | 最終 shutdown・rebind 双方でのキャンセル伝播を検証する統合テスト、`BoundServer::run_until` doc「既知の限界」・`docs/design/graceful-shutdown.md` 8 節・`docs/design/rebind.md` 5.4 節/6 節の記述更新（本設計が解決したことを反映） | 全節 |
| #499（ハンドラ実行中・Reply 送出中の即時反映） | `run_session` のユーザーハンドラ呼び出し（Text/Binary）・`apply_outcome` の `ws.send`/`ws.close` を `race_cancel` で包み、キャンセル発火時に打ち切って `handle_cancellation` へ分岐 | 10 節 |

### 受け入れ条件との対応

1. **方式比較（最低 2 案）と採用理由**: 3 節（シグナル方式 4 案）・
   3.2 節（受け渡し型 3 案）
2. **`UpgradeHandler` 契約変更の要否・後方互換影響**: 4 節
3. **最終 shutdown / rebind 世代 drain 両経路をカバーする設計**: 5 節
4. **新規依存・tokio feature 追加なしで実現可能なことの確認**: 6 節

## 8. セキュリティ考慮事項（OWASP Top 10 観点）

- **リソース枯渇（DoS）— 本設計の主目的**: 長時間 WS 接続が permit を
  握り続けると、rebind 反復時に世代を跨いで `max_connections` の permit
  が消費され続け、最終 shutdown も grace 内に完了しない
  （`run_until` 自体は permit 回収 timeout により grace + ε 以内には
  必ず戻るが、握られた permit 分だけ新規接続の余地が実質的に減る）。
  キャンセル伝播はこの資源解放経路を確立する DoS 耐性の強化である
- **フェイルクローズの維持**: キャンセルシグナルの取りこぼし・WS タスク
  の非応答があっても、既存保証（`run_until` は permit 回収 timeout に
  より grace + ε 以内に必ず戻る）を設計の不変条件として維持する。
  キャンセル機構は既存フェイルセーフの上に足すものであり、置き換えない
  （5.3 節）
- **シグナル取りこぼし競合**: 委譲確定とキャンセル発火の競合（発火後に
  subscribe するタスクが通知を失う TOCTOU）を採用方式の決定打とした
  （3.1 節）。ただし `watch::Receiver::changed()` 自体はレベルトリガ
  ではなく、「レシーバ生成後に届いた変更」のみを検出する API である
  （生成時点で既に発火済みの値は「観測済み」扱いになり、単純な
  `changed()` は待ち続けてしまう）。この落とし穴を踏まえ、消費側は
  必ず現在値を先に確認してから待つ実装（`wait_for(|&v| v)`）を使う
  ことを設計上の必須要件とした（3.1 節「消費側の必須実装」）。この
  要件を満たす限り、委譲タスクが `watch::Receiver` を取得した時点で
  既に発火済みであっても即座に解決し、取りこぼしが起きない
- **攻撃表面・供給網**: 新規依存・feature 追加ゼロ（6 節）により攻撃
  表面・供給網リスクは増加しない
- **情報漏えい**: 本イシューは docs のみで実行コードを含まない。設計上
  も、キャンセル起因の Close frame（#492 実装予定）へ内部状態・機密を
  含めない（固定の close code・理由句のみ）ことを実装指針として明記する
- **シークレット**: 本ドキュメント・関連コミットに機密情報は含まれない

## 9. スコープ外（本イシューに混入させない）

- キャンセル配線の実装（#491）・`plugin-websocket` の Close frame 送信
  実装（#492）・統合テストと `run_until` doc「既知の限界」更新（#493）
- `webrtc-proxy` 等、WebSocket 以外の Upgrade/長時間接続プラグインへの
  同機構の水平展開。将来必要になった場合は
  [[out-of-scope-tracking]] に従いユーザー承認を得て別途 Issue 化する
- rebind の accept backlog 喪失（`docs/design/rebind.md` 6 節の別の
  既知の限界。本設計とは独立の課題）

いずれも [[out-of-scope-tracking]] に従い、Issue 化はユーザー承認を得て
から行う。

## 10. ハンドラ実行中・Reply 送出中のキャンセル意味論（#499）

#492 時点では `run_session` の**受信待ち**（`ws.next()`）でのみキャンセルを
最優先ポーリングしており、ユーザーハンドラ（`WsMessageHandler::on_message`）
の `await` 中・`WsOutcome::Reply`/`WsOutcome::Close` の送出中（`ws.send`/
`ws.close`）はキャンセルを観測しない既知の制約があった（`session` モジュール
doc に明記済み）。長時間かかるハンドラ・送信バッファ満杯の slow client が
あると、キャンセル反映が次の受信待ち復帰まで遅延し、grace 内のクローズが
遅れる（permit 解放の遅延）。本節はこの制約を解消する設計判断を記録する。

### 10.1 意味論の 3 案比較

| 案 | 概要 | 評価 |
|----|------|------|
| (a) 完走を待つ（現状） | ハンドラ・送出完了後の次の受信待ちで反映 | 本イシューが問題視する挙動そのもの。長時間ハンドラで grace を食い潰す |
| **(b) 即時打ち切り（採用）** | `race_cancel` でハンドラ Future・送出 Future を race し、キャンセル発火時に drop して `handle_cancellation` へ即分岐 | 5 節が採用した「drain 開始時に発火し、grace 期間をまるごと正常 Close の試行に使う」意味論と整合する。Future の drop は `tokio::select!`/`tokio::time::timeout` と同型の Rust async 標準のキャンセル意味論で、追加の設定・タイマー不要 |
| (c) 上限付きで待つ | キャンセル発火後もハンドラを上限 X 秒まで poll し続け、超過で drop | 新たな時間定数が増え、grace 内クローズの遅延が X 秒分残る。外側に permit 回収 timeout のフェイルセーフが既にあるため、中間の猶予層は複雑さに見合う利得がない |

(b) を採用する。5 節の意味論（drain 開始時に発火し grace をまるごと正常
Close の試行に使う）と、ハンドラ完走待ち（案 a）は「grace を Close
ハンドシェイクに充てる」意図と矛盾するため案 a は棄却する。案 c は
`CLOSE_GRACE`（10 秒、外側フェイルセーフ）と別に新たな待機上限を持ち込み、
既存の 2 層フェイルセーフ構造（`run_until` の permit 回収 timeout・
`CLOSE_GRACE`）に 3 層目を追加するだけの複雑さに見合わない。

### 10.2 ハンドラ Future の中断安全性契約

Future の drop によるキャンセルは Rust async の標準機構であり、ハンドラ
実装者への契約は次のとおり明記する（`WsMessageHandler::on_message` の
doc・`.claude/rules/coding-rust.md` の並行性規約と同一原則）:

- `on_message` が返す `Future` は shutdown/rebind 時に**任意の `await` 点で
  drop されうる**
- 中断されては困る副作用（完了保証が必要な書き込み等）は `tokio::spawn` で
  セッションから切り離して実行する（既存の「並行処理したい場合は自前に
  `tokio::spawn` する」建て付けと同一）
- キャンセル発火済みでメッセージ受信済みの場合、`race_cancel` はキャンセル
  最優先のためハンドラは呼ばれない

### 10.3 Reply 送出中の打ち切りのワイヤ安全性

`ws.send()` の Future を drop しても、フレーミングバッファ（書き込み位置を
含む）は Future ではなく `WebSocketStream` 本体が保持するため、後続の
`ws.close()` が未送出バイトの続きから flush する。フレーム途中で切れた
不正バイト列が独立に送出されることはない。この性質は
`crates/plugin-websocket/tests/cancellation.rs` の統合テストで
「打ち切り後もクライアントが有効な Close frame を受信できる」ことを
検証する（tokio-tungstenite の `Sink<Message>` 実装が内部バッファを
Future 跨ぎで保持する挙動に依拠する）。

### 10.4 実装への反映

`crates/plugin-websocket/src/session.rs` の `run_session` のハンドラ呼び出し
（Text/Binary）を `race_cancel` で包み、キャンセル発火時は `handle_cancellation`
へ分岐する。`apply_outcome` は戻り値を `Result<bool, WsError>` から
`SessionFlow`（`Continue`/`Closed`/`Cancelled` の 3 値）へ拡張し、
`WsOutcome::Reply` の各 `ws.send`・`WsOutcome::Close` の `ws.close` を
`race_cancel` で包む。既存の `handle_cancellation` → `close_and_drain`
（`CLOSE_GRACE` 有界化・`ConnectionClosed`/`AlreadyClosed` 許容）は無変更で
共有する。
