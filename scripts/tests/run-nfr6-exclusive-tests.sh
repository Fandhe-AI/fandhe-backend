#!/usr/bin/env bash
# benches/lib/exclusive.sh のオフライン・セルフテスト（TASK-15.4 相当 / #178）。
#
# `nfr6-exclusive.sh` 本体（実 bench 実行・flock・sleep を伴う静穏待機等の
# 副作用）は対象にせず、`benches/lib/exclusive.sh` のみを source し、
# `get_loadavg1` / `list_busy_process_names` をモック値で再定義した上で
# 静穏判定・ロック相互排他・symlink 拒否・BLOCKED 終了条件を cargo/oha/
# ネットワーク非依存で回帰検証する
# （`scripts/tests/run-webrtc-accept-tests.sh` 等、既存の受け入れ系オフライン
# テストと同じ「副作用のある呼び出し元本体は source しない」方針）。
#
# 呼び出し元: 人間 / CI が `bash scripts/tests/run-nfr6-exclusive-tests.sh` として
# 直接実行する（CI 常設組み込みは行わない。兄弟の accept セルフテストと同じ
# 手動実行、.claude/rules/ci.md の schedule 負荷抑制と整合）。

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# shellcheck source=../../benches/lib/exclusive.sh
source "${REPO_ROOT}/benches/lib/exclusive.sh"

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

echo "===== check_quiescence_once: loadavg 閾値判定（LOAD1_MAX=1.0 既定） ====="
LOAD1_MAX="1.0"
get_loadavg1() { echo "0.50"; }
list_busy_process_names() { :; }
assert_eq "loadavg 0.50（閾値未満）・他プロセスなしは QUIESCENT" "QUIESCENT" "$(check_quiescence_once)"

get_loadavg1() { echo "1.00"; }
assert_eq "loadavg 1.00（閾値と同値）は QUIESCENT" "QUIESCENT" "$(check_quiescence_once)"

get_loadavg1() { echo "1.01"; }
assert_eq "loadavg 1.01（閾値超過）は BUSY" "BUSY" "$(check_quiescence_once)"

get_loadavg1() { echo "not-a-number"; }
assert_eq "loadavg が数値でない場合はフェイルクローズで BUSY" "BUSY" "$(check_quiescence_once)"

echo "===== check_quiescence_once: 他プロセス検出は loadavg が閾値内でも BUSY にする ====="
get_loadavg1() { echo "0.10"; }
list_busy_process_names() { echo "cargo"; }
assert_eq "loadavg 低くても cargo 稼働中なら BUSY" "BUSY" "$(check_quiescence_once)"

list_busy_process_names() { :; }
assert_eq "他プロセスなしに戻れば QUIESCENT" "QUIESCENT" "$(check_quiescence_once)"

echo "===== wait_for_quiescence: 静穏がすぐ得られる場合は即 return 0 ====="
get_loadavg1() { echo "0.10"; }
list_busy_process_names() { :; }
QUIESCE_WAIT_SECS=10
QUIESCE_POLL_INTERVAL_SECS=1
if wait_for_quiescence; then
    pass "静穏即時取得で return 0"
else
    fail "静穏即時取得のはずが失敗扱いになった"
fi

echo "===== wait_for_quiescence: 静穏が最後まで得られない場合は待機超過で return 1（BLOCKED 相当） ====="
get_loadavg1() { echo "5.0"; }
list_busy_process_names() { :; }
QUIESCE_WAIT_SECS=2
QUIESCE_POLL_INTERVAL_SECS=1
if wait_for_quiescence; then
    fail "常時 BUSY のはずが return 0 になった（フェイルクローズ違反）"
else
    pass "常時 BUSY 時は待機超過で return 1（BLOCKED 相当、PASS へ丸めない）"
fi

echo "===== wait_for_quiescence: QUIESCE_POLL_INTERVAL_SECS=0 でも有界待機で return 1 する（PR #193 Bugbot 指摘の回帰テスト） ====="
get_loadavg1() { echo "5.0"; }
list_busy_process_names() { :; }
QUIESCE_WAIT_SECS=2
QUIESCE_POLL_INTERVAL_SECS=0
# 同一プロセス内呼び出しだと万一の無限ループでテストごとハングするため、外側の
# `timeout` コマンドで打ち切れるようサブシェル（別プロセス）に切り出して呼ぶ。
# 戻り値 124（timeout による強制終了）なら「無限ループ化した」= 回帰と判定する。
if timeout 10 bash -c "$(declare -f wait_for_quiescence check_quiescence_once get_loadavg1 list_busy_process_names _nfr6_validate_numeric); QUIESCE_WAIT_SECS=${QUIESCE_WAIT_SECS} QUIESCE_POLL_INTERVAL_SECS=${QUIESCE_POLL_INTERVAL_SECS} LOAD1_MAX=${LOAD1_MAX} wait_for_quiescence"; then
    fail "poll_interval=0 で常時 BUSY のはずが return 0 になった"
else
    status=$?
    if [ "${status}" -eq 124 ]; then
        fail "poll_interval=0 で wait_for_quiescence が有界時間内に終了しなかった（無限ループ回帰）"
    else
        pass "poll_interval=0 でも有界待機のうえで return 1（BLOCKED 相当）"
    fi
fi

echo "===== acquire_exclusive_lock: symlink ロックパスは拒否する ====="
TMP_TEST_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_TEST_DIR}"' EXIT

REAL_TARGET="${TMP_TEST_DIR}/real-target"
: >"${REAL_TARGET}"
SYMLINK_LOCK="${TMP_TEST_DIR}/symlink.lock"
ln -s "${REAL_TARGET}" "${SYMLINK_LOCK}"

if acquire_exclusive_lock "${SYMLINK_LOCK}" 2>/dev/null; then
    fail "symlink ロックパスを受理してしまった（squat 対策違反）"
else
    pass "symlink ロックパスを拒否した"
fi

echo "===== acquire_exclusive_lock / release_exclusive_lock: 通常パスは取得・解放できる ====="
NORMAL_LOCK="${TMP_TEST_DIR}/normal.lock"
if acquire_exclusive_lock "${NORMAL_LOCK}"; then
    pass "通常パスのロック取得に成功"
    release_exclusive_lock
    pass "ロック解放が例外なく完了"
else
    fail "通常パスのロック取得に失敗した"
fi

echo "===== acquire_exclusive_lock: 他プロセスがロック保持中はタイムアウトで失敗する ====="
CONTENDED_LOCK="${TMP_TEST_DIR}/contended.lock"
# サブシェルでロックを保持したまま長時間 sleep させ、そのプロセスが生きている間に
# 本体側の取得がタイムアウトすることを確認する。
(
    exec 8>"${CONTENDED_LOCK}"
    flock 8
    sleep 5
) &
HOLDER_PID="$!"
sleep 0.3

QUIESCE_WAIT_SECS=1
if acquire_exclusive_lock "${CONTENDED_LOCK}" 2>/dev/null; then
    fail "他プロセス保持中のロックを取得できてしまった（相互排他が機能していない）"
    release_exclusive_lock
else
    pass "他プロセス保持中のロックはタイムアウトで取得失敗する（相互排他が機能）"
fi

kill "${HOLDER_PID}" 2>/dev/null || true
wait "${HOLDER_PID}" 2>/dev/null || true

echo "===== snapshot_environment: 必須フィールドを出力する ====="
get_loadavg1() { echo "0.42"; }
list_busy_process_names() { :; }
snapshot_out="$(snapshot_environment before)"
assert_eq "snapshot_label が引数どおり" "snapshot_label=before" "$(echo "${snapshot_out}" | grep '^snapshot_label=')"
if echo "${snapshot_out}" | grep -q '^snapshot_loadavg1=0.42$'; then
    pass "snapshot_loadavg1 がモック値と一致"
else
    fail "snapshot_loadavg1 がモック値と一致しない: ${snapshot_out}"
fi
if echo "${snapshot_out}" | grep -q '^snapshot_busy_processes=none$'; then
    pass "他プロセスなし時は snapshot_busy_processes=none"
else
    fail "snapshot_busy_processes が期待どおりでない: ${snapshot_out}"
fi

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
exit 0
