# fandhe-backend

**fandhe-backend** は、AI によるセキュリティ脆弱性発見リスクに備えて Rust で
新規構築された、軽量・高速・高並行なバックエンドフレームワークです。axum 級の
性能を目標に、**最小コア + Cargo feature 駆動プラグイン**設計で、WebSocket /
GraphQL / WebRTC / OpenAPI 自動生成 / 可観測性などを段階的に拡張できます。

## 2 つの核となる原則

- **pay-for-what-you-use**: feature を無効化したら、その依存・コード・`unsafe`・
  バイナリサイズ増をすべてゼロにします。使わない機能のコストを一切払わせません。
- **AI ファースト保守性**: doc test・網羅テスト・CI ガードレールを整備し、
  AI エージェントが安全に保守できる状態を保ちます。

## 主な構成要素

- **最小コア**（`fandhe-backend-core`）: HTTP/1.1 サーバと 3 種の拡張点
  （`Middleware` / `UpgradeHandler` / `RequestGate`）のみを持つ軽量コア
- **feature 駆動プラグイン**: websocket / graphql / openapi / webrtc 系 /
  tracing / cors / compression / static の各プラグインを Cargo feature で着脱
- **CI ガードレール**: 依存監査・unsafe 集計・性能ベンチ・ファジングを CI で継続実行

## ドキュメントの歩き方

- [Getting Started](/fandhe-backend/getting-started/) — クローンから最小サーバ
  起動・動作確認までの最短手順
- [ガイドの読み方](/fandhe-backend/guides/) — 利用者向けガイド全体の入口
- [feature 構成別サンプル](/fandhe-backend/guides/feature-samples/) — feature
  ごとの最小サンプルと pay-for-what-you-use の検証手順
- [チュートリアル](/fandhe-backend/guides/tutorial/) — 最小サーバ→拡張点の実装→
  feature 有効化まで段階的に学ぶ

ソースコードは [GitHub リポジトリ](https://github.com/Fandhe-AI/fandhe-backend)
で公開されています（MIT OR Apache-2.0 デュアルライセンス）。
