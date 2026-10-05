<!-- source: skills/setup-github-pages/SKILL.md の更新フロー Step U1・U2（このスキル自身の手順。挙動の正は scripts/update-snapshot.sh と tests/test_rebrand.py の UpdateSnapshotTest・UpdateSnapshotHardeningTest） -->
<!-- 最終確認日: 2026-10-05 -->
<!-- 取得状況: ✅ 実際の git リポジトリで手順を実行して確認済み -->

# 更新の取り消し（Step U2 の復旧手順）

更新フロー（SKILL.md の Step U0〜U5）で、ローカルビルドが**決定的に**失敗したときに、更新を取り消す手順。

## いつ復旧するか

- 復旧するのは**決定的な失敗**だけ（`rebrand_site.py` の「一致数が 0」、`verify` の失敗など。上流のデザイン更新で HTML 構造が変わり、後処理の置換対象が合わなくなったことの検知）。対象リポジトリ側の後処理は書き換えず、スキル側の修正が必要であることを利用者に報告する
- ネットワーク断や cargo の一時障害は復旧せず、まず**再試行**する

## 仕組み: 書き込み直後のスナップショットとの比較

「U1 の後に利用者が手を入れたか」は、**書き込み直後の内容のハッシュを記録しておき、取り消しの時点で現在のハッシュと比べる**ことで判定する。生成予定との比較では判定できない（理由は末尾）。

| いつ | 何を | コマンド（`update-snapshot.sh`。`SKILL_DIR/scripts/` から起動し、カレントは対象リポジトリのルート） |
|------|------|------|
| scaffold の各実行の**直前**（再実行を含む）、ビルドの直前 | HEAD を記録し、「HEAD とも前回の記録とも違う」パスに TAINT の印を付ける | `guard "${SNAP}"` |
| scaffold の各実行の直後 | その実行の JSON から、スキルが書いたパスのハッシュを記録する | `record-json "${SNAP}" < "${RESULT}"` |
| ビルドの直後（**成否にかかわらず**） | `THIRD-PARTY-LICENSES` のハッシュを記録する | `record "${SNAP}" THIRD-PARTY-LICENSES` |
| 取り消しの前 | 予定を見る（何も変更しない） | `restore "${SNAP}" --dry-run` |
| 取り消し | 記録と一致するパスだけを戻す | `restore "${SNAP}"` |

- `${SNAP}` は `mktemp` で作る**作業ツリーの外**のファイル（リポジトリの中・symlink は拒否される）。値は控える（別シェルでは渡し直す）
- `THIRD-PARTY-LICENSES` は `build-local.sh --write-third-party` が**ビルドの前段**（`check_site`・wrapper のビルド・生成・rebrand より前）で書く。後段が決定的に失敗しても書き換わっているので、ビルドの成否にかかわらず直後に記録する
- **TAINT（基準を信頼できない印）**: 同じパスに 2 回目の記録が来ると、その間に利用者が触った内容が基準へ取り込まれ得る（例: exit 3 → 利用者が `pages.yml` の区間へ追記 → `--update` で再実行 → 追記込みの内容が基準になる）。そこで scaffold・ビルドの直前に `guard` を呼び、現在の内容が HEAD とも前回の記録とも違うパスに印を付ける。印の付いたパスは、以後ずっと自動では戻さない（`ASK`）。記録の直前に作業ツリーの内容が HEAD か前回の記録と一致していれば、利用者が触っていないと判定できる
- **許可リスト**: 記録・復元・削除してよいのは、所有ファイル・マニフェスト・`.gitignore`・`THIRD-PARTY-LICENSES` だけ。一覧は `scaffold.py --list-paths` が唯一の定義元（スクリプトはそれを呼ぶ）。利用者編集ファイル（`brand.toml`・`nav.toml`・`index.md`・`rust-toolchain.toml`）は `created`（`missing` の再作成）に含まれても記録しないので、自動では削除せず、利用者に確認する
- **ハッシュ（2 種類。取り違えない）**: 記録と「前回の記録との比較」は `git hash-object --no-filters`（生バイト）。選んだ理由: macOS と Linux で同じ結果になる（`sha256sum` と `shasum` の違いに依存しない）、`core.autocrlf`・属性のフィルタを通さないので設定で結果が変わらない、ファイルを書き込まない。一方、**「HEAD と同じか」の比較だけ**は、HEAD の blob が改行変換後の表現なので、作業ツリーも同じ表現にそろえる: filter 属性が指定されていない（`attr_filter` で確認）パスに限り、改行変換つきの `git hash-object`（`--no-filters` なし。内部の改行変換だけが適用される）を使う。生バイトのまま HEAD の blob と比べると、`core.autocrlf` や `.gitattributes` の `text` / `eol` が有効な環境（作業ツリーが CRLF、HEAD が LF）で、未編集の追跡ファイルまで「HEAD と違う」と誤判定して TAINT になる（実際の git で、`core.autocrlf=true` と `* text=auto eol=crlf` のどちらでも、未編集は一致・1 文字編集は不一致になることを確認済み）。filter 属性つきのパスでは、clean フィルタという外部コマンドを起動しないよう変換つきハッシュを呼ばず、TAINT / `ASK` にする。どちらのハッシュも `file_state` の中（祖先の検証の後）にだけ置く。symlink は辿らず（`-L` を先に判定）、通常ファイル以外はハッシュしない
- **パス**: ルートからの相対パスに限り、`..`・絶対パス・末尾の `/`・`.git`（大文字小文字を区別しない）配下・先頭の `-` などを拒否する。すべてクォートして `--` の後に渡す
- **ファイルに触れる入口は 1 つ（`file_state`）**: パスの中身・種別を見る処理（`-L`・`-e`・`-f`・`git hash-object`）は `file_state` の中だけに置き、その最初に祖先の検証（祖先ディレクトリが symlink・ファイルでない、実体がルートの下）を行う。不適なら、ファイルを開かず stat もせずに `ANCESTOR` を返し、`guard` では TAINT、`status` では `ancestor`、`restore` では `ASK` として扱う（記録後に親ディレクトリがリポジトリ外への symlink に差し替わっても、リンク先を読まない）。`rm`・`git restore` は、`file_state` が記録と一致したパスにだけ、`cmd_restore` の中で行う。この構造は `UpdateSnapshotEntrypointTest` がソースの検査で固定している（`hash-object` を呼ぶのは `file_state` の中だけ、など）
- **index の照合**: `git restore --staged --worktree` は index も HEAD に戻すため、記録後に利用者が別の内容をステージし、作業ツリーだけを記録時の内容に保つと、ステージ済みの変更が消える。そこで復元・削除の前に index も照合する。U0 で作業ツリーはクリーンで、scaffold はステージしないので、スキルが触っただけなら index は HEAD と同じはず、という前提に立ち、**そのパスの index が HEAD と違えば `ASK`**にする（HEAD に無いパスで index にエントリがある = ステージ済みの追加、HEAD にあるのに index に無い = ステージ済みの削除、blob またはモードが違う）。記録時の index を SNAP に残す方式にしなかったのは、基準を「HEAD」1 つに保ち、SNAP の形式と TAINT の仕組みを増やさないため。stage が 0 以外（マージ中）・skip-worktree・assume-unchanged（`git ls-files -v` の印）・git のエラー・想定外の出力は判定不能で `ASK`（fail-closed）。読むのは `git ls-files -s` / `-v` と `git ls-tree` だけ（外部コマンドを起動しない）。復元は作業ツリーだけ（`git restore --source=HEAD --worktree`）で、index は書き換えない。`guard` も、ステージ済み（または判定不能）のパスに TAINT を付ける
- **submodule**: 祖先に gitlink がある（`tools` が submodule など）と、その下のパスは親の HEAD から見えないだけで「無い」わけではない。無いと誤判定して削除しないよう、判定不能（`ASK`）にする
- **環境変数**: `GIT_DIR`・`GIT_WORK_TREE`・`GIT_INDEX_FILE`・`GIT_OBJECT_DIRECTORY`・`GIT_ALTERNATE_OBJECT_DIRECTORIES`・`GIT_COMMON_DIR`・`GIT_NAMESPACE` は起動時に unset する（別のリポジトリの HEAD・index を読む・書くことを防ぐ）
- **HEAD での存在判定**: `git ls-tree HEAD -- <path>` の終了コードと出力で判定する。終了コード 0 で出力が空 = HEAD に無いと確定（削除してよい）、出力あり = HEAD に通常ファイルとして在る（復元する）、終了コード非 0・通常ファイル以外 = 判定不能（`ASK`）。`git ls-files --error-unmatch` は使わない（致命的エラー・index から外された場合も「未追跡」と誤判定し、HEAD にあるファイルを削除し得る）
- **対象リポジトリの設定**: git は `-c core.fsmonitor=false -c core.hooksPath=/dev/null` を付けて実行する。次の 2 段で外部コマンドの実行を防ぐ。
  - **ローカル設定の検査（`ASK-ALL`）**: `restore`（`--dry-run` を含む）は、リポジトリのローカル設定に `filter.*.(smudge|clean|process)`・`core.fsmonitor`・`core.hooksPath`・`diff.*.(textconv|command)`・`include.path` / `includeIf.*.path` が 1 件でもあれば、何も戻さず `ASK-ALL` で止める
  - **パスごとの filter 属性の検査（`ASK`）**: 戻そうとするパスごとに `git check-attr filter -- <path>` を調べ、`unspecified` / `unset` 以外（フィルタ名・`set`）、判定不能（`check-attr` の失敗・想定外の出力形式）は、自動では戻さず `ASK` にする。filter の**定義**がグローバル・システム設定にあっても、対象リポジトリの `.gitattributes`（信頼しない）が `filter=<name>` を付ければ `git restore` は smudge / process フィルタを実行する（実際の git で確認済み）ため、定義のスコープにかかわらずパスごとに止める。`check-attr` は属性（`.gitattributes`・`info/attributes`・`core.attributesFile`）を読むだけで外部コマンドを起動しない。git-lfs などを使う利用者でも、filter 属性の付かないパスの復旧は止まらない
  - 復元経路で他に外部コマンドを実行し得るものは無い: `required` は filter の定義がある場合だけ効く、フックは `git restore` では呼ばれない（念のため `hooksPath` も無効化）、`core.alternateRefsCommand`・`credential`・`sshCommand` はネットワーク系で復元の経路に無い、`diff.external`・`textconv`・`merge.*` は `diff` / `merge` の経路。`record`・`guard`・`status` は `hash-object --no-filters` と `ls-tree` だけの読み取りなので止めない

## 保証する範囲

**U1（と記録した直後のビルド）が書いたままの内容のファイルだけを自動で戻す。** 記録後に 1 バイトでも変わったファイル、記録の前に利用者が触った（TAINT）ファイル、消えた・symlink に変わったファイル、HEAD での状態を判定できないファイルは、自動では戻さず、利用者に差分を示して個別に判断を仰ぐ。U0 以降に利用者が新しく作ったファイルや、スキルが触っていないファイルは、そもそも対象にしない（`git clean` や `git restore -- .` は使わない）。

## 復旧の手順

### 1. 予定を見て、了承を取り、実行する

```bash
bash "${SKILL_DIR}/scripts/update-snapshot.sh" status "${SNAP}"            # 各パスの状態（match / changed / missing / symlink / special / taint）
bash "${SKILL_DIR}/scripts/update-snapshot.sh" restore "${SNAP}" --dry-run  # 何も変更せず、予定を出す
git status --short                                                          # 利用者へ示す
# 利用者の了承を得てから:
bash "${SKILL_DIR}/scripts/update-snapshot.sh" restore "${SNAP}"            # 一致するものだけを戻す
```

出力の各行の意味と、終了コード:

| 出力 | 意味 | 扱い |
|------|------|------|
| `WOULD-RESTORE` / `WOULD-DELETE <path>`（`--dry-run`） | 戻す・消す予定 | 利用者に示す |
| `RESTORED <path>` | 記録と一致し、index が HEAD と同じで、HEAD に通常ファイルとして在ったので、作業ツリーを HEAD の内容へ復元した（index は変更しない） | 済み |
| `SKIP <path> …` | 記録とは違うが、**すでに戻っている**（復旧の 2 回目など）: 現在の内容が HEAD と同じ（HEAD と同じ表現＝改行変換後で比べる。filter 属性つきは比べない）で index も HEAD と同じ、または HEAD に無いパスで、ファイルも index のエントリも無い | 何もしない（`ASK` にしない） |
| `DELETED <path>` | 記録と一致し、HEAD に無いと確定できたので、削除した | 済み |
| `ASK <path> …` | 記録と一致しない・TAINT・消えた・symlink に変わった・祖先が symlink・index が HEAD と違う（ステージ済み）または index の状態を判定できない・HEAD での状態を判定できない・filter 属性が指定されている（または判定できない）・復元や削除に失敗した。**何もしていない** | 利用者に差分を示して個別に判断を仰ぐ（戻す・残す・手動で統合）。差分は下の `git diff`（外部 diff・textconv を使わず、行数と制御文字を絞る）や `scaffold.py --show-diff` で見せる。symlink・特殊ファイルは内容を読まない |
| `ASK-ALL …`（stderr） | `${SNAP}` が無い・空・読めない、HEAD が動いた（更新用ブランチでコミットした等）、HEAD を解決できない、ローカル設定に外部コマンドを実行し得るキーがある。**何も戻していない** | すべて利用者に確認する |

差分の見せ方（`ASK` のパスごと）:

```bash
git diff --no-ext-diff --no-textconv --no-color HEAD -- <path> | head -200 | cat -v
```

終了コード: 0 = 完了（`ASK` なし）、2 = 使い方・入力の不正、3 = `ASK-ALL`（何も戻していない）、4 = `ASK` が 1 件以上。

### 2. 元のブランチへ戻る

**`ASK` が 1 件でも残っている間は `git switch`・`git branch -D` へ進まない**（終了コード 4）。`ASK` で残したファイルの扱いが決まってから戻る。更新用ブランチにコミットが無いこと（`git log --oneline "${BASE}..${NAME}"` が空）を確かめてから削除する。コミットがあれば削除せず、利用者に確認する。

```bash
git -c core.fsmonitor=false -c core.hooksPath=/dev/null switch "${START_BRANCH}" && git branch -D "${NAME}"     # U0 で控えた値
```

復旧後、利用者が U0 以降に手で行った変更（exit 3 の統合、exit 4 の `brand.toml` 追記、新規ファイル）と、`ASK` で残したファイルは、作業ツリーに残り、元のブランチへ持ち越される。その旨を利用者に伝える。

## なぜ `scaffold.py --show-diff` の `same` を判定に使わないか

`same` は「**いま scaffold が生成する内容と一致するか**」であって、「U1 の直後から変わっていないか」ではない。`pages.yml` は現在の利用者区間の内容を取り込んで生成予定を組み立てるため、U1 の後に利用者が区間へ監視パスを足しても `same` のままになり、`same` を根拠に `git restore` するとその編集が消える（`UpdateSnapshotTest` が、この状況で `same` が編集を検知できないこと・スナップショットとの比較なら検知できることを固定している）。`--show-diff` は、`ASK` になったパスの差分を利用者に見せる用途にだけ使う。
