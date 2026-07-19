#!/usr/bin/env bash
# コア全体の行カバレッジ計測（TASK-11.5-2、#78、docs/spec/05-tasks.md）。
#
# 親タスク TASK-11.5（#37）は「コア全体の自動テスト行カバレッジ 80% 以上」を要求する。
# 本スクリプトは cargo-llvm-cov + cargo-nextest（.config/nextest.toml の profile ci）で
# 「コア」（`crates/plugin-*` と `crates/axum-ref` を除いた workspace メンバー）の行カバレッジを
# 計測し、`--fail-under-lines 80` を満たさない場合は非 0 で終了する（退行ゲート）。
#
# `scripts/accept-task-11-5.sh` のチェック 1・`.github/workflows/ci.yml` の `coverage`
# ジョブから呼ばれる。dep-audit.sh と同様、対象クレートは `cargo metadata` から動的に
# 列挙するため、TASK-2.1 以降で `crates/plugin-*` が増えても本スクリプトの変更は不要。
#
# doc test（`cargo test --doc`）は stable ツールチェーンでは instrumented coverage の対象に
# ならない（cargo-llvm-cov は nightly 専用の `--doctests` フラグでのみ doc test を計測できる。
# rust-toolchain.toml は stable 固定のため対象外とする）。よって本スクリプトが計測するのは
# 単体テスト・統合テスト（nextest 経由）の行カバレッジのみである。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

# 80% 閾値の出典: docs/spec/05-tasks.md TASK-11.5（#37）。
FAIL_UNDER_LINES="${FAIL_UNDER_LINES:-80}"

OUTPUT_DIR="${REPO_ROOT}/target/llvm-cov"
mkdir -p "${OUTPUT_DIR}"

# --------------------------------------------------
# 前提ツールの存在検査（自動インストールしない。dep-audit.sh・security.md の既存規約に準拠）
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

check_command "cargo-llvm-cov" "cargo install --locked cargo-llvm-cov@0.8.7"
check_command "cargo-nextest" "cargo install --locked cargo-nextest@0.9.137"
check_command "jq" "OS のパッケージマネージャで jq を導入してください（例: apt install jq）"

if ! rustup component list --installed 2>/dev/null | grep -q '^llvm-tools'; then
    echo "エラー: llvm-tools-preview コンポーネントが見つかりません。次のコマンドで導入してください:" >&2
    echo "  rustup component add llvm-tools-preview" >&2
    exit 1
fi

# --------------------------------------------------
# 「コア」クレートの動的決定
#
# docs/spec/05-tasks.md TASK-11.5 の「コア全体」は `crates/core`（fandhe-backend-core）・
# `crates/http`（fandhe-backend-http）・将来追加される `crates/routes` を指し、性能比較用参照実装
# （axum-ref）とプラグイン（fandhe-backend-plugin-*）は含まない（プラグインは pay-for-what-you-use により
# feature 単位で着脱されるため、コアの 80% ゲートに混ぜると feature 追加のたびに閾値の意味が
# 変わってしまう）。Cargo.toml 冒頭コメントのクレート分割方針に合わせ、`cargo metadata` から
# workspace メンバーを列挙し、`axum-ref` と `fandhe-backend-plugin-*`（名前が `plugin-` を含むパッケージ）を
# 除いた残りを「コア」として動的に決定する。
# --------------------------------------------------
mapfile -t all_members < <(
    cargo metadata --no-deps --format-version 1 2>/dev/null \
        | jq -r '.packages[].name' \
        | sort -u
)

core_packages=()
for pkg in "${all_members[@]}"; do
    case "${pkg}" in
        axum-ref) continue ;;
        *plugin*) continue ;;
        *) core_packages+=("${pkg}") ;;
    esac
done

if [ "${#core_packages[@]}" -eq 0 ]; then
    echo "エラー: コア対象のパッケージが 1 件も見つかりませんでした（workspace 構成を確認してください）" >&2
    exit 1
fi

echo "==> コア対象パッケージ: ${core_packages[*]}"

package_args=()
for pkg in "${core_packages[@]}"; do
    package_args+=(-p "${pkg}")
done

# --------------------------------------------------
# 計測前クリーンアップ（CI #162/#89 の障害切り分けで発覚、self-hosted runner 特有）:
# self-hosted ランナーは `target/` が実行間で永続化されるため、直前の実行が残した
# instrumented coverage 成果物（profraw・カバレッジマップ）が残存しうる。特に
# 直近で変更されたファイルのソースが再ビルドで置き換わった場合、古いカバレッジ
# マップの行数と新しいソースの行数が食い違い、`cargo llvm-cov report` が両者を
# 誤って合算して「実際には存在しない未カバー行」を計上することがある
# （観測事例: CI run 29626701284 で `core/src/server.rs` のみ行数が実ソースの
# 約 1.8 倍に膨れ、被覆行数自体は新鮮な計測と完全一致していた＝ゴースト分すべてが
# missed 側に計上されていた）。`cargo llvm-cov clean --workspace` で計測前に
# 既存の instrumented 成果物を明示的に破棄し、常に当該コミットのソースのみを
# 反映した計測にする（`cargo llvm-cov clean --help` 参照）。
# --------------------------------------------------
echo "==> cargo llvm-cov clean --workspace（stale coverage 成果物の除去）"
cargo llvm-cov clean --workspace

# --------------------------------------------------
# 計測本体: workspace 全体（プラグイン含む、--all-features）を 1 回だけ計測し、
# lcov/summary はコア対象・workspace 全体の双方を `cargo llvm-cov report` の
# パッケージフィルタ（再計測なし）で出し分ける。プラグインを含めて計測すること自体は
# 各プラグインの feature ゲート下のテストも実行対象にするために必要（doc test は
# nextest が実行しないため対象外、上記コメント参照）。閾値判定（80%）は
# コア対象のみに適用し、プラグインの増減がコアのゲートに影響しないようにする。
# --------------------------------------------------
echo "==> cargo llvm-cov nextest（workspace 全体、--all-features、1 回のみ計測）"
cargo llvm-cov nextest --profile ci --workspace --all-features --no-report

echo "==> カバレッジ判定（コア対象、閾値 ${FAIL_UNDER_LINES}%）"
set +e
cargo llvm-cov report \
    "${package_args[@]}" \
    --lcov --output-path "${OUTPUT_DIR}/lcov.info" \
    --fail-under-lines "${FAIL_UNDER_LINES}"
core_status=$?
set -e

echo "==> カバレッジサマリ（コア対象、再計測なしでレポートのみ出力）"
cargo llvm-cov report "${package_args[@]}" | tee "${OUTPUT_DIR}/summary-core.txt"

# --------------------------------------------------
# 参考情報: workspace 全体（プラグイン含む）のカバレッジも併記する。
# 閾値判定には使わない（コア対象の判定はあくまで core_status）。
# --------------------------------------------------
echo "==> カバレッジサマリ（参考: workspace 全体、プラグイン含む）"
cargo llvm-cov report --summary-only | tee "${OUTPUT_DIR}/summary-workspace.txt"

if [ "${core_status}" -ne 0 ]; then
    echo "==> coverage.sh: コア対象の行カバレッジが ${FAIL_UNDER_LINES}% 未満です" >&2
    exit "${core_status}"
fi

echo "==> coverage.sh: コア対象の行カバレッジが ${FAIL_UNDER_LINES}% 以上であることを確認しました"
echo "    lcov: ${OUTPUT_DIR}/lcov.info"
