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

## 6. リリース CI（実ファイル化済み、イシュー #373）

`.github/workflows/release.yml` として実ファイル化済み（イシュー #373。草案保留の理由
だった「private リポジトリのため実行不能」は public 化により解消）。verify（fmt /
clippy / test / dep-audit）→ dry-run（`cargo publish --workspace --dry-run`）→
publish（`cargo publish --workspace`、依存順自動解決）の 3 段構成で、トリガは `v*`
タグ push と `workflow_dispatch` のみ。

- **認証**: org シークレット `CARGO_REGISTRY_TOKEN` によるトークン認証を当面採用する
  （本節の当初第一候補だった
  [Trusted Publishing（OIDC）](https://crates.io/docs/trusted-publishing) への移行は
  将来課題として残す。トークンは publish ステップの env 経由でのみ参照し、コード・
  ログへ露出させない）
- **承認**: `publish` ジョブは GitHub Environments `crates-io-release`（required
  reviewers、deployment branch policy: `main` ブランチ + `v*` タグ）による人間承認を
  必須とし、AI や CI による自動 publish は行わない（[[feature-modification]] の
  自動マージ禁止と同一原則）
- **CI 規約準拠**（[[ci]]）: 全ジョブ `runs-on: self-hosted`・`timeout-minutes` 設定・
  `permissions` は `contents: read` のみ・fork PR からのトリガ不可（`push` タグ・
  `workflow_dispatch` のみで `pull_request` をトリガに含めない）・publish の途中
  キャンセルによる部分公開を防ぐため `concurrency` は `cancel-in-progress: false`

## 7. バージョニング

- [SemVer](https://semver.org/) に従う
- `0.x` 系では Cargo の慣例に従い minor バージョンアップで breaking change を許容する
- workspace 内クレートは lockstep（一括バージョン更新）方針とする。個別クレートごとに
  バージョンを分離すると、`fandhe-backend-http` → `fandhe-backend-routes` → `fandhe-backend-core` の依存順
  publish 時にバージョン整合の管理コストが増えるため

### 7.1 v0.2.0 リリース（イシュー #437、2026-08-01 publish 完了）

v0.1.0 公開（2026-07-21）後に breaking change が 2 件 main に入ったため、7 節の
lockstep 方針に従い公開対象 13 クレートを 0.2.0 へ一括バンプした。

- **BREAKING CHANGES**（詳細・移行手順は `CHANGELOG.md` 参照）:
  1. `fandhe-backend-core`: `GateOutcome::Reject` が `{ status, body }` から
     検証済み `Response` を運ぶ `{ response: Response }` へ変更（イシュー #424、PR #431）
  2. `fandhe-backend-plugin-static`: `StaticConfigError`（非 `#[non_exhaustive]`）へ
     `InvalidMimeMapping` バリアントを追加（イシュー #423、PR #430）
- **本イシューで実施した機械作業**: 公開対象 13 クレート + workspace 内 path 依存の
  `version` 併記（`crates/core` 等）+ `templates/app`・`examples/with-*` の依存
  `version` 併記を 0.2.0 へバンプ、`crates/plugin-openapi/openapi.json` の
  `info.version` 再生成、`CHANGELOG.md` 新設、6 節の検証を PASS させたうえでの PR 作成
- **実 publish（Phase B）は 2026-08-01 に完了**。`v0.2.0` タグ push により
  `release.yml` の verify → dry-run → GitHub Environments `crates-io-release` の
  承認を経て `cargo publish --workspace` が実行され、公開対象 13 クレートすべての
  0.2.0 が crates.io インデックスへ反映されたことを確認済み。あわせて本ドキュメント・
  README・`docs/guide/getting-started.md`・`site/index.md`・`CHANGELOG.md` の
  「v0.2.0 準備中」表現を「公開済み（2026-08-01）」へ切り替えた
- タグ push 初回のリリース CI は verify ジョブの checkout（`submodules: recursive`）が
  private の `fandhe-backend-spec` submodule を取得できず失敗した。verify は
  spec 本文を参照しないため submodule 非取得へ修正（PR #453）し、タグを付け直して
  成功した
- `standalone-crates-io.yml`（`scripts/standalone-crates-io-check.sh`）は
  crates.io 公開版のみで templates/examples を検証する性質上、v0.2.0 publish
  完了までは構造的に FAIL する（required check 対象外のためマージは阻害しない）。
  publish 完了後に `workflow_dispatch` で再実行し PASS を確認する（実施済み、
  結果は 8 節チェックリスト参照）

### 7.2 version 一元管理への移行（イシュー #452）

v0.2.0 バンプ（7.1 節）は公開対象 13 クレート + workspace 内 path 依存の
`version` 併記を各クレートの Cargo.toml に分散したまま個別に書き換えており、
書き換え箇所が多く漏れリスクが高かった。lockstep 方針（本節冒頭）を続ける前提で、
root `Cargo.toml` の `[workspace.package] version` + `[workspace.dependencies]`
（内部 13 クレートを `path` + `version` で定義）へ集約し、各クレートは
`version.workspace = true` + `{ workspace = true }`（optional なものは
`{ workspace = true, optional = true }`）でこれを継承する形へ統一した。

- 対象は workspace メンバー 16 クレート全部（公開対象 13 + 恒久非公開 3
  `axum-ref`・`docs-site`・`ws-load-client`）。恒久非公開 3 クレートも
  `publish = false` のまま 0.2.0 へ追随させ、二重管理を残さない
- `crates/ws-load-client/Cargo.toml` は上記恒久非公開 3 クレートの 1 つで、
  本移行に伴い `version` 併記行を `version.workspace = true` へ変更した
  （拡張点閉包判定の E 分類対象。閉じない理由: WebSocket 負荷試験専用の
  非公開バイナリクレートの Cargo.toml メタデータ変更であり、`crates/plugin-*`
  の拡張点契約にもコアの拡張点実装にも影響しない。`extension-closure-check.sh`
  の分類規則 A〜D が `crates/ws-load-client/**` を走査対象に含めていないことに
  起因する機械的な E 判定で、`crates/axum-ref/Cargo.toml`・
  `crates/docs-site/Cargo.toml` と同一の運用上のギャップ）
- `crates/http/fuzz` は root workspace から `exclude` された独立 workspace
  のため workspace inheritance を使えず、`version = "0.0.0"` のまま対象外
- `crates/plugin-hub-wiring` の dev-dependency `fandhe-backend-routes = { path
  = "../routes" }`（version なし）は意図的に `[workspace.dependencies]` へ
  含めず現状維持した。`workspace = true` 化すると version が付き、publish
  時に strip されていた dev 専用依存が公開版 Cargo.toml の内容を変えて
  しまうため（公開成果物の完全性維持を優先した安全側の判断）
- **次回バンプの手順**: root `Cargo.toml` の `[workspace.package] version` 1 行
  + `[workspace.dependencies]` の内部 13 クレートの `version` 値（14 箇所）を
  書き換える。加えて standalone workspace の `templates/app`・`examples/with-*`
  （workspace inheritance の対象外）の依存 `version` 併記、
  `crates/plugin-openapi/openapi.json` の再生成、`CHANGELOG.md` は従来どおり
  個別に更新する（7.1 節の手順から変わらない）
- `[workspace.dependencies]` の `version` は `workspace.package.version` を
  参照できない Cargo の仕様上、`[workspace.package] version` と別に保守する
  必要があるが、変更が root `Cargo.toml` 1 ファイルに閉じる点は達成している

### 7.3 v0.3.0 リリース（イシュー #506、2026-08-05 publish 完了）

v0.2.0 公開（2026-08-01）後に breaking change が 2 件 main に入ったため、7 節の
lockstep 方針に従い公開対象 13 クレートを 0.3.0 へ一括バンプした。

- **実 publish（Phase B）は 2026-08-05 に完了**。`v0.3.0` タグ push により
  `release.yml` の verify → dry-run → GitHub Environments `crates-io-release` の
  承認を経て `cargo publish --workspace` が実行され、公開対象 13 クレートすべての
  0.3.0 が crates.io インデックスへ反映されたことを確認済み（release run 31012481870。
  あわせて本ドキュメント・`CHANGELOG.md` の「publish は準備中」表現を「公開済み
  （2026-08-05）」へ切り替えた）。

- **BREAKING CHANGES**（詳細・移行手順は `CHANGELOG.md` 参照）:
  1. `fandhe-backend-core`: `RequestGate::check` へ `ctx: &GateContext` 引数を追加
     （イシュー #486、PR #487）
  2. `fandhe-backend-plugin-websocket`: `handle_upgrade` へキャンセル `Future`
     引数（第 5 引数）を追加（イシュー #492、設計 #490）
- **実施した機械作業**: 7.2 節の一元管理手順に従い、root `Cargo.toml` の
  `[workspace.package] version` + `[workspace.dependencies]` の内部 13 クレート
  `version` 値（計 14 箇所）を 0.3.0 へ書き換え。加えて standalone workspace
  （`templates/app`・`examples/with-*` 4 件）の依存 `version` 併記、
  `crates/plugin-openapi/openapi.json` / `openapi.yaml` の `info.version` 再生成
  （`scripts/openapi-two-stage.sh --update`）、`CHANGELOG.md` の `[Unreleased]` →
  `[0.3.0] - 2026-08-05` 改題を実施した（`crates/plugin-webrtc/tests-e2e` は
  `crates/http/fuzz` と同じ root workspace exclude の独立 workspace で
  `version = "0.0.0"`・path 依存に version 併記なしのため対象外）
- `standalone-crates-io.yml`（`scripts/standalone-crates-io-check.sh`）は
  crates.io 公開版のみで templates/examples を検証する性質上、v0.3.0 publish
  完了までは構造的に FAIL する（required check 対象外のためマージは阻害しない。
  v0.2.0 時（7.1 節）と同一のニワトリ卵問題であり、`.standalone-crates-io-skip`
  マーカーは全件 SKIP を fail-closed で拒否する設計のため追加しない）。
  publish 完了後に `workflow_dispatch` で再実行し PASS を確認（実施済み、success）

### 7.4 v0.4.0 リリース（2026-08-13 準備、publish は準備中）

v0.3.0 公開（2026-08-05）後に breaking change が 1 件 main に入ったため、7 節の
lockstep 方針に従い公開対象 13 クレートを 0.4.0 へ一括バンプした
（ユーザー指示 2026-08-13。将来の v1.0.0 昇格に先立ち、未リリースの
breaking change を 0.x 系で一度安定させる位置づけ）。

- **BREAKING CHANGES**（詳細・移行手順は `CHANGELOG.md` 参照）:
  1. `fandhe-backend-http`: `RequestHead` の `method` / `target` フィールドを
     非公開化し、`method()` / `target()` アクセサ（`&str` 返却）経由の取得へ
     変更（イシュー #591、性能改善ツリー #579 Phase 3。設計は
     `docs/design/zero-copy-request-head.md`）
- **実施した機械作業**: 7.2 節の一元管理手順に従い、root `Cargo.toml` の
  `[workspace.package] version` + `[workspace.dependencies]` の内部 13 クレート
  `version` 値（計 14 箇所）を 0.4.0 へ書き換え。加えて standalone workspace
  （`templates/app`・`examples/with-*` 4 件）の依存 `version` 併記、
  `crates/plugin-openapi/openapi.json` / `openapi.yaml` の `info.version` 再生成
  （`scripts/openapi-two-stage.sh --update`）、`CHANGELOG.md` の `[Unreleased]` →
  `[0.4.0] - 2026-08-13` 改題を実施した
- `standalone-crates-io.yml` は v0.4.0 publish 完了までは構造的に FAIL する
  （v0.2.0（7.1 節）・v0.3.0（7.3 節）と同一のニワトリ卵問題、required check
  対象外のためマージは阻害しない）。publish 完了後に `workflow_dispatch` で
  再実行し PASS を確認する

## 8. 公開前チェックリスト

実際に publish を実行する際に確認すべき項目。

### v0.1.0（2026-07-21 実施済み）

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
- [ ] `examples/*/.standalone-crates-io-skip`（crates.io 未再公開の新 API 依存を理由に
      `scripts/standalone-crates-io-check.sh` を一時 SKIP しているマーカー、イシュー #433）
      を全件確認する。今回の公開で当該 API が crates.io 公開版に含まれるようになった
      マーカーは削除し、`bash scripts/standalone-crates-io-check.sh` を再実行して
      SKIP なしで通ることを確認する（削除し忘れは検証の穴を恒久化させるため必須）

### v0.2.0（イシュー #437、2026-08-01 publish 完了）

- [x] 公開対象 13 クレートの `version` および workspace 内 path 依存の `version` 併記が
      すべて 0.2.0 に揃っている（恒久非公開 3 クレートは対象外）
- [x] `cargo publish --workspace --dry-run` が公開対象 13 クレート全件で成功する
- [x] Phase B: `v0.2.0` タグ push → verify → dry-run → 人間承認 → `cargo publish --workspace`
      の実行（2026-08-01 実施済み。13 クレートすべての 0.2.0 が crates.io
      インデックスへ反映されたことを確認済み）
- [x] Phase B publish 完了後、`standalone-crates-io.yml` を `workflow_dispatch` で
      再実行し、`templates/app`・`examples/with-cors`・`examples/with-graphql`・
      `examples/with-websocket` の 4 クレートが `fandhe-backend-core = "^0.2.0"` 等を
      crates.io 公開版のみで解決できて PASS することを確認する（2026-08-01 実施済み、
      success）。あわせて `examples/with-interceptor/.standalone-crates-io-skip`
      （イシュー #433）は Interceptor を収録した 0.2.0 の公開により解消したため削除し、
      SKIP なしで通ることを PR CI（paths トリガー）で再検証した
      （v0.1.0 チェックリストの同種項目と同一原則。削除し忘れは検証の穴を恒久化させる）

### v0.3.0（イシュー #506、2026-08-05 publish 完了）

- [x] 公開対象 13 クレートの `version` および workspace 内 path 依存の `version` 併記が
      すべて 0.3.0 に揃っている（恒久非公開 3 クレートは対象外）
- [x] `cargo publish --workspace --dry-run` が公開対象 13 クレート全件で成功する
- [x] Phase B: `v0.3.0` タグ push → verify → dry-run → 人間承認 → `cargo publish --workspace`
      の実行（2026-08-05 実施済み。13 クレートすべての 0.3.0 が crates.io
      インデックスへ反映されたことを確認済み。release run 31012481870）
- [x] Phase B publish 完了後、`standalone-crates-io.yml` を `workflow_dispatch` で
      再実行し、`templates/app`・`examples/with-cors`・`examples/with-graphql`・
      `examples/with-websocket`・`examples/with-interceptor` の 5 クレートが
      `fandhe-backend-core = "^0.3.0"` 等を crates.io 公開版のみで解決できて PASS することを
      確認する（2026-08-05 実施済み、success）

### v0.4.0（2026-08-13 準備、publish は準備中）

- [x] 公開対象 13 クレートの `version` および workspace 内 path 依存の `version` 併記が
      すべて 0.4.0 に揃っている（恒久非公開 3 クレートは対象外。2026-08-13 実施済み）
- [x] `cargo publish --workspace --dry-run` が公開対象 13 クレート全件で成功する
      （2026-08-13 実施済み）
- [ ] Phase B: `v0.4.0` タグ push → verify → dry-run → 人間承認 → `cargo publish --workspace`
      の実行
- [ ] Phase B publish 完了後、`standalone-crates-io.yml` を `workflow_dispatch` で
      再実行し、`templates/app`・`examples/with-*` 4 件の 5 クレートが
      `fandhe-backend-core = "^0.4.0"` 等を crates.io 公開版のみで解決できて PASS することを
      確認する

## 参照

- 委譲・レビューゲート: [[feature-modification]]・[[delegation-impl]]
- pay-for-what-you-use: [[pay-for-what-you-use]]
- CI 実行環境規約: [[ci]]
- セキュリティ規約: [[security]]
- スコープ外課題の追跡: [[out-of-scope-tracking]]
