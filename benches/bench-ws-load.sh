#!/usr/bin/env bash
# TASK-4.3（#24）10,000 同時 WebSocket 接続負荷試験・RSS 再計測ハーネス。
#
# このスクリプトの役割:
#   fullscratch（`crates/core/examples/ws_echo.rs`）と axum-ref（`ws` feature 有効）の
#   2 実装へ `crates/ws-load-client` で同一の WebSocket 負荷（接続数 1,000/5,000/10,000）
#   を掛け、保持期間中のサーバ RSS を継続サンプリングして「接続あたり RSS 増分」を
#   算出・比較する。docs/spec/03-poc/high-concurrency-scale（PoC-7）で確認された
#   axum 比 155.2%（TASK-4.2 最適化後、未実測）を正式に再計測し、REQ 基準
#   （axum 比 110% 以内・確立成功率 99% 以上・1k→10k の線形性）を判定する。
#
# `benches/bench-rss.sh`（試行内複数サンプル×複数試行の中央値評価）と同じ計測
# 思想を踏襲するが、対象が HTTP（oha）ではなく WebSocket 長時間接続（専用クライアント）
# である点が異なるため独立スクリプトとする（`benches/lib/common.sh` の
# `start_server`/`check_dependencies` は oha・単一 TARGET_BIN を前提にしており、
# 本スクリプトの「2 実装 × 3 接続数 × RUNS 試行」構成には合わないため、
# `median`/`to_json_array`/`write_result_json`/`validate_numeric` のみ再利用する）。
#
# 前提（自動ビルドしない、サプライチェーン考慮 .claude/rules/security.md）:
#   cargo build --release -p backend-framework-core --features websocket --example ws_echo
#   cargo build --release -p axum-ref --features ws --target-dir target/ws-bench
#   cargo build --release -p ws-load-client
#
# 呼び出し元: 人間が `bash benches/bench-ws-load.sh` として直接実行する。
# 結果は `benches/reports/task-4.3-ws-load-rss.md` へ転記する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=lib/common.sh
# median/to_json_array/write_result_json/validate_numeric のみ利用する
# （`RUNS`/`DURATION`/`CONNECTIONS`/`TARGET_*` の既定値・`check_dependencies`・
# `start_server` は本スクリプト独自の構成のため使わない）。
source "${SCRIPT_DIR}/lib/common.sh"

# --- 計測パラメータ（env で上書き可能） ---------------------------------
RUNS="${RUNS:-3}"
HOLD_SECS="${HOLD_SECS:-60}"
RAMP_BATCH="${RAMP_BATCH:-200}"
RAMP_DELAY_MS="${RAMP_DELAY_MS:-50}"
HEARTBEAT_MS="${HEARTBEAT_MS:-2000}"
SAMPLE_INTERVAL_SEC="${SAMPLE_INTERVAL_SEC:-1}"
# 接続数マトリクス（空白区切り）。既定は受け入れ基準どおり 1,000/5,000/10,000。
# 動作確認用にスモーク（例: CONNECTION_TIERS="100"）へ縮小できる。
CONNECTION_TIERS="${CONNECTION_TIERS:-1000 5000 10000}"
# 判定基準（受け入れ条件、docs/spec/05-tasks.md TASK-4.3）。
SUCCESS_RATE_MIN_PCT="${SUCCESS_RATE_MIN_PCT:-99}"
AXUM_RATIO_MAX_PCT="${AXUM_RATIO_MAX_PCT:-110}"

FULLSCRATCH_BIN="${FULLSCRATCH_BIN:-${WORKSPACE_ROOT}/target/release/examples/ws_echo}"
AXUM_BIN="${AXUM_BIN:-${WORKSPACE_ROOT}/target/ws-bench/release/axum-ref}"
CLIENT_BIN="${CLIENT_BIN:-${WORKSPACE_ROOT}/target/release/ws-load-client}"

FULLSCRATCH_HOST="${FULLSCRATCH_HOST:-127.0.0.1}"
FULLSCRATCH_PORT="${FULLSCRATCH_PORT:-3007}"
AXUM_HOST="${AXUM_HOST:-127.0.0.1}"
AXUM_PORT="${AXUM_PORT:-3008}"

if [ "${RUNS}" -lt 3 ]; then
    echo "エラー: RUNS は最低 3 回必要です（現在: ${RUNS}）。中央値評価の前提を満たせません" >&2
    exit 1
fi

# --- 前提ツール・環境検査 -----------------------------------------------
check_prereqs() {
    local missing=0
    if ! command -v jq >/dev/null 2>&1; then
        echo "エラー: jq が見つかりません。導入してください（例: apt install jq）" >&2
        missing=1
    fi
    if ! command -v curl >/dev/null 2>&1; then
        echo "エラー: curl が見つかりません。導入してください（例: apt install curl）" >&2
        missing=1
    fi
    for bin_path in "${FULLSCRATCH_BIN}" "${AXUM_BIN}" "${CLIENT_BIN}"; do
        if [ ! -x "${bin_path}" ]; then
            echo "エラー: ${bin_path} が見つかりません。先に本スクリプト冒頭の前提コマンドでビルドしてください" >&2
            missing=1
        fi
    done
    if [ "${missing}" -ne 0 ]; then
        exit 1
    fi
}

# ulimit -n（オープンファイルディスクリプタ数上限）が最大接続数を賄えるか検査する。
# WebSocket 1 接続はクライアント・サーバ双方で 1 fd を消費するため、安全側に
# 「最大接続数 × 2 + 余裕 100」を最小要求値とする（PoC-7 の環境制約の再発防止）。
check_ulimit() {
    local max_conn="$1"
    local required=$((max_conn * 2 + 100))
    local current
    current="$(ulimit -n)"
    if [ "${current}" != "unlimited" ] && [ "${current}" -lt "${required}" ]; then
        echo "エラー: ulimit -n（${current}）が不足しています（必要: ${required} 以上）。" >&2
        echo "        'ulimit -n ${required}' を実行してから再実行してください" >&2
        exit 1
    fi
}

# クライアント側エフェメラルポート範囲が最大接続数を賄えるか検査する
# （ループバック対向の負荷試験ではクライアント側エフェメラルポートが実質上限になる、
# PoC-7 の環境制約）。Linux 以外・取得不能な場合は判定不能として警告のみに留める
# （フェイルクローズより「計測自体を止めない」ことを優先する非致命的前提検査）。
check_ephemeral_port_range() {
    local max_conn="$1"
    local range_file="/proc/sys/net/ipv4/ip_local_port_range"
    if [ ! -r "${range_file}" ]; then
        echo "警告: ${range_file} を読み取れません（Linux 以外の可能性）。エフェメラルポート枯渇の事前検査をスキップします" >&2
        return 0
    fi
    local lo hi span required
    read -r lo hi <"${range_file}"
    span=$((hi - lo))
    required=$((max_conn + 1000))
    if [ "${span}" -lt "${required}" ]; then
        echo "エラー: エフェメラルポート範囲（${lo}-${hi}、幅 ${span}）が不足しています（必要: ${required} 以上）。" >&2
        echo "        範囲を広げてから再実行してください（例: echo '10000 65535' | sudo tee ${range_file}）" >&2
        exit 1
    fi
}

max_tier() {
    # shellcheck disable=SC2086  # CONNECTION_TIERS は意図的な空白区切りリストの単語分割
    printf '%s\n' ${CONNECTION_TIERS} | sort -n | tail -1
}

# --- サーバプロセス管理 --------------------------------------------------
SERVER_PID=""

stop_server() {
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    SERVER_PID=""
}
trap stop_server EXIT

wait_for_health_at() {
    local url="$1" timeout_ms="${2:-5000}"
    local elapsed_ms=0 interval_ms=5
    while [ "${elapsed_ms}" -lt "${timeout_ms}" ]; do
        if curl -s -o /dev/null -w '%{http_code}' "${url}/health" 2>/dev/null | grep -q '^200$'; then
            return 0
        fi
        sleep "$(LC_NUMERIC=C awk "BEGIN { print ${interval_ms} / 1000 }")"
        elapsed_ms=$((elapsed_ms + interval_ms))
    done
    echo "エラー: ${url}/health が ${timeout_ms}ms 以内に応答しませんでした" >&2
    return 1
}

# 1 実装・1 接続数・1 試行を計測する。
# 引数: $1 impl 名（fullscratch|axum）、$2 接続数
# 標準出力: "idle_rss_kb load_rss_kb connected success_rate_pct" の 1 行
run_single_trial() {
    local impl="$1" conn="$2"
    local bind_addr result_json

    if [ "${impl}" = "fullscratch" ]; then
        bind_addr="${FULLSCRATCH_HOST}:${FULLSCRATCH_PORT}"
        # 監視用ヘルスチェック接続の余裕を見込み、目標接続数 + 100 を上限にする
        # （crates/core/examples/ws_echo.rs の MAX_CONNECTIONS env）。
        MAX_CONNECTIONS=$((conn + 100)) BIND_ADDR="${bind_addr}" "${FULLSCRATCH_BIN}" \
            >/dev/null 2>&1 &
        SERVER_PID="$!"
    else
        bind_addr="${AXUM_HOST}:${AXUM_PORT}"
        BIND_ADDR="${bind_addr}" "${AXUM_BIN}" >/dev/null 2>&1 &
        SERVER_PID="$!"
    fi

    wait_for_health_at "http://${bind_addr}" 5000

    # アイドル RSS（負荷印加前の基準値。1 秒待機して安定させる、bench-rss.sh と同方針）。
    sleep 1
    local idle_rss_kb
    idle_rss_kb="$(ps -o rss= -p "${SERVER_PID}" | tr -d ' ')"

    result_json="$(mktemp)"
    TARGET_URL="ws://${bind_addr}/ws" \
        CONNECTIONS="${conn}" \
        RAMP_BATCH="${RAMP_BATCH}" \
        RAMP_DELAY_MS="${RAMP_DELAY_MS}" \
        HOLD_SECS="${HOLD_SECS}" \
        HEARTBEAT_MS="${HEARTBEAT_MS}" \
        RESULT_JSON="${result_json}" \
        "${CLIENT_BIN}" >&2 &
    local client_pid="$!"

    local samples=()
    while kill -0 "${client_pid}" 2>/dev/null; do
        local rss
        rss="$(ps -o rss= -p "${SERVER_PID}" 2>/dev/null | tr -d ' ' || true)"
        if [ -n "${rss}" ]; then
            samples+=("${rss}")
        fi
        sleep "${SAMPLE_INTERVAL_SEC}"
    done

    local client_exit_code=0
    wait "${client_pid}" 2>/dev/null || client_exit_code="$?"
    if [ "${client_exit_code}" -ne 0 ]; then
        echo "エラー: ws-load-client が失敗しました（終了コード ${client_exit_code}）" >&2
        rm -f "${result_json}"
        stop_server
        exit 1
    fi

    if [ "${#samples[@]}" -eq 0 ]; then
        echo "エラー: 負荷印加中の RSS サンプルを取得できませんでした（保持時間 HOLD_SECS=${HOLD_SECS} がサンプリング間隔 ${SAMPLE_INTERVAL_SEC}s に対して短すぎる可能性）" >&2
        rm -f "${result_json}"
        stop_server
        exit 1
    fi

    local load_rss_kb
    load_rss_kb="$(printf '%s\n' "${samples[@]}" | median)"

    local connected success_rate_pct
    connected="$(jq -r '.connected' "${result_json}")"
    success_rate_pct="$(jq -r '.success_rate_percent' "${result_json}")"
    rm -f "${result_json}"

    stop_server

    echo "${idle_rss_kb} ${load_rss_kb} ${connected} ${success_rate_pct}"
}

# --- メイン処理 ----------------------------------------------------------
check_prereqs
check_ulimit "$(max_tier)"
check_ephemeral_port_range "$(max_tier)"

echo "=== bench-ws-load.sh（RUNS=${RUNS} HOLD_SECS=${HOLD_SECS} CONNECTION_TIERS=\"${CONNECTION_TIERS}\"） ===" >&2

declare -A PER_CONN_INCREMENT_MEDIAN_KB
declare -A SUCCESS_RATE_MEDIAN_PCT

for impl in fullscratch axum; do
    for conn in ${CONNECTION_TIERS}; do
        echo "--- impl=${impl} connections=${conn} ---" >&2
        increments=()
        success_rates=()
        for ((trial = 1; trial <= RUNS; trial++)); do
            read -r idle_rss_kb load_rss_kb connected success_rate_pct \
                <<<"$(run_single_trial "${impl}" "${conn}")"

            if [ "${connected}" -eq 0 ]; then
                echo "エラー: impl=${impl} connections=${conn} 試行 ${trial} で確立接続数が 0 でした" >&2
                exit 1
            fi

            increment_kb="$(LC_NUMERIC=C awk -v load_kb="${load_rss_kb}" -v idle="${idle_rss_kb}" -v n="${connected}" \
                'BEGIN { printf "%.4f", (load_kb - idle) / n }')"
            increments+=("${increment_kb}")
            success_rates+=("${success_rate_pct}")
            echo "  試行 ${trial}: idle=${idle_rss_kb}KB load中央値=${load_rss_kb}KB connected=${connected} success_rate=${success_rate_pct}% 接続あたり増分=${increment_kb}KB" >&2
        done

        increment_median="$(printf '%s\n' "${increments[@]}" | median)"
        success_rate_median="$(printf '%s\n' "${success_rates[@]}" | median)"
        PER_CONN_INCREMENT_MEDIAN_KB["${impl}_${conn}"]="${increment_median}"
        SUCCESS_RATE_MEDIAN_PCT["${impl}_${conn}"]="${success_rate_median}"
        echo "  => ${impl}/${conn}: 接続あたり RSS 増分中央値=${increment_median}KB 成立率中央値=${success_rate_median}%" >&2
    done
done

# --- 判定・出力 ------------------------------------------------------------
max_conn="$(max_tier)"

echo "" >&2
echo "=== 結果サマリ ===" >&2
printf '%-12s %-12s %-20s %-16s\n' "impl" "connections" "rss_increment_kb" "success_rate_pct" >&2
for impl in fullscratch axum; do
    for conn in ${CONNECTION_TIERS}; do
        printf '%-12s %-12s %-20s %-16s\n' "${impl}" "${conn}" \
            "${PER_CONN_INCREMENT_MEDIAN_KB[${impl}_${conn}]}" \
            "${SUCCESS_RATE_MEDIAN_PCT[${impl}_${conn}]}" >&2
    done
done

axum_ratio_pct="$(LC_NUMERIC=C awk \
    -v fs="${PER_CONN_INCREMENT_MEDIAN_KB[fullscratch_${max_conn}]}" \
    -v ax="${PER_CONN_INCREMENT_MEDIAN_KB[axum_${max_conn}]}" \
    'BEGIN { printf "%.2f", (fs / ax) * 100 }')"

echo "" >&2
echo "接続あたり RSS 増分 axum 比（${max_conn} 接続時点）: ${axum_ratio_pct}%（基準: ${AXUM_RATIO_MAX_PCT}% 以内）" >&2

overall_pass=0

judge_ratio="PASS"
if ! LC_NUMERIC=C awk -v r="${axum_ratio_pct}" -v max="${AXUM_RATIO_MAX_PCT}" 'BEGIN { exit !(r <= max) }'; then
    judge_ratio="FAIL"
    overall_pass=1
fi
echo "判定(1) RSS 増分 axum 比 ${AXUM_RATIO_MAX_PCT}% 以内: ${judge_ratio}" >&2

judge_success="PASS"
for impl in fullscratch axum; do
    for conn in ${CONNECTION_TIERS}; do
        rate="${SUCCESS_RATE_MEDIAN_PCT[${impl}_${conn}]}"
        if ! LC_NUMERIC=C awk -v r="${rate}" -v min="${SUCCESS_RATE_MIN_PCT}" 'BEGIN { exit !(r >= min) }'; then
            judge_success="FAIL（${impl}/${conn}: ${rate}%）"
            overall_pass=1
        fi
    done
done
echo "判定(2) 確立成功率 ${SUCCESS_RATE_MIN_PCT}% 以上（全 impl × 全接続数）: ${judge_success}" >&2

echo "判定(3) 1k→10k の線形性: 上記接続あたり RSS 増分の表を目視確認すること（自動判定は行わない。増分が接続数に対して概ね一定であれば線形とみなす）" >&2

if [ "${overall_pass}" -eq 0 ]; then
    echo "" >&2
    echo "=== 総合判定: PASS ===" >&2
else
    echo "" >&2
    echo "=== 総合判定: FAIL（詳細は上記判定行を参照） ===" >&2
fi

# 機械可読出力（RESULT_JSON 指定時のみ）。
if [ -n "${RESULT_JSON:-}" ]; then
    entries=()
    for impl in fullscratch axum; do
        for conn in ${CONNECTION_TIERS}; do
            entries+=("$(jq -n \
                --arg impl "${impl}" \
                --argjson connections "${conn}" \
                --argjson rss_increment_kb "${PER_CONN_INCREMENT_MEDIAN_KB[${impl}_${conn}]}" \
                --argjson success_rate_pct "${SUCCESS_RATE_MEDIAN_PCT[${impl}_${conn}]}" \
                '{impl: $impl, connections: $connections, rss_increment_kb: $rss_increment_kb, success_rate_pct: $success_rate_pct}')")
        done
    done
    matrix_json="$(printf '%s\n' "${entries[@]}" | jq -s '.')"
    result_json_out="$(jq -n \
        --argjson runs "${RUNS}" \
        --argjson hold_secs "${HOLD_SECS}" \
        --argjson axum_ratio_pct_at_max "${axum_ratio_pct}" \
        --arg max_connections "${max_conn}" \
        --argjson matrix "${matrix_json}" \
        '{runs: $runs, hold_secs: $hold_secs, axum_ratio_pct_at_max_connections: $axum_ratio_pct_at_max,
          max_connections: ($max_connections | tonumber), matrix: $matrix}')"
    write_result_json "${result_json_out}"
fi

exit "${overall_pass}"
