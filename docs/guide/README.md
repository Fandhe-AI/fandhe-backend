# 利用者向けガイド

`docs/guide/` は fandhe-backend を**使う**ための入口です。
`docs/design/`（どう作るか＝実装設計判断の記録）・`docs/spec/`（何を作るか＝要件・
ロードマップの submodule）とは責務が異なり、本ディレクトリは「どう使うか」だけを扱います。

## 対象読者

`docs/spec/04-requirements.md` の適用範囲に合わせ、次の 3 層を読者として想定します。

- **一次消費者**: Fandhe 内製サービスからフレームワークを使うチーム
- **二次消費者**: 一次消費者が構築したサービスを利用するチーム
- **外部ユーザー**: OSS 公開後に本リポジトリを直接利用するユーザー

いずれの読者も、まずは [`getting-started.md`](./getting-started.md) から読み進めてください。

## 文書一覧

| 文書 | 内容 |
|------|------|
| [`getting-started.md`](./getting-started.md) | クローン〜ビルド〜最小サーバ起動〜動作確認までの最短手順 |
| [`feature-samples.md`](./feature-samples.md) | Cargo feature（websocket / graphql / openapi / webrtc 系 / tracing / hub-wiring）ごとの最小サンプルと実行手順 |
| [`tutorial.md`](./tutorial.md) | 最小サーバ→拡張点の実装→feature 有効化まで段階的に学ぶチュートリアル |

## サンプルコードの原則（二重管理をしない）

本ガイドはサンプルコードの全文を markdown に複製しません。実行可能なサンプルは
`crates/core/examples/*`（`cargo run --example <name>` で実行できる）と
`crates/core/src/lib.rs` のクレート doc（`cargo test --doc` で検証される doc test）を
「正」とし、本ガイドはそれらへの導線と実行手順のみを提供します。
markdown に複製したコードは `cargo test --doc` の検証対象にならずドリフトするため
（AI ファースト保守性、[`AGENTS.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/AGENTS.md) 参照）、変更が入った場合は
サンプル側を更新すればガイドの記載（コマンド・パス・feature 名）はそのまま有効です。

## 設計・要件との対応

- 実装がどう作られているかは [`docs/design/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/docs/design/) を参照（例:
  [`plugin-boundary.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/plugin-boundary.md) はプラグイン境界パターンの
  詳細）
- 要件・受け入れ基準は [`docs/spec/04-requirements.md`](https://github.com/Fandhe-AI/fandhe-backend-spec/blob/main/04-requirements.md) を参照
