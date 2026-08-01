# ガイド一覧

Guides セクションは fandhe-backend を**使う**ための利用者向けガイド群です。
「どう作るか」を扱う `docs/design/` や「何を作るか」を扱う `docs/spec/` とは
責務が異なり、本セクションは「どう使うか」だけを扱います。まず読む対象を
決めかねる場合は [Getting Started](../docs/guide/getting-started.md) から
着手してください（クローン〜ビルド〜最小サーバ起動〜動作確認までの最短手順）。

## 収録ガイド

- [ガイドの読み方](../docs/guide/README.md) — 対象読者（一次消費者・二次消費者・
  外部ユーザー）の想定、`docs/design/`・`docs/spec/` との責務分離、サンプルコードを
  markdown に複製しない原則
- [Getting Started](../docs/guide/getting-started.md) — クローンから最小サーバ
  起動・動作確認までの最短手順（crates.io 版・リポジトリクローン版の両方を収録）
- [feature 構成別サンプル](../docs/guide/feature-samples.md) — websocket /
  graphql / openapi / webrtc 系 / tracing 等、Cargo feature ごとの有効化方法・
  実行可能なサンプル・pay-for-what-you-use の検証手順
- [チュートリアル](../docs/guide/tutorial.md) — 最小サーバから始め、拡張点
  （`Middleware`）の実装、feature 有効化までを段階的に学ぶ
- [拡張点自作ガイド](../docs/guide/extension-points.md) — 4 拡張点
  （`Middleware` / `UpgradeHandler` / `RequestGate` / `Interceptor`）の契約と自作手順
- [レスポンスストリーミング](../docs/guide/streaming.md) — chunked
  ストリーミング送信（`handle_streaming`）の使い方
- [graceful shutdown](../docs/guide/graceful-shutdown.md) —
  `BoundServer::run_until` による安全な停止手順と grace 上限設定

GitHub 上の実体は
[`docs/guide/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/docs/guide)
を参照してください。
