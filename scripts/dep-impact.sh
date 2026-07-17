#!/usr/bin/env bash
# 依存インパクト計測（TASK-15.2、#17）: feature 構成ごとに依存クレート数・
# リリースバイナリサイズ・`unsafe` 件数を計測し、記録に貼れる markdown 表として
# 標準出力する。
#
# 想定利用者（docs/dep-impact/README.md 参照）: `crates/plugin-*` を追加する PR で
# plugin-builder が変更前後の差分を計測し、reviewer が
# pay-for-what-you-use（feature 無効構成の依存数が増えていないこと）を確認する。
#
# 前提ツールは自動ダウンロードしない（benches/ と同じ方針、security.md）。
# cargo-geiger のみ未導入でも他の計測は継続し、unsafe 件数の行だけ案内表示に置き換える。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

check_command() {
    local cmd="$1"
    if ! command -v "${cmd}" >/dev/null 2>&1; then
        return 1
    fi
    return 0
}

if ! check_command jq; then
    echo "エラー: jq が見つかりません。OS のパッケージマネージャで導入してください（例: apt install jq）" >&2
    exit 1
fi

GEIGER_AVAILABLE=0
if check_command cargo-geiger; then
    GEIGER_AVAILABLE=1
fi

# workspace メンバー名（依存クレート数から除外するため）
mapfile -t workspace_members < <(
    cargo metadata --no-deps --format-version 1 2>/dev/null | jq -r '.packages[].name'
)

is_workspace_member() {
    local name="$1"
    local member
    for member in "${workspace_members[@]}"; do
        if [ "${member}" = "${name}" ]; then
            return 0
        fi
    done
    return 1
}

count_deps() {
    # $1 以降: cargo tree に渡す feature フラグ
    local names
    names="$(cargo tree -e normal --prefix none "$@" 2>/dev/null \
        | sed -E 's/ \(\*\)\s*$//' \
        | awk '{print $1}' \
        | sort -u)"
    local count=0
    local name
    while IFS= read -r name; do
        [ -z "${name}" ] && continue
        if ! is_workspace_member "${name}"; then
            count=$((count + 1))
        fi
    done <<< "${names}"
    echo "${count}"
}

echo "# 依存インパクト計測結果"
echo
echo "計測日時: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
echo

echo "## 依存クレート数（workspace メンバー除外・重複バージョンは union で 1 件として計上）"
echo
echo "| feature 構成 | 依存クレート数 |"
echo "|---|---|"
echo "| --no-default-features | $(count_deps --no-default-features) |"
echo "| default | $(count_deps) |"
echo "| --all-features | $(count_deps --all-features) |"
echo

echo "## リリースバイナリサイズ"
echo
echo "対象: workspace 内の bin ターゲット（現時点は axum-ref のみ。"
echo "フルスクラッチコアの bin は TASK-1.4 以降で追加予定）"
echo
bin_targets="$(cargo metadata --no-deps --format-version 1 2>/dev/null \
    | jq -r '.packages[].targets[] | select(.kind[] == "bin") | .name')"

if [ -z "${bin_targets}" ]; then
    echo "対象 bin なし（スキップ）"
else
    echo "==> cargo build --release（全 bin 対象）" >&2
    cargo build --release --quiet
    echo "| bin | サイズ (bytes) |"
    echo "|---|---|"
    while IFS= read -r bin; do
        [ -z "${bin}" ] && continue
        bin_path="target/release/${bin}"
        if [ -f "${bin_path}" ]; then
            size="$(stat -c '%s' "${bin_path}" 2>/dev/null || stat -f '%z' "${bin_path}")"
            echo "| ${bin} | ${size} |"
        else
            echo "| ${bin} | 対象バイナリなし（スキップ） |"
        fi
    done <<< "${bin_targets}"
fi
echo

echo "## unsafe 件数（cargo-geiger）"
echo
if [ "${GEIGER_AVAILABLE}" -eq 1 ]; then
    cargo geiger --output-format Utf8 2>/dev/null || echo "cargo geiger の実行に失敗しました（詳細は標準エラー出力を確認してください）"
else
    echo "cargo-geiger が未導入のためスキップしました。導入する場合:"
    echo '```'
    echo "cargo install --locked cargo-geiger"
    echo '```'
fi
