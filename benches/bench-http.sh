#!/usr/bin/env bash
# RPS / p50 / p95 / p99 を複数回計測し、raw 値と中央値を出力する（TASK-1.2）。
#
# 対象は 4 エンドポイント（GET /health, GET /hello/{name}, GET /users/{id},
# POST /echo）。前提: `cargo build --release --bin axum-ref`
# （または TARGET_BIN で指定した release バイナリ）。
#
# 使い方・パラメータは benches/README.md を参照。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

check_dependencies
check_runs_minimum

trap stop_server EXIT
start_server
wait_for_health >/dev/null

# 計測開始前の 3 秒ウォームアップ（JIT・キャッシュ安定化。PoC-2 と同条件）。
oha -z 3s -c "${CONNECTIONS}" --no-tui --output-format json "${TARGET_URL}/health" >/dev/null 2>&1 || true

# ラベル・oha への追加引数を並列配列で保持する（文字列分割による誤パースを避ける）。
LABELS=(
    "GET /health"
    "GET /hello/{name}"
    "GET /users/{id}"
    "POST /echo"
)
URLS=(
    "${TARGET_URL}/health"
    "${TARGET_URL}/hello/world"
    "${TARGET_URL}/users/42"
    "${TARGET_URL}/echo"
)
# POST /echo のみ追加引数（メソッド・content-type・body）が必要
EXTRA_ARGS_ECHO=(-m POST -T "application/json" -d '{"message":"bench"}')

echo "# bench-http.sh 結果（RUNS=${RUNS} DURATION=${DURATION} CONNECTIONS=${CONNECTIONS}）"
echo

for idx in "${!LABELS[@]}"; do
    label="${LABELS[${idx}]}"
    url="${URLS[${idx}]}"

    rps_values=()
    p50_values=()
    p95_values=()
    p99_values=()

    for ((i = 1; i <= RUNS; i++)); do
        if [ "${label}" = "POST /echo" ]; then
            json="$(oha -z "${DURATION}" -c "${CONNECTIONS}" --no-tui --output-format json \
                "${EXTRA_ARGS_ECHO[@]}" "${url}")"
        else
            json="$(oha -z "${DURATION}" -c "${CONNECTIONS}" --no-tui --output-format json "${url}")"
        fi
        rps="$(echo "${json}" | jq -r '.summary.requestsPerSec')"
        p50="$(echo "${json}" | jq -r '.latencyPercentiles.p50')"
        p95="$(echo "${json}" | jq -r '.latencyPercentiles.p95')"
        p99="$(echo "${json}" | jq -r '.latencyPercentiles.p99')"
        rps_values+=("${rps}")
        p50_values+=("${p50}")
        p95_values+=("${p95}")
        p99_values+=("${p99}")
    done

    rps_median="$(printf '%s\n' "${rps_values[@]}" | median)"
    p50_median="$(printf '%s\n' "${p50_values[@]}" | median)"
    p95_median="$(printf '%s\n' "${p95_values[@]}" | median)"
    p99_median="$(printf '%s\n' "${p99_values[@]}" | median)"

    echo "## ${label}"
    echo "raw RPS: ${rps_values[*]}"
    echo "raw p50: ${p50_values[*]}"
    echo "raw p95: ${p95_values[*]}"
    echo "raw p99: ${p99_values[*]}"
    echo "median  RPS=${rps_median} p50=${p50_median}s p95=${p95_median}s p99=${p99_median}s"
    echo
done
