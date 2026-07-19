#!/usr/bin/env bash
# TASK-10.6（#90）: 非同期 writer（`tracing_appender::non_blocking`、既定 lossy=true）の
# 高負荷時ログ欠落率を負荷段階（イベント総数 × 送出スレッド数）別に複数回計測し、
# 中央値を算出するスクリプト。
#
# このスクリプトの役割:
#   `crates/plugin-tracing/examples/backpressure_probe.rs`（計測プローブ、既定構成
#   lossy=true・buffered_lines_limit 既定値のまま高負荷送出して欠落率を JSON で返す）を
#   負荷段階ごとに RUNS 回実行し、`benches/lib/common.sh` の `median` ヘルパーで
#   欠落率・実効イベントレートの中央値を求める。PoC-10 実測（約 23 万イベント/秒
#   〔115,612 RPS × 2 イベント〕、`docs/spec/03-poc/observability-tracing/README.md`）を
#   跨ぐ範囲の負荷段階を既定とする。
#
# 前提:
#   cargo build --release -p fandhe-backend-plugin-tracing --example backpressure_probe
#   （本スクリプトはビルドを自動実行しない。benches/lib/common.sh の
#    「サプライチェーン考慮・前提ツールを自動取得しない」方針を踏襲）
#
# 呼び出し元: 人間が `bash benches/tracing-backpressure-bench.sh` として直接実行する。
# 結果は `benches/reports/task-10.6-tracing-backpressure.md` へ転記する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

# lib/common.sh の RUNS/DURATION/CONNECTIONS は本スクリプトでは未使用の項目を含むため、
# RUNS のみ踏襲し最低回数チェックも共有する（webrtc-nfr6-bench.sh と同じ理由で、
# ソート済み source 後の `:-` フォールバックはここでは問題にならない。RUNS は
# common.sh 側で export 済みの値をそのまま使う）。
check_runs_minimum

PROBE_BIN="${WORKSPACE_ROOT}/target/release/examples/backpressure_probe"
if [ ! -x "${PROBE_BIN}" ]; then
    echo "エラー: ${PROBE_BIN} が見つかりません。先に" >&2
    echo "  cargo build --release -p fandhe-backend-plugin-tracing --example backpressure_probe" >&2
    echo "を実行してください" >&2
    exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "エラー: jq が見つかりません。導入してください（例: apt install jq）" >&2
    exit 1
fi

# 負荷段階（イベント総数:送出スレッド数）。PoC-10 実測（約 23 万イベント/秒）を跨ぐ範囲。
# 環境変数 STAGES（空白区切り "events:threads" のリスト）で上書き可能（動作確認の短縮用）。
STAGES_DEFAULT="100000:1 1000000:1 1000000:4 5000000:4 5000000:8"
read -r -a STAGES <<<"${STAGES:-${STAGES_DEFAULT}}"

LINE_BYTES="${LINE_BYTES:-64}"

TMP_DIR="$(mktemp -d)"
# シークレット・一時生成物をリポジトリに残さない（.claude/rules/security.md）。
# trap は EXIT のみで十分（本スクリプトはサーバプロセスを起動しないため
# stop_server 相当の後始末は不要）。
trap 'rm -rf "${TMP_DIR}"' EXIT

echo "=== TASK-10.6 バックプレッシャー・ログ欠落率計測（RUNS=${RUNS} LINE_BYTES=${LINE_BYTES}） ===" >&2
echo "" >&2

# 機械可読出力。ステージごとの結果を JSON 配列として蓄積する。
results_json="[]"

for stage in "${STAGES[@]}"; do
    events="${stage%%:*}"
    threads="${stage##*:}"
    echo "--- ステージ: events=${events} threads=${threads} ---" >&2

    drop_rates=()
    events_per_sec_values=()
    dropped_values=()

    for ((i = 1; i <= RUNS; i++)); do
        out_file="${TMP_DIR}/probe-${events}-${threads}-${i}.log"
        json="$(FANDHE_BACKEND_TRACING_PROBE_OUTPUT="${out_file}" \
            FANDHE_BACKEND_TRACING_PROBE_EVENTS="${events}" \
            FANDHE_BACKEND_TRACING_PROBE_THREADS="${threads}" \
            FANDHE_BACKEND_TRACING_PROBE_LINE_BYTES="${LINE_BYTES}" \
            "${PROBE_BIN}")"
        rm -f "${out_file}"

        drop_rate="$(echo "${json}" | jq -r '.drop_rate_pct')"
        eps="$(echo "${json}" | jq -r '.events_per_sec')"
        dropped="$(echo "${json}" | jq -r '.dropped_lines')"

        drop_rates+=("${drop_rate}")
        events_per_sec_values+=("${eps}")
        dropped_values+=("${dropped}")
        echo "  run ${i}: drop_rate_pct=${drop_rate} events_per_sec=${eps} dropped_lines=${dropped}" >&2
    done

    drop_rate_median="$(printf '%s\n' "${drop_rates[@]}" | median)"
    eps_median="$(printf '%s\n' "${events_per_sec_values[@]}" | median)"
    dropped_median="$(printf '%s\n' "${dropped_values[@]}" | median)"

    echo "  中央値: drop_rate_pct=${drop_rate_median} events_per_sec=${eps_median} dropped_lines=${dropped_median}" >&2
    echo "" >&2

    stage_json="$(jq -n \
        --argjson events "${events}" \
        --argjson threads "${threads}" \
        --argjson drop_rate_pct_median "${drop_rate_median}" \
        --argjson events_per_sec_median "${eps_median}" \
        --argjson dropped_lines_median "${dropped_median}" \
        --argjson drop_rate_pct_runs "$(printf '%s\n' "${drop_rates[@]}" | to_json_array)" \
        '{events: $events, threads: $threads, drop_rate_pct_median: $drop_rate_pct_median, events_per_sec_median: $events_per_sec_median, dropped_lines_median: $dropped_lines_median, drop_rate_pct_runs: $drop_rate_pct_runs}')"
    results_json="$(echo "${results_json}" | jq --argjson stage "${stage_json}" '. + [$stage]')"
done

echo "=== 結果一覧（中央値、負荷段階別） ===" >&2
echo "${results_json}" | jq -r '.[] | "events=\(.events) threads=\(.threads) drop_rate_pct_median=\(.drop_rate_pct_median) events_per_sec_median=\(.events_per_sec_median)"' >&2

write_result_json "${results_json}"

# stdout には機械可読な JSON をそのまま出す（レポート転記の自動化用）。
echo "${results_json}"
