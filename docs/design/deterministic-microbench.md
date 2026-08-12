# 決定的マイクロベンチ（alloc カウンタ）の導入（イシュー #615）

## 1. 背景・目的

実時間ベンチ（`benches/bench-accept.sh` 系）はホステッドランナー上でも VM 個体差・
共有テナンシーのノイズを受け、微小な性能退行（P1 ゼロコピー化級の per-request alloc
削減など）は測定分解能未満になる。`docs/design/bench-hosted-runner.md`（イシュー #611、
ベンチ判定安定化ツリー #607 Phase 1）は「方式 3（決定的計測）を補助指標として #615 で
導入する」と確定しており、引き渡し事項として (a) 計測依存は bench 専用クレート・
`dev-dependencies` に閉じ公開 13 クレートの依存グラフに影響させないこと、(b) 実行
カデンツ（毎回実行かオンデマンドか）を実装時に確定し同文書 5 節 2 項へ反映すること、
を指定していた。

本文書は、ノイズ非依存で 1 回実行により退行を検知できる決定的指標のベンチ
（`benches/microbench/`）の方式選定・カデンツ確定・運用方針を記録する。

## 2. 方式選定

### 2.1 採用: per-request alloc カウンタ方式

`docs/design/zero-copy-request-head.md` 5 節で実証済みの手法（`GlobalAlloc` を実装する
計測専用 crate を `#[global_allocator]` として使い、alloc 呼び出し回数・バイト数を
集計する）を常設ベンチ化した。選定根拠:

- 決定的（同一コード・同一 toolchain・同一プロファイルなら毎回同一値）で、1 回実行・
  しきい値ゼロの厳密比較が成立する
- ubuntu-latest で追加ツール（valgrind 等）不要、実行時間も秒オーダー
- P1 ゼロコピー化が守った alloc プロファイル（構造上定数 2 alloc/req 級）の退行を
  そのまま監視できる

### 2.2 カウンティングアロケータの実装: `stats_alloc`（dev 相当依存）に委ねる

計画段階では「自前で `unsafe impl GlobalAlloc` を書く」方針を想定していたが、
`crates/http/tests/alloc_count.rs`（イシュー #591、PR #602 レビュー指摘 P0 対応）に
本リポジトリ自身の先例があり、そこでは自前の `unsafe impl GlobalAlloc` から
`GlobalAlloc` を実装する既存の計測専用 crate `stats_alloc`（外部依存）へ置き換え済み
だった。同一の問題（`unsafe` の記述規律・レビュー負荷）を新規クレートで繰り返さない
ため、本実装（`benches/microbench/`）も `stats_alloc` を採用する。

この結果、`benches/microbench` は `#![forbid(unsafe_code)]` を crate 冒頭に置ける
（`GlobalAlloc` トレイト実装は `stats_alloc` 内部に閉じるため）。ハンドラ future の
駆動（後述）も `std::task::Waker::noop()`（1.85 で安定化）を使い自前の `RawWaker`
実装を避けたため、本クレートには `unsafe` が一切登場しない。**計画段階の
「外部依存ゼロ」という記述は、iai-callgrind（2.3 節）に対する評価軸であり、本クレート
内部の `unsafe` 回避には適用しない**（計画からの意図的な逸脱、実装時に確定）。

### 2.3 不採用（今回見送り）: iai-callgrind（命令数カウント）

- 新規外部依存（`iai-callgrind` crate + バージョン一致必須の `iai-callgrind-runner`
  バイナリ + valgrind ランタイム）が必要で、本リポジトリに criterion/iai 系の前例が
  なく導入コストが高い
- alloc カウンタで本イシューの受け入れ基準（決定的・ローカル + CI 再現・ベースライン・
  fail-closed）はすべて満たせる
- 命令数レベルの網羅（alloc を伴わない CPU 退行の検知）は将来の拡張候補として
  残す。`docs/design/bench-hosted-runner.md` 6 節が既に指摘するとおり、決定的計測は
  ランタイム分岐・非同期スケジューリング起因の退行（ロック保持中の await 等）を
  検知できないため、実時間計測（方式 1・2）の代替にはならない。Issue 起票はユーザー
  承認前提（`.claude/rules/out-of-scope-tracking.md`）のため本実装では行わず、
  実装 PR 本文で切り出し候補として報告する

## 3. 配置: `benches/microbench/` の standalone crate

`crates/http/fuzz`・`crates/plugin-webrtc/tests-e2e` と同じ確立済みパターン（空
`[workspace]` テーブルで独立 workspace 化、`publish = false`、path 依存で本体クレートを
参照）を踏襲する。root workspace の `cargo metadata`／`cargo tree`／`cargo geiger` に
一切現れず、`scripts/pay-for-what-you-use-check.sh` を偽陽性で汚さない。

計測対象は `fandhe-backend-http`（`parse_request_head`・`Response::serialize`）と
`fandhe-backend-routes`（`Router::dispatch`）の同期・決定的レイヤに限定する。

### 既知の限界: `crates/core` の非同期経路は対象外

`crates/core` の接続受理・tokio ランタイム経由の非同期処理はスケジューリング起因で
alloc 数が非決定になりうるため対象外とする。実時間退行クラス（ロック競合・OS
スケジューリング等）は方式 1（`benches/bench-accept.sh` 系）の守備範囲であり、本ベンチ
（方式 3）が補償するのは alloc レベルの**構造的退行クラスのみ**（`docs/design/
bench-hosted-runner.md` 5 節 2 項・6 節と同じ切り分け）。

### `Router::dispatch` の駆動

`Router::dispatch` は boxed future（`HandlerFuture = Pin<Box<dyn Future<Output =
Response> + Send>>`）を返すが、同期登録ハンドラ（`Router::route` / `Router::route_param`）
は内部で `std::future::ready` に包まれているため初回 poll で必ず完了する契約
（`crates/routes/src/lib.rs` の doc 参照）。本ベンチは tokio ランタイムに依存せず、
`std::task::Waker::noop()` を使った手書きの poll ループでこれを駆動する
（`MAX_POLLS` 回で完了しなければ「同期ハンドラ前提が崩れた」として panic する
fail-closed な前提条件チェック）。追加依存を増やさない。

## 4. 計測シナリオ・指標・比較方法

- **シナリオ**: 実時間ベンチ 4 エンドポイント相当をミラーする —
  静的ルート（`GET /health`）・パラメータルート（`GET /hello/{name}`、`GET /users/{id}`）・
  POST + ボディ（`POST /echo`）。各シナリオで「`parse_request_head` →
  `Router::dispatch`（+ future 完走）→ `Response::serialize`」の per-request パスを
  計測する
- **指標**: per-request の alloc 回数・alloc 総バイト数（`alloc`/`alloc_zeroed`/
  `realloc` を計上。`dealloc` は非計上）。`stats_alloc` は `realloc` 呼び出しを
  `Stats::allocations` に含めず `Stats::reallocations` として別カウントするため、
  回数側は `benches/microbench/src/main.rs` の `measure` で両者を合算して計上する
  （イシュー #619 Bugbot 指摘 Medium 対応。旧実装は `Stats::allocations` のみを
  読んでいたため `Vec`/`String` の容量拡張〔`realloc` 経由〕による呼び出し回数の
  増加を検知できなかった）。バイト側は `stats_alloc` 0.1.10 の `realloc` 実装が
  growth 分の差分を `Stats::bytes_allocated` へ既に加算しているため
  `change.bytes_allocated` のみで正しく計上できる（`Stats::bytes_reallocated` は
  正負混在の net 差分のため使用しない）
- **決定性の自己検証**: 1 回のウォームアップ後に各シナリオを 10 回実行し、全反復で
  計数が一致しなければベンチ自体が非 0 終了する（fail-closed の前提条件チェック。
  measure_scenario 関数）
- **比較方法**: ベースライン固定 + しきい値ゼロのラチェット方式
  （`scripts/unsafe-triage.sh` と同型）。コミット済み `benches/microbench/baseline.json`
  （シナリオ別 alloc 回数・バイト数 + メタデータとして rustc バージョン）と厳密比較し、
  **増加は即 exit 1**（fail-closed）、減少は exit 0 で通過しつつベースライン縮小を
  提案、`--update-baseline` で明示更新（レビュー承認前提）とする

## 5. カデンツ: ci.yml で PR/push 毎回実行

軽量（秒オーダー実行 + 小規模ビルド）なので、週次オンデマンドではなく **ci.yml の
`microbench` 常設ジョブとして PR/push ごとに毎回実行**する（`ci-complete` の needs に
組み込み済み）。これにより `docs/design/bench-hosted-runner.md` 5 節 2 項の受容条件
(a)（「#615 が方式 1 の判定と独立に毎回（少なくとも週次で）実行される構成になる」）が
成立する（ただし補償対象が alloc レベルの構造的退行クラスに限る点は不変。同文書へ
確定結果を反映済み）。

## 6. プロファイル固定・toolchain 差異への対応

`benches/microbench` は独立 workspace のため、root `Cargo.toml` の
`[profile.release] lto = true` は継承されない。alloc カウンタの計測値は最適化設定
（インライン化・定数畳み込み）に依存しうるため、`benches/microbench/Cargo.toml` に
`opt-level = 3` + `lto = true` を明示し「どの最適化設定で計測したか」を単一真実源として
固定する。

`baseline.json` には計測時の `rustc --version` 出力を `rustc_version` フィールドとして
記録する。rustc の stable 更新で std 内部の alloc 特性が変わった場合、コード変更なしに
計測値がずれることがある。この場合の運用は次のとおり:

1. CI（`microbench` ジョブ）が FAIL したら、まず差分がコード変更起因か toolchain 起因か
   `rustc_version` の変化で切り分ける
2. toolchain 起因と確認できた場合のみ、理由を明記して
   `bash benches/microbench.sh --update-baseline` でベースラインを更新する
   （レビュー承認前提、`.claude/rules/improvement-proposal.md` の「自動更新提案」に
   準じる運用）
3. コード変更起因（意図しない alloc 増加）の場合はベースライン更新ではなく実装側の
   原因調査・修正で対処する（安易な更新はラチェットの趣旨に反する）

`rust-toolchain.toml` は `channel = "stable"` を指定しており、`benches/microbench` は
ディレクトリツリー上方への探索でこれを継承する（独自の toolchain pin は持たない）。
fuzz 専用の pinned nightly（`scripts/fuzz.sh`）とは異なり、本ベンチは stable の自動
追従を許容し、上記の toolchain 差異対応手順で運用する（決定的計測の価値は
「同一環境での厳密比較」にあり、toolchain 固定までは要求しない設計判断）。

## 7. セキュリティ考慮（OWASP Top 10 観点）

- **サプライチェーン／脆弱な依存**: 新規外部依存は `stats_alloc`（計測専用）・
  `serde_json`（ベースライン JSON の読み書き）の 2 件のみで、いずれも独立 workspace
  内に閉じ公開 13 クレートの依存グラフに影響しない。`scripts/dep-audit.sh` は root
  workspace（`cargo metadata --no-deps` 起点）を走査対象とし `[workspace] exclude`
  クレートは対象外（既存の `crates/http/fuzz`・`tests-e2e` と同じ扱い）だが、本クレートは
  依存決定性の主張自体がサプライチェーン監査と表裏一体のため（codex-review PR #619
  P1 指摘対応）、`ci.yml` `microbench` ジョブ内で個別に `cargo audit`（コミット済み
  `benches/microbench/Cargo.lock` を対象）・`cargo deny check`（root の `deny.toml` を
  そのまま再利用）を実行し監査対象へ含める。依存解決自体は `benches/microbench/Cargo.lock`
  を **例外的にコミット対象**とし（`.gitignore` の `!benches/microbench/Cargo.lock`）、
  `cargo run/test/clippy --locked` で固定する。lockfile 非固定だとクリーンな CI 実行の
  たびに `serde_json` 等が再解決され、PR のコード変更なしに alloc 特性が変わって
  ラチェットが揺れる（逆方向の変化ではベースライン退行検知を隠す）ため、
  「同一コード・同一環境での厳密比較」という運用契約上 lockfile 固定は必須とする。
  依存追加・更新時は `cargo generate-lockfile --manifest-path benches/microbench/Cargo.toml`
  で明示的に再生成しコミットする
- **メモリ安全性**: `#![forbid(unsafe_code)]`（`benches/microbench/src/main.rs`
  冒頭）で unsafe を構造上排除する。`GlobalAlloc` 実装は `stats_alloc` 内部に閉じる
  （2.2 節）。`unsafe-triage.sh` の走査対象（`crates/*/src`）外である事実を明記する
- **fail-closed**: ベースライン欠落・パース失敗・決定性自己検証の不一致・指標増加は
  すべて非 0 終了する。暗黙スキップは作らない
- **リソース枯渇（DoS）**: ベンチは有界反復（`REPEAT = 10`）・有界入力サイズで実行し、
  CI ジョブに `timeout-minutes: 15` を設定（NFR-10 多層防御の既存方針に整合）
- **CI 権限最小化**: `microbench` ジョブはシークレット不要・`pull_request_target` 不使用。
  ワークフロー全体の `permissions` 最小権限方針を変更しない

## 8. 検証記録

初回ベースライン生成・決定性の再現確認・ミューテーション検知能力の検証記録は
`benches/reports/issue615-deterministic-microbench.md` を参照。

## 9. 将来の拡張候補（スコープ外）

- iai-callgrind 等による命令数レベルの決定的計測（2.3 節。alloc を伴わない CPU
  退行の検知）
- `crates/core` の非同期経路を含めた決定的計測（3 節「既知の限界」。tokio
  ランタイムのスケジューリング非決定性の扱いが未解決のため、現時点では実現方式が
  ない）

いずれもユーザー承認前提の Issue 化を要する（`.claude/rules/out-of-scope-tracking.md`）。
