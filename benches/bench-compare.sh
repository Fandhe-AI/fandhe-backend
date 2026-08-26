#!/usr/bin/env bash
# 複数フレームワークの参照実装を同一パラメータで横並び計測し、比較表（Markdown）を出力する。
#
# このスクリプトの役割:
#   `crates/axum-ref`（axum）・`crates/core/examples/core-bench.rs`（fandhe-backend）・
#   `benches/refs/`（actix-web / Rocket）のように、同一 4 エンドポイント
#   （GET /health, GET /hello/{name}, GET /users/{id}, POST /echo）を提供する
#   計測対象バイナリを `NAME=BIN` の組で受け取り、`bench-http.sh` / `bench-rss.sh` /
#   `bench-footprint.sh` を **順に**（同時起動しない）実行して RPS・p50/p95/p99・
#   負荷時 RSS・アイドル RSS・バイナリサイズ・起動時間を収集する。
#
#   `bench-accept.sh`（REQ-1 / NFR-1 / NFR-2 の axum 比受け入れ判定）とは目的が異なり、
#   **PASS/FAIL 判定を持たない**。先頭に指定した対象を比率計算の基準（=1.00）として
#   相対値を併記するだけの情報提供用ハーネスであり、CI ゲートには組み込まない
#   （同一ホスト計測ノイズ、benches/README.md）。
#
# 呼び出し元: 開発者が手動実行する想定。サブスクリプトは `RESULT_JSON=<tmp>` 付きで
#   起動し、機械可読 JSON を jq で読む（stdout テキストのパースは行わない。
#   lib/common.sh の write_result_json 契約）。
#
# 使い方:
#   ./benches/bench-compare.sh axum=target/release/axum-ref \
#       fandhe-backend=target/release/examples/core-bench \
#       actix-web=benches/refs/target/release/actix-ref \
#       rocket=benches/refs/target/release/rocket-ref
#
# 環境変数:
#   RUNS / DURATION / CONNECTIONS  lib/common.sh の既定（5 / 15s / 128）を継承
#   REPORT_MD                      指定時、比較表（Markdown）をこのパスにも書き出す
#   RESULT_DIR                     各対象の RESULT_JSON 保存先（既定: mktemp -d）
#   BASE_PORT                      対象ごとに BASE_PORT+i を割り当てる（既定 3201。直前の
#                                  計測の TIME_WAIT 残留と衝突しないようポートを分ける）
#   SETTLE_SECS                    対象の切り替え間の待機秒数（既定 5）

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

REPORT_MD="${REPORT_MD:-}"
RESULT_DIR="${RESULT_DIR:-$(mktemp -d)}"
BASE_PORT="${BASE_PORT:-3201}"
SETTLE_SECS="${SETTLE_SECS:-5}"
validate_integer "${BASE_PORT}" "BASE_PORT"
validate_integer "${SETTLE_SECS}" "SETTLE_SECS"

if [ "$#" -lt 2 ]; then
    echo "使い方: $0 NAME=BIN NAME=BIN [NAME=BIN ...]（先頭が比率の基準）" >&2
    exit 1
fi

check_dependencies
check_runs_minimum
mkdir -p "${RESULT_DIR}"

NAMES=()
BINS=()
for pair in "$@"; do
    case "${pair}" in
        *=*) ;;
        *)
            echo "エラー: 引数は NAME=BIN 形式である必要があります（現在: ${pair}）" >&2
            exit 1
            ;;
    esac
    name="${pair%%=*}"
    bin="${pair#*=}"
    # NAME は表・ファイル名に使うため英数字・ハイフン・ドット・アンダースコアに限定する
    # （jq / Markdown へのインジェクション防止、.claude/rules/security.md）。
    if ! [[ "${name}" =~ ^[A-Za-z0-9._-]+$ ]]; then
        echo "エラー: NAME は英数字・'.'・'_'・'-' のみ使用できます（現在: ${name}）" >&2
        exit 1
    fi
    # NAME は RESULT_DIR 内の JSON ファイル名（`<NAME>-http.json` 等）と比較表の行キーを
    # 兼ねるため一意でなければならない。重複を許すと後続の計測が先行の JSON を上書きし、
    # 表生成時に両行が最後のバイナリの結果を読んで「正常終了したまま誤った比較表」に
    # なる（先頭 NAME が重複すると比率の基準値まで別バイナリへ置き換わる）。
    # フェイルクローズで即時エラー終了する（PR #651 codex-review P1 対応。macOS 既定の
    # bash 3.2 には連想配列がないため配列走査で検査する）。
    for existing in "${NAMES[@]+"${NAMES[@]}"}"; do
        if [ "${existing}" = "${name}" ]; then
            echo "エラー: NAME '${name}' が重複しています。対象ごとに一意の NAME を指定してください" >&2
            exit 1
        fi
    done
    case "${bin}" in
        /*) ;;
        *) bin="${WORKSPACE_ROOT}/${bin}" ;;
    esac
    if [ ! -x "${bin}" ]; then
        echo "エラー: ${bin} が見つからないか実行できません（${name}）。先に release ビルドしてください" >&2
        exit 1
    fi
    NAMES+=("${name}")
    BINS+=("${bin}")
done

echo "# bench-compare.sh（RUNS=${RUNS} DURATION=${DURATION} CONNECTIONS=${CONNECTIONS}）"
echo "結果 JSON: ${RESULT_DIR}"

# 各対象を順に計測する。lib/common.sh は本スクリプト自身の source 時に TARGET_URL を
# 既定ポートで確定させ環境へ載せるため、サブスクリプトには TARGET_PORT と整合する
# TARGET_URL を明示的に渡す（start_server の不整合検査に合わせる）。
for idx in "${!NAMES[@]}"; do
    name="${NAMES[${idx}]}"
    bin="${BINS[${idx}]}"
    port=$((BASE_PORT + idx))
    echo ""
    echo "## [${name}] ${bin}（port ${port}）"
    TARGET_BIN="${bin}" TARGET_PORT="${port}" TARGET_URL="http://${TARGET_HOST}:${port}" \
        RESULT_JSON="${RESULT_DIR}/${name}-http.json" \
        "${SCRIPT_DIR}/bench-http.sh"
    sleep "${SETTLE_SECS}"
    TARGET_BIN="${bin}" TARGET_PORT="${port}" TARGET_URL="http://${TARGET_HOST}:${port}" \
        RESULT_JSON="${RESULT_DIR}/${name}-rss.json" \
        "${SCRIPT_DIR}/bench-rss.sh"
    sleep "${SETTLE_SECS}"
    TARGET_BIN="${bin}" TARGET_PORT="${port}" TARGET_URL="http://${TARGET_HOST}:${port}" \
        RESULT_JSON="${RESULT_DIR}/${name}-footprint.json" \
        "${SCRIPT_DIR}/bench-footprint.sh"
    sleep "${SETTLE_SECS}"
done

# --- 比較表の生成 ---
# 先頭対象を基準（=1.00）に RPS 比・p99 比を算出する。レイテンシは oha の秒単位を ms に変換。
base="${NAMES[0]}"
md=""
md+="### 計測環境"$'\n\n'
md+="- 実施日時: $(date -u '+%Y-%m-%d %H:%M UTC')"$'\n'
md+="- OS: $(uname -srm)"$'\n'
if command -v nproc >/dev/null 2>&1; then
    md+="- CPU コア数: $(nproc)"$'\n'
elif command -v sysctl >/dev/null 2>&1; then
    md+="- CPU: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown)（論理コア $(sysctl -n hw.ncpu 2>/dev/null || echo ?)）"$'\n'
fi
md+="- rustc: $(rustc -V 2>/dev/null || echo unknown) / oha: $(oha --version 2>/dev/null || echo unknown)"$'\n'
md+="- 計測パラメータ: \`RUNS=${RUNS} DURATION=${DURATION} CONNECTIONS=${CONNECTIONS}\`（各値は ${RUNS} 回計測の中央値。比率の基準は ${base}）"$'\n\n'

endpoint_count="$(jq '.endpoints | length' "${RESULT_DIR}/${base}-http.json")"
for ((e = 0; e < endpoint_count; e++)); do
    label="$(jq -r ".endpoints[${e}].label" "${RESULT_DIR}/${base}-http.json")"
    base_rps="$(jq -r ".endpoints[${e}].rps.median" "${RESULT_DIR}/${base}-http.json")"
    base_p99="$(jq -r ".endpoints[${e}].p99.median" "${RESULT_DIR}/${base}-http.json")"
    md+="### ${label}"$'\n\n'
    md+="| フレームワーク | RPS | p50 (ms) | p95 (ms) | p99 (ms) | RPS 比 | p99 比 |"$'\n'
    md+="| --- | ---: | ---: | ---: | ---: | ---: | ---: |"$'\n'
    for name in "${NAMES[@]}"; do
        row="$(jq -r --arg e "${e}" --argjson base_rps "${base_rps}" --argjson base_p99 "${base_p99}" --arg name "${name}" '
            .endpoints[($e | tonumber)] as $ep
            | "| \($name) | \($ep.rps.median | floor) | \($ep.p50.median * 1000 | . * 1000 | round / 1000) | \($ep.p95.median * 1000 | . * 1000 | round / 1000) | \($ep.p99.median * 1000 | . * 1000 | round / 1000) | \($ep.rps.median / $base_rps | . * 100 | round / 100) | \($ep.p99.median / $base_p99 | . * 100 | round / 100) |"
        ' "${RESULT_DIR}/${name}-http.json")"
        md+="${row}"$'\n'
    done
    md+=$'\n'
done

md+="### フットプリント"$'\n\n'
md+="| フレームワーク | アイドル RSS (KB) | 負荷時 RSS (KB) | バイナリサイズ (bytes) | 起動時間 (ms) | バイナリ比 | 負荷時 RSS 比 |"$'\n'
md+="| --- | ---: | ---: | ---: | ---: | ---: | ---: |"$'\n'
base_bin="$(jq -r '.binary_size_bytes' "${RESULT_DIR}/${base}-footprint.json")"
base_load="$(jq -r '.load_rss_kb_median' "${RESULT_DIR}/${base}-rss.json")"
for name in "${NAMES[@]}"; do
    idle="$(jq -r '.idle_rss_kb.median' "${RESULT_DIR}/${name}-footprint.json")"
    load="$(jq -r '.load_rss_kb_median' "${RESULT_DIR}/${name}-rss.json")"
    size="$(jq -r '.binary_size_bytes' "${RESULT_DIR}/${name}-footprint.json")"
    startup="$(jq -r '.startup_ms.median' "${RESULT_DIR}/${name}-footprint.json")"
    ratio_bin="$(jq -n --argjson a "${size}" --argjson b "${base_bin}" '$a / $b | . * 100 | round / 100')"
    ratio_load="$(jq -n --argjson a "${load}" --argjson b "${base_load}" '$a / $b | . * 100 | round / 100')"
    md+="| ${name} | ${idle} | ${load} | ${size} | ${startup} | ${ratio_bin} | ${ratio_load} |"$'\n'
done

echo ""
echo "${md}"
if [ -n "${REPORT_MD}" ]; then
    printf '%s\n' "${md}" >"${REPORT_MD}"
    echo "比較表を ${REPORT_MD} に書き出しました"
fi
