#!/usr/bin/env bash
# NFR-6（docs/spec/04-requirements.md）の empirical 計測スクリプト（TASK-8.4 / #29）。
#
# このスクリプトの役割:
#   `webrtc` feature 有効時、無関係パス（`GET /`）への RPS・p95 レイテンシ影響が
#   誤差範囲に収まることを、実際にビルドした 2 バイナリ（ベースライン
#   `examples/minimal`＝`webrtc` feature 無効／比較対象
#   `examples/webrtc_nfr6`＝`webrtc` feature 有効・`Server::webrtc` 登録済み）へ
#   `oha` で負荷をかけて検証する。production 配線（`crates/core/src/plugin.rs` の
#   `try_intercept`）自体は変更しない（計測専用の example を叩くのみ）。
#
# 前提:
#   - `cargo build --release -p backend-framework-core --example minimal
#      --no-default-features`
#   - `cargo build --release -p backend-framework-core --example webrtc_nfr6
#      --features webrtc`
#   （本スクリプトはビルドを自動実行しない。既存バイナリの存在を検査するのみ。
#    benches/lib/common.sh の「サプライチェーン考慮・自動取得しない」方針を踏襲）
#
# 呼び出し元: 人間が `bash benches/webrtc-nfr6-bench.sh` として直接実行する。
# 結果は `docs/acceptance/req8-webrtc-attack-surface.md` §NFR-6 /
# `benches/reports/task-8.4-webrtc-nfr6.md` へ転記する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

RUNS="${RUNS:-5}"
DURATION="${DURATION:-5s}"
CONNECTIONS="${CONNECTIONS:-32}"

BASELINE_BIN="${WORKSPACE_ROOT}/target/release/examples/minimal"
WEBRTC_BIN="${WORKSPACE_ROOT}/target/release/examples/webrtc_nfr6"
BASELINE_PORT=3000
WEBRTC_PORT=3002

if ! command -v oha >/dev/null 2>&1; then
    echo "エラー: oha が見つかりません。導入してください（例: cargo install oha）" >&2
    exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "エラー: jq が見つかりません。導入してください（例: apt install jq）" >&2
    exit 1
fi
if [ ! -x "${BASELINE_BIN}" ]; then
    echo "エラー: ${BASELINE_BIN} が見つかりません。先に" >&2
    echo "  cargo build --release -p backend-framework-core --example minimal --no-default-features" >&2
    echo "を実行してください" >&2
    exit 1
fi
if [ ! -x "${WEBRTC_BIN}" ]; then
    echo "エラー: ${WEBRTC_BIN} が見つかりません。先に" >&2
    echo "  cargo build --release -p backend-framework-core --example webrtc_nfr6 --features webrtc" >&2
    echo "を実行してください" >&2
    exit 1
fi

CURRENT_PID=""
# trap から呼ぶプロセス回収。start_measurement が起動した直近のサーバのみを対象にする
# （benches/lib/common.sh の stop_server と同じ「確実な回収」方針だが、本スクリプトは
# ベースライン・webrtc の 2 バイナリを順に起動するため PID を都度更新する）。
cleanup() {
    if [ -n "${CURRENT_PID}" ] && kill -0 "${CURRENT_PID}" 2>/dev/null; then
        kill "${CURRENT_PID}" 2>/dev/null || true
        wait "${CURRENT_PID}" 2>/dev/null || true
    fi
}
trap cleanup EXIT

wait_ready() {
    local url="$1" timeout_ms=5000 elapsed_ms=0
    while [ "${elapsed_ms}" -lt "${timeout_ms}" ]; do
        if curl -s -o /dev/null -w '%{http_code}' "${url}" 2>/dev/null | grep -q '^200$'; then
            return 0
        fi
        sleep 0.05
        elapsed_ms=$((elapsed_ms + 50))
    done
    echo "エラー: ${url} が ${timeout_ms}ms 以内に応答しませんでした" >&2
    return 1
}

# 1 系統（ベースライン or webrtc 有効）の RPS/p95 を RUNS 回計測し中央値を返す。
# 引数: $1 バイナリパス、$2 ポート、$3 ラベル
measure() {
    local bin="$1" port="$2" label="$3"
    local url="http://127.0.0.1:${port}/"

    "${bin}" >/dev/null 2>&1 &
    CURRENT_PID="$!"
    wait_ready "${url}"

    # ウォームアップ（JIT・キャッシュ安定化、benches/bench-http.sh と同条件）。
    oha -z 2s -c "${CONNECTIONS}" --no-tui --output-format json "${url}" >/dev/null 2>&1 || true

    local rps_values=() p95_values=()
    local i json rps p95
    for ((i = 1; i <= RUNS; i++)); do
        json="$(oha -z "${DURATION}" -c "${CONNECTIONS}" --no-tui --output-format json "${url}")"
        rps="$(echo "${json}" | jq -r '.summary.requestsPerSec')"
        p95="$(echo "${json}" | jq -r '.latencyPercentiles.p95')"
        rps_values+=("${rps}")
        p95_values+=("${p95}")
        echo "  [${label}] run ${i}: rps=${rps} p95=${p95}" >&2
    done

    kill "${CURRENT_PID}" 2>/dev/null || true
    wait "${CURRENT_PID}" 2>/dev/null || true
    CURRENT_PID=""

    local rps_median p95_median
    rps_median="$(printf '%s\n' "${rps_values[@]}" | median)"
    p95_median="$(printf '%s\n' "${p95_values[@]}" | median)"
    echo "${rps_median} ${p95_median}"
}

echo "=== NFR-6 計測（RUNS=${RUNS} DURATION=${DURATION} CONNECTIONS=${CONNECTIONS}） ===" >&2
echo "baseline: ${BASELINE_BIN}（webrtc feature 無効）" >&2
echo "webrtc  : ${WEBRTC_BIN}（webrtc feature 有効、Server::webrtc 登録済み）" >&2
echo "" >&2

read -r baseline_rps baseline_p95 <<<"$(measure "${BASELINE_BIN}" "${BASELINE_PORT}" baseline)"
read -r webrtc_rps webrtc_p95 <<<"$(measure "${WEBRTC_BIN}" "${WEBRTC_PORT}" webrtc)"

rps_ratio_pct="$(LC_NUMERIC=C awk -v a="${webrtc_rps}" -v b="${baseline_rps}" 'BEGIN { printf "%.2f", (a / b) * 100 }')"
p95_ratio_pct="$(LC_NUMERIC=C awk -v a="${webrtc_p95}" -v b="${baseline_p95}" 'BEGIN { printf "%.2f", (a / b) * 100 }')"

echo "" >&2
echo "=== 結果（中央値、対象: GET / 無関係パス） ===" >&2
echo "baseline RPS 中央値: ${baseline_rps}" >&2
echo "webrtc   RPS 中央値: ${webrtc_rps}（baseline 比 ${rps_ratio_pct}%）" >&2
echo "baseline p95 中央値: ${baseline_p95}" >&2
echo "webrtc   p95 中央値: ${webrtc_p95}（baseline 比 ${p95_ratio_pct}%）" >&2

# machine-readable な結果を stdout へ（レポート転記の自動化・再実行比較用）。
printf 'baseline_rps=%s\nwebrtc_rps=%s\nrps_ratio_pct=%s\nbaseline_p95=%s\nwebrtc_p95=%s\np95_ratio_pct=%s\n' \
    "${baseline_rps}" "${webrtc_rps}" "${rps_ratio_pct}" "${baseline_p95}" "${webrtc_p95}" "${p95_ratio_pct}"
