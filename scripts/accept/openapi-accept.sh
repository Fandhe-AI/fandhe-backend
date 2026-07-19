#!/usr/bin/env bash
# REQ-3（OpenAPI 自動生成）の受け入れ検証オーケストレータ（TASK-3.3、#32）。
#
# `docs/spec/04-requirements.md` REQ-3 / `docs/spec/05-tasks.md` TASK-3.3 の受け入れ基準を
# 次の 5 節で検証する:
#   1. `openapi.json` が OpenAPI 3.x バリデータで構文妥当性エラー 0 件
#      （API 設計ベストプラクティス系ルールとは区別する。`openapi-spec-validator` は
#      スキーマ妥当性のみを検証しベストプラクティス系ルールを含まないため採用）
#   2. 生成定義とエンドポイント実装の齟齬 0 件
#      （機械検証 2a: `cargo test -p fandhe-backend-plugin-openapi`、うち `openapi_consistency.rs` が
#      path/method/パラメータ名・型/レスポンススキーマを網羅アサート。
#      機械検証 2b: `crates/core/examples/openapi_endpoints.rs`（#257 で 5 エンドポイントを
#      実サービング）のテストで実装側の method/パラメータ/応答/Content-Type を検証。
#      手動突合表は docs/acceptance/req3-openapi-generation.md に記録）
#   3. `openapi` feature 無効時の依存完全除外
#   4. OpenAPI 生成有無での `GET /health` 相当の性能有意差なし（±5% 以内）
#   5. CI の 2 段階ビルド順序（TASK-3.2 実装済み）の存在確認
#
# 節 3・4 の前提だった `crates/core` のサーバ側 `openapi` feature
# （`openapi = ["dep:fandhe-backend-plugin-openapi"]`）は #256 で配線済み。節 3 は
# scripts/pay-for-what-you-use-check.sh で機械検証する。節 4 の A/B 性能計測は
# 実行時間・専有計測枠（benches/lib/exclusive.sh）を要するため本スクリプトでは
# 再実行せず、benches/reports/task-3.3-openapi-performance.md の確定判定を参照する
# （判定行が見つからない場合はフェイルクローズで FAIL）。
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
# `pip install --user` は CLI 実行ファイルを PATH に配置しない環境がある一方、
# モジュールとしては `python3 -m openapi_spec_validator` で確実に呼び出せる
# （.github/workflows/ci.yml の openapi.json 構文妥当性検証ステップと同一の
# 呼び出し方式に揃える）。CLI 実行ファイルが PATH にある環境ではそちらを優先し、
# 無ければ python3 -m 実行形式にフォールバックする。両方見つからない場合のみ
# 判定不能としてフェイルクローズする。
if check_tool openapi-spec-validator 'pip install --user --break-system-packages "openapi-spec-validator==0.7.1"'; then
    VALIDATOR_CMD=(openapi-spec-validator)
elif command -v python3 >/dev/null 2>&1 && python3 -c 'import openapi_spec_validator' >/dev/null 2>&1; then
    # `python3 -m openapi_spec_validator --version` はこのモジュールの CLI が
    # `--version` フラグ自体を持たず常にエラー終了するため疎通確認に使えない
    # （ローカル検証で確認済み）。モジュールの import 可否で存在確認する。
    VALIDATOR_CMD=(python3 -m openapi_spec_validator)
else
    VALIDATOR_CMD=()
fi

if [ "${#VALIDATOR_CMD[@]}" -eq 0 ]; then
    record_fail "1: openapi.json 構文妥当性" "openapi-spec-validator（CLI・python3 -m openapi_spec_validator のいずれも）が見つかりません（判定不能、フェイルクローズ）"
elif "${VALIDATOR_CMD[@]}" "${OPENAPI_JSON_PATH}" >/tmp/openapi-accept-validator.log 2>&1; then
    record_pass "1: openapi.json 構文妥当性" "${VALIDATOR_CMD[*]} でエラー 0 件（詳細: /tmp/openapi-accept-validator.log）"
else
    record_fail "1: openapi.json 構文妥当性" "${VALIDATOR_CMD[*]} がエラーを検出（詳細: /tmp/openapi-accept-validator.log）"
fi

# ---------------------------------------------------------------------------
# 2: 生成定義とエンドポイント実装の齟齬（機械検証分）
# ---------------------------------------------------------------------------
if cargo test -p fandhe-backend-plugin-openapi >/tmp/openapi-accept-consistency.log 2>&1; then
    record_pass "2a: ApiDoc/openapi.json 内部整合（機械検証）" "cargo test -p fandhe-backend-plugin-openapi が PASS（詳細: /tmp/openapi-accept-consistency.log）"
else
    record_fail "2a: ApiDoc/openapi.json 内部整合（機械検証）" "cargo test -p fandhe-backend-plugin-openapi が FAIL（詳細: /tmp/openapi-accept-consistency.log）"
fi
# 2b: 実装側（crates/core/examples/openapi_endpoints.rs、#257）のテストで
# 5 エンドポイントの method・パラメータ・応答・Content-Type を機械検証する。
# 宣言と実装の対応関係そのものの手動突合表は docs/acceptance/
# req3-openapi-generation.md に記録済み（#259 で PASS へ更新）。
if cargo test -p fandhe-backend-core --example openapi_endpoints >/tmp/openapi-accept-endpoints.log 2>&1; then
    record_pass "2b: 実装（openapi_endpoints example）との突合" "cargo test -p fandhe-backend-core --example openapi_endpoints が PASS（5 エンドポイントの実サービング検証、手動突合表: docs/acceptance/req3-openapi-generation.md）"
else
    record_fail "2b: 実装（openapi_endpoints example）との突合" "cargo test -p fandhe-backend-core --example openapi_endpoints が FAIL（詳細: /tmp/openapi-accept-endpoints.log）"
fi

# ---------------------------------------------------------------------------
# 3: openapi feature 無効時の依存完全除外
# ---------------------------------------------------------------------------
if metadata="$(cargo metadata --format-version 1 --no-deps 2>/tmp/openapi-accept-metadata.log)" && command -v jq >/dev/null 2>&1; then
    core_features="$(printf '%s' "${metadata}" | jq -r '
        .packages[] | select(.name == "fandhe-backend-core") | .features | keys[]
    ' 2>/dev/null || true)"
    if printf '%s\n' "${core_features}" | grep -qx "openapi"; then
        # openapi feature が存在する場合のみ、その存在を根拠に PASS を詐称せず
        # 実際に scripts/pay-for-what-you-use-check.sh（動的列挙により openapi
        # feature も自動的に検証対象へ含まれる）を実行して依存除外を検証する
        # （Bugbot 指摘、PR #141: cargo metadata/jq のみでは feature の存在確認に
        # とどまり依存除外の検証にならない）。
        if bash "${WORKSPACE_ROOT}/scripts/pay-for-what-you-use-check.sh" >/tmp/openapi-accept-pfwu.log 2>&1; then
            record_pass "3: openapi feature 存在・依存除外検証" "openapi feature が存在し、scripts/pay-for-what-you-use-check.sh の実行で依存除外を検証済み（詳細: /tmp/openapi-accept-pfwu.log）"
        else
            record_fail "3: openapi feature 存在・依存除外検証" "openapi feature は存在するが scripts/pay-for-what-you-use-check.sh が FAIL（詳細: /tmp/openapi-accept-pfwu.log）"
        fi
    else
        record_skip "3: openapi feature 存在・依存除外検証" "fandhe-backend-core に openapi feature が存在しない（TASK-2.1、#18 のスコープとして接続契約に明記されたが未配線・後継 Issue 未起票）。配線後は scripts/pay-for-what-you-use-check.sh が動的列挙により自動的に検証対象へ含める（本スクリプトの変更不要）"
    fi
else
    record_fail "3: openapi feature 存在・依存除外検証" "cargo metadata または jq の実行に失敗（判定不能、フェイルクローズ）"
fi

# ---------------------------------------------------------------------------
# 4: OpenAPI 生成有無での GET /health 性能有意差（±5% 以内）
# ---------------------------------------------------------------------------
# A/B 性能計測は数分の専有実行枠（benches/lib/exclusive.sh の flock・静穏確認）を
# 要するため本スクリプト内では再実行せず、確定済みレポートの判定行を参照する。
# PASS/BLOCKED とも見出し行アンカーの厳密一致で判定する（レポート本文・履歴注記に
# 含まれる恒久的な "BLOCKED" 文字列への誤ヒットで、判定行の欠落・変質を SKIP に
# 丸め込まないため）。判定行がいずれにも一致しない・レポート不在の場合は
# フェイルクローズで FAIL。
perf_report="${WORKSPACE_ROOT}/benches/reports/task-3.3-openapi-performance.md"
if [ ! -f "${perf_report}" ]; then
    record_fail "4: GET /health 性能有意差（±5% 以内）" "${perf_report} が見つかりません（判定不能、フェイルクローズ）"
elif grep -q "^### 判定結果（再計測、#259）: PASS" "${perf_report}"; then
    record_pass "4: GET /health 性能有意差（±5% 以内）" "benches/reports/task-3.3-openapi-performance.md の再計測（#259、RUNS=5 中央値・専有計測枠）で PASS 確定。再計測はレポート記載の手順で行う"
elif grep -q "^### 判定結果（再計測、#259）: BLOCKED" "${perf_report}"; then
    record_skip "4: GET /health 性能有意差（±5% 以内）" "benches/reports/task-3.3-openapi-performance.md の再計測判定行が BLOCKED（判定不能を PASS へ丸めない）。レポート記載の再計測手順で確定させること"
else
    record_fail "4: GET /health 性能有意差（±5% 以内）" "benches/reports/task-3.3-openapi-performance.md に確定判定行（^### 判定結果（再計測、#259）: PASS|BLOCKED）が見つかりません（判定不能、フェイルクローズ）"
fi

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
