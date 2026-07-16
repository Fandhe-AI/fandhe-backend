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
