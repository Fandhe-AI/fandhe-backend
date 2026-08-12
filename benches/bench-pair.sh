#!/usr/bin/env bash
# 交互ペア測定による二次判定エントリポイント（イシュー #613）。
#
# このスクリプトの役割:
#   `bench-accept.sh`（一次判定、axum-ref 比のしきい値・exit 0/1/2 契約）とは
#   別経路の新設スクリプト。`BIN_A`（pre）・`BIN_B`（cur）2 バイナリを
#   `benches/lib/interleave.sh` で `PAIRS` 回交互セッション計測し、エンドポイント
#   ごとの cur/pre 比の採用ペア中央値が `PAIR_M2` 以内かを判定する
#   （`docs/design/bench-p95-criteria.md` 5.2 節の二次判定基準）。
#
#   一次判定（`bench-accept.sh`）が FAIL または境界に近い場合の**退行帰属手段**
#   として使う想定（`docs/design/bench-hosted-runner.md`）。新設経路のため
#   一次判定の互換制約を持たず、本スクリプトで初めて INCONCLUSIVE（exit 3）を
#   導入する。
#
# 汚染窓（外部占有率 > `EXT_CPU_MAX_PCT`）を含むペアは採用から除外し、除外
# 理由・生値を必ず記録する（`docs/design/bench-p95-criteria.md` 6.1 節、
# silent drop 禁止）。
#
# 終了コード:
#   0 = PASS（全エンドポイントで採用ペア中央値 cur/pre <= 1 + PAIR_M2）
#   1 = FAIL（1 エンドポイント以上で超過）
#   2 = BLOCKED（BIN_A/BIN_B 未整備・依存ツール欠如等の決定論的失敗）
#   3 = INCONCLUSIVE（1 エンドポイント以上で採用ペア数が PAIR_MIN_PAIRS 未満。
#       #612 5.2 節。`nfr6_run_with_fail_retry` 等の 0/1/2 前提の呼び出し元へは
#       まだ接続しない — `bench-schedule.yml` への接続は既存イシュー #614 の
#       スコープ）
#
# 使い方・env 一覧は benches/README.md を参照。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
# shellcheck source=lib/interleave.sh
source "${SCRIPT_DIR}/lib/interleave.sh"

BIN_A="${BIN_A:-}"
BIN_B="${BIN_B:-}"
PORT_A="${PORT_A:-3111}"
PORT_B="${PORT_B:-3112}"
REPORT_MD="${REPORT_MD:-}"

if [ -z "${BIN_A}" ] || [ -z "${BIN_B}" ]; then
    echo "エラー: BIN_A・BIN_B の両方を指定してください（pre/cur の 2 バイナリ）" >&2
    exit 2
fi
if [ ! -x "${BIN_A}" ]; then
    echo "## 判定結果: BLOCKED" >&2
    echo "BIN_A（${BIN_A}）が見つかりません" >&2
    exit 2
fi
if [ ! -x "${BIN_B}" ]; then
    echo "## 判定結果: BLOCKED" >&2
    echo "BIN_B（${BIN_B}）が見つかりません" >&2
    exit 2
fi

check_dependencies
check_runs_minimum

echo "# bench-pair.sh: 交互ペア測定（二次判定）"
echo "実行日時: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
echo "パラメータ: RUNS=${RUNS} DURATION=${DURATION} CONNECTIONS=${CONNECTIONS} PAIRS=${PAIRS} PAIR_M2=${PAIR_M2} PAIR_MIN_PAIRS=${PAIR_MIN_PAIRS}"
echo "A（pre）: ${BIN_A}（127.0.0.1:${PORT_A}）"
echo "B（cur）: ${BIN_B}（127.0.0.1:${PORT_B}）"
echo

PAIR_DIR="$(mktemp -d)"
cleanup_pair_dir() {
    rm -rf "${PAIR_DIR}"
}
trap cleanup_pair_dir EXIT

# セッション実行（`bench-http.sh` 委譲）の失敗は、ポート衝突・バイナリ未整備・
# サーバ起動失敗等の決定論的な環境失敗であり、性能退行 FAIL（exit 1）とは
# 区別して BLOCKED（exit 2）として扱う。`set -e` 下では素通しすると
# `interleave_run_pairs` → `interleave_run_session` → `bench-http.sh` の exit 1
# がそのまま本スクリプトの exit 1 になり「性能退行」と誤認されてしまう
# （`bench-accept.sh` が #478/#479 で修正した同種の誤分類を再発させないための
# 対処）。
if ! interleave_run_pairs "${BIN_A}" "${PORT_A}" "${BIN_B}" "${PORT_B}" "${PAIR_DIR}"; then
    echo "## 判定結果: BLOCKED" >&2
    echo "交互ペア測定のセッション実行に失敗しました（ポート衝突・サーバ起動失敗等の決定論的失敗として BLOCKED 扱い。exit 1 の性能 FAIL とは区別する）" >&2
    exit 2
fi
if [ ! -f "${PAIR_DIR}/a-1.json" ]; then
    echo "## 判定結果: BLOCKED" >&2
    echo "交互ペア測定の結果 JSON（${PAIR_DIR}/a-1.json）が見つかりません" >&2
    exit 2
fi

# --- エンドポイントごとに採用/除外ペアを分類し、cur/pre 比を算出する ---
#
# 汚染フラグ（CPU_PROBE=1 のときのみ RESULT_JSON に含まれる `cpu_probe.
# contaminated`）が立っている窓を含むセッションは、そのセッション全体
# （全エンドポイント）を当該ペアから除外する。CPU_PROBE 未指定時（既定）は
# 汚染判定情報が存在しないため全ペアを採用する（既定は現行相当の挙動）。
endpoint_count="$(jq '.endpoints | length' "${PAIR_DIR}/a-1.json")"

OVERALL_EXIT=0
declare -a PAIR_ROWS=()

for ((e = 0; e < endpoint_count; e++)); do
    label="$(jq -r ".endpoints[${e}].label" "${PAIR_DIR}/a-1.json")"
    ratios=()
    excluded_count=0
    excluded_detail=()

    for ((p = 1; p <= PAIRS; p++)); do
        a_json="${PAIR_DIR}/a-${p}.json"
        b_json="${PAIR_DIR}/b-${p}.json"

        # セッション内のいずれかの窓が汚染されていれば、このペアを除外する
        # （CPU_PROBE 未指定時は cpu_probe フィールド自体が存在せず `//empty`
        # で 0 件になるため、常に非汚染扱い＝全ペア採用の既定挙動になる）。
        a_contaminated="$(jq '[.endpoints[].cpu_probe.contaminated[]? // empty] | add // 0' "${a_json}")"
        b_contaminated="$(jq '[.endpoints[].cpu_probe.contaminated[]? // empty] | add // 0' "${b_json}")"

        pre="$(jq -r ".endpoints[${e}].p95.median" "${a_json}")"
        cur="$(jq -r ".endpoints[${e}].p95.median" "${b_json}")"

        if [ "${a_contaminated}" != "0" ] || [ "${b_contaminated}" != "0" ]; then
            excluded_count=$((excluded_count + 1))
            excluded_detail+=("pair=${p} pre=${pre} cur=${cur}（汚染窓を含むため除外）")
            continue
        fi

        ratio="$(LC_NUMERIC=C awk -v c="${cur}" -v p="${pre}" 'BEGIN { if (p == 0) { print "nan" } else { printf "%.6f", c / p } }')"
        if [ "${ratio}" != "nan" ]; then
            ratios+=("${ratio}")
        else
            excluded_count=$((excluded_count + 1))
            excluded_detail+=("pair=${p} pre=${pre} cur=${cur}（pre=0 のため比率計算不能で除外）")
        fi
    done

    verdict="$(interleave_pair_verdict "${PAIR_M2}" "${PAIR_MIN_PAIRS}" "${ratios[*]:-}")"
    case "${verdict}" in
        FAIL) OVERALL_EXIT=1 ;;
        INCONCLUSIVE) [ "${OVERALL_EXIT}" -eq 0 ] && OVERALL_EXIT=3 ;;
    esac

    adopted_count="${#ratios[@]}"
    echo "## ${label}"
    echo "採用ペア: ${adopted_count}/${PAIRS}（cur/pre 比 p95: ${ratios[*]:-なし}）"
    if [ "${excluded_count}" -gt 0 ]; then
        echo "除外ペア: ${excluded_count} 件"
        for detail in "${excluded_detail[@]}"; do
            echo "  - ${detail}"
        done
    fi
    echo "判定: ${verdict}"
    echo
    PAIR_ROWS+=("${label}|${adopted_count}|${excluded_count}|${verdict}")
done

echo "## 総合判定: $(
    case "${OVERALL_EXIT}" in
        0) echo "PASS" ;;
        1) echo "FAIL" ;;
        3) echo "INCONCLUSIVE" ;;
    esac
)"

if [ -n "${REPORT_MD}" ]; then
    {
        echo
        echo "## bench-pair.sh 判定表（$(date -u '+%Y-%m-%dT%H:%M:%SZ')）"
        echo
        echo "| エンドポイント | 採用ペア | 除外ペア | 判定 |"
        echo "|------|------|------|------|"
        for row in "${PAIR_ROWS[@]}"; do
            IFS='|' read -r label adopted excluded verdict <<<"${row}"
            echo "| ${label} | ${adopted}/${PAIRS} | ${excluded} | ${verdict} |"
        done
        echo
        echo "**総合判定: $(
            case "${OVERALL_EXIT}" in
                0) echo "PASS" ;;
                1) echo "FAIL" ;;
                3) echo "INCONCLUSIVE" ;;
            esac
        )**"
    } >>"${REPORT_MD}"
fi

exit "${OVERALL_EXIT}"
