#!/usr/bin/env bash
# third-party-stability-aggregate.sh のセルフテスト（TASK-12.5、#46）:
# ネットワーク・cargo ビルドに依存せず、合成 fixture（scripts/tests/fixtures/
# third-party-stability/）で集計ロジックのみを検証する。
# scripts/tests/run-third-party-feasibility-tests.sh と同じ体裁に従う。
#
# 重要（バイアス混入防止の注意）: 本テストが検証するのは「集計ハーネスの算出ロジックが
# 正しく動くこと」のみである。ここで green になっても、独立した被験 AI による実測定
# （docs/design/multi-trial-stability-verification.md）で安定性を確認したことには
# ならない。両者を混同しないこと。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/third-party-stability"
HARNESS="${SCRIPTS_DIR}/third-party-stability-aggregate.sh"

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

assert_exit_code() {
    local desc="$1"
    local expected="$2"
    local actual="$3"
    if [ "${expected}" -eq "${actual}" ]; then
        pass "${desc}（exit code: ${actual}）"
    else
        fail "${desc}（期待 exit code: ${expected}, 実際: ${actual}）"
    fi
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

if [ ! -x "${HARNESS}" ] && [ ! -f "${HARNESS}" ]; then
    echo "エラー: ハーネスが見つかりません: ${HARNESS}" >&2
    exit 2
fi

if [ ! -d "${FIXTURES_DIR}" ]; then
    echo "エラー: fixture ディレクトリが見つかりません: ${FIXTURES_DIR}" >&2
    exit 2
fi

echo "=== 引数検証 ==="

set +e
out="$(bash "${HARNESS}" 2>&1)"
code=$?
set -e
assert_exit_code "引数なしは PENDING（exit 2）" 2 "${code}"
assert_contains "引数なしの案内メッセージ" "${out}" "試行が 1 件も指定されていません"

set +e
out="$(bash "${HARNESS}" --trial "onlylabel" 2>&1)"
code=$?
set -e
assert_exit_code "--trial の label:file 形式違反は exit 2" 2 "${code}"

set +e
out="$(bash "${HARNESS}" --trial 'bad label!:x' 2>&1)"
code=$?
set -e
assert_exit_code "不正な試行ラベルは exit 2" 2 "${code}"

set +e
out="$(bash "${HARNESS}" --trial "missing:${FIXTURES_DIR}/does-not-exist.summary" 2>&1)"
code=$?
set -e
assert_exit_code "存在しないサマリファイルは非 0 exit" 1 "${code}"
assert_contains "存在しないファイルの PENDING メッセージ" "${out}" "サマリファイルが見つかりません"

echo
echo "=== 正常系（2 試行、全指標充足） ==="

set +e
out="$(bash "${HARNESS}" \
    --trial "trial1:${FIXTURES_DIR}/trial-normal-1.summary" \
    --trial "trial2:${FIXTURES_DIR}/trial-normal-2.summary" 2>&1)"
code=$?
set -e
assert_exit_code "正常系 2 試行は exit 0" 0 "${code}"
assert_contains "完遂率の試行 1 行が表に出る" "${out}" "| trial1 | 8 | 2 | 0 | 10 | 80.0% | 充足 |"
assert_contains "完遂率の試行 2 行が表に出る" "${out}" "| trial2 | 7 | 3 | 0 | 10 | 70.0% | 充足 |"
assert_contains "完遂率のレンジが算出される" "${out}" "レンジ 10.0 ポイント"
assert_contains "可否判定正解率の平均が算出される" "${out}" "可否判定正解率"
assert_contains "破壊 0 件が充足と判定される" "${out}" "| trial1 | 0 | 充足 |"

echo
echo "=== --trials-dir 経由（正常系と同一 2 試行を再現） ==="

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT
cp "${FIXTURES_DIR}/trial-normal-1.summary" "${TMP_DIR}/trial-t1.summary"
cp "${FIXTURES_DIR}/trial-normal-2.summary" "${TMP_DIR}/trial-t2.summary"

set +e
out="$(bash "${HARNESS}" --trials-dir "${TMP_DIR}" 2>&1)"
code=$?
set -e
assert_exit_code "--trials-dir 経由は exit 0" 0 "${code}"
assert_contains "--trials-dir 経由でも試行 t1 が集計される" "${out}" "| t1 |"
assert_contains "--trials-dir 経由でも試行 t2 が集計される" "${out}" "| t2 |"

set +e
out="$(bash "${HARNESS}" --trials-dir "${FIXTURES_DIR}/does-not-exist" 2>&1)"
code=$?
set -e
assert_exit_code "存在しない --trials-dir は exit 2" 2 "${code}"

echo
echo "=== 閾値未達検知（完遂率 50% < 60%） ==="

set +e
out="$(bash "${HARNESS}" --trial "miss:${FIXTURES_DIR}/trial-threshold-miss.summary" 2>&1)"
code=$?
set -e
assert_exit_code "閾値未達のみでも解析自体は成功（exit 0）" 0 "${code}"
assert_contains "閾値未達が「未充足」と明示される" "${out}" "| miss | 5 | 5 | 0 | 10 | 50.0% | 未充足 |"

echo
echo "=== 破壊検知（destruction count=2 > 0） ==="

set +e
out="$(bash "${HARNESS}" --trial "destr:${FIXTURES_DIR}/trial-destruction.summary" 2>&1)"
code=$?
set -e
assert_exit_code "破壊検知のみでも解析自体は成功（exit 0）" 0 "${code}"
assert_contains "破壊 2 件が「未充足」と明示される" "${out}" "| destr | 2 | 未充足 |"

echo
echo "=== 不正入力の fail-closed（pass+fail+pending != total） ==="

set +e
out="$(bash "${HARNESS}" --trial "bad:${FIXTURES_DIR}/trial-malformed.summary" 2>&1)"
code=$?
set -e
assert_exit_code "不正入力は非 0 exit（fail-closed）" 1 "${code}"
assert_contains "不正入力の PENDING メッセージ" "${out}" "total と一致しません"
assert_contains "不正入力の指標は PENDING 扱い" "${out}" "PENDING（本指標のデータを含む試行がありません）"

echo
echo "=== 結果 ==="
echo "PASS: ${PASS_COUNT}, FAIL: ${FAIL_COUNT}"

if [ "${FAIL_COUNT}" -ne 0 ]; then
    exit 1
fi
exit 0
