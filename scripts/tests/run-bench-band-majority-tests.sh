#!/usr/bin/env bash
# `benches/lib/common.sh` の p95 3 帯域判定（`p95_band_verdict`）と
# `benches/lib/exclusive.sh` の一次判定多数決（`nfr6_run_with_majority`）の
# オフライン・セルフテスト（イシュー #614）。
#
# `bench-accept.sh`/`bench-accept-exclusive.sh` 本体（実計測・cargo build・
# flock・sleep を伴う静穏待機等の副作用）は対象にせず、判定ロジックのみを
# source・モック関数で検証する（`scripts/tests/run-nfr6-exclusive-tests.sh` と
# 同じ「副作用のある呼び出し元本体は対象にしない」方針）。
#
# `P95_BAND=0`/`MAJORITY_TRIALS=0`（既定）での後方互換性（既存 exit 0/1/2 契約が
# 完全に不変であること）は、本テストではなく `bench-accept.sh`/
# `bench-accept-exclusive.sh` 自体の実行契約（doc comment 記載の分岐条件）で
# 担保する。本テストは新規追加した純関数・多数決ロジックのみを対象にする。
#
# 呼び出し元: 人間 / CI が `bash scripts/tests/run-bench-band-majority-tests.sh`
# として直接実行する（CI 常設組み込みは行わない。兄弟の `run-nfr6-exclusive-tests.sh`
# と同じ手動実行、.claude/rules/ci.md の schedule 負荷抑制と整合）。

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# shellcheck source=../../benches/lib/common.sh
source "${REPO_ROOT}/benches/lib/common.sh"
# shellcheck source=../../benches/lib/exclusive.sh
source "${REPO_ROOT}/benches/lib/exclusive.sh"
# `common.sh` は独立実行スクリプトを前提に `set -euo pipefail` する（doc comment
# 参照）。本テストは意図的に非 0 終了するモックコマンドを多数呼ぶため、source 後に
# 明示的に `set +e` へ戻す（`run-nfr6-exclusive-tests.sh` は `exclusive.sh` のみを
# source するため本問題が起きないが、本テストは `p95_band_verdict` 検証のため
# `common.sh` も source する必要がある）。
set +e

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

echo "===== p95_band_verdict: 境界値（limit=1.10, margin=0.10 → 判定不能上限 1.21） ====="
assert_eq "ratio=1.00（limit 未満）は PASS" "PASS" "$(p95_band_verdict 1.00 1.10 0.10)"
assert_eq "ratio=1.10（limit と同値、境界は <=）は PASS" "PASS" "$(p95_band_verdict 1.10 1.10 0.10)"
assert_eq "ratio=1.1001（limit をわずかに超過）は INCONCLUSIVE" "INCONCLUSIVE" "$(p95_band_verdict 1.1001 1.10 0.10)"
assert_eq "ratio=1.21（判定不能上限と同値、境界は <=）は INCONCLUSIVE" "INCONCLUSIVE" "$(p95_band_verdict 1.21 1.10 0.10)"
assert_eq "ratio=1.2101（判定不能上限をわずかに超過）は FAIL" "FAIL" "$(p95_band_verdict 1.2101 1.10 0.10)"
assert_eq "ratio=2.00（大幅超過）は FAIL" "FAIL" "$(p95_band_verdict 2.00 1.10 0.10)"
assert_eq "ratio=nan（分母 0 等）は無条件 FAIL" "FAIL" "$(p95_band_verdict nan 1.10 0.10)"

# wait_for_quiescence / snapshot_environment が即座に成立するようモック化する
# （多数決ロジックそのものの検証が目的。静穏判定の中身は
# `run-nfr6-exclusive-tests.sh` 側で別途検証済み、同方針を踏襲）。
get_loadavg1() { echo "0.10"; }
list_busy_process_names() { :; }
QUIESCE_WAIT_SECS=5
QUIESCE_POLL_INTERVAL_SECS=1

echo "===== nfr6_run_with_majority: 初回 PASS は再試行なしで即確定 ====="
_call_count=0
mock_cmd() {
    _call_count=$((_call_count + 1))
    return 0
}
nfr6_run_with_majority mock_cmd >/dev/null 2>&1
status=$?
assert_eq "初回 PASS の戻り値は 0" "0" "${status}"
assert_eq "初回 PASS はコマンドを 1 回しか呼ばない（再試行しない）" "1" "${_call_count}"

echo "===== nfr6_run_with_majority: 初回 BLOCKED は即座に返す（再試行しない） ====="
_call_count=0
mock_cmd() {
    _call_count=$((_call_count + 1))
    return 2
}
nfr6_run_with_majority mock_cmd >/dev/null 2>&1
status=$?
assert_eq "初回 BLOCKED の戻り値は 2" "2" "${status}"
assert_eq "初回 BLOCKED はコマンドを 1 回しか呼ばない（再試行しない）" "1" "${_call_count}"

echo "===== nfr6_run_with_majority: 2/3 FAIL で多数決 FAIL 確定（2 回で早期確定） ====="
_call_count=0
mock_cmd() {
    _call_count=$((_call_count + 1))
    return 1
}
nfr6_run_with_majority mock_cmd >/dev/null 2>&1
status=$?
assert_eq "FAIL が 2 連続した時点で戻り値 1（FAIL）確定" "1" "${status}"
assert_eq "2 試行連続一致で早期確定し 3 回目は呼ばない" "2" "${_call_count}"

echo "===== nfr6_run_with_majority: 2/3 INCONCLUSIVE で多数決 INCONCLUSIVE 確定 ====="
_call_count=0
mock_cmd() {
    _call_count=$((_call_count + 1))
    return 3
}
nfr6_run_with_majority mock_cmd >/dev/null 2>&1
status=$?
assert_eq "INCONCLUSIVE が 2 連続した時点で戻り値 3（INCONCLUSIVE）確定" "3" "${status}"
assert_eq "2 試行連続一致で早期確定し 3 回目は呼ばない" "2" "${_call_count}"

echo "===== nfr6_run_with_majority: 3 試行が割れる（0/1/3）場合は INCONCLUSIVE（3）を返す ====="
_call_count=0
mock_cmd() {
    _call_count=$((_call_count + 1))
    case "${_call_count}" in
        1) return 1 ;;
        2) return 0 ;;
        3) return 3 ;;
    esac
}
nfr6_run_with_majority mock_cmd >/dev/null 2>&1
status=$?
assert_eq "3 試行（1/0/3）すべて異なる場合は INCONCLUSIVE（3）へ丸める" "3" "${status}"
assert_eq "3 回とも呼ばれる（多数決不成立まで打ち切らない）" "3" "${_call_count}"

echo "===== nfr6_run_with_majority: 初回 FAIL・2 回目 PASS・3 回目 FAIL で多数決 FAIL 確定 ====="
_call_count=0
mock_cmd() {
    _call_count=$((_call_count + 1))
    case "${_call_count}" in
        1) return 1 ;;
        2) return 0 ;;
        3) return 1 ;;
    esac
}
nfr6_run_with_majority mock_cmd >/dev/null 2>&1
status=$?
assert_eq "1/0/1 の 3 試行は 2/3 一致（FAIL）で確定" "1" "${status}"
assert_eq "3 回とも呼ばれる" "3" "${_call_count}"

echo "===== nfr6_run_with_majority: FAIL 単発票（1/0/0）は多数決で PASS へ上書きされず INCONCLUSIVE（3）====="
# PR #621 codex-review P0 対応の回帰テスト: 最初の試行が FAIL・後続 2 試行が PASS でも
# 多数決で PASS 確定させない（docs/design/bench-p95-criteria.md 5.1 節参照）。
_call_count=0
mock_cmd() {
    _call_count=$((_call_count + 1))
    case "${_call_count}" in
        1) return 1 ;;
        2) return 0 ;;
        3) return 0 ;;
    esac
}
nfr6_run_with_majority mock_cmd >/dev/null 2>&1
status=$?
assert_eq "1/0/0 の 3 試行は FAIL 単発票のため多数決で確定せず INCONCLUSIVE（3）" "3" "${status}"
assert_eq "3 回とも呼ばれる" "3" "${_call_count}"

echo "===== nfr6_run_with_majority: FAIL 単発票（1/3/3）も多数決で INCONCLUSIVE へ上書きされず INCONCLUSIVE（3）のまま ====="
_call_count=0
mock_cmd() {
    _call_count=$((_call_count + 1))
    case "${_call_count}" in
        1) return 1 ;;
        2) return 3 ;;
        3) return 3 ;;
    esac
}
nfr6_run_with_majority mock_cmd >/dev/null 2>&1
status=$?
assert_eq "1/3/3 の 3 試行も FAIL 単発票のため INCONCLUSIVE（3）" "3" "${status}"
assert_eq "3 回とも呼ばれる" "3" "${_call_count}"

echo "===== nfr6_run_with_majority: FAIL 票が 0 回の 2/3 一致（3/0/0）は従来どおり多数決で確定する ====="
# FAIL 単発票の非上書きは FAIL が絡む場合のみの近似であり、FAIL 不在の
# INCONCLUSIVE/PASS 混在まで一律 INCONCLUSIVE へ倒す変更ではないことを確認する。
_call_count=0
mock_cmd() {
    _call_count=$((_call_count + 1))
    case "${_call_count}" in
        1) return 3 ;;
        2) return 0 ;;
        3) return 0 ;;
    esac
}
nfr6_run_with_majority mock_cmd >/dev/null 2>&1
status=$?
assert_eq "3/0/0 の 3 試行は FAIL 不在のため通常どおり多数決で PASS（0）確定" "0" "${status}"
assert_eq "3 回とも呼ばれる" "3" "${_call_count}"

echo "===== nfr6_run_with_majority: 試行中に BLOCKED が出たら即座に打ち切って BLOCKED を返す ====="
_call_count=0
mock_cmd() {
    _call_count=$((_call_count + 1))
    case "${_call_count}" in
        1) return 1 ;;
        2) return 2 ;;
        3) fail "BLOCKED 検出後に 3 回目が呼ばれてはいけない"; return 1 ;;
    esac
}
nfr6_run_with_majority mock_cmd >/dev/null 2>&1
status=$?
assert_eq "2 回目で BLOCKED（2）が出たら再試行せず BLOCKED を返す" "2" "${status}"
assert_eq "BLOCKED 検出直後に打ち切り、3 回目は呼ばれない" "2" "${_call_count}"

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
exit 0
