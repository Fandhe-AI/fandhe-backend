#!/usr/bin/env bash
# NFR-6（docs/spec/04-requirements.md）の empirical 計測スクリプト（TASK-4.4 / #25）。
#
# このスクリプトの役割:
#   `websocket` feature 有効時、無関係パス（`GET /health`）への RPS・p95 レイテンシ
#   影響が誤差範囲に収まることを、実際にビルドした 2 バイナリ（ベースライン
#   `examples/minimal`＝`websocket` feature 無効／比較対象
#   `examples/ws_nfr6`＝`websocket` feature 有効・`Server::websocket` 登録済み）へ
#   `oha` で負荷をかけて検証する。production 配線（`crates/plugin-websocket` の
#   ハンドシェイク検証・`crates/core` の `UpgradeHandler` 拡張点）自体は変更しない
#   （計測専用の example を叩くのみ）。`benches/graphql-nfr6-bench.sh`・
#   `benches/webrtc-nfr6-bench.sh` と同型。
#
# `examples/ws_echo.rs`（TASK-4.3 / #24、10,000 同時接続負荷試験専用、
# `#[tokio::main(flavor = "multi_thread")]`）を本計測へ流用しない: ベースライン
# `examples/minimal.rs` は `current_thread` ランタイムで動くため、`ws_echo` を
# そのまま比較対象にするとランタイムのスレッド数差（1 vs 全コア）が RPS 差を支配し、
# `websocket` feature 自体の処理コストを計測できない（実測で判明、baseline 比
# RPS 約190% という説明のつかない値になった。`crates/core/examples/ws_nfr6.rs` の
# doc comment・`benches/reports/task-4.4-ws-latency.md` 参照）。そのため
# `graphql_nfr6.rs`・`webrtc_nfr6.rs` と同型の `current_thread` 専用 example
# （`examples/ws_nfr6.rs`、待受 127.0.0.1:3009 固定）を使う。
#
# 前提:
#   - `cargo build --release -p fandhe-backend-core --example minimal
#      --no-default-features`
#   - `cargo build --release -p fandhe-backend-core --features websocket
#      --example ws_nfr6`
#   （本スクリプトはビルドを自動実行しない。既存バイナリの存在を検査するのみ。
#    benches/lib/common.sh の「サプライチェーン考慮・自動取得しない」方針を踏襲）
#
# 呼び出し元: 人間が `bash benches/ws-nfr6-bench.sh` として直接実行する。
# 結果は `docs/acceptance/req4-websocket.md` §NFR /
# `benches/reports/task-4.4-ws-latency.md` へ転記する。

set -euo pipefail

# lib/common.sh は DURATION=15s / CONNECTIONS=128 を既定値としてソース時に確定
# させてしまうため、`${DURATION:-5s}` のような事後デフォルト指定は常に無効化される
# （bash はソース済み変数がある限り `:-` フォールバックを発火しない）。
# 呼び出し元が env で明示指定したかどうかを source 前に退避し、未指定時のみ
# 本スクリプト固有の既定値（5s/32、NFR 計測の軽量デフォルト）を後段で復元する
# （`graphql-nfr6-bench.sh`/`webrtc-nfr6-bench.sh` と同一対策）。
_CALLER_DURATION="${DURATION-}"
_CALLER_CONNECTIONS="${CONNECTIONS-}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# WORKSPACE_ROOT・BENCH_TARGET_DIR は lib/common.sh が導出・export する
# （イシュー #480、BENCH_TARGET_DIR の優先順位は common.sh 冒頭コメント参照）。
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

RUNS="${RUNS:-5}"
DURATION="${_CALLER_DURATION:-5s}"
CONNECTIONS="${_CALLER_CONNECTIONS:-32}"

# BENCH_TARGET_DIR は common.sh が導出する実効 target ディレクトリ（イシュー #480、
# self-hosted runner のホスト共有 CARGO_TARGET_DIR 注入対策）。
BASELINE_BIN="${BENCH_TARGET_DIR}/release/examples/minimal"
WS_BIN="${BENCH_TARGET_DIR}/release/examples/ws_nfr6"
BASELINE_PORT=3000
WS_PORT=3009

if ! command -v oha >/dev/null 2>&1; then
    echo "エラー: oha が見つかりません。導入してください（例: cargo install oha）" >&2
    exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "エラー: jq が見つかりません。導入してください（例: apt install jq）" >&2
    exit 1
fi
# wait_ready が health check に curl を使用する（他の bench スクリプトは
# benches/lib/common.sh の check_dependencies で curl 不在を明示検出するが、
# 本スクリプトは共通関数ではなく専用の wait_ready を持つため個別に検査する）。
if ! command -v curl >/dev/null 2>&1; then
    echo "エラー: curl が見つかりません。導入してください（例: apt install curl）" >&2
    exit 1
fi
if [ ! -x "${BASELINE_BIN}" ]; then
    echo "エラー: ${BASELINE_BIN} が見つかりません。先に" >&2
    echo "  cargo build --release -p fandhe-backend-core --example minimal --no-default-features" >&2
    echo "を実行してください" >&2
    exit 1
fi
if [ ! -x "${WS_BIN}" ]; then
    echo "エラー: ${WS_BIN} が見つかりません。先に" >&2
    echo "  cargo build --release -p fandhe-backend-core --example ws_nfr6 --features websocket" >&2
    echo "を実行してください" >&2
    exit 1
fi

CURRENT_PID=""
# trap から呼ぶプロセス回収。start_measurement が起動した直近のサーバのみを対象にする
# （benches/lib/common.sh の stop_server と同じ「確実な回収」方針だが、本スクリプトは
# ベースライン・ws_nfr6 の 2 バイナリを順に起動するため PID を都度更新する）。
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

# 1 系統（ベースライン or ws_nfr6）の RPS/p95 を RUNS 回計測し中央値を返す。
# 引数: $1 バイナリパス、$2 待受アドレス（host:port）、$3 ラベル
# （`examples/ws_nfr6.rs` は待受アドレスを 127.0.0.1:3009 に固定しているため、
#  `graphql-nfr6-bench.sh` の `measure()` と同様に env 注入なしで起動できる）。
measure() {
    local bin="$1" bind_addr="$2" label="$3"
    local url="http://${bind_addr}/health"

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
echo "baseline: ${BASELINE_BIN}（websocket feature 無効、GET /health、current_thread）" >&2
echo "ws_nfr6 : ${WS_BIN}（websocket feature 有効、127.0.0.1:${WS_PORT}、GET /health、current_thread）" >&2
echo "" >&2

read -r baseline_rps baseline_p95 <<<"$(measure "${BASELINE_BIN}" "127.0.0.1:${BASELINE_PORT}" baseline)"
read -r ws_rps ws_p95 <<<"$(measure "${WS_BIN}" "127.0.0.1:${WS_PORT}" ws_nfr6)"

rps_ratio_pct="$(LC_NUMERIC=C awk -v a="${ws_rps}" -v b="${baseline_rps}" 'BEGIN { printf "%.2f", (a / b) * 100 }')"
p95_ratio_pct="$(LC_NUMERIC=C awk -v a="${ws_p95}" -v b="${baseline_p95}" 'BEGIN { printf "%.2f", (a / b) * 100 }')"

echo "" >&2
echo "=== 結果（中央値、対象: GET /health 無関係パス） ===" >&2
echo "baseline RPS 中央値: ${baseline_rps}" >&2
echo "ws_nfr6  RPS 中央値: ${ws_rps}（baseline 比 ${rps_ratio_pct}%）" >&2
echo "baseline p95 中央値: ${baseline_p95}" >&2
echo "ws_nfr6  p95 中央値: ${ws_p95}（baseline 比 ${p95_ratio_pct}%）" >&2

# machine-readable な結果を stdout へ（レポート転記の自動化・再実行比較用）。
printf 'baseline_rps=%s\nws_rps=%s\nrps_ratio_pct=%s\nbaseline_p95=%s\nws_p95=%s\np95_ratio_pct=%s\n' \
    "${baseline_rps}" "${ws_rps}" "${rps_ratio_pct}" "${baseline_p95}" "${ws_p95}" "${p95_ratio_pct}"
