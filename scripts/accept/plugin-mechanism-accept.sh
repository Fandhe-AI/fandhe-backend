#!/usr/bin/env bash
# REQ-2（プラグイン機構）の受け入れ検証オーケストレータ（TASK-2.4、#21）。
#
# `docs/spec/04-requirements.md` REQ-2 の受け入れ基準のうち、TASK-2.4 が担う 3 点を検証する:
#   1. 少なくとも 2 種のプラグインを feature flag で着脱できる
#      （`webrtc-proxy`・`graphql` の 2 feature が `cargo metadata` に存在することを確認）
#   2. `scripts/pay-for-what-you-use-check.sh` を呼び出し、feature 無効時の依存・
#      unsafe・バイナリサイズ完全除外を検証する（動的列挙のため graphql 追加時も
#      本スクリプトの変更は不要）
#   3. 各 feature 構成（無効・graphql 単独・webrtc-proxy 単独・全 feature）で
#      `cargo build` / `cargo test` が成功する
#   4. コンパイル時 vs 実行時動的ロードのトレードオフ設計文書
#      （`docs/design/plugin-loading-tradeoffs.md`）が存在する
#
# 両 feature 無効時のコア性能（REQ-1 基準維持）は計測用バイナリ（axum-ref 等価
# 4 エンドポイント）が別イシュー（#15、#71、TASK-1.6-1「BLOCKED」を参照）で
# 未整備のため、本スクリプトの自動検証対象に含めない。手動実行手順を
# `docs/acceptance/req2-plugin-mechanism.md` に記録する（判定不能を PASS と
# 偽らない、.claude/rules/security.md のフェイルクローズ原則）。
#
# 判定不能（cargo metadata 失敗・jq 未導入・前提スクリプト不在等）はフェイルクローズで
# FAIL とする。
#
# 呼び出し元: 人間が `bash scripts/accept/plugin-mechanism-accept.sh` として直接実行する。
# CI 常設ジョブへの組み込みは既存の `pay-for-what-you-use` ジョブ（TASK-2.2）が
# 本質的に同じ検証を担うため、本スクリプトは受け入れ記録用の手動実行を主目的とする。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
cd "${WORKSPACE_ROOT}"

# ---------------------------------------------------------------------------
# 1: 少なくとも 2 種のプラグイン feature の存在確認
# ---------------------------------------------------------------------------
if ! command -v jq >/dev/null 2>&1; then
    record_fail "1: 2 種プラグイン feature 存在確認" "jq が見つかりません（判定不能、フェイルクローズ）"
elif ! metadata="$(cargo metadata --format-version 1 --no-deps 2>/tmp/plugin-mechanism-accept-metadata.log)"; then
    record_fail "1: 2 種プラグイン feature 存在確認" "cargo metadata に失敗しました（/tmp/plugin-mechanism-accept-metadata.log 参照）"
else
    core_features="$(printf '%s' "${metadata}" | jq -r '
        .packages[] | select(.name == "backend-framework-core") | .features | keys[]
    ' 2>/dev/null || true)"
    missing=()
    for f in webrtc-proxy graphql; do
        if ! printf '%s\n' "${core_features}" | grep -qx "${f}"; then
            missing+=("${f}")
        fi
    done
    if [ "${#missing[@]}" -eq 0 ]; then
        record_pass "1: 2 種プラグイン feature 存在確認" "webrtc-proxy・graphql の両 feature が backend-framework-core に存在（着脱可能な 2 プラグインの前提を充足）"
    else
        record_fail "1: 2 種プラグイン feature 存在確認" "未検出の feature: ${missing[*]}"
    fi
fi

# ---------------------------------------------------------------------------
# 2: pay-for-what-you-use 機械検証（TASK-2.2、#19）の呼び出し
#    graphql feature は dep:bf-plugin-graphql 命名規約に従うため、
#    scripts/pay-for-what-you-use-check.sh の動的列挙により追加対応なしで
#    検証対象に含まれる（同スクリプトの doc を参照）。
# ---------------------------------------------------------------------------
if [ ! -x "${WORKSPACE_ROOT}/scripts/pay-for-what-you-use-check.sh" ] && [ ! -f "${WORKSPACE_ROOT}/scripts/pay-for-what-you-use-check.sh" ]; then
    record_fail "2: pay-for-what-you-use 機械検証" "scripts/pay-for-what-you-use-check.sh が見つかりません"
elif bash "${WORKSPACE_ROOT}/scripts/pay-for-what-you-use-check.sh" >/tmp/plugin-mechanism-accept-pfwyu.log 2>&1; then
    record_pass "2: pay-for-what-you-use 機械検証" "scripts/pay-for-what-you-use-check.sh が PASS（詳細: /tmp/plugin-mechanism-accept-pfwyu.log）"
else
    record_fail "2: pay-for-what-you-use 機械検証" "scripts/pay-for-what-you-use-check.sh が FAIL（詳細: /tmp/plugin-mechanism-accept-pfwyu.log）"
fi

# ---------------------------------------------------------------------------
# 3: 各 feature 構成のビルド・テスト
# ---------------------------------------------------------------------------
build_and_test() {
    local label="$1"
    shift
    local feature_args=("$@")
    if cargo build -p backend-framework-core "${feature_args[@]}" >/tmp/plugin-mechanism-accept-build-"${label}".log 2>&1 \
        && cargo test -p backend-framework-core "${feature_args[@]}" >/tmp/plugin-mechanism-accept-test-"${label}".log 2>&1; then
        record_pass "3: build/test（${label}）" "cargo build/test 成功"
    else
        record_fail "3: build/test（${label}）" "cargo build/test に失敗（詳細: /tmp/plugin-mechanism-accept-{build,test}-${label}.log）"
    fi
}

build_and_test "no-default-features" --no-default-features
build_and_test "graphql" --features graphql
build_and_test "webrtc-proxy" --features webrtc-proxy
build_and_test "all-features" --all-features

# ---------------------------------------------------------------------------
# 4: 設計文書の存在確認
# ---------------------------------------------------------------------------
tradeoff_doc="${WORKSPACE_ROOT}/docs/design/plugin-loading-tradeoffs.md"
if [ -f "${tradeoff_doc}" ] && grep -q "実行時動的ロード" "${tradeoff_doc}"; then
    record_pass "4: 安全性トレードオフ設計文書" "docs/design/plugin-loading-tradeoffs.md が存在し、実行時動的ロードとの比較記述を含む"
else
    record_fail "4: 安全性トレードオフ設計文書" "docs/design/plugin-loading-tradeoffs.md が見つからない、または比較記述を確認できません"
fi

# ---------------------------------------------------------------------------
# 5: 性能維持（手動、#15/#71 BLOCKED のため自動検証対象外）
# ---------------------------------------------------------------------------
record_skip "5: 両 feature 無効時の性能維持（REQ-1 基準）" "axum-ref 等価計測用バイナリが #15/#71（TASK-1.6-1）BLOCKED のため自動検証対象外。手動手順は docs/acceptance/req2-plugin-mechanism.md を参照"

print_summary "REQ-2、TASK-2.4 / #21"
exit "$(summary_exit_code)"
