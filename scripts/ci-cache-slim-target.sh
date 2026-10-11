#!/usr/bin/env bash
# CI のビルドキャッシュ保存前に target ディレクトリからワークスペース自身の成果物を
# 取り除き、保存サイズを第三者依存クレートの成果物だけに絞る。
#
# 背景（.claude/rules/ci.md「キャッシュ方針」）: actions/checkout はファイルの mtime を
# チェックアウト時刻にするため、ワークスペースのクレート（lib・テスト・example・bin）は
# キャッシュから復元しても毎回再ビルドされる。保存しても再利用されないこれらの成果物が
# target の 7 割強（ローカル実測で 4.2 GB 中 3.2 GB）を占め、リポジトリのキャッシュ上限を
# 圧迫して小さなツールキャッシュまで LRU で追い出していた。
#
# 削除対象（各 `deps/` ディレクトリを持つプロファイルディレクトリごと）:
#   - deps/(lib)<target 名>-<16hex>[.*]   ワークスペースのターゲット（lib/test/example/bin）
#   - .fingerprint/<パッケージ名>-* / build/<パッケージ名>-*
#   - examples/ / incremental/              （example はワークスペース由来のみ）
#   - プロファイル直下の <target 名>[.*]     （bin の実体・.d・.pdb 等）
#   - TARGET_DIR/doc                        （`cargo doc --no-deps` の出力。毎回再生成される）
# 第三者依存の成果物は残す（名前が偶然一致した場合は次回再ビルドされるだけで、正当性には
# 影響しない。cargo は成果物の欠落を検知して再ビルドする）。
#
# ワークスペースの名前は `cargo metadata` から得る（詳細は metadata_json のコメント）。fuzz / tests-e2e / microbench の
# ような standalone workspace は `--manifest-path` を追加で渡す（ルート workspace の名前は
# 常に含める。standalone 側が path 依存でルートのクレートを取り込むため）。
#
# キャッシュは最適化であり、本スクリプトの失敗は保存サイズが大きくなるだけ（ci.yml 側は
# continue-on-error で保存を続行する）。
#
# 使い方:
#   bash scripts/ci-cache-slim-target.sh [--manifest-path PATH]... [--metadata-file FILE]... TARGET_DIR...
#     --manifest-path PATH  追加で名前を集める Cargo.toml（ルートの Cargo.toml は常に含む）
#     --metadata-file FILE  cargo metadata の代わりに読む JSON（セルフテスト用。指定時は
#                           cargo metadata を呼ばない）
#     --dry-run             削除対象を表示するだけで消さない
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

manifests=("${REPO_ROOT}/Cargo.toml")
metadata_files=()
targets=()
dry_run=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --manifest-path)
            [ "$#" -ge 2 ] || { echo "エラー: --manifest-path に値がありません" >&2; exit 2; }
            manifests+=("$2")
            shift 2
            ;;
        --metadata-file)
            [ "$#" -ge 2 ] || { echo "エラー: --metadata-file に値がありません" >&2; exit 2; }
            metadata_files+=("$2")
            shift 2
            ;;
        --dry-run)
            dry_run=1
            shift
            ;;
        -h | --help)
            sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        -*)
            echo "エラー: 不明なオプション: $1" >&2
            exit 2
            ;;
        *)
            targets+=("$1")
            shift
            ;;
    esac
done

if [ "${#targets[@]}" -eq 0 ]; then
    echo "エラー: TARGET_DIR を 1 つ以上指定してください" >&2
    exit 2
fi

command -v jq >/dev/null 2>&1 || { echo "エラー: jq が見つかりません" >&2; exit 2; }

# パッケージ名（.fingerprint/build 用、ハイフンのまま）とターゲット名（成果物用、
# ハイフンをアンダースコアへ正規化）を集める。依存グラフ込みの metadata（ビルド直後なので
# Cargo.lock と registry が揃っており --offline で解決できる）を優先し、`source` が null の
# パッケージ（ワークスペース・path 依存）を削除対象、それ以外（crates.io 等の第三者依存）と
# 名前が衝突するもの（例: テスト名 interceptor と webrtc 依存の interceptor クレート）を
# 削除対象から外す。依存込みで解決できない場合は --no-deps へフォールバックする（衝突分は
# 次回再ビルドされるだけ）。
metadata_json() {
    if [ "${#metadata_files[@]}" -gt 0 ]; then
        cat "${metadata_files[@]}"
    else
        local m host
        # --filter-platform: 他プラットフォーム専用の依存（未ダウンロード）で --offline 解決が
        # 失敗しないよう、ホストで実際にビルドされる依存グラフに絞る。
        host="$(rustc -vV | sed -n 's/^host: //p' | tr -d '\r')"
        for m in "${manifests[@]}"; do
            cargo metadata --format-version 1 --all-features --offline --filter-platform "${host}" \
                --manifest-path "${m}" 2>/dev/null \
                || cargo metadata --no-deps --format-version 1 --manifest-path "${m}"
        done
    fi
}

# Windows（Git Bash）の jq は CRLF で出力するため \r を落とす。sort/comm の照合順は C で揃える。
METADATA="$(metadata_json | tr -d '\r')"
# macOS ランナーの /bin/bash は 3.2 で mapfile・連想配列が無いため、名前集合は改行区切りの
# 文字列（前後に改行を付ける）で持ち、in_set で部分一致判定する。
PKG_SET=$'\n'"$(jq -r '.packages[] | select(.source == null) | .name' <<<"${METADATA}" | tr -d '\r' | LC_ALL=C sort -u)"$'\n'
TARGET_SET=$'\n'"$(
    LC_ALL=C comm -23 \
        <(jq -r '.packages[] | select(.source == null) | .name, .targets[].name' <<<"${METADATA}" \
            | tr -d '\r' | tr '-' '_' | LC_ALL=C sort -u) \
        <(jq -r '.packages[] | select(.source != null)
            | .name, (.targets[] | select(.kind | any(. == "test" or . == "example" or . == "bench") | not) | .name)' \
            <<<"${METADATA}" | tr -d '\r' | tr '-' '_' | LC_ALL=C sort -u)
)"$'\n'

if [ -z "${TARGET_SET//$'\n'/}" ]; then
    echo "エラー: ワークスペースのターゲット名を 1 件も取得できませんでした" >&2
    exit 1
fi

# $2 が集合 $1 に含まれるか（右辺はクォートしてグロブ文字を文字どおりに扱う）
in_set() {
    [ -n "$2" ] && [[ "$1" == *$'\n'"$2"$'\n'* ]]
}

removed=0
remove() {
    removed=$((removed + 1))
    if [ "${dry_run}" -eq 1 ]; then
        echo "  (dry-run) rm -rf $1"
    else
        rm -rf -- "$1"
    fi
}

size_kb() {
    du -sk "$1" 2>/dev/null | cut -f1 || echo 0
}

# 成果物名から <name> を取り出して STEM に入れる: (lib)<name>-<16hex>[.*] → <name>。
# Windows（Git Bash）ではプロセス生成が遅いため、コマンド置換を使わずグローバル変数で返す。
STEM=""
artifact_stem() {
    local base="${1#lib}"
    STEM=""
    if [[ "${base}" =~ ^(.+)-[0-9a-f]{16}(\..*)?$ ]]; then
        STEM="${BASH_REMATCH[1]}"
    fi
}

# .fingerprint/build のディレクトリ名 <pkg>-<16hex> → <pkg>（STEM に入れる）
dir_pkg() {
    STEM=""
    if [[ "$1" =~ ^(.+)-[0-9a-f]{16}$ ]]; then
        STEM="${BASH_REMATCH[1]}"
    fi
}

slim_profile_dir() {
    local profile="$1" entry base stem
    [ -d "${profile}/incremental" ] && remove "${profile}/incremental"
    [ -d "${profile}/examples" ] && remove "${profile}/examples"

    for entry in "${profile}/deps"/*; do
        [ -e "${entry}" ] || continue
        artifact_stem "${entry##*/}"
        if in_set "${TARGET_SET}" "${STEM}"; then
            remove "${entry}"
        fi
    done

    local sub
    for sub in .fingerprint build; do
        [ -d "${profile}/${sub}" ] || continue
        for entry in "${profile}/${sub}"/*; do
            [ -e "${entry}" ] || continue
            dir_pkg "${entry##*/}"
            if in_set "${PKG_SET}" "${STEM}"; then
                remove "${entry}"
            fi
        done
    done

    # プロファイル直下のファイル（bin の実体・.d・.pdb・.exe）。ディレクトリは触らない。
    for entry in "${profile}"/*; do
        [ -f "${entry}" ] || continue
        base="${entry##*/}"
        stem="${base%%.*}"
        stem="${stem//-/_}"
        if in_set "${TARGET_SET}" "${stem}"; then
            remove "${entry}"
        fi
    done
}

for target_dir in "${targets[@]}"; do
    if [ ! -d "${target_dir}" ]; then
        echo "==> ${target_dir}: 存在しないためスキップ"
        continue
    fi
    before="$(size_kb "${target_dir}")"
    [ -d "${target_dir}/doc" ] && remove "${target_dir}/doc"
    # deps/ を持つディレクトリ = プロファイルディレクトリ（target/debug、
    # target/<triple>/release、target/llvm-cov-target/debug 等）。
    while IFS= read -r deps_dir; do
        slim_profile_dir "$(dirname "${deps_dir}")"
    done < <(find "${target_dir}" -maxdepth 4 -type d -name deps)
    after="$(size_kb "${target_dir}")"
    echo "==> ${target_dir}: $((before / 1024)) MB -> $((after / 1024)) MB"
done

echo "==> ci-cache-slim-target.sh: ${removed} 件を削除$([ "${dry_run}" -eq 1 ] && echo '（dry-run）')"
