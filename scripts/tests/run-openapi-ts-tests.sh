#!/usr/bin/env bash
# scripts/openapi-ts.sh のセルフテスト（TASK-6.1、#54）。
#
# openapi-ts.sh 本体はネットワーク（npm レジストリ）・cargo ビルド・npm の存在に依存する
# ため、run-openapi-accept-tests.sh・run-pay-for-what-you-use-tests.sh 等と同じく、本
# スクリプトは判定ロジックの部分（引数検証・diff による鮮度判定・ツール不在時の
# fail-closed 挙動・CI ジョブ存在確認）を fixture・直接呼び出しで切り出して検証する。
#
# 検証範囲外（本スクリプトが担わないもの）:
#   - openapi-ts.sh 全体の実行結果そのもの（gen-openapi 実行・npm ci・openapi-typescript
#     呼び出し・tsc 型検査を含むため、CI・人間によるローカル実行で確認する）
#   - openapi-typescript / tsc 自体の判定精度（ツール側の責務）
#
# 呼び出し元: .github/workflows/ci.yml の unsafe-triage ジョブから既存セルフテスト群と
# 同列で呼ばれる想定。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/openapi-ts"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
OPENAPI_TS_SH="${WORKSPACE_ROOT}/scripts/openapi-ts.sh"

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

echo "===== 引数検証 ====="

# 未知引数は非 0 終了する（openapi-two-stage.sh と同一パターン）。
set +e
bash "${OPENAPI_TS_SH}" --bogus-flag >/dev/null 2>&1
actual=$?
set -e
if [ "${actual}" -eq 2 ]; then
    pass "未知引数 --bogus-flag は exit 2 で拒否される"
else
    fail "未知引数 --bogus-flag が exit 2 で拒否されなかった（実際: ${actual}）"
fi

# -h/--help は使い方を表示して exit 0 する（node/npm・cargo・ネットワーク不要のはず）。
set +e
help_output="$(bash "${OPENAPI_TS_SH}" -h 2>&1)"
actual=$?
set -e
if [ "${actual}" -eq 0 ]; then
    pass "-h は exit 0 で終了する"
else
    fail "-h が exit 0 で終了しなかった（実際: ${actual}）"
fi
if printf '%s' "${help_output}" | grep -qF -- "--update"; then
    pass "-h の出力に --update の説明が含まれる"
else
    fail "-h の出力に --update の説明が含まれない"
fi

echo ""
echo "===== node/npm 不在時の fail-closed 挙動 ====="

# PATH から node/npm を含むディレクトリのみを取り除いた PATH で実行し、自動ダウンロード
# せず非 0 終了 + 導入コマンド案内を行うことを確認する（dirname 等の基本コマンドは
# 残す。前提ツールを自動ダウンロードしない既存規約）。
node_dir="$(dirname "$(command -v node)")"
filtered_path="$(printf '%s' "${PATH}" | tr ':' '\n' | grep -vF -- "${node_dir}" | paste -sd: -)"
set +e
no_tool_output="$(PATH="${filtered_path}" bash "${OPENAPI_TS_SH}" 2>&1)"
actual=$?
set -e
if [ "${actual}" -ne 0 ]; then
    pass "node/npm が PATH にない場合は非 0 終了する（fail-closed）"
else
    fail "node/npm が PATH にない場合でも exit 0 になった（fail-closed 違反）"
fi
if printf '%s' "${no_tool_output}" | grep -qF -- "volta install"; then
    pass "node/npm 不在時に導入コマンド（volta install）を案内する"
else
    fail "node/npm 不在時に導入コマンドの案内が出力されない"
fi

echo ""
echo "===== schema.d.ts 鮮度判定（diff ロジック）====="

# openapi-ts.sh の --check モードは「一時ディレクトリへ再生成 → コミット済み
# schema.d.ts と diff」で乖離検知する。ここでは diff 自体の判定条件（一致/不一致）を
# fixture で固定化する（openapi-typescript の生成結果そのものは対象外）。
if diff -u "${FIXTURES_DIR}/schema-a.d.ts" "${FIXTURES_DIR}/schema-b-identical.d.ts" >/dev/null; then
    pass "一致する schema.d.ts ペアは diff 差分なし（--check 相当で PASS）と判定される"
else
    fail "一致する schema.d.ts ペアが誤って差分ありと判定された"
fi

if ! diff -u "${FIXTURES_DIR}/schema-a.d.ts" "${FIXTURES_DIR}/schema-c-drifted.d.ts" >/dev/null; then
    pass "乖離した schema.d.ts ペアは diff 差分あり（--check 相当で FAIL）と判定される"
else
    fail "乖離した schema.d.ts ペアが誤って差分なしと判定された"
fi

echo ""
echo "===== CI openapi-ts ジョブ存在確認のロジック検証 ====="

ci_job_check() {
    local file="$1"
    grep -q "openapi-ts:" "${file}" && grep -q "scripts/openapi-ts.sh" "${file}"
}

if ci_job_check "${FIXTURES_DIR}/ci-with-job.yml"; then
    pass "openapi-ts ジョブ + スクリプト呼び出しを含む fixture は PASS 相当と判定される"
else
    fail "openapi-ts ジョブ + スクリプト呼び出しを含む fixture が PASS 相当と判定されなかった"
fi

if ! ci_job_check "${FIXTURES_DIR}/ci-without-job.yml"; then
    pass "openapi-ts ジョブを含まない fixture は FAIL 相当と判定される"
else
    fail "openapi-ts ジョブを含まない fixture が誤って PASS 相当と判定された"
fi

echo ""
echo "===== 実リポジトリの ci.yml・ts/ に対する疎通確認 ====="

if ci_job_check "${WORKSPACE_ROOT}/.github/workflows/ci.yml"; then
    pass "実リポジトリの .github/workflows/ci.yml は openapi-ts ジョブを含む（TASK-6.1 実装済みの回帰検知）"
else
    fail "実リポジトリの .github/workflows/ci.yml から openapi-ts ジョブが検出できない（退行の可能性）"
fi

if [ -f "${WORKSPACE_ROOT}/ts/src/generated/schema.d.ts" ]; then
    pass "ts/src/generated/schema.d.ts がコミット対象として存在する"
else
    fail "ts/src/generated/schema.d.ts が存在しない（TASK-6.1 生成物の欠落）"
fi

if [ -f "${WORKSPACE_ROOT}/ts/package-lock.json" ]; then
    pass "ts/package-lock.json がコミット対象として存在する（npm ci の単一真実源）"
else
    fail "ts/package-lock.json が存在しない（サプライチェーン対策の欠落）"
fi

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
