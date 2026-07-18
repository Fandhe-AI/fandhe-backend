# docs/design/ について

`docs/design/` は、本リポジトリ側で管理する実装フェーズの設計ドキュメント置き場である。
`docs/spec/`（submodule、[Fandhe-AI/backend-framework-spec](https://github.com/Fandhe-AI/backend-framework-spec)）
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
- [`gray-zone-feasibility-verification.md`](./gray-zone-feasibility-verification.md):
  グレーゾーン（条件付き可）タスクを含めた可否判定再検証プロトコル（TASK-12.6、#47、
  REQ-12、Conditional Go 条件(3)。`third-party-feasibility-verification.md` が除外した
  「条件付き可」を主対象に、可・不可・要エスカレーションとの上下境界を含む N=10 の
  タスクセット（G-01〜G-10）・`third-party-feasibility-verify.sh` の後方互換拡張
  （4 値受理・`check_conditional_fields`・`--task-ids`）を記述）
- [`fuzzing.md`](./fuzzing.md): fuzz 実行環境（nightly / 代替 fuzzer）の整備
  （TASK-15.3-1、#87、Conditional Go 条件(4)。cargo-fuzz 選定根拠・nightly pin 方針・
  fuzz target 一覧・smoke（CI 常設）と本実行（#88）の 2 段構えを記述）
- [`http-buffer-reuse-tcp-nodelay.md`](./http-buffer-reuse-tcp-nodelay.md): `bf-http`
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
  `bf-plugin-openapi` の非該当理由を記述。受け入れテストは
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
- [`framework-naming.md`](./framework-naming.md): フレームワーク正式名称の決定記録
  （#92。命名基準・候補評価マトリクス・決定候補 `wrenframe`（レビューゲート確定待ち）・
  `bf-` プレフィックスからの 4 段階移行計画・責務分界（リポジトリ名変更・crates.io
  名称確保は人間管理者実施）を記述）
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
