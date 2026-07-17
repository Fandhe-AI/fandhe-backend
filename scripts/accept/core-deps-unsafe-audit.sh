#!/usr/bin/env bash
# REQ-1（最小コア、docs/spec/04-requirements.md）の非性能系受け入れ基準を検証する
# 受け入れテストスクリプト（TASK-1.6-2、#72）。
#
# このスクリプトの役割:
#   性能計測（RPS・レイテンシ・RSS・バイナリサイズ・起動時間）は姉妹イシュー
#   TASK-1.6-1（#71）が benches/ で担当する。本スクリプトはそれ以外の受け入れ基準
#   （依存クレート数比・unsafe 根拠・audit/deny・実質コード行数・拡張点・
#   プラグイン非依存）を検証する。
#
# 検証対象クレートは動的に検出する（TASK-1.4-2 #70 のコアループ・TASK-1.5 #14 の
# routes クレートが並列進行中で未マージの可能性があるため）。存在しない対象は
# SKIP として記録し、非 0 終了させない。前提タスクマージ後に再実行すれば
# 完全な受け入れ判定になる（べき等・再実行可能）。
#
# 呼び出し元: 人間 / CI（将来 TASK-15.2 #17 で組み込み予定、本スクリプトはまだ
# CI に組み込まない）が `./scripts/accept/core-deps-unsafe-audit.sh` として直接実行する。

set -euo pipefail

# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

cd "${WORKSPACE_ROOT}"

echo "=== REQ-1 受け入れ検証（依存数・unsafe・監査） ==="
echo "workspace root: ${WORKSPACE_ROOT}"
echo ""

# 検証対象コアクレートの動的検出。crates/routes は TASK-1.5（#14）が並列進行中で
# 本スクリプト実行時点では存在しない場合がある。
CORE_DIRS=("crates/core" "crates/http")
if [ -d "crates/routes" ]; then
    CORE_DIRS+=("crates/routes")
    ROUTES_PRESENT=1
else
    ROUTES_PRESENT=0
fi

# ---------------------------------------------------------------------------
# 基準 A: コアの推移的依存クレート数が axum-ref の 50% 以下
# ---------------------------------------------------------------------------
check_dep_count() {
    if [ ! -f "Cargo.lock" ]; then
        cargo generate-lockfile >/dev/null 2>&1
    fi

    local core_deps axum_deps ratio_pct
    core_deps="$(cargo tree -p backend-framework-core -e normal --prefix none 2>/dev/null | sed 's/ v[0-9].*$//' | sort -u | wc -l | tr -d ' ')"
    axum_deps="$(cargo tree -p axum-ref -e normal --prefix none 2>/dev/null | sed 's/ v[0-9].*$//' | sort -u | wc -l | tr -d ' ')"

    if [ "${axum_deps}" -eq 0 ]; then
        record_fail "A: 依存クレート数比" "axum-ref の依存数が 0 と算出された（測定不能）"
        return
    fi

    # 整数演算で比率を切り捨て算出（bc 非依存）。
    ratio_pct=$(((core_deps * 100) / axum_deps))

    local detail="core=${core_deps} 種類 / axum-ref=${axum_deps} 種類（比率 ${ratio_pct}%、自クレート含む同一手法での cargo tree -e normal 集計）"
    if [ "${core_deps}" -le $((axum_deps / 2)) ]; then
        record_pass "A: 依存クレート数比 <=50%" "${detail}"
    else
        record_fail "A: 依存クレート数比 <=50%" "${detail}"
    fi
}

# ---------------------------------------------------------------------------
# 基準 B: 自コード unsafe 0 件、または各箇所に // SAFETY: 根拠 100% 記述
# ---------------------------------------------------------------------------
check_unsafe() {
    local dir unsafe_lines_all=""
    for dir in "${CORE_DIRS[@]}"; do
        if [ -d "${dir}/src" ]; then
            # grep は該当 0 件で終了コード 1 を返すため `|| true` で set -e を回避する。
            # 0 件（PASS ケース）が本チェックの主経路であるため必須のガード。
            local hits
            hits="$(grep -rn --include='*.rs' -E '\bunsafe\b' "${dir}/src" || true)"
            if [ -n "${hits}" ]; then
                unsafe_lines_all="${unsafe_lines_all}${hits}
"
            fi
        fi
    done

    if [ -z "${unsafe_lines_all}" ]; then
        record_pass "B: unsafe 0件/根拠明記" "対象コアクレート（${CORE_DIRS[*]}）の src/ に unsafe 0 件"
    else
        # unsafe が見つかった箇所ごとに直前の非空行へ // SAFETY: があるか検査する。
        # 空行を挟んで SAFETY コメントを書いた場合の false FAIL を避けるため、
        # 直前行から遡って最初に現れる非空行のみを見る（ブロックコメント
        # `/* ... */` 中の記述までは検査しない grep ベースの限界は残る。README に明記）。
        local missing_safety=0
        local file line
        while IFS=: read -r file line _rest; do
            local prev_line=$((line - 1))
            local found_safety=0
            while [ "${prev_line}" -ge 1 ]; do
                local prev_content
                prev_content="$(sed -n "${prev_line}p" "${file}" 2>/dev/null || true)"
                if [ -z "$(echo "${prev_content}" | tr -d '[:space:]')" ]; then
                    prev_line=$((prev_line - 1))
                    continue
                fi
                if echo "${prev_content}" | grep -q '// SAFETY:'; then
                    found_safety=1
                fi
                break
            done
            if [ "${found_safety}" -eq 0 ]; then
                missing_safety=1
                echo "  根拠欠落候補: ${file}:${line}" >&2
            fi
        done <<<"${unsafe_lines_all}"

        if [ "${missing_safety}" -eq 0 ]; then
            record_pass "B: unsafe 0件/根拠明記" "unsafe 使用箇所すべてに直前行の // SAFETY: コメントあり"
        else
            record_fail "B: unsafe 0件/根拠明記" "// SAFETY: 根拠を欠く unsafe 箇所あり（標準エラー出力参照）"
        fi
    fi

    # workspace lint による実質 deny 体制の確認（参考記録、grep 検証の補強）。
    if grep -q 'unsafe_code = "warn"' Cargo.toml 2>/dev/null; then
        record_warn "B補足: workspace lint" "ルート Cargo.toml で unsafe_code=\"warn\" を設定済み。CI の clippy -D warnings と組み合わせ実質 deny として機能（.claude/rules/security.md）"
    fi

    # cargo geiger は導入済みの場合のみ参考値として実行する（自動導入しない）。
    if check_tool cargo-geiger "cargo install cargo-geiger"; then
        record_warn "B補足: cargo geiger" "$(cargo geiger --output-format Ascii -p backend-framework-core 2>/dev/null | tail -5 | tr '\n' ' ' || echo '実行に失敗（参考値のため受け入れ判定に影響しない）')"
    else
        record_skip "B補足: cargo geiger" "cargo-geiger 未導入のため参考値なし（導入: cargo install cargo-geiger）"
    fi
}

# ---------------------------------------------------------------------------
# 基準 C: cargo audit 既知脆弱性 0 件・cargo deny check ライセンス/出所違反 0 件
# ---------------------------------------------------------------------------
check_audit_and_deny() {
    if [ ! -f "Cargo.lock" ]; then
        cargo generate-lockfile >/dev/null 2>&1
    fi

    if check_tool cargo-audit "cargo install cargo-audit"; then
        local audit_out audit_status
        set +e
        audit_out="$(cargo audit 2>&1)"
        audit_status=$?
        set -e
        if [ "${audit_status}" -eq 0 ]; then
            record_pass "C: cargo audit 既知脆弱性 0件" "$(echo "${audit_out}" | tail -3 | tr '\n' ' ')"
        else
            # axum-ref は比較専用の参照実装であり、そこ由来の advisory はコア
            # （crates/core・crates/http）の受け入れ基準そのものを直ちに FAIL にはしない。
            # ただし握りつぶさず、混入経路をレポートへ明記して報告する。
            record_fail "C: cargo audit 既知脆弱性 0件" "検出あり。出力: $(echo "${audit_out}" | tail -10 | tr '\n' ' ')"
        fi
    else
        record_skip "C: cargo audit 既知脆弱性 0件" "cargo-audit 未導入（導入: cargo install cargo-audit）。受け入れ判定には実行必須のため要再実行"
    fi

    if [ -f "deny.toml" ]; then
        if check_tool cargo-deny "cargo install cargo-deny"; then
            local deny_out deny_status
            set +e
            deny_out="$(cargo deny check 2>&1)"
            deny_status=$?
            set -e
            if [ "${deny_status}" -eq 0 ]; then
                record_pass "C: cargo deny check" "deny.toml による全項目チェックで違反 0 件"
            else
                record_fail "C: cargo deny check" "違反あり: $(echo "${deny_out}" | tail -10 | tr '\n' ' ')"
            fi
        else
            record_skip "C: cargo deny check" "cargo-deny 未導入（導入: cargo install cargo-deny）"
        fi
    else
        # deny.toml 整備は TASK-15.1（#16）のスコープ。既定設定で advisories/bans/sources
        # のみ実行し、licenses チェックは #16 待ちとして WARN に留める（out-of-scope-tracking）。
        if check_tool cargo-deny "cargo install cargo-deny"; then
            local deny_out deny_status
            set +e
            deny_out="$(cargo deny check advisories bans sources 2>&1)"
            deny_status=$?
            set -e
            if [ "${deny_status}" -eq 0 ]; then
                record_warn "C: cargo deny check（既定設定）" "deny.toml 未整備のため既定設定で advisories/bans/sources のみ実行し違反 0 件。licenses は #16（TASK-15.1）待ち"
            else
                record_fail "C: cargo deny check（既定設定）" "advisories/bans/sources で違反あり: $(echo "${deny_out}" | tail -10 | tr '\n' ' ')"
            fi
        else
            record_skip "C: cargo deny check" "cargo-deny 未導入（導入: cargo install cargo-deny）。deny.toml も未整備（#16 待ち）"
        fi
    fi
}

# ---------------------------------------------------------------------------
# 基準 D: コア実質コード行数 5,000 行以内（空行・// コメント行を除く）
# ---------------------------------------------------------------------------
check_loc() {
    local dir total=0
    for dir in "${CORE_DIRS[@]}"; do
        if [ -d "${dir}/src" ]; then
            local n
            n="$(find "${dir}/src" -name '*.rs' -exec cat {} + 2>/dev/null | \
                grep -v -E '^\s*$' | grep -v -E '^\s*//' | wc -l | tr -d ' ')"
            total=$((total + n))
        fi
    done

    local detail="実質コード行数（空行・// コメント行除外、/* */ ブロックコメントは未除外のため参考値に上振れの可能性あり）: ${total} 行（対象: ${CORE_DIRS[*]}）"
    if check_tool tokei ""; then
        detail="${detail} / tokei 参考値: $(tokei "${CORE_DIRS[@]}" --output json 2>/dev/null | grep -o '"code":[0-9]*' | head -1 || echo 'N/A')"
    fi

    if [ "${total}" -le 5000 ]; then
        record_pass "D: コア実質コード行数 <=5000" "${detail}"
    else
        record_fail "D: コア実質コード行数 <=5000" "${detail}"
    fi
}

# ---------------------------------------------------------------------------
# 基準 E: 3 種拡張点が trait 定義され、コアループ本体が feature 有無で分岐しない
# ---------------------------------------------------------------------------
check_extension_points() {
    local missing=""
    local trait_name
    for trait_name in Middleware UpgradeHandler RequestGate; do
        if ! grep -rq --include='*.rs' -E "trait[[:space:]]+${trait_name}\b" crates/core/src 2>/dev/null; then
            missing="${missing} ${trait_name}"
        fi
    done

    if [ -z "${missing}" ]; then
        record_pass "E: 3拡張点 trait 定義" "Middleware / UpgradeHandler / RequestGate すべて crates/core/src に定義あり"
    else
        record_fail "E: 3拡張点 trait 定義" "未定義:${missing}"
    fi

    # コアループ本体（接続受理・リクエストループ）は TASK-1.4-2（#70）で追加される。
    # 現時点で lib.rs 以外に実装ファイルが増えていない場合は SKIP とし、
    # #70 マージ後の再実行で本チェックを完了させる。
    local loop_files
    loop_files="$(find crates/core/src -name '*.rs' ! -name 'lib.rs' ! -name 'extension.rs' 2>/dev/null || true)"
    if [ -z "${loop_files}" ]; then
        record_skip "E: コアループの feature 非分岐" "コアループ実装（TASK-1.4-2 #70）が本 worktree 未マージのため検証対象なし。マージ後に再実行すること"
    else
        local cfg_hits
        cfg_hits="$(echo "${loop_files}" | xargs grep -l '#\[cfg(feature' 2>/dev/null || true)"
        if [ -z "${cfg_hits}" ]; then
            record_pass "E: コアループの feature 非分岐" "コアループ実装ファイルに #[cfg(feature ...)] 分岐なし"
        else
            record_fail "E: コアループの feature 非分岐" "feature 分岐を検出: ${cfg_hits}"
        fi
    fi
}

# ---------------------------------------------------------------------------
# 基準 F: routes・http/ がプラグイン固有シンボルへ依存しない
# ---------------------------------------------------------------------------
check_plugin_independence() {
    local target_dirs=("crates/http")
    if [ "${ROUTES_PRESENT}" -eq 1 ]; then
        target_dirs+=("crates/routes")
    else
        record_skip "F: routes のプラグイン非依存" "crates/routes（TASK-1.5 #14）が本 worktree 未作成のため検証対象なし。作成後に再実行すること"
    fi

    local dir hits_all=""
    for dir in "${target_dirs[@]}"; do
        if [ -d "${dir}/src" ]; then
            # 日本語 doc comment 中の「プラグイン」誤検出を避けるため、// 行コメントは除外して
            # 識別子パターン（Plugin/plugin を含む識別子）のみを検査する。
            # 注意: grep -rn の出力は "file:line:content" 形式のため、除外パターンは
            # 素の "^\s*//" ではなく "file:line:" プレフィックスを踏まえたものにする必要がある
            # （プレフィックス無視の "^\s*//" は常に不一致となり、コメント行を除外できない
            # 誤りだった。TASK-1.6-2 #72 レビューで検出）。
            local hits
            hits="$(grep -rn --include='*.rs' -E '[A-Za-z_]*[Pp]lugin' "${dir}/src" | grep -v -E '^[^:]*:[0-9]+:[[:space:]]*//' || true)"
            if [ -n "${hits}" ]; then
                hits_all="${hits_all}${hits}
"
            fi
        fi
        if [ -f "${dir}/Cargo.toml" ] && grep -q 'plugin-' "${dir}/Cargo.toml"; then
            hits_all="${hits_all}${dir}/Cargo.toml に plugin- 依存あり
"
        fi
    done

    if [ ${#target_dirs[@]} -gt 0 ] && [ -d "crates/http/src" ]; then
        # ラベルは実際に検証した対象のみを反映する。`${ROUTES_PRESENT:+/routes}` のような
        # `:+` 展開は "0"（非空文字列）でも真になり "検証していない routes" を
        # ラベルに含めてしまう誤りがあったため、条件分岐で明示的に組み立てる。
        local label="F: プラグイン非依存（http）"
        if [ "${ROUTES_PRESENT}" -eq 1 ]; then
            label="F: プラグイン非依存（http/routes）"
        fi
        if [ -z "${hits_all}" ]; then
            record_pass "${label}" "対象（${target_dirs[*]}）にプラグイン固有シンボル・依存を検出せず"
        else
            record_fail "${label}" "検出: ${hits_all}"
        fi
    fi
}

check_dep_count
check_unsafe
check_audit_and_deny
check_loc
check_extension_points
check_plugin_independence

print_summary
exit "$(summary_exit_code)"
