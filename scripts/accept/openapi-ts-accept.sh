#!/usr/bin/env bash
# REQ-6（openapi-typescript 連携）TASK-6.2（#55）の受け入れ検証オーケストレータ。
#
# `docs/spec/05-tasks.md` TASK-6.2「陰性対照 CI 型検査整備・受け入れテスト」の
# 受け入れ基準を機械検証する:
#   A: 陽性対照。最低 1 つのエンドポイント呼び出し（`ts/src/usage.ts` の 5
#      エンドポイント一巡）が `tsc --noEmit` を通ること。`scripts/openapi-ts.sh`
#      （TASK-6.1、#54）が成功することで検証する。
#   B: 陰性対照。意図的な型不一致が `tsc --noEmit` のエラーとして確実に検出
#      されること。`scripts/openapi-ts-negative.sh`（N1: TS 側陰性対照、N2:
#      openapi.json 境界からの型不一致伝搬）が成功することで検証する。
#   C: Rust 定義変更の伝搬。`crates/plugin-openapi/src/docs.rs` の
#      `/users/{id}` の `id` を `u64`→`String` へ一時的に変更 →
#      `gen-openapi --update` → `npm run gen:types` →
#      `ts/src/generated/schema.d.ts` に差分が現れること・既存 `usage.ts` の
#      型検査が失敗することを確認し、`trap` で必ず元に戻す。対象パスに
#      未コミット変更がある場合は SKIP（勝手に破棄しない、`.claude/rules/
#      security.md` 作業ツリー整合性）。
#
# 判定不能（前提ツール未導入・対象パス未クリーン等）はフェイルクローズで
# FAIL または SKIP とし、PASS と偽らない（.claude/rules/security.md）。
# `scripts/accept/graphql-accept.sh`（TASK-5.2、#53）と同型のオーケストレータ。
#
# 呼び出し元: 人間が `bash scripts/accept/openapi-ts-accept.sh` として直接実行する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
cd "${WORKSPACE_ROOT}"

echo "=== REQ-6 / TASK-6.2 受け入れ検証（陰性対照 CI 型検査整備・受け入れテスト） ==="
echo "workspace root: ${WORKSPACE_ROOT}"
echo ""

# ---------------------------------------------------------------------------
# A: 陽性対照
# ---------------------------------------------------------------------------
check_positive_control() {
    if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
        record_skip "A: 陽性対照（tsc --noEmit 通過）" "node/npm 未導入（ts/package.json の volta フィールド参照）"
        return
    fi

    local out status
    if out="$(bash "${WORKSPACE_ROOT}/scripts/openapi-ts.sh" 2>&1)"; then
        status=0
    else
        status=$?
    fi

    if [ "${status}" -eq 0 ]; then
        record_pass "A: 陽性対照（tsc --noEmit 通過）" "scripts/openapi-ts.sh が成功（5 エンドポイント呼び出しの型検査を通過）"
    else
        record_fail "A: 陽性対照（tsc --noEmit 通過）" "scripts/openapi-ts.sh が非 0 終了: $(echo "${out}" | tail -10 | tr '\n' ' ')"
    fi
}

# ---------------------------------------------------------------------------
# B: 陰性対照
# ---------------------------------------------------------------------------
check_negative_control() {
    if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
        record_skip "B: 陰性対照（意図的な型不一致の検出）" "node/npm 未導入（ts/package.json の volta フィールド参照）"
        return
    fi

    local out status
    if out="$(bash "${WORKSPACE_ROOT}/scripts/openapi-ts-negative.sh" 2>&1)"; then
        status=0
    else
        status=$?
    fi

    if [ "${status}" -eq 0 ]; then
        record_pass "B: 陰性対照（意図的な型不一致の検出）" "scripts/openapi-ts-negative.sh が成功（N1: 4 類型 / N2: openapi.json 境界伝搬 とも意図した型不一致を検出、陽性対照も同時に成功）"
    else
        record_fail "B: 陰性対照（意図的な型不一致の検出）" "scripts/openapi-ts-negative.sh が非 0 終了: $(echo "${out}" | tail -20 | tr '\n' ' ')"
    fi
}

# ---------------------------------------------------------------------------
# C: Rust 定義変更の伝搬（一時変更 + trap 復元）
# ---------------------------------------------------------------------------
check_rust_definition_propagation() {
    local criterion="C: Rust 定義変更の伝搬（docs.rs → openapi.json → schema.d.ts → tsc）"

    if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
        record_skip "${criterion}" "node/npm 未導入"
        return
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        record_skip "${criterion}" "cargo 未導入"
        return
    fi

    local docs_rs="${WORKSPACE_ROOT}/crates/plugin-openapi/src/docs.rs"
    local openapi_json="${WORKSPACE_ROOT}/crates/plugin-openapi/openapi.json"
    local schema_dts="${WORKSPACE_ROOT}/ts/src/generated/schema.d.ts"

    # 対象パスに未コミット変更がある場合は SKIP し、勝手に破棄しない
    # （.claude/rules/security.md 作業ツリー整合性、ユーザーの作業内容保護）。
    if ! git -C "${WORKSPACE_ROOT}" diff --quiet -- "${docs_rs}" "${openapi_json}" "${schema_dts}" 2>/dev/null \
        || ! git -C "${WORKSPACE_ROOT}" diff --cached --quiet -- "${docs_rs}" "${openapi_json}" "${schema_dts}" 2>/dev/null; then
        record_skip "${criterion}" "対象パス（docs.rs / openapi.json / schema.d.ts）に未コミット変更があるため SKIP（意図しない破棄を避ける）"
        return
    fi

    local restore_needed=0
    restore_files() {
        if [ "${restore_needed}" -eq 1 ]; then
            git -C "${WORKSPACE_ROOT}" checkout -- "${docs_rs}" "${openapi_json}" "${schema_dts}" 2>/dev/null || true
        fi
    }
    trap restore_files RETURN

    # docs.rs の /users/{id} の id 型を一時的に u64 → String へ変更する
    # （固定パターンの sed 置換のみで完結させ、外部入力を受けない）。
    if ! grep -qF '("id" = u64, Path, description = "ユーザー ID（非負整数）")' "${docs_rs}"; then
        record_skip "${criterion}" "docs.rs の想定パターンが見つからず変更対象を特定できない（docs.rs 構造変更の可能性）"
        return
    fi
    restore_needed=1
    sed -i.bak 's/("id" = u64, Path, description = "ユーザー ID（非負整数）")/("id" = String, Path, description = "TASK-6.2 受け入れテスト C 用の一時的な型変更")/' "${docs_rs}"
    rm -f "${docs_rs}.bak"

    local out status
    if out="$(cd "${WORKSPACE_ROOT}" && cargo run -p bf-plugin-openapi --features gen-cli --bin gen-openapi -- --update 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "${status}" -ne 0 ]; then
        record_fail "${criterion}" "gen-openapi --update が失敗: $(echo "${out}" | tail -10 | tr '\n' ' ')"
        return
    fi

    if out="$(cd "${WORKSPACE_ROOT}/ts" && npm run --silent gen:types 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "${status}" -ne 0 ]; then
        record_fail "${criterion}" "npm run gen:types が失敗: $(echo "${out}" | tail -10 | tr '\n' ' ')"
        return
    fi

    if git -C "${WORKSPACE_ROOT}" diff --quiet -- "${schema_dts}" 2>/dev/null; then
        record_fail "${criterion}" "docs.rs の型変更後も schema.d.ts に差分が現れなかった（型再生成のみでの伝搬が機能していない）"
        return
    fi

    local tsc_out tsc_status
    if tsc_out="$(cd "${WORKSPACE_ROOT}/ts" && npm run --silent typecheck 2>&1)"; then
        tsc_status=0
    else
        tsc_status=$?
    fi

    if [ "${tsc_status}" -eq 0 ]; then
        record_fail "${criterion}" "id を String へ変更後も tsc --noEmit が成功した（usage.ts が id: 42 を渡している前提が崩れた可能性、型検査が実効性を失っている可能性）"
        return
    fi
    if ! printf '%s\n' "${tsc_out}" | grep -q "TS2322"; then
        record_fail "${criterion}" "tsc --noEmit は失敗したが期待した TS2322 が出力に見つからない: $(echo "${tsc_out}" | tail -10 | tr '\n' ' ')"
        return
    fi

    record_pass "${criterion}" "docs.rs の id: u64→String 変更が gen-openapi --update・npm run gen:types のみで schema.d.ts に伝搬し、usage.ts の型検査が TS2322 で失敗することを確認（変更は trap で復元済み）"
}

check_positive_control
check_negative_control
check_rust_definition_propagation

print_summary "REQ-6、TASK-6.2 / #55"
exit "$(summary_exit_code)"
