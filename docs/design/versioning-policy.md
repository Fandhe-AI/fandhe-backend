# バージョニング方針（semver・破壊的変更ポリシー）

- **対応イシュー**: [#96](https://github.com/Fandhe-AI/backend-framework/issues/96)「バージョニング方針
  （semver・破壊的変更ポリシー）の策定」
- **出典**: [#91](https://github.com/Fandhe-AI/backend-framework/issues/91) スコープ外トラッキング
  「運用面の欠落」（本体の semver 運用・破壊的変更ポリシー・サポートポリシーが `docs/spec/**` にも
  `docs/design/**` にも存在しない）
- **ステータス**: ドラフト（自動運転モードでの実装であるため、本ドキュメントは安全側の保守的判断として
  作成したドラフトであり、**最終承認は人間レビュー（本イシューの PR レビュー）で行う**）
- **対応可否判定（feasibility-guardrail）**: **可**。受け入れ基準あり（semver 運用・破壊的変更・サポート
  ポリシーの文書化、検証可能）・安全性方針と整合（ドキュメント追加のみ、コード変更なし）・影響範囲限定
  （`docs/design/` + 同 README のみ）の 3 軸すべて充足。判定内容は本イシューの実装コミット・PR に記録する

## 0. スコープ（本体 semver と `webrtc-rs` バージョン戦略の軸の違い）

既存の [`webrtc-rs-version-strategy.md`](./webrtc-rs-version-strategy.md)（TASK-8.3）は「独立 WebRTC
サービスが依存する `webrtc-rs` のバージョン戦略」であり、同ドキュメント 6 節が明記するとおり
**本ドキュメントのスコープ（フレームワーク本体の semver）とは別軸**。両者を混同・混在させない。

- **適用対象**（本ドキュメントが定義する semver 運用の対象クレート）:
  `fandhe-backend-core`（`crates/core`）、`fandhe-backend-http`（`crates/http`）、`fandhe-backend-routes`（`crates/routes`）、
  `fandhe-backend-plugin-websocket` / `fandhe-backend-plugin-graphql` / `fandhe-backend-plugin-openapi` / `fandhe-backend-plugin-webrtc` /
  `fandhe-backend-plugin-webrtc-proxy` / `fandhe-backend-plugin-hub-wiring` / `fandhe-backend-plugin-tracing`（`crates/plugin-*`）
- **非対象**（内部専用・バージョン保証の対象外）: `axum-ref`（性能比較用参照実装）・`ws-load-client`
  （負荷生成ハーネス）・`crates/http/fuzz`（cargo-fuzz 専用クレート、root workspace から exclude 済み）。
  いずれも `Cargo.toml` に `publish = false` が設定されており（`axum-ref`・`ws-load-client` は確認済み）、
  外部公開 API としての互換性保証を負わない

## 1. 現状（策定時点の前提事実）

- 全クレートが `version = "0.1.0"`（pre-1.0）
- `publish = false` は `axum-ref`・`fandhe-backend-http`・`fandhe-backend-routes`・全 `fandhe-backend-plugin-*`・`ws-load-client` に設定済みだが、
  **`crates/core`（`fandhe-backend-core`）には未設定**。他クレートとの不揃いであり、本ドキュメントでは
  是正せず「6. 関連・スコープ外」に記録し #94（crates.io 公開準備）側の対応とする
  （バージョニング運用の方針策定自体は crates.io 公開状態と独立に定義可能なため、本イシューをブロックしない）
- ツールチェーンは `rust-toolchain.toml` で `stable` に追随、`edition = "2024"`。MSRV（最小サポート
  Rust バージョン）の明示方針はこれまで未定義

## 2. semver 運用

[Cargo の semver 互換性規則](https://doc.rust-lang.org/cargo/reference/semver.html)に準拠する。

### pre-1.0（0.x）期の規則

Cargo 慣行どおり、現行の `0.y.z` では **`y` が実質 major**（破壊的変更は `y` を上げる）、`z` が
互換変更（バグ修正・非破壊追加）を表す。現行 `0.1.0` からこの運用で開始する。

### workspace 内バージョン同期

「1. 適用対象」の全公開クレートを **lockstep（同一バージョン一斉更新）** とする。

- 根拠: `crates/core` が `dep:` 構文（[`plugin-boundary.md`](./plugin-boundary.md)）でプラグインを
  束ねる密結合構成であり、独立バージョニングは組み合わせ検証コスト（どの core バージョンとどのプラグイン
  バージョンの組み合わせが検証済みか）を増大させ、AI ファースト保守性を損なうため
- 安全側判断として記録し、最終確定は人間レビューに委ねる

### v1.0 昇格基準

以下をすべて満たした時点で `1.0.0` への昇格を検討する。

1. `docs/spec/06-roadmap.md` の MS-1〜MS-6 が完了していること
2. 各マイルストーンの受け入れ検証（`docs/acceptance/**`）が充足していること
3. [#94](https://github.com/Fandhe-AI/backend-framework/issues/94)（OSS 公開準備・crates.io）・
   [#95](https://github.com/Fandhe-AI/backend-framework/issues/95)（Getting Started）が完了していること

## 3. 破壊的変更の定義（何が公開 API か）

以下のいずれかを変更する場合、破壊的変更として扱う。

- **Rust 公開 API**: `pub` アイテムのシグネチャ変更・エラー型変更・trait への必須メソッド追加等
  （`missing_docs = "warn"` + CI doc ジョブで doc comment 網羅は機械強制済み、[[coding-rust]]）
- **Cargo feature**: feature の削除・リネームは破壊的変更。**default feature への追加は破壊的変更として
  禁止する**（[`pay-for-what-you-use.md`](../../.claude/rules/pay-for-what-you-use.md) 原則により、
  feature 無効時の依存・`unsafe`・バイナリ増ゼロを利用者が期待できることを、default 変更で崩してはならない
  ため）。feature 名自体を公開 API とする方針は [`plugin-boundary.md`](./plugin-boundary.md) 既定義に従う
- **拡張点 3 trait**: `Middleware` / `UpgradeHandler` / `RequestGate`
  （[[coding-rust]]、[`dependency-graph-contract.md`](./dependency-graph-contract.md)）のシグネチャ変更
- **ワイヤ契約**: シグナリング HTTP 契約（`POST /rtc/offer`、
  [`webrtc-process-isolation.md`](./webrtc-process-isolation.md)）・`openapi.json`（生成物、
  [`openapi-typescript-pipeline.md`](./openapi-typescript-pipeline.md)）の後方非互換変更
  （既存フィールド削除・型変更）。openapi-typescript パイプライン経由の TS クライアント利用者への影響が
  あるため
- **MSRV / edition**: MSRV は初版として現行 `rust-toolchain.toml` の `stable` 系列（実装時点の最新 stable）
  で確定し、以後の引き上げは **minor 扱い**（Rust エコシステム慣行に従う）とする。edition 更新は、それ自体が
  破壊的変更を伴わない限り minor 扱いとする

## 4. 破壊的変更の手続き

1. コミット・PR は `feat!:` または `BREAKING CHANGE:` footer で明示する
   （[`.claude/rules/conventional-commits.md`](../../.claude/rules/conventional-commits.md)）
2. 可能な限り `#[deprecated]` による **1 リリース以上の非推奨期間**を挟む 2 段階
   （deprecate → remove）で進める。即時削除は緊急のセキュリティ対応等やむを得ない場合に限る
3. 変更内容・移行手順をリリースノートに記載する。CHANGELOG 運用そのものの整備は
   [#94](https://github.com/Fandhe-AI/backend-framework/issues/94) と調整する

## 5. サポートポリシー

- **pre-1.0**: 最新リリースのみサポートする（旧バージョンへのバックポートは行わない）
- **v1.0 以降**: 最新 minor へのセキュリティ修正提供を基本とする。旧系列へのバックポート条件は
  v1.0 昇格時に再定義する
- RUSTSEC advisory の検知は既存の `dep-audit` schedule 実行（`scripts/dep-audit.sh`）・
  `audit-triage.sh` トリアージが担い、修正リリース（patch）のトリガーとして接続する
  （[[improvement-proposal]] のトリアージ一次対応）

## 6. 検証・機械化の展望（スコープ外の明示）

`cargo-semver-checks` 等による破壊的変更検知の CI 機械化は本イシューでは実施しない。
[[out-of-scope-tracking]] に従い、承認を得たうえで別途改善提案として起票を検討する
（PR 本文で提案に留め、本イシューでは起票しない）。

## 7. セキュリティ考慮事項（OWASP Top 10 観点）

- **A06 脆弱・古いコンポーネント**: 「5. サポートポリシー」でセキュリティ修正の提供範囲を明確化し、
  「どのバージョンが修正を受けるか不明」という利用者側リスクを解消する。RUSTSEC 検知
  （`dep-audit` schedule + `audit-triage.sh`）を修正リリース（patch）のトリガーとして接続する
- **A08 ソフトウェア・データ整合性（サプライチェーン）**: バージョン規則の明文化は、依存側（利用者）の
  caret 指定（`^0.1`）で意図しない破壊的変更・挙動変更を取り込まないための前提となる。alpha/プレリリース版を
  安定版として案内しないことを明記する（[`webrtc-rs-version-strategy.md`](./webrtc-rs-version-strategy.md)
  と同一原則）
- **攻撃表面最小化（本フレームワークの核）**: default feature への追加を破壊的変更（実質禁止）と定義する
  ことで、pay-for-what-you-use 原則（feature 無効時の依存・`unsafe`・バイナリ増ゼロ）をバージョニング
  ポリシーの側からも固定する
- **シークレット混入防止**: 本ドキュメントにトークン・内部 URL 等の機密は含まない
- 本ドキュメントは版管理手続きのみを扱い、攻撃手順・具体的なエクスプロイト情報は含めない

## 8. 関連・スコープ外

- **[#94](https://github.com/Fandhe-AI/backend-framework/issues/94)**（OSS 公開準備・crates.io）:
  `crates/core` の `publish = false` 未設定（他クレートと不揃い）の是正・crates.io 公開手順そのものは
  同イシューのスコープ
- **[#95](https://github.com/Fandhe-AI/backend-framework/issues/95)**（Getting Started）:
  v1.0 昇格基準の前提の一つ
- **[`webrtc-rs-version-strategy.md`](./webrtc-rs-version-strategy.md)**（TASK-8.3）: 独立 WebRTC サービス側が
  依存する `webrtc-rs` のバージョン戦略。本ドキュメント（フレームワーク本体の semver）とは別軸であり、
  判断内容を混在させない
- **[`plugin-boundary.md`](./plugin-boundary.md)**: feature 名を公開 API とする既定義（本ドキュメント
  「3. 破壊的変更の定義」の feature 部分の前提）
- **[`pay-for-what-you-use.md`](../../.claude/rules/pay-for-what-you-use.md)**: default feature 追加禁止の
  根拠原則
- **`cargo-semver-checks` 等による CI 機械化・CHANGELOG / リリースノート運用の整備**:
  本イシューでは実施せず記録のみ（「6. 検証・機械化の展望」参照）
