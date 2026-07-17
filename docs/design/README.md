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
  テストの一貫改修フロー（TASK-12.2-1、#81、REQ-12(b)。改善提案フローと対になる、
  外部からの機能要求を起点とするフロー。受付形式・要求解釈・影響範囲判定・実装・
  テスト追加・検証ゲートの各段階と #82・TASK-12.3 との境界を記述）
