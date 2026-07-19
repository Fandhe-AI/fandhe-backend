#!/usr/bin/env bash
# レビューゲート運用の受け入れテスト（TASK-14.3、#41、docs/spec/04-requirements.md REQ-14）:
# REQ-14 の受け入れ基準のうち機械的に確認できる 2 点を再実行可能なテストとして担保する。
#
#   1. AI が生成した変更が CI 全通過（ci-complete）を必須条件としてマージされること
#      → --offline 層で .github/workflows/ci.yml の構成を静的確認
#   2. 危険な unsafe パターンが cargo clippy の deny lint で機械的に検出されること
#      → フル層で PoC-9 模擬パターンを一時複製へ注入し実際に clippy を実行して確認
#
# 設計判断・実施記録は docs/design/review-gate.md を参照。
#
# 2 層構成:
#   --offline （既定は無指定時と異なりこちらを明示）: ネットワーク・cargo ビルド不要。
#     lint 表・ci.yml 構成の静的確認のみ。ci.yml の unsafe-triage ジョブから常時呼ばれる。
#   （オプションなし、既定）: フル層。上記に加えて cargo clippy 実行・gh api での
#     ruleset 検証を行う。受け入れ実施時に手動/任意実行する（cargo ビルド・gh 認証が必要）。
#
# 各テストは独立した assert 関数で実行し、失敗があれば非 0 で終了する
# （フェイルクローズ、.claude/rules/security.md）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${SCRIPTS_DIR}/.." && pwd)"

MODE="full"
if [ "${1:-}" = "--offline" ]; then
    MODE="offline"
fi

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

assert_file_contains() {
    local desc="$1"
    local file="$2"
    local needle="$3"
    if grep -qF -- "${needle}" "${file}"; then
        pass "${desc}"
    else
        fail "${desc}（'${needle}' が ${file} に含まれません）"
    fi
}

# lint 表の各行はコメントアウト（行頭 '#'）されていると Cargo.toml 上は無効になる。
# assert_file_contains の単純な部分文字列検索はコメントアウトされた行にもマッチしてしまい、
# 「コメントアウトで deny/forbid を無効化する」退行を見逃す（PR #117 レビュー指摘）。
# 行頭に非空白文字（'#' を含まない）から始まる、有効な設定行のみを対象にする。
assert_active_config_line() {
    local desc="$1"
    local file="$2"
    local pattern="$3"
    if grep -qE "^${pattern}$" "${file}"; then
        pass "${desc}"
    else
        fail "${desc}（有効な設定行 '${pattern}' が ${file} に見つかりません。コメントアウトされていないか確認してください）"
    fi
}

assert_contains() {
    local desc="$1"
    local haystack="$2"
    local needle="$3"
    if printf '%s' "${haystack}" | grep -qF -- "${needle}"; then
        pass "${desc}"
    else
        fail "${desc}（'${needle}' が出力に含まれません）"
    fi
}

# ==================================================
# オフライン層: lint 表・ci.yml 構成の退行検知（両モード共通）
# ==================================================

echo "===== オフライン層: Cargo.toml lint 表 ====="
CARGO_TOML="${REPO_ROOT}/Cargo.toml"

# TASK-14.2（#40）の forbid 層 11 lint。1 件でも欠けると deny lint 検出の多層防御が
# 弱体化する退行のため、全件を個別に確認する（グループ指定への置換も検知できるよう
# 個別 lint 名で assert する）。
for lint in uninit_vec uninit_assumed_init mem_replace_with_uninit transmuting_null \
    wrong_transmute unsound_collection_transmute eager_transmute \
    cast_slice_different_sizes zst_offset out_of_bounds_indexing \
    not_unsafe_ptr_arg_deref; do
    assert_active_config_line "forbid lint '${lint}' が Cargo.toml に有効な状態で存在する" "${CARGO_TOML}" "${lint} = \"forbid\""
done

# deny 層 3 lint
for lint in undocumented_unsafe_blocks unnecessary_safety_comment multiple_unsafe_ops_per_block; do
    assert_active_config_line "deny lint '${lint}' が Cargo.toml に有効な状態で存在する" "${CARGO_TOML}" "${lint} = \"deny\""
done

assert_active_config_line "unsafe_op_in_unsafe_fn = deny が Cargo.toml に有効な状態で存在する" "${CARGO_TOML}" 'unsafe_op_in_unsafe_fn = "deny"'

echo "===== オフライン層: ci.yml の ci-complete 集約ゲート構成 ====="
CI_YML="${REPO_ROOT}/.github/workflows/ci.yml"

assert_file_contains "ci-complete ジョブが存在する" "${CI_YML}" "ci-complete:"

# ci-complete の判定対象（needs）が黙って縮小される退行を検知する。
# .claude/rules/coding-rust.md が要求する fmt/clippy/test に加え、リポジトリ運用上の
# doc/dep-audit/unsafe-triage・pay-for-what-you-use（TASK-2.2、#19）・
# openapi-two-stage（TASK-3.2、#31）も対象に含める（docs/design/ci-completion-criteria.md）。
#
# 単純なジョブ名の部分文字列検索（grep -qF "${job}" 全体）は、コメントや他ジョブの
# ジョブ ID・ステップ名にジョブ名が偶然出現するだけで PASS してしまい、実際に
# needs から削除された退行を検知できない（PR #117 レビュー指摘）。
# そのため ci-complete ジョブブロックを抽出し、その中の needs 配列の要素として
# 厳密一致で確認する。
CI_COMPLETE_BLOCK="$(awk '
    /^  ci-complete:/ { inblock = 1; print; next }
    inblock && /^  [A-Za-z0-9_-]+:/ { exit }
    inblock { print }
' "${CI_YML}")"

if [ -z "${CI_COMPLETE_BLOCK}" ]; then
    fail "ci-complete ジョブブロックを ${CI_YML} から抽出できない"
else
    # grep/sed の '\s' は GNU 拡張で BSD 実装（macOS 標準）では空白文字として解釈されない
    # ため、POSIX 文字クラス '[[:space:]]' を使う。また「非マッチ時に pipefail で
    # パイプライン全体が非 0 終了し set -e で即座に script が中断する」問題（PR #117
    # レビュー指摘）を避けるため、grep 部分に '|| true' を付けて非マッチを正常系として扱い、
    # 後続の空 NEEDS_LIST 判定に処理を委ねる。printf は sed 側の BSD 実装が末尾行を確実に
    # 処理できるよう明示的に改行を付与する。
    NEEDS_LINE="$(printf '%s\n' "${CI_COMPLETE_BLOCK}" | grep -E '^[[:space:]]*needs:' | head -1 || true)"
    if [ -z "${NEEDS_LINE}" ]; then
        NEEDS_LIST=""
    else
        NEEDS_LIST="$(printf '%s\n' "${NEEDS_LINE}" | sed -E 's/^[[:space:]]*needs:[[:space:]]*\[(.*)\][[:space:]]*$/\1/')"
    fi

    if [ -z "${NEEDS_LIST}" ]; then
        for job in fmt clippy test doc coverage dep-audit unsafe-triage pay-for-what-you-use fuzz-smoke openapi-two-stage; do
            fail "ci-complete の needs 配列を抽出できない（'${job}' を確認できません）"
        done
    else
        for job in fmt clippy test doc coverage dep-audit unsafe-triage pay-for-what-you-use fuzz-smoke openapi-two-stage; do
            found="no"
            IFS=',' read -ra needs_arr <<< "${NEEDS_LIST}"
            for item in "${needs_arr[@]}"; do
                # xargs でトリム（needs: [fmt, clippy, ...] のカンマ区切り前後の空白を除去）
                item="$(printf '%s' "${item}" | xargs)"
                if [ "${item}" = "${job}" ]; then
                    found="yes"
                    break
                fi
            done
            if [ "${found}" = "yes" ]; then
                pass "ci-complete の needs 配列に '${job}' が要素として含まれる"
            else
                fail "ci-complete の needs 配列に '${job}' が含まれない（needs: [${NEEDS_LIST}]）"
            fi
        done
    fi
fi

if [ "${MODE}" = "offline" ]; then
    echo
    echo "===== 結果（--offline）: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
    if [ "${FAIL_COUNT}" -ne 0 ]; then
        exit 1
    fi
    exit 0
fi

# ==================================================
# フル層: deny lint 検出テスト
# PoC-9 模擬パターンを一時複製（git archive HEAD）へ注入し、cargo clippy が
# 実際に forbid 層で検出することを実証する。作業ツリーは一切変更しない。
# ==================================================

check_command() {
    local cmd="$1"
    local install_hint="$2"
    if ! command -v "${cmd}" >/dev/null 2>&1; then
        echo "エラー: ${cmd} が見つかりません。次のコマンドで導入してください:" >&2
        echo "  ${install_hint}" >&2
        exit 2
    fi
}

check_command "cargo" "https://rustup.rs/ の手順に従い導入してください"
check_command "gh" "https://cli.github.com/ の手順に従い GitHub CLI を導入してください"

echo "===== フル層: deny lint 検出テスト（PoC-9 模擬パターン注入） ====="

CLONE_DIR="$(mktemp -d)"
cleanup_clone() {
    rm -rf "${CLONE_DIR}"
}
trap cleanup_clone EXIT

# git archive HEAD で「コミット済みの内容」のみを複製する。作業ツリーの未コミット差分は
# 複製に含めない（コミット済み内容に対する検証、.claude/rules/security.md A08 整合性）。
git -C "${REPO_ROOT}" archive HEAD | tar -x -C "${CLONE_DIR}"

TARGET_LIB="${CLONE_DIR}/crates/http/src/lib.rs"
if [ ! -f "${TARGET_LIB}" ]; then
    echo "エラー: ${TARGET_LIB} が見つかりません（crates/http のレイアウト変更を確認してください）" >&2
    exit 2
fi

# PoC-9（docs/spec/03-poc/ai-first-maintainability/README.md）が実測した代表パターン:
# with_capacity の直後に reserve/set_len で未初期化領域を露出させる。
cat >> "${TARGET_LIB}" <<'RUST_EOF'

// TASK-14.3（#41）受け入れテストが一時的に注入する PoC-9 模擬パターン。
// scripts/tests/run-review-gate-tests.sh の実行時のみ複製ファイルへ追記され、
// 作業ツリー（コミット対象）には一切含まれない。
#[allow(dead_code)]
fn __review_gate_test_uninit_vec(n: usize) -> Vec<u8> {
    let mut v: Vec<u8> = Vec::with_capacity(n);
    unsafe {
        v.reserve(n);
        v.set_len(n);
    }
    v
}
RUST_EOF

set +e
clippy_out="$(cd "${CLONE_DIR}" && cargo clippy -p fandhe-backend-http -- -D warnings 2>&1)"
clippy_exit=$?
set -e

if [ "${clippy_exit}" -ne 0 ]; then
    pass "uninit_vec 注入パターンで cargo clippy が非 0 終了する"
else
    fail "uninit_vec 注入パターンで cargo clippy が exit 0 のまま（forbid lint が機能していない）"
fi
assert_contains "clippy 出力に uninit_vec が含まれる" "${clippy_out}" "uninit_vec"

# #[allow] による抑制の試行 → forbid 層は E0453（allow が forbid と非互換）で
# 抑制自体を許さないことを確認する（PoC-9「#[allow] で黙らせるべきではない」への対応）。
# `sed -i` はバックアップ拡張子の扱いが GNU sed と BSD sed（macOS）で異なり、
# 拡張子省略の GNU 形式（`-i ''` 相当を暗黙に許す）は BSD sed ではスクリプト自体を
# バックアップ拡張子として誤解釈しエラーになる。両方で動く「拡張子を明示して
# バックアップを作り、直後に削除する」形式に統一する（PR #117 レビュー指摘）。
sed -i.bak \
    's/#\[allow(dead_code)\]/#[allow(dead_code, clippy::uninit_vec, clippy::undocumented_unsafe_blocks)]/' \
    "${TARGET_LIB}"
rm -f "${TARGET_LIB}.bak"

set +e
clippy_allow_out="$(cd "${CLONE_DIR}" && cargo clippy -p fandhe-backend-http -- -D warnings 2>&1)"
clippy_allow_exit=$?
set -e

if [ "${clippy_allow_exit}" -ne 0 ]; then
    pass "#[allow(clippy::uninit_vec)] 変種でも cargo clippy が非 0 終了する（抑制不可）"
else
    fail "#[allow(clippy::uninit_vec)] 変種で cargo clippy が exit 0 になった（forbid 抑制不可の保証が破られている）"
fi
assert_contains "clippy 出力に E0453（forbid への allow 禁止）が含まれる" "${clippy_allow_out}" "E0453"

# 作業ツリーを変更していないことを最終確認する（フェイルセーフ、A08 整合性）。
if git -C "${REPO_ROOT}" status --porcelain -- crates/http/src/lib.rs | grep -q .; then
    fail "作業ツリーの crates/http/src/lib.rs が変更されている（一時複製への注入に失敗し実ファイルを汚した疑い）"
else
    pass "作業ツリー（crates/http/src/lib.rs）は変更されていない"
fi

# ==================================================
# フル層: ruleset 検証テスト（gh api）
# ==================================================

echo "===== フル層: ruleset main-required-checks の検証 ====="

REPO_NWO="$(gh repo view --json nameWithOwner --jq '.nameWithOwner' 2>/dev/null || true)"
if [ -z "${REPO_NWO}" ]; then
    fail "gh repo view でリポジトリを特定できない（gh auth login 未実施の可能性）"
else
    RULESETS_JSON="$(gh api "repos/${REPO_NWO}/rulesets" 2>/dev/null || echo '[]')"
    RULESET_OBJ="$(printf '%s' "${RULESETS_JSON}" | jq -c \
        --arg name "main-required-checks" '.[] | select(.name == $name)' | head -1)"

    if [ -z "${RULESET_OBJ}" ]; then
        fail "ruleset 'main-required-checks' が見つからない（setup-required-checks.sh 未実行の可能性）"
    else
        RULESET_ID="$(printf '%s' "${RULESET_OBJ}" | jq -r '.id')"
        ENFORCEMENT="$(printf '%s' "${RULESET_OBJ}" | jq -r '.enforcement')"
        if [ "${ENFORCEMENT}" = "active" ]; then
            pass "ruleset 'main-required-checks' が active"
        else
            fail "ruleset 'main-required-checks' が active でない（enforcement=${ENFORCEMENT}）"
        fi

        # RULESET_DETAIL の取得失敗をフォールバック（空オブジェクト）で握りつぶすと、
        # 後続の jq 抽出が軒並み「存在しない」判定になり、bypass_actors の
        # 'length == 0' チェックだけが意図せず PASS してしまう（fail-open、PR #117
        # レビュー指摘）。取得失敗はここで即座に FAIL とし、後続の内容検証はスキップする
        # （フェイルクローズ、.claude/rules/security.md）。
        if ! RULESET_DETAIL="$(gh api "repos/${REPO_NWO}/rulesets/${RULESET_ID}" 2>/dev/null)"; then
            fail "ruleset 'main-required-checks'（id=${RULESET_ID}）の詳細取得に失敗した（gh api エラー、以降の内容検証をスキップ）"
        else
            if printf '%s' "${RULESET_DETAIL}" | jq -e \
                '.rules[] | select(.type == "required_status_checks") | .parameters.required_status_checks[] | select(.context == "ci-complete")' \
                >/dev/null 2>&1; then
                pass "required_status_checks に ci-complete が含まれる"
            else
                fail "required_status_checks に ci-complete が含まれない"
            fi

            if printf '%s' "${RULESET_DETAIL}" | jq -e '.rules[] | select(.type == "pull_request")' >/dev/null 2>&1; then
                pass "pull_request ルールが有効"
            else
                fail "pull_request ルールが存在しない（PR 必須化が未設定）"
            fi

            if printf '%s' "${RULESET_DETAIL}" | jq -e '.rules[] | select(.type == "non_fast_forward")' >/dev/null 2>&1; then
                pass "non_fast_forward ルールが有効"
            else
                fail "non_fast_forward ルールが存在しない（force push 禁止が未設定）"
            fi

            if printf '%s' "${RULESET_DETAIL}" | jq -e '.rules[] | select(.type == "deletion")' >/dev/null 2>&1; then
                pass "deletion ルールが有効"
            else
                fail "deletion ルールが存在しない（ブランチ削除禁止が未設定）"
            fi

            BYPASS_LEN="$(printf '%s' "${RULESET_DETAIL}" | jq '.bypass_actors | length')"
            if [ "${BYPASS_LEN}" = "0" ]; then
                pass "bypass_actors が空（例外経路なし）"
            else
                fail "bypass_actors が空でない（例外経路が設定されている、fail-closed 違反の疑い: ${BYPASS_LEN} 件）"
            fi

            # conditions.ref_name の静的確認だけでは「ルール内容は正しいが対象ブランチが
            # 誤っている ruleset」を見逃す（PR #117 レビュー指摘）。GitHub がデフォルト
            # ブランチに実際に適用しているルール集合を返す
            # `repos/{nwo}/rules/branches/{branch}` を呼び、この ruleset_id が
            # そこに含まれることまで確認する（docs/design/review-gate.md のブランチ
            # エンドポイント検証要件）。
            DEFAULT_BRANCH="$(gh repo view --json defaultBranchRef --jq '.defaultBranchRef.name' 2>/dev/null || true)"
            if [ -z "${DEFAULT_BRANCH}" ]; then
                fail "gh repo view でデフォルトブランチを特定できない（ruleset のブランチ適用確認をスキップ）"
            else
                BRANCH_RULES_JSON="$(gh api "repos/${REPO_NWO}/rules/branches/${DEFAULT_BRANCH}" 2>/dev/null || true)"
                if [ -z "${BRANCH_RULES_JSON}" ]; then
                    fail "repos/${REPO_NWO}/rules/branches/${DEFAULT_BRANCH} の取得に失敗した（ブランチ適用確認不可）"
                else
                    if printf '%s' "${BRANCH_RULES_JSON}" | jq -e \
                        --argjson rid "${RULESET_ID}" \
                        'any(.[]; .ruleset_id == $rid)' >/dev/null 2>&1; then
                        pass "ruleset 'main-required-checks' がデフォルトブランチ '${DEFAULT_BRANCH}' に実際に適用されている"
                    else
                        fail "ruleset 'main-required-checks'（id=${RULESET_ID}）がデフォルトブランチ '${DEFAULT_BRANCH}' の適用ルールに含まれない（対象ブランチ誤設定の疑い）"
                    fi
                fi

                # setup-required-checks.sh は conditions.ref_name.include に
                # "refs/heads/${DEFAULT_BRANCH}" を厳密指定して ruleset を作成する
                # （ワイルドカード '~DEFAULT_BRANCH' / 'refs/heads/*' は使わない）。
                # テストも実装と同じ具体値で照合する。
                if printf '%s' "${RULESET_DETAIL}" | jq -e \
                    --arg ref "refs/heads/${DEFAULT_BRANCH}" \
                    '.conditions.ref_name.include[]? | select(. == $ref)' \
                    >/dev/null 2>&1; then
                    pass "conditions.ref_name.include がデフォルトブランチ '${DEFAULT_BRANCH}' を対象にしている"
                else
                    fail "conditions.ref_name.include に 'refs/heads/${DEFAULT_BRANCH}' が含まれない（対象ブランチ誤設定の疑い）"
                fi
            fi
        fi
    fi
fi

echo
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -ne 0 ]; then
    exit 1
fi
exit 0
