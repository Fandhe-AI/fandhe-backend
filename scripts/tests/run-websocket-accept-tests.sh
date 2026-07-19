#!/usr/bin/env bash
# scripts/accept/websocket-accept.sh の判定ロジックのオフライン・セルフテスト
# （TASK-4.4 / #25）。
#
# `websocket-accept.sh` 本体は cargo tree・cargo test・bench-ws-load.sh・oha 実行等の
# 副作用を持つため直接 source せず、`evaluate_nfr6_ratio`（`scripts/accept/lib/nfr6-ratio.sh`、
# `scripts/tests/run-webrtc-accept-tests.sh` が既にオフライン検証済みのためここでは
# 重複させない）以外の websocket-accept.sh 固有ロジック（依存除外 grep パイプライン・
# unsafe grep パイプライン・レイテンシ RESULT_JSON 検証ロジック）を、
# cargo・ネットワーク非依存の擬似データ・フィクスチャで回帰検証する
# （`scripts/tests/run-graphql-accept-tests.sh` と同じ方針）。
#
# 呼び出し元: 人間 / CI が `bash scripts/tests/run-websocket-accept-tests.sh` として
# 直接実行する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/websocket-accept"

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

echo "===== 基準A: 依存除外 grep パイプライン（websocket-accept.sh check_dep_exclusion と同一パターン） ====="
# websocket-accept.sh check_dep_exclusion と同一の grep パイプライン
# （tokio-tungstenite|tungstenite|fandhe-backend-plugin-websocket）を擬似 cargo tree 出力へ
# 適用し、陽性/陰性対照の両方を検証する（`graphql-accept.sh` の
# check_dep_exclusion の graphql grep と同種のロジック）。
count_ws_deps() {
    printf '%s\n' "$1" | grep -c -E 'tokio-tungstenite|tungstenite|fandhe-backend-plugin-websocket' || true
}

no_deps_tree="fandhe-backend-core v0.1.0
fandhe-backend-http v0.1.0
fandhe-backend-routes v0.1.0
tokio v1.53.0"
assert_eq "websocket 無効相当ツリーは 0 件" "0" "$(count_ws_deps "${no_deps_tree}")"

with_deps_tree="fandhe-backend-core v0.1.0
fandhe-backend-plugin-websocket v0.1.0
tokio-tungstenite v0.24.0
tungstenite v0.24.0"
assert_eq "websocket 有効相当ツリーは 3 件（陽性対照）" "3" "$(count_ws_deps "${with_deps_tree}")"

echo "===== 基準A: cargo tree 失敗時のフェイルクローズ（webrtc-accept.sh と同一パターン） ====="
# websocket-accept.sh check_dep_exclusion と同一の判定パターン（cargo tree の
# 終了コードを明示確認し、失敗時は測定不能として FAIL を返す）を検証する。実 cargo は
# 使わず、成功/失敗をシミュレートするダミー関数の出力をパイプラインへ渡す
# （`scripts/tests/run-graphql-accept-tests.sh` の evaluate_dep_exclusion と同型）。
evaluate_dep_exclusion() {
    # $1: cargo tree 相当の標準出力
    # $2: cargo tree 相当の終了コード（0=成功、非 0=失敗）
    local tree_output="$1" exit_code="$2" disabled_count
    if [ "${exit_code}" -ne 0 ]; then
        echo "FAIL"
        return
    fi
    disabled_count="$(count_ws_deps "${tree_output}")"
    if [ "${disabled_count}" -eq 0 ]; then
        echo "PASS"
    else
        echo "FAIL"
    fi
}

assert_eq "cargo tree 失敗時（stdout 空・非 0 終了）は FAIL" "FAIL" "$(evaluate_dep_exclusion "" 1)"
assert_eq "cargo tree 成功・websocket 依存 0 件は PASS" "PASS" "$(evaluate_dep_exclusion "${no_deps_tree}" 0)"
assert_eq "cargo tree 成功・websocket 依存残留は FAIL" "FAIL" "$(evaluate_dep_exclusion "${with_deps_tree}" 0)"

echo "===== 基準A': unsafe grep ロジック（webrtc-accept.sh check_unsafe と同一パターン） ====="
# websocket-accept.sh の check_unsafe と同一の grep パイプラインをフィクスチャへ適用し、
# SAFETY コメント誤検出・複数行検出のリグレッションを防ぐ
# （`scripts/tests/run-graphql-accept-tests.sh` の同名テストと同型。フィクスチャは
# plugin-websocket 向けに本テスト専用ディレクトリを持つ）。
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

echo "===== 基準C: レイテンシ計測 RESULT_JSON 検証ロジック（websocket-accept.sh check_latency と同一パターン） ====="
# websocket-accept.sh check_latency は bench-ws-load.sh の RESULT_JSON から
# matrix の件数・劣化率フィールドの有無を検証する。実 jq 呼び出しを含めて
# 判定パターンを検証する（cargo・ネットワーク非依存、jq のみに依存）。
if ! command -v jq >/dev/null 2>&1; then
    echo "警告: jq 未導入のため基準C のセルフテストをスキップします" >&2
else
    evaluate_latency_json() {
        # $1: RESULT_JSON 相当のファイルパス
        local result_json="$1" matrix_len fs_degradation
        if ! matrix_len="$(jq -r '.matrix | length' "${result_json}" 2>/dev/null)" || [ "${matrix_len}" = "null" ]; then
            echo "FAIL(no-matrix)"
            return
        fi
        fs_degradation="$(jq -r '.heartbeat_rtt_p95_degradation.heartbeat_rtt_p95_degradation_pct.fullscratch' "${result_json}" 2>/dev/null || echo "null")"
        if [ "${matrix_len}" -eq 0 ] || [ "${fs_degradation}" = "null" ]; then
            echo "FAIL(empty-or-no-degradation)"
            return
        fi
        echo "PASS"
    }

    valid_json="$(mktemp)"
    cat >"${valid_json}" <<'EOF'
{
  "runs": 3,
  "hold_secs": 30,
  "matrix": [
    {"impl": "fullscratch", "connections": 1000, "heartbeat_rtt_us": {"p50": 400, "p95": 1200, "p99": 1500, "max": 2000}}
  ],
  "heartbeat_rtt_p95_degradation": {
    "min_connections": 1000,
    "max_connections": 10000,
    "heartbeat_rtt_p95_degradation_pct": {"fullscratch": 105.2, "axum": 108.1}
  }
}
EOF
    assert_eq "matrix・劣化率を含む正常な RESULT_JSON は PASS" "PASS" "$(evaluate_latency_json "${valid_json}")"

    empty_matrix_json="$(mktemp)"
    cat >"${empty_matrix_json}" <<'EOF'
{"runs": 3, "hold_secs": 30, "matrix": []}
EOF
    assert_eq "matrix が空の RESULT_JSON は FAIL" "FAIL(empty-or-no-degradation)" "$(evaluate_latency_json "${empty_matrix_json}")"

    no_matrix_json="$(mktemp)"
    cat >"${no_matrix_json}" <<'EOF'
{"runs": 3, "hold_secs": 30}
EOF
    # jq の `null | length` は 0 を返す（エラーにならない）ため、matrix フィールド
    # 欠落は「matrix 長さ 0」と同じ経路（empty-or-no-degradation）で FAIL になる
    # （websocket-accept.sh check_latency と同一の実挙動。フィールド欠落を専用の
    # エラーメッセージで区別したい場合は `has("matrix")` 等の追加検査が必要だが、
    # 現状は「FAIL であること」自体を保証できていれば受け入れ判定としては十分）。
    assert_eq "matrix フィールドを欠く RESULT_JSON は FAIL" "FAIL(empty-or-no-degradation)" "$(evaluate_latency_json "${no_matrix_json}")"

    rm -f "${valid_json}" "${empty_matrix_json}" "${no_matrix_json}"
fi

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
exit 0
