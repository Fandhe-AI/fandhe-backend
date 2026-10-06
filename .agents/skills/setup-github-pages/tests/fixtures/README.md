# fixtures

`redirect/` は FF_REV の docs-site-gen が出力した redirect ページ（旧 `raw/old-usage/index.html` を
バイト不変で移したもの。手書きではない）。`VerifyAttributionTest` が「帰属表記を持たない redirect ページを
許容する」ことの確認に使う。FF_REV を更新したら、上流の redirect 出力が変わっていないか確認して再生成する。

最終再生成: FF_REV `b3e31ef663a98b6080feb98c84ade238d1074a08`。

## site-keys/

`site-keys/` は FF_REV の docs-site に `nav.toml` の `[site]`（10 キー）を指定して生成した**実出力**
（`index.html`・`404.html`。手書き・加工なし）。`VerifyAttributionTest` が `verify_attribution` の入力に使う。
上流の DOM が変わって帰属表記の並びが崩れたら、実ビルドの verify とこのテストが止まる。

再生成: 空の一時ディレクトリへ `python3 -I -B scripts/scaffold.py --target <dir> --owner acme --repo mini-repo --branch main --title Mini --tagline "Tiny site" --year 2026`
を実行し、`<dir>` で（使い捨ての `CARGO_HOME` を付けて）`bash tools/docs-site-gen/build-local.sh --clean --write-third-party` を実行する。
後処理は無い。`_site/index.html` と `_site/404.html` をそのままコピーする。最終再生成: FF_REV `b3e31ef663a98b6080feb98c84ade238d1074a08`。

`redirect/` と `site-keys/` は生成器の出力をバイト一致で保持するため、`.editorconfig` で final newline 等の検査を免除している
（新しい fixture ディレクトリを足したら同じ免除を `.editorconfig` に追加する）。
