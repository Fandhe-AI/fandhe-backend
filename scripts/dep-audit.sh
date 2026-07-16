#!/usr/bin/env bash
# 依存監査（TASK-15.2、#17、docs/spec/05-tasks.md）: cargo audit（既知脆弱性）と
# cargo deny check（ライセンス・出所・重複、deny.toml）を「全 feature 構成」で実行する。
#
# ci.yml の dep-audit ジョブから呼ばれる（deny.toml 冒頭コメント参照）。TASK-2.1 以降で
# crates/plugin-* に feature が増えても、本スクリプトはそれを動的に列挙して監査対象に
# 加えるため、CI 定義（ci.yml）・本スクリプト自体の変更は不要になる設計にしている。
#
# 実装メモ（cargo-deny 0.19 時点の仕様確認結果）:
# `cargo deny check` には `--features` / `--no-default-features` / `--all-features` の
# ような CLI フラグが存在しない（`cargo deny check --help` で確認済み）。feature 構成の
# 切り替えは deny.toml の `[graph]` セクションでのみ制御される。
# そこで本スクリプトは `cargo metadata --format-version 1 <feature フラグ>` で構成ごとの
# 依存グラフ JSON を生成し、`cargo deny check --metadata-path <json>` に渡すことで
# deny.toml 自体を書き換えずに構成を切り替える（`--metadata-path` 指定時は
# `cargo metadata` が確定させた依存グラフがそのまま使われ、deny.toml 側の
# `[graph] all-features` 等は評価に影響しない。実機確認済み）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

# --------------------------------------------------
# 前提ツールの存在検査（自動インストールしない。security.md・benches/ の既存規約に準拠）
# --------------------------------------------------
check_command() {
    local cmd="$1"
    local install_hint="$2"
    if ! command -v "${cmd}" >/dev/null 2>&1; then
        echo "エラー: ${cmd} が見つかりません。次のコマンドで導入してください:" >&2
        echo "  ${install_hint}" >&2
        exit 1
    fi
}

check_command "cargo-deny" "cargo install --locked cargo-deny@0.19.8"
check_command "cargo-audit" "cargo install --locked cargo-audit@0.22.2"
check_command "jq" "OS のパッケージマネージャで jq を導入してください（例: apt install jq）"

# --------------------------------------------------
# cargo audit（RustSec advisory DB による既知脆弱性検知、OWASP A06）
#
# Cargo.lock は全 optional 依存を含む feature 非依存の依存解決結果であり
# （`cargo generate-lockfile` は feature 構成に関わらず workspace 全クレートの
# 依存を解決する）、1 回の cargo audit 実行で全 feature 構成の依存をカバーできる。
# Cargo.lock は .gitignore 対象（コミットしない運用）のため、監査直前に生成する。
# --------------------------------------------------
echo "==> Cargo.lock を生成（既存があれば最新化せず利用）"
if [ ! -f Cargo.lock ]; then
    cargo generate-lockfile
fi

echo "==> cargo audit"
cargo audit

# --------------------------------------------------
# cargo deny check（ライセンス・出所・重複）を feature 構成ごとに実行
# --------------------------------------------------
WORKDIR="$(mktemp -d)"
trap 'rm -rf "${WORKDIR}"' EXIT

# workspace 全メンバーの feature 名を union で列挙する。
# 現時点（TASK-2.1 未着手）は feature を持つクレートが存在しないため空集合になり、
# no-default / default / all-features の 3 構成のみが実行される。
mapfile -t features < <(
    cargo metadata --no-deps --format-version 1 2>/dev/null \
        | jq -r '.packages[].features | keys[]' \
        | sort -u
)

run_deny_check() {
    local label="$1"
    shift
    local metadata_json="${WORKDIR}/metadata-${label}.json"

    # `cargo metadata` はワークスペースルートで実行すると常に全メンバーを対象にする
    # （`--workspace` のような限定フラグは存在しない）。feature フラグのみで
    # 構成を切り替える。
    echo "==> cargo metadata (${label})"
    cargo metadata --format-version 1 "$@" > "${metadata_json}"

    echo "==> cargo deny check (${label})"
    cargo deny check -c deny.toml --metadata-path "${metadata_json}"
}

overall_status=0

run_deny_check "no-default-features" --no-default-features || overall_status=1
run_deny_check "default" || overall_status=1

for feature in "${features[@]}"; do
    run_deny_check "feature-${feature}" --no-default-features --features "${feature}" || overall_status=1
done

run_deny_check "all-features" --all-features || overall_status=1

if [ "${overall_status}" -ne 0 ]; then
    echo "==> dep-audit.sh: 1 件以上の構成で cargo deny check が失敗しました" >&2
    exit 1
fi

echo "==> dep-audit.sh: 全構成で cargo audit / cargo deny check が正常終了しました"
