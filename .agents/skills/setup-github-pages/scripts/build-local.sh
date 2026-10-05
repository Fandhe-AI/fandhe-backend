#!/usr/bin/env bash
# build-local.sh: docs サイトのビルド入口。ローカルと GitHub Actions（pages.yml）が同じ
# スクリプトを通るため、「CI でだけ壊れる」差分を作らない。
#
# 対象リポジトリでは tools/docs-site-gen/build-local.sh に置かれ、リポジトリのルートは
# このスクリプトの 2 階層上として解決する（呼び出し時のカレントディレクトリに依存しない）。
#
# 工程: FF_REV 検証 → 匿名 shallow fetch（_ff/） → [THIRD-PARTY-LICENSES 生成]
#       → check_site → registry 依存 0 件の検査 → wrapper build → 生成 → rebrand → 最小 verify・残存検査
#
# なぜ cargo install --git ではなく手動 fetch か: fandhe-frontend は submodule
# （docs/spec → private リポジトリ）を持ち、cargo の git 取得は submodule まで再帰するため
# 匿名環境では認証失敗になる。submodule を取らない shallow fetch + path 依存で回避する。
#
# 使い方: build-local.sh [--out DIR] [--clean] [--write-third-party]
#   --out DIR              出力先（既定 <root>/_site）。既存かつ非空ならエラー
#   --clean                既定の出力先 <root>/_site のみを削除してから生成する
#   --write-third-party    _ff/LICENSE-MIT から <root>/THIRD-PARTY-LICENSES を（再）生成する
# 環境変数: CARGO_HOME / RUSTUP_HOME は呼び出し側で上書きできる（隔離ビルド用）。
# 終了コード: 0 成功 / それ以外は失敗（どの工程かは stderr の "==> " 行で分かる）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
FF_DIR="${ROOT}/_ff"
FF_URL="https://github.com/Fandhe-AI/fandhe-frontend.git"
DEFAULT_OUT="${ROOT}/_site"

OUT="${DEFAULT_OUT}"
CLEAN=0
WRITE_THIRD_PARTY=0

usage() {
  sed -n '/^# 使い方/,/^# 終了コード/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)
      [[ $# -ge 2 ]] || { echo "エラー: --out には値が必要" >&2; usage; exit 2; }
      OUT="$2"; shift 2 ;;
    --clean) CLEAN=1; shift ;;
    --write-third-party) WRITE_THIRD_PARTY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "エラー: 未知の引数: $1" >&2; usage; exit 2 ;;
  esac
done

# 相対パスは呼び出し時のカレントディレクトリ基準で絶対化する
case "${OUT}" in
  /*) ;;
  *) OUT="${PWD}/${OUT}" ;;
esac

# >>> guards（tests/test_rebrand.py がこの区間を取り出して単体実行する。区間の目印を消さない）
# 不変条件: このスクリプトが書く・消す先（_ff/・target/・Cargo.lock・THIRD-PARTY-LICENSES・既定の
# 出力先 _site/）は、(a) 対象リポジトリの実体パス（ROOT_REAL）配下に解決され、(b) 末端自体が
# symlink でないこと。`git init` / `fetch` / `checkout` / `cargo build` / `mv` / `rm -r` は
# symlink を辿ってリンク先を書き換え得るため、書く前に必ずこの関数を通す。
# 唯一の例外は --out（CI は ${RUNNER_TEMP} など対象リポジトリ外へ出力する）で、guard_out が別途検査する。
#
# macOS 標準の realpath には -m が無いため、移植性のある python3 の os.path.realpath（存在しない
# パスも許容する非 strict 動作）で正規化する。
# python3 は、`-c` なら cwd、スクリプト起動ならそのスクリプトのディレクトリが sys.path の先頭に入る。対象
# リポジトリ（信頼できない場合がある）の .py（argparse.py 等の標準モジュール名）が標準ライブラリより先に
# import されないよう、すべての起動に `-I`（隔離モード: cwd・スクリプトのディレクトリ・PYTHONPATH・user site を
# 使わない）を付ける。`-B` は __pycache__ を作らない（置かれた .pyc の読み込みを避ける）。check_site.py /
# rebrand_site.py は自分で自ディレクトリを sys.path の末尾に足すので、_common は import できる。
canon() { python3 -I -B -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$1"; }

# 親ディレクトリだけを実体化し、末端の名前はそのまま残す。末端が symlink かの判定（-L）を
# 正規化後のパスに対して正しく行うため（`sub/../x` のように途中が未作成でも `-L` が誤って偽にならない）。
canon_leaf() {
  python3 -I -B -c '
import os, sys
p = sys.argv[1].rstrip("/") or "/"
b = os.path.basename(p)
print(os.path.realpath(p) if b in ("", ".", "..") else os.path.join(os.path.realpath(os.path.dirname(p)), b))
' "$1"
}

guard_path() {   # guard_path <path> <label>
  local p="$1" label="$2" real
  if [[ -L "${p}" ]]; then
    echo "エラー: ${label}（${p}）がシンボリックリンクのため、書き込み・削除をしない" >&2
    return 1
  fi
  real="$(canon "${p}")"
  case "${real}/" in
    "${ROOT_REAL}/"*) ;;
    *)
      echo "エラー: ${label}（${p}）が対象リポジトリの外（${real}）へ解決される。親ディレクトリが symlink の可能性" >&2
      return 1 ;;
  esac
}
# <<< guards

ROOT_REAL="$(canon "${ROOT}")"
OUT="$(canon_leaf "${OUT}")"
OUT_REAL="$(canon "${OUT}")"
DEFAULT_OUT_REAL="$(canon "${DEFAULT_OUT}")"

step() { echo "==> $*" >&2; }

# ---- FF_REV の検証（唯一の定義元は FF_REV ファイル。使用前に必ず 40 桁 hex か確認する）
step "FF_REV を検証"
FF_REV="$(tr -d '[:space:]' < "${SCRIPT_DIR}/FF_REV")"
if [[ ! "${FF_REV}" =~ ^[0-9a-f]{40}$ ]]; then
  echo "エラー: FF_REV が 40 桁の小文字 hex ではない" >&2
  exit 1
fi

# ---- 書き込み先の安全確認（symlink 経由で対象リポジトリの外へ書かない）
guard_path "${SCRIPT_DIR}" "tools/docs-site-gen" || exit 2
guard_path "${FF_DIR}" "_ff" || exit 2
guard_path "${SCRIPT_DIR}/target" "tools/docs-site-gen/target（cargo の出力先）" || exit 2
guard_path "${SCRIPT_DIR}/Cargo.lock" "tools/docs-site-gen/Cargo.lock（cargo が書く）" || exit 2
if [[ "${WRITE_THIRD_PARTY}" -eq 1 ]]; then
  guard_path "${ROOT}/THIRD-PARTY-LICENSES" "THIRD-PARTY-LICENSES" || exit 2
  if [[ -d "${ROOT}/THIRD-PARTY-LICENSES" ]]; then
    echo "エラー: THIRD-PARTY-LICENSES がディレクトリ（mv が配下へ移動してしまう）" >&2
    exit 2
  fi
fi

# ---- 出力先の安全確認
# --out は対象リポジトリの外でもよいが、末端が symlink なら拒否する（生成器・rm がリンク先を書き換えるため）。
# 対象リポジトリ内を指す場合は、実体も対象リポジトリ内に収まること（親ディレクトリ経由の脱出を防ぐ）。
if [[ -L "${OUT}" ]]; then
  echo "エラー: 出力先 ${OUT} がシンボリックリンク" >&2
  exit 2
fi
# 内外の判定は正規化後（OUT_REAL）で行う。生パスで判定すると `--out ../dist` のように実際は外へ
# 解決される指定を誤って「内」として扱い、外部出力を許す例外が効かなくなる。
case "${OUT_REAL}/" in
  "${ROOT_REAL}/"*) guard_path "${OUT}" "出力先" || exit 2 ;;
esac
if [[ "${CLEAN}" -eq 1 ]]; then
  # 削除してよいのは既定の出力先だけ（任意パスの再帰削除を許さない）
  if [[ "${OUT_REAL}" != "${DEFAULT_OUT_REAL}" ]]; then
    echo "エラー: --clean は既定の出力先（${DEFAULT_OUT}）でのみ使える" >&2
    exit 2
  fi
  if [[ -L "${DEFAULT_OUT}" ]]; then
    echo "エラー: ${DEFAULT_OUT} がシンボリックリンクのため削除しない" >&2
    exit 2
  fi
  if [[ -e "${DEFAULT_OUT}" ]]; then
    rm -r -- "${DEFAULT_OUT}"
  fi
fi
if [[ -e "${OUT}" ]] && [[ -n "$(ls -A "${OUT}" 2>/dev/null || true)" ]]; then
  echo "エラー: 出力先が既に存在し空ではない: ${OUT}（--clean か手動削除で空にする）" >&2
  exit 1
fi
case "${OUT_REAL}/" in
  "${ROOT_REAL}/site/"*)
    echo "エラー: 出力先を site/ 配下にできない（入力と混ざる）" >&2
    exit 2 ;;
esac
if [[ "${OUT_REAL}" == "${ROOT_REAL}" || "${ROOT_REAL}/" == "${OUT_REAL}/"* ]]; then
  echo "エラー: 出力先がリポジトリのルートまたはその上位ディレクトリになっている" >&2
  exit 2
fi

# ---- 匿名 shallow fetch（submodule は取らない）
# >>> fetch_ff（tests/test_rebrand.py がこの区間を取り出して単体実行する。区間の目印を消さない）
# _ff/ は生成器のキャッシュ専用ディレクトリで、利用者の作業物を置く場所ではない。それでも
# 手で編集された場合に備え、未コミット変更・未追跡ファイルがあれば**破棄せず中止**する
# （checkout -f / clean を使わない）。HEAD が FF_REV と違う clean な状態だけを checkout で進める。
fetch_ff() {
  if [[ -e "${FF_DIR}" && ! -d "${FF_DIR}/.git" && -n "$(ls -A "${FF_DIR}")" ]]; then
    echo "エラー: ${FF_DIR} が git 作業ツリーではない（内容を確認して手動で退避してから再実行）" >&2
    return 1
  fi
  if [[ -e "${FF_DIR}/.git" || -L "${FF_DIR}/.git" ]]; then
    if [[ -L "${FF_DIR}/.git" || ! -d "${FF_DIR}/.git" ]]; then
      echo "エラー: ${FF_DIR}/.git が通常のディレクトリではない（symlink・gitdir ファイルはリンク先を書き換え得るため中止）" >&2
      return 1
    fi
    local origin
    if origin="$(git -C "${FF_DIR}" remote get-url origin 2>/dev/null)" && [[ "${origin}" != "${FF_URL}" ]]; then
      echo "エラー: ${FF_DIR} の origin が期待する上流 URL と異なる（${origin}）。利用者の別リポジトリの可能性があるため書き換えず中止する。" >&2
      echo "       不要なら手動で ${FF_DIR} を削除してから再実行する。" >&2
      return 1
    fi
    local dirty
    if ! dirty="$(git -C "${FF_DIR}" status --porcelain)"; then
      echo "エラー: ${FF_DIR} の git 状態を取得できない（破損の可能性。内容を確認して手動で退避してから再実行）" >&2
      return 1
    fi
    if [[ -n "${dirty}" ]]; then
      echo "エラー: ${FF_DIR} に未コミットの変更または未追跡ファイルがある。破棄しないため中止する。" >&2
      echo "       必要な変更は退避し、不要なら手動で ${FF_DIR} を削除してから再実行する。" >&2
      return 1
    fi
    if [[ "$(git -C "${FF_DIR}" rev-parse HEAD 2>/dev/null || true)" == "${FF_REV}" ]]; then
      echo "  既存の _ff が FF_REV と一致するため再利用" >&2
      return 0
    fi
  else
    git init -q "${FF_DIR}"
  fi
  # origin が無い場合（初回の git init 直後）だけ追加する。既存の origin は上で一致確認済みで、書き換えない
  if ! git -C "${FF_DIR}" remote get-url origin >/dev/null 2>&1; then
    git -C "${FF_DIR}" remote add origin "${FF_URL}"
  fi
  # 認証プロンプトで止まらず失敗させる（CI・隔離環境での無限待機を避ける）
  GIT_TERMINAL_PROMPT=0 git -C "${FF_DIR}" fetch -q --depth 1 origin "${FF_REV}"
  git -C "${FF_DIR}" checkout -q FETCH_HEAD
  if [[ "$(git -C "${FF_DIR}" rev-parse HEAD)" != "${FF_REV}" ]]; then
    echo "エラー: checkout 後の HEAD が FF_REV と一致しない" >&2
    return 1
  fi
}
# <<< fetch_ff
step "fandhe-frontend ${FF_REV} を取得"
fetch_ff || exit 1

if [[ "${WRITE_THIRD_PARTY}" -eq 1 ]]; then
  step "THIRD-PARTY-LICENSES を生成"
  # 途中失敗で欠けたファイルを残さないよう、一時ファイルへ書いてから mv（同一ディレクトリ内で原子的に置換）
  TPL_TMP="$(mktemp "${ROOT_REAL}/.THIRD-PARTY-LICENSES.XXXXXX")"
  trap 'rm -f -- "${TPL_TMP}"' EXIT   # 失敗時に一時ファイルを残さない（成功時は mv 済みで no-op）
  {
    printf '%s\n' \
      "This repository's documentation site is generated with the docs-site generator of" \
      "fandhe-frontend (https://github.com/Fandhe-AI/fandhe-frontend, commit ${FF_REV})," \
      "which is licensed under MIT OR Apache-2.0. The MIT license text follows." \
      ""
    cat "${FF_DIR}/LICENSE-MIT"
  } > "${TPL_TMP}"
  chmod 0644 "${TPL_TMP}"   # mktemp は 0600 で作るため、通常ファイルと同じ権限へ
  # 宛先が symlink・ディレクトリでないことは冒頭で確認済み（mv は宛先が symlink なら symlink 自体を
  # 置換するが、ディレクトリを指す symlink やディレクトリだと配下へ移動してしまうため事前に拒否している）
  mv -f -- "${TPL_TMP}" "${ROOT_REAL}/THIRD-PARTY-LICENSES"
fi

# ---- 事前検証
step "check_site"
python3 -I -B "${SCRIPT_DIR}/check_site.py" --root "${ROOT}"

# ---- 依存検査: wrapper と上流の依存はすべて path 依存（source が null）であること。
# crates.io 等の registry 依存が混入すると、匿名・隔離ビルドの前提と供給網の固定方針が崩れる。
# FF_REV を更新した上流が外部 crate を導入した場合にここで fail-closed になる。
step "registry 依存が 0 件であることを検査"
# cargo metadata の失敗を set -e で拾うため、プロセス置換ではなくコマンド置換で受ける。
META="$(cd "${SCRIPT_DIR}" && cargo metadata --format-version 1 --offline --manifest-path "${SCRIPT_DIR}/Cargo.toml")"
printf '%s' "${META}" | python3 -I -B -c '
import json, sys
meta = json.load(sys.stdin)
ext = sorted("%s %s" % (p["name"], p["version"]) for p in meta["packages"] if p.get("source") is not None)
if ext:
    print("エラー: registry 依存が混入している（%d 件）: %s" % (len(ext), ", ".join(ext[:10])), file=sys.stderr)
    sys.exit(1)
print("registry 依存 0 件（packages=%d・すべて path 依存）" % len(meta["packages"]), file=sys.stderr)
'

# ---- wrapper build（CARGO_TARGET_DIR を固定し、cache 対象の tools/docs-site-gen/target と一致させる）
step "wrapper を build"
export CARGO_TARGET_DIR="${SCRIPT_DIR}/target"
cargo build --release --manifest-path "${SCRIPT_DIR}/Cargo.toml"

# ---- 生成（リンク検査は fail-closed。1 件でも壊れていれば何も書かず非 0）
step "サイトを生成"
"${CARGO_TARGET_DIR}/release/docs-site-gen" --root "${ROOT}" --out "${OUT}"

# ---- rebrand
step "rebrand"
python3 -I -B "${SCRIPT_DIR}/rebrand_site.py" --dist "${OUT}" --brand "${SCRIPT_DIR}/brand.toml"

# ---- 最小 verify（空サイト・アセット欠落を黙って公開しない。-s で 0 バイトも検出）
step "verify"
for f in index.html 404.html assets/site.css assets/site.js assets/search-index.json; do
  test -s "${OUT}/${f}" || { echo "エラー: ${f} が無い、または空" >&2; exit 1; }
done
python3 -I -B "${SCRIPT_DIR}/rebrand_site.py" --dist "${OUT}" --brand "${SCRIPT_DIR}/brand.toml" --verify-only

step "完了: ${OUT}"
