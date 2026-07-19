# crates.io 公開手順

イシュー #94「chore(global): OSS 公開準備（crates.io・LICENSE・CONTRIBUTING）」対応。
本ドキュメントは crates.io への公開手順（名前確保・所有権・リリース CI）を**定める**もので、
現時点で実際の `cargo publish` 実行・リポジトリ public 化・crates.io 上での名前確保は
行わない（1 節「前提条件」参照）。実ファイル化・実行判断は前提条件充足後の別イシューで扱う
（`docs/design/README.md` の記載規約に従い、対応するタスク・要件 ID と紐付ける）。

## 1. 前提条件（公開ブロッカー）

以下がすべて完了するまで、本ドキュメントの手順 2 節以降は実行しない。

1. **正式名称の確定**: `fandhe-backend` に確定済み（#200、
   [`docs/design/framework-naming.md`](./framework-naming.md) 参照。crate/import 改名は
   #202・PR #209 で反映済み）。本ブロッカーは解消済み
2. **リポジトリの public 化**: 現状 `PRIVATE`（`gh repo view` で確認）。OSS として公開する
   判断が正式に下されてから public 化する
3. **公開対象クレートの最終選定**: 4 節の区分表をレビューで確定させる

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
| `fandhe-backend-plugin-websocket` / `fandhe-backend-plugin-graphql` / `fandhe-backend-plugin-openapi` / `fandhe-backend-plugin-webrtc` / `fandhe-backend-plugin-webrtc-proxy` / `fandhe-backend-plugin-tracing` / `fandhe-backend-plugin-hub-wiring`（存在するもの） | 公開対象 | feature 駆動プラグイン本体 |
| `axum-ref` | 恒久非公開 | 性能比較用参照実装。フレームワーク利用者向け成果物ではない |
| `ws-load-client` | 恒久非公開 | WebSocket 負荷試験専用バイナリ |
| `crates/http/fuzz` | 恒久非公開（対象外） | cargo-fuzz 専用クレート。root workspace から `exclude` 済み（TASK-15.3-1、#87）であり、`cargo publish` の対象にも入らない |

- 公開対象クレートは現時点ではすべて `Cargo.toml` に `publish = false` を持たない
  （`crates/core` を除く。5 節参照）。**正式名称確定・公開可否のレビュー承認が下りるまでは、
  公開対象クレートも含めて全クレートに `publish = false` を設定し publish をフェイル
  クローズで禁止する**（5 節）
- publish 順序は依存方向（`server → routes → http::*`）に従う:
  1. `fandhe-backend-http`
  2. `fandhe-backend-routes`
  3. `fandhe-backend-core`
  4. `fandhe-backend-plugin-*`（相互依存がなければ順不同。`fandhe-backend-plugin-websocket` 等はコアに依存しない
     設計のため、コアより先でも問題ない。ただし本ドキュメントでは分かりやすさのため
     コアの後に統一する）
- 公開判断が下り、個別クレートの publish を解除する際は、当該クレートの
  `Cargo.toml` から `publish = false` の行を削除し、本ドキュメントの本表の
  「恒久非公開」以外のクレートについて解除する

## 5. publish フェイルクローズ（現時点の実装）

- `crates/core/Cargo.toml` に `publish = false` を理由コメント付きで追加した
  （本イシュー #94 で対応済み）。名称確定・公開判断が下るまで、意図しない
  `cargo publish` 事故を機械的に防ぐ
- 他の 11 クレート（`fandhe-backend-http` / `fandhe-backend-routes` / `fandhe-backend-plugin-*` / `axum-ref` /
  `ws-load-client`）はすでに `publish = false` が設定済みであることを調査で確認済み
  （2026-07 時点）。したがって現状、全 12 クレートが `publish = false` で
  `cargo publish` 不能な状態を維持している

## 6. リリース CI 設計（YAML 草案）

実ファイル（`.github/workflows/release.yml`）は今回追加しない（名称未確定・private
リポジトリ・Trusted Publishing 未設定のため追加しても実行不能なデッドコードになる。
[[out-of-scope-tracking]] に従い、実ファイル化は名称確定後の別イシューに切り出す）。
以下は将来実装時の草案。

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
      # 依存順（fandhe-backend-http → fandhe-backend-routes → fandhe-backend-core → fandhe-backend-plugin-*）で
      # 各クレートに対し dry-run を実行する
      - run: cargo publish --dry-run -p fandhe-backend-http

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
      # 依存順に publish する（各クレート間で crates.io のインデックス反映待ちを挟む）
      - run: cargo publish -p fandhe-backend-http
      - run: cargo publish -p fandhe-backend-routes
      - run: cargo publish -p fandhe-backend-core
      # 以降 fandhe-backend-plugin-* を順次 publish
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

公開判断が下り、実際に publish を実行する際は以下をすべて確認する。

- [ ] `cargo publish --dry-run -p <crate>` が全公開対象クレートで成功する
- [ ] `cargo package --list -p <crate>` で同梱内容を確認し、シークレット・計測データ
      （`benches/reports/**` 等）・ローカル設定が誤って含まれていないことを確認する
- [ ] `cargo audit` / `cargo deny check` が 0 件で通過する（`scripts/dep-audit.sh`）
- [ ] README・doc comment 内のドキュメントリンクが public URL で解決すること
      （private リポジトリ前提のリンクが残っていないこと）
- [ ] 1 節の前提条件（正式名称確定・public 化・公開対象クレート最終選定）がすべて完了している

## 参照

- 委譲・レビューゲート: [[feature-modification]]・[[delegation-impl]]
- pay-for-what-you-use: [[pay-for-what-you-use]]
- CI 実行環境規約: [[ci]]
- セキュリティ規約: [[security]]
- スコープ外課題の追跡: [[out-of-scope-tracking]]
