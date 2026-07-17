#!/usr/bin/env bash
# 変更影響範囲閉包の機械判定（TASK-13.1/#49 で新設、docs/spec/04-requirements.md REQ-13・
# docs/spec/05-tasks.md TASK-13.1）。
#
# 3 種の拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`。実体は
# `crates/core/src/plugin.rs` の固定シーム `try_intercept` / `try_handle_upgrade` に
# 集約される。`docs/design/plugin-boundary.md` 3〜5 節参照）への集約により、新規プロトコル
# 追加時の変更ファイル一覧が「プラグインクレート内 + コア側許容シーム + テスト + 文書/運用」の
# 4 カテゴリ（A〜D）に閉じるかを機械判定する。A〜D の外（E）に該当するファイルが 1 件でも
# あれば「閉包していない」と判定し、当該ファイルを列挙する（REQ-13 の「新規プロトコル追加が
# 既存拡張点に閉じるか、閉じない場合はその理由が設計文書に明記される」という受け入れ基準を
# 実例で検証するための土台）。
#
# 分類（先勝ち・上から順に評価）:
#   C. テスト:               crates/core/tests/**、crates/plugin-*/tests/**
#   A. プラグインクレート内: crates/plugin-*/**（C に該当しない残り）
#   B. コア側許容シーム:     crates/core/Cargo.toml・crates/core/src/plugin.rs・
#                             crates/core/src/server.rs・crates/core/src/lib.rs
#   D. ドキュメント・運用:   docs/**、scripts/**、CLAUDE.md、AGENTS.md、.github/**、
#                             deny.toml（依存ライセンス許可リスト。crate 実装ではなく
#                             workspace 全体の依存ガバナンス設定。`docs/design/plugin-boundary.md`
#                             345 行が示すとおり、新規プラグイン追加に伴うライセンス許可追加は
#                             `dep-audit.sh` が検証する運用上の副作用であり、拡張点の設計失陥では
#                             ないためホワイトリストに含める）
#   E. 上記いずれにも該当しない残り（例: crates/http/**、crates/routes/**、
#      crates/core/src/ のその他ファイル）→ 閉包違反
#
# 使い方:
#   scripts/extension-closure-check.sh --commit <sha>       # git diff-tree で変更ファイルを取得
#   scripts/extension-closure-check.sh --files-from <file>  # セルフテスト用注入口（1 行 1 パス）
#
# 判定不能（sha 形式不正・git 失敗・引数欠落・対象ファイル 0 件）はフェイルクローズで FAIL とし、
# 「検証していないのに PASS 扱い」を防ぐ（`.claude/rules/security.md`）。sha は
# `^[0-9a-fA-F]{7,40}$` で形式検証してから git へ渡し、`eval` は使用しない
# （`.claude/rules/security.md` A03 インジェクション対策）。
#
# 呼び出し元: 人間が実例 3 件（WebSocket/GraphQL/WebRTC 追加コミット）に対して直接実行する
# （`docs/design/extension-closure-verification.md` 参照）。実コミット sha の検証自体は
# shallow clone 環境で履歴が無いと誤 FAIL するため CI には組み込まない
# （常設受け入れテスト化は TASK-13.2/#50 のスコープ）。
#
# セルフテスト: `scripts/tests/run-extension-closure-tests.sh`
#   （fixture のファイルリストで PASS/FAIL/フェイルクローズ挙動を固定化する）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${WORKSPACE_ROOT}"

COMMIT_SHA=""
FILES_FROM=""
while [ $# -gt 0 ]; do
    case "$1" in
        --commit)
            COMMIT_SHA="$2"
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

# 入力チェック: --commit と --files-from はどちらか一方が必須（フェイルクローズ）
if [ -z "${COMMIT_SHA}" ] && [ -z "${FILES_FROM}" ]; then
    fail "入力 — --commit <sha> または --files-from <file> のいずれかが必須です"
    echo "[RESULT] FAIL"
    exit 1
fi
if [ -n "${COMMIT_SHA}" ] && [ -n "${FILES_FROM}" ]; then
    fail "入力 — --commit と --files-from は同時指定できません"
    echo "[RESULT] FAIL"
    exit 1
fi

changed_files=""

if [ -n "${COMMIT_SHA}" ]; then
    # sha 形式検証（インジェクション対策・誤入力の早期検出）。フェイルクローズ
    if ! printf '%s' "${COMMIT_SHA}" | grep -Eq '^[0-9a-fA-F]{7,40}$'; then
        fail "入力 — --commit の値 '${COMMIT_SHA}' が commit sha 形式（英数字 7〜40 文字）ではありません"
        echo "[RESULT] FAIL"
        exit 1
    fi
    if ! git cat-file -e "${COMMIT_SHA}^{commit}" 2>/dev/null; then
        fail "git — commit ${COMMIT_SHA} が解決できません（履歴が浅い clone か、sha が誤っている可能性）"
        echo "[RESULT] FAIL"
        exit 1
    fi
    changed_files="$(git diff-tree --no-commit-id --name-only -r "${COMMIT_SHA}" -- 2>/tmp/extension-closure-check-git.log || true)"
    if [ -s /tmp/extension-closure-check-git.log ]; then
        fail "git — diff-tree の実行に失敗しました（/tmp/extension-closure-check-git.log 参照）"
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

if [ -z "${changed_files}" ]; then
    fail "対象 — 変更ファイルが 0 件でした（測定不能）"
    echo "[RESULT] FAIL"
    exit 1
fi

declare -a cat_a=()
declare -a cat_b=()
declare -a cat_c=()
declare -a cat_d=()
declare -a cat_e=()

while IFS= read -r f; do
    [ -z "${f}" ] && continue
    case "${f}" in
        crates/core/tests/*|crates/plugin-*/tests/*)
            cat_c+=("${f}")
            ;;
        crates/plugin-*/*)
            cat_a+=("${f}")
            ;;
        crates/core/Cargo.toml|crates/core/src/plugin.rs|crates/core/src/server.rs|crates/core/src/lib.rs)
            cat_b+=("${f}")
            ;;
        docs/*|scripts/*|CLAUDE.md|AGENTS.md|.github/*|deny.toml)
            cat_d+=("${f}")
            ;;
        *)
            cat_e+=("${f}")
            ;;
    esac
done <<< "${changed_files}"

echo "対象ファイル総数: $(printf '%s\n' "${changed_files}" | grep -c . || true)"
echo "  A. プラグインクレート内: ${#cat_a[@]} 件"
echo "  B. コア側許容シーム:     ${#cat_b[@]} 件"
echo "  C. テスト:               ${#cat_c[@]} 件"
echo "  D. ドキュメント・運用:   ${#cat_d[@]} 件"
echo "  E. 閉包違反候補:         ${#cat_e[@]} 件"

if [ "${#cat_e[@]}" -gt 0 ]; then
    fail "閉包 — A〜D のいずれにも該当しないファイルが ${#cat_e[@]} 件あります（拡張点への閉包違反）"
    for f in "${cat_e[@]}"; do
        echo "  [E] ${f}" >&2
    done
else
    pass "閉包 — 全 $(printf '%s\n' "${changed_files}" | grep -c . || true) 件が A〜D に収まっています"
fi

if [ "${HAS_FAIL}" -eq 1 ]; then
    echo "[RESULT] FAIL"
    exit 1
else
    echo "[RESULT] PASS"
    exit 0
fi
