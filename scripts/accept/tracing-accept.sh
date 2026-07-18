#!/usr/bin/env bash
# REQ-10（可観測性）TASK-10.4（#59）サンプリング適用後性能再検証 +
# TASK-10.5（#60）依存インパクト記録・文書化の受け入れ検証オーケストレータ。
#
# `docs/spec/05-tasks.md` TASK-10.4 / TASK-10.5 の受け入れ基準を機械検証する:
#   A: `tracing` feature 無効時、`backend-framework-core` の依存ツリーに
#      `bf-plugin-tracing` / `tracing*` 系依存が一切現れない
#      （pay-for-what-you-use の完全除外、`.claude/rules/pay-for-what-you-use.md`）
#   B: 全 feature 構成でのテスト回帰（`cargo test -p backend-framework-core`
#      無効/有効の両方、`cargo test -p bf-plugin-tracing`）が成功する
#   C: NFR（TASK-10.1〜10.3 の全緩和策適用後、`GET /health` への RPS 劣化 5% 以内・
#      p95 悪化 110% 以内）。ビルド済み計測用バイナリ（`target/release/examples/minimal`・
#      `target/release/examples/tracing_nfr`）と `oha` が揃っていれば
#      `benches/tracing-nfr-bench.sh` で empirical 計測する。揃っていなければ
#      判定不能として SKIP + 実行手順を案内する（フェイルクローズ、自動ビルド・
#      自動ダウンロードは行わない）
#   D（TASK-10.5）: 依存インパクト記録・連携方式設計文書の存在検証
#      （`docs/dep-impact/records.md` の plugin-tracing エントリ・
#      `docs/design/tracing-integration.md` の存在を grep）
#   E（TASK-10.5）: 依存クレート数増分の機械検証（`cargo tree -p
#      backend-framework-core --features tracing` の union 展開差分件数を算出し、
#      `docs/dep-impact/records.md` 記録値（+24）と突合）。バイナリサイズ・RSS は
#      A/C チェックと同じくビルド済みバイナリが無ければフェイルクローズで SKIP
#
# 基準未達（FAIL）でも `docs/spec/06-roadmap.md` の分岐どおり「デフォルト無効・
# 明示的 opt-in feature」を維持する結論自体は成立する（現状 `default = []`）。
# 本スクリプトは実測結果を PASS/WARN/FAIL として機械的に記録するのみで、
# 分岐判断そのものはレポート（`benches/reports/task-10.4-tracing-performance.md`）
# 側の役割とする。判定不能はフェイルクローズで SKIP とし、PASS と偽らない
# （`.claude/rules/security.md`）。`scripts/accept/graphql-accept.sh`
# （TASK-5.2 / #53）と同型のオーケストレータ。
#
# 呼び出し元: 人間が `bash scripts/accept/tracing-accept.sh` として直接実行する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
cd "${WORKSPACE_ROOT}"

# TASK-10.4 固有の受け入れ閾値（実装計画・#59）。
# graphql-accept.sh / webrtc-accept.sh の NFR-6 判定帯（狭義 100.3〜100.8%）とは
# 別物で、REQ-10 の成功基準「RPS 劣化 5% 以内・p95 悪化 110% 以内」をそのまま帯とする。
RPS_RATIO_MIN=95
P95_RATIO_MAX=110

echo "=== REQ-10 / TASK-10.4 受け入れ検証（サンプリング適用後性能再検証） ==="
echo "workspace root: ${WORKSPACE_ROOT}"
echo ""

# ---------------------------------------------------------------------------
# A: tracing feature 無効時の完全除外
# ---------------------------------------------------------------------------
check_dep_exclusion() {
    if [ ! -d "crates/plugin-tracing" ]; then
        record_skip "A: tracing 無効時の依存完全除外" "crates/plugin-tracing（TASK-10.1 #56）が本 worktree 未存在のため検証対象なし"
        return
    fi

    # release ビルドの依存グラフのみを対象にする（dev-dependency は除外）。
    # `crates/core/Cargo.toml` は `plugin_tracing_boundary.rs` 用に tracing /
    # tracing-subscriber を dev-dependency として持つため、これを含めると
    # 「残留」を誤検知する（graphql-accept.sh の check_dep_exclusion と同一対策）。
    local tree_output disabled_count
    if ! tree_output="$(cargo tree -p backend-framework-core -e normal --no-default-features 2>/dev/null)"; then
        record_fail "A: tracing 無効時の依存完全除外" "cargo tree -p backend-framework-core -e normal --no-default-features 自体が失敗し測定不能"
        return
    fi
    disabled_count="$(printf '%s\n' "${tree_output}" | grep -c -E 'bf-plugin-tracing|tracing-appender|tracing-subscriber' || true)"

    if [ "${disabled_count}" -eq 0 ]; then
        record_pass "A: tracing 無効時の依存完全除外" "cargo tree -p backend-framework-core -e normal --no-default-features | grep -c -E 'bf-plugin-tracing|tracing-appender|tracing-subscriber' = 0"
    else
        record_fail "A: tracing 無効時の依存完全除外" "tracing 系依存が ${disabled_count} 件残留（cargo tree -p backend-framework-core -e normal --no-default-features）"
    fi

    # 陽性対照: --features tracing では出現すること（列挙腐敗・配線切れの検知）。
    local enabled_tree_output enabled_count
    if ! enabled_tree_output="$(cargo tree -p backend-framework-core -e normal --no-default-features --features tracing 2>/dev/null)"; then
        record_warn "A補足: tracing 有効時の依存インパクト（陽性対照）" "cargo tree -p backend-framework-core -e normal --no-default-features --features tracing 自体が失敗し測定不能"
        return
    fi
    enabled_count="$(printf '%s\n' "${enabled_tree_output}" | grep -c -E 'bf-plugin-tracing|tracing-appender|tracing-subscriber' || true)"
    if [ "${enabled_count}" -eq 0 ]; then
        record_fail "A補足: tracing 有効時の依存インパクト（陽性対照）" "cargo tree -p backend-framework-core -e normal --no-default-features --features tracing に tracing 系依存が 0 件（配線切れ・列挙腐敗の疑い）"
    else
        record_warn "A補足: tracing 有効時の依存インパクト（陽性対照）" "cargo tree -p backend-framework-core -e normal --no-default-features --features tracing | grep -c -E 'bf-plugin-tracing|tracing-appender|tracing-subscriber' = ${enabled_count}"
    fi
}

# ---------------------------------------------------------------------------
# B: テスト回帰
# ---------------------------------------------------------------------------
check_regression() {
    local out status

    set +e
    out="$(cargo test -p backend-framework-core --no-default-features 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        record_pass "B: cargo test -p backend-framework-core --no-default-features" "tracing feature 無効時のフォールスルーを含め成功"
    else
        record_fail "B: cargo test -p backend-framework-core --no-default-features" "非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi

    set +e
    out="$(cargo test -p backend-framework-core --features tracing 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        record_pass "B: cargo test -p backend-framework-core --features tracing" "plugin_tracing_boundary.rs（サンプリング判定・除外パス）を含め成功"
    else
        record_fail "B: cargo test -p backend-framework-core --features tracing" "非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi

    set +e
    out="$(cargo test -p bf-plugin-tracing 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        record_pass "B: cargo test -p bf-plugin-tracing" "Sampler / TracingConfig / TracingLayer の契約テストが成功"
    else
        record_fail "B: cargo test -p bf-plugin-tracing" "非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi
}

# ---------------------------------------------------------------------------
# C: NFR（サンプリング適用後の RPS・p95 影響、シナリオ A のみを受け入れ判定対象とする）
# ---------------------------------------------------------------------------
check_nfr() {
    local baseline_bin="${WORKSPACE_ROOT}/target/release/examples/minimal"
    local tracing_bin="${WORKSPACE_ROOT}/target/release/examples/tracing_nfr"

    if ! command -v oha >/dev/null 2>&1; then
        record_skip "C: NFR サンプリング適用後の性能影響" "oha 未導入（導入: cargo install oha）。導入後 benches/tracing-nfr-bench.sh を実行して再判定すること"
        return
    fi
    if [ ! -x "${baseline_bin}" ] || [ ! -x "${tracing_bin}" ]; then
        record_skip "C: NFR サンプリング適用後の性能影響" "計測用バイナリ未ビルド。'cargo build --release -p backend-framework-core --example minimal --no-default-features' と '... --example tracing_nfr --features tracing' を実行後、benches/tracing-nfr-bench.sh を実行して再判定すること"
        return
    fi

    local tmp_log
    tmp_log="$(mktemp)"
    local out
    out="$(bash "${WORKSPACE_ROOT}/benches/tracing-nfr-bench.sh" 2>"${tmp_log}")" || {
        record_fail "C: NFR サンプリング適用後の性能影響" "benches/tracing-nfr-bench.sh が失敗: $(tail -10 "${tmp_log}" | tr '\n' ' ')"
        rm -f "${tmp_log}"
        return
    }
    rm -f "${tmp_log}"

    local rps_ratio_pct p95_ratio_pct
    rps_ratio_pct="$(echo "${out}" | grep '^rps_a_ratio_pct=' | cut -d= -f2)"
    p95_ratio_pct="$(echo "${out}" | grep '^p95_a_ratio_pct=' | cut -d= -f2)"

    local detail="シナリオA（サンプリング + イベント統合 + /health 除外）RPS 比 ${rps_ratio_pct}% / p95 比 ${p95_ratio_pct}%（tracing 有効 / ベースライン、GET /health への負荷計測）。受け入れ帯: RPS 比 >= ${RPS_RATIO_MIN}% かつ p95 比 <= ${P95_RATIO_MAX}%（REQ-10 成功基準）。シナリオB（除外なし・参考値）は benches/reports/task-10.4-tracing-performance.md 参照"

    local rps_ok p95_ok
    # ロケール非依存の小数点判定（LC_NUMERIC=C、graphql-accept.sh / webrtc-accept.sh と同一対策）。
    rps_ok="$(LC_NUMERIC=C awk -v v="${rps_ratio_pct}" -v lo="${RPS_RATIO_MIN}" 'BEGIN { print (v >= lo) ? 1 : 0 }')"
    p95_ok="$(LC_NUMERIC=C awk -v v="${p95_ratio_pct}" -v hi="${P95_RATIO_MAX}" 'BEGIN { print (v <= hi) ? 1 : 0 }')"

    if [ "${rps_ok}" -eq 1 ] && [ "${p95_ok}" -eq 1 ]; then
        record_pass "C: NFR サンプリング適用後の性能影響" "${detail}"
    else
        # FAIL でも「デフォルト無効・opt-in 維持」でタスクは完了可能（実装計画 7 節）。
        # 本スクリプトは実測を機械記録するのみで、分岐判断はレポート側の役割とする。
        record_fail "C: NFR サンプリング適用後の性能影響" "${detail}（基準未達。spec の安全側分岐＝デフォルト無効・opt-in 維持＝の適用はレポート側で判断する）"
    fi
}

# ---------------------------------------------------------------------------
# D: 依存インパクト記録・連携方式設計文書の存在検証（TASK-10.5、#60）
# ---------------------------------------------------------------------------
check_dep_impact_docs() {
    if grep -q "crates/plugin-tracing.*依存インパクト記録" docs/dep-impact/records.md 2>/dev/null; then
        record_pass "D: 依存インパクト記録の存在（docs/dep-impact/records.md）" "plugin-tracing エントリを検出"
    else
        record_fail "D: 依存インパクト記録の存在（docs/dep-impact/records.md）" "plugin-tracing 依存インパクトエントリが見つからない"
    fi

    if [ -f "docs/design/tracing-integration.md" ]; then
        record_pass "D: 連携方式設計文書の存在（docs/design/tracing-integration.md）" "ファイル存在を確認"
    else
        record_fail "D: 連携方式設計文書の存在（docs/design/tracing-integration.md）" "ファイルが見つからない"
    fi
}

# ---------------------------------------------------------------------------
# E: 依存クレート数増分の機械検証（TASK-10.5、#60）
# ---------------------------------------------------------------------------
check_dep_count_increment() {
    if [ ! -d "crates/plugin-tracing" ]; then
        record_skip "E: 依存クレート数増分の機械検証" "crates/plugin-tracing が本 worktree 未存在のため検証対象なし"
        return
    fi

    # `name vX.Y.Z` 形式のユニークパッケージ行を union 展開して集合差分を取る
    # （records.md の既存手法、A チェックの grep -c は行出現数のみで実クレート数
    # とは一致しないため別集計とする）。
    local disabled_pkgs enabled_pkgs disabled_count enabled_count new_count
    disabled_pkgs="$(cargo tree -p backend-framework-core -e normal --no-default-features 2>/dev/null \
        | sed -E 's/^[│├└─ ]*//; s/ \(\*\)$//' \
        | grep -E '^[a-zA-Z0-9_-]+ v[0-9]' | sort -u)"
    enabled_pkgs="$(cargo tree -p backend-framework-core -e normal --no-default-features --features tracing 2>/dev/null \
        | sed -E 's/^[│├└─ ]*//; s/ \(\*\)$//' \
        | grep -E '^[a-zA-Z0-9_-]+ v[0-9]' | sort -u)"

    # disabled 側（無効時ベースライン）が空・パース不能な場合も enabled 側と同様に
    # fail closed する。ここを素通りさせると comm -13 の差分が水増しされ、稀に
    # ハードコードされた +24 の許容帯へ偶然一致して「無効時 0 件」という虚偽の
    # PASS/WARN を出しかねない（Bugbot 指摘、PR #160 review-4727137460）。
    if [ -z "${disabled_pkgs}" ]; then
        record_fail "E: 依存クレート数増分の機械検証" "cargo tree -p backend-framework-core -e normal --no-default-features が空・失敗（無効時ベースライン取得不可のため新規クレート数を算出不能）"
        return
    fi

    if [ -z "${enabled_pkgs}" ]; then
        record_fail "E: 依存クレート数増分の機械検証" "cargo tree -p backend-framework-core -e normal --no-default-features --features tracing が空・失敗"
        return
    fi

    disabled_count="$(printf '%s\n' "${disabled_pkgs}" | grep -c . || true)"
    enabled_count="$(printf '%s\n' "${enabled_pkgs}" | grep -c . || true)"
    new_count="$(comm -13 <(printf '%s\n' "${disabled_pkgs}") <(printf '%s\n' "${enabled_pkgs}") | grep -c . || true)"

    # records.md 記録値（+24、2026-07-18 エントリ）との突合。`Cargo.lock` 更新等で
    # 若干変動しうるため許容帯を持たせる（webrtc-accept.sh の帯判定と同様の方針）。
    local expected=24
    local tolerance=5
    local diff_abs
    diff_abs="$(( new_count > expected ? new_count - expected : expected - new_count ))"

    if [ "${diff_abs}" -le "${tolerance}" ]; then
        record_pass "E: 依存クレート数増分の機械検証" "無効時 ${disabled_count} 件 → 有効時 ${enabled_count} 件（union 展開、新規 +${new_count} 件）。records.md 記録値 +${expected} 件と許容帯（±${tolerance}）内で一致"
    else
        record_warn "E: 依存クレート数増分の機械検証" "無効時 ${disabled_count} 件 → 有効時 ${enabled_count} 件（union 展開、新規 +${new_count} 件）。records.md 記録値 +${expected} 件と乖離（許容帯 ±${tolerance} 超過、Cargo.lock 更新等の環境差の可能性。records.md 再計測・更新を検討）"
    fi
}

check_dep_exclusion
check_regression
check_nfr
check_dep_impact_docs
check_dep_count_increment

print_summary "REQ-10、TASK-10.4 / #59、TASK-10.5 / #60"
exit "$(summary_exit_code)"
