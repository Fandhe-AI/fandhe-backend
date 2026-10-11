---
paths:
  - ".github/workflows/**"
  - ".github/actions/**"
  - ".github/actionlint.yaml"
description: >
  GitHub Actions を編集する際の規約。runner 方針（public は GitHub ホステッド）・必須チェック名と
  集約ジョブ ci-complete・発火条件・ジョブ追加チェックリスト・計測方法・キャッシュ方針（上限 20 GB・
  予算 $10・main のみ保存・キー設計）を規定する。
---

# CI 実行環境規約

## Runner 方針（GitHub ホステッド既定）

GitHub Actions のジョブは **GitHub ホステッドランナー（`runs-on: ubuntu-latest` 等の
標準スペック）を既定とする**（ユーザー指示 2026-08-08。本リポジトリは 2026-08-08 に
public 化されており、public リポジトリでは標準ホステッドランナーが無料・分数消費なし）。

本規約は組織 runner 方針（ユーザー決定 2026-08-07）「リポジトリの**可視性**で runner を
決める: public は GitHub ホステッド（`ubuntu-latest` 等）、private は self-hosted」の
**public 側**の適用である。方針の正は Fandhe-AI/actions の
[`docs/runner-policy.md`](https://github.com/Fandhe-AI/actions/blob/main/docs/runner-policy.md)
（Fandhe-AI/actions#33 の成果物。対象リポジトリ一覧・ai-review（旧 codex-review）の
self-hosted 専用 runner 例外を含む）を参照し、本節では書き写さない（ドリフト防止）。

- 新規ジョブ追加時もホステッドランナーを使用し、`runs-on: self-hosted` を使わない。
  larger runner（有料の大型ホステッドランナー）も使わない
- **唯一の例外（ai-review の codex 実行ジョブ）**: `ai-review.yml`（旧 codex-review.yml）
  が呼び出す reusable workflow の `review` ジョブ（`runner: codex`）のみ、codex-home
  方式の認証情報を runner 上に配置する構成のため self-hosted な codex 専用 runner の
  使用を認める（`docs/runner-policy.md` §3 の明文例外）。例外は `review` ジョブに閉じる:
  資格情報に触れない `preflight` / `post_feedback` ジョブ（`post-feedback-runner`）は
  ubuntu-latest を明示指定し、この例外を根拠に他ジョブを self-hosted 化しない
- 旧方針（private 前提の self-hosted 既定、PR #549）は public 化に伴い廃止。既存
  ワークフローの移行はトラッキングイシュー #550（子 #551〜#556）で実施した
- self-hosted カスタムラベル（`.github/actionlint.yaml` の `self-hosted-runner.labels`
  ホワイトリスト）は codex 例外のためにのみ残す。未登録ラベルは `scripts/actionlint.sh`
  が runner-label エラーで検知する（fail-closed 構成、不変）。ラベルの実体は
  Fandhe-AI/local-server の gha-runner 手順書で管理する
- codex 専用 runner は org（Fandhe-AI）レベルで登録されている。リポジトリの runner
  一覧（`gh api repos/{owner}/{repo}/actions/runners`）が 0 件でも正常

## ホステッド前提への読み替え（旧 self-hosted 前提だった箇所）

- ホステッドランナーはジョブごとにクリーンな使い捨て VM のため、ツールの
  「導入済みならスキップ」冪等判定（`Fandhe-AI/actions/cargo-tool-install` 等）は
  常に新規インストール側へ倒れる。バージョン固定・検証付きの毎ジョブ導入が主経路に
  なるため、既存の導入ステップは削除・弱体化しない
- Rust toolchain の導入は `dtolnay/rust-toolchain`（コミット SHA 固定）を正とする
  （self-hosted の永続環境前提だった `Fandhe-AI/actions/rust-toolchain-setup` の代替。
  イシュー #551 で確立。旧 `docs-site.yml` build ジョブが先行事例で、同 workflow は
  `pages.yml` へ移行済み）
- ビルドを伴うジョブは cargo registry / `target` をキャッシュしてよい。キー設計・保存条件・
  容量の扱いは下記「キャッシュ方針」に従う。キャッシュは最適化であり正当性の前提にしない
  （miss はフルビルドへフォールバック、fail-open）

## 運用ルール（runner 種別に依らず維持）

- **全ジョブに `timeout-minutes` を設定する**。ハングしたジョブが runner を無期限に
  占有するのを防ぐ（TASK-11.4 / NFR-10 の多層防御。テスト実行は cargo-nextest の
  テスト単位タイムアウトと併用する）
- **schedule 実行は軽量に保つ**。日次 schedule では dep-audit のみを走らせ、
  ビルドを伴うジョブ（fmt/clippy/test/openapi 系）は `if: github.event_name != 'schedule'`
  で除外する（旧 self-hosted 時代は runner 負荷抑制が動機。ホステッドでも無駄な
  ジョブ実行・Actions キュー消費を避ける原則として維持する）
- schedule 系ワークフロー同士は cron をずらして負荷を分散する
  （例: ci.yml 00:30 UTC / update-external.yml 00:00 UTC / bench-schedule.yml
  週次一次判定 02:00 UTC 日曜・月次二次判定 04:00 UTC 毎月 1 日、イシュー #614。
  日曜と重なる月初でも `concurrency` の直列化により同時実行しない）
- **週次ベンチ workflow（`bench-schedule.yml`、イシュー #285）は「日次 schedule は
  dep-audit のみ」方針の例外ではない**。REQ-1/NFR-1 性能ベンチ（`benches/
  bench-accept-exclusive.sh`）はビルド + 専有計測を伴い重いため、ci.yml の日次
  schedule には相乗りさせず、別 workflow・週次実行に切り出すことで両立させる
  （設計比較は `docs/design/bench-scheduled-run.md` 参照）
- **セキュリティ**: ワークフローの `permissions` は最小権限（原則 `contents: read`）とし、
  fork からの PR に対してシークレットを露出するトリガー（`pull_request_target` 等）を
  追加しない（[[security]]。public 化により fork PR の可能性が現実化したため、旧
  self-hosted 時代より重要度が上がっている。codex 専用 runner（唯一の self-hosted
  例外）は永続環境のため、fork PR での実行拒否等の多層防御を弱体化しない）

## 検証

| 検証 | コマンド |
|------|---------|
| runs-on の確認 | `grep -rhE "^[[:space:]]*runs-on:" .github/workflows/ \| sed -E 's/^[[:space:]]*runs-on:[[:space:]]*//' \| sort -u \| grep -vxF ubuntu-latest \| grep -vxE '\$\{\{[[:space:]]*matrix\.os[[:space:]]*\}\}'`（移行完了後は出力が空であること。値全体（`runs-on:` 接頭辞のみ `sed` で除去した行全体。旧 `awk '{print $2}'` の先頭トークン切り出しでは matrix 式の `${{` のみが残り self-hosted ラベルと機械的に区別できなかったため、値全体比較へ変更した）を `ubuntu-latest` リテラルと `${{ matrix.os }}`（イシュー #679 で `ci.yml` の `fmt`/`clippy`/`test` ジョブに導入、matrix 値は `ubuntu-latest`/`macos-latest`/`windows-latest` の GitHub ホステッドのみ）の 2 パターンのみ許可リストとして除外する。許可リストはこの `matrix.os` 参照 1 式に限定したリテラル一致（フェイルクローズ、他の matrix フィールド名や未知の `${{ }}` 展開は自動では許可されず要目視判定）で、self-hosted ラベル（例: `runs-on: self-hosted`）はもちろん `${{ matrix.os }}` 以外の任意の `${{ }}` 展開もこの 1 パターンに一致しないため引き続き出力され違反候補として検知される。ただし本ワンライナーは `runs-on:` の値の形が許可パターンに一致するかのみを見ており、`matrix.os` 自体に列挙される値（`ubuntu-latest`/`macos-latest`/`windows-latest` 以外の値、例えば `self-hosted` が `strategy.matrix.os` に追加された場合）までは検査しない。matrix 値一覧の妥当性は本コマンドの検査範囲外であり、次段の `scripts/actionlint.sh` を含め別途目視確認が必要。codex 例外は reusable workflow の `runner-label` input 経由のため本コマンドには現れない。`runs-on:` のスカラー表記のみ対応し、配列 `[a, b]` 表記は誤検知しうる。#551〜#555 で全ワークフローの移行が完了済みのため、上記 2 パターン以外の出力は常に違反である） |
| ラベル未登録・typo の機械検知 | `bash scripts/actionlint.sh`（`.github/actionlint.yaml` 未登録の `runs-on` ラベルを runner-label エラーとして検知。actionlint 未導入環境では前提ツールエラーで exit 2 になる） |
| timeout の確認 | 各ジョブに `timeout-minutes` があることを目視確認 |

CI ジョブ構成の変更時は本ルールへの準拠を `reviewer` が確認する。

## 必須チェックと集約ジョブ

ruleset `main-protection` の required status checks は **ジョブ名（`name:`）** で参照される
（2026-10-11 時点。実体は `gh api repos/Fandhe-AI/fandhe-backend/rulesets` で確認する）。

- `ci.yml` の全ジョブ名（matrix は `cargo fmt --check (<os>)` / `cargo clippy (<os>)` /
  `cargo test (<os>)` の 3 OS 展開名ごと）と集約ジョブ `ci-complete`
- `ai-review.yml` の `codex / preflight`・`codex / review`・`codex / post_feedback`
- 外部アプリの `Cursor Bugbot`
- branch protection（classic）は未設定。ruleset のみが正

守ること:

- **ジョブ名（`name:`）・matrix の os 一覧を変えると required check が pending のまま
  マージ不能になる**。変更は ruleset の更新（`scripts/setup-required-checks.sh`）と同時にユーザー判断を仰ぐ
- ジョブを追加したら `ci-complete` の `needs` とシェル判定（`RESULT_*`）にも追加する
  （[[git-hooks]] の判断手順 3 と同じ）
- **required check を含む workflow に workflow 単位の `paths` / `paths-ignore` を付けない**
  （一致しないとチェックが報告されず pending のままになる）。matrix ジョブをジョブレベルの
  `if` で skipped にすると、展開前の名前（`${{ matrix.os }}` 未展開）で報告され
  `cargo fmt --check (ubuntu-latest)` 等の required check が満たされない。変更検知で省略する
  場合はステップレベルの `if` を使い、ジョブ自体は常に実行して success を返す
- `standalone-crates-io.yml` は required ではないため workflow 単位の `paths` を許容する

## 発火条件の方針

- `pull_request`（`branches: [main]`）と `push`（`branches: [main]`）のみ。feature ブランチへの
  `push` トリガーは付けない（PR と二重実行になる）
- `concurrency` は `group: <workflow>-${{ github.ref }}` +
  `cancel-in-progress: ${{ github.event_name == 'pull_request' }}`（PR のみ打ち切り。main・
  schedule・tag・release 系は打ち切らない）。リリース・pages・bench 系は `cancel-in-progress: false`
- draft PR の扱い: 現状は draft でも実行する（required check の skipped 化は上記の通り
  マージ不能リスクがあるため、導入する場合はユーザー判断）
- schedule は軽量に保つ（上記「運用ルール」）。頻度: ci.yml 日次 / update-external.yml 日次 /
  bench-schedule.yml 週次 + 月次 / standalone-crates-io.yml 週次。増やさない
- `merge_group` は未使用（merge queue 未導入）

## ジョブ・ワークフロー追加チェックリスト

1. 新しいワークフローを増やさず、既存 `ci.yml` へジョブを追加する（トリガーが同じなら統合）
2. `ci-complete` の `needs` と判定へ追加し、ruleset の required check へ追加する（ユーザー判断事項）
3. `runs-on` は `ubuntu-latest`（上記 runner 方針）、`timeout-minutes` を設定、`permissions` は最小
4. 不要な `needs` を作らない（直列化すると wall-clock が伸びる。集約は `ci-complete` のみ）
5. checkout は既定の `fetch-depth: 1`、submodule は必要なジョブだけ取得する
6. ビルドを伴うなら「キャッシュ方針」の 3 ステップ（`Cache cargo registry and target`
    = `actions/cache/restore`、`Slim cargo target for cache`、`Save ... (main only)` =
    `actions/cache/save`）を既存ジョブからコピーして付ける。
    `cargo install` で導入するツールは `./.github/actions/cargo-tool-install-cached` を使う
    （`~/.cargo/bin/<tool>*` をバージョン込みキーでキャッシュし、導入検証は
    `Fandhe-AI/actions/cargo-tool-install` に委ねる）。素の `cargo install` をステップに書かない
7. action のピン止めは既存と同じ（サードパーティはコミット SHA。`Fandhe-AI/actions` は `@latest`）
8. 処理の定義は `scripts/*.sh` に置き、ワークフローは呼ぶだけにする（ローカルで再現できること。
    [[makefile]] 参照）。lefthook と重複する検査は CI 側を正とする（[[git-hooks]]）
9. 変更後は `actionlint`（`bash scripts/actionlint.sh`）を通し、PR の CI で前後の所要時間を比較する

## 計測方法

```bash
gh run list --workflow ci.yml --limit 30 --json databaseId,conclusion,createdAt,updatedAt,event
gh run view <run-id> --json jobs   # ジョブ・ステップ別の startedAt / completedAt
```

## 棚卸し結果（2026-10-11、直近 30 run 実測）

| workflow | 中央値 | 最大 | 備考 |
| --- | --- | --- | --- |
| ci.yml | 15.3 分 | 20.6 分 | クリティカルパスは `cargo test (windows-latest)` 16 分（cargo-nextest のソースビルド 7.4 分） |
| ai-review.yml | 1.6 分 | 32.9 分 | self-hosted codex 専用 runner（例外） |
| pages.yml / standalone-crates-io.yml / update-external.yml | 1.3〜1.8 分 | 3.5 分 | 軽量 |
| bench-schedule.yml | 20.2 分 | 89.2 分 | 週次・月次のみ |
| release.yml | 16.9 分 | 62.5 分 | タグ時のみ |

`cargo install` 由来のソースビルドが最長ジョブの大半を占めていたため、ツールバイナリを
キャッシュする方針を採った（`cargo-nextest` / `cargo-geiger` / `cargo-llvm-cov` / `cargo-fuzz` /
`cargo-audit` / `cargo-deny` / `oha`）。release.yml は検証経路の保守性を優先し対象外。

## キャッシュ方針

### 前提（2026-10-11）

- リポジトリのキャッシュ上限は **20 GB**（既定 10 GB から引き上げ）。Actions の予算は **$10/月**
  （超過時停止）で、**10 GB を超えた分は 1 GB あたり月 $0.07 の課金**になる。定常の main 一式は
  10 GB 未満（課金ゼロ）に収め、20 GB は世代交代時の一時的な二重化の余裕として使う
- 上限を超えると GitHub は **最終アクセス時刻の古い順（LRU）** に追い出す。restore は run 冒頭、
  save は数分後に集中するため、1 run の保存量が空き容量を超えると、その run の冒頭で復元した
  数 MB のツールキャッシュ（`cargo-tool-*`）が直後に保存した数 GB より「古い」と判定されて
  先に消える。ツールキャッシュを守る条件は「保存総量を上限より十分小さく保つ」ことで、
  キー設計では守れない。7 日間アクセスの無いキャッシュも削除される
- PR（`refs/pull/<n>/merge`）スコープのキャッシュは同じ PR からしか復元できない。main スコープは
  全 PR から復元できる

### ルール

1. **保存は main のみ**。ビルドキャッシュは `actions/cache/restore`（`id: cargo-cache`）と
   `actions/cache/save`（`if: github.ref == 'refs/heads/main' && steps.cargo-cache.outputs.cache-hit != 'true'`、
   `key: ${{ steps.cargo-cache.outputs.cache-primary-key }}`、`path` は restore と完全一致）に分割し、
   PR は restore のみにする。トレードオフ: Cargo.toml を変える PR は 2 回目以降の push でも
   main の世代から部分復元（restore-keys）になる（同じ PR 内での exact hit は無くなる）
2. **キー**: `<family>-${{ runner.os }}-v<N>-${{ steps.toolchain.outputs.cachekey }}-${{ hashFiles('**/Cargo.toml', '!.agents/**', '!.claude/**') }}`、
   restore-keys は `<family>-${{ runner.os }}-v<N>-${{ steps.toolchain.outputs.cachekey }}-`。
   - `hashFiles` からスキルのサンプル（`.agents/**`・`.claude/**` 配下の Cargo.toml）を除外する。
     含めるとスキル更新 PR のたびに全系統のキーが変わり、全ジョブ miss → 全保存 → 追い出しになる
     （#779 で実測: 15/15 ジョブ miss）
   - 再帰 glob（`**/Cargo.toml`・`**/Cargo.lock` 等）は必ず `!.agents/**`・`!.claude/**` を付ける。
     `.agents/skills/make/samples/` には Cargo.toml・Cargo.lock があり、`.claude/skills/make` は
     そこへの symlink のため、除外しないと依存が変わらなくてもキーが変わる
   - **rustc の実体をキーに含める**: `dtolnay/rust-toolchain` の `steps.toolchain.outputs.cachekey`
     （rustc の commit 日付 + hash）を使う。toolchain は action が毎回導入するため runner イメージの
     rustc に依存しない。runner の rustc やキーに出ない toolchain を使うと、rustc 更新後も
     exact hit したまま旧 rustc の target を復元し、依存を毎回全再ビルドする（他リポジトリで実例）。
     pinned nightly（fuzz）は `scripts/fuzz.sh` をハッシュに含めて版を反映する
   - SHA・run_id 等の一意値をキーに入れない
   - ビルド設定（下記 env・プロファイル・slim の対象）を変えたら版 `v<N>` を上げる。旧設定の
     成果物を復元して積み増すと、初回保存が「旧 + 新」で逆に太るため
3. **保存前に target を縮める**: `bash scripts/ci-cache-slim-target.sh [--manifest-path <standalone>] <target>`
   でワークスペース自身の成果物（lib・テスト・example・bin・incremental・`doc/`）を除く。
   actions/checkout が mtime をチェックアウト時刻にするため、ワークスペースのクレートは
   復元しても毎回再ビルドされ、保存しても再利用されない。第三者依存の成果物だけを残す。
   PR でも実行してスクリプトの退行を 3 OS で検知する（数秒）
4. **ビルド設定**: `ci.yml` の workflow `env` で `CARGO_INCREMENTAL=0`・
   `CARGO_PROFILE_DEV_DEBUG=line-tables-only`。release ビルド（pay-for-what-you-use のバイナリ
   サイズ計測・microbench）・fuzz には影響しない
5. **旧世代の削除**: main への push 後に `cache-prune` ジョブ（`scripts/ci-cache-prune-generations.sh`、
   `actions: write`）が、main スコープの同じ `<family>-<OS>` で最大版・最新の 1 件だけを残す。
   品質ゲートではないため `ci-complete` の needs・required check には含めない（PR では skipped）
6. **ツールバイナリ**は `./.github/actions/cargo-tool-install-cached`（数 MB、
   `cargo-tool-<tool>-<version>-<OS>-<arch>`）。小さいため PR からの保存も許容する
   （main 未保存のバージョンを PR が導入した場合のみ PR スコープに保存される）。
   週次の `bench-schedule.yml` だけが使う `oha` は 7 日の未使用期限と同じ周期のため、
   消えていればソースビルドにフォールバックする
7. キャッシュを新設・拡大する場合は、main 一式が 10 GB 未満に収まるかを下記コマンドで確認し、
   本節の実測表を更新する。効果（復元時間 < 節約時間）が無いジョブにはキャッシュを付けない

### 計測方法

```bash
gh api repos/Fandhe-AI/fandhe-backend/actions/cache/usage                 # 使用量
gh api repos/Fandhe-AI/fandhe-backend/actions/cache/storage-limit         # 上限
gh api --paginate "repos/Fandhe-AI/fandhe-backend/actions/caches?per_page=100" \
  --jq '.actions_caches[] | [.key, .ref, .size_in_bytes, .created_at, .last_accessed_at] | @tsv'
# ヒット判定（exact = primary key で復元 / partial = restore-keys で別キー / miss = Cache not found）
gh run view <run-id> --log | grep -E "Cache restored from key|Cache not found for input keys|Cache saved with key|Cache Size"
bash scripts/ci-cache-prune-generations.sh --dry-run                     # 旧世代の削除対象
```

### 実測（2026-10-11）

変更前（直近 10 run = PR 7・main push 3、ビルドキャッシュ 150 ステップ）:

| 項目 | 値 |
| --- | --- |
| 使用量 | 16.5 GB / 18 件（当時の上限 10 GB を超過、追い出し待ち） |
| main スコープ | 8.2 GB / 13 件（`test-Linux` 2.9・`coverage` 2.9 GB 等。`test-macOS`/`test-Windows` は追い出し済み） |
| PR スコープ（マージ済み #778/#779） | 8.2 GB / 6 件（`test-Windows` 2.2 GB × 2・`coverage` 2.9 GB 等、誰も復元できない死蔵） |
| `cargo-tool-*` | 0 件（#778 の PR スコープで作成後、LRU で全滅） |
| ヒット率 | exact 95（63%）/ partial 11（7%）/ miss 44（29%） |
| miss の原因 | スキルサンプルの Cargo.toml によるキー変化（#778・#779）と追い出し（38119559192 で `test-Linux-…-` の prefix すら見つからない） |

target 縮小のローカル実測（macOS、`cargo test --workspace --all-features --no-run`、非圧縮）:

| 設定 | target | 内訳 |
| --- | --- | --- |
| 従来（incremental・debug=2） | 7.6 GB | ワークスペース成果物 5.1 GB・incremental 1.3 GB・第三者依存 1.3 GB |
| `CARGO_INCREMENTAL=0` + `line-tables-only` | 4.2 GB | ワークスペース成果物 3.2 GB・第三者依存 1.0 GB |
| 上記 + `ci-cache-slim-target.sh` | 1.2 GB | 第三者依存のみ（再ビルドはワークスペース 15 クレートのみ） |

変更後（PR #780 の CI、v2 キーは main 未保存のためビルドキャッシュは全系統 cold・ツールは main から hit）:

| 項目 | 値 |
| --- | --- |
| PR からの保存 | 0 件（`Save ... (main only)` は全ジョブ skipped、PR スコープのエントリ増加なし） |
| `cargo-tool-*` | 8 件すべて main スコープに存在し全ジョブで exact hit |
| `Slim cargo target for cache` | 1〜7 秒（3 OS で動作。macOS の `/bin/bash` 3.2 互換） |
| ワークフロー所要 | 7.9 分（クリティカルパスは fuzz smoke 7.8 分。fuzz 実行時間が支配的） |
| `cargo test` cold | ubuntu 2.7 / macOS 3.1 / windows 3.9 分（変更前は warm でも 3.6〜7.2 / 7.0〜15.7 分） |
| `cargo clippy` cold | 1.3〜1.9 分 |

line-tables-only・incremental 無効化で cold ビルド自体が速くなったため、ビルドキャッシュの寄与は
ツールキャッシュより小さい。マージ後に main の v2 世代のサイズと warm 時の所要時間を計測し、
復元時間に見合わないジョブはキャッシュを外す（ルール 7）。
