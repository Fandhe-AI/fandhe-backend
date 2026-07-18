#!/usr/bin/env bash
# dep-audit-accept.sh のセルフテスト（TASK-15.4、#52）。
#
# `scripts/accept/dep-audit-accept.sh` はネットワーク・cargo ビルド・cargo-audit /
# cargo-deny の有無に依存するため、本スクリプトは判定ロジックの部分（deny.toml の
# 許可ライセンス・all-features・ignore 判定、ci.yml の fuzz-smoke ジョブ存在確認の
# grep パターン、docs/design/fuzzing.md の本実行結果記録確認、lib/common.sh の
# PASS/FAIL/SKIP/WARN 集計と終了コードの対応）を fixture・直接呼び出しで切り出して
# 検証する。`run-openapi-accept-tests.sh` と同じくネットワーク・cargo ビルドに
# 依存せず完結させる。
#
# 検証範囲外（本スクリプトが担わないもの）:
#   - dep-audit-accept.sh 全体の実行結果そのもの（cargo audit / cargo deny check
#     実行を含むため、CI・人間によるローカル実行で確認する）
#   - scripts/dep-audit.sh 自体の判定精度（同スクリプトの責務）
#
# 呼び出し元: `.github/workflows/ci.yml` の unsafe-triage ジョブから既存セルフテスト群と
# 同列で呼ばれる（本イシューで追加）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures/dep-audit-accept"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

PASS_COUNT=0
FAIL_COUNT=0

pass() {
    echo "PASS: $1"
    PASS_COUNT=$((PASS_COUNT + 1))
}

fail() {
    echo "FAIL: $1" >&2
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

# dep-audit-accept.sh と同じセクションスコープ抽出（無 scope の grep によるコメント
# アウト・コメント内記述の誤検知を避けるため、判定対象を該当 TOML セクション範囲に
# 限定する）。dep-audit-accept.sh 本体の extract_toml_section と同一ロジックを
# セルフテスト側でも再現する（本体を source せず fixture 経由で判定ロジックのみを
# 検証する既存方針を維持するため）。
extract_toml_section() {
    local section_name="$1"
    local file="$2"
    # dep-audit-accept.sh の extract_toml_section と同一ロジック。
    #   1. 末尾 "\r"（CRLF 由来）除去 → 見出し行の完全一致失敗（false-fail）を防ぐ
    #   2. 見出し行末の trailing comment を除去してから比較 → `[graph]  # 注記`
    #      のような見出しも検出する
    #   3. 本文行も行末インラインコメントを除去してから出力 → コメント側の
    #      文字列で誤って PASS 判定されるのを防ぐ（行全体コメントは除去後に
    #      空行となり自然に除外される）
    #   4. セクション終端は本物のテーブル見出し行（`[section]` / `[[section]]`）
    #      でのみ判定する（単なる行頭 `[` 判定は複数行配列の開き括弧単独行
    #      `ignore =` / `[` / `]` を新規見出しと誤認識しセクションを打ち切る
    #      回帰があったため、dep-audit-accept.sh 本体と同一ロジックに統一）。
    awk -v target="[${section_name}]" '
        {
            sub(/\r$/, "")
        }
        {
            header = $0
            sub(/[ \t]*#.*$/, "", header)
            sub(/[ \t]+$/, "", header)
        }
        header == target { in_section = 1; next }
        (header ~ /^\[[^][]+\]$/ || header ~ /^\[\[[^][]+\]\]$/) { in_section = 0 }
        in_section {
            line = $0
            sub(/#.*/, "", line)
            sub(/^[ \t]+/, "", line)
            sub(/[ \t]+$/, "", line)
            if (line != "") print line
        }
    ' "${file}"
}

# dep-audit-accept.sh 節 1b が使う判定ロジック（必須 5 ライセンスすべてが
# [licenses] allow に含まれるか）を fixture に対して適用する。
required_licenses=(
    "MIT"
    "Apache-2.0"
    "Apache-2.0 WITH LLVM-exception"
    "Unicode-3.0"
    "BSD-3-Clause"
)

licenses_complete() {
    local file="$1"
    local section lic
    section="$(extract_toml_section "licenses" "${file}")"
    for lic in "${required_licenses[@]}"; do
        printf '%s\n' "${section}" | grep -qF "\"${lic}\"" || return 1
    done
    return 0
}

echo "===== 許可ライセンスリスト判定（節 1b）のロジック検証 ====="

if licenses_complete "${FIXTURES_DIR}/deny-full.toml"; then
    pass "5 ライセンス完備の fixture は完備と判定される"
else
    fail "5 ライセンス完備の fixture が完備と判定されなかった"
fi

if ! licenses_complete "${FIXTURES_DIR}/deny-missing-license.toml"; then
    pass "ライセンス欠落（Apache-2.0 WITH LLVM-exception 欠如）の fixture は欠落と判定される"
else
    fail "ライセンス欠落の fixture が誤って完備と判定された"
fi

if ! licenses_complete "${FIXTURES_DIR}/deny-license-comment-only.toml"; then
    pass "[licenses] allow 外（コメントのみ）にライセンスが書かれた fixture は欠落と判定される（無 scope grep の誤 PASS 回帰検知）"
else
    fail "[licenses] allow 外のコメント記述を誤って完備と判定した（無 scope grep への退行）"
fi

if licenses_complete "${FIXTURES_DIR}/deny-license-inline-comment.toml"; then
    pass "allow エントリに行末インラインコメントが付いた fixture でも完備と判定される（正当なコメントを誤って除外しない）"
else
    fail "行末インラインコメント付きの正当な allow エントリを誤って欠落と判定した"
fi

if licenses_complete "${FIXTURES_DIR}/deny-crlf.toml"; then
    pass "CRLF 改行の fixture でも 5 ライセンス完備と判定される（見出し行 \\r 除去の回帰検知）"
else
    fail "CRLF 改行の fixture でライセンス完備を検出できなかった（見出し行の完全一致失敗によるセクション未検出の疑い）"
fi

if licenses_complete "${FIXTURES_DIR}/deny-header-trailing-comment.toml"; then
    pass "見出し行に trailing comment が付いた fixture（[licenses]  # 注記）でも完備と判定される"
else
    fail "見出し行の trailing comment によりセクションが検出できず欠落と誤判定した"
fi

if licenses_complete "${FIXTURES_DIR}/deny-allow-lonebracket-array.toml"; then
    pass "allow = / [ / ] 形式（開き括弧が独立行）の複数行配列でも 5 ライセンス完備と判定される（セクション終端誤判定の回帰検知、PR #145 review 4724103171）"
else
    fail "開き括弧単独行を新規テーブル見出しと誤認識してセクションを打ち切り、完備な allow リストを欠落と誤判定した（セクション終端誤判定への退行）"
fi

echo ""
echo "===== [graph] all-features 判定（節 1c）のロジック検証 ====="

all_features_enabled() {
    local file="$1"
    extract_toml_section "graph" "${file}" | grep -q "all-features = true"
}

if all_features_enabled "${FIXTURES_DIR}/deny-full.toml"; then
    pass "all-features = true を含む fixture は PASS 相当と判定される"
else
    fail "all-features = true を含む fixture が PASS 相当と判定されなかった"
fi

if ! all_features_enabled "${FIXTURES_DIR}/deny-missing-allfeatures.toml"; then
    pass "all-features = false の fixture は FAIL 相当と判定される"
else
    fail "all-features = false の fixture が誤って PASS 相当と判定された"
fi

if ! all_features_enabled "${FIXTURES_DIR}/deny-allfeatures-commented.toml"; then
    pass "all-features = true がコメントアウトされ実値が false の fixture は FAIL 相当と判定される（無 scope grep の誤 PASS 回帰検知）"
else
    fail "コメントアウトされた all-features = true を誤って PASS 相当と判定した（無 scope grep への退行）"
fi

if ! all_features_enabled "${FIXTURES_DIR}/deny-allfeatures-inline-comment.toml"; then
    pass "all-features = false の行末に '# all-features = true' が付いた fixture は FAIL 相当と判定される（インラインコメント false-pass 回帰検知）"
else
    fail "実値が false でも行末インラインコメントの文字列に釣られて PASS 相当と誤判定した（インラインコメント false-pass への退行）"
fi

if all_features_enabled "${FIXTURES_DIR}/deny-crlf.toml"; then
    pass "CRLF 改行の fixture でも all-features = true が PASS 相当と判定される（見出し行 \\r 除去の回帰検知）"
else
    fail "CRLF 改行の fixture で [graph] セクションを検出できなかった"
fi

echo ""
echo "===== [advisories] ignore 判定（節 1d）のロジック検証 ====="

# dep-audit-accept.sh 節 1d の判定ロジック（角括弧の対応を追跡して ignore 配列の
# 実際の中身を取り出し、空白・カンマを除いた残りが空かどうかで判定）を同一ロジックで
# 再現する。3 状態（empty/nonempty/missing）を返し、dep-audit-accept.sh の
# PASS/WARN/FAIL の 3 区分にそのまま対応させる（Cursor Bugbot 指摘: 旧実装の単一
# boolean ヘルパーは missing（FAIL 相当）と nonempty（WARN 相当）を区別せず両方
# "false" にまとめてしまい、self-test がその区別を検証できていなかった）。
ignore_classify() {
    local file="$1"
    local advisories_section ignore_block ignore_inner ignore_inner_stripped
    advisories_section="$(extract_toml_section "advisories" "${file}")"
    if ! printf '%s\n' "${advisories_section}" | grep -q '^ignore[ \t]*='; then
        echo "missing"
        return
    fi
    ignore_block="$(printf '%s\n' "${advisories_section}" | awk '
        BEGIN { depth = 0; started = 0; done = 0 }
        /^ignore[ \t]*=/ { started = 1 }
        started && !done {
            print
            line = $0
            n = length(line)
            for (i = 1; i <= n; i++) {
                c = substr(line, i, 1)
                if (c == "[") depth++
                else if (c == "]") {
                    depth--
                    if (depth == 0) { done = 1 }
                }
            }
        }
    ')"
    ignore_inner="$(printf '%s' "${ignore_block}" | tr '\n' ' ')"
    ignore_inner="$(printf '%s' "${ignore_inner}" | sed -e 's/^ignore[ \t]*=[ \t]*\[//' -e 's/\][ \t]*$//')"
    ignore_inner_stripped="$(printf '%s' "${ignore_inner}" | tr -d '[:space:],')"
    if [ -z "${ignore_inner_stripped}" ]; then
        echo "empty"
    else
        echo "nonempty"
    fi
}

if [ "$(ignore_classify "${FIXTURES_DIR}/deny-full.toml")" = "empty" ]; then
    pass "ignore = [] の fixture は空維持（PASS 相当）と判定される"
else
    fail "ignore = [] の fixture が誤って非空と判定された"
fi

if [ "$(ignore_classify "${FIXTURES_DIR}/deny-nonempty-ignore.toml")" = "nonempty" ]; then
    pass "ignore が非空の fixture は非空（WARN 相当）と判定される"
else
    fail "ignore が非空の fixture が誤って空維持と判定された"
fi

# 実リポジトリの deny.toml と同じく [advisories] 直後に説明コメント行が挟まる
# レイアウト（-A1 固定行数指定では検知できない）でも正しく判定できることを確認する。
if [ "$(ignore_classify "${FIXTURES_DIR}/deny-commented-advisories.toml")" = "empty" ]; then
    pass "[advisories] 直後にコメント行を挟む fixture でも ignore = [] を空維持と判定できる"
else
    fail "[advisories] 直後にコメント行を挟む fixture で ignore = [] を検出できなかった"
fi

if [ "$(ignore_classify "${FIXTURES_DIR}/deny-missing-ignore-line.toml")" = "missing" ]; then
    pass "[advisories] セクションに ignore 行自体が無い fixture は missing（FAIL 相当）と、nonempty（WARN 相当）とは区別して判定される"
else
    fail "ignore 行が無い fixture を missing（FAIL 相当）と判定できなかった（nonempty との混同の疑い）"
fi

if [ "$(ignore_classify "${FIXTURES_DIR}/deny-ignore-inline-comment.toml")" = "nonempty" ]; then
    pass "ignore が非空でも行末に '# ignore = []' が付いた fixture は非空（WARN 相当）と判定される（インラインコメント false-pass 回帰検知）"
else
    fail "実値が非空でも行末インラインコメントの文字列に釣られて空維持と誤判定した（インラインコメント false-pass への退行）"
fi

if [ "$(ignore_classify "${FIXTURES_DIR}/deny-crlf.toml")" = "empty" ]; then
    pass "CRLF 改行の fixture でも ignore = [] が空維持と判定される（見出し行 \\r 除去の回帰検知）"
else
    fail "CRLF 改行の fixture で [advisories] セクションを検出できなかった"
fi

# 複数行形式（`ignore = [` / `]` が別行）でも空判定できることを確認する
# （Cursor Bugbot 指摘: 旧実装は単一行の '[]' 部分文字列一致のみで判定しており、
# 複数行の空配列を誤って非空（WARN）と判定していた）。
if [ "$(ignore_classify "${FIXTURES_DIR}/deny-multiline-empty-ignore.toml")" = "empty" ]; then
    pass "複数行形式の空 ignore（\`ignore = [\` / \`]\`）は空維持（PASS 相当）と判定される（複数行見逃し回帰検知）"
else
    fail "複数行形式の空 ignore を誤って非空と判定した（複数行見逃しへの退行）"
fi

if [ "$(ignore_classify "${FIXTURES_DIR}/deny-multiline-nonempty-ignore.toml")" = "nonempty" ]; then
    pass "複数行形式の非空 ignore は非空（WARN 相当）と判定される"
else
    fail "複数行形式の非空 ignore を誤って空維持と判定した"
fi

# オブジェクト形式（`{ id = "...", reason = "..." }`）の非空エントリで reason
# フィールドに部分文字列 '[]' を含む場合でも非空と判定できることを確認する
# （Cursor Bugbot 指摘: 旧実装は行に '[]' が含まれるかだけで判定しており、
# reason 文字列中の '[]' に釣られて誤って PASS 判定していた）。
if [ "$(ignore_classify "${FIXTURES_DIR}/deny-ignore-object-reason-brackets.toml")" = "nonempty" ]; then
    pass "reason フィールドに '[]' を含むオブジェクト形式の非空 ignore は非空（WARN 相当）と判定される（'[]' 部分文字列誤 PASS 回帰検知）"
else
    fail "reason フィールド中の '[]' に釣られて非空 ignore を誤って空維持と判定した（'[]' 部分文字列誤 PASS への退行）"
fi

echo ""
echo "===== CI fuzz-smoke ジョブ存在確認（節 3b）のロジック検証 ====="

fuzz_job_check() {
    local file="$1"
    grep -q "fuzz-smoke:" "${file}" && grep -q "scripts/fuzz.sh" "${file}"
}

if fuzz_job_check "${FIXTURES_DIR}/ci-with-fuzz-job.yml"; then
    pass "fuzz-smoke ジョブ + スクリプト呼び出しを含む fixture は PASS 相当と判定される"
else
    fail "fuzz-smoke ジョブ + スクリプト呼び出しを含む fixture が PASS 相当と判定されなかった"
fi

if ! fuzz_job_check "${FIXTURES_DIR}/ci-without-fuzz-job.yml"; then
    pass "fuzz-smoke ジョブを含まない fixture は FAIL 相当と判定される"
else
    fail "fuzz-smoke ジョブを含まない fixture が誤って PASS 相当と判定された"
fi

echo ""
echo "===== fuzz 本実行結果記録確認（節 3c）のロジック検証 ====="

fuzz_result_recorded() {
    local file="$1"
    grep -q "fuzz 本実行結果" "${file}" && grep -q "crash/hang を検出せず" "${file}"
}

if fuzz_result_recorded "${FIXTURES_DIR}/fuzzing-with-result.md"; then
    pass "本実行結果の記録を含む fixture は PASS 相当と判定される"
else
    fail "本実行結果の記録を含む fixture が PASS 相当と判定されなかった"
fi

if ! fuzz_result_recorded "${FIXTURES_DIR}/fuzzing-without-result.md"; then
    pass "本実行結果の記録を含まない fixture は FAIL 相当と判定される"
else
    fail "本実行結果の記録を含まない fixture が誤って PASS 相当と判定された"
fi

echo ""
echo "===== 実リポジトリの deny.toml・ci.yml・docs/design/fuzzing.md に対する疎通確認 ====="

if licenses_complete "${WORKSPACE_ROOT}/deny.toml"; then
    pass "実リポジトリの deny.toml は 5 ライセンスすべてを含む（TASK-15.1 実装済みの回帰検知）"
else
    fail "実リポジトリの deny.toml から必須ライセンスの欠落が検出された（退行の可能性）"
fi

if all_features_enabled "${WORKSPACE_ROOT}/deny.toml"; then
    pass "実リポジトリの deny.toml は [graph] all-features = true を含む"
else
    fail "実リポジトリの deny.toml から all-features = true が検出できない（退行の可能性）"
fi

if [ "$(ignore_classify "${WORKSPACE_ROOT}/deny.toml")" = "empty" ]; then
    pass "実リポジトリの deny.toml は [advisories] ignore = [] を維持している（TASK-15.1 実装済みの回帰検知。[advisories] ignore 行削除等の退行を検知）"
else
    fail "実リポジトリの deny.toml から [advisories] ignore = [] の空維持が検出できない（退行の可能性）"
fi

if fuzz_job_check "${WORKSPACE_ROOT}/.github/workflows/ci.yml"; then
    pass "実リポジトリの ci.yml は fuzz-smoke ジョブを含む（TASK-15.3-1 実装済みの回帰検知）"
else
    fail "実リポジトリの ci.yml から fuzz-smoke ジョブが検出できない（退行の可能性）"
fi

if fuzz_result_recorded "${WORKSPACE_ROOT}/docs/design/fuzzing.md"; then
    pass "実リポジトリの docs/design/fuzzing.md は本実行結果を記録している（#88 実装済みの回帰検知）"
else
    fail "実リポジトリの docs/design/fuzzing.md から本実行結果の記録が検出できない（退行の可能性）"
fi

echo ""
echo "===== lib/common.sh の PASS/FAIL/SKIP/WARN 集計と終了コードの対応検証 ====="

# サブシェルで lib/common.sh を source し、record_* の組み合わせごとに
# summary_exit_code() が正しい終了コードを返すことを検証する（dep-audit-accept.sh の
# 「SKIP・WARN は判定不能・運用上の許容の安全側記録であり非 0 終了させない」という
# 設計方針そのものを固定化する）。
check_exit_code() {
    local desc="$1"
    local expected="$2"
    shift 2
    local actual
    actual="$(
        # shellcheck source=../accept/lib/common.sh
        source "${WORKSPACE_ROOT}/scripts/accept/lib/common.sh" >/dev/null
        for entry in "$@"; do
            "record_${entry%%:*}" "criterion" "${entry#*:}" >/dev/null
        done
        summary_exit_code
    )"
    if [ "${actual}" -eq "${expected}" ]; then
        pass "${desc}（exit code: ${actual}）"
    else
        fail "${desc}（期待 exit code: ${expected}, 実際: ${actual}）"
    fi
}

check_exit_code "PASS のみ → exit 0" 0 "pass:ok"
check_exit_code "SKIP のみ → exit 0（ツール未導入等の判定不能を非 0 にしない）" 0 "skip:tool-missing"
check_exit_code "WARN のみ → exit 0（運用上の許容を非 0 にしない）" 0 "warn:ignore-nonempty"
check_exit_code "PASS + SKIP + WARN 混在 → exit 0" 0 "pass:ok" "skip:tool-missing" "warn:ignore-nonempty"
check_exit_code "FAIL を含む → exit 1" 1 "pass:ok" "skip:tool-missing" "warn:ignore-nonempty" "fail:ng"

echo ""
echo "===== 結果: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} ====="
if [ "${FAIL_COUNT}" -gt 0 ]; then
    exit 1
fi
