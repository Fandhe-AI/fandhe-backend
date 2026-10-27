# 利用者向けガイド

`docs/guide/` は fandhe-backend を**使う**ための入口です。
内部の要件・設計記録（`docs/design/`・`docs/spec/`）とは責務が異なり、
本ディレクトリは「どう使うか」だけを扱います。

## 対象読者

`docs/spec/04-requirements.md` の適用範囲に合わせ、次の 3 層を読者として想定します。

- **一次消費者**: Fandhe 内製サービスからフレームワークを使うチーム
- **二次消費者**: 一次消費者が構築したサービスを利用するチーム
- **外部ユーザー**: OSS 公開後に本リポジトリを直接利用するユーザー

いずれの読者も、まずは [`getting-started.md`](./getting-started.md) から読み進めてください。

## 文書一覧

| 文書 | 内容 |
|------|------|
| [`getting-started.md`](./getting-started.md) | crates.io からの依存追加〜最小サーバ起動〜動作確認までの最短手順 |
| [`feature-samples.md`](./feature-samples.md) | Cargo feature（websocket / graphql / openapi / webrtc 系 / tracing / hub-wiring）ごとの最小サンプルと実行手順 |
| [`tutorial.md`](./tutorial.md) | 最小サーバ→拡張点の実装→feature 有効化まで段階的に学ぶチュートリアル |
| [`jwks-refresh.md`](./jwks-refresh.md) | `fandhe-backend-plugin-jwt` の `jwks-refresh` feature。JWKS 取得関数の実装例（SSRF・サイズ上限・時間切れの契約）と取り直しタスクの配線 |
| [`dpop.md`](./dpop.md) | `fandhe-backend-plugin-jwt` の `dpop` feature。DPoP 結合トークンの検証の設定・配線例、`jti` を共有しない既知の限界 |
| [`ws-gate-extensions.md`](./ws-gate-extensions.md) | ゲートの検証結果を WebSocket のフック・`on_open`・`on_message_with_ctx`・`on_close` から読む方法と、セッション中は `exp` を再検証しない既知の限界 |

## サンプルコードの原則（二重管理をしない）

本ガイドはサンプルコードの全文を markdown に複製しません。実行可能なサンプルは
`crates/core/examples/*`（`cargo run --example <name>` で実行できる）と
`crates/core/src/lib.rs` のクレート doc（`cargo test --doc` で検証される doc test）を
「正」とし、本ガイドはそれらへの導線と実行手順のみを提供します。
markdown に複製したコードは `cargo test --doc` の検証対象にならずドリフトするため
（AI ファースト保守性、[`AGENTS.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/AGENTS.md) 参照）、変更が入った場合は
サンプル側を更新すればガイドの記載（コマンド・パス・feature 名）はそのまま有効です。

例外として [`jwks-refresh.md`](./jwks-refresh.md) の取得関数の例は、利用側が選ぶ HTTP
クライアントに依存するためフレームワークの依存に加えられず、コンパイル検証の対象外の
説明用コードです。フレームワーク側の API の呼び出し形は rustdoc の doc test を「正」とします。
[`dpop.md`](./dpop.md) の設定・配線の例も同様に、コンパイル検証の対象外の説明用コード
（`rust,ignore`）で、`JwtAuthenticatorBuilder::dpop` などの doc test が「正」です。
[`ws-gate-extensions.md`](./ws-gate-extensions.md) の例も、hub-wiring・plugin-jwt の型を使うため
コンパイル検証の対象外（`rust,ignore`）で、`WsOpenContext::extensions` などの doc test が「正」です。

`crates/core/examples/*` は feature 単体の実装パターンを示す最小 example ですが、
独立したプロジェクトとしてそのまま `cargo run` したい場合は
[`examples/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/examples/)
（Next.js 流の `with-<feature>` 独立サンプル集）を、複数 feature を組み合わせた
実運用形の雛形は
[`templates/app/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/templates/app/)
を参照してください。サンプル置き場の重複回避方針の詳細は
[`examples/README.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/examples/README.md)
にまとめてあります（本節では再掲しません）。

## 設計・要件との対応

- 実装がどう作られているかは [`docs/design/`](https://github.com/Fandhe-AI/fandhe-backend/tree/main/docs/design/) を参照（例:
  [`plugin-boundary.md`](https://github.com/Fandhe-AI/fandhe-backend/blob/main/docs/design/plugin-boundary.md) はプラグイン境界パターンの
  詳細）
- 要件・受け入れ基準は [`docs/spec/04-requirements.md`](https://github.com/Fandhe-AI/fandhe-backend-spec/blob/main/04-requirements.md) を参照
