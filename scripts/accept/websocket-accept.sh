#!/usr/bin/env bash
# REQ-4（WebSocket）TASK-4.4（#25）の受け入れ検証オーケストレータ。
#
# `docs/spec/05-tasks.md` TASK-4.4「WebSocket プラグイン受け入れテスト」の受け入れ
# 基準を機械検証する:
#   A: `websocket` feature 無効時、`backend-framework-core` の依存ツリーに
#      `tokio-tungstenite` / `tungstenite` / `bf-plugin-websocket` 系依存が
#      一切現れない（pay-for-what-you-use の完全除外、
#      `.claude/rules/pay-for-what-you-use.md`）。
#      `scripts/pay-for-what-you-use-check.sh`（動的列挙のため websocket feature も
#      自動的に検証対象へ含まれる）も併走させ、依存・unsafe・バイナリサイズ除外を
#      二重に確認する
#   B: 回帰テスト（`cargo test -p backend-framework-core --features websocket` /
#      `cargo test -p bf-plugin-websocket` / `cargo test -p backend-framework-core
#      --no-default-features`）がすべて成功する
#   C: 維持中の WebSocket 接続でメッセージ往復レイテンシ（p95）を計測記録し、
#      接続数増（1,000 / 5,000 / 10,000）による劣化度合いを定量化する。
#      `benches/bench-ws-load.sh` の `RESULT_JSON` が指定されていればティア別 p95・
#      劣化率の記録を検証する。未指定・バイナリ未ビルド時は判定不能として SKIP + 実行
#      手順を案内する（フェイルクローズ、自動ビルド・自動実行は行わない）
#   D: NFR-6（無関係パスへの RPS・レイテンシ影響が誤差範囲内）。ビルド済み計測用
#      バイナリ（`target/release/examples/minimal`・
#      `target/release/examples/ws_echo`）と `oha` が揃っていれば
#      `benches/ws-nfr6-bench.sh` で empirical 計測する。揃っていなければ判定不能
#      として SKIP + 実行手順を案内する
#
# 判定不能（前提ツール未導入等）はフェイルクローズで FAIL または SKIP とし、
# PASS と偽らない（.claude/rules/security.md）。`scripts/accept/graphql-accept.sh`
# （TASK-5.2 / #53）・`scripts/accept/webrtc-accept.sh`（TASK-8.4 / #29）と同型の
# オーケストレータ。
#
# 呼び出し元: 人間が `bash scripts/accept/websocket-accept.sh` として直接実行する。
# 判定ロジックのオフライン・セルフテストは
# `scripts/tests/run-websocket-accept-tests.sh` を参照（cargo 非依存）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
# shellcheck source=lib/nfr6-ratio.sh
source "${SCRIPT_DIR}/lib/nfr6-ratio.sh"
cd "${WORKSPACE_ROOT}"

echo "=== REQ-4 / TASK-4.4 受け入れ検証（WebSocket 受け入れテスト） ==="
echo "workspace root: ${WORKSPACE_ROOT}"
echo ""

# ---------------------------------------------------------------------------
# A: websocket feature 無効時の完全除外
# ---------------------------------------------------------------------------
check_dep_exclusion() {
    if [ ! -d "crates/plugin-websocket" ]; then
        record_skip "A: websocket 無効時の依存完全除外" "crates/plugin-websocket（TASK-4.1 #22）が本 worktree 未存在のため検証対象なし"
        return
    fi

    # `cargo tree` 自体が失敗した場合、stdout が空になり後続の `grep -c` が 0 を
    # 返してしまう。「tungstenite 系依存が真に 0 件」と「cargo tree 呼び出し自体が
    # 壊れている」を区別するため、終了コードを明示的に確認する
    # （`graphql-accept.sh`/`webrtc-accept.sh` の check_dep_exclusion と同一の
    # フェイルクローズパターン）。
    #
    # `-e normal --no-default-features` を明示する: release ビルドの依存グラフのみを
    # 対象とし、境界テスト専用の dev-dependency（あれば）を「残留」と誤検知しない
    # （`scripts/pay-for-what-you-use-check.sh` の (b) と同一のフラグ構成）。
    local tree_output disabled_count
    if ! tree_output="$(cargo tree -p backend-framework-core -e normal --no-default-features 2>/dev/null)"; then
        record_fail "A: websocket 無効時の依存完全除外" "cargo tree -p backend-framework-core -e normal --no-default-features 自体が失敗し測定不能（cargo 呼び出しが壊れている可能性）"
        return
    fi
    disabled_count="$(printf '%s\n' "${tree_output}" | grep -c -E 'tokio-tungstenite|tungstenite|bf-plugin-websocket' || true)"

    if [ "${disabled_count}" -eq 0 ]; then
        record_pass "A: websocket 無効時の依存完全除外" "cargo tree -p backend-framework-core -e normal --no-default-features | grep -c -E 'tokio-tungstenite|tungstenite|bf-plugin-websocket' = 0（release ビルドの依存グラフのみを対象）"
    else
        record_fail "A: websocket 無効時の依存完全除外" "websocket 系依存が ${disabled_count} 件残留（cargo tree -p backend-framework-core -e normal --no-default-features）"
    fi

    # 陽性対照: --features websocket では両者が出現すること（列挙腐敗・配線切れの検知）。
    local enabled_tree_output enabled_count
    if ! enabled_tree_output="$(cargo tree -p backend-framework-core -e normal --no-default-features --features websocket 2>/dev/null)"; then
        record_warn "A補足: websocket 有効時の依存インパクト（陽性対照）" "cargo tree -p backend-framework-core -e normal --no-default-features --features websocket 自体が失敗し測定不能"
        return
    fi
    enabled_count="$(printf '%s\n' "${enabled_tree_output}" | grep -c -E 'tokio-tungstenite|tungstenite|bf-plugin-websocket' || true)"
    if [ "${enabled_count}" -eq 0 ]; then
        record_fail "A補足: websocket 有効時の依存インパクト（陽性対照）" "cargo tree -p backend-framework-core -e normal --no-default-features --features websocket に tokio-tungstenite/tungstenite/bf-plugin-websocket が 0 件（配線切れ・列挙腐敗の疑い）"
    else
        record_warn "A補足: websocket 有効時の依存インパクト（陽性対照）" "cargo tree -p backend-framework-core -e normal --no-default-features --features websocket | grep -c -E 'tokio-tungstenite|tungstenite|bf-plugin-websocket' = ${enabled_count}（docs/dep-impact/records.md 参照）"
    fi
}

# ---------------------------------------------------------------------------
# A補足: pay-for-what-you-use-check.sh 併走
# ---------------------------------------------------------------------------
check_pay_for_what_you_use() {
    if [ ! -x "${WORKSPACE_ROOT}/scripts/pay-for-what-you-use-check.sh" ]; then
        record_skip "A補足: pay-for-what-you-use-check.sh" "scripts/pay-for-what-you-use-check.sh が見つかりません"
        return
    fi

    local out status
    set +e
    out="$(bash "${WORKSPACE_ROOT}/scripts/pay-for-what-you-use-check.sh" 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        record_pass "A補足: pay-for-what-you-use-check.sh" "全プラグイン feature（websocket 含む、動的列挙）の依存・unsafe・バイナリサイズ完全除外を確認"
    else
        record_fail "A補足: pay-for-what-you-use-check.sh" "非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi
}

# ---------------------------------------------------------------------------
# A': plugin-websocket 自コード unsafe 0 件
# ---------------------------------------------------------------------------
check_unsafe() {
    if [ ! -d "crates/plugin-websocket/src" ]; then
        record_skip "A': plugin-websocket 自コード unsafe 0件" "crates/plugin-websocket/src が未存在のため検証対象なし"
        return
    fi

    local hits
    hits="$(grep -rn --include='*.rs' -E '\bunsafe\b' crates/plugin-websocket/src | grep -v -E '^[^:]*:[0-9]+:[[:space:]]*//' || true)"
    if [ -z "${hits}" ]; then
        record_pass "A': plugin-websocket 自コード unsafe 0件" "crates/plugin-websocket/src に unsafe 0 件（テキストベース走査）"
    else
        record_fail "A': plugin-websocket 自コード unsafe 0件" "unsafe 使用箇所を検出: ${hits}"
    fi
}

# ---------------------------------------------------------------------------
# B: 回帰テスト
# ---------------------------------------------------------------------------
check_regression() {
    local out status

    set +e
    out="$(cargo test -p backend-framework-core --features websocket 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        record_pass "B: cargo test -p backend-framework-core --features websocket" "websocket_upgrade.rs・websocket_respawn.rs 等の境界テストを含め成功"
    else
        record_fail "B: cargo test -p backend-framework-core --features websocket" "非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi

    set +e
    out="$(cargo test -p backend-framework-core --no-default-features 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        record_pass "B補足: cargo test -p backend-framework-core --no-default-features" "websocket feature 無効時のフォールスルー（websocket_upgrade_disabled.rs）を含め成功"
    else
        record_fail "B補足: cargo test -p backend-framework-core --no-default-features" "非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi

    if [ ! -d "crates/plugin-websocket" ]; then
        record_skip "B: cargo test -p bf-plugin-websocket" "crates/plugin-websocket が未存在のため検証対象なし"
        return
    fi
    set +e
    out="$(cargo test -p bf-plugin-websocket 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        record_pass "B: cargo test -p bf-plugin-websocket" "RFC 6455 ハンドシェイク検証・フレーミング委譲の契約テストが成功"
    else
        record_fail "B: cargo test -p bf-plugin-websocket" "非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi
}

# ---------------------------------------------------------------------------
# C: レイテンシ計測（p95・劣化定量化）
# ---------------------------------------------------------------------------
# `benches/bench-ws-load.sh` の RESULT_JSON（`WEBSOCKET_ACCEPT_RESULT_JSON` env で
# 指定。本スクリプト自体は負荷試験を自動実行しない、サプライチェーン考慮・
# 長時間試行の二重実行防止のため）を検証する。
check_latency() {
    local result_json="${WEBSOCKET_ACCEPT_RESULT_JSON:-}"
    if [ -z "${result_json}" ]; then
        record_skip "C: レイテンシ計測（p95・劣化定量化）" "WEBSOCKET_ACCEPT_RESULT_JSON 未指定。'cargo build --release -p backend-framework-core --features websocket --example ws_echo' 等の前提ビルド後、'CONNECTION_TIERS=\"1000 5000 10000\" HOLD_SECS=30 RUNS=3 RESULT_JSON=/tmp/ws-bench-result.json bash benches/bench-ws-load.sh' を実行し、'WEBSOCKET_ACCEPT_RESULT_JSON=/tmp/ws-bench-result.json bash scripts/accept/websocket-accept.sh' として再実行すること"
        return
    fi
    if [ ! -r "${result_json}" ]; then
        record_fail "C: レイテンシ計測（p95・劣化定量化）" "WEBSOCKET_ACCEPT_RESULT_JSON=${result_json} を読み取れません"
        return
    fi
    if ! command -v jq >/dev/null 2>&1; then
        record_skip "C: レイテンシ計測（p95・劣化定量化）" "jq 未導入のため RESULT_JSON を検証できません"
        return
    fi

    local matrix_len fs_degradation ax_degradation
    if ! matrix_len="$(jq -r '.matrix | length' "${result_json}" 2>/dev/null)" || [ "${matrix_len}" = "null" ]; then
        record_fail "C: レイテンシ計測（p95・劣化定量化）" "${result_json} が期待した matrix フィールドを含みません（bench-ws-load.sh 出力ではない可能性）"
        return
    fi
    fs_degradation="$(jq -r '.heartbeat_rtt_p95_degradation.heartbeat_rtt_p95_degradation_pct.fullscratch' "${result_json}" 2>/dev/null || echo "null")"
    ax_degradation="$(jq -r '.heartbeat_rtt_p95_degradation.heartbeat_rtt_p95_degradation_pct.axum' "${result_json}" 2>/dev/null || echo "null")"

    if [ "${matrix_len}" -eq 0 ] || [ "${fs_degradation}" = "null" ]; then
        record_fail "C: レイテンシ計測（p95・劣化定量化）" "${result_json} の matrix が空、または劣化率を算出できていません"
        return
    fi

    local min_conn max_conn
    min_conn="$(jq -r '.heartbeat_rtt_p95_degradation.min_connections' "${result_json}")"
    max_conn="$(jq -r '.heartbeat_rtt_p95_degradation.max_connections' "${result_json}")"
    record_pass "C: レイテンシ計測（p95・劣化定量化）" "${result_json}（${min_conn}→${max_conn} 接続、matrix ${matrix_len} 件）: 心拍 RTT p95 劣化率 fullscratch=${fs_degradation}% axum=${ax_degradation}%（自動 PASS/FAIL 判定は行わない、定量化の記録が存在することのみ検証。詳細評価は benches/reports/task-4.4-ws-latency.md 参照）"
}

# ---------------------------------------------------------------------------
# D: NFR-6（無関係パスへの RPS・レイテンシ影響）
# ---------------------------------------------------------------------------
# 判定ロジック本体（evaluate_nfr6_ratio）は lib/nfr6-ratio.sh を再利用する
# （graphql-accept.sh 基準 C・webrtc-accept.sh 基準 E と同一の判定帯・オフライン
# セルフテスト資産を共有）。
check_nfr() {
    local baseline_bin="${WORKSPACE_ROOT}/target/release/examples/minimal"
    local ws_bin="${WORKSPACE_ROOT}/target/release/examples/ws_nfr6"

    if ! command -v oha >/dev/null 2>&1; then
        record_skip "D: NFR-6 無関係パス影響" "oha 未導入（導入: cargo install oha）。導入後 benches/ws-nfr6-bench.sh を実行して再判定すること"
        return
    fi
    if [ ! -x "${baseline_bin}" ] || [ ! -x "${ws_bin}" ]; then
        record_skip "D: NFR-6 無関係パス影響" "計測用バイナリ未ビルド。'cargo build --release -p backend-framework-core --example minimal --no-default-features' と '... --example ws_nfr6 --features websocket' を実行後、benches/ws-nfr6-bench.sh を実行して再判定すること"
        return
    fi

    local out rps_ratio_pct p95_ratio_pct
    out="$(bash "${WORKSPACE_ROOT}/benches/ws-nfr6-bench.sh" 2>/tmp/websocket-accept-nfr.log)" || {
        record_fail "D: NFR-6 無関係パス影響" "benches/ws-nfr6-bench.sh が失敗: $(tail -10 /tmp/websocket-accept-nfr.log | tr '\n' ' ')"
        return
    }
    rps_ratio_pct="$(echo "${out}" | grep '^rps_ratio_pct=' | cut -d= -f2)"
    p95_ratio_pct="$(echo "${out}" | grep '^p95_ratio_pct=' | cut -d= -f2)"

    local verdict
    verdict="$(evaluate_nfr6_ratio "${rps_ratio_pct}" "${p95_ratio_pct}")"
    local detail="RPS 比 ${rps_ratio_pct}% / p95 比 ${p95_ratio_pct}%（websocket 有効 / ベースライン、GET /health への負荷計測。狭義帯 100.3〜100.8% との照合は benches/reports/task-4.4-ws-latency.md 参照）"
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

check_dep_exclusion
check_pay_for_what_you_use
check_unsafe
check_regression
check_latency
check_nfr

print_summary "REQ-4、TASK-4.4 / #25"
exit "$(summary_exit_code)"
