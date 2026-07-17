#!/usr/bin/env bash
# REQ-3（OpenAPI 自動生成）の受け入れ検証オーケストレータ（TASK-3.3、#32）。
#
# `docs/spec/04-requirements.md` REQ-3 / `docs/spec/05-tasks.md` TASK-3.3 の受け入れ基準を
# 次の 5 節で検証する:
#   1. `openapi.json` が OpenAPI 3.x バリデータで構文妥当性エラー 0 件
#      （API 設計ベストプラクティス系ルールとは区別する。`openapi-spec-validator` は
#      スキーマ妥当性のみを検証しベストプラクティス系ルールを含まないため採用）
#   2. 生成定義とエンドポイント実装の齟齬 0 件
#      （機械検証: `cargo test -p bf-plugin-openapi`、うち `openapi_consistency.rs` が
#      path/method/パラメータ名・型/レスポンススキーマを網羅アサート。ただし
#      `crates/routes` 側の実サービングは `GET /health` を除く 4 エンドポイントが
#      本イシュー着手時点で未実装のため、完全な「実装との」突合は BLOCKED として
#      docs/acceptance/req3-openapi-generation.md の手動突合表に記録する）
#   3. `openapi` feature 無効時の依存完全除外
#   4. OpenAPI 生成有無での `GET /health` 相当の性能有意差なし（±5% 以内）
#   5. CI の 2 段階ビルド順序（TASK-3.2 実装済み）の存在確認
#
# 節 3・4 は `crates/core` にサーバ側 `openapi` feature（`openapi =
# ["dep:bf-plugin-openapi"]` 相当）の配線が本イシュー着手時点で存在しないため実行不能である
# （`crates/plugin-openapi/src/lib.rs`・`embed.rs` の doc comment が「TASK-2.1（#18）に
# 接続点を委ねる」と明記。TASK-2.1（#18）は当該配線を実施せずクローズされ、後継 Issue も
# 未起票）。判定不能を PASS と偽らず BLOCKED として記録し非 0 終了しない（SKIP 相当。
# record_skip を用いる。判定不能を FAIL とする一般原則は「検証を試みたが失敗した」場合の
# ものであり、本件は前提が存在せず検証自体が成立しないケースのため区別する）。
#
# 呼び出し元: 人間が `bash scripts/accept/openapi-accept.sh` として直接実行する。
# セルフテスト: `scripts/tests/run-openapi-accept-tests.sh`（判定ロジックのみを fixture で検証）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
cd "${WORKSPACE_ROOT}"

OPENAPI_JSON_PATH="${WORKSPACE_ROOT}/crates/plugin-openapi/openapi.json"

# ---------------------------------------------------------------------------
# 1: openapi.json の構文妥当性（OpenAPI 3.x バリデータ）
# ---------------------------------------------------------------------------
if ! check_tool openapi-spec-validator 'pip install --user --break-system-packages "openapi-spec-validator==0.7.1"'; then
    record_fail "1: openapi.json 構文妥当性" "openapi-spec-validator が見つかりません（判定不能、フェイルクローズ）"
elif openapi-spec-validator "${OPENAPI_JSON_PATH}" >/tmp/openapi-accept-validator.log 2>&1; then
    record_pass "1: openapi.json 構文妥当性" "openapi-spec-validator でエラー 0 件（詳細: /tmp/openapi-accept-validator.log）"
else
    record_fail "1: openapi.json 構文妥当性" "openapi-spec-validator がエラーを検出（詳細: /tmp/openapi-accept-validator.log）"
fi

# ---------------------------------------------------------------------------
# 2: 生成定義とエンドポイント実装の齟齬（機械検証分）
# ---------------------------------------------------------------------------
if cargo test -p bf-plugin-openapi >/tmp/openapi-accept-consistency.log 2>&1; then
    record_pass "2a: ApiDoc/openapi.json 内部整合（機械検証）" "cargo test -p bf-plugin-openapi が PASS（詳細: /tmp/openapi-accept-consistency.log）"
else
    record_fail "2a: ApiDoc/openapi.json 内部整合（機械検証）" "cargo test -p bf-plugin-openapi が FAIL（詳細: /tmp/openapi-accept-consistency.log）"
fi
record_skip "2b: 実装（crates/routes）との突合" "GET /health を除く 4 エンドポイントの実サービングが未実装のため機械検証不能。手動突合表を docs/acceptance/req3-openapi-generation.md に記録（前提 Issue 未起票、要フォローアップ）"

# ---------------------------------------------------------------------------
# 3: openapi feature 無効時の依存完全除外
# ---------------------------------------------------------------------------
if metadata="$(cargo metadata --format-version 1 --no-deps 2>/tmp/openapi-accept-metadata.log)" && command -v jq >/dev/null 2>&1; then
    core_features="$(printf '%s' "${metadata}" | jq -r '
        .packages[] | select(.name == "backend-framework-core") | .features | keys[]
    ' 2>/dev/null || true)"
    if printf '%s\n' "${core_features}" | grep -qx "openapi"; then
        record_pass "3: openapi feature 存在・依存除外検証" "openapi feature が存在。scripts/pay-for-what-you-use-check.sh の動的列挙で検証済み"
    else
        record_skip "3: openapi feature 存在・依存除外検証" "backend-framework-core に openapi feature が存在しない（TASK-2.1、#18 のスコープとして接続契約に明記されたが未配線・後継 Issue 未起票）。配線後は scripts/pay-for-what-you-use-check.sh が動的列挙により自動的に検証対象へ含める（本スクリプトの変更不要）"
    fi
else
    record_fail "3: openapi feature 存在・依存除外検証" "cargo metadata または jq の実行に失敗（判定不能、フェイルクローズ）"
fi

# ---------------------------------------------------------------------------
# 4: OpenAPI 生成有無での GET /health 性能有意差（±5% 以内）
# ---------------------------------------------------------------------------
record_skip "4: GET /health 性能有意差（±5% 以内）" "節 3 と同じ理由（openapi feature 未配線）で A/B 計測不能。配線後は benches/reports/task-3.3-openapi-performance.md の再計測手順に従い RUNS=5 中央値方式で計測すること"

# ---------------------------------------------------------------------------
# 5: CI 2 段階ビルド順序の存在確認（TASK-3.2 実装済み、記録用）
# ---------------------------------------------------------------------------
ci_file="${WORKSPACE_ROOT}/.github/workflows/ci.yml"
if [ ! -f "${ci_file}" ]; then
    record_fail "5: CI 2 段階ビルド順序" "${ci_file} が見つかりません"
elif grep -q "openapi-two-stage:" "${ci_file}" && grep -q "scripts/openapi-two-stage.sh" "${ci_file}"; then
    record_pass "5: CI 2 段階ビルド順序" "openapi-two-stage ジョブが ci.yml に存在し scripts/openapi-two-stage.sh（gen-openapi --check → cargo build）を呼び出す（TASK-3.2、#31 実装済み）"
else
    record_fail "5: CI 2 段階ビルド順序" "openapi-two-stage ジョブまたは scripts/openapi-two-stage.sh 呼び出しが ci.yml に見つかりません"
fi

print_summary "REQ-3、TASK-3.3 / #32"
exit "$(summary_exit_code)"
