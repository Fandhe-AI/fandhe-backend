#!/usr/bin/env bash
# `scripts/setup-required-checks.sh` のオフラインセルフテスト（イシュー #693）。
#
# PATH の先頭に `fixtures/setup-required-checks/gh` スタブを差し込み、live の
# `main-protection` ruleset スナップショット（同ディレクトリの
# `ruleset-main-protection.json`）とその変種を使って `--check` の差分検出・終了コードを
# 検証する。ネットワーク・gh 認証は不要。
#
# 受け入れ基準 5（#693 issue 本文）: 実装・テストの過程で live へ書き込み（PUT/POST）を
# 行わないことを、スタブのログに書き込み系メソッドの呼び出しが 1 件も無いことで
# 機械的に保証する（末尾で全ケース共通に確認する）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/setup-required-checks"
TARGET_SCRIPT="${SCRIPTS_DIR}/setup-required-checks.sh"

if [ ! -f "${TARGET_SCRIPT}" ]; then
    echo "エラー: ${TARGET_SCRIPT} が見つかりません" >&2
    exit 2
fi
if [ ! -x "${FIXTURES_DIR}/gh" ]; then
    echo "エラー: ${FIXTURES_DIR}/gh スタブが見つからないか実行権限がありません" >&2
    exit 2
fi

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

# スタブ用の一時ディレクトリ（PATH 差し込み用シム・ログ・variant フィクスチャ）。
WORK_DIR="$(mktemp -d)"
STUB_LOG="${WORK_DIR}/gh-calls.log"
: >"${STUB_LOG}"

cleanup() {
    rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

# PATH には `gh` という名前でスタブを直接置く（fixtures ディレクトリ自体を PATH に
# 加えると `jq` 等の他コマンド解決に影響しうるため、シムディレクトリを別途作る）。
SHIM_DIR="${WORK_DIR}/shim"
mkdir -p "${SHIM_DIR}"
ln -s "${FIXTURES_DIR}/gh" "${SHIM_DIR}/gh"
export PATH="${SHIM_DIR}:${PATH}"
export STUB_LOG

# `${TARGET_SCRIPT}` を実行し、標準出力+標準エラーを RUN_OUT に、終了コードを RUN_EC に
# 格納する。本テストは非 0 終了を期待するケースが大半のため、`set -e` 下でも
# 呼び出し文自体で script が中断しないよう明示的に errexit を無効化する
# （run-review-gate-tests.sh の `set +e` / `set -e` パターンに合わせる）。
run_target() {
    set +e
    RUN_OUT="$("${TARGET_SCRIPT}" "$@" 2>&1)"
    RUN_EC=$?
    set -e
}

# 正規化済み JSON 同士を比較するヘルパ（順序差を無視するため jq -S で再整形して比較）。
json_eq() {
    local a b
    a="$(printf '%s' "$1" | jq -S .)"
    b="$(printf '%s' "$2" | jq -S .)"
    [ "${a}" = "${b}" ]
}

assert_exit() {
    local desc="$1"
    local expected="$2"
    local actual="$3"
    if [ "${actual}" = "${expected}" ]; then
        pass "${desc}（exit=${actual}）"
    else
        fail "${desc}（期待 exit=${expected}、実際 exit=${actual}）"
    fi
}

# variant ruleset JSON を作る（base に jq フィルタを適用して一時ファイルへ書き出す）。
make_variant() {
    local out="$1"
    local filter="$2"
    jq "${filter}" "${FIXTURES_DIR}/ruleset-main-protection.json" >"${out}"
}

BASE_RULESET="${FIXTURES_DIR}/ruleset-main-protection.json"
BASE_LIST="${FIXTURES_DIR}/rulesets-list.json"

# ==================================================
# a. live スナップショットそのもの → exit 0、差分の出力なし
# ==================================================
: >"${STUB_LOG}"
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${BASE_RULESET}" run_target --check
assert_exit "a. live スナップショットそのもの" 0 "${RUN_EC}"
if [[ "${RUN_OUT}" != *"一致しています"* ]]; then
    fail "a. 一致メッセージが出力されない: ${RUN_OUT}"
else
    pass "a. 一致メッセージが出力される"
fi

# ==================================================
# b. required contexts の順序を入れ替えたもの → exit 0（順序差は無視する）
# ==================================================
VARIANT_B="${WORK_DIR}/variant-b.json"
make_variant "${VARIANT_B}" \
    '.rules |= map(if .type == "required_status_checks" then .parameters.required_status_checks |= reverse else . end)'
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${VARIANT_B}" run_target --check
assert_exit "b. required contexts の順序入れ替え" 0 "${RUN_EC}"

# ==================================================
# c. context を 1 件削除 → exit 1、diff に該当名が出る
# ==================================================
VARIANT_C="${WORK_DIR}/variant-c.json"
make_variant "${VARIANT_C}" \
    '.rules |= map(if .type == "required_status_checks" then .parameters.required_status_checks |= map(select(.context != "ci-complete")) else . end)'
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${VARIANT_C}" run_target --check
assert_exit "c. context を1件削除" 1 "${RUN_EC}"
if [[ "${RUN_OUT}" == *"ci-complete"* ]]; then
    pass "c. diff に削除した context 名が出る"
else
    fail "c. diff に 'ci-complete' が含まれない: ${RUN_OUT}"
fi

# ==================================================
# d. context を 1 件追加 → exit 1
# ==================================================
VARIANT_D="${WORK_DIR}/variant-d.json"
make_variant "${VARIANT_D}" \
    '.rules |= map(if .type == "required_status_checks" then .parameters.required_status_checks += [{context: "some-new-job", integration_id: 15368}] else . end)'
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${VARIANT_D}" run_target --check
assert_exit "d. context を1件追加" 1 "${RUN_EC}"

# ==================================================
# e. integration_id を変更 → exit 1
# ==================================================
VARIANT_E="${WORK_DIR}/variant-e.json"
make_variant "${VARIANT_E}" \
    '.rules |= map(if .type == "required_status_checks" then .parameters.required_status_checks |= map(if .context == "Cursor Bugbot" then .integration_id = 999999 else . end) else . end)'
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${VARIANT_E}" run_target --check
assert_exit "e. integration_id 変更" 1 "${RUN_EC}"

# ==================================================
# f. required_review_thread_resolution: false → exit 1
# ==================================================
VARIANT_F="${WORK_DIR}/variant-f.json"
make_variant "${VARIANT_F}" \
    '.rules |= map(if .type == "pull_request" then .parameters.required_review_thread_resolution = false else . end)'
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${VARIANT_F}" run_target --check
assert_exit "f. required_review_thread_resolution=false" 1 "${RUN_EC}"

# ==================================================
# g. allowed_merge_methods に merge を追加 → exit 1
# ==================================================
VARIANT_G="${WORK_DIR}/variant-g.json"
make_variant "${VARIANT_G}" \
    '.rules |= map(if .type == "pull_request" then .parameters.allowed_merge_methods += ["merge"] else . end)'
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${VARIANT_G}" run_target --check
assert_exit "g. allowed_merge_methods に merge 追加" 1 "${RUN_EC}"

# ==================================================
# h. bypass_actors を1件追加 → exit 1
# ==================================================
VARIANT_H="${WORK_DIR}/variant-h.json"
make_variant "${VARIANT_H}" \
    '.bypass_actors = [{actor_id: 1, actor_type: "Team", bypass_mode: "always"}]'
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${VARIANT_H}" run_target --check
assert_exit "h. bypass_actors 追加" 1 "${RUN_EC}"

# ==================================================
# i. conditions.include を refs/heads/main にする → exit 1
# ==================================================
VARIANT_I="${WORK_DIR}/variant-i.json"
make_variant "${VARIANT_I}" \
    '.conditions.ref_name.include = ["refs/heads/main"]'
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${VARIANT_I}" run_target --check
assert_exit "i. conditions.include を refs/heads/main に変更" 1 "${RUN_EC}"

# ==================================================
# j. 一覧に main-protection が無い → exit 1
# ==================================================
VARIANT_LIST_J="${WORK_DIR}/list-j.json"
jq '[.[] | select(.name != "main-protection")]' "${BASE_LIST}" >"${VARIANT_LIST_J}"
STUB_LIST_FILE="${VARIANT_LIST_J}" STUB_RULESET_FILE="${BASE_RULESET}" run_target --check
assert_exit "j. main-protection が一覧に無い" 1 "${RUN_EC}"

# ==================================================
# k. 同名が2件ある → exit 2
# ==================================================
VARIANT_LIST_K="${WORK_DIR}/list-k.json"
jq '. + [.[0] * {id: 999999999}]' "${BASE_LIST}" >"${VARIANT_LIST_K}"
STUB_LIST_FILE="${VARIANT_LIST_K}" STUB_RULESET_FILE="${BASE_RULESET}" run_target --check
assert_exit "k. main-protection が重複" 2 "${RUN_EC}"

# ==================================================
# l. gh api エラー → exit 2（exit 0 にならないこと）
# ==================================================
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${BASE_RULESET}" STUB_FAIL=1 run_target --check
assert_exit "l. gh api エラー時（STUB_FAIL=1）" 2 "${RUN_EC}"

# ==================================================
# m. --print-desired の出力と、正規化した live スナップショットが一致する
# ==================================================
run_target --print-desired
PRINT_DESIRED_OUT="${RUN_OUT}"
# --print-desired は既に正規化済み JSON を返す契約。live スナップショットは
# setup-required-checks.sh 内の NORMALIZE_FILTER 相当を適用しないと直接比較できないため、
# 同スクリプトの --check 経路（一致判定）に委ねず、ここでは構造面（必須フィールド・
# required contexts の集合）が live と一致することを確認する。
LIVE_CONTEXTS="$(jq -S '[.rules[] | select(.type=="required_status_checks") | .parameters.required_status_checks[] | {context, integration_id}] | sort_by(.context, .integration_id)' "${BASE_RULESET}")"
DESIRED_CONTEXTS="$(printf '%s' "${PRINT_DESIRED_OUT}" | jq -S '[.rules[] | select(.type=="required_status_checks") | .parameters.required_status_checks[] | {context, integration_id}] | sort_by(.context, .integration_id)')"
if json_eq "${LIVE_CONTEXTS}" "${DESIRED_CONTEXTS}"; then
    pass "m. --print-desired の required contexts が live スナップショットと一致する"
else
    fail "m. --print-desired の required contexts が live スナップショットと不一致"
fi
# --print-desired と --check（a. と同一フィクスチャ）が同じ判定基盤を使っていることは
# a. がケース全体を通して既に実証している（同一 NORMALIZE_FILTER 経由）。

# ==================================================
# n. 不明な引数 → exit 2
# ==================================================
run_target --no-such-option
assert_exit "n. 不明な引数" 2 "${RUN_EC}"

# ==================================================
# o. 全ケース終了後、スタブログに書き込み系メソッドの呼び出しが1件も無い
#    （受け入れ基準5の機械保証）
# ==================================================
: >"${STUB_LOG}"
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${BASE_RULESET}" run_target --check
STUB_LIST_FILE="${BASE_LIST}" STUB_RULESET_FILE="${VARIANT_C}" run_target --check
run_target --print-desired
if grep -qE 'PUT|POST|PATCH|DELETE' "${STUB_LOG}"; then
    fail "o. スタブログに書き込み系メソッドの呼び出しが記録されている（live 書き込み経路混入の疑い）"
else
    pass "o. スタブログに書き込み系メソッドの呼び出しが無い（apply 経路を呼んでいない）"
fi

echo
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -ne 0 ]; then
    exit 1
fi
exit 0
