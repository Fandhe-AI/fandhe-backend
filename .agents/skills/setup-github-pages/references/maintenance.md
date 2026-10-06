<!-- source: skills/setup-github-pages（このスキル自身の保守手順。上流は https://github.com/Fandhe-AI/fandhe-frontend） -->
<!-- 最終確認日: 2026-10-05 -->
<!-- 取得状況: ✅ 実装とテストで確認済み（FF_REV b3e31ef663a98b6080feb98c84ade238d1074a08 で匿名ビルド・生成・rebrand を実測） -->

# スキル保守者向けの手順（FF_REV の更新・上流改修の追跡）

この文書は**スキル自身（このリポジトリの `templates/`）を保守する人**向け。構築済みの対象リポジトリへ反映する
利用者の手順は SKILL.md の「更新フロー」（Step U0〜U5）で、保守者がスキルを更新して配布したあと、利用者が再実行して取り込む。
両者を混同しない。

## FF_REV の更新手順

現在の固定値: `b3e31ef663a98b6080feb98c84ade238d1074a08`（2026-10-05 時点で匿名ビルド・生成・rebrand を実測済み）。

唯一の定義元は `tools/docs-site-gen/FF_REV`（スキル側は `templates/docs-site-gen/FF_REV`）。`pages.yml` と
`build-local.sh` は値を直書きせずこのファイルを読み、使用前に `^[0-9a-f]{40}$` で検証する。cache キーも同ファイルの
ハッシュを含むため、変更すると古いビルド成果物は再利用されない。

更新するときは次の順で行う。

1. 上流の新しい commit を確認する（`gh api repos/Fandhe-AI/fandhe-frontend/commits/main --jq .sha`）。**固定値は 40 桁の
   commit SHA のみ**。ブランチ名・タグは使わない
2. `FF_REV` を書き換える。このスキル内では `templates/docs-site-gen/FF_REV` と本節の「現在の固定値」を同時に更新する
   （`tests/rev-pin.test.mjs` が不一致を検出する）。対象リポジトリ側への反映は保守者の作業ではなく、利用者が更新フローで
   取り込む（`scaffold.py` を通さず対象リポジトリの `FF_REV` だけ手で書き換えると、次回の更新は「利用者が編集した」競合として止まる）
3. 模擬の対象リポジトリ（`scaffold.py` で配置したもの。更新の確認は旧版を配置してから新版で `scaffold.py --target .`
   を再実行する）で `bash tools/docs-site-gen/build-local.sh --clean --write-third-party` を実行する。匿名・隔離環境（`GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1` と使い捨ての `CARGO_HOME`）で `cargo install --git ... --locked` が通ることを実測する。
   `build-local.sh` は install 前に新しい rev の上流 `Cargo.lock` を取得し、docs-site から辿れる依存に source 付き（registry 等）が増えていれば停止する。
   停止したら供給網の固定方針を見直すまで更新しない（検査済みの記録は `docs-site-install/.registry-checked` に rev で残り、同一 rev では再取得しない）。`docs-site` の CLI 引数（`--root`・`--out`・`--no-page-sections`）の変更は生成の失敗として現れる
4. `rebrand_site.py` が「一致数が 0」で失敗したら、上流 DOM の変更を意味する。生成された HTML を読み、ヘッダー・フッターの
   該当要素を特定して `rebrand_site.py` のルールを直す。`brand.toml` に無い新しいハードコード表示が増えていないかも、
   `grep -o fandhe-frontend` と `Fandhe-AI` で確認する
5. 帰属表記の件数を再確認する。`grep -o fandhe-frontend _site/index.html | wc -l` が 3（`Built with …` と LICENSE リンク 2 件）の
   ままであること。増減していれば上流がフッターを変えている
6. `references/site-format.md` の制約（nav.toml の書式・予約パス・予約アセット・Markdown 対応範囲）が変わっていないか上流
   ソースで再確認し、`scripts/check_site.py` の `RESERVED_ASSET_NAMES`（上流 `build.rs` の `RESERVED_ASSET_NAMES`）を更新する
7. `tests/fixtures/raw/` を新 rev の生成物で作り直し、`node --test "tests/*.test.mjs"` を通す

## 上流改修の追跡

本スキルの wrapper と後処理は、上流 fandhe-frontend の次の制約を回避するための**暫定措置**である。上流は外部利用に対応済み
（PR #3728〜#3737 がマージ済み）で、スキル側の回避策は #49〜#53 で削除する（#49 は反映済み）。表の「上流の対応」は上流側の状態を指し、
スキル側の回避策は行ごとの記載のとおり。

| 回避している制約 | 上流の対応（Issue → PR） | スキル側の削除（イシュー） |
|------------------|--------------------------|----------------------------|
| stock の `docs-site` が page section registry を強制し、ショーケースを注入する | [#3716](https://github.com/Fandhe-AI/fandhe-frontend/issues/3716) → PR #3728（`--no-page-sections`）、[#3717](https://github.com/Fandhe-AI/fandhe-frontend/issues/3717) → PR #3731（ショーケース注入の停止） | `--no-page-sections` の利用は #49 で反映済み。wrapper の削除は #53、予約パス禁止の撤去は #52 |
| ブランド表示がハードコード | [#3720](https://github.com/Fandhe-AI/fandhe-frontend/issues/3720) → PR #3732（`brand`・`repository_url`）、[#3721](https://github.com/Fandhe-AI/fandhe-frontend/issues/3721) → PR #3733（`tagline`・`copyright`・`version_badge`・`lang`）、[#3722](https://github.com/Fandhe-AI/fandhe-frontend/issues/3722) → PR #3734（`brand_mark`・`brand_color`） | `rebrand_site.py` と `brand.toml` の廃止（#50・#51・#53） |
| `cargo install --git` が submodule で失敗する | [#3718](https://github.com/Fandhe-AI/fandhe-frontend/issues/3718) → PR #3730（submodule 非依存化。匿名の `cargo install --git` が通る） | #49 で反映済み（匿名 `cargo install --git`。`_ff/` と path 依存のビルドは廃止。wrapper テンプレートの削除は #53） |
| 外部利用の契約・手順 | #3724 → PR #3735（契約テスト）、#3725 → PR #3737（CI 経路）、#3726 → PR #3736（利用ガイド）、#3715 → PR #3729（設計文書） | 参照のみ（#56 で文書をリンク化） |

追跡 Issue: [https://github.com/Fandhe-AI/fandhe-frontend/issues/3713](https://github.com/Fandhe-AI/fandhe-frontend/issues/3713)（トラッキング。2026-10-05 時点で open）

- 未完了: [#3723](https://github.com/Fandhe-AI/fandhe-frontend/issues/3723) と [#3727](https://github.com/Fandhe-AI/fandhe-frontend/issues/3727)（試行用リポジトリでの CI デプロイ確認。スキル側 #57・#58 に対応）
- 未取り込み: [#3739](https://github.com/Fandhe-AI/fandhe-frontend/issues/3739)（lang 別クローム文言・リポジトリリンク表示・`--help`）は main に未取り込み。#48 の FF_REV 更新時に取り込み済みか確認する
- 各 Issue の状態は `gh issue view <n> -R Fandhe-AI/fandhe-frontend` で再確認する。FF_REV 更新時は上流の `crates/docs-site/src/` の変更も見比べる

## 簡素化の設計判断（決定記録・2026-10-05）

上流の外部利用対応を受けて回避策を削除するにあたり、上流のキー表だけでは決まらない 5 点を決めた。後続イシュー #49〜#54 の前提である。
上流の仕様の正は Fandhe-AI/fandhe-frontend の `docs/design/docs-site-external-use.md`（§4 キー表・§5 フラグ・§6 帰属表記）と
`docs/guides/docs-site-external-repos.md` で、キー表はここへ書き写さない（二重管理を避ける）。

### 決定 1: 空の `tagline`

- **決定**: scaffold が `tagline` を必ず埋め、`check_site.py` が欠落を拒否する。「空文字で非表示」は提供しない
  - 新規構築で `--tagline` が空なら既定値を補う。lang が `ja` 系なら `<brand> のドキュメント`、それ以外は `<brand> documentation`
  - `check_site.py` は `[site]` に `tagline` が無い `nav.toml` をエラーにする（上流の既定文言の漏洩を防ぐ）
  - 同じ理由で必須とするキーは `brand`・`repository_url`・`tagline`・`copyright`・`version_badge`・`brand_mark` の 6 つ（未指定だと上流の識別情報が出るキー）。`lang`・`brand_color` の既定は無害なので任意
- **理由**: 必須化だけでは非対話の自動運転で入力が止まる。上流への依頼は上流の合意と rev 更新に依存して後続をブロックする。既定値の補完は `copyright` と同じ既存パターンで、利用者の追加入力が要らない
- **非採用**: 上流へ「空で非表示」を依頼する案。将来の任意対応とし、依存にしない
- **影響**: #50 の `tagline` 行を直す。#54 の移行では、旧 `brand.toml` の `tagline = ""` を既定値へ置き換えた案を出し、「行ごと削除していた旧挙動からの変更」と明記する

### 決定 2: `THIRD-PARTY-LICENSES` の生成元

- **決定**: `FF_REV` の固定 rev の raw URL（`https://raw.githubusercontent.com/Fandhe-AI/fandhe-frontend/<FF_REV>/LICENSE-MIT`）から取得し、本文を同梱しない（#47 の 2026-10-06 の決定）。ヘッダーの commit SHA だけ `FF_REV` から埋める
  - 取得 URL は固定で、可変部分は検証済み `FF_REV` のみ。https 限定・リダイレクト非追従・`--max-time 30`・`--max-filesize 65536`、HTTP 200 のみ許容する
  - 本文が上流の著作権行（`Copyright (c) <年> Fandhe-AI / fandhe-frontend contributors`）と許諾文の先頭を含まない場合、空・64KiB 超・NUL 入りの場合は、既存の `THIRD-PARTY-LICENSES` に触れず非 0 で停止する（一時ファイル → `mv` の原子的置換）
  - テストはネットワークに依存しない。スタブ `curl` で成功・失敗・検証不合格の各経路と、curl 引数の固定性を確かめる
- **理由**: インストールするバイナリと同じ rev の文面になる。ライセンス本文を `build-local.sh` に二重管理せず、FF_REV 更新時の同期作業が要らない
- **非採用**: `build-local.sh` への heredoc 同梱（FF_REV 更新のたびに本文のドリフトを手で追う必要がある）。`templates/` への別ファイル同梱（scaffold の所有ファイルが増え、得るものが無い）。`cargo install` の checkout を読む案（CARGO_HOME 配下の不安定なパス）
- **影響**: #49 の生成元を具体化する。`update-recovery.md` の復旧対象は変わらない

### 決定 3: 生成後の検査

- **決定**: `rebrand_site.py` の置換は削除するが、残存検査（`residual_hits`）は**廃止せず維持**する。帰属表記の検査を加えた「検査のみの verify」にする
  - 帰属表記の検査対象は、フッターの有無に依存しない条件で選ぶ。ビルド成果物（`_site/`）の全 HTML（最低 `index.html` と `404.html`）を対象とし、`docs-footer-bottom` を含むものだけに絞らない。上流でフッターが欠落したページは検査を通過させず、**フッターの欠落も帰属表記の欠落もエラー**にする
  - 各対象 HTML に `Built with fandhe-frontend docs-site` の帰属表記があること
  - `LICENSE-MIT` と `LICENSE-APACHE` へのリンクがそれぞれちょうど 1 本あること
  - 残存検査は現行方式を維持する。ヘッダー・フッターなど上流由来の表示領域に `fandhe-frontend` と `Fandhe-AI` が帰属表記の外に残っていればエラーにする。利用者の本文領域（Markdown 由来）は対象から除く（本文で `fandhe-frontend` を正当に書くサイトを許容する）。誤検出がある箇所だけ判定を調整し、検査自体は削らない
  - 検査を担う場所（`check_site.py` の `--verify-dist` か `rebrand_site.py` の検査部か）は #51 に委ねる。symlink を読まない・サイズ上限付きで読む・エラーに内容の断片を載せない方針を引き継ぐ
- **理由**: 帰属表記の保持は MIT / Apache-2.0 の通知義務に直結する。上流が新しい表示箇所（ヘッダー・フッター等）を足したときに、ブランドキーの事前必須化だけでは `fandhe-frontend` の残存を捕捉できない。残存検査の廃止は既存防御の弱体化になる。事前必須化（`brand`・`repository_url`・`copyright`・`version_badge` 等）は残存検査の代替ではなく、前段の防御として併用する
- **非採用**: 残存検査の全面廃止（上流の新表示箇所の取りこぼし）。帰属表記を `docs-footer-bottom` の有無で検査対象にする案（フッター欠落ページを素通りさせる）。検査をすべて上流に任せて消す案
- **影響**: #51。FF_REV 更新時の手動 grep（手順 5）も引き続き残す

### 決定 4: `title` の上流名拒否

- **決定**: 決定 3 で残存検査を維持するため、`check_site.py` の「`title` に `fandhe-frontend` を独立した語として含めない」規則（`has_upstream_word`）と、`scaffold.py` の入力検証（`validate_text`）の同種の拒否は**現状維持**とする。`is_upstream_repo`（`repository` が上流リポジトリ自身を指す拒否）も残す
- **理由**: この規則は残存検査との衝突（ビルド最終段の原因不明の失敗）を防ぐためで、`title` は `<title>` などの表示領域に出るため、残存検査を維持する限り衝突は残る。削除は、残存検査が `title` の出現箇所を誤検出しないよう調整できた場合に限り再評価する
- **影響**: #52 は `title` の規則を削除しない。`tests/test_rebrand.py` の関連テストも反転しない。#50 の scaffold の入力検証は `has_upstream_word` を使い続ける

### 決定 5: `brand.toml` から `[site]` への移行

- **決定**: 更新モードの scaffold は旧 `brand.toml` を読み、`[site]` ブロックの**案**を JSON と人間向け出力の両方に出す。`nav.toml` も `brand.toml` も書き換えない・削除しない
  1. 利用者がブロックを確認する
  2. 合意したら、SKILL.md の更新フローの手順で `nav.toml` の `[site]` へ反映する
  3. 旧 `brand.toml` と `_ff/`・`target/`・`Cargo.lock` は削除候補として案内する。自動削除はしない
  - 案は `check_site.py` の新 `[site]` 規則を通す。通らない旧値は、どのキーか（値は載せない）を示して停止する
  - 自動運転（非対話）では適用しない。案を出して「要対応」として報告する
  - 適用前にビルドしても、決定 1 の必須キー欠落で `check_site.py` が fail-closed になるので、上流の既定文言が黙って公開されることはない
- **理由**: 更新モードは利用者編集ファイルを書き換えない不変条件を守る。`nav.toml` はコメントを含む利用者編集物で自動編集は事故が起きやすい。一方、8 キーの検証規則と TOML エスケープは人手だと間違えやすく、決定的な案の生成が要る
- **非採用**: 完全に人手の移行。キーごとの規則が多く誤りやすい
- **影響**: #54 の方式を上記に確定する。#50 は `Brand`・`load_brand`・`BRAND_REQUIRED_KEYS` を参照が無くなった分だけ削除し、#54 の旧 `brand.toml` 読み取りは自己完結した関数（既存の TOML 部分集合パーサを使う）とする

### 決定を覆す条件

- 上流が `tagline` の「空で非表示」を入れた場合は、決定 1 を再評価する
- 上流が帰属表記の DOM に識別用の `data-*` 属性を足した場合は、決定 3 の verify を精緻化できる
- 残存検査が `title` の出現箇所を誤検出しないよう調整できた場合は、決定 4 の削除を再評価する
