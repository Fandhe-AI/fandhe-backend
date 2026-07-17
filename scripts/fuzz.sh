#!/usr/bin/env bash
# fuzz 実行スクリプト（TASK-15.3-1、#87、docs/spec/05-tasks.md）。
#
# `crates/http/fuzz`（cargo-fuzz / libFuzzer、root Cargo.toml の workspace から
# exclude 済み）の全 fuzz target を、pinned nightly ツールチェーンで順次実行する。
# 対象は `crates/http/src` の sans-IO パーサ群（`request.rs` の doc コメント
# 「I/O なしでそのまま fuzz（TASK-15.3 / #51）に供せる」を実現する CI 側の受け皿）。
#
# 本スクリプトは CI（ci.yml の fuzz-smoke ジョブ）から短時間実行（既定 60 秒/target）
# される「smoke」用途と、ローカルでの長時間スクリーニング（#88 のスコープ）の両方に
# 使う `--max-total-time` で秒数を切り替える。
#
# nightly バージョンは本スクリプトの定数を単一真実源とする
# （rust-toolchain.toml はリポジトリ既定 = stable を維持し変更しない。
# .claude/rules/coding-rust.md「fuzz / サニタイザは nightly を明示的に使う」）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
FUZZ_DIR="${REPO_ROOT}/crates/http/fuzz"

# fuzz / サニタイザ計装専用の nightly pin（単一真実源）。更新する場合は本行のみ
# 変更し、CI 側（ci.yml fuzz-smoke ジョブ）は `rustup toolchain install` で
# この値をそのまま使うため追随不要。
PINNED_NIGHTLY="nightly-2026-07-15"

# バージョン固定インストール（scripts/dep-audit.sh 等の既存方針に合わせる）。
CARGO_FUZZ_VERSION="0.13.2"

MAX_TOTAL_TIME=60
LIST_ONLY=0

usage() {
    cat <<EOF
使い方: bash scripts/fuzz.sh [--max-total-time SECONDS] [--list]

  --max-total-time SECONDS  各 fuzz target あたりの実行秒数（既定: 60。smoke 用途）。
                             長時間スクリーニング（#88）ではより大きい値を指定する。
  --list                    fuzz target 名を列挙して終了する（実行しない）。
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --max-total-time)
            MAX_TOTAL_TIME="$2"
            shift 2
            ;;
        --list)
            LIST_ONLY=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "エラー: 不明な引数です: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

# --------------------------------------------------
# 前提ツールの存在検査（自動インストールしない。.claude/rules/security.md・
# scripts/README.md の既存規約に準拠）
# --------------------------------------------------
if ! rustup toolchain list | grep -qF "${PINNED_NIGHTLY}"; then
    echo "エラー: nightly ツールチェーン ${PINNED_NIGHTLY} が見つかりません。次のコマンドで導入してください:" >&2
    echo "  rustup toolchain install ${PINNED_NIGHTLY} --profile minimal" >&2
    exit 1
fi

if ! command -v cargo-fuzz >/dev/null 2>&1; then
    echo "エラー: cargo-fuzz が見つかりません。次のコマンドで導入してください:" >&2
    echo "  cargo install --locked cargo-fuzz@${CARGO_FUZZ_VERSION}" >&2
    exit 1
fi

if ! cargo "+${PINNED_NIGHTLY}" -V >/dev/null 2>&1; then
    echo "エラー: cargo +${PINNED_NIGHTLY} を実行できません（ツールチェーンの破損の可能性）。" >&2
    exit 1
fi

# libFuzzer のサニタイザ計装ビルドには C コンパイラが必要（libfuzzer-sys の
# C++ ランタイムのビルドに使う）。
check_c_compiler() {
    if command -v cc >/dev/null 2>&1; then
        return 0
    fi
    if command -v clang >/dev/null 2>&1; then
        return 0
    fi
    return 1
}

if ! check_c_compiler; then
    echo "エラー: C コンパイラ（cc / clang）が見つかりません。次のいずれかで導入してください:" >&2
    echo "  apt install build-essential  # または clang" >&2
    echo "  （self-hosted runner で C ツールチェーンが欠如する場合、afl.rs へのフォールバックを検討する。" >&2
    echo "   docs/design/fuzzing.md 参照）" >&2
    exit 1
fi

cd "${FUZZ_DIR}"

# --------------------------------------------------
# fuzz target の列挙（fuzz_targets/*.rs のファイル名 = target 名という cargo-fuzz の慣習）
# --------------------------------------------------
mapfile -t targets < <(
    find "${FUZZ_DIR}/fuzz_targets" -maxdepth 1 -name '*.rs' -exec basename {} .rs \; | sort
)

if [ "${#targets[@]}" -eq 0 ]; then
    echo "エラー: crates/http/fuzz/fuzz_targets/ に fuzz target が見つかりません" >&2
    exit 1
fi

if [ "${LIST_ONLY}" -eq 1 ]; then
    printf '%s\n' "${targets[@]}"
    exit 0
fi

echo "==> pinned nightly: ${PINNED_NIGHTLY} / max-total-time: ${MAX_TOTAL_TIME}s / targets: ${targets[*]}"

# --------------------------------------------------
# 各 target を順次実行する。crash を検出した場合は非 0 終了し artifacts のパスを
# 表示する（フェイルクローズ、.claude/rules/security.md）。self-hosted runner の
# 無期限占有を避けるため `-max_total_time` に加え `-rss_limit_mb` でメモリも制限する。
# --------------------------------------------------
overall_status=0

for target in "${targets[@]}"; do
    echo "==> cargo +${PINNED_NIGHTLY} fuzz run ${target}"
    if ! cargo "+${PINNED_NIGHTLY}" fuzz run "${target}" -- \
        "-max_total_time=${MAX_TOTAL_TIME}" \
        "-rss_limit_mb=2048"; then
        echo "エラー: fuzz target ${target} でクラッシュ/ハングを検出しました。" >&2
        echo "  再現入力: crates/http/fuzz/artifacts/${target}/" >&2
        overall_status=1
    fi
done

if [ "${overall_status}" -ne 0 ]; then
    echo "==> fuzz.sh: 1 件以上の target でクラッシュ/ハングを検出しました" >&2
    exit 1
fi

echo "==> fuzz.sh: 全 target が正常終了しました（クラッシュなし）"
