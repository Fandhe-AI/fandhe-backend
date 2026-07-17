#!/usr/bin/env bash
# openapi 2 段階ビルド検証（TASK-3.2、#31、docs/spec/05-tasks.md）。
#
# OpenAPI 定義（crates/plugin-openapi の ApiDoc）とサーバー本体の依存関係は
# 「gen-openapi CLI 実行 → openapi.json 生成 → openapi.json を静的埋め込みするクレート
# （bf-plugin-openapi）を含むサーバー本体ビルド」の 2 段階になる（TASK-3.2 受け入れ基準）。
# 本スクリプトはこの順序を CI（.github/workflows/ci.yml の openapi-two-stage ジョブ）・
# ローカルの双方から同一コマンドで再現するための薄いラッパーであり、cargo コマンドを
# 直列実行するのみでパースロジックを持たないため、他の scripts/*.sh と異なり
# scripts/tests/ 配下の専用セルフテストは設けない（本ファイル冒頭コメントで明記）。
#
# - stage 1: gen-openapi CLI をビルド・実行し、コミット済み crates/plugin-openapi/openapi.json
#   が ApiDoc の最新定義から生成した内容と一致するかを `--check` で検証する
#   （fail-closed。乖離時は非 0 終了、.claude/rules/security.md A08 対策）。
# - stage 2: ワークスペース全体を --all-features でビルドする。埋め込み済み openapi.json を
#   含む bf-plugin-openapi と、TASK-2.1（#18）マージ後に増える `openapi` feature 配線も
#   --all-features で自動的にカバーする。
#
# 使い方:
#   bash scripts/openapi-two-stage.sh            # stage 1（--check）+ stage 2（既定）
#   bash scripts/openapi-two-stage.sh --update    # stage 1 を --check ではなく --update で
#                                                  # 実行し、openapi.json を in-place 再生成
#                                                  # してから stage 2 を実行する（開発者向け）
set -euo pipefail

usage() {
  echo "usage: $0 [--update]" >&2
  echo "  （引数なし）: stage 1 を --check で検証してから stage 2 をビルドする（CI 既定）" >&2
  echo "  --update    : stage 1 で openapi.json を再生成してから stage 2 をビルドする（開発者向け）" >&2
}

gen_openapi_arg="--check"
case "${1:-}" in
  "") ;;
  --update)
    gen_openapi_arg="--update"
    ;;
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

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

echo "== stage 1: gen-openapi CLI（${gen_openapi_arg}） =="
cargo run -p bf-plugin-openapi --features gen-cli --bin gen-openapi -- "${gen_openapi_arg}"

echo "== stage 2: cargo build --workspace --all-features =="
cargo build --workspace --all-features
