# check_repo: GitHub の `owner/repo` 形式の検証（SKILL.md の手順が `source` して使う）。
#
# 命名規則（owner: 英数字とハイフン・先頭末尾と連続ハイフン不可・39 文字以内、repo: 英数字 - _ .・100 文字以内・
# `.` `..` 単独と `.git` 終端は不可）は scripts/_common.py の valid_owner / valid_repo_name が正で、
# tests/test_rebrand.py が本関数との一致を検証する。値を gh や git のコマンドへ渡す前に必ず通す
# （外部入力をそのままシェルへ渡さない）。使い方: check_repo "${REPO}" || { echo "不正"; exit 1; }
check_repo() {
  local owner="${1%%/*}" name="${1#*/}"
  [[ "$1" == */* && "${name}" != */* && "${#owner}" -le 39 && "${#name}" -le 100 ]] \
    && [[ "${owner}" =~ ^[A-Za-z0-9]+(-[A-Za-z0-9]+)*$ ]] \
    && [[ "${name}" =~ ^[A-Za-z0-9_.-]+$ && "${name}" != "." && "${name}" != ".." && "${name}" != *.git ]]
}
