# crates.io 公開手順

イシュー #94「chore(global): OSS 公開準備（crates.io・LICENSE・CONTRIBUTING）」対応。
本ドキュメントは crates.io への公開手順（名前確保・所有権・リリース CI）を**定める**もの。
2026-07-21 に公開判断が下り、公開対象 13 クレートの `publish = false` 解除・メタデータ整備・
`cargo publish --workspace --dry-run` 全件成功・実 publish・リポジトリ public 化が
完了済み（4・5・8 節参照）。リリース CI の実ファイル化は引き続き別イシューで扱う
（`docs/design/README.md` の記載規約に従い、対応するタスク・要件 ID と紐付ける）。

## 1. 前提条件（公開ブロッカー）

以下がすべて完了し、実 publish（`cargo publish --workspace`）は 2026-07-21 に実施完了した。

1. **正式名称の確定**: `fandhe-backend` に確定済み（#200、
   [`docs/design/framework-naming.md`](./framework-naming.md) 参照。crate/import 改名は
   #202・PR #209 で反映済み）。本ブロッカーは解消済み
2. **公開判断**: 2026-07-21 にリポジトリオーナーから公開指示済み。本ブロッカーは解消済み
3. **公開対象クレートの最終選定**: 公開対象 13 クレートで確定済み（4 節の区分表参照）。
   本ブロッカーは解消済み
4. **リポジトリの public 化**: 2026-07-21 に public 化完了済み。README・doc comment 内の
   リンクを crates.io 上で解決可能にするため実施。本ブロッカーは解消済み
5. **`cargo login`**: crates.io への認証はユーザーが実施済み。トークンをリポジトリ・
   エージェント環境に残さない。本ブロッカーは解消済み

## 2. 名前確保

- crates.io には「予約だけの空クレート」を事前公開するネームスクワッティング防止ポリシーが
  ある。したがって空クレートによる事前予約は**行わない**
- 名前は実体を伴う初回 `cargo publish`（4 節の依存順に従った最初の公開）で確保する
- 正式名称確定後、公開前に必ず名前の空き状況を確認する:

  ```bash
  cargo search <crate-name> --limit 1
  # または https://crates.io/crates/<crate-name> を直接確認
  ```

- 確認対象は 4 節の公開予定クレート名すべて（`fandhe-backend-http` / `fandhe-backend-routes` /
  `fandhe-backend-core` 等）

## 3. 所有権

- crates.io の owner は**個人アカウントではなく GitHub org team**（
  `github:fandhe-ai:<team>`）で管理する。個人の異動・アカウント侵害が単一障害点に
  ならないようにするため（OWASP A01 アクセス制御対策）
- 初回 publish 後、team を owner として追加する:

  ```bash
  cargo owner --add github:fandhe-ai:<team> -p <crate-name>
  ```

- 個人 owner を作業用に一時追加した場合は、team 追加後に必ず削除し、個人 owner を
  残さない運用とする:

  ```bash
  cargo owner --remove <individual-account> -p <crate-name>
  ```

- `<team>` の具体名は org 側のチーム構成確定後に定める（本ドキュメントでは方針のみ固定する）

## 4. 公開対象クレートと publish フラグ

| クレート | 区分 | 理由 |
|---------|------|------|
| `fandhe-backend-http` | 公開対象 | HTTP プリミティブ（下位層）。単体でも再利用価値がある |
| `fandhe-backend-routes` | 公開対象 | ルーティング。`fandhe-backend-http` にのみ依存する中間層 |
| `fandhe-backend-core`（`crates/core`） | 公開対象 | 最小コア本体 |
| `fandhe-backend-plugin-websocket` / `fandhe-backend-plugin-graphql` / `fandhe-backend-plugin-openapi` / `fandhe-backend-plugin-webrtc` / `fandhe-backend-plugin-webrtc-proxy` / `fandhe-backend-plugin-tracing` / `fandhe-backend-plugin-hub-wiring` / `fandhe-backend-plugin-cors` / `fandhe-backend-plugin-compression` / `fandhe-backend-plugin-static` | 公開対象 | feature 駆動プラグイン本体（10 クレート） |
| `axum-ref` | 恒久非公開 | 性能比較用参照実装。フレームワーク利用者向け成果物ではない |
| `ws-load-client` | 恒久非公開 | WebSocket 負荷試験専用バイナリ |
| `docs-site` | 恒久非公開 | GitHub Pages ドキュメントサイト生成ツール（SSG）。開発者・CI 用でフレームワーク利用者向け成果物ではない |
| `crates/http/fuzz` | 恒久非公開（対象外） | cargo-fuzz 専用クレート。root workspace から `exclude` 済み（TASK-15.3-1、#87）であり、`cargo publish` の対象にも入らない |

- **公開対象 13 クレートは crates.io v0.1.0 として公開済み**（2026-07-21 実施。5 節参照）。
  恒久非公開 3 クレート（`axum-ref` / `ws-load-client` / `docs-site`）は `publish = false`
  で維持し、workspace `exclude` の `crates/http/fuzz` と合わせて公開物から除外
- 解除と併せて各公開対象クレートの `Cargo.toml` に次のメタデータを整備済み:
  - path 依存への `version = "0.1.0"` 併記（crates.io 公開には version 指定が必須）
  - `readme = "../../README.md"`（ルート README を各クレートの crates.io 掲載 README
    として同梱）
  - `keywords` / `categories` の付与、陳腐化していた `description` の更新
    （core / routes / http）
  - `fandhe-backend-plugin-hub-wiring` のテスト専用 RSA 鍵 `tests/fixtures/*.pk8` は
    公開物に**同梱する**。src/（`#[cfg(test)]`）・tests/・examples/ が
    `include_bytes!` でコンパイル時に参照しており、除外すると公開版クレートの
    `cargo test`・examples ビルドがコンパイル不能になるため（PR #350 Bugbot 指摘で
    当初の除外方針を取り消し）。鍵は `tests/fixtures/README.md` に「テスト専用・
    秘匿性なし・本番使用禁止」と明記された公開前提のフィクスチャであり、
    [[security]] のシークレット混入防止の対象となる実運用鍵ではない
- publish は **`cargo publish --workspace` 1 コマンドで実施済み**（2026-07-21）。cargo 1.96 の
  `--workspace` publish は依存順（`fandhe-backend-http` → `fandhe-backend-plugin-*` →
  `fandhe-backend-routes` → `fandhe-backend-core` → `fandhe-backend-plugin-hub-wiring`）を
  自動解決するため、クレート個別の逐次 publish・インデックス反映待ちの手作業は不要。
  `cargo publish --workspace --dry-run` は 13 クレート全件で成功済み（2026-07-21）
- 本公開準備に伴うドキュメント追随の更新対象は、本ドキュメントのほか `README.md`
  （インストール節・crates.io 掲載用の絶対 URL 化）・`docs/guide/getting-started.md`
  （crates.io からの導入手順）・`site/index.md`（ドキュメントサイトトップへの公開準備
  状況とインストール手順の追記）である。いずれもコードの拡張点閉包とは無関係な
  公開準備ドキュメントの追随であり、実 publish 完了後に「公開済み」表現へ切り替える

## 5. publish フェイルクローズ（解除済み）

- 名称確定・公開判断が下るまでの間、全クレートに `publish = false` を設定し、意図しない
  `cargo publish` 事故を機械的に防ぐフェイルクローズ状態を維持していた（本イシュー #94 で
  `crates/core` に追加、他クレートは設定済みを確認。2026-07 時点）
- **2026-07-21 の公開判断（1 節）に基づき、公開対象 13 クレートの `publish = false` を
  解除した**。恒久非公開 3 クレート（`axum-ref` / `ws-load-client` / `docs-site`）のみ
  `publish = false` を維持し、フェイルクローズの対象を「利用者向け成果物でないクレート」に
  限定する運用へ移行した（4 節の区分表が正）

## 6. リリース CI 設計（YAML 草案）

実ファイル（`.github/workflows/release.yml`）は今回追加しない（private リポジトリ・
Trusted Publishing 未設定のため追加しても実行不能なデッドコードになる。
[[out-of-scope-tracking]] に従い、実ファイル化は別イシューに切り出す）。
以下は将来実装時の草案。publish 手順は 4 節で正式化した
`cargo publish --workspace`（依存順自動解決）に整合させている。

```yaml
name: release

on:
  push:
    tags:
      - "v*"
  workflow_dispatch:

permissions:
  contents: read

jobs:
  verify:
    runs-on: self-hosted
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
        with:
          submodules: recursive
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets --all-features -- -D warnings
      - run: cargo test --workspace --all-features
      - run: bash scripts/dep-audit.sh

  dry-run:
    needs: verify
    runs-on: self-hosted
    timeout-minutes: 15
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@v4
      # 公開対象 13 クレート（publish = false でない全クレート）を依存順自動解決で dry-run する
      - run: cargo publish --workspace --dry-run

  publish:
    needs: dry-run
    runs-on: self-hosted
    timeout-minutes: 15
    environment: crates-io-release # GitHub Environments の required reviewers で人間承認を必須化
    permissions:
      contents: read
      id-token: write # Trusted Publishing（OIDC）用。長命トークンをシークレットに保存しない
    steps:
      - uses: actions/checkout@v4
      # cargo 1.96 の --workspace publish は依存順（fandhe-backend-http → fandhe-backend-plugin-* →
      # fandhe-backend-routes → fandhe-backend-core → fandhe-backend-plugin-hub-wiring）と
      # crates.io のインデックス反映待ちを自動解決するため、逐次 publish の手作業は不要
      - run: cargo publish --workspace
```

- **認証**: crates.io の
  [Trusted Publishing（OIDC）](https://crates.io/docs/trusted-publishing) を第一候補とする。
  長命 API トークンをリポジトリシークレットに保存しない。Trusted Publishing が利用できない
  場合のフォールバックとして、スコープ限定・短期限トークンを GitHub Environments の
  シークレットで管理する
- **承認**: `publish` ジョブは GitHub Environments の required reviewers による人間承認を
  必須とし、AI や CI による自動 publish は行わない（[[feature-modification]] の
  自動マージ禁止と同一原則）
- **CI 規約準拠**（[[ci]]）: 全ジョブ `runs-on: self-hosted`・`timeout-minutes` 設定・
  `permissions` 最小（既定 `contents: read`、`publish` ジョブのみ `id-token: write` を追加）・
  fork PR からのトリガ不可（`push` タグ・`workflow_dispatch` のみで `pull_request` を
  トリガに含めない）

## 7. バージョニング

- [SemVer](https://semver.org/) に従う
- `0.x` 系では Cargo の慣例に従い minor バージョンアップで breaking change を許容する
- workspace 内クレートは lockstep（一括バージョン更新）方針とする。個別クレートごとに
  バージョンを分離すると、`fandhe-backend-http` → `fandhe-backend-routes` → `fandhe-backend-core` の依存順
  publish 時にバージョン整合の管理コストが増えるため

## 8. 公開前チェックリスト

実際に publish を実行する際に確認すべき項目（2026-07-21 実施状況）。

- [x] `cargo publish --workspace --dry-run` が公開対象 13 クレート全件で成功する
      （2026-07-21 実施済み）
- [x] `cargo package --list -p <crate>` で同梱内容を確認し、シークレット・計測データ
      （`benches/reports/**` 等）・ローカル設定が誤って含まれていないことを確認する
      （`fandhe-backend-plugin-hub-wiring` のテスト専用 RSA 鍵 `tests/fixtures/*.pk8` は
      `exclude` 済み。4 節参照。2026-07-21 実施済み）
- [x] 各クレートの `categories` スラッグを crates.io 公式カテゴリ一覧
      （<https://crates.io/categories>）と 1 件ずつ照合する。2026-07-21 実施済み
- [x] `cargo audit` / `cargo deny check` が 0 件で通過する（`scripts/dep-audit.sh`）。
      2026-07-21 実施済み
- [ ] README・doc comment 内のドキュメントリンクが public URL で解決すること。
      注: `fandhe-backend-spec` リポジトリが private リポジトリのままで、README から
      `docs/spec/` への 4 箇所のリンクが crates.io 掲載時に private リポジトリへの
      リンクとなるため、完全解決は spec リポジトリの public 化待ち
- [x] 1 節の前提条件がすべて完了している。2026-07-21 実施済み

## 参照

- 委譲・レビューゲート: [[feature-modification]]・[[delegation-impl]]
- pay-for-what-you-use: [[pay-for-what-you-use]]
- CI 実行環境規約: [[ci]]
- セキュリティ規約: [[security]]
- スコープ外課題の追跡: [[out-of-scope-tracking]]
