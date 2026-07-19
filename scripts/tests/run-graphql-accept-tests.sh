#!/usr/bin/env bash
# scripts/accept/graphql-accept.sh の判定ロジックのオフライン・セルフテスト
# （TASK-5.2 / #53）。
#
# `graphql-accept.sh` 本体は cargo tree・cargo test・oha 実行等の副作用を持つため
# 直接 source せず、`evaluate_nfr6_ratio`（`scripts/accept/lib/nfr6-ratio.sh`、
# `scripts/tests/run-webrtc-accept-tests.sh` が既にオフライン検証済みのためここでは
# 重複させない）以外の graphql-accept.sh 固有ロジック（依存除外 grep パイプライン・
# unsafe grep パイプライン）を、cargo・ネットワーク非依存の擬似データ・フィクスチャで
# 回帰検証する（`scripts/tests/run-webrtc-accept-tests.sh` と同じ方針）。
#
# 呼び出し元: 人間 / CI が `bash scripts/tests/run-graphql-accept-tests.sh` として
# 直接実行する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/graphql-accept"

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

assert_eq() {
    local desc="$1" expected="$2" actual="$3"
    if [ "${expected}" = "${actual}" ]; then
        pass "${desc}"
    else
        fail "${desc}（期待: '${expected}'、実際: '${actual}'）"
    fi
}

echo "===== 基準A: 依存除外 grep パイプライン（graphql-accept.sh check_dep_exclusion と同一パターン） ====="
# graphql-accept.sh check_dep_exclusion と同一の grep パイプライン
# （async-graphql|fandhe-backend-plugin-graphql）を擬似 cargo tree 出力へ適用し、
# 陽性/陰性対照の両方を検証する（`webrtc-accept.sh` の check_dep_exclusion の
# webrtc grep と同種のロジック）。
count_graphql_deps() {
    printf '%s\n' "$1" | grep -c -E 'async-graphql|fandhe-backend-plugin-graphql' || true
}

no_deps_tree="fandhe-backend-core v0.1.0
fandhe-backend-http v0.1.0
fandhe-backend-routes v0.1.0
tokio v1.53.0"
assert_eq "graphql 無効相当ツリーは 0 件" "0" "$(count_graphql_deps "${no_deps_tree}")"

with_deps_tree="fandhe-backend-core v0.1.0
fandhe-backend-plugin-graphql v0.1.0
async-graphql v7.2.1
async-graphql-parser v7.2.1
async-graphql-value v7.2.1"
assert_eq "graphql 有効相当ツリーは 4 件（陽性対照）" "4" "$(count_graphql_deps "${with_deps_tree}")"

echo "===== 基準A: cargo tree 失敗時のフェイルクローズ（webrtc-accept.sh と同一パターン） ====="
# graphql-accept.sh check_dep_exclusion と同一の判定パターン（cargo tree の終了コードを
# 明示確認し、失敗時は測定不能として FAIL を返す）を検証する。実 cargo は使わず、
# 成功/失敗をシミュレートするダミー関数の出力をパイプラインへ渡す
# （`scripts/tests/run-webrtc-accept-tests.sh` の evaluate_dep_exclusion と同型）。
evaluate_dep_exclusion() {
    # $1: cargo tree 相当の標準出力
    # $2: cargo tree 相当の終了コード（0=成功、非 0=失敗）
    local tree_output="$1" exit_code="$2" disabled_count
    if [ "${exit_code}" -ne 0 ]; then
        echo "FAIL"
        return
    fi
    disabled_count="$(count_graphql_deps "${tree_output}")"
    if [ "${disabled_count}" -eq 0 ]; then
        echo "PASS"
    else
        echo "FAIL"
    fi
}

assert_eq "cargo tree 失敗時（stdout 空・非 0 終了）は FAIL" "FAIL" "$(evaluate_dep_exclusion "" 1)"
assert_eq "cargo tree 成功・graphql 依存 0 件は PASS" "PASS" "$(evaluate_dep_exclusion "${no_deps_tree}" 0)"
assert_eq "cargo tree 成功・graphql 依存残留は FAIL" "FAIL" "$(evaluate_dep_exclusion "${with_deps_tree}" 0)"

echo "===== 基準A': unsafe grep ロジック（webrtc-accept.sh check_unsafe と同一パターン） ====="
# graphql-accept.sh の check_unsafe と同一の grep パイプラインをフィクスチャへ適用し、
# SAFETY コメント誤検出・複数行検出のリグレッションを防ぐ
# （`scripts/tests/run-webrtc-accept-tests.sh` の同名テストと同型。フィクスチャは
# plugin-graphql 向けに本テスト専用ディレクトリを持つ）。
check_unsafe_fixture() {
    local dir="$1"
    grep -rn --include='*.rs' -E '\bunsafe\b' "${dir}" | grep -v -E '^[^:]*:[0-9]+:[[:space:]]*//' || true
}

clean_hits="$(check_unsafe_fixture "${FIXTURES_DIR}/clean")"
if [ -z "${clean_hits}" ]; then
    pass "clean フィクスチャは unsafe 0 件と判定"
else
    fail "clean フィクスチャで誤検出: ${clean_hits}"
fi

unsafe_hits="$(check_unsafe_fixture "${FIXTURES_DIR}/with-unsafe")"
if [ -n "${unsafe_hits}" ]; then
    pass "with-unsafe フィクスチャは unsafe 検出あり"
else
    fail "with-unsafe フィクスチャで見逃し（unsafe 使用を検知できず）"
fi

comment_only_hits="$(check_unsafe_fixture "${FIXTURES_DIR}/comment-only")"
if [ -z "${comment_only_hits}" ]; then
    pass "comment-only フィクスチャ（// SAFETY コメントのみ）は誤検出なし"
else
    fail "comment-only フィクスチャで誤検出（コメント中の unsafe 字句を実コードと誤認）: ${comment_only_hits}"
fi

echo "===== 基準B: live 疎通確認の応答判定ロジック（graphql-accept.sh check_min_connectivity と同一パターン） ====="
# curl 応答文字列に対する grep 判定（"hello":"world" の有無）が期待どおり動くことを
# 実 HTTP 通信なしで検証する。
assert_response_ok() {
    local response="$1"
    if echo "${response}" | grep -q '"hello":"world"'; then
        echo "PASS"
    else
        echo "FAIL"
    fi
}
assert_eq "data.hello == world を含む応答は PASS" "PASS" "$(assert_response_ok '{"data":{"hello":"world"}}')"
assert_eq "errors のみの応答（実行失敗相当）は FAIL" "FAIL" "$(assert_response_ok '{"errors":[{"message":"boom"}]}')"
assert_eq "空応答（サーバ無応答相当）は FAIL" "FAIL" "$(assert_response_ok '')"

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
exit 0
