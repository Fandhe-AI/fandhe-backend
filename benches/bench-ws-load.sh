#!/usr/bin/env bash
# TASK-4.3（#24）10,000 同時 WebSocket 接続負荷試験・RSS 再計測ハーネス。
#
# このスクリプトの役割:
#   fullscratch（`crates/core/examples/ws_echo.rs`）と axum-ref（`ws` feature 有効）の
#   2 実装へ `crates/ws-load-client` で同一の WebSocket 負荷（接続数 1,000/5,000/10,000）
#   を掛け、保持期間中のサーバ RSS を継続サンプリングして「接続あたり RSS 増分」を
#   算出・比較する。docs/spec/03-poc/high-concurrency-scale（PoC-7）で確認された
#   axum 比 155.2%（TASK-4.2 最適化後、未実測）を正式に再計測し、REQ 基準
#   （axum 比 110% 以内・確立・維持成功率 99% 以上・1k→10k の線形性）を判定する。
#   「確立・維持成功率」は `crates/ws-load-client` が算出する
#   `success_rate_percent`（ハンドシェイク成功かつ `HOLD_SECS` の維持を完了できた
#   接続の割合。ハンドシェイクのみ成功し hold 中に切断された接続は含まない）を指す。
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
# common.sh は RUNS="${RUNS:-5}" を設定・export するため、本スクリプト固有の
# 既定値（3）を後段の `RUNS="${RUNS:-3}"` で適用しようとしても、呼び出し元が
# RUNS を未設定の場合は常に common.sh の 5 が先に確定し、本スクリプトの
# デフォルト 3 は決して適用されない（Bugbot 指摘、PR #164/#24）。呼び出し元が
# 明示した値かどうかをソース前に退避し、ソース後に本スクリプト固有の既定値へ
# 差し替えることで、他の NFR ベンチ（bench-rss.sh 等）と同様に呼び出し元の
# 明示指定を尊重しつつ、本スクリプト固有の既定値も正しく効かせる。
runs_caller_override="${RUNS:-}"
# shellcheck source=lib/common.sh
# median/to_json_array/write_result_json/validate_numeric のみ利用する
# （`RUNS`/`DURATION`/`CONNECTIONS`/`TARGET_*` の既定値・`check_dependencies`・
# `start_server` は本スクリプト独自の構成のため使わない）。
source "${SCRIPT_DIR}/lib/common.sh"

# --- 計測パラメータ（env で上書き可能） ---------------------------------
RUNS="${runs_caller_override:-3}"
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
# `ulimit -n` はプロセス単位の上限であり、クライアント（ws-load-client）と
# サーバ（fullscratch/axum-ref）は別プロセスとして起動するため、各プロセスが
# 消費する fd は概ね「最大接続数」分のみ（サーバ側は listen socket 等を含めても
# 同程度）。誤って「最大接続数 × 2」を要求すると、実際には 10,000 接続の負荷試験を
# 問題なく実行できる環境まで弾いてしまう（Bugbot 指摘、PR #164/#24）。
# 安全側の余裕（100）のみを加えた「最大接続数 + 余裕 100」を最小要求値とする。
check_ulimit() {
    local max_conn="$1"
    local required=$((max_conn + 100))
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

# TASK-4.4（#25）: 接続数増によるレイテンシ劣化率算出（最小ティア比）に使う
# 最小接続数ティア。`max_tier` と対の関数。
min_tier() {
    # shellcheck disable=SC2086  # CONNECTION_TIERS は意図的な空白区切りリストの単語分割
    printf '%s\n' ${CONNECTION_TIERS} | sort -n | head -1
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
# 標準出力: "idle_rss_kb load_rss_kb connected success_rate_pct p50_us p95_us p99_us max_us" の 1 行
# （TASK-4.4 / #25 でハートビート RTT percentile（`ws-load-client` が算出する
# `heartbeat_rtt_us.p50/p95/p99/max`）を末尾に追加。呼び出し元の `read -r` は
# フィールド数を追随させる必要がある。既存のフィールド順・意味は変更しない）。
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
    # TASK-4.4（#25）: メッセージ往復レイテンシ（心拍 RTT）の percentile を
    # `ws-load-client` の RESULT_JSON（`heartbeat_rtt_us.p50/p95/p99/max`）から
    # 抽出する。フィールド欠落時（旧バイナリ等）は判定不能を示す空文字ではなく
    # `null` を明示し、後段の awk 計算がロケール非依存の数値エラーとして
    # 早期に失敗するようにする（フェイルクローズ、.claude/rules/security.md）。
    local p50_us p95_us p99_us max_us
    p50_us="$(jq -r '.heartbeat_rtt_us.p50' "${result_json}")"
    p95_us="$(jq -r '.heartbeat_rtt_us.p95' "${result_json}")"
    p99_us="$(jq -r '.heartbeat_rtt_us.p99' "${result_json}")"
    max_us="$(jq -r '.heartbeat_rtt_us.max' "${result_json}")"
    rm -f "${result_json}"

    stop_server

    echo "${idle_rss_kb} ${load_rss_kb} ${connected} ${success_rate_pct} ${p50_us} ${p95_us} ${p99_us} ${max_us}"
}

# --- メイン処理 ----------------------------------------------------------
check_prereqs
check_ulimit "$(max_tier)"
check_ephemeral_port_range "$(max_tier)"

echo "=== bench-ws-load.sh（RUNS=${RUNS} HOLD_SECS=${HOLD_SECS} CONNECTION_TIERS=\"${CONNECTION_TIERS}\"） ===" >&2

declare -A PER_CONN_INCREMENT_MEDIAN_KB
declare -A SUCCESS_RATE_MEDIAN_PCT
# TASK-4.4（#25）: ハートビート RTT percentile（心拍往復レイテンシ）の中央値
# （RUNS 試行中）。p95 は「維持中の WebSocket 接続でメッセージ往復レイテンシを
# 計測記録し、接続数増による劣化度合いを定量化する」受け入れ条件(1)の直接の
# 判定材料になる。
declare -A RTT_P50_MEDIAN_US
declare -A RTT_P95_MEDIAN_US
declare -A RTT_P99_MEDIAN_US
declare -A RTT_MAX_MEDIAN_US

for impl in fullscratch axum; do
    for conn in ${CONNECTION_TIERS}; do
        echo "--- impl=${impl} connections=${conn} ---" >&2
        increments=()
        success_rates=()
        rtt_p50s=()
        rtt_p95s=()
        rtt_p99s=()
        rtt_maxs=()
        for ((trial = 1; trial <= RUNS; trial++)); do
            read -r idle_rss_kb load_rss_kb connected success_rate_pct \
                p50_us p95_us p99_us max_us \
                <<<"$(run_single_trial "${impl}" "${conn}")"

            if [ "${connected}" -eq 0 ]; then
                echo "エラー: impl=${impl} connections=${conn} 試行 ${trial} で確立接続数が 0 でした" >&2
                exit 1
            fi

            increment_kb="$(LC_NUMERIC=C awk -v load_kb="${load_rss_kb}" -v idle="${idle_rss_kb}" -v n="${connected}" \
                'BEGIN { printf "%.4f", (load_kb - idle) / n }')"
            increments+=("${increment_kb}")
            success_rates+=("${success_rate_pct}")
            rtt_p50s+=("${p50_us}")
            rtt_p95s+=("${p95_us}")
            rtt_p99s+=("${p99_us}")
            rtt_maxs+=("${max_us}")
            echo "  試行 ${trial}: idle=${idle_rss_kb}KB load中央値=${load_rss_kb}KB connected=${connected} 確立・維持成功率=${success_rate_pct}% 接続あたり増分=${increment_kb}KB 心拍RTT(us) p50=${p50_us} p95=${p95_us} p99=${p99_us} max=${max_us}" >&2
        done

        increment_median="$(printf '%s\n' "${increments[@]}" | median)"
        success_rate_median="$(printf '%s\n' "${success_rates[@]}" | median)"
        rtt_p50_median="$(printf '%s\n' "${rtt_p50s[@]}" | median)"
        rtt_p95_median="$(printf '%s\n' "${rtt_p95s[@]}" | median)"
        rtt_p99_median="$(printf '%s\n' "${rtt_p99s[@]}" | median)"
        rtt_max_median="$(printf '%s\n' "${rtt_maxs[@]}" | median)"
        PER_CONN_INCREMENT_MEDIAN_KB["${impl}_${conn}"]="${increment_median}"
        SUCCESS_RATE_MEDIAN_PCT["${impl}_${conn}"]="${success_rate_median}"
        RTT_P50_MEDIAN_US["${impl}_${conn}"]="${rtt_p50_median}"
        RTT_P95_MEDIAN_US["${impl}_${conn}"]="${rtt_p95_median}"
        RTT_P99_MEDIAN_US["${impl}_${conn}"]="${rtt_p99_median}"
        RTT_MAX_MEDIAN_US["${impl}_${conn}"]="${rtt_max_median}"
        echo "  => ${impl}/${conn}: 接続あたり RSS 増分中央値=${increment_median}KB 確立・維持成功率中央値=${success_rate_median}% 心拍RTT中央値(us) p50=${rtt_p50_median} p95=${rtt_p95_median} p99=${rtt_p99_median} max=${rtt_max_median}" >&2
    done
done

# --- 判定・出力 ------------------------------------------------------------
max_conn="$(max_tier)"
min_conn="$(min_tier)"

echo "" >&2
echo "=== 結果サマリ ===" >&2
printf '%-12s %-12s %-20s %-16s %-12s %-12s\n' "impl" "connections" "rss_increment_kb" "success_rate_pct" "rtt_p95_us" "rtt_p99_us" >&2
for impl in fullscratch axum; do
    for conn in ${CONNECTION_TIERS}; do
        printf '%-12s %-12s %-20s %-16s %-12s %-12s\n' "${impl}" "${conn}" \
            "${PER_CONN_INCREMENT_MEDIAN_KB[${impl}_${conn}]}" \
            "${SUCCESS_RATE_MEDIAN_PCT[${impl}_${conn}]}" \
            "${RTT_P95_MEDIAN_US[${impl}_${conn}]}" \
            "${RTT_P99_MEDIAN_US[${impl}_${conn}]}" >&2
    done
done

# TASK-4.4（#25）受け入れ条件(1)「接続数増による劣化度合いを定量化する」の直接の
# 判定材料。最小ティア→最大ティアの p95 心拍 RTT 劣化率（%）を impl ごとに算出する。
# `CONNECTION_TIERS` が単一ティアのみの場合（min == max）、劣化率は 100.00% 固定
# （比較対象がないため「劣化なし」として扱う。0 除算を避けるための早期判定）。
declare -A RTT_P95_DEGRADATION_PCT
echo "" >&2
echo "=== 心拍 RTT p95 劣化率（最小ティア=${min_conn} → 最大ティア=${max_conn}） ===" >&2
for impl in fullscratch axum; do
    if [ "${min_conn}" = "${max_conn}" ]; then
        RTT_P95_DEGRADATION_PCT["${impl}"]="100.00"
    else
        RTT_P95_DEGRADATION_PCT["${impl}"]="$(LC_NUMERIC=C awk \
            -v min_p95="${RTT_P95_MEDIAN_US[${impl}_${min_conn}]}" \
            -v max_p95="${RTT_P95_MEDIAN_US[${impl}_${max_conn}]}" \
            'BEGIN { if (min_p95 == 0) { print "null" } else { printf "%.2f", (max_p95 / min_p95) * 100 } }')"
    fi
    echo "  ${impl}: ${min_conn}接続 p95=${RTT_P95_MEDIAN_US[${impl}_${min_conn}]}us → ${max_conn}接続 p95=${RTT_P95_MEDIAN_US[${impl}_${max_conn}]}us（劣化率 ${RTT_P95_DEGRADATION_PCT[${impl}]}%）" >&2
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
echo "判定(2) 確立・維持成功率 ${SUCCESS_RATE_MIN_PCT}% 以上（全 impl × 全接続数）: ${judge_success}" >&2

echo "判定(3) 1k→10k の線形性: 上記接続あたり RSS 増分の表を目視確認すること（自動判定は行わない。増分が接続数に対して概ね一定であれば線形とみなす）" >&2

echo "判定(4)（TASK-4.4/#25）心拍 RTT p95 劣化率（${min_conn}→${max_conn}接続）: fullscratch=${RTT_P95_DEGRADATION_PCT[fullscratch]}% axum=${RTT_P95_DEGRADATION_PCT[axum]}%（自動 PASS/FAIL 判定は行わない。定量化した劣化率を benches/reports/task-4.4-ws-latency.md へ転記し目視評価する）" >&2

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
                --argjson heartbeat_rtt_p50_us "${RTT_P50_MEDIAN_US[${impl}_${conn}]}" \
                --argjson heartbeat_rtt_p95_us "${RTT_P95_MEDIAN_US[${impl}_${conn}]}" \
                --argjson heartbeat_rtt_p99_us "${RTT_P99_MEDIAN_US[${impl}_${conn}]}" \
                --argjson heartbeat_rtt_max_us "${RTT_MAX_MEDIAN_US[${impl}_${conn}]}" \
                '{impl: $impl, connections: $connections, rss_increment_kb: $rss_increment_kb, success_rate_pct: $success_rate_pct,
                  heartbeat_rtt_us: {p50: $heartbeat_rtt_p50_us, p95: $heartbeat_rtt_p95_us, p99: $heartbeat_rtt_p99_us, max: $heartbeat_rtt_max_us}}')")
        done
    done
    matrix_json="$(printf '%s\n' "${entries[@]}" | jq -s '.')"
    degradation_json="$(jq -n \
        --argjson fullscratch "${RTT_P95_DEGRADATION_PCT[fullscratch]}" \
        --argjson axum "${RTT_P95_DEGRADATION_PCT[axum]}" \
        --arg min_connections "${min_conn}" \
        --arg max_connections "${max_conn}" \
        '{min_connections: ($min_connections | tonumber), max_connections: ($max_connections | tonumber),
          heartbeat_rtt_p95_degradation_pct: {fullscratch: $fullscratch, axum: $axum}}')"
    result_json_out="$(jq -n \
        --argjson runs "${RUNS}" \
        --argjson hold_secs "${HOLD_SECS}" \
        --argjson degradation "${degradation_json}" \
        --argjson axum_ratio_pct_at_max "${axum_ratio_pct}" \
        --arg max_connections "${max_conn}" \
        --argjson matrix "${matrix_json}" \
        '{runs: $runs, hold_secs: $hold_secs, axum_ratio_pct_at_max_connections: $axum_ratio_pct_at_max,
          max_connections: ($max_connections | tonumber), matrix: $matrix,
          heartbeat_rtt_p95_degradation: $degradation}')"
    write_result_json "${result_json_out}"
fi

exit "${overall_pass}"
