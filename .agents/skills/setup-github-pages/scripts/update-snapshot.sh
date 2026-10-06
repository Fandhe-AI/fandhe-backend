#!/usr/bin/env bash
# update-snapshot.sh: 更新フロー（SKILL.md の Step U1〜U2）で、スキルが書いた直後のファイルの内容を記録し、
# 更新を取り消すとき「書いたまま変わっていないファイルだけ」を戻す。
#
# 対象リポジトリへは配置しない（スキル側の道具。SKILL_DIR から起動する）。カレントディレクトリは対象リポジトリのルート。
#
# # なぜ「記録との比較」なのか
# scaffold.py --show-diff の same は「いま scaffold が生成する内容と一致するか」であり、「U1 の直後から変わっていないか」
# ではない（pages.yml は現在の利用者区間を取り込んで生成予定を組み立てるため、U1 の後に利用者が区間へ足した監視パスも
# same のまま）。取り消しで利用者の編集を消さないため、書き込み直後の内容のハッシュをリポジトリの外のスナップショット
# ファイルへ記録し、取り消しの時点で現在のハッシュと比べる。
#
# # 基準を信頼できなくなる経路への対処（TAINT）
# 同じパスに 2 回目の記録が来ると、その間に利用者が触った内容が基準へ取り込まれ得る（例: exit 3 → 利用者が編集 →
# --update で再実行 → 追記込みの内容が基準になる）。そこで scaffold・ビルドを実行する**直前**に `guard` を呼び、
# 対象パスの現在の内容が「HEAD」とも「前回の記録」とも違えば、そのパスに TAINT の印を付ける。印の付いたパスは、
# 以後ずっと自動では戻さない（ASK）。
#
# # 触ってよい対象（許可リスト）
# 所有ファイル（scaffold.py が唯一の定義元。`scaffold.py --list-paths`）・マニフェスト・.gitignore・
# THIRD-PARTY-LICENSES に限る。利用者編集ファイル（nav.toml 等）と、廃止された旧構成のファイル（旧 brand.toml・
# Cargo.toml・src/main.rs・_ff/・Cargo.lock・target/）は許可リストの外で、記録も復元も削除もしない（scaffold は廃止ファイルを
# 書かない・消さないため、取り消しの対象にならない）。
#
# # ハッシュと git
# 記録と「前回の記録との比較」のハッシュは `git hash-object --no-filters`（macOS・Linux で同じ。autocrlf・属性のフィルタを
# 通さないので設定で結果が変わらない生バイト）。「HEAD と同じか」の比較だけ、HEAD の blob（改行変換後の表現）と
# そろえるため、filter 属性の無いパスに限り改行変換つきの `git hash-object`（既定）を使う。symlink は辿らず、通常ファイル以外はハッシュしない。git は `-c core.fsmonitor=false
# -c core.hooksPath=/dev/null` を付けて実行する（対象リポジトリの設定で外部コマンドが走らないように）。さらに
# `restore` は、リポジトリのローカル設定に filter（smudge/clean/process）・core.fsmonitor・core.hooksPath・
# diff の textconv/command・include があれば、何も戻さず ASK-ALL で止める（git restore が smudge フィルタを実行するため）。
# ローカル設定だけでは足りない: filter の定義がグローバル・システム設定にあっても、対象リポジトリの .gitattributes
# （信頼しない）が `filter=<name>` を付ければ git restore は外部コマンドを実行する。そこで**戻そうとするパスごと**に
# `git check-attr filter -- <path>` を調べ、filter 属性が指定されているパスは、定義がどのスコープにあるかにかかわらず
# 自動では戻さず ASK にする（git-lfs などを使う利用者の、filter の付かないパスの復旧は止めない）。check-attr は
# 属性を読むだけで外部コマンドを起動しない（core.attributesFile・info/attributes・.gitattributes をすべて見る）。
# 失敗・出力の形式が想定と違うときも ASK（fail-closed）。git restore が復元経路で実行し得る外部コマンドは filter
# （smudge / process。required は filter の定義がある場合だけ効く）と core.fsmonitor だけで、フックは restore では
# 呼ばれず（念のため hooksPath も無効化）、core.alternateRefsCommand・credential・sshCommand 等はネットワーク系で
# restore の経路に無い。
# 「HEAD に在るか」は `git ls-tree HEAD -- <path>`（終了コード 0 で出力が空 = HEAD に無いと確定、出力あり = 在る、
# 非 0 = 判定不能）で判定し、判定不能は触らない。パスの祖先に symlink があれば触らない。
#
# # ファイルに触れる入口の集約（file_state）
# パスの中身や種別を見る処理（`-L`・`-e`・`-f`・`git hash-object`）は `file_state` の中だけに置き、その最初に
# `ancestors_ok`（祖先ディレクトリが symlink・ファイルでない、実体がルートの下）を検証する。不適なら stat も open も
# せず ANCESTOR を返す（記録後に親ディレクトリがリポジトリ外への symlink に差し替わっても、リンク先を読まない）。
# `rm`・`git restore` は、`file_state` が記録と一致したパスにだけ行う（cmd_restore の中で file_state の後）。
# scaffold.py の `_open_regular` と同じ考え方。
#
# # index の照合
# `git restore --staged --worktree` は index も HEAD に戻す。記録後に利用者が別の内容をステージしていれば、作業ツリーが
# 記録時のままでもステージ済みの変更が消える。U0 で作業ツリーはクリーンで、scaffold はステージしないので、スキルが
# 触っただけなら index は HEAD と同じはず（方針: 記録時の index を SNAP に残すのではなく、HEAD との一致で判定する。
# 理由は、基準が「HEAD」1 つで済み、SNAP の形式・TAINT の仕組みを増やさないため）。index のそのパスが HEAD と違う
# （HEAD に無いパスなら index にエントリがある、HEAD にあるパスなら index に無い = ステージ済みの削除）場合は ASK。
# stage が 0 以外・skip-worktree・assume-unchanged・git のエラー・想定外の出力は判定不能で ASK（fail-closed）。
# 読むのは `git ls-files -s` / `-v` と `git ls-tree` だけ（外部コマンドを起動しない）。復元は作業ツリーだけ
# （`git restore --source=HEAD --worktree`）で、index は書き換えない。
#
# 使い方（SNAP は mktemp 等でリポジトリの外に作ったファイル）:
#   update-snapshot.sh guard       SNAP             scaffold・ビルドの直前に呼ぶ。HEAD を記録し、触られたパスに TAINT を付ける
#   update-snapshot.sh record      SNAP <path>...   指定パスの現在の内容を記録する（許可リスト内のみ）
#   update-snapshot.sh record-json SNAP < JSON      scaffold.py --json の出力から、スキルが書いたパスを記録する
#   update-snapshot.sh status      SNAP             記録したパスの現在の状態（match / changed / missing / symlink / special / taint）
#   update-snapshot.sh restore     SNAP [--dry-run] 記録と一致するパスだけを戻す（HEAD に在れば復元、無ければ削除）。
#                                                   --dry-run は何も変更せず、予定（WOULD-RESTORE / WOULD-DELETE / ASK）を出す
# 出力: RESTORED / DELETED / ASK <path> <理由> / ASK-ALL <理由>（stderr）
# 終了コード: 0 正常 / 2 使い方・入力の不正 / 3 ASK-ALL（何も戻していない） / 4 ASK が 1 件以上（ASK が残る間は
#            git switch・git branch -D へ進まない）

set -u -o pipefail

# リポジトリの場所・index・オブジェクトを環境変数で差し替えられないようにする（別のリポジトリの HEAD・index を
# 読む・書くことを防ぐ。決定的な動作のため）
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR GIT_NAMESPACE

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT_REAL=""
ALLOWED=""

die() { echo "エラー: $*" >&2; exit 2; }

# 出力に出す文字列の無害化。対象リポジトリ由来の文字列（.gitattributes の値・設定のキー名・git の出力など）は
# 制御文字・ESC・bidi・不可視文字を含められるため、安全な文字種に一致するときだけ表示し、それ以外は値を出さない。
# このスクリプトの出力は、検証済みのパス・固定の文言・この関数を通した値だけにする。
SAFE_RE='^[A-Za-z0-9_./:@+-]{1,200}$'   # `${2:-…}` の中に `{1,200}` を書くと最初の `}` で展開が終わるため、変数に出す
safe() {   # safe <値> [正規表現]
  local re="${2:-${SAFE_RE}}"
  if [[ "$1" =~ ${re} ]]; then printf '%s' "$1"; else printf '（表示しない）'; fi
}
safe_name() { safe "$1" '^[A-Za-z0-9_.-]{1,64}$'; }

# 対象リポジトリの設定に左右されない git 実行（fsmonitor・フックを無効化）
g() { git -c core.fsmonitor=false -c core.hooksPath=/dev/null "$@"; }

# git の作業ツリーのルートで実行されていること（相対パスの解釈をぶらさない）
require_root() {
  local top
  g rev-parse --git-dir >/dev/null 2>&1 || die "git リポジトリではない"
  top="$(g rev-parse --show-toplevel 2>/dev/null)" || die "作業ツリーのルートを取得できない"
  ROOT_REAL="$(pwd -P)"
  [[ "${ROOT_REAL}" == "$(cd "${top}" && pwd -P)" ]] || die "リポジトリのルートで実行する（現在: $(safe "${ROOT_REAL}")）"
}

# 許可リスト: scaffold.py が唯一の定義元（所有ファイル・マニフェスト・.gitignore・THIRD-PARTY-LICENSES）
load_allowed() {
  # python3 -I: cwd（信頼できないリポジトリ）の .py を import しない
  ALLOWED="$(python3 -I -B "${SCRIPT_DIR}/scaffold.py" --target . --list-paths | python3 -I -B -c '
import json, sys
j = json.load(sys.stdin)
for p in j["owned"] + [j["manifest"]] + j["extra"]:
    print(p)
')" || die "scaffold.py --list-paths を実行できない"
  [[ -n "${ALLOWED}" ]] || die "許可リストが空"
}

is_allowed() { printf '%s\n' "${ALLOWED}" | grep -Fxq -- "$1"; }

# パスの検証: ルートからの相対・安全な文字・`..` なし・末尾 `/` なし・`.git`（大文字小文字を区別しない）配下でない
valid_path() {
  local p="$1" seg lower
  [[ -n "${p}" && "${p}" =~ ^[A-Za-z0-9_.][A-Za-z0-9_./-]{0,200}$ ]] || return 1
  [[ "${p}" != */ ]] || return 1
  lower="$(printf '%s' "${p}" | tr 'A-Z' 'a-z')"
  [[ "${lower}" != .git && "${lower}" != .git/* ]] || return 1
  local IFS=/
  for seg in ${p}; do
    [[ -n "${seg}" && "${seg}" != "." && "${seg}" != ".." ]] || return 1
  done
}

# 祖先ディレクトリが symlink・ファイルでないこと、実体がルートの下にあること
ancestors_ok() {
  local p="$1" acc="" seg dir real
  if [[ "${p}" == */* ]]; then
    local IFS=/
    for seg in ${p%/*}; do
      acc="${acc:+${acc}/}${seg}"
      [[ ! -L "${acc}" ]] || return 1
      if [[ -e "${acc}" && ! -d "${acc}" ]]; then return 1; fi
    done
    IFS=$' \t\n'
    dir="${p%/*}"
    if [[ -d "${dir}" ]]; then
      real="$(cd "${dir}" 2>/dev/null && pwd -P)" || return 1
      [[ "${real}" == "${ROOT_REAL}" || "${real}" == "${ROOT_REAL}"/* ]] || return 1
    fi
  fi
  return 0
}

# 現在の状態: ハッシュ（通常ファイル）/ MISSING / SYMLINK / SPECIAL / ANCESTOR（祖先が不適）。
# パスの中身・種別を見る唯一の入口。最初に祖先を検証し、不適なら何も開かず stat もしない。
#
# 第 2 引数 `norm`: 「HEAD と同じか」を比べるための、改行変換を適用したハッシュ（`git hash-object` の既定。HEAD の blob は
# 改行変換後の表現なので、生バイトの `--no-filters` では core.autocrlf・`text` / `eol` 属性が有効な環境で未編集のファイルも
# 「違う」になる）。filter 属性が指定されている（または判定不能な）パスでは、clean フィルタという外部コマンドを起動しない
# ために変換つきハッシュを呼ばず FILTERED を返す。既定（引数なし）は生バイト（`--no-filters`）で、記録・記録との比較に使う。
file_state() {
  local p="$1" mode="${2:-raw}" opt="--no-filters"
  if ! ancestors_ok "${p}"; then echo ANCESTOR
  elif [[ -L "${p}" ]]; then echo SYMLINK
  elif [[ ! -e "${p}" ]]; then echo MISSING
  elif [[ -f "${p}" ]]; then
    if [[ "${mode}" == norm ]]; then
      attr_filter "${p}" || { echo FILTERED; return 0; }
      opt=""
    fi
    # shellcheck disable=SC2086   # opt は空か --no-filters の定数（意図した単語分割）
    g hash-object ${opt} -- "${p}" 2>/dev/null || echo SPECIAL
  else echo SPECIAL
  fi
}

# 祖先に gitlink（submodule）があるか: 0 = ある / 1 = ない / 2 = 判定不能
ancestor_gitlink() {
  local p="$1" acc="" seg out rc
  [[ "${p}" == */* ]] || return 1
  local IFS=/
  for seg in ${p%/*}; do
    acc="${acc:+${acc}/}${seg}"
    out="$(g ls-tree HEAD -- "${acc}" 2>/dev/null)"; rc=$?
    [[ ${rc} -eq 0 ]] || return 2
    [[ "${out}" != *" commit "* ]] || return 0
  done
  return 1
}

# HEAD の状態: 0 = HEAD に通常ファイルとして在る（HEAD_SHA を設定）/ 1 = HEAD に無いと確定 / 2 = 判定不能・通常ファイルでない
HEAD_SHA=""
HEAD_MODE=""
head_entry() {
  local out rc mode type sha
  HEAD_SHA=""; HEAD_MODE=""
  out="$(g ls-tree HEAD -- "$1" 2>/dev/null)"; rc=$?
  [[ ${rc} -eq 0 ]] || return 2
  if [[ -z "${out}" ]]; then
    # 空 = HEAD に無いと確定、ただし祖先が submodule（gitlink）なら、その下のパスは親リポジトリの HEAD からは見えない
    # だけで「無い」わけではない。無いと誤判定して削除しないよう、判定不能として扱う
    ancestor_gitlink "$1"
    case $? in 0) return 2 ;; 1) return 1 ;; *) return 2 ;; esac
  fi
  read -r mode type sha _ <<< "${out%%$'\n'*}"
  [[ "${type}" == blob && ( "${mode}" == 100644 || "${mode}" == 100755 ) && "${sha}" =~ ^[0-9a-f]{40,64}$ ]] || return 2
  HEAD_SHA="${sha}"; HEAD_MODE="${mode}"
  return 0
}

# index のそのパスと HEAD の比較: 0 = 同じ（HEAD にも index にも無い場合を含む）/ 1 = 違う（ステージ済み）/ 2 = 判定不能
index_vs_head() {
  local p="$1" he flags lsout idx_mode idx_sha idx_stage
  head_entry "${p}"; he=$?
  [[ ${he} -ne 2 ]] || return 2
  flags="$(g ls-files -v -- "${p}" 2>/dev/null)" || return 2
  lsout="$(g ls-files -s -- "${p}" 2>/dev/null)" || return 2
  if [[ -z "${lsout}" ]]; then
    [[ -z "${flags}" ]] || return 2
    if [[ ${he} -eq 1 ]]; then return 0; else return 1; fi   # HEAD にあるのに index に無い = ステージ済みの削除
  fi
  [[ "${lsout}" != *$'\n'* ]] || return 2                      # 複数行 = 複数 stage（マージ中）
  [[ "${flags}" == "H ${p}" ]] || return 2                      # skip-worktree（S）・assume-unchanged（小文字）・未マージ等
  read -r idx_mode idx_sha idx_stage _ <<< "${lsout}"
  [[ "${idx_stage}" == 0 && "${idx_sha}" =~ ^[0-9a-f]{40,64}$ ]] || return 2
  [[ "${idx_mode}" == 100644 || "${idx_mode}" == 100755 ]] || return 2
  [[ ${he} -eq 0 ]] || return 1                                  # HEAD に無いのに index にエントリがある = ステージ済みの追加
  if [[ "${idx_sha}" == "${HEAD_SHA}" && "${idx_mode}" == "${HEAD_MODE}" ]]; then return 0; else return 1; fi
}

# スナップショットを「HEAD<TAB>sha」「REC<TAB>hash<TAB>path」「TAINT<TAB>path」の行へ整える（REC は後の記録が優先）
parse_snapshot() {
  [[ -f "$1" && ! -L "$1" ]] || return 1
  awk -F'\t' '
    $1=="HEAD" && NF==2 && !h { h=1; print "HEAD\t" $2 }
    $1=="REC" && NF==3 { r[$3]=$2 }
    $1=="TAINT" && NF==2 { t[$2]=1 }
    END { for (k in r) print "REC\t" r[k] "\t" k; for (k in t) print "TAINT\t" k }' "$1" | sort
}

# 記録先は作業ツリーの外の通常ファイルに限る
check_snap_for_write() {
  local snap="$1" dir real
  [[ ! -L "${snap}" ]] || die "SNAP がシンボリックリンク: $(safe "${snap}")"
  [[ ! -e "${snap}" || -f "${snap}" ]] || die "SNAP が通常ファイルでない: $(safe "${snap}")"
  dir="$(dirname "${snap}")"
  real="$(cd "${dir}" 2>/dev/null && pwd -P)" || die "SNAP の置き場所を解決できない: $(safe "${dir}")"
  if [[ "${real}" == "${ROOT_REAL}" || "${real}" == "${ROOT_REAL}"/* ]]; then
    die "SNAP を対象リポジトリの中に置かない（作業ツリーを汚す）: $(safe "${snap}")"
  fi
}

ensure_head() {
  local snap="$1" head
  if [[ -f "${snap}" ]] && grep -q $'^HEAD\t' "${snap}"; then return 0; fi
  head="$(g rev-parse --verify -q HEAD)" || die "HEAD を解決できない"
  printf 'HEAD\t%s\n' "${head}" >> "${snap}"
}

# 許可リストに載り、形式が正しく、祖先が安全なパスだけを返す（引数: パス）
usable_path() { valid_path "$1" && is_allowed "$1"; }

cmd_record() {
  local snap="$1"; shift
  local p st strict="${RECORD_STRICT:-1}"
  [[ $# -gt 0 ]] || die "record にパスが無い"
  check_snap_for_write "${snap}"
  ensure_head "${snap}"
  for p in "$@"; do
    valid_path "${p}" || die "不正なパス: $(safe "${p}")"
    if ! is_allowed "${p}"; then
      [[ "${strict}" == 1 ]] && die "許可リスト外のパス（所有ファイル・マニフェスト・.gitignore・THIRD-PARTY-LICENSES 以外は記録しない）: $(safe "${p}")"
      echo "記録しない（対象外）: $(safe "${p}")" >&2
      continue
    fi
    st="$(file_state "${p}")"
    if [[ "${st}" == ANCESTOR ]]; then echo "記録しない（祖先が symlink 等）: $(safe "${p}")" >&2; continue; fi
    if [[ ! "${st}" =~ ^[0-9a-f]{40,64}$ ]]; then
      echo "記録しない（${st}）: $(safe "${p}")" >&2
      continue
    fi
    printf 'REC\t%s\t%s\n' "${st}" "${p}" >> "${snap}"
    echo "記録: ${p}"
  done
}

cmd_record_json() {
  local snap="$1" paths p
  paths="$(python3 -I -B -c '
import json, sys
data = sys.stdin.buffer.read(1024 * 1024 + 1)
if len(data) > 1024 * 1024:
    sys.exit("JSON が大きすぎる")
j = json.loads(data.decode("utf-8"))
out = []
for u in j.get("updated") or []:
    out.append(u["path"] if isinstance(u, dict) else u)
out += list(j.get("created") or [])
if j.get("manifest_written"):
    out.append(sys.argv[1])
if j.get("gitignore_added"):
    out.append(".gitignore")
for p in out:
    if not isinstance(p, str) or "\n" in p or "\t" in p:
        sys.exit("不正なパス")
    print(p)
' "tools/docs-site-gen/.scaffold-manifest.json")" || die "JSON を読めない"
  [[ -n "${paths}" ]] || { echo "記録するパスなし（書き込みが起きていない）"; return 0; }
  while IFS= read -r p; do
    # created に利用者編集ファイル（許可リスト外）が含まれていても、記録しない = 自動では削除しない
    RECORD_STRICT=0 cmd_record "${snap}" "${p}" || return $?
  done <<< "${paths}"
}

cmd_guard() {
  local snap="$1" parsed p st he headsha last reason
  check_snap_for_write "${snap}"
  ensure_head "${snap}"
  parsed="$(parse_snapshot "${snap}")" || die "スナップショットを読めない"
  while IFS= read -r p; do
    valid_path "${p}" || continue
    st="$(file_state "${p}")"
    head_entry "${p}"; he=$?
    headsha=""; [[ ${he} -eq 0 ]] && headsha="${HEAD_SHA}"
    last="$(printf '%s\n' "${parsed}" | awk -F'\t' -v p="${p}" '$1=="REC" && $3==p { print $2 }')"
    reason=""
    if [[ "${st}" == ANCESTOR ]]; then
      reason="祖先が symlink 等"
    elif [[ "${st}" == SYMLINK || "${st}" == SPECIAL ]]; then
      reason="symlink・特殊ファイル"
    elif [[ "${st}" == MISSING ]]; then
      if [[ ${he} -ne 1 || -n "${last}" ]]; then reason="HEAD または前回の記録にあるのに消えている"; fi
    elif [[ "${st}" != "${last}" ]]; then
      # 前回の記録との比較は生バイト同士。HEAD との比較だけ、改行変換後の表現にそろえる（HEAD の blob は変換後）
      if [[ "$(file_state "${p}" norm)" != "${headsha}" || -z "${headsha}" ]]; then
        reason="HEAD とも前回の記録とも違う（利用者が触った）"
      fi
    fi
    if [[ -z "${reason}" && "${st}" != ANCESTOR ]] && ! index_vs_head "${p}"; then
      reason="index に HEAD と違う内容がある、または index の状態を判定できない（利用者がステージした可能性）"
    fi
    if [[ -n "${reason}" ]]; then
      printf 'TAINT\t%s\n' "${p}" >> "${snap}"
      echo "印: ${p}（${reason}。以後は自動では戻さない）"
    fi
  done <<< "${ALLOWED}"
}

cmd_status() {
  local snap="$1" parsed h t p st
  parsed="$(parse_snapshot "${snap}")" || { echo "スナップショットが無い・読めない: $(safe "${snap}")"; return 3; }
  while IFS=$'\t' read -r h t p; do
    [[ "${h}" == REC && -n "${p}" ]] || continue
    if printf '%s\n' "${parsed}" | grep -Fxq -- "$(printf 'TAINT\t%s' "${p}")"; then echo "taint $(safe "${p}")"; continue; fi
    st="$(file_state "${p}")"
    if [[ "${st}" == ANCESTOR ]]; then echo "ancestor $(safe "${p}")"
    elif [[ "${st}" == "${t}" ]]; then
      if index_vs_head "${p}"; then echo "match $(safe "${p}")"; else echo "staged $(safe "${p}")"; fi
    elif [[ "${st}" == MISSING ]]; then echo "missing $(safe "${p}")"
    elif [[ "${st}" == SYMLINK ]]; then echo "symlink $(safe "${p}")"
    elif [[ "${st}" == SPECIAL ]]; then echo "special $(safe "${p}")"
    else echo "changed $(safe "${p}")"
    fi
  done <<< "${parsed}"
}

# パスの filter 属性: 0 = 指定なし（unspecified / unset）/ 1 = 指定あり（値は FILTER_ATTR）/ 2 = 判定不能
# 出力は `<path>: filter: <value>`。パスは valid_path の文字種（コロン・空白を含まない）なので、先頭の完全一致で堅く解析する
FILTER_ATTR=""
attr_filter() {
  local out rc prefix val
  FILTER_ATTR=""
  out="$(g check-attr filter -- "$1" 2>/dev/null)"; rc=$?
  [[ ${rc} -eq 0 ]] || return 2
  prefix="$1: filter: "
  [[ "${out}" == "${prefix}"* && "${out}" != *$'\n'* ]] || return 2
  val="${out#"${prefix}"}"
  FILTER_ATTR="${val}"
  case "${val}" in
    unspecified|unset) return 0 ;;
    "") return 2 ;;
    *) return 1 ;;
  esac
}

# リポジトリのローカル設定に、git restore で外部コマンドが走り得るキーがあるか（あれば 0）
unsafe_local_config() {
  local out rc
  out="$(git config --local --get-regexp '^(filter\..*\.(smudge|clean|process)|core\.(fsmonitor|hookspath)|diff\..*\.(textconv|command)|include\.path|includeif\..*\.path)$' 2>/dev/null)"; rc=$?
  # キー名（filter.<サブセクション名>.smudge の名前部分は対象リポジトリ由来で信頼できない）は、安全な文字種のときだけ
  # 表示する。それ以外は「<セクション>.（名前は表示しない）」にする（セクション名は git が英数字とハイフンに制限する）
  case ${rc} in
    0)
      printf '%s\n' "${out}" | cut -d' ' -f1 | sort -u | while IFS= read -r k; do
        if [[ "${k}" =~ ^[A-Za-z0-9_.-]{1,100}$ ]]; then printf '%s ' "${k}"
        elif [[ "${k%%.*}" =~ ^[A-Za-z0-9-]{1,32}$ ]]; then printf '%s.（名前は表示しない） ' "${k%%.*}"
        else printf '（表示しない） '; fi
      done
      return 0 ;;
    1) return 1 ;;
    *) printf '(git config を取得できない)'; return 0 ;;
  esac
}

ask_all() { echo "ASK-ALL $1。何も戻していない。すべて利用者に確認する" >&2; exit 3; }

cmd_restore() {
  local snap="$1" dry="${2:-}" parsed kind h p t st he cur_head rec_head unsafe asks=0 act skip
  [[ -z "${dry}" || "${dry}" == "--dry-run" ]] || die "未知のオプション: ${dry}"
  parsed="$(parse_snapshot "${snap}")" || ask_all "スナップショットが無い・読めない"
  printf '%s\n' "${parsed}" | grep -q $'^REC\t' || ask_all "スナップショットに記録が無い"
  rec_head="$(printf '%s\n' "${parsed}" | awk -F'\t' '$1=="HEAD" { print $2; exit }')"
  [[ "${rec_head}" =~ ^[0-9a-f]{40,64}$ ]] || ask_all "スナップショットに HEAD の記録が無い"
  cur_head="$(g rev-parse --verify -q HEAD)" || ask_all "HEAD を解決できない"
  [[ "${cur_head}" == "${rec_head}" ]] || ask_all "記録後に HEAD が動いた（コミット・ブランチ移動など。記録 ${rec_head:0:12} → 現在 ${cur_head:0:12}）"
  if unsafe="$(unsafe_local_config)"; then
    ask_all "リポジトリのローカル設定に外部コマンドを実行し得るキーがある（${unsafe}）"
  fi
  while IFS=$'\t' read -r kind h p; do
    [[ "${kind}" == REC && -n "${p}" ]] || continue
    if ! usable_path "${p}"; then echo "ASK $(safe "${p}") 許可リスト外・不正なパス"; asks=$((asks + 1)); continue; fi
    if printf '%s\n' "${parsed}" | grep -Fxq -- "$(printf 'TAINT\t%s' "${p}")"; then
      echo "ASK ${p} 記録の基準を信頼できない（記録の前に利用者が触った）。差分を示して判断を仰ぐ"; asks=$((asks + 1)); continue
    fi
    st="$(file_state "${p}")"    # 祖先の検証を最初に行う入口（不適なら ANCESTOR。ここより前にパスを stat・open しない）
    if [[ "${st}" != "${h}" ]]; then
      # 記録（生バイト）と違う。ただし、すでに戻っている（復旧の 2 回目など）なら、現在の内容が HEAD と同じで index も
      # HEAD と同じことを、HEAD と同じ表現（改行変換後）で確かめて、ASK にせず済ませる。
      #   HEAD に通常ファイルとして在る: 変換後のハッシュ == HEAD の blob（filter 属性つきは比べない = ASK）
      #   HEAD に無い: ファイルが無く、index にもエントリが無い（すでに削除済み）
      skip=""
      if [[ "${st}" == MISSING ]]; then
        head_entry "${p}"; he=$?
        if [[ ${he} -eq 1 ]] && index_vs_head "${p}"; then skip="すでに無い（HEAD にも index にも無い）"; fi
      elif [[ "${st}" =~ ^[0-9a-f]{40,64}$ ]]; then
        head_entry "${p}"; he=$?
        if [[ ${he} -eq 0 && "$(file_state "${p}" norm)" == "${HEAD_SHA}" ]] && index_vs_head "${p}"; then
          skip="すでに HEAD と同じ内容"
        fi
      fi
      if [[ -n "${skip}" ]]; then echo "SKIP ${p} ${skip}"; continue; fi
      case "${st}" in
        ANCESTOR) echo "ASK ${p} 祖先ディレクトリが symlink 等（リンク先は読まない）" ;;
        MISSING) echo "ASK ${p} 消えている（利用者が削除した可能性）" ;;
        SYMLINK) echo "ASK ${p} symlink に変わっている" ;;
        SPECIAL) echo "ASK ${p} 通常ファイルでなくなっている" ;;
        *)       echo "ASK ${p} 書き込み後に内容が変わっている（利用者の編集の可能性）。差分を示して判断を仰ぐ" ;;
      esac
      asks=$((asks + 1)); continue
    fi
    index_vs_head "${p}"; he=$?
    if [[ ${he} -ne 0 ]]; then
      if [[ ${he} -eq 1 ]]; then echo "ASK ${p} index に HEAD と違う内容がある（利用者がステージした）。復元は index も戻すため自動では戻さない"
      else echo "ASK ${p} index の状態を判定できない（マージ中・skip-worktree・assume-unchanged・git のエラー）"; fi
      asks=$((asks + 1)); continue
    fi
    head_entry "${p}"; he=$?
    case ${he} in
      0) act=restore ;;
      1) act=delete ;;
      *) echo "ASK ${p} HEAD での状態を判定できない（git のエラー・通常ファイルでない）"; asks=$((asks + 1)); continue ;;
    esac
    if [[ "${act}" == restore ]]; then
      attr_filter "${p}"; he=$?
      if [[ ${he} -ne 0 ]]; then
        if [[ ${he} -eq 1 ]]; then echo "ASK ${p} filter 属性が指定されている（$(safe_name "${FILTER_ATTR}")）。復元で外部コマンドが動き得るため自動では戻さない"
        else echo "ASK ${p} filter 属性を判定できない（git check-attr の失敗・想定外の出力）"; fi
        asks=$((asks + 1)); continue
      fi
      if [[ -n "${dry}" ]]; then echo "WOULD-RESTORE ${p}"
      elif g restore --source=HEAD --worktree -- "${p}" 2>/dev/null; then echo "RESTORED ${p}"
      else echo "ASK ${p} git restore に失敗した"; asks=$((asks + 1)); fi
    else
      if [[ -n "${dry}" ]]; then echo "WOULD-DELETE ${p}"
      elif rm -- "${p}" 2>/dev/null; then echo "DELETED ${p}"
      else echo "ASK ${p} 削除に失敗した"; asks=$((asks + 1)); fi
    fi
  done <<< "${parsed}"
  if [[ ${asks} -gt 0 ]]; then
    echo "ASK が ${asks} 件ある。利用者の判断が済むまで git switch・git branch -D へ進まない" >&2
    return 4
  fi
  return 0
}

main() {
  [[ $# -ge 2 ]] || die "使い方: $0 guard|record|record-json|status|restore SNAP [path...|--dry-run]"
  local cmd="$1" snap="$2"; shift 2
  require_root
  load_allowed
  case "${cmd}" in
    guard)       cmd_guard "${snap}" ;;
    record)      cmd_record "${snap}" "$@" ;;
    record-json) cmd_record_json "${snap}" ;;
    status)      cmd_status "${snap}" ;;
    restore)     cmd_restore "${snap}" "${1:-}" ;;
    *)           die "未知のサブコマンド: ${cmd}" ;;
  esac
}

main "$@"
