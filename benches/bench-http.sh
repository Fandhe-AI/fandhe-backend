#!/usr/bin/env bash
# RPS / p50 / p95 / p99 を複数回計測し、raw 値と中央値を出力する（TASK-1.2）。
#
# 対象は 4 エンドポイント（GET /health, GET /hello/{name}, GET /users/{id},
# POST /echo）。前提: `cargo build --release --bin axum-ref`
# （または TARGET_BIN で指定した release バイナリ）。
#
# 使い方・パラメータは benches/README.md を参照。

# `CPU_PROBE=1`（既定 0、opt-in）で各計測窓の直前直後に外部 CPU 占有率
# プローブ（`benches/lib/cpu-probe.sh`）を実行し、窓単位で汚染検知・有界な
# 再計測を行う（イシュー #613。背景・実証は
# `benches/reports/issue593-p1-zero-copy-bench.md` 9 節・9.7 節）。
# 未指定時（既定）は本ファイルの以下の挙動は一切変わらない
# （プローブ呼び出し自体を行わないため、CPU_PROBE 未対応の既存呼び出し元・
# `bench-accept.sh`（本イシューでは INTERLEAVE 統合のみ）に影響しない）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

CPU_PROBE="${CPU_PROBE:-0}"
if [ "${CPU_PROBE}" != "0" ] && [ "${CPU_PROBE}" != "1" ]; then
    echo "エラー: CPU_PROBE は 0 または 1 である必要があります（現在: ${CPU_PROBE}）" >&2
    exit 1
fi
if [ "${CPU_PROBE}" = "1" ]; then
    # shellcheck source=lib/cpu-probe.sh
    source "${SCRIPT_DIR}/lib/cpu-probe.sh"
fi

check_dependencies
check_runs_minimum

trap stop_server EXIT
start_server
wait_for_health >/dev/null

# 計測開始前の 3 秒ウォームアップ（JIT・キャッシュ安定化。PoC-2 と同条件）。
oha -z 3s -c "${CONNECTIONS}" --no-tui --output-format json "${TARGET_URL}/health" >/dev/null 2>&1 || true

# RESULT_JSON 出力時に各エンドポイントの JSON オブジェクトを蓄積する配列。
# 未指定時（RESULT_JSON 未設定）は使わないため、既存の stdout 専用利用に影響しない。
ENDPOINT_JSON=()

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
    # CPU_PROBE=1 時のみ意味を持つ並列配列（未指定時は空のまま、RESULT_JSON
    # 生成時に参照しない）。
    ext_cpu_pct_values=()
    contaminated_values=()
    remeasure_count_values=()

    for ((i = 1; i <= RUNS; i++)); do
        # CPU_PROBE=0（既定）のときは以下の分岐に一切入らず、プローブ呼び出しの
        # オーバーヘッド・挙動変化がゼロであることを保証する。
        remeasure_count=0
        while :; do
            if [ "${CPU_PROBE}" = "1" ]; then
                total_before_pair="$(probe_read_total_jiffies)"
                server_before="$(probe_read_pid_jiffies "${SERVER_PID}")"
                children_before="$(probe_read_self_children_jiffies)"
            fi

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

            if [ "${CPU_PROBE}" != "1" ]; then
                break
            fi

            total_after_pair="$(probe_read_total_jiffies)"
            server_after="$(probe_read_pid_jiffies "${SERVER_PID}")"
            children_after="$(probe_read_self_children_jiffies)"
            total_before="$(echo "${total_before_pair}" | cut -d' ' -f1)"
            busy_before="$(echo "${total_before_pair}" | cut -d' ' -f2)"
            total_after="$(echo "${total_after_pair}" | cut -d' ' -f1)"
            busy_after="$(echo "${total_after_pair}" | cut -d' ' -f2)"
            attributed_before=$((server_before + children_before))
            attributed_after=$((server_after + children_after))
            ext_cpu_pct="$(probe_external_share "${total_before}" "${total_after}" \
                "${busy_before}" "${busy_after}" "${attributed_before}" "${attributed_after}")"

            if probe_is_contaminated "${ext_cpu_pct}" && [ "${remeasure_count}" -lt "${WINDOW_REMEASURE_MAX}" ]; then
                remeasure_count=$((remeasure_count + 1))
                echo "  [CPU_PROBE] ${label} run ${i}: 外部占有率 ${ext_cpu_pct}% > ${EXT_CPU_MAX_PCT}%、窓を再計測します（${remeasure_count}/${WINDOW_REMEASURE_MAX}）" >&2
                continue
            fi
            break
        done

        rps_values+=("${rps}")
        p50_values+=("${p50}")
        p95_values+=("${p95}")
        p99_values+=("${p99}")

        if [ "${CPU_PROBE}" = "1" ]; then
            contaminated_flag=0
            if probe_is_contaminated "${ext_cpu_pct}"; then
                contaminated_flag=1
                echo "  [CPU_PROBE] ${label} run ${i}: 再計測上限（${WINDOW_REMEASURE_MAX}）到達。汚染フラグ付きで採用します（外部占有率 ${ext_cpu_pct}%）" >&2
            fi
            ext_cpu_pct_values+=("${ext_cpu_pct}")
            contaminated_values+=("${contaminated_flag}")
            remeasure_count_values+=("${remeasure_count}")
        fi
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
    if [ "${CPU_PROBE}" = "1" ]; then
        contaminated_count=0
        for flag in "${contaminated_values[@]}"; do
            [ "${flag}" = "1" ] && contaminated_count=$((contaminated_count + 1))
        done
        echo "CPU_PROBE 外部占有率(%): ${ext_cpu_pct_values[*]}（汚染窓 ${contaminated_count}/${RUNS}、再計測発生 ${remeasure_count_values[*]}）"
    fi
    echo

    if [ -n "${RESULT_JSON:-}" ]; then
        rps_raw_json="$(printf '%s\n' "${rps_values[@]}" | to_json_array)"
        p50_raw_json="$(printf '%s\n' "${p50_values[@]}" | to_json_array)"
        p95_raw_json="$(printf '%s\n' "${p95_values[@]}" | to_json_array)"
        p99_raw_json="$(printf '%s\n' "${p99_values[@]}" | to_json_array)"
        endpoint_obj="$(jq -n \
            --arg label "${label}" \
            --argjson rps_raw "${rps_raw_json}" --argjson rps_median "${rps_median}" \
            --argjson p50_raw "${p50_raw_json}" --argjson p50_median "${p50_median}" \
            --argjson p95_raw "${p95_raw_json}" --argjson p95_median "${p95_median}" \
            --argjson p99_raw "${p99_raw_json}" --argjson p99_median "${p99_median}" \
            '{label: $label,
              rps: {raw: $rps_raw, median: $rps_median},
              p50: {raw: $p50_raw, median: $p50_median},
              p95: {raw: $p95_raw, median: $p95_median},
              p99: {raw: $p99_raw, median: $p99_median}}')"
        # CPU_PROBE=1 のときのみ endpoint_obj へ cpu_probe フィールドを追加する
        # （既存フィールドの形・意味は一切変えない後方互換な追加。
        # `ext_cpu_pct` は "nan"（計測不能）が `jq -R tonumber` で null になる
        # ことを利用し、契約どおり値を捏造せず null として記録する）。
        if [ "${CPU_PROBE}" = "1" ]; then
            ext_cpu_pct_json="$(printf '%s\n' "${ext_cpu_pct_values[@]}" | to_json_array)"
            contaminated_json="$(printf '%s\n' "${contaminated_values[@]}" | to_json_array)"
            remeasure_count_json="$(printf '%s\n' "${remeasure_count_values[@]}" | to_json_array)"
            endpoint_obj="$(echo "${endpoint_obj}" | jq \
                --argjson ext_cpu_pct "${ext_cpu_pct_json}" \
                --argjson contaminated "${contaminated_json}" \
                --argjson remeasure_count "${remeasure_count_json}" \
                '. + {cpu_probe: {ext_cpu_pct: $ext_cpu_pct, contaminated: $contaminated, remeasure_count: $remeasure_count}}')"
        fi
        ENDPOINT_JSON+=("${endpoint_obj}")
    fi
done

# 機械可読出力（RESULT_JSON 指定時のみ）。bench-accept.sh（TASK-1.6-1）が
# 比較・閾値判定のために stdout テキストをパースせずに済むよう分離する。
if [ -n "${RESULT_JSON:-}" ]; then
    endpoints_array="$(printf '%s\n' "${ENDPOINT_JSON[@]}" | jq -s '.')"
    result_json="$(jq -n \
        --argjson runs "${RUNS}" --arg duration "${DURATION}" --argjson connections "${CONNECTIONS}" \
        --argjson endpoints "${endpoints_array}" \
        '{runs: $runs, duration: $duration, connections: $connections, endpoints: $endpoints}')"
    write_result_json "${result_json}"
fi
