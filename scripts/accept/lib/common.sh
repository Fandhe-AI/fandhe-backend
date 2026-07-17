#!/usr/bin/env bash
# scripts/accept/*.sh 共通関数（TASK-1.6-2、#72）。
#
# このライブラリの役割:
#   - REQ-1（docs/spec/04-requirements.md）の受け入れ基準ごとに PASS / FAIL / SKIP / WARN を
#     記録し、末尾にサマリー表を出力する集計基盤
#   - 前提ツール（cargo-audit・cargo-deny 等）の存在検査（スクリプトが勝手にバイナリを
#     取得しない。benches/lib/common.sh の方針を踏襲）
#   - 並列タスク（#70 コアループ・#14 routes クレート・#16 deny.toml）が未マージの間は
#     該当チェックを SKIP として扱い、非 0 終了させない
#
# 呼び出し元: core-deps-unsafe-audit.sh が
# `source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"` で読み込む。
# 単体では実行しない（関数定義のみで副作用を持たない）。

set -euo pipefail

# 呼び出し元スクリプトの1階層上（scripts/accept/ の親の親）を基準に workspace ルートを解決する。
WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

# 判定結果の集計。連想配列ではなく単純な行の配列に "基準|判定|詳細" 形式で積む
# （bash 4 未満の環境でも動くよう連想配列に依存しない）。
RESULT_ROWS=()
HAS_FAIL=0

# 基準を PASS として記録する。
# 引数: $1 基準名（例 "A: 依存クレート数"）、$2 詳細（実測値等）
record_pass() {
    RESULT_ROWS+=("PASS|${1}|${2}")
    echo "[PASS] ${1}: ${2}"
}

# 基準を FAIL として記録する。受け入れ未達を意味し、スクリプト終了コードを非 0 にする。
# 引数: $1 基準名、$2 詳細（何が閾値・期待値を満たさなかったか）
record_fail() {
    RESULT_ROWS+=("FAIL|${1}|${2}")
    HAS_FAIL=1
    echo "[FAIL] ${1}: ${2}" >&2
}

# 基準を SKIP として記録する。前提タスク（#70/#14/#16 等）が未完で検証対象が
# 存在しない場合に使う。SKIP は終了コードに影響しない（未達ではなく保留のため）。
# 引数: $1 基準名、$2 理由（前提イシュー番号を含める）
record_skip() {
    RESULT_ROWS+=("SKIP|${1}|${2}")
    echo "[SKIP] ${1}: ${2}"
}

# 基準を WARN として記録する。参考情報や暫定運用（例: deny.toml 未整備で既定設定運用）
# を示す。終了コードには影響しない。
# 引数: $1 基準名、$2 詳細
record_warn() {
    RESULT_ROWS+=("WARN|${1}|${2}")
    echo "[WARN] ${1}: ${2}"
}

# 前提ツールの存在検査。見つからなければ導入コマンドを案内するだけで、
# スクリプトが自動でインストールすることはない（サプライチェーン考慮、
# .claude/rules/security.md）。
# 引数: $1 コマンド名、$2 導入コマンド案内
# 戻り値: 0 = 存在する、1 = 存在しない
check_tool() {
    local cmd="${1}"
    local hint="${2}"
    if command -v "${cmd}" >/dev/null 2>&1; then
        return 0
    fi
    echo "情報: ${cmd} が見つかりません。導入する場合は: ${hint}" >&2
    return 1
}

# サマリー表を標準出力へ出す。
print_summary() {
    echo ""
    echo "=== 受け入れ検証サマリー（REQ-1、TASK-1.6-2 / #72） ==="
    printf '%-6s | %-40s | %s\n' "判定" "基準" "詳細"
    printf '%-6s-+-%-40s-+-%s\n' "------" "----------------------------------------" "----------------------------------------"
    local row
    for row in "${RESULT_ROWS[@]}"; do
        # "|" 区切りだが detail（第3フィールド）に実改行を含み得る（例: check_plugin_independence
        # の複数行 grep 結果）。here-string 経由の `read` は改行までしか読まず 2 行目以降を
        # silently 落とすため、パラメータ展開で先頭2つの "|" のみを区切りとして分割し、
        # detail 内の改行はすべて継続行として表に残す。
        local status="${row%%|*}"
        local rest="${row#*|}"
        local criterion="${rest%%|*}"
        local detail="${rest#*|}"
        local first_line="${detail%%$'\n'*}"
        printf '%-6s | %-40s | %s\n' "${status}" "${criterion}" "${first_line}"
        if [[ "${detail}" == *$'\n'* ]]; then
            local cont_line
            while IFS= read -r cont_line; do
                # 空行はそのまま出力すると罫線が崩れるため読み飛ばす。
                [ -z "${cont_line}" ] && continue
                printf '%-6s | %-40s | %s\n' "" "" "${cont_line}"
            done <<<"${detail#*$'\n'}"
        fi
    done
    echo ""
    if [ "${HAS_FAIL}" -ne 0 ]; then
        echo "結果: FAIL あり。受け入れ未達の基準を確認してください。"
    else
        echo "結果: FAIL なし（PASS / SKIP / WARN のみ）。"
    fi
}

# 集計結果に基づき終了コードを返す。FAIL が 1 件でもあれば非 0。
# 呼び出し元スクリプトの末尾で `exit "$(summary_exit_code)"` のように使う。
summary_exit_code() {
    if [ "${HAS_FAIL}" -ne 0 ]; then
        echo 1
    else
        echo 0
    fi
}

export WORKSPACE_ROOT
