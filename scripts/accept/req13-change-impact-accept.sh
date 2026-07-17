#!/usr/bin/env bash
# REQ-13（変更影響範囲の機械判定構造）の受け入れ検証オーケストレータ
# （TASK-13.2、#50、docs/spec/04-requirements.md REQ-13）。
#
# REQ-13 の受け入れ基準
#   (1) 新規プロトコル・機能の追加が既存 3 拡張点のいずれかに閉じるか、閉じない場合は
#       その理由が設計文書に明記される
#   (2) モジュール境界・依存方向が `lib.rs` 等の doc コメントで機械可読に明示されている
# を、次の基準 A〜F で検証する（`docs/design/dependency-graph-contract.md` 対応）:
#   A. 依存方向一方向性の機械検証（scripts/dep-direction-check.sh の呼び出し）
#   B. プラグイン全クレートの拡張点対応宣言（統一形式・語彙・参照先文書の存在）
#   C. 契約ドキュメント（dependency-graph-contract.md）の存在・必須セクション
#   D. 実例 3 コミット（WebSocket/GraphQL/WebRTC）の閉包判定再現
#   E. 閉包違反（WebRTC の E ファイル）の理由明記照合
#   F. ゲート・判定エンジンのセルフテスト
#
# 判定不能（前提スクリプト不在・jq 未導入等）はフェイルクローズで FAIL とする
# （`.claude/rules/security.md`）。前提タスク未完了による検証不能は SKIP として記録し
# PASS を偽らない（`scripts/accept/lib/common.sh` の既存方針）。
#
# 呼び出し元: 人間が `bash scripts/accept/req13-change-impact-accept.sh` として直接実行する。
# CI 常設運用は `.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる
# （実コミット検証を要する基準 D のため `fetch-depth: 0` の Checkout を前提とする）。
#
# `--crates-dir <dir>` でプラグインクレート探索先を差し替え可能
#（run-req13-accept-tests.sh のセルフテスト注入口、dep-direction-check.sh の慣例を踏襲）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
cd "${WORKSPACE_ROOT}"

CRATES_DIR="crates"
CONTRACT_DOC_OVERRIDE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --crates-dir)
            CRATES_DIR="$2"
            shift 2
            ;;
        --contract-doc)
            # 基準 C のセルフテスト注入口（run-req13-accept-tests.sh）。
            CONTRACT_DOC_OVERRIDE="$2"
            shift 2
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

# ---------------------------------------------------------------------------
# A: 依存方向一方向性の機械検証（1 節の正準グラフの機械検証ソース）
# ---------------------------------------------------------------------------
if [ ! -f "${WORKSPACE_ROOT}/scripts/dep-direction-check.sh" ]; then
    record_fail "A: 依存方向一方向性の機械検証" "scripts/dep-direction-check.sh が見つかりません（判定不能、フェイルクローズ）"
elif bash "${WORKSPACE_ROOT}/scripts/dep-direction-check.sh" >/tmp/req13-accept-dep-direction.log 2>&1; then
    record_pass "A: 依存方向一方向性の機械検証" "scripts/dep-direction-check.sh が PASS（詳細: /tmp/req13-accept-dep-direction.log）"
else
    record_fail "A: 依存方向一方向性の機械検証" "scripts/dep-direction-check.sh が FAIL（詳細: /tmp/req13-accept-dep-direction.log）"
fi

# ---------------------------------------------------------------------------
# B: プラグイン全クレートの拡張点対応宣言
#
# `docs/design/dependency-graph-contract.md` 3 節の統一形式
# `//! 拡張点対応: <値>` の存在・語彙のホワイトリスト適合・非該当/パスインターセプト型
# 宣言の参照先設計文書の存在を、`${CRATES_DIR}/plugin-*/src/lib.rs` 全件について検査する。
# ---------------------------------------------------------------------------
# CRATES_DIR は既定値 "crates"（WORKSPACE_ROOT 相対）だが、セルフテストの
# --crates-dir では絶対パスのフィクスチャディレクトリを渡す運用のため、
# 絶対パスならそのまま、相対パスなら WORKSPACE_ROOT 起点で解決する。
case "${CRATES_DIR}" in
    /*)
        resolved_crates_dir="${CRATES_DIR}"
        ;;
    *)
        resolved_crates_dir="${WORKSPACE_ROOT}/${CRATES_DIR}"
        ;;
esac

if [ ! -d "${resolved_crates_dir}" ]; then
    record_fail "B: プラグイン拡張点対応宣言" "${CRATES_DIR} が存在しません（判定不能）"
else
    declared_missing=()
    vocab_invalid=()
    ref_missing=()
    plugin_crate_count=0

    for crate_dir in "${resolved_crates_dir}"/plugin-*/; do
        [ -d "${crate_dir}" ] || continue
        crate_name="$(basename "${crate_dir}")"
        entrypoint="${crate_dir}src/lib.rs"
        plugin_crate_count=$((plugin_crate_count + 1))

        if [ ! -f "${entrypoint}" ]; then
            declared_missing+=("${crate_name}（src/lib.rs 不在）")
            continue
        fi

        # 宣言行の抽出。1 クレート 1 行の統一形式を前提とする（複数行あっても先頭 1 件で判定）。
        decl_line="$(grep -m1 -E '^//! 拡張点対応: ' "${entrypoint}" || true)"
        if [ -z "${decl_line}" ]; then
            declared_missing+=("${entrypoint}")
            continue
        fi

        decl_value="${decl_line#//! 拡張点対応: }"

        case "${decl_value}" in
            "UpgradeHandler（try_handle_upgrade）"|"Middleware"|"RequestGate")
                : # 許可語彙（固定文字列）
                ;;
            "パスインターセプト型（try_intercept）")
                # 3.2 節: 宣言直後に extension-closure-verification.md 3.4 節への参照が必須
                if ! grep -q 'extension-closure-verification\.md' "${entrypoint}"; then
                    ref_missing+=("${entrypoint}（パスインターセプト型宣言だが extension-closure-verification.md への参照なし）")
                fi
                ;;
            非該当*)
                # 非該当（<理由の参照: docs/design/*.md>）形式。参照先ファイルパスを抽出して存在確認する。
                ref_path="$(printf '%s' "${decl_value}" | grep -oE 'docs/design/[A-Za-z0-9_.-]+\.md' | head -n1 || true)"
                if [ -z "${ref_path}" ]; then
                    ref_missing+=("${entrypoint}（非該当宣言だが docs/design/*.md への参照が抽出できません）")
                elif [ ! -f "${WORKSPACE_ROOT}/${ref_path}" ]; then
                    ref_missing+=("${entrypoint}（参照先 ${ref_path} が存在しません）")
                fi
                ;;
            *)
                vocab_invalid+=("${entrypoint}（'${decl_value}' は許可語彙外）")
                ;;
        esac
    done

    if [ "${plugin_crate_count}" -eq 0 ]; then
        record_fail "B: プラグイン拡張点対応宣言" "${CRATES_DIR} 直下に plugin-* クレートが 1 件も見つかりませんでした（判定不能）"
    elif [ ${#declared_missing[@]} -eq 0 ] && [ ${#vocab_invalid[@]} -eq 0 ] && [ ${#ref_missing[@]} -eq 0 ]; then
        record_pass "B: プラグイン拡張点対応宣言" "${CRATES_DIR} 直下 ${plugin_crate_count} プラグインクレート全てに統一形式・許可語彙の宣言あり"
    else
        detail=""
        [ ${#declared_missing[@]} -gt 0 ] && detail="${detail}宣言欠落: ${declared_missing[*]}; "
        [ ${#vocab_invalid[@]} -gt 0 ] && detail="${detail}語彙外: ${vocab_invalid[*]}; "
        [ ${#ref_missing[@]} -gt 0 ] && detail="${detail}参照先不備: ${ref_missing[*]}; "
        record_fail "B: プラグイン拡張点対応宣言" "${detail}"
    fi
fi

# ---------------------------------------------------------------------------
# C: 契約ドキュメントの存在・必須セクション
# ---------------------------------------------------------------------------
if [ -n "${CONTRACT_DOC_OVERRIDE}" ]; then
    contract_doc="${CONTRACT_DOC_OVERRIDE}"
else
    contract_doc="${WORKSPACE_ROOT}/docs/design/dependency-graph-contract.md"
fi
if [ ! -f "${contract_doc}" ]; then
    record_fail "C: 契約ドキュメントの存在・必須セクション" "${contract_doc} が見つかりません"
else
    required_headings=(
        "## 1. 正準依存グラフ"
        "## 2. 契約一覧"
        "## 3. 機械可読宣言の規約"
        "## 4. 非該当時の理由明記運用"
    )
    missing_headings=()
    for h in "${required_headings[@]}"; do
        if ! grep -qF -- "${h}" "${contract_doc}"; then
            missing_headings+=("${h}")
        fi
    done
    if [ ${#missing_headings[@]} -eq 0 ]; then
        record_pass "C: 契約ドキュメントの存在・必須セクション" "docs/design/dependency-graph-contract.md が存在し必須見出し ${#required_headings[@]} 件全て確認"
    else
        record_fail "C: 契約ドキュメントの存在・必須セクション" "必須見出し欠落: ${missing_headings[*]}"
    fi
fi

# ---------------------------------------------------------------------------
# D: 実例 3 コミットの閉包判定再現
#    （WebSocket=PASS・GraphQL=PASS・WebRTC=FAIL（E 1 件）を機械再現する）
# ---------------------------------------------------------------------------
if [ ! -x "${WORKSPACE_ROOT}/scripts/extension-closure-check.sh" ] && [ ! -f "${WORKSPACE_ROOT}/scripts/extension-closure-check.sh" ]; then
    record_fail "D: 実例 3 コミットの閉包判定再現" "scripts/extension-closure-check.sh が見つかりません"
else
    declare -A example_commits=(
        [websocket]="3ae6d11"
        [graphql]="6a6fb9c"
        [webrtc]="1877cfa"
    )
    declare -A example_expect_pass=(
        [websocket]=1
        [graphql]=1
        [webrtc]=0
    )

    d_detail=""
    d_ok=1
    d_any_skip=0
    for proto in websocket graphql webrtc; do
        sha="${example_commits[${proto}]}"
        if ! git cat-file -e "${sha}^{commit}" 2>/dev/null; then
            d_detail="${d_detail}${proto}(${sha}): SKIP（履歴に存在しない。shallow clone の可能性）; "
            d_any_skip=1
            continue
        fi
        set +e
        bash "${WORKSPACE_ROOT}/scripts/extension-closure-check.sh" --commit "${sha}" >/tmp/req13-accept-closure-"${proto}".log 2>&1
        status=$?
        set -e
        expect_pass="${example_expect_pass[${proto}]}"
        if { [ "${expect_pass}" -eq 1 ] && [ "${status}" -eq 0 ]; } || { [ "${expect_pass}" -eq 0 ] && [ "${status}" -ne 0 ]; }; then
            d_detail="${d_detail}${proto}(${sha}): 期待どおり（詳細: /tmp/req13-accept-closure-${proto}.log）; "
        else
            d_detail="${d_detail}${proto}(${sha}): 期待外れ（exit=${status}, 詳細: /tmp/req13-accept-closure-${proto}.log）; "
            d_ok=0
        fi
    done

    if [ "${d_ok}" -eq 0 ]; then
        record_fail "D: 実例 3 コミットの閉包判定再現" "${d_detail}"
    elif [ "${d_any_skip}" -eq 1 ]; then
        record_skip "D: 実例 3 コミットの閉包判定再現" "一部コミットが履歴に存在せず SKIP（${d_detail}）"
    else
        record_pass "D: 実例 3 コミットの閉包判定再現" "${d_detail}"
    fi
fi

# ---------------------------------------------------------------------------
# E: 閉包違反（WebRTC の E ファイル）の理由明記照合
# ---------------------------------------------------------------------------
verification_doc="${WORKSPACE_ROOT}/docs/design/extension-closure-verification.md"
if [ ! -f "${verification_doc}" ]; then
    record_fail "E: 閉包違反の理由明記照合" "${verification_doc} が見つかりません"
elif grep -qF -- "crates/http/src/response.rs" "${verification_doc}" && grep -qF -- "1877cfa" "${verification_doc}"; then
    record_pass "E: 閉包違反の理由明記照合" "WebRTC の E ファイル crates/http/src/response.rs と sha 1877cfa が docs/design/extension-closure-verification.md に記載済み"
else
    record_fail "E: 閉包違反の理由明記照合" "crates/http/src/response.rs または sha 1877cfa の記載が docs/design/extension-closure-verification.md に見つかりません"
fi

# ---------------------------------------------------------------------------
# F: ゲート・判定エンジンのセルフテスト
# ---------------------------------------------------------------------------
for selftest in run-extension-closure-tests.sh run-extension-closure-gate-tests.sh; do
    selftest_path="${WORKSPACE_ROOT}/scripts/tests/${selftest}"
    if [ ! -f "${selftest_path}" ]; then
        record_fail "F: ${selftest} セルフテスト" "${selftest_path} が見つかりません"
    elif bash "${selftest_path}" >/tmp/req13-accept-selftest-"${selftest}".log 2>&1; then
        record_pass "F: ${selftest} セルフテスト" "PASS（詳細: /tmp/req13-accept-selftest-${selftest}.log）"
    else
        record_fail "F: ${selftest} セルフテスト" "FAIL（詳細: /tmp/req13-accept-selftest-${selftest}.log）"
    fi
done

print_summary "REQ-13、TASK-13.2 / #50"
exit "$(summary_exit_code)"
