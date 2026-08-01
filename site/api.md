# API リファレンス

API Reference セクションは fandhe-backend の公開 API を横断的に俯瞰するための
索引です。個々のシグネチャ・doc test を含む一次情報源は各クレートの rustdoc
（`cargo doc`）であり、本セクションの各ページはそれぞれ「全体像・契約・feature
前提を俯瞰する読み物」として rustdoc への導線を担います。本ページと個別ページの
記述が食い違う場合は常に rustdoc を正としてください。

## 収録ページ

- [サーバ API（core）](../docs/api/server-api.md) — `fandhe-backend-core` の
  `Server` ビルダー・`BoundServer`・`Handler` trait・`streaming` モジュールの
  契約と feature 前提
- [同期 3 拡張点契約](../docs/api/extension-api.md) — `extension` モジュールが公開する
  同期 3 trait（`Middleware` / `UpgradeHandler` / `RequestGate`）の契約・呼び出し
  タイミング
- [Interceptor 契約](../docs/api/interceptor-api.md) — 4 種目の拡張点 `Interceptor`
  （インターセプト・レスポンス改変）の契約・評価順序
- [HTTP プリミティブ API（http）](../docs/api/http-api.md) — `fandhe-backend-http`
  の公開 API・モジュール間の契約・DoS 上限の俯瞰
- [ルーティング API（routes）](../docs/api/router-api.md) — `Router` と関連型の
  ルート登録・ディスパッチ契約
- [プラグイン設定 API](../docs/api/plugin-config-api.md) — 各プラグイン
  （`crates/plugin-*`）の feature 名・登録方法・Config 型・既定値の横断一覧

curl 例を含む使い方の詳細は [feature 構成別サンプル](../docs/guide/feature-samples.md)
を参照してください（本セクションでは重複させません）。GitHub 上の実体は
[`docs/api/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/docs/api)
を参照してください。
