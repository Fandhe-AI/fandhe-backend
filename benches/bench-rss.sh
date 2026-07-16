#!/usr/bin/env bash
# 負荷時 RSS を「試行内複数サンプル × 複数試行の中央値」で計測する（TASK-1.2）。
#
# PoC-2（docs/spec/03-poc/fullscratch-performance）の README には
# 「負荷時 RSS は各実装 1 回のみの単発計測」という環境制約が明記されている。
# 本スクリプトはこれを是正し、1 回の負荷印加中に複数回 RSS をサンプリングして
# その中央値を「当該試行の負荷時 RSS」とし、これをさらに RUNS 回繰り返して
# 試行間の中央値で評価する（PoC-2 の外れ値事例を踏まえ、平均値ではなく中央値を採用）。
#
# 使い方・パラメータは benches/README.md を参照。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

# RSS サンプリング間隔（秒）。1 試行あたりのサンプル数は DURATION から自動算出する。
SAMPLE_INTERVAL_SEC="${SAMPLE_INTERVAL_SEC:-1}"

check_dependencies
check_runs_minimum

trap stop_server EXIT
start_server
wait_for_health >/dev/null

# アイドル RSS（負荷印加前の基準値。ウォームアップとして 1 秒待機し安定させる）。
sleep 1
idle_rss_kb="$(ps -o rss= -p "${SERVER_PID}" | tr -d ' ')"

trial_medians=()

for ((trial = 1; trial <= RUNS; trial++)); do
    # 負荷印加をバックグラウンドで開始し、印加中に RSS を複数回サンプリングする。
    # oha の出力は読まないため、固定・予測可能なパスではなく mktemp で
    # 衝突・symlink 追従のないファイル名を都度生成する（CWE-377/59 対策）。
    load_output_json="$(mktemp)"
    oha -z "${DURATION}" -c "${CONNECTIONS}" --no-tui --output-format json "${TARGET_URL}/health" \
        >"${load_output_json}" 2>&1 &
    load_pid="$!"

    samples=()
    while kill -0 "${load_pid}" 2>/dev/null; do
        rss="$(ps -o rss= -p "${SERVER_PID}" 2>/dev/null | tr -d ' ' || true)"
        if [ -n "${rss}" ]; then
            samples+=("${rss}")
        fi
        sleep "${SAMPLE_INTERVAL_SEC}"
    done
    wait "${load_pid}" 2>/dev/null || true
    rm -f "${load_output_json}"

    if [ "${#samples[@]}" -eq 0 ]; then
        echo "エラー: 試行 ${trial} で RSS サンプルを取得できませんでした" >&2
        exit 1
    fi

    trial_median="$(printf '%s\n' "${samples[@]}" | median)"
    trial_medians+=("${trial_median}")
    echo "試行 ${trial}: サンプル数=${#samples[@]} raw=[${samples[*]}] 中央値=${trial_median}KB"
done

overall_median="$(printf '%s\n' "${trial_medians[@]}" | median)"

echo
echo "# bench-rss.sh 結果（RUNS=${RUNS} DURATION=${DURATION} CONNECTIONS=${CONNECTIONS}）"
echo "アイドル RSS: ${idle_rss_kb}KB"
echo "試行別中央値: ${trial_medians[*]}"
echo "負荷時 RSS（試行間中央値）: ${overall_median}KB"
