#!/usr/bin/env bash
# REQ-5（GraphQL）TASK-5.2（#53）の受け入れ検証オーケストレータ。
#
# `docs/spec/05-tasks.md` TASK-5.2「GraphQL 受け入れテスト」の受け入れ基準を
# 機械検証する:
#   A: `graphql` feature 無効時、`backend-framework-core` の依存ツリーに
#      `async-graphql` / `bf-plugin-graphql` 系依存が一切現れない
#      （pay-for-what-you-use の完全除外、`.claude/rules/pay-for-what-you-use.md`）。
#      `scripts/pay-for-what-you-use-check.sh`（動的列挙のため graphql feature も
#      自動的に検証対象へ含まれる）も併走させ、依存・unsafe・バイナリサイズ除外を
#      二重に確認する
#   B: 最小疎通（クエリ実行と結果 JSON の返却）が成立する。
#      `cargo test -p backend-framework-core --features graphql`（境界テスト
#      `plugin_graphql_boundary.rs`）・`cargo test -p bf-plugin-graphql`（契約テスト）
#      に加え、ビルド済み `graphql_nfr6` バイナリがあれば curl で live 検証する
#   C: NFR（無関係パスへの RPS・p95 影響が誤差範囲内）。ビルド済み計測用バイナリ
#      （`target/release/examples/minimal`・`target/release/examples/graphql_nfr6`）と
#      `oha` が揃っていれば `benches/graphql-nfr6-bench.sh` で empirical 計測する。
#      揃っていなければ判定不能として SKIP + 実行手順を案内する（フェイルクローズ、
#      自動ビルド・自動ダウンロードは行わない）
#
# 判定不能（前提ツール未導入等）はフェイルクローズで FAIL または SKIP とし、
# PASS と偽らない（.claude/rules/security.md）。`scripts/accept/webrtc-accept.sh`
# （TASK-8.4 / #29）と同型のオーケストレータ。
#
# 呼び出し元: 人間が `bash scripts/accept/graphql-accept.sh` として直接実行する。
# 判定ロジックのオフライン・セルフテストは
# `scripts/tests/run-graphql-accept-tests.sh` を参照（cargo 非依存）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
# shellcheck source=lib/nfr6-ratio.sh
source "${SCRIPT_DIR}/lib/nfr6-ratio.sh"
cd "${WORKSPACE_ROOT}"

echo "=== REQ-5 / TASK-5.2 受け入れ検証（GraphQL 受け入れテスト） ==="
echo "workspace root: ${WORKSPACE_ROOT}"
echo ""

# ---------------------------------------------------------------------------
# A: graphql feature 無効時の完全除外
# ---------------------------------------------------------------------------
check_dep_exclusion() {
    if [ ! -d "crates/plugin-graphql" ]; then
        record_skip "A: graphql 無効時の依存完全除外" "crates/plugin-graphql（TASK-5.1 #38）が本 worktree 未存在のため検証対象なし"
        return
    fi

    # `cargo tree` 自体が失敗した場合、stdout が空になり後続の `grep -c` が 0 を
    # 返してしまう。「async-graphql 依存が真に 0 件」と「cargo tree 呼び出し自体が
    # 壊れている」を区別するため、終了コードを明示的に確認する
    # （`webrtc-accept.sh` の check_dep_exclusion と同一のフェイルクローズパターン）。
    #
    # `-e normal --no-default-features` を明示する: `crates/core/Cargo.toml` は
    # `graphql` feature の境界テスト（`plugin_graphql_boundary.rs`）専用に
    # `async-graphql`（dynamic-schema）を dev-dependency として持つ（release
    # ビルドには一切含まれない、`crates/core/Cargo.toml` のコメント参照）。
    # `cargo tree` は既定で dev-dependency のエッジも辿るため、これを指定しないと
    # release バイナリには含まれない dev 専用依存を「残留」と誤検知する
    # （`scripts/pay-for-what-you-use-check.sh` の (b) と同一のフラグ構成）。
    local tree_output disabled_count
    if ! tree_output="$(cargo tree -p backend-framework-core -e normal --no-default-features 2>/dev/null)"; then
        record_fail "A: graphql 無効時の依存完全除外" "cargo tree -p backend-framework-core -e normal --no-default-features 自体が失敗し測定不能（cargo 呼び出しが壊れている可能性）"
        return
    fi
    disabled_count="$(printf '%s\n' "${tree_output}" | grep -c -E 'async-graphql|bf-plugin-graphql' || true)"

    if [ "${disabled_count}" -eq 0 ]; then
        record_pass "A: graphql 無効時の依存完全除外" "cargo tree -p backend-framework-core -e normal --no-default-features | grep -c -E 'async-graphql|bf-plugin-graphql' = 0（release ビルドの依存グラフのみを対象、dev-dependency は除外）"
    else
        record_fail "A: graphql 無効時の依存完全除外" "graphql 系依存が ${disabled_count} 件残留（cargo tree -p backend-framework-core -e normal --no-default-features）"
    fi

    # 陽性対照: --features graphql では両者が出現すること（列挙腐敗・配線切れの検知）。
    local enabled_tree_output enabled_count
    if ! enabled_tree_output="$(cargo tree -p backend-framework-core -e normal --no-default-features --features graphql 2>/dev/null)"; then
        record_warn "A補足: graphql 有効時の依存インパクト（陽性対照）" "cargo tree -p backend-framework-core -e normal --no-default-features --features graphql 自体が失敗し測定不能"
        return
    fi
    enabled_count="$(printf '%s\n' "${enabled_tree_output}" | grep -c -E 'async-graphql|bf-plugin-graphql' || true)"
    if [ "${enabled_count}" -eq 0 ]; then
        record_fail "A補足: graphql 有効時の依存インパクト（陽性対照）" "cargo tree -p backend-framework-core -e normal --no-default-features --features graphql に async-graphql/bf-plugin-graphql が 0 件（配線切れ・列挙腐敗の疑い）"
    else
        record_warn "A補足: graphql 有効時の依存インパクト（陽性対照）" "cargo tree -p backend-framework-core -e normal --no-default-features --features graphql | grep -c -E 'async-graphql|bf-plugin-graphql' = ${enabled_count}（docs/dep-impact/records.md 参照）"
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
        record_pass "A補足: pay-for-what-you-use-check.sh" "全プラグイン feature（graphql 含む、動的列挙）の依存・unsafe・バイナリサイズ完全除外を確認"
    else
        record_fail "A補足: pay-for-what-you-use-check.sh" "非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi
}

# ---------------------------------------------------------------------------
# A': plugin-graphql 自コード unsafe 0 件
# ---------------------------------------------------------------------------
check_unsafe() {
    if [ ! -d "crates/plugin-graphql/src" ]; then
        record_skip "A': plugin-graphql 自コード unsafe 0件" "crates/plugin-graphql/src が未存在のため検証対象なし"
        return
    fi

    local hits
    hits="$(grep -rn --include='*.rs' -E '\bunsafe\b' crates/plugin-graphql/src | grep -v -E '^[^:]*:[0-9]+:[[:space:]]*//' || true)"
    if [ -z "${hits}" ]; then
        record_pass "A': plugin-graphql 自コード unsafe 0件" "crates/plugin-graphql/src に unsafe 0 件（テキストベース走査）"
    else
        record_fail "A': plugin-graphql 自コード unsafe 0件" "unsafe 使用箇所を検出: ${hits}"
    fi
}

# ---------------------------------------------------------------------------
# B: 最小疎通
# ---------------------------------------------------------------------------
check_min_connectivity() {
    local out status

    set +e
    out="$(cargo test -p backend-framework-core --features graphql 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        record_pass "B: cargo test -p backend-framework-core --features graphql" "plugin_graphql_boundary.rs（POST /graphql の実クエリ実行・200・application/json・hello:world）を含め成功"
    else
        record_fail "B: cargo test -p backend-framework-core --features graphql" "非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi

    set +e
    out="$(cargo test -p backend-framework-core --no-default-features 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        record_pass "B補足: cargo test -p backend-framework-core --no-default-features" "graphql feature 無効時のフォールスルー（plugin_graphql_boundary_disabled.rs）を含め成功"
    else
        record_fail "B補足: cargo test -p backend-framework-core --no-default-features" "非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi

    set +e
    out="$(cargo test -p bf-plugin-graphql 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        record_pass "B: cargo test -p bf-plugin-graphql" "try_handle_graphql の契約テスト（クエリ実行・エラー処理・不正 JSON 拒否等）が成功"
    else
        record_fail "B: cargo test -p bf-plugin-graphql" "非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi

    local bin="${WORKSPACE_ROOT}/target/release/examples/graphql_nfr6"
    if [ ! -x "${bin}" ]; then
        record_skip "B補足: graphql_nfr6 live 疎通確認" "計測用バイナリ未ビルド。'cargo build --release -p backend-framework-core --example graphql_nfr6 --features graphql' を実行後、graphql-accept.sh を再実行すること"
        return
    fi
    if ! command -v curl >/dev/null 2>&1; then
        record_skip "B補足: graphql_nfr6 live 疎通確認" "curl 未導入"
        return
    fi

    local port=3003
    "${bin}" >/dev/null 2>&1 &
    local pid=$!
    # サーバ起動待ち（webrtc-accept.sh 系の wait_ready と同等の簡易ポーリング）。
    local elapsed_ms=0
    local ready=0
    while [ "${elapsed_ms}" -lt 5000 ]; do
        if curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${port}/" 2>/dev/null | grep -q '^200$'; then
            ready=1
            break
        fi
        sleep 0.05
        elapsed_ms=$((elapsed_ms + 50))
    done

    if [ "${ready}" -ne 1 ]; then
        kill "${pid}" 2>/dev/null || true
        wait "${pid}" 2>/dev/null || true
        record_fail "B補足: graphql_nfr6 live 疎通確認" "${bin} が 5000ms 以内に応答しませんでした"
        return
    fi

    local response
    response="$(curl -s -X POST "http://127.0.0.1:${port}/graphql" -H 'Content-Type: application/json' -d '{"query":"{ hello }"}' 2>/dev/null || true)"
    kill "${pid}" 2>/dev/null || true
    wait "${pid}" 2>/dev/null || true

    if echo "${response}" | grep -q '"hello":"world"'; then
        record_pass "B補足: graphql_nfr6 live 疎通確認" "POST /graphql に { hello } を送信し data.hello == world を確認: ${response}"
    else
        record_fail "B補足: graphql_nfr6 live 疎通確認" "期待した hello:world を含まない応答: ${response}"
    fi
}

# ---------------------------------------------------------------------------
# C: NFR（無関係パスへの RPS・レイテンシ影響）
# ---------------------------------------------------------------------------
# 判定ロジック本体（evaluate_nfr6_ratio）は lib/nfr6-ratio.sh を再利用する
# （webrtc-accept.sh 基準 E と同一の判定帯・オフラインセルフテスト資産を共有）。
check_nfr() {
    local baseline_bin="${WORKSPACE_ROOT}/target/release/examples/minimal"
    local graphql_bin="${WORKSPACE_ROOT}/target/release/examples/graphql_nfr6"

    if ! command -v oha >/dev/null 2>&1; then
        record_skip "C: NFR 無関係パス影響" "oha 未導入（導入: cargo install oha）。導入後 benches/graphql-nfr6-bench.sh を実行して再判定すること"
        return
    fi
    if [ ! -x "${baseline_bin}" ] || [ ! -x "${graphql_bin}" ]; then
        record_skip "C: NFR 無関係パス影響" "計測用バイナリ未ビルド。'cargo build --release -p backend-framework-core --example minimal --no-default-features' と '... --example graphql_nfr6 --features graphql' を実行後、benches/graphql-nfr6-bench.sh を実行して再判定すること"
        return
    fi

    local out rps_ratio_pct p95_ratio_pct
    out="$(bash "${WORKSPACE_ROOT}/benches/graphql-nfr6-bench.sh" 2>/tmp/graphql-accept-nfr.log)" || {
        record_fail "C: NFR 無関係パス影響" "benches/graphql-nfr6-bench.sh が失敗: $(tail -10 /tmp/graphql-accept-nfr.log | tr '\n' ' ')"
        return
    }
    rps_ratio_pct="$(echo "${out}" | grep '^rps_ratio_pct=' | cut -d= -f2)"
    p95_ratio_pct="$(echo "${out}" | grep '^p95_ratio_pct=' | cut -d= -f2)"

    local verdict
    verdict="$(evaluate_nfr6_ratio "${rps_ratio_pct}" "${p95_ratio_pct}")"
    local detail="RPS 比 ${rps_ratio_pct}% / p95 比 ${p95_ratio_pct}%（graphql 有効 / ベースライン、GET / への負荷計測。狭義帯 100.3〜100.8% との照合は benches/reports/task-5.2-graphql-performance.md 参照）"
    case "${verdict}" in
    PASS)
        record_pass "C: NFR 無関係パス影響" "${detail}"
        ;;
    WARN)
        record_warn "C: NFR 無関係パス影響（実務許容帯内・狭義帯外）" "${detail}"
        ;;
    *)
        record_fail "C: NFR 無関係パス影響" "${detail}"
        ;;
    esac
}

check_dep_exclusion
check_pay_for_what_you_use
check_unsafe
check_min_connectivity
check_nfr

print_summary "REQ-5、TASK-5.2 / #53"
exit "$(summary_exit_code)"
