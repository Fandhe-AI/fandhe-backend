#!/usr/bin/env bash
# REQ-16（plugin-jwt）の NFR-6 empirical 計測スクリプト（イシュー #821）。
#
# このスクリプトの役割:
#   `fandhe-backend-plugin-jwt` の `JwtGate` を登録した最小サーバ
#   （`crates/plugin-jwt/examples/jwt_gate_server.rs`）が、無関係パス（`GET /`）への
#   RPS・p95 レイテンシに与える影響を、実際にビルドしたバイナリへ `oha` で負荷をかけて
#   計測する。`benches/hub-nfr6-bench.sh` と同型。
#
# 比較設計（hub 版との違い）:
#   `JwtGate::check` は経路の除外を持たず、トークンが無ければどのパスでも 401 を返す。
#   ヘッダ無しで `GET /` を叩くと baseline の 200 とゲートの 401 という異なる応答形状を
#   比べてしまう（task-9.5 レポートが撤回した不正な比較と同じ形）。そのため:
#   - サーバが起動時に有効トークンを作り、`FANDHE_BACKEND_JWT_BENCH_TOKEN_FILE` へ書く
#   - 全系統（baseline を含む）に同じ `Authorization: Bearer <token>` を付けて負荷をかける
#     （ヘッダ解析コストが揃い、差がゲートのコストだけになる）
#   RS256 は署名が長くヘッダ転送量・解析コストが異なるため、RS256 の比率は RS256 トークンを
#   付けた baseline を分母にする（ES256 の baseline とは別に計測する）。
#   各試行は oha の JSON で成功率 100% かつ全応答 200 を検証し、満たさなければ中止する。
#   系統は (a) baseline（minimal）、(b) リンクのみ（ゲート未登録）、(c) ゲート有効 +
#   有効トークン（ES256 が主計測、RS256 も 1 系列）。
#
# 判定の解釈:
#   NFR-6 の狭義帯は「パスが一致したときだけ介入する拡張点」向け。全パスに介入する
#   ゲートは NFR-7（標準ミドルウェア有効時の劣化 5% 以内）に近いため、合否は実務許容帯
#   （95〜105%）で見て、狭義帯外は WARN として記録する（PASS に丸めない）。
#   主判定は (c) ES256 対 (a)。stdout の `rps_ratio_pct=` / `p95_ratio_pct=` がそれ。
#
# 前提（本スクリプトはビルドも oha の取得も自動実行しない）:
#   - `cargo build --release -p fandhe-backend-core --example minimal --no-default-features`
#   - `cargo build --release -p fandhe-backend-plugin-jwt --example jwt_gate_server`
#   - oha / jq / curl
#
# 終了コード: 計測できれば判定に関わらず 0。前提欠落・正常性確認の失敗は非 0
# （誤った比較を記録しないため）。
# 呼び出し元: 人間が `bash benches/jwt-nfr6-bench.sh` として直接実行する、または
# `TARGETS=jwt bash benches/nfr6-exclusive.sh`（専有枠）。結果は
# `benches/reports/issue821-jwt-verify-performance.md` へ転記する。

set -euo pipefail

# lib/common.sh は source 時に DURATION・CONNECTIONS を確定させるため、呼び出し元の
# 指定を先に退避する（hub-nfr6-bench.sh と同一対策）。
_CALLER_DURATION="${DURATION-}"
_CALLER_CONNECTIONS="${CONNECTIONS-}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
# shellcheck source=../scripts/accept/lib/nfr6-ratio.sh
source "${WORKSPACE_ROOT}/scripts/accept/lib/nfr6-ratio.sh"

RUNS="${RUNS:-5}"
DURATION="${_CALLER_DURATION:-5s}"
CONNECTIONS="${_CALLER_CONNECTIONS:-32}"

BASELINE_BIN="${BENCH_TARGET_DIR}/release/examples/minimal"
JWT_BIN="${BENCH_TARGET_DIR}/release/examples/jwt_gate_server"
BASELINE_PORT=3000
JWT_PORT=3103

for tool in oha jq curl; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "エラー: ${tool} が見つかりません。導入してください（oha は例: cargo install oha）" >&2
        exit 1
    fi
done
if [ ! -x "${BASELINE_BIN}" ]; then
    echo "エラー: ${BASELINE_BIN} が見つかりません。先に" >&2
    echo "  cargo build --release -p fandhe-backend-core --example minimal --no-default-features" >&2
    echo "を実行してください" >&2
    exit 1
fi
if [ ! -x "${JWT_BIN}" ]; then
    echo "エラー: ${JWT_BIN} が見つかりません。先に" >&2
    echo "  cargo build --release -p fandhe-backend-plugin-jwt --example jwt_gate_server" >&2
    echo "を実行してください" >&2
    exit 1
fi

TOKEN_FILE="$(mktemp)"
CURRENT_PID=""
cleanup() {
    stop_server
    rm -f "${TOKEN_FILE}"
}
stop_server() {
    if [ -n "${CURRENT_PID}" ] && kill -0 "${CURRENT_PID}" 2>/dev/null; then
        kill "${CURRENT_PID}" 2>/dev/null || true
        wait "${CURRENT_PID}" 2>/dev/null || true
    fi
    CURRENT_PID=""
}
trap cleanup EXIT

# 200 が返るまで待つ。トークン付きで叩く（ゲート有効サーバは無トークンだと 401 のため）。
wait_ready() {
    local url="$1" token="$2" timeout_ms=5000 elapsed_ms=0
    while [ "${elapsed_ms}" -lt "${timeout_ms}" ]; do
        # 起動したプロセスが bind 失敗等で終了していたら、別プロセスの 200 を拾わず中止する。
        if [ -n "${CURRENT_PID}" ] && ! kill -0 "${CURRENT_PID}" 2>/dev/null; then
            echo "エラー: 起動したサーバ（pid=${CURRENT_PID}）が ready 前に終了しました（ポート bind 失敗の可能性）。誤った比較を記録しないため中止します" >&2
            CURRENT_PID=""
            return 1
        fi
        if curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer ${token}" "${url}" 2>/dev/null | grep -q '^200$'; then
            return 0
        fi
        sleep 0.05
        elapsed_ms=$((elapsed_ms + 50))
    done
    echo "エラー: ${url} が ${timeout_ms}ms 以内に 200 を返しませんでした" >&2
    return 1
}

# 指定ポートを既に誰かが listen していないことを確認する。占有されていると起動した
# サーバが bind に失敗し、既存サーバの応答を計測対象と取り違えるため。
# 引数: $1 ポート
assert_port_free() {
    local port="$1"
    if (exec 3<>"/dev/tcp/127.0.0.1/${port}") 2>/dev/null; then
        echo "エラー: 127.0.0.1:${port} は既に使用されています。別プロセスを停止してから再実行してください" >&2
        exit 1
    fi
}

# baseline（minimal）を起動する。
start_baseline() {
    assert_port_free "${BASELINE_PORT}"
    "${BASELINE_BIN}" >/dev/null 2>&1 &
    CURRENT_PID="$!"
}

# サーバを起動する（stdout は捨てる。トークンは出力しない）。
# 引数: $1 バイナリ、$2 FANDHE_BACKEND_JWT_GATE の値（"" なら未設定）、$3 ALG（"" なら既定）
start_server() {
    local bin="$1" gate="$2" alg="$3"
    assert_port_free "${JWT_PORT}"
    : >"${TOKEN_FILE}"
    (
        if [ -n "${gate}" ]; then export FANDHE_BACKEND_JWT_GATE="${gate}"; fi
        if [ -n "${alg}" ]; then export FANDHE_BACKEND_JWT_BENCH_ALG="${alg}"; fi
        export FANDHE_BACKEND_JWT_BENCH_TOKEN_FILE="${TOKEN_FILE}"
        exec "${bin}" >/dev/null 2>&1
    ) &
    CURRENT_PID="$!"
}

# 起動直後のサーバのトークンファイルが書かれるのを待って読み込む。
read_token() {
    local i
    for i in $(seq 1 100); do
        if [ -s "${TOKEN_FILE}" ]; then
            cat "${TOKEN_FILE}"
            return 0
        fi
        sleep 0.05
    done
    echo "エラー: トークンファイルが書かれませんでした" >&2
    return 1
}

http_code() {
    # $1 URL、$2 トークン（空ならヘッダ無し）
    if [ -n "$2" ]; then
        curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $2" "$1"
    else
        curl -s -o /dev/null -w '%{http_code}' "$1"
    fi
}

expect_code() {
    local want="$1" got="$2" what="$3"
    if [ "${got}" != "${want}" ]; then
        echo "エラー: 正常性確認に失敗: ${what}（期待 ${want}、実際 ${got}）。誤った比較を記録しないため中止します" >&2
        exit 1
    fi
}

# 試行の全応答が 200 で、通信失敗が無いことを確認する。満たさない試行（401 の混入、
# 接続失敗等）の RPS/p95 は別の応答形状の値になるため、計測失敗として中止する。
# 引数: $1 oha の JSON、$2 ラベル
verify_all_200() {
    local json="$1" what="$2"
    if ! echo "${json}" | jq -e '
        (.summary.successRate == 1)
        and ((.errorDistribution // {}) | length == 0)
        and ((.statusCodeDistribution // {}) | to_entries | length > 0)
        and ((.statusCodeDistribution // {}) | to_entries | all(.key == "200"))
    ' >/dev/null 2>&1; then
        echo "エラー: 計測失敗（${what}）: 全応答が 200 ではない、または通信失敗があります。" >&2
        echo "  statusCodeDistribution=$(echo "${json}" | jq -c '.statusCodeDistribution // null')" >&2
        echo "  successRate=$(echo "${json}" | jq -c '.summary.successRate // null')" >&2
        echo "  errorDistribution=$(echo "${json}" | jq -c '.errorDistribution // null')" >&2
        echo "誤った比較を記録しないため中止します" >&2
        exit 1
    fi
}

# 1 系統の RPS/p95 を RUNS 回計測し中央値を返す（"rps p95"）。
# 引数: $1 ポート、$2 ラベル、$3 トークン。サーバは起動済みで ready 済みであること。
run_load() {
    local port="$1" label="$2" token="$3"
    local url="http://127.0.0.1:${port}/"
    local hdr="Authorization: Bearer ${token}"

    oha -z 2s -c "${CONNECTIONS}" -H "${hdr}" --no-tui --output-format json "${url}" >/dev/null 2>&1 || true

    local rps_values=() p95_values=()
    local i json rps p95
    for ((i = 1; i <= RUNS; i++)); do
        json="$(oha -z "${DURATION}" -c "${CONNECTIONS}" -H "${hdr}" --no-tui --output-format json "${url}")"
        verify_all_200 "${json}" "${label} run ${i}"
        rps="$(echo "${json}" | jq -r '.summary.requestsPerSec')"
        p95="$(echo "${json}" | jq -r '.latencyPercentiles.p95')"
        rps_values+=("${rps}")
        p95_values+=("${p95}")
        echo "  [${label}] run ${i}: rps=${rps} p95=${p95}" >&2
    done
    echo "$(printf '%s\n' "${rps_values[@]}" | median) $(printf '%s\n' "${p95_values[@]}" | median)"
}

ratio() {
    LC_NUMERIC=C awk -v a="$1" -v b="$2" 'BEGIN { printf "%.2f", (a / b) * 100 }'
}

echo "=== JWT NFR-6 計測（RUNS=${RUNS} DURATION=${DURATION} CONNECTIONS=${CONNECTIONS}） ===" >&2

# --- 正常性確認（fail-closed）と、全系統で共通に使うトークンの取得 ---
JWT_URL="http://127.0.0.1:${JWT_PORT}/"
BASE_URL="http://127.0.0.1:${BASELINE_PORT}/"

start_server "${JWT_BIN}" "" "es256"
ES_TOKEN="$(read_token)"
wait_ready "${JWT_URL}" "${ES_TOKEN}" || exit 1
expect_code 200 "$(http_code "${JWT_URL}" "${ES_TOKEN}")" "ゲート有効・有効トークン付き GET /"
expect_code 401 "$(http_code "${JWT_URL}" "")" "ゲート有効・ヘッダ無し GET /（ゲートが効いている証跡）"
stop_server

# RS256 のトークンは署名が長くヘッダ転送量・解析コストが ES256 と異なるため、RS256 の
# baseline は RS256 トークンで別途計測する。ここで長さの代表となる RS_TOKEN0 を得る。
start_server "${JWT_BIN}" "" "rs256"
RS_TOKEN0="$(read_token)"
wait_ready "${JWT_URL}" "${RS_TOKEN0}" || exit 1
expect_code 200 "$(http_code "${JWT_URL}" "${RS_TOKEN0}")" "ゲート有効・RS256 有効トークン付き GET /"
stop_server

start_server "${JWT_BIN}" "off" "es256"
wait_ready "${JWT_URL}" "${ES_TOKEN}" || exit 1
expect_code 200 "$(http_code "${JWT_URL}" "${ES_TOKEN}")" "ゲート無効・トークン付き GET /"
stop_server

start_baseline
wait_ready "${BASE_URL}" "${ES_TOKEN}" || exit 1
expect_code 200 "$(http_code "${BASE_URL}" "${ES_TOKEN}")" "baseline・トークン付き GET /"
stop_server
echo "正常性確認 OK" >&2

# --- 計測 ---
start_baseline
wait_ready "${BASE_URL}" "${ES_TOKEN}" || exit 1
load_out="$(run_load "${BASELINE_PORT}" baseline "${ES_TOKEN}")" || exit 1
read -r baseline_rps baseline_p95 <<<"${load_out}"
stop_server

# RS256 比較用 baseline（RS256 トークンのヘッダを付けた minimal）
start_baseline
wait_ready "${BASE_URL}" "${RS_TOKEN0}" || exit 1
load_out="$(run_load "${BASELINE_PORT}" "baseline(RS256 token)" "${RS_TOKEN0}")" || exit 1
read -r rs_baseline_rps rs_baseline_p95 <<<"${load_out}"
stop_server

start_server "${JWT_BIN}" "off" "es256"
wait_ready "${JWT_URL}" "${ES_TOKEN}" || exit 1
load_out="$(run_load "${JWT_PORT}" "jwt(gate off)" "${ES_TOKEN}")" || exit 1
read -r off_rps off_p95 <<<"${load_out}"
stop_server

start_server "${JWT_BIN}" "" "es256"
ES_TOKEN="$(read_token)"
wait_ready "${JWT_URL}" "${ES_TOKEN}" || exit 1
load_out="$(run_load "${JWT_PORT}" "jwt(gate on, ES256)" "${ES_TOKEN}")" || exit 1
read -r es_rps es_p95 <<<"${load_out}"
stop_server

start_server "${JWT_BIN}" "" "rs256"
RS_TOKEN="$(read_token)"
wait_ready "${JWT_URL}" "${RS_TOKEN}" || exit 1
load_out="$(run_load "${JWT_PORT}" "jwt(gate on, RS256)" "${RS_TOKEN}")" || exit 1
read -r rs_rps rs_p95 <<<"${load_out}"
stop_server

es_rps_ratio="$(ratio "${es_rps}" "${baseline_rps}")"
es_p95_ratio="$(ratio "${es_p95}" "${baseline_p95}")"
off_rps_ratio="$(ratio "${off_rps}" "${baseline_rps}")"
off_p95_ratio="$(ratio "${off_p95}" "${baseline_p95}")"
rs_rps_ratio="$(ratio "${rs_rps}" "${rs_baseline_rps}")"
rs_p95_ratio="$(ratio "${rs_p95}" "${rs_baseline_p95}")"

es_verdict="$(evaluate_nfr6_ratio "${es_rps_ratio}" "${es_p95_ratio}")"
off_verdict="$(evaluate_nfr6_ratio "${off_rps_ratio}" "${off_p95_ratio}")"
rs_verdict="$(evaluate_nfr6_ratio "${rs_rps_ratio}" "${rs_p95_ratio}")"

{
    echo ""
    echo "=== 結果（中央値、対象: トークン付き GET /） ==="
    echo "baseline            RPS=${baseline_rps} p95=${baseline_p95}"
    echo "jwt ゲート無効       RPS=${off_rps}（${off_rps_ratio}%） p95=${off_p95}（${off_p95_ratio}%） 判定=${off_verdict}"
    echo "jwt ゲート有効 ES256 RPS=${es_rps}（${es_rps_ratio}%） p95=${es_p95}（${es_p95_ratio}%） 判定=${es_verdict}（主判定）"
    echo "baseline(RS256 token) RPS=${rs_baseline_rps} p95=${rs_baseline_p95}（RS256 の比較分母）"
    echo "jwt ゲート有効 RS256 RPS=${rs_rps}（${rs_rps_ratio}%） p95=${rs_p95}（${rs_p95_ratio}%） 判定=${rs_verdict}"
} >&2

# machine-readable な結果を stdout へ。`rps_ratio_pct=` / `p95_ratio_pct=` が主判定
# （ゲート有効 ES256 対 baseline）で、nfr6-exclusive.sh がこの 2 行を読む。
printf 'baseline_rps=%s\nbaseline_p95=%s\n' "${baseline_rps}" "${baseline_p95}"
printf 'gate_on_es256_rps=%s\ngate_on_es256_p95=%s\n' "${es_rps}" "${es_p95}"
printf 'rps_ratio_pct=%s\np95_ratio_pct=%s\nverdict=%s\n' "${es_rps_ratio}" "${es_p95_ratio}" "${es_verdict}"
printf 'gate_off_rps=%s\ngate_off_p95=%s\ngate_off_rps_ratio=%s\ngate_off_p95_ratio=%s\ngate_off_verdict=%s\n' \
    "${off_rps}" "${off_p95}" "${off_rps_ratio}" "${off_p95_ratio}" "${off_verdict}"
printf 'baseline_rs256_token_rps=%s\nbaseline_rs256_token_p95=%s\n' "${rs_baseline_rps}" "${rs_baseline_p95}"
printf 'gate_on_rs256_rps=%s\ngate_on_rs256_p95=%s\ngate_on_rs256_rps_ratio=%s\ngate_on_rs256_p95_ratio=%s\ngate_on_rs256_verdict=%s\n' \
    "${rs_rps}" "${rs_p95}" "${rs_rps_ratio}" "${rs_p95_ratio}" "${rs_verdict}"
