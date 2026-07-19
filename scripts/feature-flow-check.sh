#!/usr/bin/env bash
# 機能要求→実装→テスト一貫改修フローの機械チェック（TASK-12.2-1、#81、REQ-12(b)）:
# 実装（crates/<name>/src/**/*.rs）の変更にテスト追加（crates/<name>/tests/** の変更、
# または src 側の追加行に #[test] / #[tokio::test] / #[cfg(test)] / doc test フェンス
# `/// ```` を含む）が伴わないクレートを検出し、伴っていなければ非 0 終了する
# （フェイルクローズ、.claude/rules/security.md）。
#
# 位置づけ: docs/design/feature-modification-flow.md の「実装」→「テスト追加」段階の
# 同時性を機械的に担保する。改善提案フロー（scripts/unsafe-triage.sh 等）が
# コードベースの「安全性」を機械検査するのに対し、本スクリプトは「実装とテストの
# 同時性」を機械検査する（役割は独立、対象データも独立）。
#
# 本スクリプトを CI の必須ゲートとして PR に組み込む対応は #82（完遂判定への
# 組み込み）のスコープ。本 TASK-12.2-1 では機構本体とセルフテストの提供に留める
# （scripts/tests/run-feature-flow-tests.sh・.github/workflows/ci.yml の
# unsafe-triage ジョブから当該セルフテストのみを呼ぶ）。
#
# 使い方:
#   feature-flow-check.sh --base <base-rev> [--head <head-rev>] \
#       [--allow-no-tests <クレート名> "<理由>"]...
#
# --allow-no-tests は「テストを追加しない」ことを明示的に許容する理由必須の除外
# フラグ。暗黙スキップは設けない（フェイルクローズ、.claude/rules/security.md）。
# 除外を使った場合は警告を出力し exit 0 を維持する（レビューで人間が理由を確認する
# 前提。REQ-12(b) の一貫改修フローは自動適用・自動マージを行わない方針と同じ発想）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# REPO_ROOT はスクリプト自身の所在（scripts/ の親）から解決するのが既定だが、
# scripts/tests/run-feature-flow-tests.sh のセルフテストが実 workspace を汚さずに
# 一時 git リポジトリで検証できるよう、環境変数で上書き可能にする
# （unsafe-triage.sh の FANDHE_BACKEND_UNSAFE_TRIAGE_REPO_ROOT と同一パターン）。
REPO_ROOT="${FANDHE_BACKEND_FEATURE_FLOW_REPO_ROOT:-$(cd "${SCRIPT_DIR}/.." && pwd)}"
cd "${REPO_ROOT}"

BASE_REV=""
HEAD_REV="HEAD"
declare -a ALLOW_NO_TESTS_CRATES=()
declare -a ALLOW_NO_TESTS_REASONS=()

usage() {
    cat <<'EOF'
使い方: feature-flow-check.sh --base <base-rev> [--head <head-rev>]
                               [--allow-no-tests <クレート名> "<理由>"]...

  --base <rev>                    比較基点（例: origin/main）。必須
  --head <rev>                    比較対象（既定: HEAD）
  --allow-no-tests <crate> <reason>
                                   指定クレートのテスト追加省略を理由付きで明示的に
                                   許容する（複数回指定可）。理由は空文字列不可
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --base)
            BASE_REV="${2:-}"
            [ -z "${BASE_REV}" ] && { echo "エラー: --base には値が必要です" >&2; exit 2; }
            shift 2
            ;;
        --head)
            HEAD_REV="${2:-}"
            [ -z "${HEAD_REV}" ] && { echo "エラー: --head には値が必要です" >&2; exit 2; }
            shift 2
            ;;
        --allow-no-tests)
            crate="${2:-}"
            reason="${3:-}"
            if [ -z "${crate}" ] || [ -z "${reason}" ]; then
                echo "エラー: --allow-no-tests には <クレート名> と \"<理由>\" の両方が必要です" >&2
                exit 2
            fi
            ALLOW_NO_TESTS_CRATES+=("${crate}")
            ALLOW_NO_TESTS_REASONS+=("${reason}")
            shift 3
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "エラー: 未知の引数です: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [ -z "${BASE_REV}" ]; then
    echo "エラー: --base が指定されていません" >&2
    usage >&2
    exit 2
fi

# crates/<crate_dir>/Cargo.toml の [package] name を返す（存在しない・取得不能なら
# 何も出力せず失敗を返す）。--allow-no-tests はディレクトリ名だけでなく Cargo
# パッケージ名（例: fandhe-backend-core, fandhe-backend-http）でも指定できるようにするための
# 補助関数（Bugbot 指摘: ディレクトリ名比較のみだとパッケージ名指定時に免除が効かない）。
crate_package_name() {
    local crate_dir="$1"
    local toml_content
    toml_content="$(git show "${HEAD_REV}:crates/${crate_dir}/Cargo.toml" 2>/dev/null)" || return 1
    printf '%s\n' "${toml_content}" | awk -F'"' '
        /^\[package\]/ { in_pkg = 1; next }
        /^\[/ { in_pkg = 0 }
        in_pkg && /^[[:space:]]*name[[:space:]]*=/ { print $2; exit }
    '
}

is_allowed() {
    local crate="$1"
    local pkg
    pkg="$(crate_package_name "${crate}")"
    local i
    for i in "${!ALLOW_NO_TESTS_CRATES[@]}"; do
        local given="${ALLOW_NO_TESTS_CRATES[$i]}"
        if [ "${given}" = "${crate}" ] || { [ -n "${pkg}" ] && [ "${given}" = "${pkg}" ]; }; then
            echo "${ALLOW_NO_TESTS_REASONS[$i]}"
            return 0
        fi
    done
    return 1
}

# --------------------------------------------------
# 変更ファイル一覧を NUL 区切りで取得する（パス中の空白・特殊文字を安全に扱う。
# .claude/rules/security.md の「信頼できない入力の安全な取り扱い」に合わせる）。
# --------------------------------------------------
declare -a CHANGED_FILES=()
while IFS= read -r -d '' f; do
    CHANGED_FILES+=("${f}")
done < <(git diff --name-only -z "${BASE_REV}...${HEAD_REV}" -- 'crates/*')

if [ "${#CHANGED_FILES[@]}" -eq 0 ]; then
    echo "==> feature-flow-check: crates/ 配下の変更なし（対象外、exit 0）"
    exit 0
fi

# クレート名 -> src 変更あり / tests 変更あり / src 差分にテストマーカーあり
declare -A SRC_CHANGED=()
declare -A TESTS_CHANGED=()
declare -A SRC_HAS_TEST_MARKER=()

TEST_MARKER_PATTERN='#\[test\]|#\[tokio::test\]|#\[cfg\(test\)\]|///\s*```'

for f in "${CHANGED_FILES[@]}"; do
    # crates/<name>/... の <name> を取り出す
    case "${f}" in
        crates/*/src/*.rs)
            crate="$(printf '%s' "${f}" | cut -d/ -f2)"
            SRC_CHANGED["${crate}"]=1
            # 変更箇所を囲む近傍限定コンテキスト（-U16、典型的な関数本体を
            # 包含する程度の狭い窓。追加行 `+` と前後の非変更コンテキスト行
            # ` ` を対象、削除行 `-` は対象外）にテストマーカーがあれば検知
            # する。追加行自体の新規マーカー（新規テスト追加）はもちろん、
            # 既存の `#[test]` 関数内のアサーションだけを書き換え新規マーカー
            # 行を追加しない編集（Bugbot 指摘: 追加行のみを見る -U0 だと未検出
            # になり誤って exit 1 する）も、この窓に収まっていれば検知する。
            # 窓を無制限（旧 -U1000000）にしないのは、ファイル中のどこか離れた
            # 場所に既存のテストマーカーがあるだけで、無関係な src 編集まで
            # テスト追加ありと誤判定される穴（Bugbot 指摘、L159-164）を縮小
            # するため（doc test 追加を検知する近似ヒューリスティックであり、
            # -U16 圏外の遠い誤判定・-U16 を超える長いテスト関数の取りこぼしは
            # 残り得る。誤検知・誤判定は --allow-no-tests + レビューで運用）。
            if git diff -U16 "${BASE_REV}...${HEAD_REV}" -- "${f}" \
                | grep -E '^[+ ]' \
                | grep -vE '^\+\+\+' \
                | grep -qE "${TEST_MARKER_PATTERN}"; then
                SRC_HAS_TEST_MARKER["${crate}"]=1
            fi
            ;;
        crates/*/tests/*)
            crate="$(printf '%s' "${f}" | cut -d/ -f2)"
            TESTS_CHANGED["${crate}"]=1
            ;;
        *)
            : # crates/ 直下の Cargo.toml 等、src/tests 以外は対象外
            ;;
    esac
done

overall_status=0

for crate in "${!SRC_CHANGED[@]}"; do
    if [ -n "${TESTS_CHANGED[${crate}]:-}" ] || [ -n "${SRC_HAS_TEST_MARKER[${crate}]:-}" ]; then
        echo "==> OK: クレート '${crate}' は実装変更にテスト追加を伴っています"
        continue
    fi

    if reason="$(is_allowed "${crate}")"; then
        echo "==> 警告: クレート '${crate}' はテスト追加なしで --allow-no-tests により許容されています（理由: ${reason}）" >&2
        continue
    fi

    echo "==> エラー: クレート '${crate}' で実装変更（crates/${crate}/src/**/*.rs）がありますが、テスト追加（crates/${crate}/tests/** の変更、または #[test] / #[tokio::test] / #[cfg(test)] / doc test の追加）が検出されません" >&2
    echo "  対応: テストを追加するか、レビューで理由を明示したうえで --allow-no-tests ${crate} \"<理由>\" を使ってください（.claude/rules/feature-modification.md）" >&2
    overall_status=1
done

if [ "${overall_status}" -ne 0 ]; then
    exit 1
fi

echo "==> feature-flow-check: 実装変更を伴う全クレートでテスト追加を確認しました"
exit 0
