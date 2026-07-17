#!/usr/bin/env bash
# REQ-15（依存監査基盤）の受け入れ検証オーケストレータ（TASK-15.4、#52）。
#
# `docs/spec/04-requirements.md` REQ-15 / `docs/spec/05-tasks.md` TASK-15.4 の
# 受け入れ基準を次の 3 節で検証する:
#   1. `deny.toml` ベースライン設定（許可ライセンスリスト・全 feature 監査対象化・
#      無視リスト空維持）がリポジトリに存在する（TASK-15.1、静的検査のみ）
#   2. 全 feature 構成で `cargo audit` 既知脆弱性 0 件・`cargo deny check` 違反 0 件
#      （TASK-15.2。重複実装を避け既存 `scripts/dep-audit.sh` をそのまま再利用する。
#      同スクリプトは feature を動的列挙するため、プラグイン追加時も本スクリプトの
#      変更は不要）
#   3. コアパーサへの fuzz スクリーニング実施（TASK-15.3、Conditional Go 条件(4)）の
#      証跡確認。fuzz target・CI 配線・本実行結果記録の存在を機械検証し、任意で
#      pinned nightly + cargo-fuzz 導入済み環境では実測 smoke も行う
#
# 呼び出し元: 人間が `bash scripts/accept/dep-audit-accept.sh` として直接実行する。
# セルフテスト: `scripts/tests/run-dep-audit-accept-tests.sh`（判定ロジックのみを
# fixture で検証。ネットワーク・cargo ビルド・監査ツールに依存しない）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
cd "${WORKSPACE_ROOT}"

DENY_TOML="${WORKSPACE_ROOT}/deny.toml"

# ---------------------------------------------------------------------------
# 1: deny.toml ベースライン設定（静的検査、ツール不要）
# ---------------------------------------------------------------------------
if [ ! -f "${DENY_TOML}" ]; then
    record_fail "1a: deny.toml 存在確認" "${DENY_TOML} が見つかりません"
else
    record_pass "1a: deny.toml 存在確認" "${DENY_TOML} が存在する"

    # TOML セクション本体（次の [section] またはファイル末尾まで）を抽出する。
    # コメントアウトされた設定やコメント内にのみ残った記述をファイル全体無scope の
    # grep で誤って PASS 判定しないため、判定対象を該当セクションの範囲に限定する
    # （セクション直後に説明コメント行が挟まる deny.toml の実際のレイアウトにも
    # 対応するため、固定行数指定（-A1 等）ではなく awk でセクション範囲を切り出す）。
    extract_toml_section() {
        local section_name="$1"
        local file="$2"
        # 見出し行 "[section_name]" は awk -v 経由だと `\[`/`\]` が正規表現の
        # エスケープではなく文字クラスとして解釈され誤マッチしうるため、正規表現
        # ではなく文字列完全一致（target 変数との ==）でセクション開始を判定する。
        # 各行は次を経てから比較・出力する:
        #   1. 末尾の "\r"（CRLF 由来）を除去する。除去しないと見出し行が
        #      "[section_name]\r" となり完全一致が常に失敗し、CRLF 保存された
        #      deny.toml に対して criteria 1b〜1d が false-fail する
        #      （feasibility-check.sh の extract_section と同一対策）
        #   2. 見出し行末の trailing comment（`#` 以降）を除去してから比較する。
        #      `[graph]  # コメント` のような見出しも正しく検出するため
        #      （advisories 正規表現時代は同等ヘッダにもマッチしていた）
        #   3. 本文行も行末インラインコメント（`#` 以降）を除去してから
        #      出力する。行全体コメント（先頭 `#`）は除去後に空行となり
        #      `line != ""` で自然に除外される。除去しないと
        #      `all-features = false  # all-features = true` のような
        #      インライン注記が生の行として後段の grep に渡り、コメント側の
        #      文字列に誤って PASS 判定される（accept-task-11-5.sh の
        #      `sub(/#.*/, "", line)` と同一対策）
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
            /^\[/ { in_section = 0 }
            in_section {
                line = $0
                sub(/#.*/, "", line)
                sub(/^[ \t]+/, "", line)
                sub(/[ \t]+$/, "", line)
                if (line != "") print line
            }
        ' "${file}"
    }

    # 仕様（TASK-15.1 / REQ-15）記載の許可ライセンス 5 件が [licenses] allow に
    # 含まれることを確認する。deny.toml は TASK-8.1（#26）で ISC を追加済みだが、
    # これは許可リストの「上位互換の拡張」であり 5 件の必須要件を損なわない
    # （許可リスト方式・デフォルト拒否の原則自体は変わらない）。
    licenses_section="$(extract_toml_section "licenses" "${DENY_TOML}")"
    required_licenses=(
        "MIT"
        "Apache-2.0"
        "Apache-2.0 WITH LLVM-exception"
        "Unicode-3.0"
        "BSD-3-Clause"
    )
    missing_licenses=()
    for lic in "${required_licenses[@]}"; do
        if ! printf '%s\n' "${licenses_section}" | grep -qF "\"${lic}\""; then
            missing_licenses+=("${lic}")
        fi
    done
    if [ "${#missing_licenses[@]}" -eq 0 ]; then
        record_pass "1b: 許可ライセンスリスト" "仕様記載 5 ライセンス（MIT/Apache-2.0/Apache-2.0 WITH LLVM-exception/Unicode-3.0/BSD-3-Clause）すべてが [licenses] allow に含まれる"
    else
        record_fail "1b: 許可ライセンスリスト" "以下のライセンスが [licenses] allow に見つかりません: ${missing_licenses[*]}"
    fi

    # 全 feature 構成を監査対象に含める設定（TASK-2.1 以降で plugin-* の optional
    # 依存が増えても監査漏れを防ぐ前提）。
    graph_section="$(extract_toml_section "graph" "${DENY_TOML}")"
    if printf '%s\n' "${graph_section}" | grep -q "all-features = true"; then
        record_pass "1c: [graph] all-features" "all-features = true が設定されている（全 feature 構成を監査対象に含める）"
    else
        record_fail "1c: [graph] all-features" "[graph] all-features = true が見つかりません"
    fi

    # advisories.ignore は空維持が既定方針。非空の場合は運用上の許容（理由コメント
    # 付き追加）がありうるため FAIL ではなく WARN として可視化する
    # （フェイルクローズを弱めず、かつ正当な運用を誤検知しない）。
    advisories_section="$(extract_toml_section "advisories" "${DENY_TOML}")"
    ignore_line="$(printf '%s\n' "${advisories_section}" | grep '^ignore' || true)"
    if [ -z "${ignore_line}" ]; then
        record_fail "1d: [advisories] ignore" "[advisories] セクション直後に ignore 行が見つかりません（想定形式との乖離、要確認）"
    elif printf '%s' "${ignore_line}" | grep -q '\[\]'; then
        record_pass "1d: [advisories] ignore" "ignore = [] が維持されている（無視リスト空）"
    else
        record_warn "1d: [advisories] ignore" "ignore が空でない可能性があります（${ignore_line}）。理由コメントの有無を目視確認してください（フェイルクローズだが運用上の許容を可視化）"
    fi
fi

# ---------------------------------------------------------------------------
# 2: 全 feature 構成で cargo audit / cargo deny check 違反 0 件
# ---------------------------------------------------------------------------
if ! check_tool "cargo-deny" "cargo install --locked cargo-deny@0.19.8"; then
    record_skip "2: 全 feature 構成の依存監査" "cargo-deny 未導入のため判定不能。導入コマンド案内済み"
elif ! check_tool "cargo-audit" "cargo install --locked cargo-audit@0.22.2"; then
    record_skip "2: 全 feature 構成の依存監査" "cargo-audit 未導入のため判定不能。導入コマンド案内済み"
elif ! command -v jq >/dev/null 2>&1; then
    record_skip "2: 全 feature 構成の依存監査" "jq 未導入のため判定不能。OS のパッケージマネージャで導入してください（例: apt install jq）"
else
    if bash "${WORKSPACE_ROOT}/scripts/dep-audit.sh" >/tmp/dep-audit-accept-dep-audit.log 2>&1; then
        record_pass "2: 全 feature 構成の依存監査" "scripts/dep-audit.sh が全構成で正常終了（cargo audit 既知脆弱性 0 件・cargo deny check 違反 0 件。詳細: /tmp/dep-audit-accept-dep-audit.log）"
    else
        record_fail "2: 全 feature 構成の依存監査" "scripts/dep-audit.sh が非 0 終了（詳細: /tmp/dep-audit-accept-dep-audit.log）"
    fi
fi

# ---------------------------------------------------------------------------
# 3: コアパーサへの fuzz スクリーニング実施の証跡確認
# ---------------------------------------------------------------------------
FUZZ_TARGETS_DIR="${WORKSPACE_ROOT}/crates/http/fuzz/fuzz_targets"
if [ ! -d "${FUZZ_TARGETS_DIR}" ]; then
    record_fail "3a: fuzz target 存在確認" "${FUZZ_TARGETS_DIR} が見つかりません"
else
    missing_targets=()
    for target in "parse_request_head" "head_semantics"; do
        if [ ! -f "${FUZZ_TARGETS_DIR}/${target}.rs" ]; then
            missing_targets+=("${target}")
        fi
    done
    if [ "${#missing_targets[@]}" -eq 0 ]; then
        record_pass "3a: fuzz target 存在確認" "期待する 2 target（parse_request_head・head_semantics）が ${FUZZ_TARGETS_DIR} に存在する"
    else
        record_fail "3a: fuzz target 存在確認" "以下の target が見つかりません: ${missing_targets[*]}"
    fi
fi

CI_FILE="${WORKSPACE_ROOT}/.github/workflows/ci.yml"
if [ ! -f "${CI_FILE}" ]; then
    record_fail "3b: CI fuzz-smoke ジョブ存在確認" "${CI_FILE} が見つかりません"
elif grep -q "fuzz-smoke:" "${CI_FILE}" && grep -q "scripts/fuzz.sh" "${CI_FILE}"; then
    record_pass "3b: CI fuzz-smoke ジョブ存在確認" "fuzz-smoke ジョブが ci.yml に存在し scripts/fuzz.sh を呼び出す（TASK-15.3-1、#87 実装済み）"
else
    record_fail "3b: CI fuzz-smoke ジョブ存在確認" "fuzz-smoke ジョブまたは scripts/fuzz.sh 呼び出しが ci.yml に見つかりません"
fi

FUZZING_DOC="${WORKSPACE_ROOT}/docs/design/fuzzing.md"
if [ ! -f "${FUZZING_DOC}" ]; then
    record_fail "3c: fuzz 本実行結果の記録確認" "${FUZZING_DOC} が見つかりません"
elif grep -q "fuzz 本実行結果" "${FUZZING_DOC}" && grep -q "crash/hang を検出せず" "${FUZZING_DOC}"; then
    record_pass "3c: fuzz 本実行結果の記録確認" "docs/design/fuzzing.md に「fuzz 本実行結果」節と crash/hang 未検出の記述が存在する（#88、TASK-15.3-2）"
else
    record_fail "3c: fuzz 本実行結果の記録確認" "docs/design/fuzzing.md に本実行結果の記録（見出し・crash/hang 未検出の記述）が見つかりません"
fi

# 任意の実測 smoke（pinned nightly + cargo-fuzz 導入済み環境でのみ）。未導入は
# 判定不能ではなく「CI fuzz-smoke ジョブで継続検証されている」既定運用として SKIP する。
if command -v cargo-fuzz >/dev/null 2>&1; then
    pinned_nightly="$(grep -oP '(?<=^PINNED_NIGHTLY=")[^"]+' "${WORKSPACE_ROOT}/scripts/fuzz.sh" || true)"
    if [ -n "${pinned_nightly}" ] && rustup toolchain list 2>/dev/null | grep -q "^${pinned_nightly}"; then
        if bash "${WORKSPACE_ROOT}/scripts/fuzz.sh" --max-total-time 60 >/tmp/dep-audit-accept-fuzz.log 2>&1; then
            record_pass "3d: fuzz smoke 実測（任意）" "scripts/fuzz.sh --max-total-time 60 が crash/hang 0 件で正常終了（詳細: /tmp/dep-audit-accept-fuzz.log）"
        else
            record_fail "3d: fuzz smoke 実測（任意）" "scripts/fuzz.sh --max-total-time 60 が非 0 終了（詳細: /tmp/dep-audit-accept-fuzz.log）"
        fi
    else
        record_skip "3d: fuzz smoke 実測（任意）" "pinned nightly（${pinned_nightly:-不明}）が未導入。CI の fuzz-smoke ジョブで継続検証されている"
    fi
else
    record_skip "3d: fuzz smoke 実測（任意）" "cargo-fuzz 未導入のため実測を省略。CI の fuzz-smoke ジョブで継続検証されている（導入する場合: cargo install --locked cargo-fuzz@0.13.2）"
fi

print_summary "REQ-15、TASK-15.4 / #52"
exit "$(summary_exit_code)"
