#!/usr/bin/env bash
# benches/lib/common.sh の BENCH_TARGET_DIR 導出ロジックのオフライン・セルフテスト
# （イシュー #480）。
#
# 背景: 週次ベンチで self-hosted runner のホスト共有 `CARGO_TARGET_DIR` 注入により
# `cargo build --release` の実成果物が決め打ちパス `${WORKSPACE_ROOT}/target` 配下に
# 存在しないケースが発生した（benches/reports/issue480-target-dir-investigation.md）。
# `common.sh` はこれを `CARGO_TARGET_DIR` env → `cargo metadata` → 従来既定
# （`${WORKSPACE_ROOT}/target`）の優先順位で解決する `BENCH_TARGET_DIR` を提供する。
# 本テストはその優先順位・相対パス絶対化・cargo/jq 不在時のフォールバックを
# cargo の実ビルド・ネットワーク非依存で回帰検証する
# （`scripts/tests/run-nfr6-exclusive-tests.sh` と同じ「副作用のある呼び出し元本体は
# source しない」方針。`common.sh` 自体は `set -euo pipefail` を持つため、各ケースは
# サブシェルで source して本テストスクリプト自体の実行を止めないようにする）。
#
# 呼び出し元: 人間 / CI が `bash scripts/tests/run-bench-target-dir-tests.sh` として
# 直接実行する（CI 常設組み込みは行わない。兄弟の accept セルフテストと同じ手動実行、
# .claude/rules/ci.md の schedule 負荷抑制と整合）。

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
COMMON_SH="${REPO_ROOT}/benches/lib/common.sh"

PASS_COUNT=0
FAIL_COUNT=0

fail() {
    echo "FAIL: $1" >&2
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

pass() {
    echo "PASS: $1"
    PASS_COUNT=$((PASS_COUNT + 1))
}

assert_eq() {
    local desc="$1" expected="$2" actual="$3"
    if [ "${expected}" = "${actual}" ]; then
        pass "${desc}"
    else
        fail "${desc}（期待: '${expected}'、実際: '${actual}'）"
    fi
}

# 各ケース専用の隔離ディレクトリ（cargo/jq 不在時のフォールバック検証にも使う
# ダミー manifest 置き場として利用。実 workspace には触れない）。
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

echo "===== BENCH_TARGET_DIR 導出: 優先順位・パス解決の回帰テスト（イシュー #480） ====="

# ケース1: CARGO_TARGET_DIR が絶対パスで指定されている場合はそれをそのまま使う
# （cargo build 自体の解釈と同じ。runner のホスト共有 target 隔離設定を模す）。
case1_result="$(
    unset -v BENCH_TARGET_DIR TARGET_BIN CARGO_TARGET_DIR 2>/dev/null
    CARGO_TARGET_DIR="/tmp/fandhe-backend-fake-target" \
        bash -c "source '${COMMON_SH}' >/dev/null 2>&1; printf '%s\n' \"\${BENCH_TARGET_DIR}\""
)"
assert_eq "CARGO_TARGET_DIR（絶対パス）指定時はそのまま採用" \
    "/tmp/fandhe-backend-fake-target" "${case1_result}"

# ケース2: CARGO_TARGET_DIR が相対パスの場合は WORKSPACE_ROOT 基準で絶対化する
# （cargo 自身も CARGO_TARGET_DIR をカレントディレクトリ基準で解釈するため、
#  WORKSPACE_ROOT 基準に固定してカレントディレクトリ非依存にする契約）。
case2_result="$(
    unset -v BENCH_TARGET_DIR TARGET_BIN CARGO_TARGET_DIR 2>/dev/null
    CARGO_TARGET_DIR="relative-target" \
        bash -c "source '${COMMON_SH}' >/dev/null 2>&1; printf '%s\n' \"\${BENCH_TARGET_DIR}\""
)"
assert_eq "CARGO_TARGET_DIR（相対パス）は WORKSPACE_ROOT 基準で絶対化" \
    "${REPO_ROOT}/relative-target" "${case2_result}"

# ケース3: CARGO_TARGET_DIR 未設定・cargo/jq 利用可能な場合は cargo metadata の
# target_directory を使う（cargo 自身の権威値。.cargo/config.toml の
# build.target-dir 設定も正しく反映される）。cargo/jq 不在環境では本ケースを
# SKIP 扱いにし、フェイルクローズで誤 PASS にしない。
if command -v cargo >/dev/null 2>&1 && command -v jq >/dev/null 2>&1; then
    case3_result="$(
        unset -v BENCH_TARGET_DIR TARGET_BIN CARGO_TARGET_DIR 2>/dev/null
        bash -c "source '${COMMON_SH}' >/dev/null 2>&1; printf '%s\n' \"\${BENCH_TARGET_DIR}\""
    )"
    expected3="$(cargo metadata --format-version 1 --no-deps --manifest-path "${REPO_ROOT}/Cargo.toml" 2>/dev/null | jq -r '.target_directory // empty')"
    if [ -n "${expected3}" ]; then
        assert_eq "CARGO_TARGET_DIR 未設定時は cargo metadata の target_directory を採用" \
            "${expected3}" "${case3_result}"
    else
        echo "SKIP: cargo metadata が target_directory を返さなかったためケース3 をスキップ" >&2
    fi
else
    echo "SKIP: cargo または jq が見つからないためケース3 をスキップ" >&2
fi

# ケース4: CARGO_TARGET_DIR 未設定・cargo/jq いずれも不在の場合は従来既定の
# WORKSPACE_ROOT/target にフォールバックする（fail-closed。例外を呼び出し元へ
# 伝播させず安全側の値を返す契約の検証）。bash・dirname 等 common.sh の実行に
# 必要な最小コマンド群だけをシンボリックリンクした隔離 PATH を組み立て、そこから
# cargo・jq のみを除外する（ディレクトリ単位で PATH を除外すると bash 自身まで
# 道連れで消えてしまう環境があるため、ファイル単位で組み立てる）。
FAKE_BIN_DIR="${TMP_ROOT}/fake-bin"
mkdir -p "${FAKE_BIN_DIR}"
for dir in $(echo "${PATH}" | tr ':' '\n'); do
    [ -d "${dir}" ] || continue
    for exe in "${dir}"/*; do
        [ -x "${exe}" ] && [ -f "${exe}" ] || continue
        name="$(basename "${exe}")"
        [ "${name}" = "cargo" ] && continue
        [ "${name}" = "jq" ] && continue
        [ -e "${FAKE_BIN_DIR}/${name}" ] && continue
        ln -s "${exe}" "${FAKE_BIN_DIR}/${name}" 2>/dev/null || true
    done
done
case4_result="$(
    unset -v BENCH_TARGET_DIR TARGET_BIN CARGO_TARGET_DIR 2>/dev/null
    PATH="${FAKE_BIN_DIR}" \
        bash -c "source '${COMMON_SH}' >/dev/null 2>&1; printf '%s\n' \"\${BENCH_TARGET_DIR}\""
)"
assert_eq "cargo/jq 不在時は従来既定 WORKSPACE_ROOT/target にフォールバック" \
    "${REPO_ROOT}/target" "${case4_result}"

# ケース5: BENCH_TARGET_DIR が明示的に env で上書きされている場合は導出処理を
# 経由せずそのまま尊重する（`BENCH_TARGET_DIR="${BENCH_TARGET_DIR:-...}"` の
# 呼び出し元指定優先の契約検証）。
case5_result="$(
    unset -v TARGET_BIN CARGO_TARGET_DIR 2>/dev/null
    BENCH_TARGET_DIR="/tmp/fandhe-backend-explicit-bench-target-dir" \
        bash -c "source '${COMMON_SH}' >/dev/null 2>&1; printf '%s\n' \"\${BENCH_TARGET_DIR}\""
)"
assert_eq "BENCH_TARGET_DIR 明示指定時は導出処理を経由せずそのまま尊重" \
    "/tmp/fandhe-backend-explicit-bench-target-dir" "${case5_result}"

# ケース6: TARGET_BIN の既定値が BENCH_TARGET_DIR/release/axum-ref に追従する
# （bench-accept.sh・bench-accept-exclusive.sh が参照する既定パスの回帰検証）。
case6_result="$(
    unset -v BENCH_TARGET_DIR TARGET_BIN CARGO_TARGET_DIR 2>/dev/null
    CARGO_TARGET_DIR="/tmp/fandhe-backend-fake-target" \
        bash -c "source '${COMMON_SH}' >/dev/null 2>&1; printf '%s\n' \"\${TARGET_BIN}\""
)"
assert_eq "TARGET_BIN 既定値は BENCH_TARGET_DIR/release/axum-ref に追従" \
    "/tmp/fandhe-backend-fake-target/release/axum-ref" "${case6_result}"

echo
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
exit 0
