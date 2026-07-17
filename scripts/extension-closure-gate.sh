#!/usr/bin/env bash
# 拡張点への変更影響範囲閉包 PR ゲート（TASK-13.2/#50 で新設、
# docs/spec/04-requirements.md REQ-13・docs/design/dependency-graph-contract.md 4 節）。
#
# `scripts/extension-closure-check.sh`（TASK-13.1/#49）が確立した A〜D/E 判定エンジンを
# 「新規プラグイン追加 PR への自動実行」（同スクリプト冒頭 doc の TASK-13.2 引き継ぎ事項）
# として運用するための薄いラッパー。
#
# 手順:
#   1. `git diff --name-only <merge-base>...HEAD` で変更ファイル一覧を取得する
#   2. 変更に `crates/plugin-*` または `crates/core/src/plugin.rs` が含まれない場合は
#      「拡張点に無関係な PR」として対象外（SKIP・exit 0）とする
#   3. 含まれる場合、`extension-closure-check.sh --files-from` で閉包判定する
#   4. E（閉包違反候補）ファイルがあれば、HEAD 時点の `docs/design/*.md` に各 E ファイル
#      パスの記載（`grep -F`）があるか照合する。全件記載ありなら「理由明記済み逸脱」として
#      WARN 付き PASS、1 件でも未記載なら FAIL とする
#      （`docs/design/dependency-graph-contract.md` 4 節の運用手順の実装）
#
# 判定不能（git 失敗・merge-base 解決不能・不正な ref・空差分等）はフェイルクローズで FAIL
# とし、「検証していないのに PASS 扱い」を防ぐ（`.claude/rules/security.md`）。
#
# 使い方:
#   scripts/extension-closure-gate.sh --base <ref>          # CI: origin/${{ github.base_ref }}
#   scripts/extension-closure-gate.sh --files-from <file>   # セルフテスト用注入口（1 行 1 パス）
#
# セキュリティ: ref は `git rev-parse --verify --end-of-options` で検証してから使用し、
# `eval` は使用しない。diff 由来のファイルパスはデータとして扱い（`printf '%s'`・`grep -F`）、
# シェル展開させない（`.claude/rules/security.md` A03 インジェクション対策）。
#
# 呼び出し元: `.github/workflows/ci.yml` の `unsafe-triage` ジョブ（`pull_request` イベント時
# のみ、`--base origin/${{ github.base_ref }}`）。人間が直接実行する場合は
# `--base origin/main` 等を指定する。
#
# セルフテスト: `scripts/tests/run-extension-closure-gate-tests.sh`
#   （fixture のファイルリストで SKIP/PASS/WARN-PASS/FAIL・フェイルクローズ挙動を固定化する）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${WORKSPACE_ROOT}"

BASE_REF=""
FILES_FROM=""
while [ $# -gt 0 ]; do
    case "$1" in
        --base)
            BASE_REF="$2"
            shift 2
            ;;
        --files-from)
            FILES_FROM="$2"
            shift 2
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

HAS_FAIL=0

fail() {
    echo "[FAIL] $1" >&2
    HAS_FAIL=1
}

pass() {
    echo "[PASS] $1"
}

warn() {
    echo "[WARN] $1"
}

skip() {
    echo "[SKIP] $1"
}

if [ -z "${BASE_REF}" ] && [ -z "${FILES_FROM}" ]; then
    fail "入力 — --base <ref> または --files-from <file> のいずれかが必須です"
    echo "[RESULT] FAIL"
    exit 1
fi
if [ -n "${BASE_REF}" ] && [ -n "${FILES_FROM}" ]; then
    fail "入力 — --base と --files-from は同時指定できません"
    echo "[RESULT] FAIL"
    exit 1
fi

changed_files=""

if [ -n "${BASE_REF}" ]; then
    # ref 検証（インジェクション対策・誤入力の早期検出）。`--end-of-options` で
    # ref 文字列がオプションとして解釈される余地を断つ。フェイルクローズ
    if ! git rev-parse --verify --end-of-options "${BASE_REF}" >/dev/null 2>/tmp/extension-closure-gate-ref.log; then
        fail "git — --base の値 '${BASE_REF}' を ref として解決できません（/tmp/extension-closure-gate-ref.log 参照。shallow clone で base が未取得の可能性）"
        echo "[RESULT] FAIL"
        exit 1
    fi

    merge_base="$(git merge-base "${BASE_REF}" HEAD 2>/tmp/extension-closure-gate-mergebase.log || true)"
    if [ -z "${merge_base}" ]; then
        fail "git — '${BASE_REF}' と HEAD の merge-base を解決できません（/tmp/extension-closure-gate-mergebase.log 参照）"
        echo "[RESULT] FAIL"
        exit 1
    fi

    changed_files="$(git diff --name-only "${merge_base}"...HEAD -- 2>/tmp/extension-closure-gate-diff.log || true)"
    if [ -s /tmp/extension-closure-gate-diff.log ]; then
        fail "git — diff の実行に失敗しました（/tmp/extension-closure-gate-diff.log 参照）"
        echo "[RESULT] FAIL"
        exit 1
    fi
else
    if [ ! -f "${FILES_FROM}" ]; then
        fail "入力 — --files-from が指す ${FILES_FROM} が存在しません"
        echo "[RESULT] FAIL"
        exit 1
    fi
    changed_files="$(cat "${FILES_FROM}")"
fi

changed_files_count="$(printf '%s\n' "${changed_files}" | grep -c . || true)"
if [ "${changed_files_count}" -eq 0 ]; then
    fail "対象 — 変更ファイルが 0 件でした（測定不能）"
    echo "[RESULT] FAIL"
    exit 1
fi

echo "変更ファイル総数: ${changed_files_count}"

# 手順 2: 拡張点関連の変更を含むかどうかで対象外判定する。
plugin_related=0
while IFS= read -r f; do
    [ -z "${f}" ] && continue
    case "${f}" in
        crates/plugin-*|crates/core/src/plugin.rs)
            plugin_related=1
            ;;
    esac
done <<< "${changed_files}"

if [ "${plugin_related}" -eq 0 ]; then
    skip "対象外 — crates/plugin-* / crates/core/src/plugin.rs への変更を含まないため閉包判定は不要"
    echo "[RESULT] SKIP"
    exit 0
fi

# 手順 3: 閉包判定エンジンを --files-from 経由で呼び出す（メモリ上の changed_files を
# 一時ファイルへ書き出す。extension-closure-check.sh は既存の入力口をそのまま使う）。
closure_files_tmp="$(mktemp /tmp/extension-closure-gate-files.XXXXXX)"
trap 'rm -f "${closure_files_tmp}"' EXIT
printf '%s\n' "${changed_files}" > "${closure_files_tmp}"

set +e
closure_output="$("${SCRIPT_DIR}/extension-closure-check.sh" --files-from "${closure_files_tmp}" 2>&1)"
closure_status=$?
set -e

echo "--- extension-closure-check.sh 出力 ---"
echo "${closure_output}"
echo "--- ここまで ---"

if [ "${closure_status}" -eq 0 ]; then
    pass "閉包 — 変更ファイルは全て A〜D カテゴリに収まっています"
    echo "[RESULT] PASS"
    exit 0
fi

# 手順 4: E ファイルの理由明記照合。extension-closure-check.sh の出力形式
# "  [E] <path>" から E ファイル一覧を抽出する（同スクリプトの出力フォーマット契約）。
mapfile -t e_files < <(printf '%s\n' "${closure_output}" | sed -n 's/^  \[E\] //p')

if [ "${#e_files[@]}" -eq 0 ]; then
    # closure_status が非 0 なのに E ファイルが 0 件 = extension-closure-check.sh 側の
    # 別の失敗要因（入力エラー等）。理由明記の照合対象がないためフェイルクローズで FAIL。
    fail "閉包 — extension-closure-check.sh が FAIL しましたが E ファイルを抽出できませんでした（判定不能。出力形式の乖離の可能性）"
    echo "[RESULT] FAIL"
    exit 1
fi

echo "E（閉包違反候補）ファイル: ${#e_files[@]} 件"

design_docs=()
if [ -d docs/design ]; then
    while IFS= read -r -d '' f; do
        design_docs+=("${f}")
    done < <(find docs/design -maxdepth 1 -name '*.md' -print0)
fi

if [ "${#design_docs[@]}" -eq 0 ]; then
    fail "閉包 — docs/design/*.md が 1 件も見つかりません（理由記載の照合先が存在せず判定不能）"
    echo "[RESULT] FAIL"
    exit 1
fi

undocumented=()
for e_file in "${e_files[@]}"; do
    documented=0
    for doc in "${design_docs[@]}"; do
        if grep -qF -- "${e_file}" "${doc}"; then
            documented=1
            break
        fi
    done
    if [ "${documented}" -eq 0 ]; then
        undocumented+=("${e_file}")
    fi
done

if [ "${#undocumented[@]}" -eq 0 ]; then
    warn "閉包 — E ファイル ${#e_files[@]} 件は全て docs/design/*.md に理由記載あり（理由明記済み逸脱）"
    pass "閉包 — 理由明記済み逸脱として受理"
    echo "[RESULT] PASS"
    exit 0
else
    fail "閉包 — 理由記載のない E ファイルが ${#undocumented[@]} 件あります（docs/design/*.md への理由記載が必要、docs/design/dependency-graph-contract.md 4 節）"
    for f in "${undocumented[@]}"; do
        echo "  [未記載] ${f}" >&2
    done
    echo "[RESULT] FAIL"
    exit 1
fi
