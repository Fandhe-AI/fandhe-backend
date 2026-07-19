# TASK-11.5 受け入れテスト実行結果レポート

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-plugin-*` 等）表記のまま
> 保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。

Issue #78（TASK-11.5-2）の成果物。`scripts/coverage.sh`・`scripts/accept-task-11-5.sh` に
よる TASK-11.5（#37、`docs/spec/05-tasks.md`）受け入れ確認の実行結果を記録する。

## 実施日時・環境

- 実施日時: 2026-07-17（JST）
- OS: Linux 7.0.0-27-generic x86_64（Ubuntu）
- CPU コア数: 12（`nproc`）
- rustc: 1.96.0（stable, 2026-05-25）
- cargo-llvm-cov: 0.8.7
- cargo-nextest: 0.9.137
- 前提コミット: `f1a68d1`（TASK-11.5-1 / #77 / PR #111 マージ済み）

## 総合判定

```
$ bash scripts/accept-task-11-5.sh
===================================================
TASK-11.5 受け入れテスト（#78）
===================================================
[PASS] チェック 1（カバレッジ 80% 以上）: coverage.sh が閾値を満たして終了しました
[PASS] チェック 2（doc コメント網羅率 100%）: missing_docs = "warn" 設定済み・clippy -D warnings 通過
[PENDING] チェック 3（AGENTS.md 各節）: AGENTS.md が未作成です（TASK-11.3 / #35 待ち）
[PASS] チェック 4（CI テストタイムアウト設定）: ci.yml 全ジョブに timeout-minutes、nextest.toml に slow-timeout を確認しました
[PASS] チェック 5（依存方向の一方向性）: 循環なし・全エッジがレイヤ順の許可リストに合致
===================================================
==> accept-task-11-5.sh: FAIL なし・PENDING 1 件（前提イシュー待ち）
```

**終了コード: 0**（FAIL 0 件・PENDING 1 件。PENDING は #35 マージ後に再実行すれば
PASS になる想定。詳細はチェック 3 の節を参照）。

## チェック 1: コア全体の行カバレッジ 80% 以上

対象（コア）: `backend-framework-core`・`bf-http`（`cargo metadata` から動的決定。
`axum-ref`・`bf-plugin-webrtc-proxy` を除外）。

```
$ bash scripts/coverage.sh
（... cargo llvm-cov nextest --workspace --all-features --no-report ...）
==> カバレッジ判定（コア対象、閾値 80%）
==> カバレッジサマリ（コア対象、再計測なしでレポートのみ出力）
Filename                      Regions    Missed Regions     Cover   Functions  Missed Functions  Executed       Lines      Missed Lines     Cover    Branches   Missed Branches     Cover
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
core/src/extension.rs             141                23    83.69%          23                 7    69.57%         117                26    77.78%           0                 0         -
core/src/lib.rs                     7                 0   100.00%           2                 0   100.00%           6                 0   100.00%           0                 0         -
http/src/body.rs                  229                 1    99.56%          25                 0   100.00%         130                 1    99.23%           0                 0         -
http/src/connection.rs            489                10    97.96%          52                 0   100.00%         275                 2    99.27%           0                 0         -
http/src/request.rs               566                 7    98.76%          63                 0   100.00%         327                 3    99.08%           0                 0         -
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
TOTAL                             1432                41    97.14%         165                 7    95.76%         855                32    96.26%           0                 0         -
==> coverage.sh: コア対象の行カバレッジが 80% 以上であることを確認しました
    lcov: target/llvm-cov/lcov.info
```

**コア行カバレッジ: 96.26%**（≥ 80%、TASK-11.5-1 / #77 のテスト追加により大きく上回った。
本イシューでの追加テスト実装は不要と判断した）。

参考情報として、workspace 全体（`bf-plugin-webrtc-proxy` 含む）の行カバレッジも
併記する（閾値判定には使わない）。

```
Filename                               Regions    Missed Regions     Cover   Functions  Missed Functions  Executed       Lines      Missed Lines     Cover    Branches   Missed Branches     Cover
--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
axum-ref/src/main.rs                       177                20    88.70%          28                 5    82.14%         162                17    89.51%           0                 0         -
core/src/extension.rs                      141                23    83.69%          23                 7    69.57%         117                26    77.78%           0                 0         -
core/src/lib.rs                              7                 0   100.00%           2                 0   100.00%           6                 0   100.00%           0                 0         -
http/src/body.rs                           229                 1    99.56%          25                 0   100.00%         130                 1    99.23%           0                 0         -
http/src/connection.rs                     489                10    97.96%          52                 0   100.00%         275                 2    99.27%           0                 0         -
http/src/request.rs                        566                 7    98.76%          63                 0   100.00%         327                 3    99.08%           0                 0         -
plugin-webrtc-proxy/src/client.rs          418                37    91.15%          33                 0   100.00%         227                11    95.15%           0                 0         -
plugin-webrtc-proxy/src/config.rs           95                 0   100.00%          15                 0   100.00%          75                 0   100.00%           0                 0         -
plugin-webrtc-proxy/src/error.rs            38                 0   100.00%           4                 0   100.00%          28                 0   100.00%           0                 0         -
plugin-webrtc-proxy/src/handler.rs         196                 5    97.45%          21                 0   100.00%         120                 4    96.67%           0                 0         -
--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
TOTAL                                      2356               103    95.63%         266                12    95.49%        1467                64    95.64%           0                 0         -
```

計測方式の整合性確認: `coverage.sh` は `--workspace --all-features` で 1 回計測した後に
`cargo llvm-cov report -p backend-framework-core -p bf-http` でコア対象へフィルタする
方式を採る（プラグインを含む実行と分離実行の重複を避けるため）。この方式がプラグイン
テストの混入でコア数値を水増ししていないことを、`-p backend-framework-core -p bf-http`
のみを対象にした独立実行（プラグインのコンパイル・テストを一切含まない）と比較して
確認した。両者は Regions 1432/Missed 41、Lines 855/Missed 32 で完全に一致しており、
数値の水増しは発生していない。

閾値ゲートの陰性対照（負けパス確認）:

```
$ FAIL_UNDER_LINES=99 bash scripts/coverage.sh
（... 実測 96.26% は 99% 未満のため ...）
==> coverage.sh: コア対象の行カバレッジが 99% 未満です
$ echo $?
1
```

非 0 終了を確認し、ゲートが正しく機能していることを検証した。

doc test（`cargo test --doc`）は stable ツールチェーンでは instrumented coverage の対象に
できないため（`cargo-llvm-cov` の `--doctests` は nightly 専用、`rust-toolchain.toml` は
stable 固定）、本計測の対象外とした（`scripts/coverage.sh` 冒頭コメント参照）。

## チェック 2: doc コメント網羅率 100%

- `Cargo.toml` の `[workspace.lints.rust]` に `missing_docs = "warn"` が設定されている
  ことを確認した（TASK-11.2-1 / #33、TASK-11.2-2 / #76 で導入済み）。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` を実行し、
  `warn` が `-D warnings` で実質 deny に昇格した状態で警告 0 件（doc コメント欠落を含む）
  であることを確認した。

## チェック 3: AGENTS.md 各節（PENDING）

`AGENTS.md` はリポジトリに未作成（2026-07-17 時点）。作成は TASK-11.3（#35）のスコープで
あり、本イシュー（#78）の姉妹イシュー #77 の out-of-scope 記載どおり、本イシューでは
チェックの実装のみを完成させた。#35 マージ後に `AGENTS.md` が以下の必須節を含むことを
`scripts/accept-task-11-5.sh` で機械検査できる:

- モジュール境界
- 変更手順
- 変更完了の判定基準
- エスカレーション基準
- アサーション網羅性要求

`#35` マージ後に本チェックを再実行し、PASS になることを別途確認する必要がある
（本レポートの更新または新規 Issue でのフォローアップを推奨）。

## チェック 4: CI テストタイムアウト設定

- `.github/workflows/ci.yml` の全ジョブ（`fmt` / `clippy` / `test` / `doc` /
  `dep-audit` / `coverage`）に `timeout-minutes` が設定されていることを確認した。
  - 実装過程で `doc` ジョブに `timeout-minutes` が漏れていたことを検出したため、
    本イシューで `timeout-minutes: 30` を追補した（TASK-11.4 / #36 の多層防御方針の
    退行）。
- `.config/nextest.toml` に `slow-timeout` 設定（`profile.default`、120 秒でテスト強制
  終了）があることを確認した。

## チェック 5: 依存方向の一方向性

`cargo metadata` から抽出した workspace 内クレート間の path 依存エッジ:

```
backend-framework-core -> bf-http
bf-plugin-webrtc-proxy -> bf-http
```

- 循環依存検査（DFS）: 循環なし
- レイヤ順の許可リスト（`backend-framework-core -> bf-http`・`bf-plugin-* -> bf-http`・
  `*routes* -> bf-http`）との照合: 全エッジが合致（違反 0 件）

`bf-http` はいかなる workspace 内クレートにも依存しない最下層であることを確認した
（`Cargo.toml` 冒頭コメントのクレート分割方針どおり）。

## 検証（アサーション網羅性の裏取り）

受け入れスクリプト自体の検出ロジックが正しく機能することを、以下の陰性対照
（意図的に壊した入力に対して FAIL/非 0 になることの確認）で検証した:

- カバレッジゲート: `FAIL_UNDER_LINES=99` で非 0 終了することを確認（上記チェック 1 参照）
- CI タイムアウト検査: `ci.yml` からいずれかのジョブの `timeout-minutes` を一時的に
  除去したコピーに対して awk 検査を実行し、該当ジョブ名が欠落ジョブとして検出される
  ことを確認した
- 依存方向検査: `a -> b -> c -> a` の合成サイクルに対して DFS が循環を検出すること、
  および許可リスト外のエッジ（`bf-http -> backend-framework-core`）が違反として
  検出されることを確認した

## その他の検証（実装計画の検証方法一覧）

| 検証 | 結果 |
|------|------|
| `cargo build --workspace --all-features` | 成功 |
| `cargo fmt --all --check` | 差分なし |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 警告なし |
| `cargo nextest run --workspace --all-features --profile ci` | 全件成功（116 tests） |
| `cargo test --doc --workspace --all-features` | 全件成功 |
| `cargo tree -p bf-http` | `cargo-llvm-cov`・`cargo-nextest` は dev ツール（`cargo install`
  で導入したバイナリ）であり workspace の `Cargo.toml` 依存グラフには現れない。新規の
  ビルド依存追加なし |

## スコープ外（out-of-scope-tracking 準拠、実装計画どおり）

- AGENTS.md 本体の作成 → #35（TASK-11.3）。本イシューはチェック実装・PENDING 判定まで
- plugin 側 doc コメント網羅の残作業 → #76 / PR #109（マージ済み、TASK-11.2-2 として反映）
- NFR-8（AI 生成テストの注入リグレッション検知率 90%）の確認 → 親 #37 の受け入れ確認で実施
- HTTP パーサ fuzz → #87 / #88（TASK-15.3）
- feature 構成別カバレッジのマトリクス化・カバレッジの PR コメント自動投稿等の拡張 →
  必要になった時点で既存 Issue 確認の上ユーザー承認を得て起票（本イシューでは起票しない）
