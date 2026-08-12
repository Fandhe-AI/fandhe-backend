# docs/design/ について

`docs/design/` は、本リポジトリ側で管理する実装フェーズの設計ドキュメント置き場である。
`docs/spec/`（submodule、[Fandhe-AI/fandhe-backend-spec](https://github.com/Fandhe-AI/fandhe-backend-spec)）
が要件定義・タスク一覧・ロードマップ・PoC 結果という「何を作るか・なぜ作るか」を扱う仕様書であるのに対し、
`docs/design/` は個別タスクの実装着手時に確定させる「どう作るか」の設計判断を記録する。

- `docs/spec/` の内容を書き換えない。設計ドキュメントから `docs/spec/**` へ根拠を相対リンク・参照するのみ
- 各設計ドキュメントは対応する `docs/spec/05-tasks.md` のタスク ID（例: TASK-8.2）・要件 ID（例: REQ-8）と対応付ける
- 実装が進み設計が確定・変更された場合はこのディレクトリを更新する（`docs/spec/` 側の PoC 記録は事後に書き換えない）

## 現在のドキュメント

- [`webrtc-process-isolation.md`](./webrtc-process-isolation.md): WebRTC プラグインの別プロセス切り出し設計
  （TASK-8.2-1、REQ-8・Conditional Go 条件(2) 対応）
- [`ci-completion-criteria.md`](./ci-completion-criteria.md): CI 完遂判定基準の実装
  （TASK-14.1、#39、REQ-14。機械判定とレビューゲートの責務分界を記述）
- [`improvement-proposal-flow.md`](./improvement-proposal-flow.md): 改善提案フロー
  （TASK-12.1-2、#80、REQ-12(a)。検知 → トリアージ → 提案 → 承認 → 実装 → 検証ゲート →
  クローズの各段階と 4 分析軸の入力ソース対応を記述）
- [`feature-modification-flow.md`](./feature-modification-flow.md): 機能要求→実装→
  テスト→ドキュメント追随→完遂判定の一貫改修フロー（TASK-12.2-1/#81 + TASK-12.2-2/#82、
  REQ-12(b)。改善提案フローと対になる、外部からの機能要求を起点とするフロー。受付形式・
  要求解釈・影響範囲判定・実装・テスト追加・検証ゲート・ドキュメント追随・完遂判定の
  各段階と TASK-12.3 との境界を記述）
- [`feasibility-guardrail.md`](./feasibility-guardrail.md): 対応可否自律判断ガードレール
  （TASK-12.3-1、#83、REQ-12(c)。判定の 3 軸・判定区分 4 値・曖昧要求/未定義依存/安全性
  方針衝突/明確な脆弱性を招く要求の不可判定 4 カテゴリの基準を PoC-9 T-11〜T-15 と
  対応付けて記述。TASK-12.3-2、#84 で判定記録バリデータ `scripts/feasibility-check.sh` に
  よる機構組み込み・機械検証を 11 節に追記）
- [`unsafe-deny-lints.md`](./unsafe-deny-lints.md): 危険な `unsafe` パターンの deny lint 設定
  （TASK-14.2、#40、REQ-14。forbid/deny 2 層 lint テーブルの選定根拠とネガティブ検証）
- [`review-gate.md`](./review-gate.md): レビューゲート運用定義・受け入れテスト
  （TASK-14.3、#41、REQ-14。PR 必須化・force push/削除禁止の ruleset 拡張と受け入れテスト実施記録）
- [`third-party-feasibility-verification.md`](./third-party-feasibility-verification.md):
  可否判定正解率の第三者再検証プロトコル（TASK-12.4-2、#86、REQ-12。PoC-9 T-11〜T-15 の
  セルフ実験バイアスを排除するための 3 役分離・タスクセット N=10・機械採点ハーネスの
  設計を記述。「可」「不可・要エスカレーション」「不可（明確な拒否）」の 3 値のみを対象と
  し「条件付き可」は意図的にスコープ外。自律完遂率の再検証は TASK-12.4-1／#85 のスコープ）
- [`third-party-model-diversity-reverification.md`](./third-party-model-diversity-reverification.md):
  第三者検証のモデル多様性制約の恒久追跡（イシュー #262（**2026-07-19 に COMPLETED で
  クローズ済み**。後継の open 追跡先はイシュー #281）、REQ-12、Conditional Go 条件(3)。
  TASK-12.4 の実測定が Claude ファミリー内（別セッション・別 Claude モデル）に留まる制約を
  イシュー #241 で明記・サインオフした後、恒久追跡先がクローズ済みイシューのみだった問題
  （イシュー #252 検出）を受け、別ベンダー LLM・人間被験による再検証の実施条件と現行
  サインオフの有効範囲（暫定運用）を整理し、open な追跡先として #281 を関連文書から
  相互参照する。人間による限界受容の確定判断は同文書 6 節で PENDING 記録中）
- [`gray-zone-feasibility-verification.md`](./gray-zone-feasibility-verification.md):
  グレーゾーン（条件付き可）タスクを含めた可否判定再検証プロトコル（TASK-12.6、#47、
  REQ-12、Conditional Go 条件(3)。`third-party-feasibility-verification.md` が除外した
  「条件付き可」を主対象に、可・不可・要エスカレーションとの上下境界を含む N=10 の
  タスクセット（G-01〜G-10）・`third-party-feasibility-verify.sh` の後方互換拡張
  （4 値受理・`check_conditional_fields`・`--task-ids`）を記述）
- [`fuzzing.md`](./fuzzing.md): fuzz 実行環境（nightly / 代替 fuzzer）の整備
  （TASK-15.3-1、#87、Conditional Go 条件(4)。cargo-fuzz 選定根拠・nightly pin 方針・
  fuzz target 一覧・smoke（CI 常設）と本実行（#88）の 2 段構えを記述）
- [`http-buffer-reuse-tcp-nodelay.md`](./http-buffer-reuse-tcp-nodelay.md): `fandhe-backend-http`
  読み取りバッファ再利用・`TCP_NODELAY` 最適化（TASK-1.3-3、#68。`RecvBuffer` の
  遅延コンパクション・ゼロ埋め回避・容量有界化設計と、feature `net` の
  `socket::configure_stream`。TASK-1.4 / #70（接続受理ループ）への引き継ぎ事項を記述）
- [`plugin-boundary.md`](./plugin-boundary.md): feature flag + `dep:` 構文による
  プラグイン境界パターン（TASK-2.1、#18、REQ-2。feature 命名規約・cfg-free な
  コアループ + 固定シグネチャシームの規約・パスインターセプト型（`webrtc-proxy`
  で確立）/ Upgrade 型の適用指針・検証コマンドを記述）
- [`pay-for-what-you-use-check.md`](./pay-for-what-you-use-check.md): pay-for-what-
  you-use 機械検証（TASK-2.2、#19、REQ-2。`scripts/pay-for-what-you-use-check.sh` に
  よる cargo tree/geiger・バイナリサイズ・全構成ビルドの PASS/FAIL 判定設計、
  `dep-impact.sh` との役割分担、セルフテスト・CI 組み込みを記述）
- [`plugin-loading-tradeoffs.md`](./plugin-loading-tradeoffs.md): プラグインロード方式
  （コンパイル時 feature flag vs 実行時動的ロード）の安全性トレードオフ（TASK-2.4、#21、
  REQ-2。`unsafe`・ABI 安定性・監査容易性の 3 観点で比較し、コンパイル時方式の採用根拠と
  限界を記述。`crates/plugin-graphql` + 既存 `webrtc-proxy` の 2 プラグイン受け入れ検証は
  `docs/acceptance/req2-plugin-mechanism.md` を参照）
- [`webrtc-rs-version-strategy.md`](./webrtc-rs-version-strategy.md): `webrtc-rs` バージョン戦略
  （TASK-8.3、#28、REQ-8。v0.17.x（保守モード）継続採用・Sans-I/O 系（v0.20 系/`rtc` クレート）の
  移行トリガー基準・スコープ外機能の実装フェーズでの扱いを記述。ドラフト、最終承認は人間レビュー）
- [`extension-closure-verification.md`](./extension-closure-verification.md): 拡張点への
  変更影響範囲閉包の実例検証（TASK-13.1、#49、REQ-13。WebSocket/WebRTC/GraphQL の実 merge
  commit を `scripts/extension-closure-check.sh` で機械判定し、A〜D カテゴリへの閉包
  可否・E 判定時の理由（`crates/http/src/response.rs` の共有 reason phrase テーブルへの
  1 行追加）を記述。TASK-13.2/#50 への引き継ぎ事項も記載）
- [`dependency-graph-contract.md`](./dependency-graph-contract.md): 依存グラフ・契約
  ドキュメント（TASK-13.2、#50、REQ-13。正準依存グラフ（`dep-direction-check.sh` の
  `allowed_edge_patterns` からの転記）・3 拡張点 + `try_intercept` 固定シームの契約一覧・
  `crates/plugin-*/src/lib.rs` 冒頭の機械可読宣言規約（`拡張点対応: <値>`）・拡張点への
  非該当時の理由明記運用（`scripts/extension-closure-gate.sh` による PR ゲート）・
  `fandhe-backend-plugin-openapi` の非該当理由を記述。受け入れテストは
  `scripts/accept/req13-change-impact-accept.sh`、実行結果は
  `docs/acceptance/req13-change-impact.md`）
- [`openapi-typescript-pipeline.md`](./openapi-typescript-pipeline.md): openapi-typescript
  連携パイプライン（TASK-6.1、#54、REQ-6。「utoipa → openapi.json → openapi-typescript →
  TS 型 → openapi-fetch クライアント」の一方向パイプライン設計・クライアントライブラリ
  選定結果（`openapi-fetch` 採用理由と比較表）・`--check`/`--update` 運用・
  サプライチェーン対策・TASK-6.2（#55）への接続点を記述）
- [`tracing-integration.md`](./tracing-integration.md): `tracing` エコシステム連携方式
  設計（TASK-10.5、#60、REQ-10。`init_tracing`/`TracingOutput`/`WorkerGuard` 保持契約・
  non_blocking の lossy 特性、`TracingConfig` によるサンプリング設定・`exclude_path` に
  よる高頻度パス除外・記録粒度を上げる場合の拡張指針（method/path/elapsed_ms 限定の
  不変条件を維持）、依存インパクト要約（`docs/dep-impact/records.md` 参照）を記述）
- [`multi-trial-stability-verification.md`](./multi-trial-stability-verification.md): 複数回
  試行による結果安定性確認プロトコル（TASK-12.5、#46、REQ-12。TASK-12.4-1／TASK-12.4-2 の
  第三者検証プロトコルを無変更で引用しつつ、試行回数 K=3・試行間の条件統一・タスクセット
  v2 の前提事前検証手順・後出し防止・安定性判定基準（全試行での REQ-12 閾値充足 +
  レンジ記録）・集計ハーネス設計を記述）
- [`outbox-consent-integration.md`](./outbox-consent-integration.md): Outbox・同意ゲート
  実データモデル統合設計（TASK-9.4、#64、REQ-9。PoC-6 インメモリ実装（`Mutex<Vec<...>>`）
  から `micro-service-hub` の Outbox Relay・同意管理サービス（PostgreSQL）への統合方針。
  `OutboxStore`/`ConsentStore` trait による依存逆転維持・データモデル対応表・
  アプリ層 403/データ層フェイルクローズ 0 行の 2 層拒否設計・完了判定を「設計完了」と
  「E2E 統合検証完了」（#97、`micro-service-hub` Outbox Relay 完了 2026-09-30 以降）の
  2 段階に分ける方針を記述）
- [`crates-io-release.md`](./crates-io-release.md): crates.io 公開手順（イシュー #94、
  OSS 公開準備。名前確保（実体を伴う初回 publish・空クレート予約禁止）・所有権（GitHub
  org team 管理）・公開対象クレート区分表・publish フラグのフェイルクローズ運用・
  リリース CI 設計の YAML 草案（実ファイル化は名称確定後に別イシューへ切り出し）・
  バージョニング方針・公開前チェックリストを記述。前提条件（正式名称確定・
  リポジトリ public 化）が充足するまで実行しない設計文書）
- [`framework-naming.md`](./framework-naming.md): フレームワーク正式名称の決定記録
  （#92 で候補選定、#200/#201 で `fandhe-backend` に確定。決定根拠・可用性証跡・
  確定版新旧マッピング表（#202〜#205 の実装が参照）・段階的移行計画・旧候補
  `wrenframe` の選定経緯（履歴節として保持）・責務分界（リポジトリ名変更・
  crates.io 名称確保は人間管理者実施）を記述）
- [`ai-maintenance-operations.md`](./ai-maintenance-operations.md): AI 保守運用体制
  （Issue #93、REQ-11・REQ-12。`docs/spec/01-brainstorm.md` 未解決事項「設計思想 4 の
  運用像」に応え、実装済みの改善提案フロー・機能改修フロー・可否判定ガードレール・
  CI/scripts 群を「どの基盤で・どの頻度で・どの範囲に・誰の責任で」回すかという運用面
  として統合定義。hub 運用チームとの合意はレビューゲートで記録する方針を明記）
- [`versioning-policy.md`](./versioning-policy.md): バージョニング方針（semver・破壊的変更
  ポリシー）（#96、出典 #91。pre-1.0（0.x）の semver 運用・workspace 内 lockstep バージョン
  同期・v1.0 昇格基準、Rust 公開 API/Cargo feature（default 追加禁止）/拡張点 3 trait/ワイヤ
  契約を対象とした破壊的変更の定義・手続き（`feat!:`/`BREAKING CHANGE:`・非推奨期間）、
  pre-1.0/v1.0 以降のサポートポリシーを記述。`webrtc-rs-version-strategy.md`（独立 WebRTC
  サービス側のバージョン戦略）とは別軸。ドラフト、最終承認は人間レビュー）
- [`async-handler.md`](./async-handler.md): async ハンドラ対応の設計判断（イシュー #314、
  REQ-1。現行の同期ハンドラ契約（`Handler::handle`・`RouteHandler`・
  `ParamRouteHandler`）が sqlx 等の非同期 DB クライアントを構造的に使えない分水嶺である
  ことを踏まえ、async trait 化・別型併設・ハンドラのみ async 化（採用）・現状維持の 4 候補を
  dyn 互換性・性能影響・後方互換・拡張点契約への影響・実装コストで比較。3 拡張点
  （`Middleware`/`UpgradeHandler`/`RequestGate`）の同期契約は変更せず、`crates/
  plugin-websocket`/`crates/plugin-graphql` の boxed-future 型消去の先例に倣い新規依存を
  追加しない移行方針・性能影響予測とベンチ検証方法・DoS/panic 境界の安全性考慮・実装
  イシュー分解方針を記述）
- [`v1-scope-tls-multipart.md`](./v1-scope-tls-multipart.md): TLS 終端・multipart/
  form-data の v1 スコープ方針（イシュー #322。両者ともフレームワーク本体では扱わず、
  TLS 終端はリバースプロキシ前提、multipart は既存 body 上限内の raw バイト列受理に
  留める方針を明文化。`docs/spec/04-requirements.md` 除外事項表 #8・#9（upstream PR
  fandhe-backend-spec#2）への相対リンクと、個別要求が来た場合の feasibility-guardrail
  接続指針を記述）
- [`docs-site-redesign.md`](./docs-site-redesign.md): GitHub Pages docs サイト刷新の
  設計ドキュメント（イシュー #388、親トラッキング #384。fandhe-frontend の docs-site
  設計正典 3 本（3 カラムレイアウト・依存ゼロ全文検索・利用者向け API/内部設計記録
  分離）を fandhe-backend の文脈へ翻訳し、3 カラム DOM/class 契約・ダークモード・
  アクセシビリティ・コンテンツ構成・公開範囲規約（`docs/design/` を移設先とする判断）・
  検索インデックス仕様・CI 追随（`docs/api/**` の paths トリガー欠落含む）を #389〜#399
  向けに確定する）
- [`finalize-seam-public-api.md`](./finalize-seam-public-api.md): `finalize_response` /
  `finalize_streaming_head`（レスポンス後処理型シーム）の公開 API 化の採否検討
  （イシュー #462、PR #458 の out-of-scope から切り出し。`Interceptor::map_response`
  との棲み分け比較・ギャップ分析・不採用根拠 4 点・再検討条件を記述）
- [`ws-cancellation-propagation.md`](./ws-cancellation-propagation.md): WS 委譲タスク
  へのキャンセル伝播機構の設計（イシュー #490。最終 graceful shutdown（#313）・rebind
  世代 drain（#485/#488）双方の grace 超過強制クローズの対象外にある WebSocket 委譲
  セッションへキャンセルを伝播する機構を設計。世代別 `tokio::sync::watch` チャネル +
  委譲境界ではキャンセル `Future` として受け渡す方式の採用理由、`UpgradeHandler`
  シグネチャ変更不要の 3 層構造根拠、`fandhe_backend_plugin_websocket::handle_upgrade`
  の breaking change 方針、新規依存・tokio feature 追加なしの確認を記述。コード実装は
  後続の #491（コア配線）・#492（Close frame 送信）・#493（統合テスト・doc 更新）が担う）
- [`per-core-runtime-decision.md`](./per-core-runtime-decision.md): P5 per-core accept
  モデルの採否検討（イシュー #589、親 #581 Phase 2、ルート #579。actix-web/ntex 帯
  （約 54 万 RPS）到達に必要な構造を、accept 並列化（`SO_REUSEPORT` + コアごとの
  accept 経路、`Send` 契約維持、軸 A）とハンドラの `!Send` 許容（`Rc`/`RefCell`、
  軸 B）の独立した 2 軸に分解して評価。軸 A 単独は 4 拡張点 trait・`crates/routes`
  ハンドラ型・13 公開クレートのバージョニングに影響せず、accept/bind 層と graceful
  shutdown（#313）・rebind（#485/#488）・WS キャンセル（#489〜#499）・
  `SessionDrain`（#498）の 4 並行機構に影響範囲を限定できる。軸 B を伴う一般形のみ
  全公開契約・13 公開クレートの breaking change に波及し影響範囲を限定できないため、
  軸 A + 軸 B の一般形を不採用と結論（軸 A 単独案は否定せず、opt-in feature 化案の
  第一候補として再検討条件に明記）。fail-closed 原則を根拠に記録）
- [`zero-copy-request-head.md`](./zero-copy-request-head.md): P1 ヘッダゼロコピー化
  （`RequestHead` の Range 保持）の設計検討（イシュー #588、性能改善ツリー #579 Phase 2。
  `RequestHead` の公開 API 全量調査（72 参照ファイル）・ライフタイム設計 3 案比較
  （借用+Range／所有バッファ+Range／Cow ハイブリッド）・段階移行 3 案比較・alloc
  プロファイル実測（現状 N=10 で 27 alloc/req → 採用案で定数 2 alloc/req 見込み）・
  fuzz/DoS 上限/UTF-8 検証の不変条件・`unsafe` 不使用方針を記述。採用案（所有ヘッド
  バッファ + `Range<usize>` 保持、ライフタイムパラメータ非導入）と Phase 3 実装
  issue（#590〜#593）の受け入れ基準確定まで含む。コード変更は Phase 3 実装
  issue（ユーザー承認後着手）が担う）
- [`bench-hosted-runner.md`](./bench-hosted-runner.md): GitHub Actions 無料枠
  ホステッドランナーでのベンチ運用方式（イシュー #611、ベンチ判定安定化ツリー
  #607 の Phase 1 設計イシュー。issue593 9 節が観測した共有 self-hosted ホスト
  固有の p95 判定不安定性が、週次ベンチ（`bench-schedule.yml`、#554 でホステッド
  ランナーへ移行済み）移行後の全 run（成功・失敗を問わず母集団を列挙、n=3 が
  全数）の実測では再現しないことを確認し、方式 1（axum 比、現行）を主判定として
  維持しつつ、その構造的弱点（baseline・core が同方向に悪化する run を検知
  できない）を明示的な受容条件・再検討トリガ付きの残存リスクとして記録した上で、
  方式 2（交互ペア測定 + 外部 CPU 占有率プローブ）の役割を「方式 1 FAIL 時の
  退行帰属限定（#613）」、方式 3（決定的計測）を「補助指標（#615、実行カデンツ
  未確定）」、方式 4（しきい値統計的再設計）を「#612 へ委譲」と再確定した採用案を
  記述。無料枠制約（ジョブ 6 時間・ワークフロー 35 日・Free 同時 20 ジョブ）の
  整理、`.claude/rules/ci.md` 等との整合確認を含む）
- [`bench-p95-criteria.md`](./bench-p95-criteria.md): p95 基準運用（マージン・
  試行回数・判定不能の扱い）の見直し設計（イシュー #612、親 #608、ルート #607、
  Phase 1。`benches/reports/issue593-p1-zero-copy-bench.md` 9 節の実測（外れ値・
  境界 FAIL・外部負荷機械検証付き交互測定）を根拠に、判定 4 値化
  （INCONCLUSIVE 新設）・p95 axum 比のしきい値マージンによる判定不能帯・最大 3 試行
  多数決 + 交互測定二次判定・外れ値の客観的除外条件（分布逸脱 + 外因の証拠）を設計。
  spec 基準値（p95 <= 1.10）は不変、数値パラメータは #616 の較正ランで実測確定済み
  （値の変更なし・緩和なし、根拠は `benches/reports/issue616-hosted-runner-calibration.md` 参照）。
  設計のみでコード変更なし、実装は Phase 2（#613・#614）へ引き渡す）
