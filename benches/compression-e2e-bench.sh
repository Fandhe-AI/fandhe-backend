#!/usr/bin/env bash
# イシュー #473: compression 有効構成の並行負荷下 E2E p99 比較を計測する。
#
# 背景: PR #471（イシュー #468）で巨大応答の gzip 圧縮を `spawn_blocking` へ
# 切り離したが、採否判定はマイクロベンチ（`compress_body` 単体の所要時間 vs
# ディスパッチ往復コスト）のみに基づいており、並行負荷下での E2E 検証
# （巨大応答の圧縮が同居する小応答のテールレイテンシを実際に保護しているか）
# は未実施だった。本スクリプトは以下の 2 構成を比較する:
#
#   構成 A（offload、既定）: BLOCKING_THRESHOLD 未指定（既定 64 KiB）。
#     /large の圧縮は spawn_blocking へ切り離される。
#   構成 B（inline、比較対象）: BLOCKING_THRESHOLD=max（usize::MAX）。
#     /large の圧縮も常にインライン実行される
#     （`CompressionConfigBuilder::blocking_threshold` の doc が明記する
#     オプトアウト相当）。
#
# 各構成で、バックグラウンド oha が GET /large（しきい値以上の圧縮対象応答）
# へ負荷を印加し続けている最中に、フォアグラウンド oha で GET /small
# （しきい値未満・常にインライン圧縮）の RPS / p50 / p95 / p99 を計測する。
# 加えて /large 自体の RPS / p99 も比較する（オフロード自体のディスパッチ
# コストによる /large 側の劣化有無の確認）。
#
# 前提: `cargo build --release --example compression_e2e_bench
# -p fandhe-backend-core --features compression`
# （`compression-e2e-exclusive.sh` はロック保持中に自動でビルドする。
# 本スクリプト単体で使う場合は事前に手動ビルドしておくこと）。
#
# 使い方・パラメータ・再現手順は benches/README.md を参照。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# common.sh の既定 TARGET_BIN/TARGET_PORT（axum-ref・3001）は他ベンチと
# 衝突するため、source 前に本スクリプト専用の既定値へ差し替える
# （common.sh 側は「呼び出し元が明示的に上書きした場合はその値をそのまま使う」
# 契約のため、ここでの export が優先される）。
export TARGET_BIN="${TARGET_BIN:-${SCRIPT_DIR}/../target/release/examples/compression_e2e_bench}"
export TARGET_PORT="${TARGET_PORT:-3011}"

# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

check_dependencies
check_runs_minimum

# /large へのバックグラウンド負荷の同時接続数（既定 32。CONNECTIONS
# （/small 側、common.sh 既定 128）とは独立に指定できるようにし、
# 「巨大応答の圧縮が同居する」条件を安定して再現する）。
LARGE_CONNECTIONS="${LARGE_CONNECTIONS:-32}"
validate_numeric "${LARGE_CONNECTIONS}" "LARGE_CONNECTIONS"

# example へ透過する疑似 body サイズ（既定は example 側の既定値
# DEFAULT_LARGE_BODY_SIZE=256 KiB と一致させるためここでは未設定のままにし、
# 明示指定時のみ env として example プロセスへ渡す）。
LARGE_BODY_SIZE="${LARGE_BODY_SIZE:-}"
if [ -n "${LARGE_BODY_SIZE}" ]; then
    validate_numeric "${LARGE_BODY_SIZE}" "LARGE_BODY_SIZE"
fi

# 一時ファイル（背景 oha の JSON 結果格納用）。trap で確実に削除する。
LARGE_JSON_TMP="$(mktemp)"

# バックグラウンドで起動中の /large 用 oha の PID（未起動時は空文字）。
# フォアグラウンドの /small 実行が失敗する、またはスクリプトが `wait` 前に
# 中断された場合でもこの PID を EXIT trap から kill し、oha プロセスの
# 残存によって後続の専有計測（quiescence チェック）がホストをビジー状態と
# 誤認して BLOCKED を返す事態を防ぐ（PR #474 Bugbot 指摘対応）。
LARGE_OHA_PID=""
cleanup_tmp() {
    rm -f "${LARGE_JSON_TMP}"
}
cleanup_large_oha() {
    if [ -n "${LARGE_OHA_PID}" ] && kill -0 "${LARGE_OHA_PID}" 2>/dev/null; then
        kill "${LARGE_OHA_PID}" 2>/dev/null || true
        wait "${LARGE_OHA_PID}" 2>/dev/null || true
    fi
}
trap 'cleanup_large_oha; cleanup_tmp; stop_server' EXIT

# 1 構成分の計測を実行する。
# 引数: $1 構成ラベル（例 "A（offload、既定）"）、$2 BLOCKING_THRESHOLD の
# 値（空文字なら env を設定しない = 既定値のまま）
run_configuration() {
    local config_label="$1"
    local blocking_threshold="$2"

    echo "## 構成 ${config_label}"
    echo

    if [ -n "${blocking_threshold}" ]; then
        export BLOCKING_THRESHOLD="${blocking_threshold}"
    else
        unset BLOCKING_THRESHOLD || true
    fi
    if [ -n "${LARGE_BODY_SIZE}" ]; then
        export LARGE_BODY_SIZE
    fi

    start_server
    wait_for_health >/dev/null

    # ウォームアップ（既存規約と同条件、3 秒）。
    oha -z 3s -c "${CONNECTIONS}" --no-tui --output-format json \
        -H 'Accept-Encoding: gzip' "${TARGET_URL}/small" >/dev/null 2>&1 || true

    local small_rps=() small_p50=() small_p95=() small_p99=()
    local large_rps=() large_p50=() large_p95=() large_p99=()

    for ((i = 1; i <= RUNS; i++)); do
        # /large への背景負荷をバックグラウンドで起動し、その最中に /small を計測する。
        oha -z "${DURATION}" -c "${LARGE_CONNECTIONS}" --no-tui --output-format json \
            -H 'Accept-Encoding: gzip' "${TARGET_URL}/large" >"${LARGE_JSON_TMP}" 2>&1 &
        local large_pid=$!
        LARGE_OHA_PID="${large_pid}"

        local small_json
        small_json="$(oha -z "${DURATION}" -c "${CONNECTIONS}" --no-tui --output-format json \
            -H 'Accept-Encoding: gzip' "${TARGET_URL}/small")"

        wait "${large_pid}"
        LARGE_OHA_PID=""
        local large_json
        large_json="$(cat "${LARGE_JSON_TMP}")"

        small_rps+=("$(echo "${small_json}" | jq -r '.summary.requestsPerSec')")
        small_p50+=("$(echo "${small_json}" | jq -r '.latencyPercentiles.p50')")
        small_p95+=("$(echo "${small_json}" | jq -r '.latencyPercentiles.p95')")
        small_p99+=("$(echo "${small_json}" | jq -r '.latencyPercentiles.p99')")

        large_rps+=("$(echo "${large_json}" | jq -r '.summary.requestsPerSec')")
        large_p50+=("$(echo "${large_json}" | jq -r '.latencyPercentiles.p50')")
        large_p95+=("$(echo "${large_json}" | jq -r '.latencyPercentiles.p95')")
        large_p99+=("$(echo "${large_json}" | jq -r '.latencyPercentiles.p99')")
    done

    stop_server

    local small_rps_median small_p50_median small_p95_median small_p99_median
    small_rps_median="$(printf '%s\n' "${small_rps[@]}" | median)"
    small_p50_median="$(printf '%s\n' "${small_p50[@]}" | median)"
    small_p95_median="$(printf '%s\n' "${small_p95[@]}" | median)"
    small_p99_median="$(printf '%s\n' "${small_p99[@]}" | median)"

    local large_rps_median large_p50_median large_p95_median large_p99_median
    large_rps_median="$(printf '%s\n' "${large_rps[@]}" | median)"
    large_p50_median="$(printf '%s\n' "${large_p50[@]}" | median)"
    large_p95_median="$(printf '%s\n' "${large_p95[@]}" | median)"
    large_p99_median="$(printf '%s\n' "${large_p99[@]}" | median)"

    echo "### GET /small（背景負荷: GET /large 同時実行中）"
    echo "raw RPS: ${small_rps[*]}"
    echo "raw p50: ${small_p50[*]}"
    echo "raw p95: ${small_p95[*]}"
    echo "raw p99: ${small_p99[*]}"
    echo "median  RPS=${small_rps_median} p50=${small_p50_median}s p95=${small_p95_median}s p99=${small_p99_median}s"
    echo
    echo "### GET /large（背景負荷本体）"
    echo "raw RPS: ${large_rps[*]}"
    echo "raw p50: ${large_p50[*]}"
    echo "raw p95: ${large_p95[*]}"
    echo "raw p99: ${large_p99[*]}"
    echo "median  RPS=${large_rps_median} p50=${large_p50_median}s p95=${large_p95_median}s p99=${large_p99_median}s"
    echo

    # マークダウン比較表への転記用に、構成ラベルと中央値を機械可読な行として
    # 標準エラー出力へも書き出す（レポート作成時の手動転記を補助する）。
    echo "SUMMARY|${config_label}|small_rps=${small_rps_median}|small_p99=${small_p99_median}|large_rps=${large_rps_median}|large_p99=${large_p99_median}" >&2
}

echo "# イシュー #473 compression E2E 負荷計測結果"
echo "# RUNS=${RUNS} DURATION=${DURATION} CONNECTIONS=${CONNECTIONS} LARGE_CONNECTIONS=${LARGE_CONNECTIONS}"
echo "# LARGE_BODY_SIZE=${LARGE_BODY_SIZE:-（既定 262144）}"
echo

run_configuration "A（offload、既定 blocking_threshold=64KiB）" ""
run_configuration "B（inline、blocking_threshold=max）" "max"

echo "上記の SUMMARY 行（標準エラー出力）を benches/reports/issue473-compression-e2e.md の比較表へ転記すること。"
