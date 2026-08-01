# Rust コーディング規約

fandhe-backend は「最小コア + Cargo feature 駆動プラグイン」を核とする Rust cargo workspace。
軽量・高速・高並行・攻撃表面最小を最優先する。

## エディション・ツールチェーン

- stable ツールチェーンを基本とし、fuzz / サニタイザは nightly を明示的に使う
- `cargo fmt`（rustfmt）で整形、`cargo clippy -- -D warnings` を CI ゲートとする

## 設計原則

- **pay-for-what-you-use**: 機能はコア外に置き Cargo feature で着脱する（[[pay-for-what-you-use]]）
- **拡張点は 4 種 trait に集約**: `Middleware` / `UpgradeHandler` / `RequestGate` /
  `Interceptor`。新機能はまずこの拡張点に載るか検討する。前 3 者は同期 trait
  （`crates/core/src/extension.rs`）、`Interceptor` はリダイレクト・レスポンス改変を
  扱う feature ゲート不要のレスポンダ系シーム（`crates/core/src/interceptor.rs`、
  `docs/design/interceptor-extension-point.md`）
- コアに重い依存・不要な `unsafe` を持ち込まない。依存追加は `reference-researcher` で妥当性を確認

## 安全性

- `unsafe` は最小限。使う場合は `// SAFETY:` コメントで不変条件と安全性の根拠を必ず書く
- `.unwrap()` / `.expect()` はライブラリコードで避け、`Result` / `?` でエラーを伝播する
- panic はライブラリ境界を越えさせない。エラー型は `thiserror` 等で明示する

## 並行性

- Tokio 上でブロッキング処理を await スレッドで実行しない（`spawn_blocking` を使う）
- 共有状態は `Arc` + 適切な同期プリミティブ。ロック保持中の `.await` を避ける
- `Middleware` 実装は同期ブロッキング I/O を行わない（非同期チャネルへの送信・別
  タスクでの I/O 実行に留める）。PoC-3 実測根拠・実装パターンの詳細は `AGENTS.md`
  を参照

## テスト・ドキュメント

- 公開 API には doc test を付け、`cargo test` で検証可能にする（AI ファースト保守性）
- コメント・doc comment は [[code-comment-style]] に従う
- 命名・モジュール構成は周辺コードのスタイルに合わせる

言語の詳細リファレンスは `rust` skill を参照。
