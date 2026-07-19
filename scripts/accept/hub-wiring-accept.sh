#!/usr/bin/env bash
# REQ-9（hub 共通配線）TASK-9.5（#65）の受け入れ検証オーケストレータ。
#
# `docs/spec/05-tasks.md` TASK-9.5「hub 共通配線受け入れテスト」の受け入れ基準を
# 機械検証する:
#   A: 越境クエリ・フェイルクローズの受け入れテスト（`tests/hub_acceptance.rs`）が
#      全件 PASS すること
#   B: 配線コード削減率。`examples/hub_service_demo.rs` のマーカー区間（`// ---
#      wiring:begin --- 〜 // --- wiring:end ---`）の LOC を PoC-6 基準（3 エンドポイント・
#      207 行）に対して評価し、ハンドラ領域に手書き JWT 検証・JWKS パース等の配線
#      シンボルが現れないこと（`scripts/accept/lib/hub-wiring-loc.sh`）
#   C: 依存方向・pay-for-what-you-use。`cargo tree -p fandhe-backend-core` に
#      `fandhe-backend-plugin-hub-wiring` が現れないこと（依存逆転型プラグインの維持）
#   D: NFR-6（無関係パスへの RPS・p95 影響が誤差範囲内）。ビルド済み
#      `target/release/examples/minimal`・`target/release/examples/hub_link_only`
#      （`BF_HUB_GATE=off`、`hub_service_demo` のアプリ層オーバーヘッドを含まない
#      リンクコスト専用最小 example。Cursor Bugbot review 4727552092 指摘1対応）と
#      `oha` が揃っていれば `benches/hub-nfr6-bench.sh` で empirical 計測する。
#      揃っていなければ判定不能として SKIP + 実行手順を案内する
#      （フェイルクローズ、自動ビルド・自動ダウンロードは行わない）
#
# 判定不能（前提ツール未導入・前提クレート未マージ等）はフェイルクローズで
# FAIL または SKIP とし、PASS と偽らない（.claude/rules/security.md）。
#
# 呼び出し元: 人間が `bash scripts/accept/hub-wiring-accept.sh` として直接実行する。
# 判定ロジックのオフライン・セルフテストは
# `scripts/tests/run-hub-wiring-accept-tests.sh` を参照（cargo 非依存）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
# shellcheck source=lib/nfr6-ratio.sh
source "${SCRIPT_DIR}/lib/nfr6-ratio.sh"
# shellcheck source=lib/hub-wiring-loc.sh
source "${SCRIPT_DIR}/lib/hub-wiring-loc.sh"
cd "${WORKSPACE_ROOT}"

DEMO_EXAMPLE="crates/plugin-hub-wiring/examples/hub_service_demo.rs"

echo "=== REQ-9 / TASK-9.5 受け入れ検証（hub 共通配線） ==="
echo "workspace root: ${WORKSPACE_ROOT}"
echo ""

# ---------------------------------------------------------------------------
# A: 越境クエリ・フェイルクローズの受け入れテスト
# ---------------------------------------------------------------------------
check_acceptance_tests() {
    if [ ! -f "crates/plugin-hub-wiring/tests/hub_acceptance.rs" ]; then
        record_skip "A: 越境遮断・フェイルクローズ受け入れテスト" "crates/plugin-hub-wiring/tests/hub_acceptance.rs（TASK-9.5 / #65）が本 worktree 未存在のため検証対象なし"
        return
    fi
    if ! check_tool cargo "Rust ツールチェーン一式（rustup 等）"; then
        record_skip "A: 越境遮断・フェイルクローズ受け入れテスト" "cargo 未導入"
        return
    fi

    local out status
    set +e
    out="$(cargo test -p fandhe-backend-plugin-hub-wiring --test hub_acceptance 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        local test_count
        test_count="$(printf '%s\n' "${out}" | grep -oE '^test result: ok\. [0-9]+ passed' | grep -oE '[0-9]+' | head -1 || true)"
        record_pass "A: 越境遮断・フェイルクローズ受け入れテスト" "cargo test -p fandhe-backend-plugin-hub-wiring --test hub_acceptance 全件 PASS（${test_count:-?} 件）"
    else
        record_fail "A: 越境遮断・フェイルクローズ受け入れテスト" "cargo test が非 0 終了: $(printf '%s\n' "${out}" | tail -15 | tr '\n' ' ')"
    fi
}

# ---------------------------------------------------------------------------
# B: 配線コード削減率
# ---------------------------------------------------------------------------
check_wiring_reduction() {
    if [ ! -f "${DEMO_EXAMPLE}" ]; then
        record_skip "B: 配線コード削減率" "${DEMO_EXAMPLE}（TASK-9.5 / #65）が本 worktree 未存在のため検証対象なし"
        return
    fi

    if [ "$(has_wiring_markers "${DEMO_EXAMPLE}")" != "1" ]; then
        record_fail "B: 配線コード削減率" "${DEMO_EXAMPLE} に wiring:begin / wiring:end マーカーの対が見つからず判定不能（マーカーなしを満点扱いにする fail-open を避けるため FAIL とする）"
        return
    fi

    local actual_loc verdict_line verdict reduction_pct
    actual_loc="$(count_wiring_loc "${DEMO_EXAMPLE}")"
    verdict_line="$(evaluate_wiring_reduction "${actual_loc}")"
    verdict="${verdict_line%% *}"
    reduction_pct="${verdict_line#* }"

    if [ "${verdict}" = "PASS" ]; then
        record_pass "B: 配線コード削減率" "マーカー区間 ${actual_loc} 行（PoC-6 基準 207 行比 削減率 ${reduction_pct}%）"
    else
        record_fail "B: 配線コード削減率" "マーカー区間 ${actual_loc} 行（PoC-6 基準 207 行比 削減率 ${reduction_pct}%、90% 未満）"
    fi

    local handwritten
    handwritten="$(detect_handwritten_auth_in_handlers "${DEMO_EXAMPLE}")"
    if [ -z "${handwritten}" ]; then
        record_pass "B補足: ハンドラ領域の手書き配線シンボル不在" "verify_token / RsaKeyPair / JwksKeySet / SharedJwks::new / TenantGateConfig::(new|from_jwks_json) いずれも build_router 内に出現なし"
    else
        record_fail "B補足: ハンドラ領域の手書き配線シンボル不在" "ハンドラ領域に手書き配線シンボルを検出: $(printf '%s' "${handwritten}" | tr '\n' ' ')"
    fi
}

# ---------------------------------------------------------------------------
# C: 依存方向・pay-for-what-you-use
# ---------------------------------------------------------------------------
check_dependency_inversion() {
    local tree_output count
    if ! tree_output="$(cargo tree -p fandhe-backend-core 2>/dev/null)"; then
        record_fail "C: 依存逆転型プラグインの維持" "cargo tree -p fandhe-backend-core 自体が失敗し測定不能（cargo 呼び出しが壊れている可能性）"
        return
    fi
    count="$(printf '%s\n' "${tree_output}" | grep -c 'fandhe-backend-plugin-hub-wiring' || true)"
    if [ "${count}" -eq 0 ]; then
        record_pass "C: 依存逆転型プラグインの維持" "cargo tree -p fandhe-backend-core に fandhe-backend-plugin-hub-wiring が現れない（プラグイン→コアの一方向依存を維持）"
    else
        record_fail "C: 依存逆転型プラグインの維持" "fandhe-backend-core の依存ツリーに fandhe-backend-plugin-hub-wiring が ${count} 件残留"
    fi
}

# ---------------------------------------------------------------------------
# D: NFR-6（無関係パスへの RPS・レイテンシ影響）
# ---------------------------------------------------------------------------
check_nfr6() {
    local baseline_bin="${WORKSPACE_ROOT}/target/release/examples/minimal"
    local hub_bin="${WORKSPACE_ROOT}/target/release/examples/hub_link_only"

    if ! command -v oha >/dev/null 2>&1; then
        record_skip "D: NFR-6 無関係パス影響" "oha 未導入（導入: cargo install oha）。導入後 benches/hub-nfr6-bench.sh を実行して再判定すること"
        return
    fi
    if [ ! -x "${baseline_bin}" ] || [ ! -x "${hub_bin}" ]; then
        record_skip "D: NFR-6 無関係パス影響" "計測用バイナリ未ビルド。'cargo build --release -p fandhe-backend-core --example minimal --no-default-features' と 'cargo build --release -p fandhe-backend-plugin-hub-wiring --example hub_link_only' を実行後、benches/hub-nfr6-bench.sh を実行して再判定すること"
        return
    fi

    local out rps_ratio_pct p95_ratio_pct
    out="$(bash "${WORKSPACE_ROOT}/benches/hub-nfr6-bench.sh" 2>/tmp/hub-wiring-accept-nfr6.log)" || {
        record_fail "D: NFR-6 無関係パス影響" "benches/hub-nfr6-bench.sh が失敗: $(tail -10 /tmp/hub-wiring-accept-nfr6.log | tr '\n' ' ')"
        return
    }
    rps_ratio_pct="$(echo "${out}" | grep '^rps_ratio_pct=' | cut -d= -f2)"
    p95_ratio_pct="$(echo "${out}" | grep '^p95_ratio_pct=' | cut -d= -f2)"

    local verdict
    verdict="$(evaluate_nfr6_ratio "${rps_ratio_pct}" "${p95_ratio_pct}")"
    local detail="RPS 比 ${rps_ratio_pct}% / p95 比 ${p95_ratio_pct}%（hub_link_only・BF_HUB_GATE=off / ベースライン、GET / への負荷計測。狭義の NFR-6 帯 100.3〜100.8% との照合は benches/reports/task-9.5-hub-wiring-performance.md 参照）"
    case "${verdict}" in
    PASS)
        record_pass "D: NFR-6 無関係パス影響" "${detail}"
        ;;
    WARN)
        record_warn "D: NFR-6 無関係パス影響（実務許容帯内・狭義帯外）" "${detail}"
        ;;
    *)
        record_fail "D: NFR-6 無関係パス影響" "${detail}"
        ;;
    esac
}

check_acceptance_tests
check_wiring_reduction
check_dependency_inversion
check_nfr6

print_summary "REQ-9、TASK-9.5 / #65"
exit "$(summary_exit_code)"
