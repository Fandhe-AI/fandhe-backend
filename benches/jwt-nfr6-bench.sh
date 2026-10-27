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
# DPoP 系列（イシュー #906、opt-in: `JWT_DPOP=1`）:
#   系列 (d)「ゲート有効 + DPoP（`DpopMode::AllowUnboundBearer`）+ `cnf` のない ES256 トークンを
#   Bearer で提示」を追加する。DPoP を有効にしたゲートを Bearer の通信が通るコスト（スキームと
#   `cnf` の検査のみ、proof なし）に当たる。比は (a) baseline と (c) ES256 ゲート有効の両方を分母に
#   して出し、主判定のキー（`rps_ratio_pct=` ほか）は変えない。
#   **`DpopMode::Required` で要求ごとに proof を付ける経路は、HTTP では計測しない**。`oha` は固定
#   ヘッダしか送れず、2 回目以降は同じ proof が `jti` の再使用検知で 401 になる（検知を無効にする
#   設定をライブラリへ足すのは安全性の後退のため行わない）。その経路はプロセス内の
#   `crates/plugin-jwt/examples/jwt_dpop_bench.rs` で計測する。
#   前提の追加ビルド: `cargo build --release -p fandhe-backend-plugin-jwt --features dpop --example jwt_gate_server`
#   （`dpop` feature なしのバイナリでは起動時にエラーで終了し、中止する）
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
# 1 のとき DPoP 系列 (d) を追加で計測する（#906）。既定は 0 で、従来の出力を変えない。
JWT_DPOP="${JWT_DPOP:-0}"

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
# 引数: $1 バイナリ、$2 FANDHE_BACKEND_JWT_GATE の値（"" なら未設定）、$3 ALG（"" なら既定）、
#       $4 FANDHE_BACKEND_JWT_BENCH_DPOP の値（省略・"" なら未設定。#906）
start_server() {
    local bin="$1" gate="$2" alg="$3" dpop="${4-}"
    assert_port_free "${JWT_PORT}"
    : >"${TOKEN_FILE}"
    (
        if [ -n "${gate}" ]; then export FANDHE_BACKEND_JWT_GATE="${gate}"; fi
        if [ -n "${alg}" ]; then export FANDHE_BACKEND_JWT_BENCH_ALG="${alg}"; fi
        if [ -n "${dpop}" ]; then export FANDHE_BACKEND_JWT_BENCH_DPOP="${dpop}"; fi
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

    # `-w`: oha 1.14 は HTTP/1 で期限到達時に実行中の要求を "aborted due to deadline" として
    # errorDistribution へ数え、下の verify_all_200 が誤って計測失敗にする。期限後も実行中の
    # 要求の完了を待たせ、通信失敗だけを失敗として扱う（#906）。
    oha -w -z 2s -c "${CONNECTIONS}" -H "${hdr}" --no-tui --output-format json "${url}" >/dev/null 2>&1 || true

    local rps_values=() p95_values=()
    local i json rps p95
    for ((i = 1; i <= RUNS; i++)); do
        json="$(oha -w -z "${DURATION}" -c "${CONNECTIONS}" -H "${hdr}" --no-tui --output-format json "${url}")"
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

if [ "${JWT_DPOP}" = "1" ]; then
    # allow-unbound を受け付けないバイナリ（dpop feature なし）は起動時に終了し、wait_ready が
    # それを検出して中止する。
    start_server "${JWT_BIN}" "" "es256" "allow-unbound"
    DPOP_TOKEN="$(read_token)"
    wait_ready "${JWT_URL}" "${DPOP_TOKEN}" || exit 1
    expect_code 200 "$(http_code "${JWT_URL}" "${DPOP_TOKEN}")" "DPoP(AllowUnboundBearer)・cnf なしトークン付き GET /"
    expect_code 401 "$(http_code "${JWT_URL}" "")" "DPoP(AllowUnboundBearer)・ヘッダ無し GET /（ゲートが効いている証跡）"
    stop_server
fi
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

dpop_rps="" dpop_p95=""
if [ "${JWT_DPOP}" = "1" ]; then
    start_server "${JWT_BIN}" "" "es256" "allow-unbound"
    DPOP_TOKEN="$(read_token)"
    wait_ready "${JWT_URL}" "${DPOP_TOKEN}" || exit 1
    load_out="$(run_load "${JWT_PORT}" "jwt(gate on, DPoP AllowUnboundBearer, ES256)" "${DPOP_TOKEN}")" || exit 1
    read -r dpop_rps dpop_p95 <<<"${load_out}"
    stop_server
fi

es_rps_ratio="$(ratio "${es_rps}" "${baseline_rps}")"
es_p95_ratio="$(ratio "${es_p95}" "${baseline_p95}")"
off_rps_ratio="$(ratio "${off_rps}" "${baseline_rps}")"
off_p95_ratio="$(ratio "${off_p95}" "${baseline_p95}")"
rs_rps_ratio="$(ratio "${rs_rps}" "${rs_baseline_rps}")"
rs_p95_ratio="$(ratio "${rs_p95}" "${rs_baseline_p95}")"

es_verdict="$(evaluate_nfr6_ratio "${es_rps_ratio}" "${es_p95_ratio}")"
off_verdict="$(evaluate_nfr6_ratio "${off_rps_ratio}" "${off_p95_ratio}")"
rs_verdict="$(evaluate_nfr6_ratio "${rs_rps_ratio}" "${rs_p95_ratio}")"

if [ "${JWT_DPOP}" = "1" ]; then
    # 分母は (a) baseline と (c) ES256 ゲート有効（DPoP なし）の 2 通り。
    dpop_rps_ratio="$(ratio "${dpop_rps}" "${baseline_rps}")"
    dpop_p95_ratio="$(ratio "${dpop_p95}" "${baseline_p95}")"
    dpop_vs_gate_rps_ratio="$(ratio "${dpop_rps}" "${es_rps}")"
    dpop_vs_gate_p95_ratio="$(ratio "${dpop_p95}" "${es_p95}")"
    dpop_verdict="$(evaluate_nfr6_ratio "${dpop_rps_ratio}" "${dpop_p95_ratio}")"
fi

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

# DPoP 系列 (d)（#906、JWT_DPOP=1 のときだけ。主判定のキー集合は変えない）。
if [ "${JWT_DPOP}" = "1" ]; then
    echo "jwt ゲート有効 DPoP(AllowUnboundBearer) ES256 RPS=${dpop_rps}（対 baseline ${dpop_rps_ratio}%、対 ゲート有効 ${dpop_vs_gate_rps_ratio}%） p95=${dpop_p95}（対 baseline ${dpop_p95_ratio}%、対 ゲート有効 ${dpop_vs_gate_p95_ratio}%） 判定=${dpop_verdict}" >&2
    printf 'gate_on_dpop_unbound_rps=%s\ngate_on_dpop_unbound_p95=%s\n' "${dpop_rps}" "${dpop_p95}"
    printf 'gate_on_dpop_unbound_rps_ratio=%s\ngate_on_dpop_unbound_p95_ratio=%s\ngate_on_dpop_unbound_verdict=%s\n' \
        "${dpop_rps_ratio}" "${dpop_p95_ratio}" "${dpop_verdict}"
    printf 'gate_on_dpop_unbound_vs_gate_rps_ratio=%s\ngate_on_dpop_unbound_vs_gate_p95_ratio=%s\n' \
        "${dpop_vs_gate_rps_ratio}" "${dpop_vs_gate_p95_ratio}"
fi
