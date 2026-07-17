#!/usr/bin/env bash
# TASK-11.5（#37、docs/spec/05-tasks.md）受け入れテスト一式（TASK-11.5-2、#78）。
#
# 親タスク TASK-11.5 が要求する 5 項目を機械的に検査する:
#   1. コア全体の自動テスト行カバレッジ 80% 以上
#   2. doc コメント網羅率 100%
#   3. AGENTS.md 各節（モジュール境界・変更手順・変更完了の判定基準・
#      エスカレーション基準・アサーション網羅性要求）
#   4. CI テストタイムアウト設定
#   5. 依存方向の一方向性
#
# チェックごとに PASS / FAIL / PENDING を出力する。PENDING は「本イシューの実装は
# 完了しているが前提となる別イシューが未完のため判定できない」状態を表し
# （チェック 3 が該当。AGENTS.md 本体は TASK-11.3 / #35 のスコープ）、FAIL とは区別する。
# 終了コードは FAIL が 1 件でもあれば非 0、PENDING のみなら 0 とする。
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

CI_FILE=".github/workflows/ci.yml"
NEXTEST_CONFIG=".config/nextest.toml"
AGENTS_FILE="AGENTS.md"

overall_fail=0
pending_count=0

report() {
    local check="$1"
    local status="$2"
    local detail="$3"
    printf '[%s] チェック %s: %s\n' "${status}" "${check}" "${detail}"
    case "${status}" in
        FAIL) overall_fail=1 ;;
        PENDING) pending_count=$((pending_count + 1)) ;;
    esac
}

echo "==================================================="
echo "TASK-11.5 受け入れテスト（#78）"
echo "==================================================="

# --------------------------------------------------
# チェック 1: コア全体の行カバレッジ 80% 以上
# --------------------------------------------------
if bash "${SCRIPT_DIR}/coverage.sh" >/tmp/accept-task-11-5-coverage.log 2>&1; then
    # coverage.sh はコアの TOTAL 行の後にワークスペース全体の TOTAL 行も出力するため、
    # 結合ログを grep '^TOTAL' | tail -1 すると workspace 全体の値を誤って拾う。
    # coverage.sh が書き出す coverage 専用ファイル（コア対象のみ）から直接読む。
    core_summary_file="${REPO_ROOT}/target/llvm-cov/summary-core.txt"
    if [ -f "${core_summary_file}" ]; then
        core_line=$(grep -E '^TOTAL' "${core_summary_file}" | tail -1)
    else
        core_line="(${core_summary_file} が見つかりません)"
    fi
    report "1（カバレッジ 80% 以上）" "PASS" "coverage.sh が閾値を満たして終了しました（${core_line}）"
else
    tail -5 /tmp/accept-task-11-5-coverage.log
    report "1（カバレッジ 80% 以上）" "FAIL" "coverage.sh が非 0 で終了しました。詳細: /tmp/accept-task-11-5-coverage.log"
fi

# --------------------------------------------------
# チェック 2: doc コメント網羅率 100%
#
# workspace ルートの missing_docs = "warn" が clippy -D warnings で実質 deny になる
# 前提（.claude/rules/coding-rust.md）が保たれているかをまず確認し、その上で
# clippy を実行して doc コメント欠落・その他 lint 違反がないことを検査する。
#
# 単純な `grep 'missing_docs = "warn"' Cargo.toml` は、同じ文字列がコメント行
# （lint 値の説明コメント）にも出現するため、実体の [workspace.lints.rust] エントリを
# 削除してコメントだけが残っていても誤って PASS してしまう。[workspace.lints.rust]
# セクション内の、コメントを除去した非空行のみを対象に判定する。
# --------------------------------------------------
if awk '
    /^\[workspace\.lints\.rust\]/ { in_section=1; next }
    /^\[/ { in_section=0 }
    in_section {
        line=$0
        sub(/#.*/, "", line)
        gsub(/[ \t]+/, "", line)
        if (line == "missing_docs=\"warn\"") { found=1 }
    }
    END { exit(found ? 0 : 1) }
' Cargo.toml; then
    if cargo clippy --workspace --all-targets --all-features -- -D warnings \
        >/tmp/accept-task-11-5-clippy.log 2>&1; then
        report "2（doc コメント網羅率 100%）" "PASS" "missing_docs = \"warn\" 設定済み・clippy -D warnings 通過"
    else
        tail -20 /tmp/accept-task-11-5-clippy.log
        report "2（doc コメント網羅率 100%）" "FAIL" "cargo clippy -D warnings が失敗しました。詳細: /tmp/accept-task-11-5-clippy.log"
    fi
else
    report "2（doc コメント網羅率 100%）" "FAIL" "Cargo.toml に [workspace.lints.rust] missing_docs = \"warn\" が見つかりません"
fi

# --------------------------------------------------
# チェック 3: AGENTS.md 各節
#
# AGENTS.md 本体の作成は TASK-11.3（#35）のスコープ（本イシューの計画・
# out-of-scope-tracking 参照）。ファイル不在時は FAIL ではなく PENDING として
# #35 待ちであることを明示する。存在する場合は必須節の見出しを grep で検査する。
# --------------------------------------------------
if [ ! -f "${AGENTS_FILE}" ]; then
    report "3（AGENTS.md 各節）" "PENDING" "AGENTS.md が未作成です（TASK-11.3 / #35 待ち）"
else
    required_sections=(
        "モジュール境界"
        "変更手順"
        "変更完了の判定基準"
        "エスカレーション基準"
        "アサーション網羅性"
    )
    missing_sections=()
    for section in "${required_sections[@]}"; do
        if ! grep -q "${section}" "${AGENTS_FILE}"; then
            missing_sections+=("${section}")
        fi
    done
    if [ "${#missing_sections[@]}" -eq 0 ]; then
        report "3（AGENTS.md 各節）" "PASS" "必須節（${required_sections[*]}）をすべて確認しました"
    else
        report "3（AGENTS.md 各節）" "FAIL" "AGENTS.md に不足節があります: ${missing_sections[*]}"
    fi
fi

# --------------------------------------------------
# チェック 4: CI テストタイムアウト設定
#
# ci.yml の全ジョブに timeout-minutes があること・.config/nextest.toml に
# slow-timeout があることを検査する（TASK-11.4、#36 の設定が退行していないことの
# 受け入れ確認）。
#
# ジョブ本体（例: "  test:"）のキーは 4 スペースインデント（例: "    timeout-minutes:"）。
# ステップ単位の timeout-minutes（例: doc-test ステップの 15 分制限）は
# "        timeout-minutes:"（8 スペース、steps: → "      - name:" の下）に現れる。
# インデント段を区別せずマッチすると、ステップレベルの timeout-minutes だけが
# 存在してジョブ全体のタイムアウト上限が欠落しているケースを見逃す
# （ジョブレベルの上限とステップレベルの上限は多層防御として別の意味を持つ）ため、
# 厳密にジョブ直下（4 スペース）のみをジョブレベルの timeout-minutes として検査する。
# --------------------------------------------------
missing_timeout_jobs=$(awk '
    /^jobs:/ { in_jobs=1; next }
    in_jobs && /^  [a-zA-Z0-9_-]+:$/ {
        if (job != "" && !has_timeout) { print job }
        job=$0; gsub(/^  /, "", job); gsub(/:$/, "", job); has_timeout=0; next
    }
    in_jobs && /^    timeout-minutes:/ { has_timeout=1 }
    END { if (job != "" && !has_timeout) print job }
' "${CI_FILE}")

if [ -n "${missing_timeout_jobs}" ]; then
    report "4（CI テストタイムアウト設定）" "FAIL" "timeout-minutes が未設定のジョブがあります: $(echo "${missing_timeout_jobs}" | tr '\n' ' ')"
elif ! grep -q 'slow-timeout' "${NEXTEST_CONFIG}"; then
    report "4（CI テストタイムアウト設定）" "FAIL" "${NEXTEST_CONFIG} に slow-timeout 設定が見つかりません"
else
    report "4（CI テストタイムアウト設定）" "PASS" "${CI_FILE} 全ジョブに timeout-minutes、${NEXTEST_CONFIG} に slow-timeout を確認しました"
fi

# --------------------------------------------------
# チェック 5: 依存方向の一方向性
#
# workspace 内クレート間の path 依存を cargo metadata から抽出し、
# (a) 循環依存がないこと（DFS によるサイクル検出）、
# (b) レイヤ順の許可リストに合致すること（Cargo.toml 冒頭コメントの分割方針:
#     core → http、routes → http、plugin-* → http の一方向。http はいかなる
#     workspace 内クレートにも依存してはならない最下層）
# を機械的に検査する。
# --------------------------------------------------
# cargo metadata / jq のいずれかが失敗すると edges_tsv が空文字列になり、以降の
# 循環検出・許可リストチェックのループが「対象 0 件」として素通りしてしまい、
# 依存グラフを一切検証していないのに check 5 が PASS を報告する誤判定になる。
# 各コマンドの終了ステータスを個別に検証し、失敗時は FAIL として報告して
# 以降の判定ロジックを実行しない。
metadata_json=$(cargo metadata --no-deps --format-version 1 2>/tmp/accept-task-11-5-metadata.log)
metadata_status=$?
edges_tsv=""
metadata_ok=1
if [ "${metadata_status}" -ne 0 ]; then
    metadata_ok=0
else
    edges_tsv=$(printf '%s' "${metadata_json}" | jq -r '.packages[] | .name as $from | .dependencies[] | select(.path != null) | "\($from)\t\(.name)"' 2>/tmp/accept-task-11-5-jq.log)
    jq_status=$?
    if [ "${jq_status}" -ne 0 ]; then
        metadata_ok=0
    fi
fi

if [ "${metadata_ok}" -ne 1 ]; then
    report "5（依存方向の一方向性）" "FAIL" "cargo metadata / jq による依存グラフ抽出に失敗しました。詳細: /tmp/accept-task-11-5-metadata.log, /tmp/accept-task-11-5-jq.log"
else

cycle_found=0
if [ -n "${edges_tsv}" ]; then
    declare -A visiting
    declare -A visited
    declare -A adj

    while IFS=$'\t' read -r from to; do
        [ -z "${from}" ] && continue
        adj["${from}"]="${adj[${from}]:-} ${to}"
    done <<< "${edges_tsv}"

    dfs_has_cycle() {
        local node="$1"
        visiting["${node}"]=1
        local neighbor
        for neighbor in ${adj[${node}]:-}; do
            if [ "${visiting[${neighbor}]:-0}" = "1" ]; then
                return 0
            fi
            if [ "${visited[${neighbor}]:-0}" != "1" ]; then
                if dfs_has_cycle "${neighbor}"; then
                    return 0
                fi
            fi
        done
        visiting["${node}"]=0
        visited["${node}"]=1
        return 1
    }

    mapfile -t all_nodes < <(printf '%s\n' "${edges_tsv}" | cut -f1 | sort -u)
    for node in "${all_nodes[@]}"; do
        if [ "${visited[${node}]:-0}" != "1" ]; then
            if dfs_has_cycle "${node}"; then
                cycle_found=1
                break
            fi
        fi
    done
fi

# レイヤ順の許可リスト（from パターン:to パターン）。fnmatch 相当の shell パターンで判定する。
# TASK-1.5（#14）で crates/routes（bf-routes）が新設され、`server → routes → http::*`
# の中間層としてグラフに加わった。core は bf-routes 経由でも bf-http を直接（server.rs
# が bf_http::request/response 型を参照するため）参照し続けるため、両エッジを許可する。
allowed_edge_patterns=(
    "backend-framework-core:bf-http"
    "backend-framework-core:bf-routes"
    "bf-routes:bf-http"
    "bf-plugin-*:bf-http"
    "bf-plugin-*:bf-routes"
    "bf-plugin-*:backend-framework-core"
    "*routes*:bf-http"
)

violating_edges=()
if [ -n "${edges_tsv}" ]; then
    while IFS=$'\t' read -r from to; do
        [ -z "${from}" ] && continue
        allowed=0
        for pattern in "${allowed_edge_patterns[@]}"; do
            from_pat="${pattern%%:*}"
            to_pat="${pattern##*:}"
            # shellcheck disable=SC2053  # パターンマッチとして意図的に未クォート展開
            if [[ "${from}" == ${from_pat} && "${to}" == ${to_pat} ]]; then
                allowed=1
                break
            fi
        done
        if [ "${allowed}" -eq 0 ]; then
            violating_edges+=("${from} -> ${to}")
        fi
    done <<< "${edges_tsv}"
fi

if [ "${cycle_found}" -eq 1 ]; then
    report "5（依存方向の一方向性）" "FAIL" "workspace 内クレート間の依存に循環が検出されました"
elif [ "${#violating_edges[@]}" -gt 0 ]; then
    report "5（依存方向の一方向性）" "FAIL" "許可リスト外の依存エッジがあります: ${violating_edges[*]}"
else
    report "5（依存方向の一方向性）" "PASS" "循環なし・全エッジがレイヤ順の許可リストに合致（$(echo "${edges_tsv}" | tr '\n' ';' | sed 's/\t/->/g')）"
fi

fi  # metadata_ok

echo "==================================================="
if [ "${overall_fail}" -ne 0 ]; then
    echo "==> accept-task-11-5.sh: 1 件以上のチェックが FAIL しました"
    exit 1
fi
if [ "${pending_count}" -gt 0 ]; then
    echo "==> accept-task-11-5.sh: FAIL なし・PENDING ${pending_count} 件（前提イシュー待ち）"
    exit 0
fi
echo "==> accept-task-11-5.sh: 全チェック PASS"
exit 0
