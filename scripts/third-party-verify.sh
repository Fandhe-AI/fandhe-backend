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
  --baseline-tests <file>  起点コミットの `cargo nextest run --workspace --all-features --profile ci` 出力ログ（リグレッション突合用。省略時は突合をスキップし判定に PENDING 注記を付与）
EOF
}

WORKTREE=""
TASK_ID=""
BASELINE_TESTS=""

while [ $# -gt 0 ]; do
    case "$1" in
        --worktree)
            if [ $# -lt 2 ]; then
                echo "[PENDING] --worktree には値が必要です" >&2
                usage
                exit 2
            fi
            WORKTREE="$2"
            shift 2
            ;;
        --task-id)
            if [ $# -lt 2 ]; then
                echo "[PENDING] --task-id には値が必要です" >&2
                usage
                exit 2
            fi
            TASK_ID="$2"
            shift 2
            ;;
        --baseline-tests)
            if [ $# -lt 2 ]; then
                echo "[PENDING] --baseline-tests には値が必要です" >&2
                usage
                exit 2
            fi
            BASELINE_TESTS="$2"
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

# テストランナーは `.github/workflows/ci.yml` の test ジョブ（TASK-11.4、#36）と同一の
# cargo-nextest（.config/nextest.toml の profile: ci、テスト単位 60 秒 slow-timeout /
# 120 秒強制終了）を使う。素の `cargo test` は stable ツールチェーンでテスト単位の
# タイムアウトを持てず、1 件のハングが全体を覆い隠す（レビュー指摘、Issue #85）。
# 未導入はこのハーネス側の前提未充足であり worktree の欠陥ではないため PENDING とする。
if ! command -v cargo-nextest >/dev/null 2>&1; then
    echo "[PENDING] タスク ${TASK_ID}: cargo-nextest が見つかりません（cargo install --locked cargo-nextest@0.9.137 で導入してください）"
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
TEST_LOG="$(mktemp)"
TEST_RC_FILE="$(mktemp)"

overall_fail=0
pending_note=""

# 失敗時（overall_fail=1）は GATE_LOG / TEST_LOG を残し、評価者が FAIL メッセージが
# 指すログを事後に復元できるようにする（レビュー指摘、Issue #85）。TEST_RC_FILE は
# 単なる内部の終了コード受け渡しでしかなく、失敗理由の復元には使わないため常に削除する。
cleanup() {
    rm -f "${TEST_RC_FILE}"
    if [ "${overall_fail}" -eq 0 ]; then
        rm -f "${GATE_LOG}" "${TEST_LOG}"
    fi
}
trap cleanup EXIT

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

    # cargo-nextest はテスト失敗があると非 0 で終了するが、ここでは即座に FAIL 扱いに
    # しない（後続のリグレッション突合に判定を委ねるため）。タイムアウト（124、
    # ハング型退行、PoC-9 BUG-3 の教訓）だけはこの場で確定 FAIL とする。
    timeout "${TIMEOUT_SECONDS}" cargo nextest run --workspace --all-features --profile ci >"${TEST_LOG}" 2>&1
    test_rc=$?
    echo "${test_rc}" >"${TEST_RC_FILE}"
    if [ "${test_rc}" -eq 124 ]; then
        echo "[FAIL] cargo nextest run --workspace --all-features --profile ci（タイムアウト。ログ: ${TEST_LOG}）"
        exit 1
    fi

    # nextest は doc test を実行しないため、`.github/workflows/ci.yml` と同型に
    # `cargo test --doc` を別ステップで補う（.claude/rules/coding-rust.md の doc test 必須方針）。
    if ! run_gate "cargo test --doc --workspace --all-features" cargo test --doc --workspace --all-features; then
        exit 1
    fi
    exit 0
)
gate_rc=$?

if [ "${gate_rc}" -ne 0 ]; then
    overall_fail=1
fi

test_rc="$(cat "${TEST_RC_FILE}" 2>/dev/null || true)"
baseline_usable=0
if [ -n "${BASELINE_TESTS}" ] && [ -f "${BASELINE_TESTS}" ]; then
    baseline_usable=1
fi

# ベースライン突合が使えない（--baseline-tests 未指定、またはログファイル不在）場合は
# 個々のテスト失敗をそのまま FAIL とする（fail-closed。突合できないなら楽観判定しない）。
if [ "${overall_fail}" -eq 0 ] && [ -n "${test_rc}" ] && [ "${test_rc}" -ne 0 ] && [ "${baseline_usable}" -eq 0 ]; then
    echo "[FAIL] cargo nextest run --workspace --all-features --profile ci（終了コード ${test_rc}。ログ: ${TEST_LOG}）"
    overall_fail=1
fi

# リグレッション突合: ベースラインのテスト結果ログが与えられている場合のみ実施する。
# 与えられない場合は機械ゲートの PASS/FAIL 判定はそのまま有効とし、リグレッション欄を
# PENDING として区別する（FAIL とは混同しない）。
#
# 失敗テスト名の抽出は cargo-nextest の実出力形式に合わせる（cargo test 標準出力の
# `test <name> ... FAILED` とは異なり、nextest は `<状態> [<経過時間>] (<n>/<m>) <crate>
# <test名>` の行頭に `FAIL` / `TIMEOUT` を出す）。旧実装は `^FAILED ` で grep しており、
# nextest（TASK-11.4 で採用済み）・cargo test のどちらの出力にも一致せず、かつこのブロック
# 自体が `overall_fail -eq 0`（＝失敗テスト集合が必ず空になる構造）でしかリグレッション突合が
# 発生しない設計だったため、実質的にリグレッション検出が機能していなかった（レビュー指摘、
# Issue #85）。上のブロックで即時 FAIL を baseline 未指定時のみに限定したことで、baseline
# 指定時は実際に失敗テスト集合を突合できる。
if [ "${overall_fail}" -eq 0 ]; then
    if [ -n "${BASELINE_TESTS}" ]; then
        if [ ! -f "${BASELINE_TESTS}" ]; then
            pending_note="ベースラインログが見つかりません: ${BASELINE_TESTS}"
        else
            extract_failed_tests() {
                grep -E '^[[:space:]]*(FAIL|TIMEOUT) \[' "$1" 2>/dev/null \
                    | sed -E 's/^[[:space:]]*(FAIL|TIMEOUT) \[[^]]*\][[:space:]]*(\([0-9 ]+\/[0-9 ]+\)[[:space:]]*)?//' \
                    | sort -u
            }
            baseline_failed="$(extract_failed_tests "${BASELINE_TESTS}" || true)"
            current_failed="$(extract_failed_tests "${TEST_LOG}" || true)"
            # nextest が非 0 終了したにもかかわらず FAIL/TIMEOUT 行を 1 件も検出できない
            # 場合、ビルド失敗や `#[cfg(test)]` コードの破損など FAIL/TIMEOUT 行を出さない
            # 種類のエラーである可能性が高い。この場合 new_failed は空集合になり得るため、
            # baseline 突合に判定を委ねず即座に FAIL とする（レビュー指摘、Issue #85。
            # 完遂率判定を楽観方向に歪めないというプロトコルの方針に従う）。
            if [ -n "${test_rc}" ] && [ "${test_rc}" -ne 0 ] && [ -z "${current_failed}" ]; then
                echo "[FAIL] cargo nextest run --workspace --all-features --profile ci（終了コード ${test_rc}）だが FAIL/TIMEOUT 行を検出できませんでした。ビルド失敗等の非テスト起因のエラーの可能性があります。ログ: ${TEST_LOG}"
                overall_fail=1
            else
                new_failed="$(comm -13 <(printf '%s\n' "${baseline_failed}") <(printf '%s\n' "${current_failed}") 2>/dev/null | grep -v '^$' || true)"
                if [ -n "${new_failed}" ]; then
                    echo "[FAIL] リグレッション検出: 起点コミットで PASS していたテストが失敗しています"
                    printf '%s\n' "${new_failed}"
                    overall_fail=1
                else
                    echo "[PASS] リグレッションなし（起点コミットとの突合）"
                fi
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
