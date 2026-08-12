#!/usr/bin/env bash
# benches/lib/cpu-probe.sh・benches/lib/interleave.sh のオフライン・セルフテスト（イシュー #613）。
#
# `benches/lib/exclusive.sh` 用の `scripts/tests/run-nfr6-exclusive-tests.sh` と
# 同じ方針（副作用のある呼び出し元本体 = `bench-http.sh`/`bench-pair.sh` は
# source しない・cargo/oha/ネットワーク非依存）で、外部 CPU 占有率プローブの
# 算出ロジック・汚染判定・窓単位再計測の有界性・交互測定の二次判定ロジックを
# 検証する。受け入れ基準 3（意図的な外部負荷注入での汚染窓検出）は、実
# `/proc/stat` を使う短時間の busy ループ注入ケース（本ファイル末尾）で満たす。
# `interleave_run_pairs` の実行順序オーケストレーション（ペアごとの A→B / B→A
# 交互化、イシュー #613 P1 レビュー指摘対応）は `interleave_run_session` を
# 呼び出し順序記録スタブへ差し替えて検証する（実サーバ・oha 非依存）。
#
# 呼び出し元: 人間 / CI が `bash scripts/tests/run-bench-cpu-probe-tests.sh` として
# 直接実行する（CI 常設組み込みは行わない、.claude/rules/ci.md の schedule
# 負荷抑制と整合、兄弟の NFR-6 セルフテストと同じ手動実行方針）。

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

PASS_COUNT=0
FAIL_COUNT=0

fail() {
    echo "FAIL: $1" >&2
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

pass() {
    echo "PASS: $1"
    PASS_COUNT=$((PASS_COUNT + 1))
}

assert_eq() {
    local desc="$1" expected="$2" actual="$3"
    if [ "${expected}" = "${actual}" ]; then
        pass "${desc}"
    else
        fail "${desc}（期待: '${expected}'、実際: '${actual}'）"
    fi
}

TMP_TEST_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_TEST_DIR}"' EXIT

# --- cpu-probe.sh を source（フィクスチャ差し替え用の env はまだ設定しない） ---
# shellcheck source=../../benches/lib/cpu-probe.sh
# shellcheck source=../../benches/lib/cpu-probe.sh
source "${REPO_ROOT}/benches/lib/cpu-probe.sh"

echo "===== probe_read_total_jiffies: /proc/stat フィクスチャのパース ====="
FIXTURE_STAT="${TMP_TEST_DIR}/proc-stat-fixture"
# user nice system idle iowait irq softirq steal guest guest_nice
printf 'cpu  100 10 50 800 20 5 5 10 0 0\ncpu0 100 10 50 800 20 5 5 10 0 0\n' >"${FIXTURE_STAT}"
# NOTE: `VAR=val out="$(cmd)"` のように代入のみの行に並べると、bash は
# コマンド名を伴わない単純コマンドとみなし `FANDHE_BACKEND_PROC_STAT` を
# シェルへ永続的に代入してしまう（一時的な env スコープにならない）。
# `VAR=val cmd` の prefix 形にして呼び出しへ確実にスコープする。
out="$(FANDHE_BACKEND_PROC_STAT="${FIXTURE_STAT}" probe_read_total_jiffies)"
# busy = 100+10+50+5+5+10 = 180, total = busy + idle(800) + iowait(20) = 1000
assert_eq "busy/total フィクスチャ算出（steal 込み）" "1000 180" "${out}"

FIXTURE_STAT_BAD="${TMP_TEST_DIR}/proc-stat-bad"
printf 'notcpu 1 2 3\n' >"${FIXTURE_STAT_BAD}"
out="$(FANDHE_BACKEND_PROC_STAT="${FIXTURE_STAT_BAD}" probe_read_total_jiffies)"
assert_eq "先頭行が cpu 行でない場合は空文字（計測不能扱い）" "" "${out}"

FIXTURE_STAT_MISSING="${TMP_TEST_DIR}/proc-stat-missing"
out="$(FANDHE_BACKEND_PROC_STAT="${FIXTURE_STAT_MISSING}" probe_read_total_jiffies)"
assert_eq "/proc/stat 不在時は空文字" "" "${out}"

echo "===== probe_read_pid_jiffies: /proc/<pid>/stat フィクスチャのパース ====="
FIXTURE_PID_DIR="${TMP_TEST_DIR}/pid-stat"
mkdir -p "${FIXTURE_PID_DIR}"
# proc(5) の state〜cstime を実フィールド数どおりに埋める
# （state,ppid,pgrp,session,tty_nr,tpgid,flags,minflt,cminflt,majflt,cmajflt=11 個 →
# utime(70)・stime(30)・cutime(5)・cstime(6) の順、man proc(5)）。
# comm に空白を含めて ")" split ロジックも同時に検証する。
printf '4242 (my proc) S 1 1 1 0 -1 0 0 0 0 0 70 30 5 6 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n' \
    >"${FIXTURE_PID_DIR}/4242"
out="$(FANDHE_BACKEND_PROC_PID_STAT_DIR="${FIXTURE_PID_DIR}" probe_read_pid_jiffies 4242)"
assert_eq "comm にスペースを含む行から utime+stime を正しく抽出（70+30=100）" "100" "${out}"

out="$(FANDHE_BACKEND_PROC_PID_STAT_DIR="${FIXTURE_PID_DIR}" probe_read_pid_jiffies 9999)"
assert_eq "存在しない PID は空文字（プロセス消滅 = 計測不能）" "" "${out}"

echo "===== probe_external_share: 増分算出の純関数テスト ====="
# 総増分1000, busy増分200, 帰属増分200 → 外部増分0 → 0%
assert_eq "帰属側が busy 増分と一致すれば外部占有率 0%" "0.0000" "$(probe_external_share 1000 2000 500 700 500 700)"
# 総増分1000, busy増分200, 帰属増分100 → 外部増分100 → 10%
assert_eq "帰属側が busy 増分より少ない場合の外部占有率算出" "10.0000" "$(probe_external_share 1000 2000 500 700 500 600)"
# 総増分0 → nan
assert_eq "総増分 0（分母 0）は nan（フェイルクローズ）" "nan" "$(probe_external_share 1000 1000 500 700 500 700)"
# 帰属側が busy を上回る（計測誤差）場合は 0% にクランプ
assert_eq "帰属増分が busy 増分を上回る場合は 0% にクランプ" "0.0000" "$(probe_external_share 1000 2000 500 700 500 800)"
# 非数値混入
assert_eq "非数値混入時は nan" "nan" "$(probe_external_share 1000 2000 500 nan 500 700)"
assert_eq "空文字混入時は nan（プロセス消滅で probe_read_pid_jiffies が空を返すケース）" "nan" "$(probe_external_share 1000 2000 500 700 500 "")"

echo "===== probe_is_contaminated: しきい値境界判定 ====="
if probe_is_contaminated "5.0" "5"; then
    fail "しきい値ちょうど（5.0 <= 5）は非汚染のはずが汚染判定された"
else
    pass "しきい値ちょうどは非汚染（超過のみ汚染、境界は含まない）"
fi
if probe_is_contaminated "5.01" "5"; then
    pass "しきい値をわずかに超過すると汚染判定"
else
    fail "しきい値超過（5.01 > 5）が非汚染判定になった"
fi
if probe_is_contaminated "4.9" "5"; then
    fail "しきい値未満（4.9 < 5）が汚染判定になった"
else
    pass "しきい値未満は非汚染"
fi
if probe_is_contaminated "nan" "5"; then
    pass "nan（計測不能）はフェイルクローズで汚染扱い"
else
    fail "nan がフェイルクローズされず非汚染扱いになった"
fi

echo "===== env 検証: 不正なしきい値・再計測上限は起動時に拒否する ====="
if EXT_CPU_MAX_PCT="not-a-number" bash -c "source '${REPO_ROOT}/benches/lib/cpu-probe.sh'" 2>/dev/null; then
    fail "EXT_CPU_MAX_PCT に非数値を指定してもエラーにならなかった"
else
    pass "EXT_CPU_MAX_PCT の非数値指定は起動時エラーで拒否される"
fi
if WINDOW_REMEASURE_MAX="1.5" bash -c "source '${REPO_ROOT}/benches/lib/cpu-probe.sh'" 2>/dev/null; then
    fail "WINDOW_REMEASURE_MAX に非整数を指定してもエラーにならなかった"
else
    pass "WINDOW_REMEASURE_MAX の非整数指定は起動時エラーで拒否される"
fi

echo "===== probe_read_self_children_jiffies: 実 /proc/self 経由の子プロセス CPU 帰属実測 ====="
# 受け入れ基準の前提検証（advisor 指摘: times ベースの帰属が実際に機能するかを
# ユニットテストのフィクスチャだけでなく実 /proc を使って検証する）。
# oha の代わりに CPU を確実に消費する perl ワンライナーをコマンド置換
# （`out="$(...)"`）経由で実行し、bench-http.sh と同じ「同期実行 + 即時 wait」の
# 形状で子プロセス CPU が正しく帰属されることを確認する。
if command -v perl >/dev/null 2>&1; then
    # NOTE: 本検証は「トップレベルスクリプトプロセスが直接 wait する」実運用の
    # 形状（`bench-http.sh` の `for` ループ本体と同じ、追加のコマンド置換や
    # サブシェルで包まない形）を再現する必要がある。`before`/`out`/`after` の
    # 読み取り・perl 実行をさらに外側の `$(...)` や `( ... )` で包むと、その
    # 包んだ分だけ別プロセス（サブシェル）が fork され、perl の reap が
    # そのサブシェル側で起きてしまい、`$$`（トップレベル PID を指す値）の
    # `/proc/<pid>/stat` には反映されない（bash の `$$` はサブシェル内でも値
    # としては「元シェルの PID」を保持するが、実際に wait() するプロセスは
    # フォークされた別プロセスであるため、cutime の記録先が食い違う）。
    # このため本テストスクリプト自身のトップレベルで直接（追加の
    # サブシェルを挟まずに）実行する。
    children_before="$(probe_read_self_children_jiffies)"
    # shellcheck disable=SC2034 # 出力を捨てて CPU 消費のみが目的
    perl_out="$(perl -e 'my $x=0; for (1..150000000) { $x += $_ }')"
    children_after="$(probe_read_self_children_jiffies)"
    before_val="${children_before}"
    after_val="${children_after}"
    if [ -n "${after_val}" ] && [ "${after_val}" -gt "${before_val}" ]; then
        pass "コマンド置換で実行した子プロセスの CPU 消費が cutime+cstime の増分として観測できる（before=${before_val} after=${after_val}）"
    else
        fail "子プロセス CPU 消費が観測できなかった（before=${before_val} after=${after_val}）。oha 帰属ロジックが機能しない可能性"
    fi
else
    echo "SKIP: perl が見つからないため probe_read_self_children_jiffies の実測検証を省略"
fi

echo "===== 受け入れ基準 3: 実負荷注入による汚染検出（実 /proc/stat 使用） ====="
# ダミーの busy ループプロセスを nproc 個注入し、窓の外部占有率が高く算出される
# ことを確認する（非注入時との対比も行う）。trap で確実に回収する。
NPROC="$(nproc 2>/dev/null || echo 2)"
BUSY_PIDS=()
cleanup_busy() {
    if [ "${#BUSY_PIDS[@]}" -gt 0 ]; then
        kill "${BUSY_PIDS[@]}" 2>/dev/null || true
        wait "${BUSY_PIDS[@]}" 2>/dev/null || true
    fi
}
trap 'cleanup_busy; rm -rf "${TMP_TEST_DIR}"' EXIT

# ダミー「サーバ」プロセス（sleep）を用意し、probe_read_pid_jiffies の対象にする。
sleep 10 &
DUMMY_SERVER_PID="$!"

measure_window() {
    # 引数: $1 窓の秒数
    local secs="$1"
    local total_before server_before children_before
    local total_after server_after children_after
    total_before="$(probe_read_total_jiffies)"
    server_before="$(probe_read_pid_jiffies "${DUMMY_SERVER_PID}")"
    children_before="$(probe_read_self_children_jiffies)"
    sleep "${secs}"
    total_after="$(probe_read_total_jiffies)"
    server_after="$(probe_read_pid_jiffies "${DUMMY_SERVER_PID}")"
    children_after="$(probe_read_self_children_jiffies)"
    local total_b total_a busy_b busy_a
    total_b="$(echo "${total_before}" | cut -d' ' -f1)"
    busy_b="$(echo "${total_before}" | cut -d' ' -f2)"
    total_a="$(echo "${total_after}" | cut -d' ' -f1)"
    busy_a="$(echo "${total_after}" | cut -d' ' -f2)"
    local attributed_b=$((server_before + children_before))
    local attributed_a=$((server_after + children_after))
    probe_external_share "${total_b}" "${total_a}" "${busy_b}" "${busy_a}" "${attributed_b}" "${attributed_a}"
}

# 非注入窓（ベースライン。占有率が低いことを期待するが、共有ホスト上のテスト
# 実行では厳密な閾値比較はしない。注入窓との相対比較で検知能力を確認する）。
share_clean="$(measure_window 1)"

for _ in $(seq 1 "${NPROC}"); do
    (while :; do :; done) &
    BUSY_PIDS+=("$!")
done
share_contaminated="$(measure_window 1)"
cleanup_busy
BUSY_PIDS=()

kill "${DUMMY_SERVER_PID}" 2>/dev/null || true
wait "${DUMMY_SERVER_PID}" 2>/dev/null || true

echo "  非注入窓の外部占有率: ${share_clean}%"
echo "  注入窓の外部占有率:   ${share_contaminated}%"

if probe_is_contaminated "${share_contaminated}" "${EXT_CPU_MAX_PCT}"; then
    pass "外部負荷（busy ループ ${NPROC} 個）注入時は汚染窓として検出される"
else
    fail "外部負荷注入時に汚染が検出されなかった（share=${share_contaminated}%）。ホストが極端に高並列で相対的に希釈された可能性、環境依存の SKIP 候補"
fi

if [ "${share_clean}" != "nan" ] && [ "${share_contaminated}" != "nan" ]; then
    if LC_NUMERIC=C awk -v c="${share_clean}" -v x="${share_contaminated}" 'BEGIN { exit !(x > c) }'; then
        pass "注入窓の外部占有率は非注入窓より明確に高い（検知能力の相対確認）"
    else
        fail "注入窓の外部占有率が非注入窓以下だった（clean=${share_clean} contaminated=${share_contaminated}）"
    fi
fi

echo "===== interleave.sh: 窓単位再計測ループの有界性・二次判定ロジック ====="
# shellcheck source=../../benches/lib/interleave.sh
# shellcheck source=../../benches/lib/interleave.sh
source "${REPO_ROOT}/benches/lib/interleave.sh"

# NOTE: `interleave_remeasure_window` の呼び出し回数計測は、結果を
# `$(...)` コマンド置換で受け取る都合上（フォークされたサブシェル内での
# 変数変更は呼び出し元へ伝播しない）、通常のシェル変数ではなく一時ファイルへ
# カウンタを書き出す方式にする（`children_before`/`after` の実測検証と同根の
# bash サブシェル分離の制約）。
echo "  --- interleave_remeasure_window: 常に汚染判定される場合は WINDOW_REMEASURE_MAX で頭打ちになる ---"
REMEASURE_COUNT_FILE="${TMP_TEST_DIR}/remeasure-count-a"
: >"${REMEASURE_COUNT_FILE}"
# shellcheck disable=SC2329 # interleave_remeasure_window へ関数名文字列として
# 渡し `"${measure_cmd}" "$@"` で間接的に呼び出す（shellcheck は追えない）。
always_contaminated_measure() {
    echo "x" >>"${REMEASURE_COUNT_FILE}"
    # 値・汚染フラグ・share を模擬出力（interleave_remeasure_window の契約に従う）
    echo "999 always-contaminated"
}
# shellcheck disable=SC2034 # interleave.sh（source 先）内の
# interleave_remeasure_window が参照するグローバル変数（shellcheck は
# 動的 source 先の参照を追えない）。
WINDOW_REMEASURE_MAX=2
result="$(interleave_remeasure_window always_contaminated_measure)"
call_count="$(wc -l <"${REMEASURE_COUNT_FILE}" | tr -d ' ')"
# 呼び出し回数は初回 + 再計測上限（2）= 3 回で頭打ち
assert_eq "常時汚染時の測定関数呼び出し回数は初回+上限で頭打ち" "3" "${call_count}"
if echo "${result}" | grep -q 'contaminated=1'; then
    pass "上限まで汚染が続いた窓は汚染フラグ付きで最後の値を採用する（silent drop しない）"
else
    fail "上限到達時の汚染フラグが立っていない: ${result}"
fi

echo "  --- interleave_remeasure_window: 最初は汚染だが再計測で解消する場合は早期終了する ---"
REMEASURE_COUNT_FILE="${TMP_TEST_DIR}/remeasure-count-b"
: >"${REMEASURE_COUNT_FILE}"
# shellcheck disable=SC2329 # 同上、間接呼び出し
contaminated_then_clean() {
    echo "x" >>"${REMEASURE_COUNT_FILE}"
    local n
    n="$(wc -l <"${REMEASURE_COUNT_FILE}" | tr -d ' ')"
    if [ "${n}" -eq 1 ]; then
        echo "999 always-contaminated"
    else
        # EXT_CPU_MAX_PCT 既定値（5）を下回る値にする（しきい値超過だと
        # 「再計測で解消する」ケースの意味が壊れるため）。
        echo "2 clean"
    fi
}
result="$(interleave_remeasure_window contaminated_then_clean)"
call_count="$(wc -l <"${REMEASURE_COUNT_FILE}" | tr -d ' ')"
assert_eq "再計測 1 回で解消した場合の呼び出し回数は 2 回" "2" "${call_count}"
if echo "${result}" | grep -q 'contaminated=0'; then
    pass "再計測で解消した窓は非汚染として採用される"
else
    fail "再計測で解消したはずが汚染フラグが残っている: ${result}"
fi

echo "  --- interleave_pair_verdict: 採用ペア中央値の M2 判定 ----"
# cur/pre = 1.02, 1.03, 1.04（すべて M2=0.05 以内）→ PASS
verdict="$(interleave_pair_verdict "0.05" "3" "1.02 1.03 1.04")"
assert_eq "全ペアが M2 以内なら PASS（exit 0 相当の文字列 PASS）" "PASS" "${verdict}"

# cur/pre 中央値が M2 を超過 → FAIL
verdict="$(interleave_pair_verdict "0.05" "3" "1.10 1.12 1.15")"
assert_eq "中央値が M2 超過なら FAIL" "FAIL" "${verdict}"

# 改善方向（cur < pre）は PASS 側
verdict="$(interleave_pair_verdict "0.05" "3" "0.90 0.92 0.95")"
assert_eq "改善方向（cur/pre < 1）は PASS" "PASS" "${verdict}"

# 採用ペア数不足 → INCONCLUSIVE
verdict="$(interleave_pair_verdict "0.05" "6" "1.02 1.03")"
assert_eq "採用ペア数が最低数未満なら INCONCLUSIVE" "INCONCLUSIVE" "${verdict}"

echo "  --- interleave_run_pairs: ペアごとの実行順序交互化（イシュー #613 P1 レビュー指摘対応） ---"
# `interleave_run_session`（実サーバ起動・oha 実行を伴う）を、呼ばれた順序を
# 記録するだけのスタブへ差し替えて `interleave_run_pairs` のオーケストレーション
# （どの順序で A/B を呼ぶか）のみを検証する。実サーバ・oha には依存しない
# （本ファイル冒頭のセルフテスト方針どおり）。
ORDER_LOG_FILE="$(mktemp)"
trap 'rm -f "${ORDER_LOG_FILE}"' EXIT
interleave_run_session() {
    local bin="$1" _port="$2" out="$3"
    echo "${bin}" >>"${ORDER_LOG_FILE}"
    # ペアの JSON 出力（a-<i>.json / b-<i>.json）が生成されないと呼び出し元
    # 判定に影響しうるため、空ファイルとして書き出す（本テストは中身を読まない）。
    : >"${out}"
    return 0
}
PAIRS_OUT_DIR="$(mktemp -d)"
PAIRS=4 interleave_run_pairs "bin-a" "18080" "bin-b" "18081" "${PAIRS_OUT_DIR}" >/dev/null
observed_order="$(tr '\n' ',' <"${ORDER_LOG_FILE}")"
expected_order="bin-a,bin-b,bin-b,bin-a,bin-a,bin-b,bin-b,bin-a,"
assert_eq "奇数ペアは A→B・偶数ペアは B→A の順で実行される（固定順序による位置効果の偏りを排除）" \
    "${expected_order}" "${observed_order}"
rm -rf "${PAIRS_OUT_DIR}"
rm -f "${ORDER_LOG_FILE}"
trap - EXIT

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
exit 0
