#!/usr/bin/env bash
# REQ-9（docs/spec/04-requirements.md）NFR-6 の empirical 計測スクリプト（TASK-9.5 / #65）。
#
# このスクリプトの役割:
#   `fandhe-backend-plugin-hub-wiring` をリンクした最小サーバ（`examples/hub_link_only.rs`、
#   `BF_HUB_GATE=off` で `TenantGate` 未登録＝リンクコストのみを分離計測）が、
#   無関係パス（`GET /`）への RPS・p95 レイテンシに与える影響が誤差範囲に収まる
#   ことを、実際にビルドした 2 バイナリ（ベースライン `examples/minimal`／
#   比較対象 `examples/hub_link_only`）へ `oha` で負荷をかけて検証する。
#   `benches/graphql-nfr6-bench.sh`・`benches/webrtc-nfr6-bench.sh`（TASK-5.2・
#   TASK-8.4）と同型。
#
#   `examples/hub_service_demo.rs`（PoC-6 相当のマルチテナント `/items` 系
#   ハンドラを持つ実データ入り example）は使わない。マルチルート登録・
#   シードストア・`Authenticator` 呼び出し等のアプリケーション層オーバーヘッドが
#   リンクコストの計測値へ混入するため（Cursor Bugbot review 4727552092
#   指摘1、PR #163）。`hub_link_only.rs` は `examples/minimal.rs` と同一の
#   `GET /` のみを持つ最小構成（`crates/plugin-hub-wiring/examples/
#   hub_link_only.rs` 冒頭 doc 参照）。
#
# 前提:
#   - `cargo build --release -p fandhe-backend-core --example minimal
#      --no-default-features`
#   - `cargo build --release -p fandhe-backend-plugin-hub-wiring --example hub_link_only`
#   （本スクリプトはビルドを自動実行しない。既存バイナリの存在を検査するのみ。
#    benches/lib/common.sh の「サプライチェーン考慮・自動取得しない」方針を踏襲）
#
# 参考値（PASS/FAIL 判定には使わない）:
#   `hub_service_demo`（実データ入り example）を使い、`BF_HUB_GATE` を未設定にした
#   「ゲート有効 + 有効トークン」構成のスループットを opt-in コストとして手動計測し
#   併記する（`benches/reports/task-9.5-hub-wiring-performance.md` に転記。
#   `hub_link_only` は空 JWKS のため実トークンでの opt-in 計測はできない）。
#
# 呼び出し元: 人間が `bash benches/hub-nfr6-bench.sh` として直接実行する。
# 結果は `docs/acceptance/req9-hub-wiring.md` §NFR /
# `benches/reports/task-9.5-hub-wiring-performance.md` へ転記する。

set -euo pipefail

# lib/common.sh は DURATION=15s / CONNECTIONS=128 を既定値としてソース時に確定
# させてしまうため、`${DURATION:-5s}` のような事後デフォルト指定は常に無効化される
# （bash はソース済み変数がある限り `:-` フォールバックを発火しない）。呼び出し元が
# env で明示指定したかどうかを source 前に退避し、未指定時のみ本スクリプト固有の
# 既定値（5s/32、NFR 計測の軽量デフォルト）を後段で復元する
# （`graphql-nfr6-bench.sh`・`webrtc-nfr6-bench.sh` と同一対策）。
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
HUB_BIN="${WORKSPACE_ROOT}/target/release/examples/hub_link_only"
BASELINE_PORT=3000
# hub_link_only は 127.0.0.1:3101 に固定でバインドする
# （crates/plugin-hub-wiring/examples/hub_link_only.rs、hub_service_demo の
# 3100 と衝突しないポート）。
HUB_PORT=3101

if ! command -v oha >/dev/null 2>&1; then
    echo "エラー: oha が見つかりません。導入してください（例: cargo install oha）" >&2
    exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "エラー: jq が見つかりません。導入してください（例: apt install jq）" >&2
    exit 1
fi
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
if [ ! -x "${HUB_BIN}" ]; then
    echo "エラー: ${HUB_BIN} が見つかりません。先に" >&2
    echo "  cargo build --release -p fandhe-backend-plugin-hub-wiring --example hub_link_only" >&2
    echo "を実行してください" >&2
    exit 1
fi

CURRENT_PID=""
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

# 1 系統の RPS/p95 を RUNS 回計測し中央値を返す。
# 引数: $1 バイナリパス、$2 ポート、$3 ラベル、$4 起動時の追加環境変数（"KEY=VAL" 形式、省略可）
measure() {
    local bin="$1" port="$2" label="$3" env_kv="${4:-}"
    local url="http://127.0.0.1:${port}/"

    if [ -n "${env_kv}" ]; then
        env "${env_kv}" "${bin}" >/dev/null 2>&1 &
    else
        "${bin}" >/dev/null 2>&1 &
    fi
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
echo "baseline: ${BASELINE_BIN}（fandhe-backend-plugin-hub-wiring 未リンク）" >&2
echo "hub     : ${HUB_BIN}（fandhe-backend-plugin-hub-wiring リンク済み・BF_HUB_GATE=off で TenantGate 未登録、GET / は無関係パス）" >&2
echo "" >&2

read -r baseline_rps baseline_p95 <<<"$(measure "${BASELINE_BIN}" "${BASELINE_PORT}" baseline)"
read -r hub_rps hub_p95 <<<"$(measure "${HUB_BIN}" "${HUB_PORT}" "hub(gate off)" "BF_HUB_GATE=off")"

rps_ratio_pct="$(LC_NUMERIC=C awk -v a="${hub_rps}" -v b="${baseline_rps}" 'BEGIN { printf "%.2f", (a / b) * 100 }')"
p95_ratio_pct="$(LC_NUMERIC=C awk -v a="${hub_p95}" -v b="${baseline_p95}" 'BEGIN { printf "%.2f", (a / b) * 100 }')"

echo "" >&2
echo "=== 結果（中央値、対象: GET / 無関係パス） ===" >&2
echo "baseline RPS 中央値: ${baseline_rps}" >&2
echo "hub      RPS 中央値: ${hub_rps}（baseline 比 ${rps_ratio_pct}%）" >&2
echo "baseline p95 中央値: ${baseline_p95}" >&2
echo "hub      p95 中央値: ${hub_p95}（baseline 比 ${p95_ratio_pct}%）" >&2

# 参考値: ゲート有効 + 有効トークン時の opt-in コスト（PASS/FAIL 判定には使わない）。
# hub_service_demo は起動時に curl コマンド例として有効トークンを標準出力へ出す
# （認証ヘッダなしでは 401 になり GET /items は計測対象にならないため、ここでは
# GET / のみを叩いた baseline/off 比較を主計測とし、opt-in コストは README/レポート
# 側の手順で `GET /items` + 有効トークンにより手動計測する）。
echo "" >&2
echo "参考: opt-in コスト（ゲート有効時の /items 系スループット）は本スクリプトの" >&2
echo "  自動計測対象外。docs/acceptance/req9-hub-wiring.md の手順に従い手動計測する。" >&2

# machine-readable な結果を stdout へ（レポート転記の自動化・再実行比較用）。
printf 'baseline_rps=%s\nhub_rps=%s\nrps_ratio_pct=%s\nbaseline_p95=%s\nhub_p95=%s\np95_ratio_pct=%s\n' \
    "${baseline_rps}" "${hub_rps}" "${rps_ratio_pct}" "${baseline_p95}" "${hub_p95}" "${p95_ratio_pct}"
