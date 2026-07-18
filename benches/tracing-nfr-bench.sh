#!/usr/bin/env bash
# REQ-10（docs/spec/04-requirements.md）TASK-10.4（#59）サンプリング適用後の
# 性能再検証スクリプト。
#
# このスクリプトの役割:
#   `tracing` feature 有効時、高頻度パス（`GET /health`）への RPS・p95 レイテンシ
#   影響が緩和策適用後の目標（RPS 劣化 5% 以内・p95 悪化 110% 以内）に収まるかを、
#   実際にビルドした 2 バイナリ（ベースライン `examples/minimal`＝`tracing` feature
#   無効／比較対象 `examples/tracing_nfr`＝`tracing` feature 有効・`init_tracing` +
#   `Server::tracing` 登録済み）へ `oha` で負荷をかけて検証する。production 配線
#   （`crates/core/src/server.rs` の `Server::tracing`）自体は変更しない（計測専用の
#   example を叩くのみ）。`benches/graphql-nfr6-bench.sh`（TASK-5.2 / #53）と同型。
#
#   2 シナリオを計測する:
#     A（受け入れ判定対象）: TASK-10.1〜10.3 の全緩和策適用
#       （サンプリング間隔 100 + 受理・応答イベント統合 + `/health` を
#        `TracingConfig::exclude_path` で除外）。`examples/tracing_nfr.rs` の
#        既定挙動（`EXCLUDE_HEALTH` 未指定 = `/health` 除外あり）をそのまま使う
#     B（参考値。受け入れ判定には使わない）: 除外なし・サンプリングのみ
#       （`EXCLUDE_HEALTH=0`）。TASK-10.3 除外機構の効果を差分として観測するために
#       残す。サンプリング対象パスに残るオーバーヘッドの実測記録が目的
#
# 前提:
#   - `cargo build --release -p backend-framework-core --example minimal
#      --no-default-features`
#   - `cargo build --release -p backend-framework-core --example tracing_nfr
#      --features tracing`
#   （本スクリプトはビルドを自動実行しない。既存バイナリの存在を検査するのみ。
#    benches/lib/common.sh の「サプライチェーン考慮・自動取得しない」方針を踏襲）
#
# 呼び出し元: 人間が `bash benches/tracing-nfr-bench.sh` として直接実行する。
# 結果は `benches/reports/task-10.4-tracing-performance.md` へ転記する。

set -euo pipefail

# lib/common.sh は DURATION=15s / CONNECTIONS=128 を既定値としてソース時に確定
# させてしまうため、事後デフォルト指定（`${DURATION:-5s}`）は無効化される
# （bash はソース済み変数がある限り `:-` フォールバックを発火しない）。
# 呼び出し元が env で明示指定したかどうかを source 前に退避し、未指定時のみ
# 本スクリプト固有の既定値（5s/32、NFR 計測の軽量デフォルト）を後段で復元する
# （graphql-nfr6-bench.sh / webrtc-nfr6-bench.sh と同一対策）。
_CALLER_DURATION="${DURATION-}"
_CALLER_CONNECTIONS="${CONNECTIONS-}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

RUNS="${RUNS:-5}"
DURATION="${_CALLER_DURATION:-5s}"
CONNECTIONS="${_CALLER_CONNECTIONS:-32}"

BASELINE_BIN="${WORKSPACE_ROOT}/target/release/examples/minimal"
TRACING_BIN="${WORKSPACE_ROOT}/target/release/examples/tracing_nfr"
BASELINE_PORT=3000
TRACING_PORT=3006

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
    echo "  cargo build --release -p backend-framework-core --example minimal --no-default-features" >&2
    echo "を実行してください" >&2
    exit 1
fi
if [ ! -x "${TRACING_BIN}" ]; then
    echo "エラー: ${TRACING_BIN} が見つかりません。先に" >&2
    echo "  cargo build --release -p backend-framework-core --example tracing_nfr --features tracing" >&2
    echo "を実行してください" >&2
    exit 1
fi

CURRENT_PID=""
# サーバの stdout（tracing シナリオでは非同期 writer 経由のトレースログが出る）を
# 一時ファイルへリダイレクトし、trap で確実に削除する（一時生成物をリポジトリに
# 残さない、.claude/rules/security.md「一時生成物」節）。
TMP_LOG_DIR="$(mktemp -d)"
# measure() はコマンド置換 `$(measure ...)` のサブシェル内で実行されるため、
# サブシェル内で更新した CURRENT_PID はサブシェル終了時に破棄され、親シェルの
# `trap cleanup EXIT` からは見えない（bash のサブシェル変数スコープ）。
# そこで起動中サーバの PID をファイル（PID_FILE）へ書き出し、親・子どちらの
# プロセスからも同じファイルを介して現在の起動状態を共有する。これにより
# `set -euo pipefail` 下で oha 実行失敗や wait_ready のタイムアウト等の異常系が
# measure() 内で発生しても、cleanup() が PID_FILE を読んで起動済みサーバを
# 確実に kill できる（残留プロセス対策）。
PID_FILE="${TMP_LOG_DIR}/current.pid"
cleanup() {
    if [ -f "${PID_FILE}" ]; then
        local pid
        pid="$(cat "${PID_FILE}" 2>/dev/null || true)"
        if [ -n "${pid}" ] && kill -0 "${pid}" 2>/dev/null; then
            kill "${pid}" 2>/dev/null || true
            wait "${pid}" 2>/dev/null || true
        fi
    fi
    rm -rf "${TMP_LOG_DIR}"
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

# 1 系統（ベースライン or tracing 有効・シナリオ指定）の RPS/p95 を RUNS 回計測し
# 中央値を返す。
# 引数: $1 バイナリパス、$2 ポート、$3 ラベル、$4 対象パス、
#       $5 サーバプロセスへ渡す追加環境変数（"KEY=VALUE" 形式、省略可・空文字なら無指定）
measure() {
    local bin="$1" port="$2" label="$3" path="$4" env_kv="${5:-}"
    local url="http://127.0.0.1:${port}${path}"
    local log_file="${TMP_LOG_DIR}/${label}.log"

    if [ -n "${env_kv}" ]; then
        env "${env_kv}" "${bin}" >"${log_file}" 2>&1 &
    else
        "${bin}" >"${log_file}" 2>&1 &
    fi
    CURRENT_PID="$!"
    # このサーバ PID を PID_FILE に記録する（measure() 自体はコマンド置換の
    # サブシェル内で動くため、親シェルの cleanup() が異常系で kill できるよう
    # ファイル経由で共有する。上記 PID_FILE 定義部のコメント参照）。
    echo "${CURRENT_PID}" >"${PID_FILE}"
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
    : >"${PID_FILE}"

    local rps_median p95_median
    rps_median="$(printf '%s\n' "${rps_values[@]}" | median)"
    p95_median="$(printf '%s\n' "${p95_values[@]}" | median)"
    echo "${rps_median} ${p95_median}"
}

ratio_pct() {
    LC_NUMERIC=C awk -v a="$1" -v b="$2" 'BEGIN { printf "%.2f", (a / b) * 100 }'
}

echo "=== NFR 計測（RUNS=${RUNS} DURATION=${DURATION} CONNECTIONS=${CONNECTIONS}） ===" >&2
echo "baseline: ${BASELINE_BIN}（tracing feature 無効）" >&2
echo "tracing : ${TRACING_BIN}（tracing feature 有効、Server::tracing 登録済み）" >&2
echo "対象パス: GET /health（高頻度パス想定）" >&2
echo "" >&2

read -r baseline_rps baseline_p95 <<<"$(measure "${BASELINE_BIN}" "${BASELINE_PORT}" baseline /health)"

echo "" >&2
echo "--- シナリオ A（受け入れ判定対象・全緩和策適用: サンプリング + イベント統合 + /health 除外） ---" >&2
read -r tracing_a_rps tracing_a_p95 <<<"$(measure "${TRACING_BIN}" "${TRACING_PORT}" tracing_a /health)"

echo "" >&2
echo "--- シナリオ B（参考値・除外なし: サンプリングのみ） ---" >&2
read -r tracing_b_rps tracing_b_p95 <<<"$(measure "${TRACING_BIN}" "${TRACING_PORT}" tracing_b /health "EXCLUDE_HEALTH=0")"

rps_a_ratio_pct="$(ratio_pct "${tracing_a_rps}" "${baseline_rps}")"
p95_a_ratio_pct="$(ratio_pct "${tracing_a_p95}" "${baseline_p95}")"
rps_b_ratio_pct="$(ratio_pct "${tracing_b_rps}" "${baseline_rps}")"
p95_b_ratio_pct="$(ratio_pct "${tracing_b_p95}" "${baseline_p95}")"

echo "" >&2
echo "=== 結果（中央値、対象: GET /health） ===" >&2
echo "baseline   RPS 中央値: ${baseline_rps} / p95 中央値: ${baseline_p95}" >&2
echo "シナリオA  RPS 中央値: ${tracing_a_rps}（baseline 比 ${rps_a_ratio_pct}%） / p95 中央値: ${tracing_a_p95}（baseline 比 ${p95_a_ratio_pct}%）" >&2
echo "シナリオB  RPS 中央値: ${tracing_b_rps}（baseline 比 ${rps_b_ratio_pct}%） / p95 中央値: ${tracing_b_p95}（baseline 比 ${p95_b_ratio_pct}%）" >&2

# machine-readable な結果を stdout へ（レポート転記の自動化・再実行比較用）。
printf 'baseline_rps=%s\nbaseline_p95=%s\ntracing_a_rps=%s\ntracing_a_p95=%s\nrps_a_ratio_pct=%s\np95_a_ratio_pct=%s\ntracing_b_rps=%s\ntracing_b_p95=%s\nrps_b_ratio_pct=%s\np95_b_ratio_pct=%s\n' \
    "${baseline_rps}" "${baseline_p95}" \
    "${tracing_a_rps}" "${tracing_a_p95}" "${rps_a_ratio_pct}" "${p95_a_ratio_pct}" \
    "${tracing_b_rps}" "${tracing_b_p95}" "${rps_b_ratio_pct}" "${p95_b_ratio_pct}"
