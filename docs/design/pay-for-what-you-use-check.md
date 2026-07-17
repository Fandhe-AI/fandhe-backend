# pay-for-what-you-use 機械検証（TASK-2.2 / #19）

対応: `docs/spec/05-tasks.md` TASK-2.2（REQ-2、`docs/spec/04-requirements.md`）。
前提タスク TASK-2.1（#18、`docs/design/plugin-boundary.md`）で確立した
`webrtc-proxy` feature 境界を対象に、`.claude/rules/pay-for-what-you-use.md` の
検証表（依存の残留確認・`unsafe` 件数の増減・バイナリサイズ比較・全構成ビルド）を
PASS/FAIL を返す自動検証スクリプトとして整備した。

## 1. 背景

TASK-2.1 までの検証は `docs/design/plugin-boundary.md` 6 節の**手動コマンド表**と、
情報提示のみで PASS/FAIL 判定を持たない `scripts/dep-impact.sh`（CI 非組み込み）に
留まっていた。本タスクは「プラグイン feature 無効時に当該プラグインの依存クレート・
`unsafe`・コードが 0 件で載らないこと」を機械的に PASS/FAIL 判定するゲートスクリプト
`scripts/pay-for-what-you-use-check.sh` を新設し、CI（`pay-for-what-you-use` ジョブ）に
常設した。

## 2. `dep-impact.sh` との役割分担

| | `dep-impact.sh` | `pay-for-what-you-use-check.sh` |
|---|---|---|
| 目的 | 記録台帳（`docs/dep-impact/records.md`）向けの計測値出力 | PASS/FAIL のゲート判定 |
| 出力形式 | markdown 表（人間が読んで記録） | `[PASS]`/`[FAIL]` ログ + 終了コード |
| CI 組み込み | なし（plugin-builder がローカル実行） | `pay-for-what-you-use` ジョブに常設 |
| cargo-geiger 未導入時 | 該当行をスキップして継続 | 必須ツール扱い（FAIL） |
| 対象 feature | no-default / default / all-features の 3 構成一括 | プラグイン feature ごとに動的列挙・個別検証 |

両者は併存する。`dep-impact.sh` は「変更前後の計測値を人間が比較・記録する」運用、
本スクリプトは「pay-for-what-you-use 違反を CI が機械的に検知する」ゲートという
異なる責務を担う。

## 3. 検証設計

### 3.1 (a) プラグイン feature の動的列挙

`cargo metadata --no-deps --format-version 1` から `backend-framework-core` の
`[features]` を取得し、値に `dep:bf-plugin-*` を含む feature を「プラグイン feature」
として抽出する。feature 増加時にスクリプト変更が不要になる設計（`dep-audit.sh`・
`dep-direction-check.sh` と同方針）。

- 列挙結果が 0 件の場合は列挙ロジックの腐敗を疑い FAIL（フェイルクローズ。現時点で
  `webrtc-proxy` が必ず 1 件存在する）
- feature 名がクレート名（`bf-plugin-<name>` の `<name>`）と一致しない場合は
  `docs/design/plugin-boundary.md` 2 節の命名規約違反として FAIL

### 3.2 (b) cargo tree 検証（依存 0 件）

- 無効構成（`--no-default-features`）: 列挙した全プラグインクレートが
  `cargo tree -p backend-framework-core -e normal --prefix none` の出力に
  出現しないこと
- 有効構成（ポジティブコントロール）: 各 feature を単独有効化した場合に当該
  プラグインクレートが出現し、かつ他プラグインのクレートが混入しないこと
  （配線切れ・列挙腐敗の検知）

### 3.3 (c) cargo geiger 検証（unsafe 0 件）

無効構成の依存グラフ（`cargo geiger --manifest-path crates/core/Cargo.toml
--no-default-features --output-format Json -q`）にプラグインクレートが含まれない
ことを検証する。依存グラフに載らなければ `unsafe` も計上対象にならないため、
依存 0 件の検証（(b)）が成立していれば理論上は自明だが、cargo-geiger の実行自体が
壊れやすいツールであるため独立したチェックとして維持し、実行失敗は FAIL とする
（原因を握りつぶさない）。

`cargo geiger` は workspace 仮想マニフェストに対しては `-p` オプションが機能せず
「virtual manifest」エラーになるため、`--manifest-path <crates/core/Cargo.toml の
絶対パス>` を使う（実装時に実機確認済み）。

### 3.4 (d) バイナリサイズ計測（コード 0 件）

`crates/core/examples/minimal` を無効構成／`--all-features` の 2 構成でリリース
ビルドし、生成物サイズを比較する。無効構成 <= 有効構成であること、差分をログ出力
することを検証する。補強として無効構成バイナリのシンボル表（`nm`）にプラグイン
由来シンボル（クレート名のハイフンをアンダースコアに変換した文字列を含むシンボル、
例 `bf_plugin_webrtc_proxy`）が出現しないことを検証する。`nm` が利用できない環境
ではこの補強のみ SKIP し、サイズ比較ゲートは維持する（fail-closed の例外を最小化）。

再ビルドの往復で共有 `target/` を汚さないよう、専用 `--target-dir`
（`target/pay-for-what-you-use-check`・`target/pay-for-what-you-use-check-all`）を使う。

### 3.5 (e) 全構成ビルド検証

無効構成・feature 単独構成（列挙した feature ごと）・`--all-features` の
`cargo build -p backend-framework-core` がすべて成功することを検証する
（`.claude/rules/pay-for-what-you-use.md` 検証表の「全構成ビルド」）。

## 4. セルフテスト

`scripts/tests/run-pay-for-what-you-use-tests.sh`（ネットワーク・cargo ビルド不要、
`unsafe-triage` ジョブに常設）。`scripts/tests/fixtures/pay-for-what-you-use/` の
fixture を注入口（`--metadata-file`・`--tree-negative-file`・`--tree-positive-dir`・
`--geiger-packages-file`・`--size-negative`/`--size-positive`・`--symbols-file`）
経由で与え、(a)〜(d) の判定ロジックを workspace の実状態・実ビルドに依存せず固定化
する。`--skip-build-steps` は (d)/(e) の実ビルドを回避しつつ、注入した値で (d) の
判定ロジックのみを検証する。

(e) は cargo ビルドそのものが検証対象のため fixture 化しない。実ビルドを伴う (e) の
動作確認は本スクリプトの通常実行（CI の `pay-for-what-you-use` ジョブ・人間による
ローカル実行）に委ねる。

## 5. CI 組み込み

- 新規ジョブ `pay-for-what-you-use`（`.github/workflows/ci.yml`）: release ビルド
  ×2 + cargo-geiger 実行を伴い重いため独立ジョブとする。`cargo-geiger@0.13.0` を
  バージョン固定でインストールし（未導入時のみ）、本体を実行する
- `clippy` ジョブに `cargo clippy -p backend-framework-core --all-targets
  --no-default-features -- -D warnings` を追加し、`docs/design/plugin-boundary.md`
  6 節の検証表「無効構成 lint」を CI 化した（既存コメントの「TASK-2.2 のスコープ」を解消）
- `unsafe-triage` ジョブへセルフテストのみを軽量ステップとして相乗り
- `ci-complete` の `needs` とループに `pay-for-what-you-use` を追加（fail-closed 維持）

## 6. 実機確認結果（実装時点、origin/main ffc6c76）

- `cargo geiger --version`: `cargo-geiger 0.13.0`（`--output-format Json` 利用可、
  `-q` 併用でビルドログを分離できることを確認）
- `cargo build --release -p backend-framework-core --example minimal`: 無効構成
  796808 bytes、`--all-features` 821912 bytes（差分 25104 bytes）
- `nm` によるシンボル表チェック: 無効構成バイナリに `bf_plugin` 由来シンボル 0 件、
  `--all-features` バイナリには `bf_plugin_webrtc_proxy` 由来シンボルが複数出現する
  ことを確認済み

## 7. スコープ外

- `dep-impact.sh` のゲート化・統廃合（情報提示ツールとして現状維持、2 節）
- WebSocket/GraphQL 2 プラグインの着脱受け入れテスト → TASK-2.4（#21）
- Middleware 非同期 I/O 規約 → TASK-2.3（#20）
- feature 構成別 clippy/test の全面マトリクス化（本タスクでは無効構成 clippy の
  最小追加に留める）
