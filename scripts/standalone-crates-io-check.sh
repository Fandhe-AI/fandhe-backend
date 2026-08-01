#!/usr/bin/env bash
# standalone クレートの crates.io 依存のみビルド検証（イシュー #371）。
#
# `templates/*`・`examples/*` の standalone クレートは、依存を
# `version = "0.1.0"` + `path = "../../crates/..."` の併記で宣言しており、CI・
# ローカルの `cargo build`/`cargo test` は常にローカル実装（HEAD）に対して行われる。
# そのため「利用者が README / Cargo.toml 冒頭コメントの案内どおり `path` を外して
# リポジトリ外へコピーした場合に、crates.io 公開版（0.1.0）だけでビルド・テストが
# 通るか」はどこでも機械検証されておらず、crates/ 側 API が公開版と乖離した際に
# 「コピーすると壊れるテンプレート」をすり抜ける。本スクリプトは各クレートを
# 一時ディレクトリへコピーし、`path` 指定を除去した上で crates.io レジストリ解決
# のみで `cargo build` → `cargo test` を実行することでこの乖離を検知する。
#
# .github/workflows/standalone-crates-io.yml から呼ばれる（PR paths トリガー +
# 週次 schedule + workflow_dispatch。ci.yml へ相乗りしない理由は同 workflow の
# 冒頭コメント参照）。ローカルでも同一コマンドで再現できる。
#
# 判定は fail-closed（.claude/rules/security.md）:
# - 対象クレートの検出が 0 件（ディレクトリ構成ドリフト）→ exit 1
# - `path` 除去件数が 0 件（Cargo.toml 書式ドリフトで sed が空振り）→ exit 1
# - 除去後に `path =` 指定が残存（除去漏れ）→ exit 1
# - 1 クレートでも build/test FAIL → exit 1
# - SKIP マーカーの理由が空 → exit 1（下記 SKIP 節参照）
# - 全クレートが SKIP され PASS が 0 件（検証が実質何もしていない）→ exit 1
#
# SKIP（crates.io 未再公開の新 API 依存、イシュー #433）:
# `crates/` 側で新規 API を追加後、次の crates.io 再公開（lockstep バージョニング、
# docs/design/crates-io-release.md）までの間は、その API を使う examples/* は
# crates.io 公開版のみでは解決できずこの検証を通らない（依存自体を後退させるのは
# 本末転倒）。対象クレート直下に `.standalone-crates-io-skip` を置くと理由 1 行を
# 読み取って stage 3 の build/test を SKIP する（PASS 扱いにはしない。集計に
# SKIP として残し、検証をすり抜けさせない）。ファイルは空であってはならない
# （理由必須、fail-closed）。
#
# 恒久的な抜け道にしない: 次回 crates.io 公開時に
# `docs/design/crates-io-release.md` の公開手順で該当マーカーを削除し再検証する
# （同ファイルに手順を明記済み）。
#
# 使い方:
#   bash scripts/standalone-crates-io-check.sh
set -euo pipefail

usage() {
    echo "usage: $0" >&2
    echo "  templates/*・examples/* の standalone クレートを一時ディレクトリへコピーし、" >&2
    echo "  path 依存を除去して crates.io 公開版のみで cargo build / cargo test を実行する" >&2
}

case "${1:-}" in
    "") ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        echo "未知の引数: $1" >&2
        usage
        exit 2
        ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

# --------------------------------------------------
# 前提ツールの存在検査（自動インストールしない。dep-audit.sh と同一方針）
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

check_command "cargo" "rustup（https://rustup.rs/）でツールチェーンを導入してください"

# --------------------------------------------------
# stage 1: 対象クレートの自動検出
#
# templates/・examples/ 直下の Cargo.toml を走査する（列挙ハードコードなし。
# 今後 examples/with-* が増えても本スクリプト・workflow の変更は不要）。
# 0 件検出はディレクトリ構成ドリフトの兆候として fail-closed で異常終了する。
# --------------------------------------------------
echo "== stage 1: 対象クレートの検出（templates/*/Cargo.toml・examples/*/Cargo.toml） =="

crate_dirs=()
for manifest in templates/*/Cargo.toml examples/*/Cargo.toml; do
    if [ -f "${manifest}" ]; then
        crate_dirs+=("$(dirname "${manifest}")")
    fi
done

if [ "${#crate_dirs[@]}" -eq 0 ]; then
    echo "エラー: 対象クレートが 1 件も検出できませんでした（templates/*/Cargo.toml・examples/*/Cargo.toml）" >&2
    exit 1
fi

echo "検出: ${#crate_dirs[@]} クレート"
for crate_dir in "${crate_dirs[@]}"; do
    echo "  - ${crate_dir}"
done

# --------------------------------------------------
# stage 2: 一時ディレクトリへのコピー + path 依存の除去
#
# 各クレートの Cargo.toml は複数行インラインテーブル形式で
#   fandhe-backend-core = { version = "0.1.0", path = "../../crates/core", features = [
#   fandhe-backend-http = { version = "0.1.0", path = "../../crates/http" }
# のように `, path = "../../crates/..."` を version と同一行に併記する
# （templates/app/Cargo.toml 冒頭コメント参照）。この `, path = ...` 部分のみを
# sed で除去し、crates.io の version 指定だけを残す。
# --------------------------------------------------
WORKDIR="$(mktemp -d)"
trap 'rm -rf "${WORKDIR}"' EXIT

echo "== stage 2: 一時コピー + path 依存の除去（作業先: ${WORKDIR}） =="

# クレートごとのコピー先（WORKDIR 配下）。crate_dirs と同一インデックスで対応する。
copy_dirs=()

for crate_dir in "${crate_dirs[@]}"; do
    # コピー先はパス区切りを `-` に潰した一意な名前にする（templates-app 等）。
    copy_dir="${WORKDIR}/${crate_dir//\//-}"
    mkdir -p "${copy_dir}"
    cp -R "${crate_dir}/." "${copy_dir}/"
    # ローカルビルドの成果物はコピー対象外（crates.io 解決だけで検証するため、
    # 既存 target/ の残骸がビルド結果へ影響しないよう除去する）。
    rm -rf "${copy_dir}/target"
    # 一時コピーはリポジトリ外にあり root の rust-toolchain.toml のピンが効かない。
    # `--default-toolchain none` の新規 runner では toolchain 未選択で cargo が失敗
    # するため、ピンごと同梱して repo と同一ツールチェーンで検証する（Bugbot 指摘対応）。
    cp "${REPO_ROOT}/rust-toolchain.toml" "${copy_dir}/"

    manifest="${copy_dir}/Cargo.toml"

    # fail-closed ガード 1: 除去対象（コメント行以外の `, path = "../../crates/..."`）
    # が 0 件なら、Cargo.toml の書式が前提からドリフトしており sed が空振りする。
    # 検証をすり抜けさせず異常終了する。
    path_count="$(grep -cE '^[^#]*, path = "\.\./\.\./crates/' "${manifest}" || true)"
    if [ "${path_count}" -eq 0 ]; then
        echo "エラー: ${crate_dir}/Cargo.toml に除去対象の path 指定（, path = \"../../crates/...\"）が見つかりません（書式ドリフト）" >&2
        exit 1
    fi

    # コメント行（`path` の案内文を含む）は変更せず、依存行の `, path = "..."` のみ除去する。
    sed '/^[[:space:]]*#/!s|, path = "\.\./\.\./crates/[^"]*"||g' "${manifest}" > "${manifest}.tmp"
    mv "${manifest}.tmp" "${manifest}"

    # fail-closed ガード 2: 除去後にコメント行以外へ `path =` 指定が残っていれば
    # 除去漏れ（ローカル実装への参照が残ったままの検証は無意味）として異常終了する。
    if grep -qE '^[^#]*path[[:space:]]*=' "${manifest}"; then
        echo "エラー: ${crate_dir}/Cargo.toml の path 除去後に path 指定が残存しています:" >&2
        grep -nE '^[^#]*path[[:space:]]*=' "${manifest}" >&2
        exit 1
    fi

    echo "path 除去: ${crate_dir}（${path_count} 件） -> ${copy_dir}"
    copy_dirs+=("${copy_dir}")
done

# --------------------------------------------------
# stage 3: crates.io 解決のみで cargo build / cargo test
#
# コピー側には path 指定が残っていないため、fandhe-backend-* 依存はすべて
# crates.io 公開版（0.1.0）から解決される。公開版に存在しない API・feature を
# 参照していれば、ここで非 0 終了する。1 件の FAIL で即座に打ち切らず全クレートを
# 検証し、最後に集計して判定する。
# --------------------------------------------------
echo "== stage 3: crates.io 依存のみで cargo build / cargo test =="

pass_crates=()
fail_crates=()
skip_crates=()

for i in "${!crate_dirs[@]}"; do
    crate_dir="${crate_dirs[${i}]}"
    copy_dir="${copy_dirs[${i}]}"

    skip_marker="${crate_dir}/.standalone-crates-io-skip"
    if [ -f "${skip_marker}" ]; then
        # fail-closed ガード 3: 理由なしの SKIP は許さない（黙って穴を空けない）。
        skip_reason="$(tr -d '[:space:]' < "${skip_marker}")"
        if [ -z "${skip_reason}" ]; then
            echo "エラー: ${skip_marker} が空です。SKIP する理由を 1 行以上書いてください" >&2
            exit 1
        fi
        skip_reason_display="$(head -n 1 "${skip_marker}")"
        skip_crates+=("${crate_dir}（${skip_reason_display}）")
        echo "SKIP: ${crate_dir}（${skip_reason_display}）"
        continue
    fi

    echo "==> ${crate_dir}: cargo build"
    if (cd "${copy_dir}" && cargo build) \
        && { echo "==> ${crate_dir}: cargo test"; (cd "${copy_dir}" && cargo test); }; then
        pass_crates+=("${crate_dir}")
    else
        fail_crates+=("${crate_dir}")
        echo "FAIL: ${crate_dir}（crates.io 公開版のみでは build/test が通らない）" >&2
    fi
done

# --------------------------------------------------
# 集計（1 件でも FAIL があれば exit 1、フェイルクローズ）
# --------------------------------------------------
echo "== 集計: PASS ${#pass_crates[@]} / SKIP ${#skip_crates[@]} / FAIL ${#fail_crates[@]} / 全 ${#crate_dirs[@]} クレート =="
for crate_dir in "${pass_crates[@]}"; do
    echo "  PASS: ${crate_dir}"
done
for crate_dir in "${skip_crates[@]}"; do
    echo "  SKIP: ${crate_dir}"
done
for crate_dir in "${fail_crates[@]}"; do
    echo "  FAIL: ${crate_dir}"
done

if [ "${#fail_crates[@]}" -ne 0 ]; then
    echo "standalone-crates-io-check.sh: ${#fail_crates[@]} クレートが crates.io 公開版のみでビルド・テストできませんでした" >&2
    exit 1
fi

# fail-closed ガード 4: 全クレートが SKIP され PASS が 0 件なら、この検証は
# 実質何も検証していない（vacuous green）。ディレクトリ構成ドリフトの検出
# （ガード「検出 0 件」）と同じ理由で異常終了する。
if [ "${#pass_crates[@]}" -eq 0 ]; then
    echo "エラー: 全 ${#crate_dirs[@]} クレートが SKIP され、build/test が 1 件も実行されませんでした" >&2
    exit 1
fi

echo "standalone-crates-io-check.sh: 全クレートが crates.io 公開版のみでビルド・テストできました（SKIP ${#skip_crates[@]} 件を除く）"
