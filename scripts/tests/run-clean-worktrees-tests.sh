#!/usr/bin/env bash
# clean-worktrees.sh のセルフテスト（イシュー #221）:
# 一時 git リポジトリに登録済み worktree（clean/dirty）と孤児（orphan）ディレクトリを
# 再現し、dry-run / --apply それぞれの分類・削除判定を検証する。run-feature-flow-tests.sh
# と同じく、実 workspace（.claude/worktrees/）や実データには一切触れない。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

PASS_COUNT=0
FAIL_COUNT=0

fail() {
    echo "FAIL: $1" >&2
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

pass() {
    echo "PASS: $1"
    PASS_COUNT=$((PASS_COUNT + 1))
}

assert_contains() {
    local desc="$1"
    local haystack="$2"
    local needle="$3"
    if printf '%s' "${haystack}" | grep -qF -- "${needle}"; then
        pass "${desc}"
    else
        fail "${desc}（'${needle}' が出力に含まれません）"
    fi
}

assert_not_contains() {
    local desc="$1"
    local haystack="$2"
    local needle="$3"
    if printf '%s' "${haystack}" | grep -qF -- "${needle}"; then
        fail "${desc}（'${needle}' が出力に含まれています）"
    else
        pass "${desc}"
    fi
}

# メインリポジトリ + .claude/worktrees/ 配下に登録済み worktree（clean/dirty）と
# 孤児ディレクトリを 1 件ずつ用意する。
setup_repo() {
    local dir
    dir="$(mktemp -d)"
    (
        cd "${dir}"
        git init -q
        git -c user.name=test -c user.email=test@example.invalid commit -q --allow-empty -m "init"
        mkdir -p .claude/worktrees

        # 登録済み・clean な worktree。
        git worktree add -q -b wt-clean-branch .claude/worktrees/wt-clean >/dev/null

        # 登録済み・dirty な worktree（未コミット変更あり）。
        git worktree add -q -b wt-dirty-branch .claude/worktrees/wt-dirty >/dev/null
        echo "uncommitted" > .claude/worktrees/wt-dirty/dirty.txt

        # 孤児を再現する: 一旦登録済み worktree を作った後、git のメタデータ
        # （.git/worktrees/<id>）だけを削除し、ディレクトリ実体は残す。
        # これはリポジトリ改名（#209〜#211）等で .git 参照が失われたケースの再現。
        git worktree add -q -b wt-orphan-branch .claude/worktrees/wt-orphan >/dev/null
        echo "orphan source file" > .claude/worktrees/wt-orphan/keep-me.txt
        mkdir -p .claude/worktrees/wt-orphan/target
        echo "build artifact" > .claude/worktrees/wt-orphan/target/dummy.bin
        rm -rf .git/worktrees/wt-orphan
    )
    printf '%s' "${dir}"
}

run_clean_in() {
    local repo="$1"
    shift
    FANDHE_BACKEND_CLEAN_WORKTREES_REPO_ROOT="${repo}" bash "${SCRIPTS_DIR}/clean-worktrees.sh" "$@"
}

echo "===== ケース1: dry-run（既定）では登録済み worktree は削除対象外、孤児は削除候補として報告される ====="
REPO1="$(setup_repo)"
out1="$(cd "${REPO1}" && run_clean_in "${REPO1}" 2>&1)"
assert_contains "clean な登録済み worktree が REGISTERED[clean] と報告される" "${out1}" "REGISTERED[clean]  wt-clean"
assert_contains "dirty な登録済み worktree が REGISTERED[dirty] と報告される" "${out1}" "REGISTERED[dirty]  wt-dirty"
assert_contains "孤児ディレクトリが ORPHAN と報告される" "${out1}" "ORPHAN  wt-orphan"
assert_contains "dry-run である旨のメッセージが出る" "${out1}" "dry-run のため削除は行いません"
if [ -d "${REPO1}/.claude/worktrees/wt-orphan" ]; then
    pass "dry-run では孤児ディレクトリが削除されない"
else
    fail "dry-run にもかかわらず孤児ディレクトリが削除された"
fi
rm -rf "${REPO1}"

echo "===== ケース2: --apply で孤児のみ退避・削除され、登録済み（clean/dirty）は残る ====="
REPO2="$(setup_repo)"
out2="$(cd "${REPO2}" && run_clean_in "${REPO2}" --apply 2>&1)"
assert_contains "削除件数が 1 件と報告される" "${out2}" "削除 1 件"
if [ -d "${REPO2}/.claude/worktrees/wt-clean" ]; then
    pass "--apply 後も登録済み clean worktree は残る"
else
    fail "--apply で登録済み clean worktree まで削除された"
fi
if [ -d "${REPO2}/.claude/worktrees/wt-dirty" ]; then
    pass "--apply 後も登録済み dirty worktree は残る"
else
    fail "--apply で登録済み dirty worktree まで削除された"
fi
if [ -d "${REPO2}/.claude/worktrees/wt-orphan" ]; then
    fail "--apply 後も孤児ディレクトリが残っている"
else
    pass "--apply で孤児ディレクトリが削除される"
fi
salvage_tar="${REPO2}/_/worktree-salvage/wt-orphan.tar.gz"
if [ -f "${salvage_tar}" ]; then
    pass "孤児の退避 tar が作成される"
    extract_dir="$(mktemp -d)"
    tar -xzf "${salvage_tar}" -C "${extract_dir}"
    if [ -f "${extract_dir}/wt-orphan/keep-me.txt" ]; then
        pass "退避 tar から source ファイルを復元できる"
    else
        fail "退避 tar に source ファイル（keep-me.txt）が含まれていない"
    fi
    if [ -f "${extract_dir}/wt-orphan/target/dummy.bin" ]; then
        fail "退避 tar に target/ の成果物が含まれている（除外されるべき）"
    else
        pass "退避 tar から target/ が除外されている"
    fi
    rm -rf "${extract_dir}"
else
    fail "孤児の退避 tar（${salvage_tar}）が作成されていない"
fi
rm -rf "${REPO2}"

echo "===== ケース3: --apply --no-salvage では退避 tar を作らずに削除する ====="
REPO3="$(setup_repo)"
out3="$(cd "${REPO3}" && run_clean_in "${REPO3}" --apply --no-salvage 2>&1)"
assert_not_contains "--no-salvage 時は退避先メッセージを出さない" "${out3}" "退避先:"
if [ -d "${REPO3}/_/worktree-salvage" ]; then
    fail "--no-salvage にもかかわらず退避ディレクトリが作成された"
else
    pass "--no-salvage では退避ディレクトリを作成しない"
fi
if [ -d "${REPO3}/.claude/worktrees/wt-orphan" ]; then
    fail "--no-salvage でも孤児ディレクトリは削除されるべきだが残っている"
else
    pass "--no-salvage でも孤児ディレクトリは削除される"
fi
rm -rf "${REPO3}"

echo "===================================="
echo "PASS: ${PASS_COUNT} / FAIL: ${FAIL_COUNT}"
if [ "${FAIL_COUNT}" -ne 0 ]; then
    exit 1
fi
exit 0
