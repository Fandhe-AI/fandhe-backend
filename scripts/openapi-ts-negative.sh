#!/usr/bin/env bash
# openapi-typescript 連携パイプラインの陰性対照（negative control）検証
# （TASK-6.2、#55、docs/spec/05-tasks.md、REQ-6）。
#
# TASK-6.1（#54）で確立した `scripts/openapi-ts.sh` は「`tsc --noEmit` が
# 成功すること」だけを検証する。しかし成功するだけでは、生成型が `any` 混入等で
# 実質的な制約を失っていても見かけ上は通ってしまう可能性が残る
# （PoC-8、docs/spec/03-poc/trpc-contract/README.md で有効性が実証された
# 「陰性対照」の考え方）。本スクリプトは「意図的な型不一致が `tsc --noEmit` の
# エラーとして確実に検出されること」を CI（.github/workflows/ci.yml の
# openapi-ts ジョブ）・ローカルの双方で常設検証する。
#
# 判定は 2 段構成:
#   - N1: TS 側陰性対照。`ts/src/negative/type-mismatch.ts`（4 類型: パスパラメータ
#     型不一致・レスポンス型誤代入・存在しないエンドポイント呼び出し・リクエスト
#     body 型不一致）を `tsc --noEmit -p tsconfig.negative.json`
#     （`npm run typecheck:negative`）にかけ、非 0 終了 **かつ** 各類型の期待
#     TS エラーコード（TS2322/TS2554）が出力に含まれることを確認する。
#   - N2: スキーマ側陰性対照。`crates/plugin-openapi/openapi.json` の一時コピーへ
#     `/users/{id}` の `id` を integer→string へ変更する node ワンライナーで
#     型不一致を注入し、一時ディレクトリへ `schema.d.ts` を再生成した上で、
#     既存（無改変）の `ts/src/usage.ts` の型検査が失敗することを確認する
#     （openapi.json 境界からの伝搬確認。Rust 側 utoipa 属性の変更が型再生成のみで
#     TypeScript 側に伝わることの縮小版であり、Rust 定義そのものの一時変更を伴う
#     完全な伝搬確認は `scripts/accept/openapi-ts-accept.sh` 基準 C が受け持つ）。
#
# fail-closed 判定（.claude/rules/security.md A08）: 「非 0 終了」だけでは
# tsconfig 不備・ファイル欠落等の環境破損による失敗を陰性対照 PASS と誤認しうる
# ため、以下の 3 条件をすべて満たした場合のみ PASS とする。
#   1. 陽性対照（`npm run typecheck`）が同一実行内で成功する
#   2. N1: `typecheck:negative` が非 0 終了し、4 類型すべての期待エラーコードが
#      出力に含まれる
#   3. N2: 一時 schema.d.ts 再生成後の `tsc --noEmit` が非 0 終了し、期待エラー
#      コード（TS2322）が出力に含まれる
#
# 前提ツール: node（>=24）・npm。未導入の場合は自動ダウンロードせず、導入コマンドを
# 案内して非 0 終了する（scripts/openapi-ts.sh と同型、scripts/README.md 既存規約）。
#
# 使い方:
#   bash scripts/openapi-ts-negative.sh
set -euo pipefail

usage() {
  echo "usage: $0" >&2
  echo "  陰性対照（意図的な型不一致が tsc --noEmit のエラーとして検出されること）を検証する" >&2
  echo "  引数は取らない" >&2
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

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

# 前提ツール検査（node / npm）。導入されていない場合は volta 経由の導入コマンドを
# 案内して非 0 終了する（自動ダウンロードしない、scripts/openapi-ts.sh と同型）。
if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
  echo "エラー: node / npm が見つかりません。" >&2
  echo "  ts/package.json の volta フィールド（node 24.13.0・npm 11.6.2）に合わせて導入してください:" >&2
  echo "    curl https://get.volta.sh | bash" >&2
  echo "    volta install node@24.13.0 npm@11.6.2" >&2
  exit 1
fi

echo "== npm ci --ignore-scripts（ts/ 依存の再現） =="
(cd ts && npm ci --ignore-scripts)

# ---------------------------------------------------------------------------
# 前提条件: 陽性対照（既存の 5 エンドポイント呼び出しが tsc --noEmit を通ること）
# が同一実行内で成功していること。これが失敗した状態では「全部壊れていて失敗
# した」を陰性対照 PASS と誤認するリスクがあるため、先に確認する。
# ---------------------------------------------------------------------------
echo "== 陽性対照: npm run typecheck =="
(cd ts && npm run --silent typecheck)
echo "陽性対照 OK（tsc --noEmit が成功）。"

# ---------------------------------------------------------------------------
# N1: TS 側陰性対照。tsc --noEmit -p tsconfig.negative.json が非 0 終了し、
# 4 類型すべての期待エラーコードを含むことを検証する。
# ---------------------------------------------------------------------------
echo ""
echo "== N1: npm run typecheck:negative（意図的な型不一致が検出されること） =="
n1_output=""
n1_status=0
n1_output="$(cd ts && npm run --silent typecheck:negative 2>&1)" || n1_status=$?

if [ "${n1_status}" -eq 0 ]; then
  echo "エラー: typecheck:negative が exit 0 で成功しました（意図的な型不一致が検出されませんでした）。" >&2
  echo "  生成型が実質的な制約として機能していない可能性があります。" >&2
  echo "${n1_output}" >&2
  exit 1
fi

n1_missing=0
# 4 類型（パスパラメータ型不一致・レスポンス型誤代入・存在しないエンドポイント
# 呼び出し・リクエスト body 型不一致）それぞれについて、期待エラーコードが
# 出力中に最低 1 回現れることを個別に確認する（単純な件数一致ではなく類型ごとの
# 存在確認とし、行番号がずれても壊れないようにする）。
n1_categories=(
  "type-mismatch.ts(34,"
  "type-mismatch.ts(46,"
  "type-mismatch.ts(62,"
  "type-mismatch.ts(73,"
)
n1_expected_codes=(
  "TS2322"
  "TS2322"
  "TS2554"
  "TS2322"
)
for i in "${!n1_categories[@]}"; do
  marker="${n1_categories[$i]}"
  expected="${n1_expected_codes[$i]}"
  if ! printf '%s\n' "${n1_output}" | grep -qF "${marker}"; then
    echo "エラー: N1 期待箇所 ${marker} のエラー行が出力に見つかりません（行番号がずれた可能性）。" >&2
    n1_missing=1
    continue
  fi
  if ! printf '%s\n' "${n1_output}" | grep -F "${marker}" | grep -q "${expected}"; then
    echo "エラー: N1 期待箇所 ${marker} の期待エラーコード ${expected} が見つかりません。" >&2
    n1_missing=1
  fi
done

if [ "${n1_missing}" -ne 0 ]; then
  echo "" >&2
  echo "== typecheck:negative 実際の出力 ==" >&2
  echo "${n1_output}" >&2
  exit 1
fi
echo "N1 OK（非 0 終了 + 4 類型すべての期待エラーコードを確認）。"
echo "${n1_output}"

# ---------------------------------------------------------------------------
# N2: スキーマ側陰性対照。openapi.json の一時コピーへ /users/{id} の id を
# integer→string へ変更し、一時ディレクトリへ schema.d.ts を再生成した上で、
# 既存（無改変）の ts/src/usage.ts の型検査が失敗することを確認する。
# ---------------------------------------------------------------------------
echo ""
echo "== N2: openapi.json 境界からの型不一致伝搬（一時ディレクトリ、既存ファイルは変更しない） =="

tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

mkdir -p "${tmp_dir}/src/generated"
cp ts/src/client.ts ts/src/usage.ts "${tmp_dir}/src/"
# node_modules はコピーせずシンボリックリンクで参照する
# （npm ci をもう一度走らせない。ts/node_modules は上の npm ci で再現済み）。
ln -s "${repo_root}/ts/node_modules" "${tmp_dir}/node_modules"

# openapi.json の一時コピーへ id: integer → string の型不一致を注入する
# （固定の node ワンライナーのみで完結させ、外部入力を受けない、
# .claude/rules/security.md インジェクション対策）。
node -e '
const fs = require("fs");
const srcPath = process.argv[1];
const dstPath = process.argv[2];
const doc = JSON.parse(fs.readFileSync(srcPath, "utf8"));
const param = doc.paths["/users/{id}"].get.parameters[0];
if (param.name !== "id") {
  throw new Error("N2 前提崩れ: /users/{id} の第 1 パラメータが id ではありません（openapi.json の構造変更の可能性）");
}
param.schema.type = "string";
delete param.schema.format;
delete param.schema.minimum;
fs.writeFileSync(dstPath, JSON.stringify(doc));
' "${repo_root}/crates/plugin-openapi/openapi.json" "${tmp_dir}/openapi.mutated.json"

"${tmp_dir}/node_modules/.bin/openapi-typescript" "${tmp_dir}/openapi.mutated.json" -o "${tmp_dir}/src/generated/schema.d.ts" >/dev/null

cat >"${tmp_dir}/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ES2022",
    "moduleResolution": "Bundler",
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true,
    "esModuleInterop": true,
    "forceConsistentCasingInFileNames": true
  },
  "include": ["src/**/*.ts"]
}
EOF

n2_output=""
n2_status=0
n2_output="$("${tmp_dir}/node_modules/.bin/tsc" --noEmit -p "${tmp_dir}/tsconfig.json" 2>&1)" || n2_status=$?

if [ "${n2_status}" -eq 0 ]; then
  echo "エラー: openapi.json の型不一致注入後も tsc --noEmit が exit 0 で成功しました。" >&2
  echo "  openapi.json → schema.d.ts → tsc の伝搬が機能していない可能性があります。" >&2
  exit 1
fi
if ! printf '%s\n' "${n2_output}" | grep -q "TS2322"; then
  echo "エラー: N2 で期待した TS2322 が出力に見つかりません（意図しない別要因での失敗の可能性）。" >&2
  echo "${n2_output}" >&2
  exit 1
fi
echo "N2 OK（非 0 終了 + 期待エラーコード TS2322 を確認、openapi.json → schema.d.ts → tsc の伝搬を検証）。"
echo "${n2_output}"

echo ""
echo "== openapi-ts-negative.sh 完了（N1/N2 とも意図した型不一致を検出、陽性対照は成功） =="
