#!/usr/bin/env bash
# NFR-8（docs/spec/04-requirements.md）注入リグレッション検知ハーネス（#238）。
#
# 既知の破壊的変更（`docs/reports/nfr8-injection-case-definitions.md` で確定した
# R-01〜R-12、`docs/reports/nfr8-injection-patches/R-*.diff`）を使い捨て
# `git worktree` に 1 件ずつ適用し、既存テストスイート（clippy / cargo-nextest /
# doc test、`.github/workflows/ci.yml` の test ジョブ相当）が検知する割合を計測する。
# `scripts/third-party-verify.sh`（TASK-12.4-1、#85）の安全設計（eval 不使用・
# 全変数クォート・worktree はメイン working copy の外・trap での確実な後始末）を
# 踏襲する（OWASP A03 対策、.claude/rules/security.md）。
#
#   検知（DETECTED）: いずれかのゲートが失敗、またはタイムアウトした
#   検知漏れ（MISSED）: 全ゲートが通過した（既存テストが破壊的変更を捕捉できなかった）
#
# 出力の最終行は `scripts/accept/ai-autonomy-accept.sh` の台帳フォーマットと同型の
# `metric=injection_detection_rate pass=<検知数> fail=<検知漏れ数> pending=0
# total=<注入総数>` を印字する（docs/reports/task-12-7-metrics.summary への転記に使う）。
#
# NFR-8 の閾値（検知率 90% 以上）を下回った場合、またはいずれかのケースでパッチが
# 適用できなかった場合は非 0 終了する（フェイルクローズ、.claude/rules/security.md）。
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

PATCHES_DIR="${REPO_ROOT}/docs/reports/nfr8-injection-patches"
BASE_REV="HEAD"
GATE_CMD=""
TIMEOUT_SECONDS="${REGRESSION_INJECTION_TIMEOUT:-600}"
# ビルド成果物を使い捨て worktree の外の固定ディレクトリへ集約し、12 ケース間で
# 依存クレートのコンパイル結果を再利用する（速度最適化。ケースごとの差分は 1 ファイル
# のみのため、対象クレートの再コンパイルのみで済み依存グラフ全体の再ビルドを避けられる）。
# `mktemp -d` は環境変数 `TMPDIR` を尊重するため、`/tmp` の残容量が少ない環境では
# 呼び出し元が `TMPDIR` を書き込み可能な別ディレクトリへ差し替えること。
TARGET_DIR=""
# 呼び出し元が `--target-dir` を明示指定したかどうか（自動生成分のみ後始末対象と
# 区別するためのフラグ。明示指定分は再実行時の再利用を意図するため削除しない）。
TARGET_DIR_AUTO=0

usage() {
    cat >&2 <<'EOF'
使い方: regression-injection-verify.sh [--patches-dir <dir>] [--base-rev <rev>] [--gate-cmd <cmd>] [--target-dir <dir>]

  --patches-dir <dir>  R-*.diff の格納先（既定: docs/reports/nfr8-injection-patches）
  --base-rev <rev>     計測の起点コミット（既定: HEAD）
  --gate-cmd <cmd>     検知ゲートの差し替え（セルフテスト用の注入口）。指定時は
                        `<cmd> <worktree-dir> <case-id>` として呼び出し、終了コード
                        0 で「全ゲート通過（検知漏れ）」、非 0 で「検知」と解釈する
  --target-dir <dir>   cargo の共有 CARGO_TARGET_DIR（既定: mktemp で自動生成し、
                        終了時に削除する。呼び出し元が明示指定した場合は削除しない
                        ＝ 再実行時の再利用を意図する）
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --patches-dir)
            [ $# -ge 2 ] || { echo "--patches-dir には値が必要です" >&2; usage; exit 2; }
            PATCHES_DIR="$2"
            shift 2
            ;;
        --base-rev)
            [ $# -ge 2 ] || { echo "--base-rev には値が必要です" >&2; usage; exit 2; }
            BASE_REV="$2"
            shift 2
            ;;
        --gate-cmd)
            [ $# -ge 2 ] || { echo "--gate-cmd には値が必要です" >&2; usage; exit 2; }
            GATE_CMD="$2"
            shift 2
            ;;
        --target-dir)
            [ $# -ge 2 ] || { echo "--target-dir には値が必要です" >&2; usage; exit 2; }
            TARGET_DIR="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "不明な引数: $1" >&2
            usage
            exit 2
            ;;
    esac
done

if [ ! -d "${PATCHES_DIR}" ]; then
    echo "[ERROR] パッチディレクトリが見つかりません: ${PATCHES_DIR}" >&2
    exit 2
fi

if [ -z "${TARGET_DIR}" ]; then
    TARGET_DIR="$(mktemp -d)"
    TARGET_DIR_AUTO=1
fi
mkdir -p "${TARGET_DIR}"

# 使い捨て worktree はメイン working copy の外に作る（third-party-verify.sh と同じ
# 独立性担保）。`mktemp -d` は `TMPDIR` を尊重する。
WT_ROOT="$(mktemp -d)"

CASE_IDS=()
CASE_RESULT=()
CASE_CHANNEL=()

# 失敗時のみログを残す。trap は EXIT で必ず worktree・登録済み `git worktree` 参照を
# 掃除し、メイン working copy に汚れを残さない（third-party-verify.sh 方針踏襲）。
cleanup() {
    for id in "${CASE_IDS[@]:-}"; do
        [ -z "${id}" ] && continue
        wt_dir="${WT_ROOT}/${id}"
        if [ -d "${wt_dir}" ]; then
            git -C "${REPO_ROOT}" worktree remove --force "${wt_dir}" >/dev/null 2>&1 || true
        fi
    done
    rm -rf "${WT_ROOT}"
    # `--target-dir` 未指定で自動生成した CARGO_TARGET_DIR はビルドキャッシュを含み
    # サイズが大きいため、デフォルト実行のたびに残留させない（呼び出し元が明示指定
    # した場合は再実行時の再利用を意図して削除しない）。
    if [ "${TARGET_DIR_AUTO}" -eq 1 ] && [ -n "${TARGET_DIR}" ]; then
        rm -rf "${TARGET_DIR}"
    fi
}
trap cleanup EXIT

run_default_gates() {
    local wt_dir="$1"
    local diff_file="$2"
    local log_file="$3"

    # パッチの diff ヘッダ（`diff --git a/<path> b/<path>`）から対象クレート
    # ディレクトリ（`crates/<name>`）を取り出す。cargo は workspace member
    # ディレクトリ内で `-p` なしに実行するとそのメンバーのみを対象にする
    # （workspace 全体の再ビルドを避け、12 ケースの計測時間を抑える）。
    local rel_path
    rel_path="$(grep -m1 '^diff --git a/' "${diff_file}" | sed -E 's#^diff --git a/([^ ]+) .*#\1#')"
    local crate_dir
    crate_dir="$(echo "${rel_path}" | cut -d/ -f1-2)"

    if [ -z "${crate_dir}" ] || [ ! -d "${wt_dir}/${crate_dir}" ]; then
        echo "[ERROR] パッチ対象クレートディレクトリを特定できません: ${rel_path}" >>"${log_file}"
        return 1
    fi

    (
        cd "${wt_dir}/${crate_dir}" || exit 1
        export CARGO_TARGET_DIR="${TARGET_DIR}"

        # 呼び出し元（メインループ）は `gate_rc -eq 124` でハングタイムアウトを
        # 「timeout」チャンネルとして識別する。ここで一律 `exit 1` に潰すと
        # `timeout` コマンドが返す 124 が失われ、ゲート失敗（clippy 等）と
        # 誤ラベル化されるため、各ゲートの実際の終了コードをそのまま伝播する。
        timeout "${TIMEOUT_SECONDS}" cargo clippy --all-targets --all-features -- -D warnings >>"${log_file}" 2>&1
        rc=$?
        if [ "${rc}" -ne 0 ]; then
            echo "GATE=clippy" >>"${log_file}"
            exit "${rc}"
        fi
        # cargo-nextest はテスト単位タイムアウト（.config/nextest.toml profile:ci）
        # を持ち、ハング型のバグ（PoC-9 BUG-3 の教訓）も検知として扱えるようにする。
        if command -v cargo-nextest >/dev/null 2>&1; then
            timeout "${TIMEOUT_SECONDS}" cargo nextest run --all-features --profile ci >>"${log_file}" 2>&1
            rc=$?
            if [ "${rc}" -ne 0 ]; then
                echo "GATE=nextest" >>"${log_file}"
                exit "${rc}"
            fi
        else
            timeout "${TIMEOUT_SECONDS}" cargo test --all-features >>"${log_file}" 2>&1
            rc=$?
            if [ "${rc}" -ne 0 ]; then
                echo "GATE=test" >>"${log_file}"
                exit "${rc}"
            fi
        fi
        timeout "${TIMEOUT_SECONDS}" cargo test --doc --all-features >>"${log_file}" 2>&1
        rc=$?
        if [ "${rc}" -ne 0 ]; then
            echo "GATE=doctest" >>"${log_file}"
            exit "${rc}"
        fi
        exit 0
    )
}

total=0
detected=0
missed=0

for diff_file in "${PATCHES_DIR}"/R-*.diff; do
    [ -e "${diff_file}" ] || continue
    case_id="$(basename "${diff_file}" .diff)"
    total=$((total + 1))
    CASE_IDS+=("${case_id}")

    wt_dir="${WT_ROOT}/${case_id}"
    if ! git -C "${REPO_ROOT}" worktree add --detach --quiet "${wt_dir}" "${BASE_REV}" >/dev/null 2>&1; then
        echo "[ERROR] ${case_id}: git worktree add に失敗しました" >&2
        CASE_RESULT+=("ERROR")
        CASE_CHANNEL+=("worktree-add-failed")
        continue
    fi

    if ! git -C "${wt_dir}" apply --check "${diff_file}" >/dev/null 2>&1; then
        echo "[ERROR] ${case_id}: パッチが適用できません（起点コミットとの不整合の可能性）: ${diff_file}" >&2
        CASE_RESULT+=("ERROR")
        CASE_CHANNEL+=("patch-apply-failed")
        git -C "${REPO_ROOT}" worktree remove --force "${wt_dir}" >/dev/null 2>&1 || true
        continue
    fi
    git -C "${wt_dir}" apply "${diff_file}"

    log_file="${WT_ROOT}/${case_id}.log"
    :>"${log_file}"

    if [ -n "${GATE_CMD}" ]; then
        # セルフテスト注入口。シェル再解釈（eval）を使わず、コマンド名と 2 引数を
        # そのまま渡す（OWASP A03 対策）。
        if timeout "${TIMEOUT_SECONDS}" "${GATE_CMD}" "${wt_dir}" "${case_id}" >>"${log_file}" 2>&1; then
            gate_rc=0
        else
            gate_rc=$?
        fi
    else
        if run_default_gates "${wt_dir}" "${diff_file}" "${log_file}"; then
            gate_rc=0
        else
            gate_rc=$?
        fi
    fi

    if [ "${gate_rc}" -eq 124 ]; then
        detected=$((detected + 1))
        CASE_RESULT+=("DETECTED")
        CASE_CHANNEL+=("timeout")
    elif [ "${gate_rc}" -ne 0 ]; then
        detected=$((detected + 1))
        channel="$(grep -m1 '^GATE=' "${log_file}" 2>/dev/null | sed -E 's/^GATE=//' || true)"
        [ -z "${channel}" ] && channel="gate-failure"
        CASE_RESULT+=("DETECTED")
        CASE_CHANNEL+=("${channel}")
    else
        missed=$((missed + 1))
        CASE_RESULT+=("MISSED")
        CASE_CHANNEL+=("none")
    fi

    git -C "${REPO_ROOT}" worktree remove --force "${wt_dir}" >/dev/null 2>&1 || true
done

echo "==================================================="
echo "NFR-8 注入リグレッション検知結果（起点: ${BASE_REV}）"
echo "==================================================="
idx=0
error_count=0
for id in "${CASE_IDS[@]:-}"; do
    [ -z "${id}" ] && continue
    result="${CASE_RESULT[$idx]}"
    channel="${CASE_CHANNEL[$idx]}"
    echo "${id}: ${result}（${channel}）"
    if [ "${result}" = "ERROR" ]; then
        error_count=$((error_count + 1))
    fi
    idx=$((idx + 1))
done

if [ "${total}" -eq 0 ]; then
    echo "[ERROR] 注入ケース（R-*.diff）が見つかりません: ${PATCHES_DIR}" >&2
    exit 2
fi

pct=$(( (detected * 100) / total ))
echo "---------------------------------------------------"
echo "検知率: ${detected}/${total}（${pct}%）・検知漏れ: ${missed}・パッチ適用エラー: ${error_count}"
echo "metric=injection_detection_rate pass=${detected} fail=${missed} pending=0 total=${total}"

# パッチ適用不能ケースは検知率の分母に含めない集計上の歪み（＝実際には未検証）を
# 生むため、1 件でもあれば無条件でフェイルクローズする。
if [ "${error_count}" -gt 0 ]; then
    echo "[FAIL] パッチ適用エラーが ${error_count} 件あります（フェイルクローズ）" >&2
    exit 1
fi

if [ "${pct}" -lt 90 ]; then
    echo "[FAIL] 検知率 ${pct}% は NFR-8 の閾値（90%）未満です" >&2
    exit 1
fi

echo "[PASS] 検知率 ${pct}% は NFR-8 の閾値（90%）以上です"
exit 0
