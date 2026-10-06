<!-- source: skills/setup-github-pages/scripts/scaffold.py（このスキル自身の実装。挙動の正は tests/test_rebrand.py） -->
<!-- 最終確認日: 2026-10-05 -->
<!-- 取得状況: ✅ 実装とテストで確認済み -->

# scaffold.py リファレンス（分類・競合・利用者区間・マニフェスト・出力）

SKILL.md の「scaffold.py の概要と終了コード」の詳細。`scaffold.py` は、モード判定（`--detect`）・配置と更新・
競合の差分表示（`--show-diff`）を担う。シェルの `sed` で自前置換しない（入力値の `/` `&` `"` で置換式や
TOML が壊れる・注入される）ため、値は `scaffold.py` が検証・エスケープして書き込む。

## オプション

| オプション | 用途 |
|-----------|------|
| `--target <dir>` | 対象リポジトリのルート（必須） |
| `--detect` | モードと根拠を表示して終了（書き込みなし）。`--json` では `mode`・`kind`・`reasons` |
| `--json` | 結果を JSON 1 つで標準出力へ出す。**どの終了コードでも出る**（exit 2 では `error`） |
| `--show-diff` | 競合した所有ファイルとスキルの新版の差分を表示して終了（書き込みなし） |
| `--update` | 競合した所有ファイルを強制的に上書きする（利用者の了承後のみ。効かない種別は後述） |
| `--owner` `--repo` `--branch` `--title` | 新規構築で必須。更新では省略できる（`--branch` は更新でも Step 1 の値を渡す） |
| `--tagline` | 新規構築で必須（`[site].tagline`。空にできない・既定値を補わない） |
| `--brand` `--copyright` `--lang` `--version-badge` `--favicon-letter` `--favicon-color` `--year` | 新規構築の任意入力（既定あり。`nav.toml` の `[site]` へ書かれる） |

## 分類

全配置先を先に分類してから書く（途中失敗による部分書き込みなし）。各ファイルの書き込みも原子的で、同じディレクトリに一時ファイル（`.<名前>.<乱数>.sgp-tmp`。拡張子で終わらないため cargo・GitHub Actions・python は拾わない）へ全バイトを書き、fsync してから `os.replace` で置き換える。途中で失敗（空き容量不足・I/O エラー・シグナル）した場合、既存のファイルは元の内容のまま、新規のファイルは作られず、一時ファイルは消される（`.gitignore` の追記・マニフェストも同じ）。権限は、既存ファイルは引き継ぎ、新規は umask に従い、`build-local.sh` は実行ビットを足す。完了したファイルは生成予定と完全に一致するため、再実行では一致（`same`）、未着手の更新は自動更新、未作成は新規作成になり、フラグを足さずに収束する。プロセスが強制終了（SIGKILL・電源断）されると一時ファイルが残り得る。内容を確認して削除してよい（`tools/docs-site-gen/` に残ったまま新規構築を再実行すると、スキルが配置しないファイルとして検出される）

| 種別 | 対象 | 状態 | 扱い |
|------|------|------|------|
| スキル所有 | `tools/docs-site-gen/{FF_REV,build-local.sh,check_site.py,_common.py}`、`.github/workflows/pages.yml` | 存在しない | 作成 |
| | | 生成予定と一致 | 変更なし（冪等） |
| | | 不一致で、ハッシュがマニフェストと一致（配置後に未編集） | **自動で更新**（`--update` 不要） |
| | | 不一致で、編集あり／マニフェストなし | **競合**。何も書かず exit 3 |
| 利用者編集 | `site/{nav.toml,index.md}`、`rust-toolchain.toml` | 既存 | **保持**。書き換えない |
| | | 欠落（更新モード） | **再作成しない**。`欠落` として報告（必要かどうかは配置後の検証が判定する）。再作成は 4 つの引数が揃っているときだけ |

### `kind=unrelated`（`mode=foreign`）の通常実行

スキルの配置とは認められない既存ファイルがあるときの挙動。`--update` なしでは、次のものが競合（exit 3）になり、何も書かない。

- 生成予定と**内容が異なる**同名の所有ファイル（`no_manifest`。旧版形式の `pages.yml` を含む）
- `tools/docs-site-gen/` にあるスキルが配置しないファイル（`foreign_dir`）
- `tools/docs-site-gen` または `src` が対象内を指す symlink（`symlink`。`--update` でも進まない）

同名で**内容が生成予定と一致する**所有ファイルは「配置済み」として扱い、他に競合がなければ欠けたファイルを補う
（配置が途中で止まった新規構築の再実行を壊さないため。バイト単位で生成予定と同一なので、置き換えるものがない）。

## 競合の種別（JSON の `conflicts[].kind`）

どの競合も exit 3 のまま（実行されるスクリプトがスキルの版と一致する、という保証を崩さないため）。

| `kind` | 意味 | `--update` |
|--------|------|-----------|
| `user_edited_skill_unchanged` | 利用者が編集した。スキル側は配置時から変更なし | 効く |
| `user_edited_skill_changed` | 利用者が編集し、スキル側も変更した | 効く |
| `no_manifest` | マニフェストが無い（旧版からの移行、または別用途の同名ファイル）。所有ファイルを編集していなければ、利用者の了承を得てから `--update` を 1 回実行すればよい | 効く |
| `not_recorded` | マニフェストに記録が無い（スキルの新版で追加された所有ファイルと同名の別内容） | 効く |
| `foreign_dir` | 構築前の `tools/docs-site-gen/` に、スキルが配置しないファイルがある（別用途のディレクトリの可能性。`path` は `tools/docs-site-gen`）。`--show-diff` の `status` は `directory`（内容は読まない） | 効く（既存のファイルは残したまま配置する。用途を確認し、利用者の了承を得てから） |
| `pages_region_invalid` | `pages.yml` の利用者区間が不正 | 効く（区間の内容は捨てられる） |
| `symlink` / `not_regular` / `unreadable` | シンボリックリンク・通常ファイルでない・読めない（大きすぎる等）。`tools/docs-site-gen` または `tools/docs-site-gen/src` が対象内を指す symlink もここ（`path` はそのディレクトリ。中に別用途のファイルがあるか確認できないため、`unrelated` として検出し、モードに関わらず競合にする） | **効かない**。手動で解消する |
| `outside_root` | 配置先の親ディレクトリが symlink で、実体が対象の外（または `.git` 配下）へ解決される。**存在も内容も読まない**（外側の状態に左右されない。所有ファイルも利用者編集ファイルも対象） | **効かない**。親ディレクトリを通常のディレクトリへ直す。root 内を指すディレクトリ symlink は、生成器ディレクトリ（`tools/docs-site-gen`・`src`）を除いて許可する（上の `symlink` の行） |

## pages.yml の利用者区間

`docs/` や `README.md` を公開する構成では、`pages.yml` の `on.push.paths` に監視パスを足す必要がある。`pages.yml` は
所有ファイルなので、追加は **`sgp:user-paths:begin` と `sgp:user-paths:end` の間の利用者区間だけ**に
`      - "docs/**"` の形で 1 行ずつ書く。

- 区間の外を編集すると競合になる。区間の中身は更新しても保持される
- 「未編集」判定とマニフェストのハッシュは、区間を空にし、マーカー行を説明文のない固定の行にした正規形で取る
  （区間だけの編集、マーカー行の説明文だけの違いは未編集として自動更新される。書き込み時はテンプレートの行へ正規化する）
- `pages.yml` の解析・比較は、改行を LF に正規化してから行う（`core.autocrlf` で CRLF になっても、区間・追加 paths・未編集判定が LF と同じ結果になり、CRLF へ変換されただけなら一致として書き換えない）。行の途中の単独の `\r` は正規化せず不正のまま。他の所有ファイルは byte 一致で比較する（改行だけの差は `newline_only` の競合）。書き込みは常に LF
- マーカーはトークンで判定する（行頭の空白 6 つ + `# sgp:user-paths:begin|end`、後ろは行末か区切りに続く説明文）。
  偽のマーカー行・重複・逆順・ネスト・欠落は `pages_region_invalid`
- 区間の行は信頼しない入力として厳格に検証する: `      - "<glob>"` の形・glob は英数字と `. _ * / -` のみ・
  `..` や絶対パスは不可・20 件まで。違反は競合になり、`${{`・引用符・改行は workflow へ持ち込まれない
- **旧版（マーカーなし）**: 追加の `paths` のうち既定以外の行は、検証を通ったものだけが利用者区間へ移る
  （通らなかった分・上限 20 件を超えた分は、件数と理由を `warnings` に出す）。この移行は、競合を解消する
  `--update` の実行で行われる。例外として、旧版配置と認められる（`kind=legacy`。マニフェストなし・所有ファイルの痕跡が揃う）場合に限り、
  `pages.yml` の本文が追加 paths を除いてテンプレートと一致するなら `--update` なしでも移行される（マニフェストも再作成される）。
  `kind=unrelated` では旧版形式の `pages.yml` も `no_manifest` の競合で、`--update` なしでは書き換えない。
  形式が合わず区間へ移せない行（シングルクォート・引用符なし）がある競合は、理由文に「`--update` で引き継がれない行がある」と出る。
  本文のそれ以外の差は競合のまま

## マニフェスト（`tools/docs-site-gen/.scaffold-manifest.json`）

所有ファイルの sha256・配置時の `FF_REV`・書式バージョン（1）を記録する。**コミットする**（次回の自動更新の判定に使う）。

- 信頼しない入力として扱う。JSON でない・未知のキーや版・64 KiB 超・101 件以上・`..` や絶対パスを含む・許可ディレクトリ
  （`tools/docs-site-gen/`、`.github/workflows/`）外・ハッシュ不正のいずれか 1 つでも該当すれば、丸ごと無視して
  「マニフェストなし」と同じに倒す（自動更新はせず、不一致は競合になる安全側）
- 対象リポジトリのファイルを開く入口は 1 か所（`_open_regular`）に集約し、親ディレクトリの symlink を含めた実体が root の外（`.git` 配下を含む）へ解決されるパスは、開く前に拒否する（`--detect`・通常実行・`--update`・`--show-diff` のいずれでも外側のファイルを読まない）
- 読む前に実体が対象リポジトリ内（`.git` 配下を除く）の通常ファイルであることを確かめる。symlink のマニフェストは
  無視し、書き込みは exit 2 で拒否する
- マニフェスト内のパスは、スキル側の固定パスとの突き合わせにのみ使い、書き込み・削除・表示の対象にしない。
  未知のエントリは警告して無視し、新しいマニフェストへ引き継がない
- スキルで廃止された所有ファイルは、スキル側の固定リスト（`DEPRECATED_OWNED`）だけで判定し、`削除候補` として
  **表示のみ**。自動削除しない（削除は取り消せず、確認は利用者が行う方が安全なため）

## 出力と JSON

人間向け出力は `モード` / `FF_REV: 旧 → 新` / `作成` / `更新`（理由付き）/ `一致（変更なし）` / `保持（利用者編集）` /
`欠落` / `追記した .gitignore 行` / `マニフェスト`（無くて再作成した場合はその旨）/ `削除候補`（あれば）で、最後に
配置後の検証（`check_site.py` 相当）を実行する。保持した利用者ファイルの内容（nav.toml の `[site]` の値等）も
ここで検査され、`nav.toml` の `[site]` に必須キーが不足していれば、不足キーと追記例を案内して exit 4 になる
（利用者ファイルは書き換えない）。対象リポジトリ由来の文字列（パス・検証エラーの断片等）は、制御文字・bidi 文字・
不可視文字（ゼロ幅・Unicode タグ文字等）を `\uXXXX` へ無害化し、長さを制限して出力する。

`--json` の主なキー:

| キー | 内容 |
|------|------|
| `mode` / `kind` / `detect` | モード、判定の種別（`none` / `manifest` / `legacy` / `upstream` / `unrelated`）、根拠 |
| `ff_rev` | `old` / `new` / `changed` |
| `created` / `updated` / `same` / `kept` / `missing` | 作成・更新（`path` と `reason`）・一致・保持・欠落 |
| `conflicts` | `path` / `kind` / `reason` |
| `deprecated` | 削除候補（`path` と `edited`。`brand.toml` のみ `note` を持つ）。`--detect --json` も、`mode=update` のときに同じキーで返す |
| `site_migration` | 更新モードで旧 `tools/docs-site-gen/brand.toml` があるときの `[site]` 移行案（無ければ `null`）。`status` は `proposal`（案あり）/ `needs_input`（旧 tagline が空。案の tagline 行は `__SGP_TAGLINE__` の目印で、貼っても check_site が止める）/ `invalid`（旧値が検証を通らない。`problems` にキー名と理由だけ。値は載せない）/ `unreadable`（symlink・64 KiB 超・構文違反等で読めない）/ `migrated`（`nav.toml` が既に必須キーを満たす）。`entries`（`key`・`from`・`state` = add / same / differs / needs_input / invalid・`value`）・`needs_input`・`block`（追加すべきキーだけの文面）・`applied`（**常に false**。scaffold は反映しない） |
| `legacy_artifacts` | 旧構成の生成物（`_ff`・`tools/docs-site-gen/Cargo.lock`・`tools/docs-site-gen/target`）の案内（`path`・`note`・`symlink`）。スキル所有ではないので `deprecated` には含めず、自動では削除しない。`target` は新構成でも `target/docs-site-install` を使うため丸ごとは消さない。直下が `docs-site-install` だけなら案内しない |
| `gitignore_added` | 追記した `.gitignore` の行 |
| `manifest_written` / `manifest_recreated` | マニフェストを書いたか／無くて再作成したか（旧版からの移行） |
| `warnings` | 警告（マニフェストの無視・`--branch` の食い違い・想定外ファイル・引き継がなかった paths など） |
| `check` | 配置後の検証（`ok` / `errors` / `warnings`） |
| `same` / `kept` / `missing` | `--show-diff` でも出る（`same` は「いま生成する内容と一致する」所有ファイル。「適用直後から変わっていない」ことの判定には使えない。更新の取り消しは [`update-recovery.md`](update-recovery.md)） |
| `diffs` | `--show-diff` のとき、競合したパスをキーにした辞書（`diffs.<path>` に `status`・`reason`・`conflict_kind`・`lines`） |
| `error` / `exit_code` | エラーメッセージ（exit 2・4）／終了コード |

## --show-diff の安全性

内容を読むのは、対象リポジトリ内に解決される通常ファイルだけ（256 KiB・200 行まで。`O_NOFOLLOW`）。symlink・特殊ファイル・
対象の外・`.git` 配下・巨大・UTF-8 でないものは内容を読まず、理由だけ出す（`diffs.<path>.status`: `symlink` / `outside` /
`not_regular` / `too_large` / `not_utf8`）。改行コードだけの差は `newline_only`。**競合の差分を `diff -u` などの外部
コマンドで確認しない**（symlink を辿ってリンク先、例えば認証情報ファイルを端末とエージェントの文脈に出してしまう）。

## 終了コード

| 終了コード | 意味 |
|-----------|------|
| 0 | 成功（配置後の check_site も通過） |
| 2 | 入力不正、書き込み先が不適（symlink・親が `--target` の外へ解決・`.git` 配下（大文字小文字違いを含む）・配置先の親パスの途中が通常ファイル・`.gitignore` が通常ファイルでない／UTF-8 でない等）、または適用対象外（上流リポジトリ自身）。書き込み前の検査で止まった場合は 1 件も書かれていない。書き込みの途中の OS エラー（権限・空き容量等）も exit 2 で、失敗したファイルは未変更か未作成のまま、それ以前に書けた分は `created` / `updated` に残る（再実行は冪等） |
| 3 | 競合（スキル所有ファイルが生成予定と違い、配置後に編集されている、またはマニフェストがない） |
| 4 | 配置・更新は完了したが check_site が失敗（`nav.toml` の `[site]` の不足キー・不正な値、予約アセット名、`nav.toml` の欠落など、利用者編集ファイルの不備） |
