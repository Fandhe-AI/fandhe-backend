# pay-for-what-you-use 原則

backend-framework の設計の核。**使わない機能のコストを一切払わせない**。
feature を無効化したら、その依存・コード・`unsafe`・バイナリサイズ増をすべてゼロにする。

## 守ること

- 機能はコアではなくプラグインに置き、`#[cfg(feature = "...")]` で厳密にゲートする
- 依存 crate は対象 feature 有効時のみ有効化する（`optional = true` + `dep:` / feature 依存）
- feature 無効時に当該依存が `cargo tree` に**出ないこと**を確認する
- feature 無効時に当該コードパス・`unsafe` がバイナリに含まれないこと

## 検証

| 検証 | コマンド |
|------|---------|
| 依存の残留確認 | `cargo tree`（feature 無効構成で当該依存が出ないこと） |
| `unsafe` 件数の増減 | `cargo geiger` |
| バイナリサイズ比較 | feature 有効/無効でビルドしサイズ差を確認 |
| 全構成ビルド | feature なし・個別・全 feature の各構成で `cargo build` |

## アンチパターン

- コアに「とりあえず」機能を足す（後で feature に切り出すのは高コスト）
- feature ゲート漏れで無効時にも依存が残る
- 複数プラグインで暗黙に共有される重い依存をコアへ押し上げる

feature ゲートの実装は `plugin-builder`、違反検出は `reviewer` / `security-auditor` が担う。
Rust 側の詳細は [[coding-rust]] を参照。
