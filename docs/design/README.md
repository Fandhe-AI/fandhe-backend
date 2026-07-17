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
  対応付けて記述）
- [`unsafe-deny-lints.md`](./unsafe-deny-lints.md): 危険な `unsafe` パターンの deny lint 設定
  （TASK-14.2、#40、REQ-14。forbid/deny 2 層 lint テーブルの選定根拠とネガティブ検証）
- [`review-gate.md`](./review-gate.md): レビューゲート運用定義・受け入れテスト
  （TASK-14.3、#41、REQ-14。PR 必須化・force push/削除禁止の ruleset 拡張と受け入れテスト実施記録）
