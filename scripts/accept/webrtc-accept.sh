#!/usr/bin/env bash
# REQ-8（WebRTC）TASK-8.4（#29）の受け入れ検証オーケストレータ。
#
# `docs/spec/05-tasks.md` TASK-8.4「依存・バイナリ・unsafe を再評価し audit / deny を
# 確認する」の受け入れ基準を機械検証する:
#   A: `webrtc` feature 無効時、`backend-framework-core` の依存ツリーに webrtc 系依存が
#      一切現れない（pay-for-what-you-use の完全除外、
#      `.claude/rules/pay-for-what-you-use.md`）
#   B: `crates/plugin-webrtc` 自コードの unsafe が 0 件（`scripts/unsafe-triage.sh`
#      と同一の grep パイプライン）。依存側 `webrtc-rs` 由来の unsafe 増分は
#      `cargo-geiger` 導入済みなら実測（`--manifest-path` を絶対パスで指定すれば
#      workspace 仮想 manifest 配下でも実行可能）、未導入時は PoC-5 実測値を参考値
#      として引用する（捏造しない）
#   C: 全 feature 構成で `cargo audit` 既知脆弱性 0 件・`cargo deny check` 違反 0 件
#      （`scripts/dep-audit.sh` 連携）
#   D: `webrtc`・`webrtc-proxy` の 2 feature が `backend-framework-core` に存在し、
#      クレート境界で分離されたまま着脱可能（REQ-8 が要求する in-process / 別プロセス
#      切り出しの選択肢が両立していることの確認）
#   E: NFR-6（無関係パスへの RPS・レイテンシ影響が誤差範囲内、
#      `docs/spec/04-requirements.md`）。ビルド済み計測用バイナリ
#      （`target/release/examples/minimal`・`target/release/examples/webrtc_nfr6`）と
#      `oha` が揃っていれば `benches/webrtc-nfr6-bench.sh` で empirical 計測する。
#      揃っていなければ判定不能として SKIP + 実行手順を案内する（フェイルクローズ、
#      自動ビルド・自動ダウンロードは行わない）
#
# 判定不能（前提ツール未導入・前提クレート未マージ等）はフェイルクローズで
# FAIL または SKIP とし、PASS と偽らない（.claude/rules/security.md）。
#
# 呼び出し元: 人間が `bash scripts/accept/webrtc-accept.sh` として直接実行する。
# 判定ロジックのオフライン・セルフテストは
# `scripts/tests/run-webrtc-accept-tests.sh` を参照（cargo 非依存）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
# shellcheck source=lib/nfr6-ratio.sh
source "${SCRIPT_DIR}/lib/nfr6-ratio.sh"
cd "${WORKSPACE_ROOT}"

echo "=== REQ-8 / TASK-8.4 受け入れ検証（WebRTC 攻撃表面評価） ==="
echo "workspace root: ${WORKSPACE_ROOT}"
echo ""

# ---------------------------------------------------------------------------
# A: webrtc feature 無効時の完全除外
# ---------------------------------------------------------------------------
check_dep_exclusion() {
    if [ ! -d "crates/plugin-webrtc" ]; then
        record_skip "A: webrtc 無効時の依存完全除外" "crates/plugin-webrtc（TASK-8.1 #26）が本 worktree 未存在のため検証対象なし"
        return
    fi

    local disabled_count
    if ! disabled_count="$(cargo tree -p backend-framework-core 2>/dev/null | grep -c webrtc || true)"; then
        disabled_count=0
    fi

    if [ "${disabled_count}" -eq 0 ]; then
        record_pass "A: webrtc 無効時の依存完全除外" "cargo tree -p backend-framework-core | grep -c webrtc = 0"
    else
        record_fail "A: webrtc 無効時の依存完全除外" "webrtc 系依存が ${disabled_count} 件残留（cargo tree -p backend-framework-core）"
    fi

    # 有効時のインパクトも参考値として記録する（docs/dep-impact/records.md に転記済みの値と突合）。
    local enabled_count
    enabled_count="$(cargo tree -p backend-framework-core --features webrtc 2>/dev/null | grep -c webrtc || true)"
    record_warn "A補足: webrtc 有効時の依存インパクト" "cargo tree -p backend-framework-core --features webrtc | grep -c webrtc = ${enabled_count}（docs/dep-impact/records.md 参照）"
}

# ---------------------------------------------------------------------------
# B: 自コード unsafe 0 件
# ---------------------------------------------------------------------------
check_unsafe() {
    if [ ! -d "crates/plugin-webrtc/src" ]; then
        record_skip "B: plugin-webrtc 自コード unsafe 0件" "crates/plugin-webrtc/src が未存在のため検証対象なし"
        return
    fi

    local hits
    hits="$(grep -rn --include='*.rs' -E '\bunsafe\b' crates/plugin-webrtc/src | grep -v -E '^[^:]*:[0-9]+:[[:space:]]*//' || true)"
    if [ -z "${hits}" ]; then
        record_pass "B: plugin-webrtc 自コード unsafe 0件" "crates/plugin-webrtc/src に unsafe 0 件（テキストベース走査）"
    else
        record_fail "B: plugin-webrtc 自コード unsafe 0件" "unsafe 使用箇所を検出: ${hits}"
    fi

    # 依存側 unsafe 増分の参考計測。`cargo geiger -p <name>` は workspace 仮想 manifest
    # 配下で誤ったエラー（"virtual manifest" 扱い）を出すため、対象クレートの
    # Cargo.toml を絶対パスで直接指定する（相対パス指定は cargo-geiger 側の制約で
    # 拒否される）。旧記録（docs/dep-impact/records.md の TASK-8.1 エントリ）は
    # 誤った呼び出し方に基づき「本環境で実行失敗」としていたが、TASK-8.4 で
    # 正しい呼び出し方を確認し実測できた。
    if check_tool cargo-geiger "cargo install --locked cargo-geiger@0.13.0"; then
        local manifest_path="${WORKSPACE_ROOT}/crates/core/Cargo.toml"
        local baseline_line webrtc_line
        baseline_line="$(cargo geiger --output-format Ascii --manifest-path "${manifest_path}" --no-default-features 2>/dev/null | grep -E '^[0-9]+/[0-9]+' | tail -1 || true)"
        webrtc_line="$(cargo geiger --output-format Ascii --manifest-path "${manifest_path}" --features webrtc 2>/dev/null | grep -E '^[0-9]+/[0-9]+' | tail -1 || true)"
        if [ -n "${baseline_line}" ] && [ -n "${webrtc_line}" ]; then
            record_warn "B補足: 依存側 unsafe 増分（webrtc-rs、cargo geiger 実測）" "baseline（webrtc 無効）: ${baseline_line} / webrtc 有効: ${webrtc_line}（Functions 列 used/total。docs/dep-impact/records.md に詳細）"
        else
            record_skip "B補足: 依存側 unsafe 増分（webrtc-rs）" "cargo geiger の出力解析に失敗。PoC-5 実測（約 2.2 倍）を参考値として引用する"
        fi
    else
        record_skip "B補足: 依存側 unsafe 増分（webrtc-rs）" "cargo-geiger 未導入（導入: cargo install --locked cargo-geiger@0.13.0）。PoC-5 実測（約 2.2 倍）を参考値として引用する"
    fi
}

# ---------------------------------------------------------------------------
# C: audit / deny
# ---------------------------------------------------------------------------
check_audit_and_deny() {
    if [ ! -x "${WORKSPACE_ROOT}/scripts/dep-audit.sh" ]; then
        record_skip "C: cargo audit / deny 0件" "scripts/dep-audit.sh が見つかりません"
        return
    fi
    if ! check_tool cargo-audit "cargo install cargo-audit"; then
        record_skip "C: cargo audit / deny 0件" "cargo-audit 未導入（導入: cargo install cargo-audit）"
        return
    fi
    if ! check_tool cargo-deny "cargo install cargo-deny"; then
        record_skip "C: cargo audit / deny 0件" "cargo-deny 未導入（導入: cargo install cargo-deny）"
        return
    fi

    local out status
    set +e
    out="$(bash "${WORKSPACE_ROOT}/scripts/dep-audit.sh" 2>&1)"
    status=$?
    set -e
    if [ "${status}" -eq 0 ]; then
        record_pass "C: cargo audit / deny 0件" "scripts/dep-audit.sh（全 feature 構成）が正常終了"
    else
        record_fail "C: cargo audit / deny 0件" "scripts/dep-audit.sh が非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi
}

# ---------------------------------------------------------------------------
# D: 2 feature の存在・分離
# ---------------------------------------------------------------------------
check_features_present() {
    if ! check_tool jq "apt install jq / cargo install jq 等"; then
        record_skip "D: webrtc/webrtc-proxy 2 feature の存在" "jq 未導入のため cargo metadata の解析不能"
        return
    fi

    local metadata
    if ! metadata="$(cargo metadata --format-version 1 --no-deps 2>/tmp/webrtc-accept-metadata.log)"; then
        record_fail "D: webrtc/webrtc-proxy 2 feature の存在" "cargo metadata 失敗: $(tail -5 /tmp/webrtc-accept-metadata.log | tr '\n' ' ')"
        return
    fi

    local has_webrtc has_proxy
    has_webrtc="$(echo "${metadata}" | jq -r '.packages[] | select(.name == "backend-framework-core") | .features | has("webrtc")')"
    has_proxy="$(echo "${metadata}" | jq -r '.packages[] | select(.name == "backend-framework-core") | .features | has("webrtc-proxy")')"

    if [ "${has_webrtc}" = "true" ] && [ "${has_proxy}" = "true" ]; then
        record_pass "D: webrtc/webrtc-proxy 2 feature の存在" "backend-framework-core に webrtc（in-process）・webrtc-proxy（別プロセス切り出し）の両 feature が存在"
    else
        record_fail "D: webrtc/webrtc-proxy 2 feature の存在" "webrtc=${has_webrtc} webrtc-proxy=${has_proxy}"
    fi

    # クレート境界の分離: plugin-webrtc が plugin-webrtc-proxy に（逆も）依存していないこと。
    # Cargo.toml の description・コメント中には相互のクレート名への言及があるため
    # （設計上の対照説明）、`grep -l` によるファイル全体検索では誤検出する。
    # 実際の [dependencies] テーブルの依存宣言行（`bf-plugin-webrtc(-proxy)? = ...`
    # 形式）のみを対象にする。
    local cross_dep=""
    if grep -qE '^bf-plugin-webrtc-proxy[[:space:]]*=' crates/plugin-webrtc/Cargo.toml 2>/dev/null; then
        cross_dep="${cross_dep}crates/plugin-webrtc/Cargo.toml が bf-plugin-webrtc-proxy に依存
"
    fi
    if grep -qE '^bf-plugin-webrtc[[:space:]]*=' crates/plugin-webrtc-proxy/Cargo.toml 2>/dev/null; then
        cross_dep="${cross_dep}crates/plugin-webrtc-proxy/Cargo.toml が bf-plugin-webrtc に依存
"
    fi
    if [ -z "${cross_dep}" ]; then
        record_pass "D補足: in-process/proxy のクレート境界分離" "crates/plugin-webrtc と crates/plugin-webrtc-proxy は相互依存なし"
    else
        record_fail "D補足: in-process/proxy のクレート境界分離" "相互依存を検出: ${cross_dep}"
    fi
}

# ---------------------------------------------------------------------------
# E: NFR-6（無関係パスへの RPS・レイテンシ影響）
# ---------------------------------------------------------------------------
# 判定ロジック本体（evaluate_nfr6_ratio）は lib/nfr6-ratio.sh に切り出す
# （scripts/tests/run-webrtc-accept-tests.sh がオフラインで単体テストするため）。
check_nfr6() {
    local baseline_bin="${WORKSPACE_ROOT}/target/release/examples/minimal"
    local webrtc_bin="${WORKSPACE_ROOT}/target/release/examples/webrtc_nfr6"

    if ! command -v oha >/dev/null 2>&1; then
        record_skip "E: NFR-6 無関係パス影響" "oha 未導入（導入: cargo install oha）。導入後 benches/webrtc-nfr6-bench.sh を実行して再判定すること"
        return
    fi
    if [ ! -x "${baseline_bin}" ] || [ ! -x "${webrtc_bin}" ]; then
        record_skip "E: NFR-6 無関係パス影響" "計測用バイナリ未ビルド。'cargo build --release -p backend-framework-core --example minimal --no-default-features' と '... --example webrtc_nfr6 --features webrtc' を実行後、benches/webrtc-nfr6-bench.sh を実行して再判定すること"
        return
    fi

    local out rps_ratio_pct p95_ratio_pct
    out="$(bash "${WORKSPACE_ROOT}/benches/webrtc-nfr6-bench.sh" 2>/tmp/webrtc-accept-nfr6.log)" || {
        record_fail "E: NFR-6 無関係パス影響" "benches/webrtc-nfr6-bench.sh が失敗: $(tail -10 /tmp/webrtc-accept-nfr6.log | tr '\n' ' ')"
        return
    }
    rps_ratio_pct="$(echo "${out}" | grep '^rps_ratio_pct=' | cut -d= -f2)"
    p95_ratio_pct="$(echo "${out}" | grep '^p95_ratio_pct=' | cut -d= -f2)"

    local verdict
    verdict="$(evaluate_nfr6_ratio "${rps_ratio_pct}" "${p95_ratio_pct}")"
    local detail="RPS 比 ${rps_ratio_pct}% / p95 比 ${p95_ratio_pct}%（webrtc 有効 / ベースライン、GET / への負荷計測。狭義の NFR-6 帯 100.3〜100.8% との照合は benches/reports/task-8.4-webrtc-nfr6.md 参照）"
    case "${verdict}" in
    PASS)
        record_pass "E: NFR-6 無関係パス影響" "${detail}"
        ;;
    WARN)
        record_warn "E: NFR-6 無関係パス影響（実務許容帯内・狭義帯外）" "${detail}"
        ;;
    *)
        record_fail "E: NFR-6 無関係パス影響" "${detail}"
        ;;
    esac
}

check_dep_exclusion
check_unsafe
check_audit_and_deny
check_features_present
check_nfr6

print_summary "REQ-8、TASK-8.4 / #29"
exit "$(summary_exit_code)"
