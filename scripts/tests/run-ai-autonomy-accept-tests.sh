#!/usr/bin/env bash
# ai-autonomy-accept.sh のセルフテスト（TASK-12.7、#48 で新設）。
#
# 判定ロジック（台帳パース・fail-closed・SKIP/FAIL の切り分け）を、workspace の実データ
# （`docs/reports/task-12-7-metrics.summary` 等）に依存せず `--ledger` /
# `--acceptance-doc` / `--reports-dir` の注入口（`req13-change-impact-accept.sh` の
# `--crates-dir` 慣例を踏襲）で固定化する。cargo・ネットワークに非依存、オフラインで
# 完結する。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/ai-autonomy-accept"
ACCEPT_SCRIPT="${SCRIPTS_DIR}/accept/ai-autonomy-accept.sh"

# ケース 1・5 の終了コード検証を workspace の実データ（D-2 人手評価台帳・F の
# trial-*.summary / task-12-6-records）から切り離すための隔離入力。実測進行に伴い
# 実データ側が FAIL を含むようになっても、台帳パース経路のセルフテストが影響を
# 受けないようにする（Issue #218 で D-2/F が実測 FAIL 確定した際のハーミシティ修正）。
EMPTY_REPORTS_DIR="$(mktemp -d)"

# ケース 14（フル実行）のログ出力先。共有 /tmp の固定パスはシンボリックリンク攻撃・
# 情報漏えいの理論的リスクがあるため mktemp（予測不能名・0600）で生成する
# （#229、#218 監査指摘）。失敗時（FAIL あり）はログを残し、fail メッセージが指す
# パスを事後に確認できるようにする（ai-autonomy-accept.sh 側の cleanup 方針を踏襲）。
FULL_RUN_LOG="$(mktemp)"

# bash の EXIT trap は単一ハンドラのため、後続ケースが個別に trap を張ると先の
# 登録が上書きされて一時ディレクトリが残留する（PR #230 review 4730615163 指摘）。
# trap 経由で消す一時ディレクトリ・ログはすべてこの関数に集約し、登録は 1 回に限る。
cleanup_temp_dirs() {
    rm -rf "${EMPTY_REPORTS_DIR}"
    if [ -n "${TRIALS_OK_DIR:-}" ]; then
        rm -rf "${TRIALS_OK_DIR}"
    fi
    if [ "${FAIL_COUNT:-0}" -eq 0 ]; then
        rm -f "${FULL_RUN_LOG}"
    fi
}
trap cleanup_temp_dirs EXIT
HERMETIC_ARGS=(--acceptance-doc "${FIXTURES_DIR}/acceptance-doc-pending.md" --reports-dir "${EMPTY_REPORTS_DIR}")

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

assert_contains() {
    local desc="$1"
    local haystack="$2"
    local needle="$3"
    if printf '%s' "${haystack}" | grep -qF -- "${needle}"; then
        pass "${desc}"
    else
        fail "${desc}（'${needle}' が出力に含まれません）"
    fi
}

assert_exit_code() {
    local desc="$1"
    local expected="$2"
    local actual="$3"
    if [ "${expected}" -eq "${actual}" ]; then
        pass "${desc}（exit code: ${actual}）"
    else
        fail "${desc}（期待 exit code: ${expected}, 実際: ${actual}）"
    fi
}

echo "=== ai-autonomy-accept.sh セルフテスト ==="

# --- ケース 1: 全指標充足の台帳は A・B・C・E が PASS、終了コード 0
#     （D-2/F は隔離入力で SKIP に固定し、workspace 実データの実測結果に依存しない） ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" "${HERMETIC_ARGS[@]}" 2>&1)"
status=$?
set -e
assert_contains "台帳充足は基準 A が PASS" "${output}" "[PASS] A:"
assert_contains "台帳充足は基準 B が PASS" "${output}" "[PASS] B:"
assert_contains "台帳充足は基準 C が PASS" "${output}" "[PASS] C:"
assert_contains "台帳充足は基準 E が PASS" "${output}" "[PASS] E:"
assert_contains "台帳充足は基準 E-2 が PASS" "${output}" "[PASS] E-2:"
# D-1 は実スクリプト（scripts/audit-triage.sh）+ 既定 fixture で必須 5 項目
# （背景・根拠データ／影響範囲／対応方針／検証方法／リスク）を全生成し PASS する正常系。
assert_contains "台帳充足は基準 D-1 が PASS（必須 5 項目全生成）" "${output}" "[PASS] D-1:"
assert_exit_code "台帳充足は終了コード 0" 0 "${status}"

# --- ケース 2: 閾値未達の台帳は A・B・E が FAIL、終了コード非 0 ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-below-threshold.summary" 2>&1)"
status=$?
set -e
assert_contains "閾値未達は基準 A が FAIL" "${output}" "[FAIL] A:"
assert_contains "閾値未達は基準 E が FAIL" "${output}" "[FAIL] E:"
assert_contains "閾値未達は基準 E-2 が FAIL" "${output}" "[FAIL] E-2:"
if [ "${status}" -ne 0 ]; then
    pass "閾値未達は終了コード非 0"
else
    fail "閾値未達は終了コード非 0（実際: ${status}）"
fi

# --- ケース 3: リグレッション・誤判定破壊ありの台帳は A・B が FAIL（閾値自体は充足） ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-destruction.summary" 2>&1)"
set -e
assert_contains "リグレッションありは基準 A が FAIL" "${output}" "[FAIL] A:"
assert_contains "誤判定破壊ありは基準 B が FAIL" "${output}" "[FAIL] B:"

# --- ケース 4: 不正値（負数・非数値・pass+fail+pending≠total）を含む台帳は
#     フェイルクローズで FAIL（SKIP と混同しない） ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-invalid.summary" 2>&1)"
set -e
assert_contains "不正値は基準 A が FAIL（フェイルクローズ）" "${output}" "[FAIL] A:"
assert_contains "不正値は基準 B が FAIL（フェイルクローズ）" "${output}" "[FAIL] B:"
assert_contains "記載なし metric（evidence_rate）は SKIP のまま" "${output}" "[SKIP] C:"

# --- ケース 5: 台帳ファイル不在は全指標 SKIP（PASS と偽らない）、終了コード 0
#     （D-2/F は隔離入力で SKIP に固定し、workspace 実データの実測結果に依存しない） ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/does-not-exist.summary" "${HERMETIC_ARGS[@]}" 2>&1)"
status=$?
set -e
assert_contains "台帳不在は基準 A が SKIP" "${output}" "[SKIP] A:"
assert_contains "台帳不在は基準 B が SKIP" "${output}" "[SKIP] B:"
assert_contains "台帳不在は基準 C が SKIP" "${output}" "[SKIP] C:"
assert_contains "台帳不在は基準 E が SKIP" "${output}" "[SKIP] E:"
assert_contains "台帳不在は基準 E-2 が SKIP" "${output}" "[SKIP] E-2:"
assert_exit_code "台帳不在（SKIP のみ）は終了コード 0" 0 "${status}"

# --- ケース 6: D-1 fixture 不在は FAIL（判定不能） ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --audit-fixtures-dir "${FIXTURES_DIR}/does-not-exist-dir" 2>&1)"
set -e
assert_contains "D-1 fixture 不在は FAIL" "${output}" "[FAIL] D-1:"

# --- ケース 6b: D-1 は audit-triage.sh 出力に「検証方法」「リスク」欄が欠ける場合
#     FAIL とする（改善提案必須 5 項目チェックの回帰テスト。コミット becf0e0 で
#     追加された両欄の退行を、欠落出力を返すスタブ（--audit-triage-script 注入口）で
#     検知できることを確認する） ---
STUB_TRIAGE_DIR="$(mktemp -d)"
cat > "${STUB_TRIAGE_DIR}/audit-triage-missing-two.sh" <<'EOF'
#!/usr/bin/env bash
# D-1 セルフテスト用スタブ: 必須 5 項目のうち「検証方法」「リスク」を欠いた出力を返す。
echo "背景・根拠データ: cargo audit --json の出力を一次データとして自動生成"
echo "| advisory ID | crate | 現バージョン | 修正版 | 概要 |"
echo "推奨アクション: 各 crate を修正版へ更新する。"
EOF
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --audit-triage-script "${STUB_TRIAGE_DIR}/audit-triage-missing-two.sh" 2>&1)"
set -e
assert_contains "検証方法・リスク欄欠落は D-1 が FAIL" "${output}" "[FAIL] D-1:"
assert_contains "D-1 の FAIL 詳細に欠落欄（検証方法・リスク）を列挙する" "${output}" "欠落欄: 検証方法 リスク"

# --- ケース 6c: D-1 は「リスク」のみ欠ける場合も FAIL とする（単一欄の欠落検知） ---
cat > "${STUB_TRIAGE_DIR}/audit-triage-missing-risk.sh" <<'EOF'
#!/usr/bin/env bash
# D-1 セルフテスト用スタブ: 必須 5 項目のうち「リスク」のみを欠いた出力を返す。
echo "背景・根拠データ: cargo audit --json の出力を一次データとして自動生成"
echo "| advisory ID | crate | 現バージョン | 修正版 | 概要 |"
echo "推奨アクション: 各 crate を修正版へ更新する。"
echo "検証方法:"
EOF
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --audit-triage-script "${STUB_TRIAGE_DIR}/audit-triage-missing-risk.sh" 2>&1)"
set -e
rm -rf "${STUB_TRIAGE_DIR}"
assert_contains "リスク欄のみ欠落でも D-1 が FAIL" "${output}" "[FAIL] D-1:"
assert_contains "D-1 の FAIL 詳細が欠落欄をリスクのみと列挙する" "${output}" "欠落欄: リスク"

# --- ケース 7: D-2 人手評価台帳が全件記入・閾値充足時は PASS ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --acceptance-doc "${FIXTURES_DIR}/acceptance-doc-pass.md" 2>&1)"
set -e
assert_contains "人手評価台帳充足は D-2 が PASS" "${output}" "[PASS] D-2:"

# --- ケース 8: D-2 人手評価台帳に PENDING 行が残る場合は SKIP（PASS と偽らない） ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --acceptance-doc "${FIXTURES_DIR}/acceptance-doc-pending.md" 2>&1)"
set -e
assert_contains "人手評価台帳 PENDING 残存は D-2 が SKIP" "${output}" "[SKIP] D-2:"

# --- ケース 9: D-2 人手評価台帳が全件記入だが閾値未達の場合は FAIL ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --acceptance-doc "${FIXTURES_DIR}/acceptance-doc-fail.md" 2>&1)"
set -e
assert_contains "人手評価台帳閾値未達は D-2 が FAIL" "${output}" "[FAIL] D-2:"

# --- ケース 10: D-2 受け入れレポート自体が未作成の場合は SKIP ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --acceptance-doc "${FIXTURES_DIR}/does-not-exist.md" 2>&1)"
set -e
assert_contains "受け入れレポート未作成は D-2 が SKIP" "${output}" "[SKIP] D-2:"

# --- ケース 11: F 試行サマリ・グレーゾーン記録とも不在なら SKIP（実施手順を案内） ---
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --reports-dir "${FIXTURES_DIR}/does-not-exist-reports" 2>&1)"
set -e
assert_contains "試行・グレーゾーン記録とも不在は F が SKIP" "${output}" "[SKIP] F:"
assert_contains "F の SKIP は実施手順を案内する" "${output}" "multi-trial-stability-verification.md"

# --- ケース 12: F 試行サマリのみ揃っていても、グレーゾーン記録が不在（片側のみ）
#     なら PASS と断定せず SKIP とする（PR #174 review 4728502197 指摘 #2 の
#     回帰テスト。安定性試行集計自体は PASS した旨は詳細に残す） ---
TRIALS_OK_DIR="$(mktemp -d)"  # 後始末は冒頭の cleanup_temp_dirs（EXIT trap）に集約
cp "${SCRIPT_DIR}/fixtures/third-party-stability/trial-normal-1.summary" "${TRIALS_OK_DIR}/"
cp "${SCRIPT_DIR}/fixtures/third-party-stability/trial-normal-2.summary" "${TRIALS_OK_DIR}/"
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --reports-dir "${TRIALS_OK_DIR}" 2>&1)"
set -e
assert_contains "試行サマリのみ・グレーゾーン記録なしは F が SKIP（片側のみで完了扱いにしない）" "${output}" "[SKIP] F:"
assert_contains "F の SKIP 詳細に安定性試行集計 PASS を記録する" "${output}" "安定性試行集計 PASS"

# --- ケース 13: F 試行サマリが不正形式（malformed）なら集計 FAIL が伝播する ---
TRIALS_BAD_DIR="$(mktemp -d)"
cp "${SCRIPT_DIR}/fixtures/third-party-stability/trial-malformed.summary" "${TRIALS_BAD_DIR}/"
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --reports-dir "${TRIALS_BAD_DIR}" 2>&1)"
set -e
rm -rf "${TRIALS_BAD_DIR}"
assert_contains "試行サマリ不正時は F が FAIL" "${output}" "[FAIL] F:"

# --- ケース 13b: F 試行サマリの exit code は 0 でも、指標が REQ-12 閾値未達
#     （テキストレポートに「未充足」）なら FAIL とする（review 4728502197 指摘 #1
#     の回帰テスト: exit code のみに依存した誤判定を防ぐ） ---
TRIALS_BELOW_DIR="$(mktemp -d)"
cp "${SCRIPT_DIR}/fixtures/third-party-stability/trial-below-threshold.summary" "${TRIALS_BELOW_DIR}/trial-below.summary"
set +e
output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --reports-dir "${TRIALS_BELOW_DIR}" 2>&1)"
set -e
rm -rf "${TRIALS_BELOW_DIR}"
assert_contains "試行サマリが閾値未達（未充足）テキストを含む場合は F が FAIL（exit code 0 でも誤判定しない）" "${output}" "[FAIL] F:"

# --- ケース 13c: F 試行サマリ・グレーゾーン記録の両方が揃い、双方の実測値が
#     REQ-12 閾値（80% 以上）を充足していれば PASS とする ---
REPO_ROOT_FOR_F_TESTS="$(cd "${SCRIPTS_DIR}/.." && pwd)"
GRAY_TASK_DEFS_FOR_F_TESTS="${REPO_ROOT_FOR_F_TESTS}/docs/reports/task-12-6-task-definitions.md"
if [ -f "${GRAY_TASK_DEFS_FOR_F_TESTS}" ]; then
    BOTH_OK_DIR="$(mktemp -d)"
    cp "${SCRIPT_DIR}/fixtures/third-party-stability/trial-normal-1.summary" "${BOTH_OK_DIR}/"
    cp "${SCRIPT_DIR}/fixtures/third-party-stability/trial-normal-2.summary" "${BOTH_OK_DIR}/"
    cp "${GRAY_TASK_DEFS_FOR_F_TESTS}" "${BOTH_OK_DIR}/task-12-6-task-definitions.md"
    mkdir -p "${BOTH_OK_DIR}/task-12-6-records"
    cp "${FIXTURES_DIR}/../feasibility-verify-gray-correct/"*.md "${BOTH_OK_DIR}/task-12-6-records/"
    set +e
    output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --reports-dir "${BOTH_OK_DIR}" 2>&1)"
    set -e
    rm -rf "${BOTH_OK_DIR}"
    assert_contains "試行サマリ・グレーゾーン記録とも揃い閾値充足なら F が PASS" "${output}" "[PASS] F:"
else
    fail "F 両側 PASS 回帰テストの前提ファイル不在: ${GRAY_TASK_DEFS_FOR_F_TESTS}"
fi

# --- ケース 13d: グレーゾーン記録が揃っていても実測正解率・根拠提示割合が
#     REQ-12 閾値（80%）未満なら F は FAIL とする（review 4728502197 指摘 #1 の
#     グレーゾーン側の回帰テスト） ---
if [ -f "${GRAY_TASK_DEFS_FOR_F_TESTS}" ]; then
    GRAY_BELOW_DIR="$(mktemp -d)"
    cp "${GRAY_TASK_DEFS_FOR_F_TESTS}" "${GRAY_BELOW_DIR}/task-12-6-task-definitions.md"
    mkdir -p "${GRAY_BELOW_DIR}/task-12-6-records"
    cp "${FIXTURES_DIR}/../feasibility-verify-gray-mixed/"*.md "${GRAY_BELOW_DIR}/task-12-6-records/"
    set +e
    output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --reports-dir "${GRAY_BELOW_DIR}" 2>&1)"
    set -e
    rm -rf "${GRAY_BELOW_DIR}"
    assert_contains "グレーゾーン記録の実測値が閾値未達なら F が FAIL" "${output}" "[FAIL] F:"
fi

# --- ケース 13e: グレーゾーン記録のみ揃い閾値充足でも、試行サマリが不在（片側のみ）
#     なら PASS と断定せず SKIP とする（ケース 12 の対称ケース、advisor 指摘） ---
if [ -f "${GRAY_TASK_DEFS_FOR_F_TESTS}" ]; then
    GRAY_ONLY_DIR="$(mktemp -d)"
    cp "${GRAY_TASK_DEFS_FOR_F_TESTS}" "${GRAY_ONLY_DIR}/task-12-6-task-definitions.md"
    mkdir -p "${GRAY_ONLY_DIR}/task-12-6-records"
    cp "${FIXTURES_DIR}/../feasibility-verify-gray-correct/"*.md "${GRAY_ONLY_DIR}/task-12-6-records/"
    set +e
    output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --reports-dir "${GRAY_ONLY_DIR}" 2>&1)"
    set -e
    rm -rf "${GRAY_ONLY_DIR}"
    assert_contains "グレーゾーン記録のみ・試行サマリなしは F が SKIP（片側のみで完了扱いにしない）" "${output}" "[SKIP] F:"
    assert_contains "F の SKIP 詳細にグレーゾーン採点 PASS を記録する" "${output}" "グレーゾーン採点 PASS"
fi

# --- ケース 13f: v2 グレーゾーン記録（task-12-6-records-v2）が存在する場合は、
#     v1（task-12-6-records）が閾値未達でも v2 を優先採点する（Issue #240）。
#     v1 = mixed（閾値未達）・v2 = correct（閾値充足）を同居させ、F が PASS になること
#     で「v2 優先・v1 は歴史的記録として残置」の挙動を回帰確認する ---
if [ -f "${GRAY_TASK_DEFS_FOR_F_TESTS}" ]; then
    V2_PREF_DIR="$(mktemp -d)"
    cp "${SCRIPT_DIR}/fixtures/third-party-stability/trial-normal-1.summary" "${V2_PREF_DIR}/"
    cp "${SCRIPT_DIR}/fixtures/third-party-stability/trial-normal-2.summary" "${V2_PREF_DIR}/"
    cp "${GRAY_TASK_DEFS_FOR_F_TESTS}" "${V2_PREF_DIR}/task-12-6-task-definitions.md"
    mkdir -p "${V2_PREF_DIR}/task-12-6-records"
    cp "${FIXTURES_DIR}/../feasibility-verify-gray-mixed/"*.md "${V2_PREF_DIR}/task-12-6-records/"
    mkdir -p "${V2_PREF_DIR}/task-12-6-records-v2"
    cp "${FIXTURES_DIR}/../feasibility-verify-gray-correct/"*.md "${V2_PREF_DIR}/task-12-6-records-v2/"
    set +e
    output="$(bash "${ACCEPT_SCRIPT}" --ledger "${FIXTURES_DIR}/ledger-ok.summary" --reports-dir "${V2_PREF_DIR}" 2>&1)"
    set -e
    rm -rf "${V2_PREF_DIR}"
    assert_contains "v2 グレーゾーン記録が存在すれば v1 閾値未達でも v2 を優先採点し F が PASS" "${output}" "[PASS] F:"
fi

# --- ケース 14: フル実行（workspace の実データ、確定値台帳・実レポート含む）が
#     クラッシュせずサマリーまで完走することを確認する（回帰確認）。
#     exit 0（FAIL なし）/ exit 1（実測 FAIL あり）はいずれも正当な判定結果であり、
#     workspace の実測状態（Issue #218 で D-2/F が実測 FAIL 確定）に依存させない。
#     exit 2 以上（引数・パースエラー等の異常終了）とサマリー欠落のみを失敗とする ---
set +e
bash "${ACCEPT_SCRIPT}" >"${FULL_RUN_LOG}" 2>&1
full_status=$?
set -e
if [ "${full_status}" -le 1 ] && grep -q "受け入れ検証サマリー" "${FULL_RUN_LOG}"; then
    pass "フル実行（workspace 実データ）がサマリーまで完走（exit ${full_status}。詳細: ${FULL_RUN_LOG}）"
else
    fail "フル実行（workspace 実データ）が異常終了またはサマリー欠落（exit ${full_status}。詳細: ${FULL_RUN_LOG}）"
fi

echo ""
echo "=== 結果: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
