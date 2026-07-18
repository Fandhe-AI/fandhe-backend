#!/usr/bin/env bash
# REQ-12（AI 自律改修支援機構）・NFR-8 の受け入れ検証オーケストレータ
# （TASK-12.7、#48、docs/spec/05-tasks.md 380〜385 行目）。
#
# TASK-12.4〜12.6 の第三者再検証結果に基づき、REQ-12・NFR-8 の受け入れ基準値を確定し、
# 機械検証可能な受け入れテストとして次の基準 A〜F を判定する:
#   A. 自律完遂率 60% 以上・リグレッション 0 件
#   B. 可否判定正解率 80% 以上・誤判定破壊 0 件
#   C. エスカレーション時の判断根拠提示 80% 以上
#   D. 自動監査タスクの妥当性判断 80% 以上
#      D-1（機械）: audit-triage.sh が影響範囲（crate 列）・対応方針（推奨アクション）欄を
#           生成すること（fixture 駆動）
#      D-2（人手）: 人手評価台帳（受け入れレポート内の評価表）の充足率。未記入時は SKIP
#   E. NFR-8: 自動修正でテストが通る修正を得られる割合 70% 以上
#   F. 複数回試行の安定性・グレーゾーン再検証の状態（試行サマリ・記録の有無を検査）
#
# A・B・C・E は `docs/reports/task-12-7-metrics.summary`（確定値台帳）を入力とする。
# 台帳は被験由来の実測値を転記した信頼できない入力として扱い、metric 名 allowlist +
# 非負整数のみを受理する fail-closed パースを行う（eval・コマンド置換への値展開なし、
# `scripts/third-party-stability-aggregate.sh` のパーサ設計を踏襲。OWASP A03 対策、
# `.claude/rules/security.md`）。
#
# 判定不能（前提スクリプト不在・台帳欠落・実測 PENDING 等）は PASS と偽らず、
# 機械検証できる範囲は FAIL、実測 PENDING が理由の場合は SKIP として記録する
# （`scripts/accept/lib/common.sh` の既存方針）。
#
# 呼び出し元: 人間が `bash scripts/accept/ai-autonomy-accept.sh` として直接実行する。
#
# `--ledger <file>` で確定値台帳を、`--audit-fixtures-dir <dir>` で D-1 の fixture 群を、
# `--acceptance-doc <file>` で D-2 の人手評価台帳（markdown）を、`--reports-dir <dir>` で
# F の試行サマリ探索先を差し替え可能（`scripts/tests/run-ai-autonomy-accept-tests.sh` の
# セルフテスト注入口、`req13-change-impact-accept.sh` の `--crates-dir` 慣例を踏襲）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
cd "${WORKSPACE_ROOT}"

LEDGER_FILE="${WORKSPACE_ROOT}/docs/reports/task-12-7-metrics.summary"
AUDIT_FIXTURES_DIR="${WORKSPACE_ROOT}/scripts/tests/fixtures"
ACCEPTANCE_DOC="${WORKSPACE_ROOT}/docs/reports/task-12-7-acceptance.md"
REPORTS_DIR="${WORKSPACE_ROOT}/docs/reports"
TASK_DEFS_124_2="${WORKSPACE_ROOT}/docs/reports/task-12-4-2-task-definitions.md"
RECORDS_DIR_124_2="${WORKSPACE_ROOT}/docs/reports/task-12-4-2-records"

while [ $# -gt 0 ]; do
    case "$1" in
        --ledger)
            LEDGER_FILE="$2"
            shift 2
            ;;
        --audit-fixtures-dir)
            AUDIT_FIXTURES_DIR="$2"
            shift 2
            ;;
        --acceptance-doc)
            ACCEPTANCE_DOC="$2"
            shift 2
            ;;
        --reports-dir)
            REPORTS_DIR="$2"
            shift 2
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

# ---------------------------------------------------------------------------
# 台帳パーサ（fail-closed、third-party-stability-aggregate.sh のパーサ設計を踏襲）
# ---------------------------------------------------------------------------
ALLOWED_METRICS="completion_rate
feasibility_accuracy
evidence_rate
auto_fix_rate
regression
destruction"

is_nonneg_int() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
        *) return 0 ;;
    esac
}

# 台帳から指定 metric の値を取り出す。呼び出し元が command substitution `$(...)` で
# 呼ぶとサブシェルで実行され結果がグローバル変数へ反映されないため、本関数は必ず
# 素の文（bare statement）として呼び出し、結果はグローバル変数 LOOKUP_* 経由で返す
# （常に戻り値 0、`set -e` 環境下でも安全に素の文として呼べる）。
#   LOOKUP_STATUS: 0=発見・妥当 / 1=未記載（PENDING） / 2=不正な値（フェイルクローズ対象）
#   rate 系（completion_rate 等）: LOOKUP_PASS/LOOKUP_FAIL/LOOKUP_PENDING/LOOKUP_TOTAL
#   count 系（regression/destruction）: LOOKUP_COUNT
LOOKUP_STATUS=1
LOOKUP_PASS=0
LOOKUP_FAIL=0
LOOKUP_PENDING=0
LOOKUP_TOTAL=0
LOOKUP_COUNT=0

lookup_metric() {
    local target_metric="$1"
    LOOKUP_STATUS=1
    LOOKUP_PASS=0
    LOOKUP_FAIL=0
    LOOKUP_PENDING=0
    LOOKUP_TOTAL=0
    LOOKUP_COUNT=0

    if [ ! -f "${LEDGER_FILE}" ]; then
        return 0
    fi

    local line
    while IFS= read -r line; do
        case "${line}" in
            '#'*|'') continue ;;
        esac
        case "${line}" in
            metric=*) ;;
            *) continue ;;
        esac

        local metric="" pass="" fail="" pending="" total="" count=""
        for field in ${line}; do
            case "${field}" in
                metric=*) metric="${field#metric=}" ;;
                pass=*) pass="${field#pass=}" ;;
                fail=*) fail="${field#fail=}" ;;
                pending=*) pending="${field#pending=}" ;;
                total=*) total="${field#total=}" ;;
                count=*) count="${field#count=}" ;;
                *) ;;
            esac
        done

        if ! printf '%s\n' "${ALLOWED_METRICS}" | grep -qxF -- "${metric}"; then
            continue
        fi
        if [ "${metric}" != "${target_metric}" ]; then
            continue
        fi

        if [ "${metric}" = "regression" ] || [ "${metric}" = "destruction" ]; then
            if ! is_nonneg_int "${count}"; then
                LOOKUP_STATUS=2
                return 0
            fi
            LOOKUP_COUNT="${count}"
            LOOKUP_STATUS=0
            return 0
        fi

        if ! is_nonneg_int "${pass}" || ! is_nonneg_int "${fail}" || ! is_nonneg_int "${pending}" || ! is_nonneg_int "${total}"; then
            LOOKUP_STATUS=2
            return 0
        fi
        if [ $((pass + fail + pending)) -ne "${total}" ]; then
            LOOKUP_STATUS=2
            return 0
        fi
        LOOKUP_PASS="${pass}"
        # LOOKUP_FAIL/LOOKUP_PENDING は現状どの基準判定（A/B/C/E）も参照しないが、
        # 台帳の pass+fail+pending=total 不変条件検査の結果を呼び出し元へ完全な形で
        # 引き渡す API として保持する（将来 fail/pending 内訳をレポートへ出す拡張に備える）。
        # shellcheck disable=SC2034
        LOOKUP_FAIL="${fail}"
        # shellcheck disable=SC2034
        LOOKUP_PENDING="${pending}"
        LOOKUP_TOTAL="${total}"
        LOOKUP_STATUS=0
        return 0
    done <"${LEDGER_FILE}"

    return 0
}

# rate（%、整数、四捨五入なしの切り捨て）を計算する。total=0 は呼び出し元で弾く。
rate_percent() {
    local pass="$1"
    local total="$2"
    echo $(( (pass * 100) / total ))
}

# ---------------------------------------------------------------------------
# A: 自律完遂率 60% 以上・リグレッション 0 件
# ---------------------------------------------------------------------------
lookup_metric completion_rate
a_completion_status="${LOOKUP_STATUS}"
a_completion_pass="${LOOKUP_PASS}"
a_completion_total="${LOOKUP_TOTAL}"
lookup_metric regression
a_regression_status="${LOOKUP_STATUS}"
a_regression_count="${LOOKUP_COUNT}"

if [ "${a_completion_status}" -eq 0 ] && [ "${a_regression_status}" -eq 0 ] && [ "${a_completion_total}" -gt 0 ]; then
    pct=$(rate_percent "${a_completion_pass}" "${a_completion_total}")
    if [ "${pct}" -ge 60 ] && [ "${a_regression_count}" -eq 0 ]; then
        record_pass "A: 自律完遂率 ≥60% かつリグレッション 0 件" "台帳: ${a_completion_pass}/${a_completion_total}（${pct}%）・リグレッション ${a_regression_count} 件。出典: docs/reports/task-12-4-1-completion-rate-verification.md（実測 2026-07-18、起点 ddc348e）"
    else
        record_fail "A: 自律完遂率 ≥60% かつリグレッション 0 件" "台帳: ${a_completion_pass}/${a_completion_total}（${pct}%）・リグレッション ${a_regression_count} 件（閾値 60% 未達またはリグレッションあり）"
    fi
elif [ "${a_completion_status}" -eq 2 ] || [ "${a_regression_status}" -eq 2 ]; then
    record_fail "A: 自律完遂率 ≥60% かつリグレッション 0 件" "台帳 ${LEDGER_FILE} の completion_rate/regression が不正な値です（フェイルクローズ）"
else
    record_skip "A: 自律完遂率 ≥60% かつリグレッション 0 件" "台帳 ${LEDGER_FILE} に completion_rate/regression の記載がありません（実測 PENDING）"
fi

# ---------------------------------------------------------------------------
# B: 可否判定正解率 80% 以上・誤判定破壊 0 件（記録が残っていれば再採点し台帳値と突合）
# ---------------------------------------------------------------------------
lookup_metric feasibility_accuracy
b_feasibility_status="${LOOKUP_STATUS}"
b_feasibility_pass="${LOOKUP_PASS}"
b_feasibility_total="${LOOKUP_TOTAL}"
lookup_metric destruction
b_destruction_status="${LOOKUP_STATUS}"
b_destruction_count="${LOOKUP_COUNT}"

if [ "${b_feasibility_status}" -eq 0 ] && [ "${b_destruction_status}" -eq 0 ] && [ "${b_feasibility_total}" -gt 0 ]; then
    pct=$(rate_percent "${b_feasibility_pass}" "${b_feasibility_total}")
    b_detail="台帳: ${b_feasibility_pass}/${b_feasibility_total}（${pct}%）・破壊 ${b_destruction_count} 件。出典: docs/reports/task-12-4-2-feasibility-judgment-verification.md（実測 2026-07-18、起点 ddc348e）"
    b_mismatch=0

    # 判定記録が残っていれば third-party-feasibility-verify.sh で再採点し台帳値との
    # 一致を確認する（TASK-12.6 の後方互換確認と同型の突合）。
    if [ -f "${TASK_DEFS_124_2}" ] && [ -d "${RECORDS_DIR_124_2}" ] \
        && [ -f "${WORKSPACE_ROOT}/scripts/third-party-feasibility-verify.sh" ]; then
        set +e
        rescored_output="$(bash "${WORKSPACE_ROOT}/scripts/third-party-feasibility-verify.sh" \
            --task-definitions "${TASK_DEFS_124_2}" \
            --records-dir "${RECORDS_DIR_124_2}" \
            --task-ids "J-01 J-02 J-03 J-04 J-05 J-06 J-07 J-08 J-09 J-10" 2>&1)"
        set -e
        expected_str="${b_feasibility_pass}/${b_feasibility_total}（${pct}%）"
        if printf '%s' "${rescored_output}" | grep -qF -- "${expected_str}"; then
            b_detail="${b_detail}; 再採点（scripts/third-party-feasibility-verify.sh）と台帳値が一致（${expected_str}）"
        else
            record_fail "B: 可否判定正解率 ≥80% かつ誤判定破壊 0 件" "${b_detail}; 再採点結果が台帳値（${expected_str}）と一致しません: ${rescored_output}"
            b_mismatch=1
        fi
    fi

    if [ "${b_mismatch}" -eq 0 ]; then
        if [ "${pct}" -ge 80 ] && [ "${b_destruction_count}" -eq 0 ]; then
            record_pass "B: 可否判定正解率 ≥80% かつ誤判定破壊 0 件" "${b_detail}"
        else
            record_fail "B: 可否判定正解率 ≥80% かつ誤判定破壊 0 件" "${b_detail}（閾値 80% 未達または破壊あり）"
        fi
    fi
elif [ "${b_feasibility_status}" -eq 2 ] || [ "${b_destruction_status}" -eq 2 ]; then
    record_fail "B: 可否判定正解率 ≥80% かつ誤判定破壊 0 件" "台帳 ${LEDGER_FILE} の feasibility_accuracy/destruction が不正な値です（フェイルクローズ）"
else
    record_skip "B: 可否判定正解率 ≥80% かつ誤判定破壊 0 件" "台帳 ${LEDGER_FILE} に feasibility_accuracy/destruction の記載がありません（実測 PENDING）"
fi

# ---------------------------------------------------------------------------
# C: エスカレーション時の判断根拠提示 80% 以上
# ---------------------------------------------------------------------------
lookup_metric evidence_rate
c_evidence_status="${LOOKUP_STATUS}"
c_evidence_pass="${LOOKUP_PASS}"
c_evidence_total="${LOOKUP_TOTAL}"

if [ "${c_evidence_status}" -eq 0 ] && [ "${c_evidence_total}" -gt 0 ]; then
    pct=$(rate_percent "${c_evidence_pass}" "${c_evidence_total}")
    if [ "${pct}" -ge 80 ]; then
        record_pass "C: エスカレーション時の判断根拠提示 ≥80%" "台帳: ${c_evidence_pass}/${c_evidence_total}（${pct}%）。出典: docs/reports/task-12-4-2-feasibility-judgment-verification.md 5 節"
    else
        record_fail "C: エスカレーション時の判断根拠提示 ≥80%" "台帳: ${c_evidence_pass}/${c_evidence_total}（${pct}%）（閾値未達）"
    fi
elif [ "${c_evidence_status}" -eq 0 ] && [ "${c_evidence_total}" -eq 0 ]; then
    record_skip "C: エスカレーション時の判断根拠提示 ≥80%" "台帳の evidence_rate の total が 0 件（対象なし）"
elif [ "${c_evidence_status}" -eq 2 ]; then
    record_fail "C: エスカレーション時の判断根拠提示 ≥80%" "台帳 ${LEDGER_FILE} の evidence_rate が不正な値です（フェイルクローズ）"
else
    record_skip "C: エスカレーション時の判断根拠提示 ≥80%" "台帳 ${LEDGER_FILE} に evidence_rate の記載がありません（実測 PENDING）"
fi

# ---------------------------------------------------------------------------
# D-1: 自動監査タスクの妥当性（機械） — audit-triage.sh が影響範囲（crate 列）・
#      対応方針（推奨アクション）欄を生成することを fixture で検証する
# ---------------------------------------------------------------------------
audit_triage_script="${WORKSPACE_ROOT}/scripts/audit-triage.sh"
d1_fixture="${AUDIT_FIXTURES_DIR}/audit-patched.json"
if [ ! -f "${audit_triage_script}" ]; then
    record_fail "D-1: 自動監査タスクの影響範囲・対応方針欄の機械生成" "scripts/audit-triage.sh が見つかりません"
elif [ ! -f "${d1_fixture}" ]; then
    record_fail "D-1: 自動監査タスクの影響範囲・対応方針欄の機械生成" "fixture ${d1_fixture} が見つかりません（判定不能）"
else
    set +e
    d1_output="$(bash "${audit_triage_script}" --input "${d1_fixture}" 2>/dev/null)"
    set -e
    d1_missing=()
    # 影響範囲: どの crate が影響を受けるかを示す列（改善提案フロー 4 節「影響範囲」の
    # 機械的代理指標。audit-triage.sh は crate 名・バージョン列を持つ表を生成する）。
    printf '%s' "${d1_output}" | grep -q '| advisory ID | crate |' || d1_missing+=("影響範囲（crate 列）")
    # 対応方針（推奨アクション）: 改善提案フロー 4 節の「対応方針（推奨アクション）」に対応。
    printf '%s' "${d1_output}" | grep -qF -- "推奨アクション" || d1_missing+=("対応方針（推奨アクション）")

    if [ ${#d1_missing[@]} -eq 0 ]; then
        record_pass "D-1: 自動監査タスクの影響範囲・対応方針欄の機械生成" "scripts/audit-triage.sh が fixture 実行で影響範囲（crate 列）・対応方針（推奨アクション）の両欄を生成することを確認（docs/design/improvement-proposal-flow.md 4 節）"
    else
        record_fail "D-1: 自動監査タスクの影響範囲・対応方針欄の機械生成" "欠落欄: ${d1_missing[*]}"
    fi
fi

# ---------------------------------------------------------------------------
# D-2: 自動監査タスクの妥当性（人手評価台帳の集計）
#      受け入れレポート内の評価表（"| 妥当" 列を含む markdown 表）を集計する。
#      未記入（PENDING）の場合は PASS と偽らず SKIP とする。
# ---------------------------------------------------------------------------
if [ ! -f "${ACCEPTANCE_DOC}" ]; then
    record_skip "D-2: 自動監査タスクの妥当性判断（人手評価台帳）" "受け入れレポート ${ACCEPTANCE_DOC} がまだ存在しません（人手評価は実測 PENDING）"
else
    # awk の範囲パターン `/開始/,/終了/` は開始行自体が終了パターンにも一致すると
    # 同一行で範囲を閉じてしまうため（見出し行 "## 人手評価台帳" 自体が "^## " に
    # 一致する）、フラグ制御で「見出し行の次から次の見出しの手前まで」を切り出す。
    d2_table="$(awk '/^## 人手評価台帳/{flag=1; next} /^## /{flag=0} flag' "${ACCEPTANCE_DOC}" 2>/dev/null | grep -E '^\|' | grep -v -- '---' || true)"
    d2_rows="$(printf '%s\n' "${d2_table}" | tail -n +2 | grep -v '^[[:space:]]*$' || true)"
    if [ -z "${d2_rows}" ]; then
        record_skip "D-2: 自動監査タスクの妥当性判断（人手評価台帳）" "${ACCEPTANCE_DOC} の人手評価台帳が未記入です（人手評価未実施、PASS と偽らず SKIP）"
    else
        total_rows=0
        valid_rows=0
        while IFS= read -r row; do
            [ -z "${row}" ] && continue
            total_rows=$((total_rows + 1))
            case "${row}" in
                *"| 妥当 |"*) valid_rows=$((valid_rows + 1)) ;;
                *"| 不当 |"*) ;;
                *"PENDING"*) ;;
                *) ;;
            esac
        done <<<"${d2_rows}"

        pending_present=0
        printf '%s\n' "${d2_rows}" | grep -q "PENDING" && pending_present=1

        if [ "${pending_present}" -eq 1 ]; then
            record_skip "D-2: 自動監査タスクの妥当性判断（人手評価台帳）" "評価表に PENDING 行が残っています（全件記入まで SKIP、PASS と偽らない）"
        elif [ "${total_rows}" -eq 0 ]; then
            record_skip "D-2: 自動監査タスクの妥当性判断（人手評価台帳）" "評価表が空です"
        else
            pct=$(rate_percent "${valid_rows}" "${total_rows}")
            if [ "${pct}" -ge 80 ]; then
                record_pass "D-2: 自動監査タスクの妥当性判断（人手評価台帳）" "評価表: ${valid_rows}/${total_rows}（${pct}%）が妥当と評価"
            else
                record_fail "D-2: 自動監査タスクの妥当性判断（人手評価台帳）" "評価表: ${valid_rows}/${total_rows}（${pct}%）が妥当と評価（閾値 80% 未達）"
            fi
        fi
    fi
fi

# ---------------------------------------------------------------------------
# E: NFR-8 自動修正でテストが通る修正を得られる割合 70% 以上
# ---------------------------------------------------------------------------
lookup_metric auto_fix_rate
e_status="${LOOKUP_STATUS}"
e_pass="${LOOKUP_PASS}"
e_total="${LOOKUP_TOTAL}"

if [ "${e_status}" -eq 0 ] && [ "${e_total}" -gt 0 ]; then
    pct=$(rate_percent "${e_pass}" "${e_total}")
    if [ "${pct}" -ge 70 ]; then
        record_pass "E: NFR-8 自動修正でテストが通る修正を得られる割合 ≥70%" "台帳: ${e_pass}/${e_total}（${pct}%、最終判定ベース）。一次機械ゲートのみは 10/10（100%、参考値）"
    else
        record_fail "E: NFR-8 自動修正でテストが通る修正を得られる割合 ≥70%" "台帳: ${e_pass}/${e_total}（${pct}%）（閾値未達）"
    fi
elif [ "${e_status}" -eq 2 ]; then
    record_fail "E: NFR-8 自動修正でテストが通る修正を得られる割合 ≥70%" "台帳 ${LEDGER_FILE} の auto_fix_rate が不正な値です（フェイルクローズ）"
else
    record_skip "E: NFR-8 自動修正でテストが通る修正を得られる割合 ≥70%" "台帳 ${LEDGER_FILE} に auto_fix_rate の記載がありません（実測 PENDING）"
fi

# ---------------------------------------------------------------------------
# F: 複数回試行の安定性（TASK-12.5 試行 2・3）・グレーゾーン再検証（TASK-12.6）の状態
# ---------------------------------------------------------------------------
f_trials_found=0
for f in "${REPORTS_DIR}"/trial-*.summary; do
    [ -e "${f}" ] || continue
    f_trials_found=1
done

gray_records_dir="${REPORTS_DIR}/task-12-6-records"
f_gray_found=0
if [ -d "${gray_records_dir}" ]; then
    for f in "${gray_records_dir}"/*.md; do
        [ -e "${f}" ] || continue
        f_gray_found=1
        break
    done
fi

if [ "${f_trials_found}" -eq 0 ] && [ "${f_gray_found}" -eq 0 ]; then
    record_skip "F: 複数回試行の安定性・グレーゾーン再検証" "試行サマリ（${REPORTS_DIR}/trial-*.summary）・グレーゾーン判定記録（${gray_records_dir}）とも未実施（TASK-12.5 試行 2・3／TASK-12.6 は PENDING）。実施手順: docs/design/multi-trial-stability-verification.md・docs/design/gray-zone-feasibility-verification.md の 3 役分離プロトコルに従い被験セッションを起動し、\`scripts/third-party-stability-aggregate.sh --trials-dir ${REPORTS_DIR}\` / \`scripts/third-party-feasibility-verify.sh --task-definitions docs/reports/task-12-6-task-definitions.md --records-dir ${gray_records_dir} --task-ids \"G-01 G-02 G-03 G-04 G-05 G-06 G-07 G-08 G-09 G-10\"\` を再実行する"
else
    f_detail=""
    f_ok=1
    if [ "${f_trials_found}" -eq 1 ]; then
        if bash "${WORKSPACE_ROOT}/scripts/third-party-stability-aggregate.sh" --trials-dir "${REPORTS_DIR}" >/tmp/ai-autonomy-accept-stability.log 2>&1; then
            f_detail="${f_detail}安定性試行集計 PASS（詳細: /tmp/ai-autonomy-accept-stability.log）; "
        else
            f_detail="${f_detail}安定性試行集計 FAIL（詳細: /tmp/ai-autonomy-accept-stability.log）; "
            f_ok=0
        fi
    else
        f_detail="${f_detail}試行サマリなし（PENDING）; "
    fi
    if [ "${f_gray_found}" -eq 1 ]; then
        if bash "${WORKSPACE_ROOT}/scripts/third-party-feasibility-verify.sh" \
            --task-definitions "${REPORTS_DIR}/task-12-6-task-definitions.md" \
            --records-dir "${gray_records_dir}" \
            --task-ids "G-01 G-02 G-03 G-04 G-05 G-06 G-07 G-08 G-09 G-10" \
            >/tmp/ai-autonomy-accept-gray.log 2>&1; then
            f_detail="${f_detail}グレーゾーン採点 PASS（詳細: /tmp/ai-autonomy-accept-gray.log）"
        else
            f_detail="${f_detail}グレーゾーン採点 FAIL（詳細: /tmp/ai-autonomy-accept-gray.log）"
            f_ok=0
        fi
    else
        f_detail="${f_detail}グレーゾーン判定記録なし（PENDING）"
    fi

    if [ "${f_ok}" -eq 1 ]; then
        record_pass "F: 複数回試行の安定性・グレーゾーン再検証" "${f_detail}"
    else
        record_fail "F: 複数回試行の安定性・グレーゾーン再検証" "${f_detail}"
    fi
fi

print_summary "REQ-12/NFR-8、TASK-12.7 / #48"
exit "$(summary_exit_code)"
