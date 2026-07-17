#!/usr/bin/env bash
# TASK-12.4-1（#85）第三者検証ハーネス。
#
# `docs/design/third-party-verification.md` 5 節の完遂判定手順を機械化する。
# 被験 AI（別セッション・別モデル）が実装した使い捨て worktree を対象に、
# 完遂の一次判定（機械ゲート）を実行し PASS / FAIL / PENDING を出力する。
#
#   PASS    : fmt / clippy / test が全通過し、起点コミットに対するリグレッションが 0 件
#   FAIL    : 上記いずれかが失敗、またはタイムアウト
#   PENDING : worktree・起点コミットが指定要件を満たさず判定できない（未実施を含む）
#
# 二次判定（タスク定義の受け入れ基準充足）は本ハーネスの対象外（評価者が別途確認し、
# `docs/reports/task-12-4-1-completion-rate-verification.md` へ記録する）。
#
# 引数を受けるためシェル再解釈（eval 等）を一切使わず、変数は必ずクォートする
# （OWASP A03 対策、.claude/rules/security.md）。被験 AI が生成したコードは信頼しない
# 前提のため、対象 worktree はメイン working copy とは別ディレクトリであることを要求し、
# 実行はそのディレクトリ内に閉じる。
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# 1 タスクあたりの各ゲートの上限時間（秒）。被験実装がハングするバグを含みうる
# （PoC-9 BUG-3 の教訓）ため、超過時は「未完遂」として扱い完遂率を楽観方向へ歪めない。
TIMEOUT_SECONDS="${THIRD_PARTY_VERIFY_TIMEOUT:-600}"

usage() {
    cat >&2 <<'EOF'
使い方: third-party-verify.sh --worktree <path> --task-id <ID> [--baseline-tests <file>]

  --worktree <path>        被験 AI が実装した使い捨て worktree の絶対パス（必須）
  --task-id <ID>           対象タスク ID（docs/reports/task-12-4-1-task-definitions.md 参照。必須）
  --baseline-tests <file>  起点コミットの `cargo test` 出力ログ（リグレッション突合用。省略時は突合をスキップし判定に PENDING 注記を付与）
EOF
}

WORKTREE=""
TASK_ID=""
BASELINE_TESTS=""

while [ $# -gt 0 ]; do
    case "$1" in
        --worktree)
            WORKTREE="${2:-}"
            shift 2
            ;;
        --task-id)
            TASK_ID="${2:-}"
            shift 2
            ;;
        --baseline-tests)
            BASELINE_TESTS="${2:-}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "[PENDING] 不明な引数: $1" >&2
            usage
            exit 2
            ;;
    esac
done

if [ -z "${WORKTREE}" ] || [ -z "${TASK_ID}" ]; then
    echo "[PENDING] --worktree と --task-id は必須です" >&2
    usage
    exit 2
fi

if [ ! -d "${WORKTREE}" ]; then
    echo "[PENDING] タスク ${TASK_ID}: worktree が見つかりません: ${WORKTREE}"
    exit 0
fi

# メイン working copy を誤って対象に渡された場合の事故防止（独立性担保、3 節 (B)）。
if [ "$(cd "${WORKTREE}" && pwd)" = "${REPO_ROOT}" ]; then
    echo "[PENDING] タスク ${TASK_ID}: worktree にメイン working copy 自体は指定できません"
    exit 0
fi

if [ ! -d "${WORKTREE}/.git" ] && [ ! -f "${WORKTREE}/.git" ]; then
    echo "[PENDING] タスク ${TASK_ID}: ${WORKTREE} は git worktree ではありません"
    exit 0
fi

run_gate() {
    local desc="$1"
    shift
    if timeout "${TIMEOUT_SECONDS}" "$@" >"${GATE_LOG}" 2>&1; then
        echo "[PASS] ${desc}"
        return 0
    else
        local rc=$?
        echo "[FAIL] ${desc}（終了コード ${rc}。ログ: ${GATE_LOG}）"
        return 1
    fi
}

GATE_LOG="$(mktemp)"
trap 'rm -f "${GATE_LOG}"' EXIT

overall_fail=0
pending_note=""

echo "==================================================="
echo "第三者検証ハーネス — タスク ${TASK_ID}（worktree: ${WORKTREE}）"
echo "==================================================="

(
    cd "${WORKTREE}" || exit 2

    if ! run_gate "cargo fmt --check" cargo fmt --all --check; then
        exit 1
    fi
    if ! run_gate "cargo clippy --all-features -- -D warnings" cargo clippy --workspace --all-features -- -D warnings; then
        exit 1
    fi
    if ! run_gate "cargo test --workspace --all-features" cargo test --workspace --all-features; then
        exit 1
    fi
    exit 0
)
gate_rc=$?

if [ "${gate_rc}" -ne 0 ]; then
    overall_fail=1
fi

# リグレッション突合: ベースラインのテスト結果ログが与えられている場合のみ実施する。
# 与えられない場合は機械ゲートの PASS/FAIL 判定はそのまま有効とし、リグレッション欄を
# PENDING として区別する（FAIL とは混同しない）。
if [ "${overall_fail}" -eq 0 ]; then
    if [ -n "${BASELINE_TESTS}" ]; then
        if [ ! -f "${BASELINE_TESTS}" ]; then
            pending_note="ベースラインログが見つかりません: ${BASELINE_TESTS}"
        else
            # `test result:` 行は nextest/cargo test 標準出力の集計行。ここでは簡易的に
            # 失敗テスト名の集合が起点コミットに対して増えていないかのみを突合する。
            baseline_failed="$(grep -E '^FAILED ' "${BASELINE_TESTS}" 2>/dev/null | sort -u || true)"
            current_failed="$(grep -E '^FAILED ' "${GATE_LOG}" 2>/dev/null | sort -u || true)"
            new_failed="$(comm -13 <(printf '%s\n' "${baseline_failed}") <(printf '%s\n' "${current_failed}") 2>/dev/null | grep -v '^$' || true)"
            if [ -n "${new_failed}" ]; then
                echo "[FAIL] リグレッション検出: 起点コミットで PASS していたテストが失敗しています"
                printf '%s\n' "${new_failed}"
                overall_fail=1
            else
                echo "[PASS] リグレッションなし（起点コミットとの突合）"
            fi
        fi
    else
        pending_note="--baseline-tests 未指定のためリグレッション突合は未実施（PENDING）"
    fi
fi

echo "---------------------------------------------------"
if [ "${overall_fail}" -ne 0 ]; then
    echo "[FAIL] タスク ${TASK_ID}: 機械ゲート不通過"
    exit 1
fi

if [ -n "${pending_note}" ]; then
    echo "[PENDING] タスク ${TASK_ID}: 機械ゲートは PASS したが ${pending_note}"
    exit 0
fi

echo "[PASS] タスク ${TASK_ID}: 機械ゲート全通過"
exit 0
