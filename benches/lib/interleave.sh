#!/usr/bin/env bash
# 交互（interleave）ペア測定エンジン（イシュー #613）。
#
# このライブラリの役割:
#   before/after（baseline/core、または A/B 2 バイナリ）を「セッション単位で
#   交互に」計測することで、順序効果・時間帯ドリフトを排除する（背景・実証は
#   `benches/reports/issue593-p1-zero-copy-bench.md` 9.7 節、
#   `docs/design/bench-hosted-runner.md`）。
#
#   呼び出し元:
#     - `benches/bench-accept.sh`（`INTERLEAVE=1` opt-in。既存の axum 比一次
#       判定を baseline/core 交互セッションへ切り替える。判定ロジック自体は
#       無変更）
#     - `benches/bench-pair.sh`（新設の二次判定エントリポイント。本ファイルの
#       `interleave_pair_verdict` で #612 5.2 節の M2 判定を行う）
#
#   セッション本体の計測（oha 実行・中央値算出・RESULT_JSON 出力）は
#   `benches/bench-http.sh` をサブプロセスとして呼び出すことで再利用する
#   （測定ロジックの二重実装を避け、`CPU_PROBE=1` 統合・RESULT_JSON スキーマを
#   1 箇所に保つ）。本ファイル自身は「どの順序で・どのポートで・何回」
#   セッションを回すかのオーケストレーションと、二次判定の純関数のみを持つ。
#
# セルフテスト: `scripts/tests/run-bench-cpu-probe-tests.sh` が
# `interleave_remeasure_window` / `interleave_pair_verdict`（副作用のない純粋な
# 判定ロジック）のみを対象に検証する。`interleave_run_session` /
# `interleave_run_pairs`（実サーバ起動・oha 実行を伴う）はセルフテスト対象外
# （`benches/lib/exclusive.sh` と同じ「副作用のある呼び出し元本体は対象に
# しない」方針）。
#
# 単体では実行しない（関数定義のみ、副作用なし）。

set -uo pipefail

INTERLEAVE_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# CPU_PROBE のしきい値・再計測上限 env（EXT_CPU_MAX_PCT・WINDOW_REMEASURE_MAX）と
# 汚染判定関数（probe_is_contaminated）を利用するため source する。
# shellcheck source=cpu-probe.sh
# shellcheck source=cpu-probe.sh
source "${INTERLEAVE_LIB_DIR}/cpu-probe.sh"

# 交互測定のペア数（既定 8、`benches/reports/issue593-p1-zero-copy-bench.md`
# 9.7 節の実証値）。env で上書き可能。
PAIRS="${PAIRS:-8}"
if ! [[ "${PAIRS}" =~ ^[0-9]+$ ]] || [ "${PAIRS}" -lt 1 ]; then
    echo "エラー: PAIRS は 1 以上の整数である必要があります（現在: ${PAIRS}）" >&2
    exit 1
fi

# 二次判定（#612 5.2 節）のしきい値。しきい値は #616 で実測較正するまでの暫定値。
PAIR_M2="${PAIR_M2:-0.05}"
_cpu_probe_validate_numeric "${PAIR_M2}" "PAIR_M2"
PAIR_MIN_PAIRS="${PAIR_MIN_PAIRS:-6}"
if ! [[ "${PAIR_MIN_PAIRS}" =~ ^[0-9]+$ ]] || [ "${PAIR_MIN_PAIRS}" -lt 1 ]; then
    echo "エラー: PAIR_MIN_PAIRS は 1 以上の整数である必要があります（現在: ${PAIR_MIN_PAIRS}）" >&2
    exit 1
fi

# 1 窓の計測を汚染判定込みで実行し、上限まで有界に再計測する。
#
# 引数: $1 測定コマンド（関数名または実行ファイル）。呼ぶと標準出力の先頭
#       トークンとして外部占有率（%、`probe_external_share` の出力または
#       "nan"）を返す契約（残りのトークンは測定結果本体、呼び出し元がそのまま
#       扱う）。$2 以降は測定コマンドへそのまま渡す追加引数。
# 標準出力: "<測定コマンドの生出力> contaminated=<0|1> remeasure_count=<N>"
#           （汚染フラグ付きで必ず出力する。上限まで汚染が続いても値を
#           捏造せず・drop せず、最後の測定結果をそのまま契約どおり返す
#           — #612 6.1 節の silent drop 禁止）。
# 有界性: 初回計測 + 最大 `WINDOW_REMEASURE_MAX` 回の再計測で必ず終了する
#         （無限リトライにしない、フェイルクローズ設計）。
interleave_remeasure_window() {
    local measure_cmd="$1"
    shift || true
    local remeasure_count=0
    local output share
    output="$("${measure_cmd}" "$@")"
    share="$(echo "${output}" | awk '{print $1}')"
    while probe_is_contaminated "${share}" && [ "${remeasure_count}" -lt "${WINDOW_REMEASURE_MAX}" ]; do
        remeasure_count=$((remeasure_count + 1))
        echo "窓を再計測します（${remeasure_count}/${WINDOW_REMEASURE_MAX}、外部占有率=${share}%）" >&2
        output="$("${measure_cmd}" "$@")"
        share="$(echo "${output}" | awk '{print $1}')"
    done
    if probe_is_contaminated "${share}"; then
        echo "${output} contaminated=1 remeasure_count=${remeasure_count}"
    else
        echo "${output} contaminated=0 remeasure_count=${remeasure_count}"
    fi
}

# 採用ペア（cur/pre 比の集合）から二次判定（#612 5.2 節）を行う純関数。
#
# 引数: $1 M2（しきい値、例 0.05） $2 最低採用ペア数 $3 空白区切りの cur/pre 比の列
#       （汚染フラグ付きで除外済みのペアのみを渡すこと。除外自体はこの関数の
#       責務外 — 呼び出し元 `bench-pair.sh` が除外理由・生値を記録した上で
#       本関数には採用ペアのみを渡す）
# 標準出力: "PASS" | "FAIL" | "INCONCLUSIVE"
#   - 採用ペア数が最低数未満: INCONCLUSIVE（#612 5.2 節。exit 3 は呼び出し元が
#     この文字列を見て決める契約とし、本関数は終了コードを持たない）
#   - 中央値 cur/pre <= 1 + M2: PASS（改善方向 cur/pre < 1 も PASS 側）
#   - それ以外: FAIL
interleave_pair_verdict() {
    local m2="$1" min_pairs="$2"
    shift 2
    local values="$*"
    local count
    count="$(echo "${values}" | wc -w | tr -d ' ')"
    if [ "${count}" -lt "${min_pairs}" ]; then
        echo "INCONCLUSIVE"
        return 0
    fi
    local median
    # shellcheck disable=SC2086 # ${values} は空白区切りの数値列を意図的に
    # word splitting させて 1 行 1 値へ展開する（glob 対象文字は含まない数値のみ）。
    median="$(printf '%s\n' ${values} | LC_NUMERIC=C sort -n | LC_NUMERIC=C awk '
        { a[NR] = $1 }
        END {
            if (NR % 2 == 1) { print a[(NR + 1) / 2] }
            else { print (a[NR / 2] + a[NR / 2 + 1]) / 2 }
        }
    ')"
    if LC_NUMERIC=C awk -v m="${median}" -v m2="${m2}" 'BEGIN { exit !(m <= 1 + m2) }'; then
        echo "PASS"
    else
        echo "FAIL"
    fi
}

# 1 セッション（1 バイナリの RUNS 回計測）を `benches/bench-http.sh` へ委譲して
# 実行する。
#
# 引数: $1 TARGET_BIN（release バイナリのパス）$2 TARGET_PORT $3 RESULT_JSON 出力先
# 環境変数: RUNS・DURATION・CONNECTIONS・CPU_PROBE（すべて呼び出し元の値を
#           そのまま bench-http.sh へ引き継ぐ）
# 戻り値: bench-http.sh の終了コードをそのまま返す。
#
# ポートを引数で明示することで、A/B 2 バイナリのセッションが同一ホスト上で
# ポート衝突せずに逐次実行できる（同時実行はしない。1 セッション = 1 サーバ
# 起動〜停止のライフサイクルを `bench-http.sh` の `trap stop_server EXIT` が
# 保証するため、逐次実行であれば安全）。
interleave_run_session() {
    local target_bin="$1" target_port="$2" result_json="$3"
    TARGET_BIN="${target_bin}" TARGET_PORT="${target_port}" \
        TARGET_URL="http://127.0.0.1:${target_port}" \
        RESULT_JSON="${result_json}" \
        bash "${INTERLEAVE_LIB_DIR}/../bench-http.sh" >/dev/null
}

# A/B 2 バイナリを `PAIRS` 回、A→B の順で交互にセッション計測する。
#
# 引数: $1 BIN_A $2 PORT_A $3 BIN_B $4 PORT_B $5 出力先ディレクトリ
#       $6 quiesce_gate_fn（省略可、既定空文字＝無効）。空でなければ**各ペア
#       開始直前**（A セッション開始前、i = 1..PAIRS 各回に 1 回）に「関数名」
#       として呼び出す（呼び出しシグネチャ: `"${quiesce_gate_fn}" "<label>"`。
#       呼び出し元スクリプトが `source` 済みの同一シェル内関数を渡す想定 —
#       本ファイルはサブシェルを起こさないため、呼び出し元定義の関数がそのまま
#       可視）。B セッション直前には呼ばない: `wait_for_quiescence`
#       （`lib/exclusive.sh`）は 1 分間 loadavg を見るため、直前に完走した
#       自分自身の A セッション負荷の残差を「外部汚染」と誤検知しうる
#       （A/B 間隔を空けずに毎セッション判定すると、この自己負荷の減衰待ちで
#       頻繁に `SECTION_QUIESCE_WAIT_SECS` 消費・誤 BLOCKED を招くおそれが
#       ある）。ペア単位（PAIRS 回、既定 8）ならこのリスクを抑えつつ、区間
#       開始前 1 回だけでは検出できない複数ペアにまたがるドリフト・汚染を
#       検出できる。静穏未達時に当該関数が `exit 2` 等でプロセスを終了させる
#       契約は呼び出し元（`bench-accept.sh` の `run_section_quiescence_gate`）
#       が持ち、本関数はその呼び出しタイミングの提供にのみ責務を限定する
#       （イシュー #613 P1 レビュー指摘対応。`SECTION_QUIESCENCE=1` +
#       `INTERLEAVE=1` 併用時、baseline/core 各区間開始前 1 回だけでは
#       最大 PAIRS 回（既定 8）にわたるドリフト・汚染を検出できないため、
#       ペア単位の静穏再確認フックを追加した）。
# 出力: `<out_dir>/a-<i>.json` / `<out_dir>/b-<i>.json`（i = 1..PAIRS、
#       `bench-http.sh` の RESULT_JSON スキーマそのまま）
# 呼び出し元（`bench-pair.sh` / `bench-accept.sh` の INTERLEAVE モード）が
# 生成された JSON ペアを読み、エンドポイントごとの cur/pre 比を算出する。
interleave_run_pairs() {
    local bin_a="$1" port_a="$2" bin_b="$3" port_b="$4" out_dir="$5"
    local quiesce_gate_fn="${6:-}"
    mkdir -p "${out_dir}"
    local i
    for ((i = 1; i <= PAIRS; i++)); do
        if [ -n "${quiesce_gate_fn}" ]; then
            "${quiesce_gate_fn}" "interleave-pair${i}"
        fi
        echo "# 交互ペア ${i}/${PAIRS}: A セッション" >&2
        # 各セッションの終了コードを個別に検査する（`set -e` を持たない本
        # ファイルでは、ループ末尾以外の失敗を検査なしに放置すると
        # `interleave_run_pairs` 自体の戻り値がループ最終コマンドの終了
        # コードだけに引きずられ、途中セッションの失敗が握りつぶされる
        # ため。呼び出し元（`bench-accept.sh`・`bench-pair.sh`）の
        # BLOCKED 判定はこの戻り値を見て行う契約）。
        if ! interleave_run_session "${bin_a}" "${port_a}" "${out_dir}/a-${i}.json"; then
            echo "交互ペア ${i}/${PAIRS} の A セッションが失敗しました" >&2
            return 1
        fi
        echo "# 交互ペア ${i}/${PAIRS}: B セッション" >&2
        if ! interleave_run_session "${bin_b}" "${port_b}" "${out_dir}/b-${i}.json"; then
            echo "交互ペア ${i}/${PAIRS} の B セッションが失敗しました" >&2
            return 1
        fi
    done
}

export PAIRS PAIR_M2 PAIR_MIN_PAIRS
