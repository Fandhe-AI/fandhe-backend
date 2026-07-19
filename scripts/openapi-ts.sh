#!/usr/bin/env bash
# openapi-typescript 連携パイプライン検証（TASK-6.1、#54、docs/spec/05-tasks.md、REQ-6）。
#
# 「utoipa 属性 → openapi.json → openapi-typescript → TS 型 → openapi-fetch クライアント」
# という一方向パイプラインを CI（.github/workflows/ci.yml の openapi-ts ジョブ）・ローカルの
# 双方から同一コマンドで再現するための薄いラッパー。scripts/openapi-two-stage.sh（TASK-3.2、
# #31）の後段に位置し、openapi.json 自体の鮮度は本スクリプトの前提として stage 1 で再検証する
# （openapi-two-stage.sh の CI ジョブと重複するが、本スクリプト単体でも fail-closed に完結させる
# ため意図的に重複させている）。詳細設計は docs/design/openapi-typescript-pipeline.md 参照。
#
# - stage 1: gen-openapi CLI を `--check` で実行し、コミット済み
#   crates/plugin-openapi/openapi.json が ApiDoc の最新定義と一致するかを検証する
#   （fail-closed。乖離時は非 0 終了、.claude/rules/security.md A08 対策）。
# - stage 2: `npm ci --ignore-scripts`（lifecycle script による任意コード実行を遮断、
#   .claude/rules/security.md A06/A08）で ts/ の依存を再現し、openapi-typescript で
#   一時ディレクトリへ schema.d.ts を再生成してコミット済みのものと diff する
#   （`--check`、乖離時は差分を表示して非 0 終了）。`--update` 指定時は in-place 再生成する。
# - stage 3: `tsc --noEmit` で ts/src 配下（client.ts・usage.ts 等）の型検査を行う。
#
# 前提ツール: node（>=24）・npm。未導入の場合は自動ダウンロードせず、導入コマンドを
# 案内して非 0 終了する（scripts/README.md の既存規約、前提ツールを自動ダウンロードしない）。
#
# 使い方:
#   bash scripts/openapi-ts.sh            # stage 1〜3 を --check で検証する（CI 既定）
#   bash scripts/openapi-ts.sh --update    # stage 1・stage 2 を再生成モードで実行してから
#                                          # stage 3 を検証する（開発者向け）
set -euo pipefail

usage() {
  echo "usage: $0 [--update]" >&2
  echo "  （引数なし）: openapi.json・schema.d.ts の鮮度を --check で検証し tsc --noEmit を実行する（CI 既定）" >&2
  echo "  --update    : openapi.json・schema.d.ts を再生成してから tsc --noEmit を実行する（開発者向け）" >&2
}

mode="check"
case "${1:-}" in
  "") ;;
  --update)
    mode="update"
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

# 前提ツール検査（node / npm）。導入されていない場合は volta 経由の導入コマンドを
# 案内して非 0 終了する（自動ダウンロードしない、scripts/README.md 既存規約）。
if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
  echo "エラー: node / npm が見つかりません。" >&2
  echo "  ts/package.json の volta フィールド（node 24.13.0・npm 11.6.2）に合わせて導入してください:" >&2
  echo "    curl https://get.volta.sh | bash" >&2
  echo "    volta install node@24.13.0 npm@11.6.2" >&2
  exit 1
fi

gen_openapi_arg="--check"
if [ "${mode}" = "update" ]; then
  gen_openapi_arg="--update"
fi

echo "== stage 1: gen-openapi CLI（${gen_openapi_arg}） =="
cargo run -p fandhe-backend-plugin-openapi --features gen-cli --bin gen-openapi -- "${gen_openapi_arg}"

echo "== stage 2: npm ci --ignore-scripts（ts/ 依存の再現） =="
(cd ts && npm ci --ignore-scripts)

schema_path="ts/src/generated/schema.d.ts"

if [ "${mode}" = "update" ]; then
  echo "== stage 2: openapi-typescript（schema.d.ts を in-place 再生成） =="
  (cd ts && npm run --silent gen:types)
else
  echo "== stage 2: openapi-typescript（一時ディレクトリへ再生成し diff で鮮度検証） =="
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "${tmp_dir}"' EXIT
  (cd ts && npx --no-install openapi-typescript ../crates/plugin-openapi/openapi.json -o "${tmp_dir}/schema.d.ts")
  if ! diff -u "${schema_path}" "${tmp_dir}/schema.d.ts"; then
    echo "" >&2
    echo "エラー: ${schema_path} が crates/plugin-openapi/openapi.json と乖離しています。" >&2
    echo "  bash scripts/openapi-ts.sh --update を実行して再生成し、差分をコミットしてください。" >&2
    exit 1
  fi
fi

echo "== stage 3: tsc --noEmit（型検査） =="
(cd ts && npm run --silent typecheck)

echo "== openapi-ts.sh 完了（mode=${mode}） =="
