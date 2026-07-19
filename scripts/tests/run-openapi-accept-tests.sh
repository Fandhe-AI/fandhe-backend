#!/usr/bin/env bash
# openapi-accept.sh のセルフテスト（TASK-3.3、#32）。
#
# `scripts/accept/openapi-accept.sh` はネットワーク・cargo ビルド・`openapi-spec-validator`
# の有無に依存するため、本スクリプトは判定ロジックの部分（CI ジョブ存在確認の grep
# パターン・`lib/common.sh` の PASS/FAIL/SKIP 集計と終了コードの対応）を fixture・直接呼び出し
# で切り出して検証する。`run-pay-for-what-you-use-tests.sh` 等と同じくネットワーク・cargo
# ビルドに依存せず完結させる。
#
# 検証範囲外（本スクリプトが担わないもの）:
#   - openapi-accept.sh 全体の実行結果そのもの（cargo test 実行・validator 呼び出しを
#     含むため、CI・人間によるローカル実行で確認する）
#   - openapi-spec-validator 自体の判定精度（ツール側の責務）
#
# 呼び出し元: `.github/workflows/ci.yml` の unsafe-triage ジョブから既存セルフテスト群と
# 同列で呼ばれる想定（追加は別途 CI 変更で行う）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/openapi-accept"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

PASS_COUNT=0
FAIL_COUNT=0

pass() {
    echo "PASS: $1"
    PASS_COUNT=$((PASS_COUNT + 1))
}

fail() {
    echo "FAIL: $1" >&2
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

# openapi-accept.sh 節 5 が使う grep パターンと同一条件を fixture に対して適用する
# （ジョブ名・スクリプト呼び出しの両方が存在して初めて PASS 相当と判定する2条件 AND）。
ci_job_check() {
    local file="$1"
    grep -q "openapi-two-stage:" "${file}" && grep -q "scripts/openapi-two-stage.sh" "${file}"
}

echo "===== CI 2 段階ビルドジョブ存在確認（節 5）のロジック検証 ====="

if ci_job_check "${FIXTURES_DIR}/ci-with-job.yml"; then
    pass "openapi-two-stage ジョブ + スクリプト呼び出しを含む fixture は PASS 相当と判定される"
else
    fail "openapi-two-stage ジョブ + スクリプト呼び出しを含む fixture が PASS 相当と判定されなかった"
fi

if ! ci_job_check "${FIXTURES_DIR}/ci-without-job.yml"; then
    pass "openapi-two-stage ジョブを含まない fixture は FAIL 相当と判定される"
else
    fail "openapi-two-stage ジョブを含まない fixture が誤って PASS 相当と判定された"
fi

echo ""
echo "===== 実リポジトリの ci.yml に対する節 5 ロジックの疎通確認 ====="
if ci_job_check "${WORKSPACE_ROOT}/.github/workflows/ci.yml"; then
    pass "実リポジトリの .github/workflows/ci.yml は openapi-two-stage ジョブを含む（TASK-3.2 実装済みの回帰検知）"
else
    fail "実リポジトリの .github/workflows/ci.yml から openapi-two-stage ジョブが検出できない（退行の可能性）"
fi

echo ""
echo "===== 性能レポート判定行チェック（節 4、#259）のロジック検証 ====="

# openapi-accept.sh 節 4 が使う grep パターンと同一条件を fixture に対して適用する。
# 判定 3 値（pass/skip/fail）を返す。PASS/BLOCKED とも見出し行アンカーの厳密一致
# であり、本文中の恒久的な "BLOCKED" 文字列には誤ヒットしない（フェイルクローズ:
# 判定行の欠落・変質は skip でなく fail）。
perf_verdict_check() {
    local file="$1"
    if [ ! -f "${file}" ]; then
        echo "fail"
    elif grep -q "^### 判定結果（再計測、#259）: PASS" "${file}"; then
        echo "pass"
    elif grep -q "^### 判定結果（再計測、#259）: BLOCKED" "${file}"; then
        echo "skip"
    else
        echo "fail"
    fi
}

if [ "$(perf_verdict_check "${FIXTURES_DIR}/perf-pass-with-blocked-note.md")" = "pass" ]; then
    pass "本文に BLOCKED 文字列（履歴注記）があっても判定行が PASS なら PASS 相当と判定される"
else
    fail "本文の BLOCKED 文字列に誤ヒットし、PASS 判定行のある fixture が PASS 相当と判定されなかった"
fi

if [ "$(perf_verdict_check "${FIXTURES_DIR}/perf-blocked-line.md")" = "skip" ]; then
    pass "判定行が BLOCKED の fixture は SKIP 相当と判定される"
else
    fail "判定行が BLOCKED の fixture が SKIP 相当と判定されなかった"
fi

if [ "$(perf_verdict_check "${FIXTURES_DIR}/perf-no-verdict.md")" = "fail" ]; then
    pass "判定行を持たない fixture（本文に BLOCKED 語のみ）は FAIL 相当と判定される（フェイルクローズ）"
else
    fail "判定行を持たない fixture が FAIL 相当と判定されなかった（SKIP への丸め込みの疑い）"
fi

if [ "$(perf_verdict_check "${FIXTURES_DIR}/no-such-file.md")" = "fail" ]; then
    pass "レポート不在は FAIL 相当と判定される（フェイルクローズ）"
else
    fail "レポート不在が FAIL 相当と判定されなかった"
fi

echo ""
echo "===== 実リポジトリの性能レポートに対する節 4 ロジックの疎通確認 ====="
if [ "$(perf_verdict_check "${WORKSPACE_ROOT}/benches/reports/task-3.3-openapi-performance.md")" = "pass" ]; then
    pass "実リポジトリの task-3.3-openapi-performance.md は PASS 判定行を含む（#259 確定判定の回帰検知）"
else
    fail "実リポジトリの task-3.3-openapi-performance.md から PASS 判定行が検出できない（退行の可能性）"
fi

echo ""
echo "===== 節 2b（実装との突合）の機械検証コマンド存在確認 ====="
# 節 2b は cargo test の実行を伴うため fixture 化せず、openapi-accept.sh が
# example テスト（crates/core/examples/openapi_endpoints.rs、15 テスト）を
# 呼び出し続けていることを回帰検知する（節 5 の ci.yml 疎通確認と同型）。
if grep -q -- "cargo test -p fandhe-backend-core --example openapi_endpoints" \
    "${WORKSPACE_ROOT}/scripts/accept/openapi-accept.sh"; then
    pass "openapi-accept.sh 節 2b が openapi_endpoints example テストを呼び出す（#259 更新の回帰検知）"
else
    fail "openapi-accept.sh から openapi_endpoints example テストの呼び出しが検出できない（退行の可能性）"
fi

echo ""
echo "===== lib/common.sh の PASS/FAIL/SKIP 集計と終了コードの対応検証 ====="

# サブシェルで lib/common.sh を source し、record_* の組み合わせごとに
# summary_exit_code() が正しい終了コードを返すことを検証する（openapi-accept.sh の
# 「SKIP は判定不能の安全側記録であり非 0 終了させない」という設計方針そのものを固定化する）。
check_exit_code() {
    local desc="$1"
    local expected="$2"
    shift 2
    local actual
    actual="$(
        # shellcheck source=../accept/lib/common.sh
        source "${WORKSPACE_ROOT}/scripts/accept/lib/common.sh" >/dev/null
        for entry in "$@"; do
            "record_${entry%%:*}" "criterion" "${entry#*:}" >/dev/null
        done
        summary_exit_code
    )"
    if [ "${actual}" -eq "${expected}" ]; then
        pass "${desc}（exit code: ${actual}）"
    else
        fail "${desc}（期待 exit code: ${expected}, 実際: ${actual}）"
    fi
}

check_exit_code "PASS のみ → exit 0" 0 "pass:ok"
check_exit_code "SKIP のみ → exit 0（判定不能を非 0 にしない）" 0 "skip:blocked"
check_exit_code "PASS + SKIP 混在 → exit 0" 0 "pass:ok" "skip:blocked"
check_exit_code "FAIL を含む → exit 1" 1 "pass:ok" "skip:blocked" "fail:ng"

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
