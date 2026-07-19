#!/usr/bin/env bash
# unsafe 追加の検知トリアージ（TASK-12.1-1、#79、docs/spec/05-tasks.md）:
# workspace（crates/*/src・crates/*/tests、target/ 除外）を走査し、クレート別の
# `unsafe` 使用箇所数・`#[allow(unsafe_code)]` 使用数を、コミット済みの
# scripts/unsafe-baseline.json と比較する「ラチェット」方式で増加を検知する。
#
# 位置づけ: workspace lint `unsafe_code = "warn"`（Cargo.toml、clippy -D warnings で
# 実質 deny）は `#[allow(unsafe_code)]` を付与すれば迂回できてしまう。本スクリプトは
# その迂回・新規追加を機械的に検知し、CI（ci.yml の unsafe-triage ジョブ）で早期に
# 拾う。依存 crate 側の unsafe 増減は scripts/dep-impact.sh（cargo-geiger）の守備範囲で
# あり、本スクリプトは workspace 自身のコードに限定する。
#
# 判定:
#   - いずれかのクレートで unsafe 使用数・allow(unsafe_code) 数が baseline より増加
#     → 増加箇所（file:line）を報告して exit 1
#   - unsafe を含むファイルに `// SAFETY:` コメントが 1 件もない
#     → baseline 内の増減に関わらず exit 1（.claude/rules/coding-rust.md の
#       SAFETY 根拠必須規約の機械強制）
#   - 減少のみ（増加なし）→ exit 0 で通過しつつベースライン縮小を提案（情報出力）
#   - 同数 → exit 0
#
# `--update-baseline`: 現状値から scripts/unsafe-baseline.json を再生成する
# （初回生成・意図した unsafe 追加をレビュー承認のうえ取り込む場合に使う）。
# SAFETY チェックが失敗する状態では更新を許可しない（SAFETY 根拠なき unsafe を
# ベースラインへ書き込ませない安全側の判断）。
#
# 既知の限界（誤検知は許容し、人間のレビューで確認する安全側に倒す方針）:
#   - コメント・文字列リテラル中の `unsafe fn` 等の字面は検知対象に含まれうる
#     （bash/grep によるテキストベース検査のため、Rust パーサではない）。
#   - 単なる `unsafe` という語（例: 説明文中の \`unsafe\`）は誤検知を避けるため
#     カウント対象外とし、`unsafe fn` / `unsafe impl` / `unsafe trait` /
#     `unsafe extern` / `unsafe {` の実利用パターンのみを対象にする。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# REPO_ROOT は既定でスクリプト自身の所在（scripts/ の親）から解決するが、
# scripts/tests/run-triage-tests.sh のセルフテストが実 workspace（crates/・
# scripts/unsafe-baseline.json）を汚さずに擬似クレートで検証できるよう、
# 環境変数 FANDHE_BACKEND_UNSAFE_TRIAGE_REPO_ROOT で上書き可能にする（テスト専用の注入口）。
REPO_ROOT="${FANDHE_BACKEND_UNSAFE_TRIAGE_REPO_ROOT:-$(cd "${SCRIPT_DIR}/.." && pwd)}"
cd "${REPO_ROOT}"

BASELINE_FILE="${REPO_ROOT}/scripts/unsafe-baseline.json"
UPDATE_BASELINE=0

usage() {
    cat <<'EOF'
使い方: unsafe-triage.sh [--update-baseline]

  --update-baseline  現状の unsafe 使用数から scripts/unsafe-baseline.json を
                      再生成する（SAFETY チェックを通過した場合のみ実行される）。
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --update-baseline)
            UPDATE_BASELINE=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "エラー: 未知の引数です: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

check_command() {
    local cmd="$1"
    local install_hint="$2"
    if ! command -v "${cmd}" >/dev/null 2>&1; then
        echo "エラー: ${cmd} が見つかりません。次のコマンドで導入してください:" >&2
        echo "  ${install_hint}" >&2
        exit 2
    fi
}

check_command "jq" "OS のパッケージマネージャで jq を導入してください（例: apt install jq）"

if [ ! -d "${REPO_ROOT}/crates" ]; then
    echo "エラー: crates/ が見つかりません（${REPO_ROOT} 直下で実行してください）" >&2
    exit 2
fi

# 実利用パターンのみを対象にする（コメント中の \`unsafe\` 等の字面誤検知を避ける）。
UNSAFE_PATTERN='\bunsafe[[:space:]]+(fn|impl|trait|extern)\b|\bunsafe[[:space:]]*\{'
ALLOW_PATTERN='#\[allow\(unsafe_code\)\]'

WORKDIR="$(mktemp -d)"
trap 'rm -rf "${WORKDIR}"' EXIT

CURRENT_JSON="${WORKDIR}/current.json"
echo '{}' > "${CURRENT_JSON}"

VIOLATIONS_FILE="${WORKDIR}/violations.txt"
: > "${VIOLATIONS_FILE}"
MISSING_SAFETY_FILE="${WORKDIR}/missing-safety.txt"
: > "${MISSING_SAFETY_FILE}"

overall_status=0

for crate_dir in "${REPO_ROOT}"/crates/*/; do
    [ -d "${crate_dir}" ] || continue
    crate_name="$(basename "${crate_dir}")"

    # 走査対象は src/・tests/ のみ（target/ はビルド成果物のため走査しない。
    # benches/・examples/ は現時点の workspace に存在しないため対象外）。
    mapfile -t rs_files < <(
        find "${crate_dir}src" "${crate_dir}tests" -type f -name "*.rs" 2>/dev/null | sort
    )

    unsafe_count=0
    allow_count=0

    for f in "${rs_files[@]:-}"; do
        [ -z "${f}" ] && continue
        file_unsafe_lines="$(grep -nE "${UNSAFE_PATTERN}" "${f}" || true)"
        file_unsafe_count="$(printf '%s' "${file_unsafe_lines}" | grep -c . || true)"
        file_allow_count="$(grep -cE "${ALLOW_PATTERN}" "${f}" || true)"

        unsafe_count=$((unsafe_count + file_unsafe_count))
        allow_count=$((allow_count + file_allow_count))

        if [ "${file_unsafe_count}" -gt 0 ]; then
            rel_path="${f#"${REPO_ROOT}"/}"
            printf '%s\n' "${file_unsafe_lines}" | while IFS=: read -r line_no _rest; do
                [ -z "${line_no}" ] && continue
                printf '%s:%s\n' "${rel_path}" "${line_no}" >> "${VIOLATIONS_FILE}"
            done

            # SAFETY 根拠の機械検査（.claude/rules/coding-rust.md）: unsafe を含む
            # ファイルには最低 1 件の `// SAFETY:` コメントを必須とする。
            if ! grep -qE '//\s*SAFETY:' "${f}"; then
                echo "${rel_path}" >> "${MISSING_SAFETY_FILE}"
            fi
        fi
    done

    jq --arg crate "${crate_name}" \
       --argjson unsafe_count "${unsafe_count}" \
       --argjson allow_count "${allow_count}" \
       '. + {($crate): {"unsafe_count": $unsafe_count, "allow_unsafe_code_count": $allow_count}}' \
       "${CURRENT_JSON}" > "${WORKDIR}/current.next.json"
    mv "${WORKDIR}/current.next.json" "${CURRENT_JSON}"
done

echo "==> 現状の unsafe 使用数"
jq . "${CURRENT_JSON}"

# --------------------------------------------------
# SAFETY コメント欠落チェック（baseline 比較より先に必須で実施する）
# --------------------------------------------------
if [ -s "${MISSING_SAFETY_FILE}" ]; then
    echo "==> エラー: unsafe を含むが // SAFETY: コメントが見つからないファイルがあります:" >&2
    sort -u "${MISSING_SAFETY_FILE}" | while IFS= read -r f; do
        echo "  - ${f}" >&2
    done
    echo "==> 対応: 各 unsafe ブロックの直前に不変条件・安全性根拠を記した // SAFETY: コメントを追加してください（.claude/rules/coding-rust.md）" >&2
    overall_status=1
fi

if [ "${overall_status}" -ne 0 ]; then
    if [ "${UPDATE_BASELINE}" -eq 1 ]; then
        echo "==> --update-baseline は SAFETY チェック失敗時には実行しません" >&2
    fi
    exit 1
fi

# --------------------------------------------------
# --update-baseline: SAFETY チェック通過後にのみベースラインを再生成する
# --------------------------------------------------
if [ "${UPDATE_BASELINE}" -eq 1 ]; then
    jq -S . "${CURRENT_JSON}" > "${BASELINE_FILE}"
    echo "==> unsafe-baseline.json を更新しました: ${BASELINE_FILE}"
    exit 0
fi

if [ ! -f "${BASELINE_FILE}" ]; then
    echo "エラー: ${BASELINE_FILE} が見つかりません。初回生成は --update-baseline を使ってください" >&2
    exit 2
fi

# --------------------------------------------------
# baseline との比較（ラチェット判定）
# --------------------------------------------------
increased=0
decreased=0

mapfile -t crate_names < <(jq -r 'keys[]' "${CURRENT_JSON}")

for crate_name in "${crate_names[@]}"; do
    cur_unsafe="$(jq -r --arg c "${crate_name}" '.[$c].unsafe_count' "${CURRENT_JSON}")"
    cur_allow="$(jq -r --arg c "${crate_name}" '.[$c].allow_unsafe_code_count' "${CURRENT_JSON}")"
    base_unsafe="$(jq -r --arg c "${crate_name}" '.[$c].unsafe_count // 0' "${BASELINE_FILE}")"
    base_allow="$(jq -r --arg c "${crate_name}" '.[$c].allow_unsafe_code_count // 0' "${BASELINE_FILE}")"

    if [ "${cur_unsafe}" -gt "${base_unsafe}" ] || [ "${cur_allow}" -gt "${base_allow}" ]; then
        increased=1
        echo "==> エラー: クレート '${crate_name}' で unsafe 使用が増加しています（unsafe: ${base_unsafe} -> ${cur_unsafe}, allow(unsafe_code): ${base_allow} -> ${cur_allow}）" >&2
        grep "^crates/${crate_name}/" "${VIOLATIONS_FILE}" 2>/dev/null | while IFS= read -r loc; do
            echo "  - ${loc}" >&2
        done
    elif [ "${cur_unsafe}" -lt "${base_unsafe}" ] || [ "${cur_allow}" -lt "${base_allow}" ]; then
        decreased=1
        echo "==> 情報: クレート '${crate_name}' で unsafe 使用が減少しました（unsafe: ${base_unsafe} -> ${cur_unsafe}, allow(unsafe_code): ${base_allow} -> ${cur_allow}）。scripts/unsafe-triage.sh --update-baseline でベースラインを縮小することを提案します"
    fi
done

if [ "${increased}" -eq 1 ]; then
    echo "==> 対応: SAFETY 根拠を記載のうえレビュー承認を得て、同一 PR で scripts/unsafe-triage.sh --update-baseline を実行しベースライン更新をコミットに含めてください" >&2
    exit 1
fi

if [ "${decreased}" -eq 1 ]; then
    echo "==> unsafe-triage.sh: 増加なし（一部クレートで減少を検知、ベースライン縮小を提案）"
else
    echo "==> unsafe-triage.sh: baseline から変化なし"
fi

exit 0
