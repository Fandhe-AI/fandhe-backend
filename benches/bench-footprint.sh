#!/usr/bin/env bash
# 起動時間・アイドル RSS・リリースバイナリサイズを複数回計測し、中央値を出力する（TASK-1.2）。
#
# 起動時間は「プロセス起動 → /health 初回応答成功」までを 5ms 間隔ポーリングで計測する。
# バイナリサイズは決定的な値のため RUNS 回の計測は行わず 1 回のみ記録する。
#
# 使い方・パラメータは benches/README.md を参照。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

check_dependencies
check_runs_minimum

if [ ! -x "${TARGET_BIN}" ]; then
    echo "エラー: ${TARGET_BIN} が見つかりません。先に 'cargo build --release' を実行してください" >&2
    exit 1
fi

# 試行間で必ずサーバプロセスを回収する（中断時のプロセス残留防止）。
trap stop_server EXIT

startup_ms_values=()
idle_rss_values=()

for ((trial = 1; trial <= RUNS; trial++)); do
    start_server

    startup_ms="$(wait_for_health 5000)"
    startup_ms_values+=("${startup_ms}")

    # 起動直後は RSS が安定していない可能性があるため 500ms 待ってからサンプリングする。
    sleep 0.5
    idle_rss="$(ps -o rss= -p "${SERVER_PID}" | tr -d ' ')"
    idle_rss_values+=("${idle_rss}")

    stop_server
done

startup_median="$(printf '%s\n' "${startup_ms_values[@]}" | median)"
rss_median="$(printf '%s\n' "${idle_rss_values[@]}" | median)"
binary_size_bytes="$(stat -c '%s' "${TARGET_BIN}" 2>/dev/null || stat -f '%z' "${TARGET_BIN}")"

echo "# bench-footprint.sh 結果（RUNS=${RUNS}）"
echo "raw 起動時間(ms): ${startup_ms_values[*]}"
echo "raw アイドル RSS(KB): ${idle_rss_values[*]}"
echo "中央値 起動時間: ${startup_median}ms"
echo "中央値 アイドル RSS: ${rss_median}KB"
echo "バイナリサイズ: ${binary_size_bytes} bytes（${TARGET_BIN}）"

# 機械可読出力（RESULT_JSON 指定時のみ）。bench-accept.sh が起動時間絶対差・
# アイドル RSS 比率・バイナリサイズ比率を判定する際の入力として使う。
if [ -n "${RESULT_JSON:-}" ]; then
    startup_raw_json="$(printf '%s\n' "${startup_ms_values[@]}" | to_json_array)"
    rss_raw_json="$(printf '%s\n' "${idle_rss_values[@]}" | to_json_array)"
    result_json="$(jq -n \
        --argjson runs "${RUNS}" \
        --argjson startup_raw "${startup_raw_json}" --argjson startup_median "${startup_median}" \
        --argjson rss_raw "${rss_raw_json}" --argjson rss_median "${rss_median}" \
        --argjson binary_size_bytes "${binary_size_bytes}" \
        --arg target_bin "${TARGET_BIN}" \
        '{runs: $runs,
          startup_ms: {raw: $startup_raw, median: $startup_median},
          idle_rss_kb: {raw: $rss_raw, median: $rss_median},
          binary_size_bytes: $binary_size_bytes, target_bin: $target_bin}')"
    write_result_json "${result_json}"
fi
