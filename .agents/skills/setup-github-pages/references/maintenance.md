<!-- source: skills/setup-github-pages（このスキル自身の保守手順。上流は https://github.com/Fandhe-AI/fandhe-frontend） -->
<!-- 最終確認日: 2026-10-05 -->
<!-- 取得状況: ✅ 実装とテストで確認済み（FF_REV cf5edb9b8f1bf2a63d51dcf50a8d2806e2d8f9f9 で匿名ビルド・生成・rebrand を実測） -->

# スキル保守者向けの手順（FF_REV の更新・上流改修の追跡）

この文書は**スキル自身（このリポジトリの `templates/`）を保守する人**向け。構築済みの対象リポジトリへ反映する
利用者の手順は SKILL.md の「更新フロー」（Step U0〜U5）で、保守者がスキルを更新して配布したあと、利用者が再実行して取り込む。
両者を混同しない。

## FF_REV の更新手順

現在の固定値: `cf5edb9b8f1bf2a63d51dcf50a8d2806e2d8f9f9`（2026-10-04 時点で匿名ビルド・生成・rebrand を実測済み）。

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
   を再実行する）で `bash tools/docs-site-gen/build-local.sh --clean --write-third-party` を実行する。「registry 依存が 0 件であることを
   検査」の工程（`cargo metadata` で `source` が null 以外のパッケージを数える）が失敗したら、上流が外部 crate を導入した合図なので、
   匿名・隔離ビルドの前提と供給網の固定方針を見直すまで更新しない。wrapper のコンパイルエラーは上流 API
   （`build_site_with` / `EMPTY_REGISTRY`）の変更を示す
4. `rebrand_site.py` が「一致数が 0」で失敗したら、上流 DOM の変更を意味する。生成された HTML を読み、ヘッダー・フッターの
   該当要素を特定して `rebrand_site.py` のルールを直す。`brand.toml` に無い新しいハードコード表示が増えていないかも、
   `grep -o fandhe-frontend` と `Fandhe-AI` で確認する
5. 帰属表記の件数を再確認する。`grep -o fandhe-frontend _site/index.html | wc -l` が 3（`Built with …` と LICENSE リンク 2 件）の
   ままであること。増減していれば上流がフッターを変えている
6. `references/site-format.md` の制約（nav.toml の書式・予約パス・予約アセット・Markdown 対応範囲）が変わっていないか上流
   ソースで再確認し、`scripts/check_site.py` の `RESERVED_ASSET_NAMES`（上流 `build.rs` の `RESERVED_ASSET_NAMES`）を更新する
7. `tests/fixtures/raw/` を新 rev の生成物で作り直し、`node --test "tests/*.test.mjs"` を通す

## 上流改修の追跡

本スキルの wrapper と後処理は、上流 fandhe-frontend の次の制約を回避するための**暫定措置**である。上流が対応したら
該当部分を削除する。

| 回避している制約 | 暫定措置 | 上流が対応したら |
|------------------|----------|------------------|
| stock の `docs-site` が page section registry（8 パス必須・固有リンク注入）を強制する | wrapper が `build_site_with(…, &EMPTY_REGISTRY)` を呼ぶ | registry を無効化するフラグ（または外部サイト向けモード）が入ったら wrapper を廃止し stock バイナリを使う。トップのヒーロー / カードグリッドも使える可能性がある |
| ブランド表示がハードコード（`[site]` で変えられるのは `title`（フッターのみ）と `base_path`） | `rebrand_site.py` + `brand.toml` | `[site]` にブランド名・リポジトリ URL・タグライン・著作権・lang・favicon 等のキーが追加されたら、`rebrand_site.py` と `brand.toml` を削除し `nav.toml` へ移す |
| `cargo install --git` が submodule（`docs/spec` → private リポジトリ）で失敗する | 手動 shallow fetch + path 依存 | submodule の分離、または docs-site が crates.io などで配布されたらそちらへ切り替える |

追跡 Issue: [https://github.com/Fandhe-AI/fandhe-frontend/issues/3713](https://github.com/Fandhe-AI/fandhe-frontend/issues/3713)（トラッキング）

| Issue | 対象 |
|-------|------|
| [#3716](https://github.com/Fandhe-AI/fandhe-frontend/issues/3716) | registry 無効化 |
| [#3717](https://github.com/Fandhe-AI/fandhe-frontend/issues/3717) | ショーケース注入 |
| [#3718](https://github.com/Fandhe-AI/fandhe-frontend/issues/3718) | submodule |
| [#3720](https://github.com/Fandhe-AI/fandhe-frontend/issues/3720) / [#3721](https://github.com/Fandhe-AI/fandhe-frontend/issues/3721) / [#3722](https://github.com/Fandhe-AI/fandhe-frontend/issues/3722) | ブランドキー |
| [#3727](https://github.com/Fandhe-AI/fandhe-frontend/issues/3727) | 試行用リポジトリでの CI デプロイ確認 |

対応状況の確認は、上記 Issue の状態と、上流の `crates/docs-site/src/{layout.rs,site_footer.rs,favicon.rs,nav.rs,page_sections.rs}` の
変更を `FF_REV` 更新時に見比べて行う。
