# イシュー #480: 週次ベンチで `target/release/axum-ref` が「消失」した原因調査

対象: `.github/workflows/bench-schedule.yml`（週次 REQ-1/NFR-1 性能ベンチ）+
`benches/bench-accept-exclusive.sh` / `benches/bench-accept.sh` / `benches/lib/common.sh`。

## 1. 事象

run 30729910081（2026-08-02、runner: `fandhe-server-2-runner-1`）で、
`cargo build --release` が成功した直後に `bench-accept.sh` が

```
エラー: baseline バイナリ /home/runner/work/fandhe-backend/fandhe-backend/target/release/axum-ref が見つかりません
```

で失敗した（FAIL_RETRIES=1 の再試行も即失敗し、「性能退行 FAIL」として
`bench-regression` ラベルで Issue が誤起票された。→ #476 → #478）。

## 2. タイムライン（run 30729910081 の実ログ、UTC）

| 時刻 | 事象 |
|---|---|
| 03:02:24 | `actions/checkout` の `git clean -ffdx` 削除対象は **`Cargo.lock` のみ**（`target/` は存在しない） |
| 03:04:11〜03:05:32 | 専有ロック取得 → `cargo build --release`（`Compiling axum-ref v0.2.0` を含む約 360 パッケージのフルビルド）→ core-bench example ビルド → 「ビルド完了」 |
| 03:05:32〜03:07:02 | `wait_for_quiescence`（約 90 秒） |
| 03:07:02 | `[ ! -x target/release/axum-ref ]` で失敗。BLOCKED を経ずに「FAIL（性能退行）」へ丸められ誤起票（#478 が別途対処） |

前週 run 30185507560（2026-07-26）でも `git clean -ffdx` の削除対象は
`Cargo.lock` のみで、同一パターン。**ci.yml が日次でビルドしているにも
かかわらず runner の作業ディレクトリの `target/` が一度も観測されていない**
（`git clean -ffdx` が `target/` を消したことは 1 度もない = そもそも
そこにビルド成果物が生成されていない、と読むのが自然）。

同時間帯（02:00〜04:00Z）に fandhe-backend の他 run は存在しない（`gh run
list` で確認）。同一ホストの別 runner インスタンス
`fandhe-server-2-runner-4` で fandhe-frontend の release ジョブが並行実行
されていたが、作業ディレクトリは `/home/runner/work/fandhe-frontend/` で
別リポジトリ・別パスのため、直接の削除主体ではない。

## 3. 根本原因（確度: 高。本リポジトリの `.claude/worktrees/` 開発環境から
直接の実機再現はできなかったため「確証」ではなく「一次証拠から高確度で
推定される原因」として記録する。4 節に確認できたこと・できなかったことを
明記する）

**self-hosted runner フリート（org: Fandhe-AI）は、ジョブへホスト共有の
`CARGO_TARGET_DIR=/cargo-target` を注入する構成になっている**可能性が高い。
`cargo build --release` の実際の成果物はリポジトリ直下の `target/` では
なく `/cargo-target` 配下に生成されており、`benches/lib/common.sh` 等が
決め打ちしていた `${WORKSPACE_ROOT}/target/release/axum-ref` には最初から
存在しない。つまり「ビルド成功後に消失した」のではなく「そのパスには
最初から生成されていない」が実態であり、この場合は毎回・決定論的に
失敗するはずである（実際、直近 2 回の週次実行はいずれも同一の失敗）。

## 4. 一次証拠と確度の内訳

| 証拠 | 内容 | 確度への寄与 |
|---|---|---|
| `gh api repos/Fandhe-AI/fandhe-frontend/contents/.github/workflows/ci.yml` | 同一 org・別リポジトリの CI が `CARGO_TARGET_DIR（self-hosted runner 既定の /cargo-target）` と明記し、`CARGO_TARGET_DIR` 由来の無ハッシュ cdylib/rlib 混入を「イシュー #1192」として自己修復するガードステップを複数ジョブに持つ | 強い傍証（同一 runner フリートの既知挙動として文書化されている） |
| 本リポジトリの `.github/workflows/` | `CARGO_TARGET_DIR` の明示指定は本イシュー対応前は皆無 | 注入されていれば無条件に影響を受ける状態だったことと整合 |
| 週次 run ログ 2 回分（2026-07-26・2026-08-02） | 両方とも `git clean -ffdx` の削除対象が `Cargo.lock` のみで `target/` が一度も検出されていない | 「target/ 配下に成果物が生成されていない」という推定と整合 |
| 本調査環境（`.claude/worktrees/` の開発コンテナ）での実測 | `CARGO_TARGET_DIR` 環境変数は未設定、`cargo metadata --no-deps` の `target_directory` は `<worktree>/target`、`/cargo-target` は存在しない、`~/.cargo/config.toml` も存在しない | **この環境は self-hosted runner とは別のホスト・別のユーザー namespace（`/home/fandhe/...` であり runner ログの `/home/runner/...` とは異なる）であるため、ここでの不在は runner 側の注入を否定する証拠にはならない**（非対称性: 一致すれば強い確証になるが、不一致は「別環境だから」で説明できてしまう） |

**確認できなかったこと**: runner 実機（`fandhe-server-2-runner-1` 等）へ
直接アクセスして `CARGO_TARGET_DIR` の実値・`/opt/actions-runner*/.env`・
`~/.cargo/config.toml` の `build.target-dir` 設定を確認することは、本調査
環境（別ホストの隔離された開発コンテナ）からはできなかった。また、本
イシュー対応の作業方針（push・PR 作成は行わず、実装コミットのみをローカル
ブランチに積んで後続エージェントへ引き継ぐ）により、ブランチを origin へ
push して `gh workflow run bench-schedule.yml --ref <branch>` で実機の
`CARGO_TARGET_DIR` 注入有無をライブ確認する計画上のステップ（実装計画 1
節）も本セッションでは実施していない。次回の週次実行（またはブランチ
push 後の `workflow_dispatch` 手動実行）でジョブログの新設診断ステップ
「実効 target dir の診断記録（イシュー #480）」の出力を確認することで、
この推定を実証的に確定できる。

## 5. 対応方針が推定の正否に依存しない理由

本対応（5 節）は、原因が (a) 環境変数 `CARGO_TARGET_DIR` の注入、
(b) `~/.cargo/config.toml` の `build.target-dir` 設定、のどちらであっても
機能する设計にしてある:

- `.github/workflows/bench-schedule.yml` のジョブへ
  `env: CARGO_TARGET_DIR: ${{ github.workspace }}/target` を設定し、
  ホスト共有 `/cargo-target`（原因が (a) の場合）から明示的に隔離する。
  ジョブローカルな環境変数は他の設定ソースより優先されるため、原因が
  仮に (b) だった場合でもこの明示指定が上書きする。
- `benches/lib/common.sh` の `BENCH_TARGET_DIR` 導出は `cargo metadata
  --no-deps` の `target_directory`（cargo 自身の権威値。(a)(b) いずれの
  設定も正しく反映する）をフォールバックとして持つため、環境変数注入を
  仮に見落としていたとしても実効パスを正しく解決できる。

つまり、根本原因の特定に多少の不確実性が残っていても、本対応は
「決め打ちパスに依存しない」という設計変更そのものによって症状を解消する。

## 6. 対応内容（実装済み）

1. `.github/workflows/bench-schedule.yml`: ジョブへ `CARGO_TARGET_DIR:
   ${{ github.workspace }}/target` を設定（ホスト共有 target からの隔離）。
   併せて恒久の診断ステップ（`CARGO_TARGET_DIR` env・`rustc -vV`・
   `cargo --version` を記録）を追加し、次回以降のトリアージで同種の
   「実効パスが分からない」事態を防ぐ。
2. `benches/lib/common.sh`: 実効 target ディレクトリを導出する
   `BENCH_TARGET_DIR`（`CARGO_TARGET_DIR` env → `cargo metadata` →
   `${WORKSPACE_ROOT}/target` の優先順位）を追加。`TARGET_BIN` の既定値を
   これに追従させた。
3. `benches/bench-accept.sh` / `benches/bench-accept-exclusive.sh`:
   `BASELINE_BIN` / `CORE_BIN` の既定値を `BENCH_TARGET_DIR` ベースへ変更。
   `bench-accept-exclusive.sh` にはビルド直後・静穏確認前（quiescence 待ち
   最大 30 分の前）にバイナリ実在を検査する fail-fast を追加し、欠如時は
   `FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE`（2）で即 BLOCKED 終了する
   （待機を浪費せず、`bench-accept.sh` 側の FAIL/BLOCKED 判定
   〔#478 の担当範囲〕とは独立に、実効パス不一致という別要因を先に
   排除する）。
4. 残る 8 本のベンチスクリプト（`webrtc-nfr6-bench.sh` 等）の
   `target/release/...` 決め打ちの既定値を機械的に `BENCH_TARGET_DIR`
   ベースへ置換した（同一欠陥クラスの水平修正）。ただし
   `bench-ws-load.sh` の `AXUM_BIN` は `--target-dir target/ws-bench` を
   明示指定してビルドする専用出力先であり、cargo の `--target-dir` CLI
   引数は `CARGO_TARGET_DIR` env より優先されるため対象外とした。

## 7. スコープ外（別イシュー化を検討、承認待ち）

- `scripts/accept/*.sh`（`webrtc-accept.sh` / `hub-wiring-accept.sh` /
  `graphql-accept.sh` / `websocket-accept.sh` / `tracing-accept.sh`）にも
  同一の `${WORKSPACE_ROOT}/target/release/...` 決め打ちが存在する。これらは
  人間が手動実行する受け入れ検証スクリプトであり週次自動実行の対象外だが、
  同一環境（self-hosted runner を手動実行環境として使う場合）では同じ影響を
  受けうる。本イシュー（#480）の実装計画は対象ファイルとして `benches/**`
  のみを列挙しており `scripts/accept/**` は含まれないため、本コミットでは
  変更していない。out-of-scope-tracking 規約に従い、別イシューとしての
  切り出しをユーザーへ提案する。
- `Cargo.lock` が `.gitignore` 対象で週次ベンチのたびに最新解決の依存
  バージョンで計測される再現性の課題（今回ログで `Locking 339 packages` を
  確認）。本イシューのスコープ外。
- self-hosted runner フリート側のホスト共有 `CARGO_TARGET_DIR=/cargo-target`
  運用自体の是非（org インフラの課題、本リポジトリ外）。

## 8. 検証方法（次回実行時に確認する）

1. ブランチ push 後、`gh workflow run bench-schedule.yml --ref <branch>` を
   実行し、新設の診断ステップに `CARGO_TARGET_DIR=<workspace>/target` が
   出力されること（ジョブローカル env が効いていること）を確認する。
2. `cargo build --release` 成功直後、バイナリ未検出が再発しないこと
   （PASS/FAIL の実判定まで到達すること）を確認する。
3. 誤った `bench-regression` Issue が起票されないことを確認する。
4. マージ後、次回週次 schedule（日曜 02:00 UTC）でも再発しないことを確認する。
