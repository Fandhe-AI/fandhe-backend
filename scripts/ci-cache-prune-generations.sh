#!/usr/bin/env bash
# main ブランチスコープの Actions キャッシュから、ビルドキャッシュの旧世代を削除する。
#
# 背景（.claude/rules/ci.md「キャッシュ方針」）: ビルドキャッシュのキーは
# `<family>-<OS>[-v<N>]-<toolchain>-<依存ハッシュ>` で、Cargo.toml の変更や toolchain の
# 更新のたびに新しい世代が main に保存される。旧世代は誰も復元しない（PR・main とも最新世代に
# 一致するか restore-keys で最新を引く）のに、7 日間アクセスが無くなるまで残り、容量
# （上限 10 GB、Org の Actions 予算 $0 のため引き上げ不可）を占有する。溢れると LRU で数 MB のツール
# キャッシュ（cargo-tool-*）まで追い出され、ツールのソースビルド（最大 7 分強）に戻る。
#
# 判定: 末尾が `-<英数字>-<64hex>` のキーを `<family>-<OS>` と版 `v<N>`（無ければ v1）に
# 分解し、同じ `<family>-<OS>` の中で「最大の版のうち created_at が最新の 1 件」だけを残す。
# 末尾が 64hex でないキー（cargo-tool-<tool>-<version>-<OS>-<arch> 等）は対象外。
# main 以外のスコープ（PR）は対象外（PR からは保存しない方針のため増えない）。
#
# 使い方:
#   bash scripts/ci-cache-prune-generations.sh [--repo OWNER/REPO] [--input FILE] [--dry-run]
#     --repo OWNER/REPO  対象リポジトリ（既定: $GITHUB_REPOSITORY、無ければ gh の現在リポジトリ）
#     --input FILE       gh api の代わりに読むキャッシュ一覧 JSON（`.actions_caches[]` 形式、
#                        セルフテスト用。指定時は削除 API も呼ばず対象を表示するだけ）
#     --dry-run          削除対象を表示するだけで消さない
set -euo pipefail

repo="${GITHUB_REPOSITORY:-}"
input=""
dry_run=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --repo)
            [ "$#" -ge 2 ] || { echo "エラー: --repo に値がありません" >&2; exit 2; }
            repo="$2"
            shift 2
            ;;
        --input)
            [ "$#" -ge 2 ] || { echo "エラー: --input に値がありません" >&2; exit 2; }
            input="$2"
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
        *)
            echo "エラー: 不明な引数: $1" >&2
            exit 2
            ;;
    esac
done

command -v jq >/dev/null 2>&1 || { echo "エラー: jq が見つかりません" >&2; exit 2; }

if [ -n "${input}" ]; then
    dry_run=1
    caches_json="$(jq -c '[.actions_caches[]]' "${input}")"
else
    command -v gh >/dev/null 2>&1 || { echo "エラー: gh が見つかりません" >&2; exit 2; }
    if [ -z "${repo}" ]; then
        repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
    fi
    caches_json="$(gh api --paginate "repos/${repo}/actions/caches?per_page=100&ref=refs/heads/main" \
        --jq '.actions_caches[]' | jq -sc '.')"
fi

# 削除対象の id と key を "id<TAB>key<TAB>size" で出す。
mapfile -t victims < <(jq -r '
    [ .[]
      | select(.ref == "refs/heads/main")
      | . as $c
      | ($c.key | capture("^(?<group>.+)-[0-9A-Za-z]+-[0-9a-f]{64}$")?) as $m
      | select($m != null)
      | ($m.group | capture("^(?<base>.+)-v(?<ver>[0-9]+)$")? // {base: $m.group, ver: "1"}) as $g
      | $c + {base: $g.base, ver: ($g.ver | tonumber)}
    ]
    | group_by(.base)[]
    | sort_by([.ver, .created_at]) | reverse
    | .[1:][]
    | "\(.id)\t\(.key)\t\(.size_in_bytes)"
' <<<"${caches_json}")

if [ "${#victims[@]}" -eq 0 ]; then
    echo "==> 削除対象の旧世代キャッシュはありません"
    exit 0
fi

total=0
failed=0
for line in "${victims[@]}"; do
    IFS=$'\t' read -r id key size <<<"${line}"
    total=$((total + size))
    if [ "${dry_run}" -eq 1 ]; then
        echo "  (dry-run) delete ${key} ($((size / 1048576)) MB)"
    elif gh api -X DELETE "repos/${repo}/actions/caches/${id}" >/dev/null; then
        echo "  deleted ${key} ($((size / 1048576)) MB)"
    else
        # 並行する別 run が先に消した等。容量最適化のため失敗は警告に留める。
        echo "::warning::キャッシュ ${key}（id ${id}）の削除に失敗しました"
        failed=$((failed + 1))
    fi
done

echo "==> 旧世代 ${#victims[@]} 件（$((total / 1048576)) MB）$([ "${dry_run}" -eq 1 ] && echo 'を検出（dry-run）' || echo "を削除（失敗 ${failed} 件）")"
