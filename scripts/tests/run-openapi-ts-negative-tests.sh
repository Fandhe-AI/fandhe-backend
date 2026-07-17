#!/usr/bin/env bash
# scripts/openapi-ts-negative.sh のセルフテスト（TASK-6.2、#55）。
#
# openapi-ts-negative.sh 本体はネットワーク（npm レジストリ）・cargo 実行と無関係だが
# npm ci・tsc・openapi-typescript の実行を伴うため、run-openapi-ts-tests.sh
# （TASK-6.1、#54）と同じく、本スクリプトは判定ロジックの部分（引数検証・node/npm
# 不在時の fail-closed 挙動・tsc 出力からのエラーコード判定・「エラーコード不一致の
# 失敗を PASS と誤認しない」discrimination・CI ステップ存在確認）を fixture・直接
# 呼び出しで切り出して検証する。
#
# 検証範囲外（本スクリプトが担わないもの）:
#   - openapi-ts-negative.sh 全体の実行結果そのもの（npm ci・tsc・openapi-typescript
#     呼び出しを含むため、CI・人間によるローカル実行で確認する）
#   - openapi-typescript / tsc 自体の判定精度（ツール側の責務）
#
# 呼び出し元: .github/workflows/ci.yml の unsafe-triage ジョブから
# run-openapi-ts-tests.sh 等の既存セルフテスト群と同列で呼ばれる想定。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/openapi-ts-negative"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
NEGATIVE_SH="${WORKSPACE_ROOT}/scripts/openapi-ts-negative.sh"

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

# scripts/openapi-ts-negative.sh 内の N1 判定ロジック（4 類型それぞれの期待エラー
# コードが該当行に存在するか）を fixture テキストに対して再現する。本体スクリプトの
# grep パターンと同一の判定を切り出したもの（本体を直接実行せず判定ロジックのみを
# テストするため、パターン文字列は本体スクリプトと二重管理になるが、CI ジョブ存在
# 確認テストと同様に本体スクリプトの実ファイルへの疎通確認（下記）で乖離を検知する）。
n1_judge() {
    local output_file="$1"
    local content
    content="$(cat "${output_file}")"

    local markers=("type-mismatch.ts(34," "type-mismatch.ts(46," "type-mismatch.ts(62," "type-mismatch.ts(73,")
    local expected=("TS2322" "TS2322" "TS2554" "TS2322")
    local i
    for i in "${!markers[@]}"; do
        local marker="${markers[$i]}"
        local code="${expected[$i]}"
        if ! printf '%s\n' "${content}" | grep -qF "${marker}"; then
            return 1
        fi
        if ! printf '%s\n' "${content}" | grep -F "${marker}" | grep -q "${code}"; then
            return 1
        fi
    done
    return 0
}

echo "===== 引数検証 ====="

set +e
bash "${NEGATIVE_SH}" --bogus-flag >/dev/null 2>&1
actual=$?
set -e
if [ "${actual}" -eq 2 ]; then
    pass "未知引数 --bogus-flag は exit 2 で拒否される"
else
    fail "未知引数 --bogus-flag が exit 2 で拒否されなかった（実際: ${actual}）"
fi

set +e
help_output="$(bash "${NEGATIVE_SH}" -h 2>&1)"
actual=$?
set -e
if [ "${actual}" -eq 0 ]; then
    pass "-h は exit 0 で終了する"
else
    fail "-h が exit 0 で終了しなかった（実際: ${actual}）"
fi
if printf '%s' "${help_output}" | grep -qF -- "陰性対照"; then
    pass "-h の出力に陰性対照の説明が含まれる"
else
    fail "-h の出力に陰性対照の説明が含まれない"
fi

echo ""
echo "===== node/npm 不在時の fail-closed 挙動 ====="

node_dir="$(dirname "$(command -v node)")"
filtered_path="$(printf '%s' "${PATH}" | tr ':' '\n' | grep -vF -- "${node_dir}" | paste -sd: -)"
set +e
no_tool_output="$(PATH="${filtered_path}" bash "${NEGATIVE_SH}" 2>&1)"
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
echo "===== N1 判定ロジック（4 類型の期待エラーコード） ====="

if n1_judge "${FIXTURES_DIR}/typecheck-negative-expected.txt"; then
    pass "4 類型すべての期待エラーコードを含む fixture は PASS 相当と判定される"
else
    fail "4 類型すべての期待エラーコードを含む fixture が PASS 相当と判定されなかった"
fi

echo ""
echo "===== N1 discrimination（誤った理由での失敗を PASS と誤認しない） ====="

# 「非 0 終了」だけを見る素朴な判定では、module 解決エラー（TS2307、tsconfig 不備等
# 型不一致とは無関係の理由）による失敗も陰性対照 PASS と誤認してしまう。ここでは
# 「非 0 終了」と「期待コードの存在」を分離して確認し、後者が discrimination を
# 担っていることを検証する。
if grep -q "error TS" "${FIXTURES_DIR}/typecheck-negative-wrong-reason.txt"; then
    pass "誤った理由の fixture も非 0 終了相当（tsc エラーを含む）である前提を満たす"
else
    fail "誤った理由の fixture の前提が崩れている（tsc エラーを含まない）"
fi
if ! n1_judge "${FIXTURES_DIR}/typecheck-negative-wrong-reason.txt"; then
    pass "誤った理由（TS2307 module 解決エラー）で失敗した fixture は N1 判定で FAIL 相当となる（PASS と誤認しない）"
else
    fail "誤った理由で失敗した fixture が誤って PASS 相当と判定された（discrimination 欠落）"
fi

if ! n1_judge "${FIXTURES_DIR}/typecheck-negative-partial-mismatch.txt"; then
    pass "4 類型中 1 類型（存在しないエンドポイント呼び出し、TS2554）が欠落した fixture は FAIL 相当となる"
else
    fail "1 類型欠落の fixture が誤って PASS 相当と判定された（部分的な回帰を検知できていない）"
fi

echo ""
echo "===== N2 判定ロジック（openapi.json 境界からの伝搬） ====="

if grep -q "error TS" "${FIXTURES_DIR}/n2-expected.txt" && grep -q "TS2322" "${FIXTURES_DIR}/n2-expected.txt"; then
    pass "N2 期待 fixture は非 0 終了相当 + TS2322 を含む"
else
    fail "N2 期待 fixture が前提を満たさない"
fi

if grep -q "error TS" "${FIXTURES_DIR}/n2-wrong-reason.txt" && ! grep -q "TS2322" "${FIXTURES_DIR}/n2-wrong-reason.txt"; then
    pass "N2 の誤った理由（TS2307 module 解決エラー）の fixture は TS2322 を含まない（discrimination 対象として妥当）"
else
    fail "N2 の誤った理由 fixture の前提が崩れている"
fi

echo ""
echo "===== CI openapi-ts / unsafe-triage ジョブへのステップ組み込み確認のロジック検証 ====="

#
# コメントやステップ名（`name:`）に "openapi-ts-negative.sh" という文字列が出現する
# だけでは配線済みとみなさない（Cursor Bugbot 指摘、PR #153 review 4724393762）。
# 実際に該当スクリプトを起動する `run:` 行（`run: bash scripts/openapi-ts-negative.sh`
# 等）が存在することを要求する。単なる文字列出現ではなく「実行行」を検証することで、
# `run:` 句からステップが削除されコメント・無関係なステップ名だけが残った回帰を検知する。
ci_negative_step_check() {
    local file="$1"
    grep -qE '^[[:space:]]*run:[[:space:]]*bash[[:space:]]+scripts/openapi-ts-negative\.sh([[:space:]]|$)' "${file}" \
        && grep -qE '^[[:space:]]*run:[[:space:]]*bash[[:space:]]+scripts/tests/run-openapi-ts-negative-tests\.sh([[:space:]]|$)' "${file}"
}

if ci_negative_step_check "${FIXTURES_DIR}/ci-with-negative-step.yml"; then
    pass "陰性対照ステップ + セルフテストステップを含む fixture は PASS 相当と判定される"
else
    fail "陰性対照ステップ + セルフテストステップを含む fixture が PASS 相当と判定されなかった"
fi

if ! ci_negative_step_check "${FIXTURES_DIR}/ci-without-negative-step.yml"; then
    pass "陰性対照ステップを含まない fixture は FAIL 相当と判定される"
else
    fail "陰性対照ステップを含まない fixture が誤って PASS 相当と判定された"
fi

if ! ci_negative_step_check "${FIXTURES_DIR}/ci-with-comment-only-negative.yml"; then
    pass "コメント・ステップ名のみに文字列が出現し実行行（run:）が存在しない fixture は FAIL 相当と判定される（Bugbot 指摘の回帰防止）"
else
    fail "コメント・ステップ名のみの fixture が誤って PASS 相当と判定された（run: 句を見ずに文字列出現のみで誤判定）"
fi

echo ""
echo "===== 実リポジトリの ci.yml・scripts/・ts/ に対する疎通確認 ====="

if ci_negative_step_check "${WORKSPACE_ROOT}/.github/workflows/ci.yml"; then
    pass "実リポジトリの .github/workflows/ci.yml は陰性対照ステップ + セルフテストステップを含む（TASK-6.2 実装済みの回帰検知）"
else
    fail "実リポジトリの .github/workflows/ci.yml から陰性対照関連ステップが検出できない（退行の可能性）"
fi

if [ -x "${NEGATIVE_SH}" ] || [ -f "${NEGATIVE_SH}" ]; then
    pass "scripts/openapi-ts-negative.sh が存在する"
else
    fail "scripts/openapi-ts-negative.sh が存在しない"
fi

if [ -f "${WORKSPACE_ROOT}/ts/src/negative/type-mismatch.ts" ]; then
    pass "ts/src/negative/type-mismatch.ts（陰性対照ファイル）が存在する"
else
    fail "ts/src/negative/type-mismatch.ts が存在しない（TASK-6.2 生成物の欠落）"
fi

if [ -f "${WORKSPACE_ROOT}/ts/tsconfig.negative.json" ]; then
    pass "ts/tsconfig.negative.json が存在する"
else
    fail "ts/tsconfig.negative.json が存在しない"
fi

if grep -q '"typecheck:negative"' "${WORKSPACE_ROOT}/ts/package.json"; then
    pass "ts/package.json に typecheck:negative スクリプトが定義されている"
else
    fail "ts/package.json に typecheck:negative スクリプトが定義されていない"
fi

if grep -q 'src/negative' "${WORKSPACE_ROOT}/ts/tsconfig.json"; then
    pass "ts/tsconfig.json が src/negative を通常 typecheck から除外している"
else
    fail "ts/tsconfig.json が src/negative を除外していない（通常 typecheck に陰性対照ファイルが混入する可能性）"
fi

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
