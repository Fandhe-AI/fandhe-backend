#!/usr/bin/env bash
# scripts/docs-site-visual.sh
#
# 役割:
#   docs サイト刷新（イシュー #384 系列、docs/design/docs-site-redesign.md）の
#   実ブラウザ描画を headless chromium で撮影し、docs/acceptance/ 配下の受け入れ
#   レポート（イシュー #399）が参照する視覚証跡一式（PNG + manifest.tsv）を
#   生成する。fandhe-frontend の tools/docs-site/visual-regression.sh からの
#   移植・改変（base_path・ビルドコマンド・撮影マトリクスを本リポ向けに変更）。
#
# 呼び出し元:
#   人手（レビュー時の目視確認）または docs/acceptance/issue399-docs-site-visual.md
#   の「再現手順」節から。CI では実行しない（chromium 常設を self-hosted runner に
#   前提できないため、.claude/rules/ci.md の対象外）。
#
# fail-closed 方針:
#   - ビルド直後に「刷新後サイト」であることをプリフライトで検証し、stale な
#     出力（3 カラム化前・テーマ JS 前・検索索引前のツリー）から誤った証跡一式が
#     生成されるのを防ぐ。
#   - 撮影失敗（chromium の無音失敗を含む）・枚数/容量バジェット超過は非 0 終了。
set -euo pipefail

# ── 定数 ─────────────────────────────────────────────────────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly REPO_ROOT
readonly BASE_PATH="/fandhe-backend"
readonly MAX_SHOTS=28
readonly MAX_TOTAL_BYTES=$((3670016)) # 3.5MiB
readonly PORT_ATTEMPTS=50

OUT_DIR="${DOCS_SITE_SHOTS_DIR:-$HOME/fandhe-backend-docs-site-visual/$(date -u +%Y%m%dT%H%M%SZ)}"

# ── 前提ツール検査 ───────────────────────────────────────────────────
CHROMIUM_BIN=""
for cand in chromium chromium-browser google-chrome; do
  if command -v "$cand" >/dev/null 2>&1; then
    CHROMIUM_BIN="$(command -v "$cand")"
    break
  fi
done
if [ -z "$CHROMIUM_BIN" ]; then
  echo "error: chromium (or chromium-browser) not found in PATH" >&2
  exit 1
fi
for cand in python3 cargo ss; do
  if ! command -v "$cand" >/dev/null 2>&1; then
    echo "error: required tool not found in PATH: $cand" >&2
    exit 1
  fi
done

# ── 出力先の絶対パス・非ドット要素検証 ──────────────────────────────
# worktree 相対の既定出力先は snap の AppArmor により chromium の書き込みが
# 無音で失敗しうる（実装計画 §ステップ1）。絶対パス・ドット始まりセグメント
# なしを要求してこれを未然に検知する。
case "$OUT_DIR" in
  /*) ;;
  *)
    echo "error: DOCS_SITE_SHOTS_DIR must be an absolute path: $OUT_DIR" >&2
    exit 1
    ;;
esac
IFS='/' read -r -a __out_parts <<<"$OUT_DIR"
for part in "${__out_parts[@]}"; do
  case "$part" in
    .*)
      if [ -n "$part" ]; then
        echo "error: output path must not contain dot-leading segments: $OUT_DIR" >&2
        exit 1
      fi
      ;;
  esac
done

SHOTS_DIR="$OUT_DIR/shots"
LOGS_DIR="$OUT_DIR/logs"
SERVE_LIGHT="$OUT_DIR/serve-light"
SERVE_DARK="$OUT_DIR/serve-dark"
SERVE_NOJS="$OUT_DIR/serve-nojs"
MANIFEST="$OUT_DIR/manifest.tsv"
DIST="$OUT_DIR/dist"

mkdir -p "$SHOTS_DIR" "$LOGS_DIR"
printf 'file\turl\twidth\theight\ttheme\tjs\tbytes\tsha256\n' >"$MANIFEST"

SERVER_PIDS=()
cleanup() {
  for pid in "${SERVER_PIDS[@]:-}"; do
    kill "$pid" >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT

# ── ステップ 1: サイトビルド ─────────────────────────────────────────
echo "==> building docs site (cargo run -p fandhe-backend-docs-site)"
( cd "$REPO_ROOT" && cargo run -p fandhe-backend-docs-site -- --out "$DIST" ) \
  >"$LOGS_DIR/build.log" 2>&1 || {
  echo "error: docs-site build failed (see $LOGS_DIR/build.log)" >&2
  exit 1
}

# ── stale ツリー検知（fail-closed プリフライト） ─────────────────────
# 刷新前のツリーで撮影すると自己矛盾のない誤った証跡一式が生成され、下流の
# 誰も気付けない（実装計画 §ステップ0 の罠）。3 マーカーで刷新後サイトである
# ことを機械確認してから撮影に進む。
if ! grep -q 'docs-toc-aside' "$DIST/index.html"; then
  echo "error: stale tree detected — docs-toc-aside (3-column layout) marker not found in $DIST/index.html" >&2
  exit 1
fi
if [ ! -s "$DIST/assets/site.js" ]; then
  echo "error: stale tree detected — assets/site.js (theme toggle) missing or empty" >&2
  exit 1
fi
if [ ! -s "$DIST/assets/search-index.json" ]; then
  echo "error: stale tree detected — assets/search-index.json (full-text search index) missing or empty" >&2
  exit 1
fi
echo "==> preflight OK: refreshed docs site tree confirmed"

# ── ダーク変種の生成（<html lang="ja"> → <html lang="ja" data-theme="dark">） ──
# 置換前後の件数一致を検証し、ライト画像がダーク証跡として混入する完全性
# 破壊を防ぐ（実装計画 §5 A08）。
# dist を配信ルート直下ではなく "$SERVE_*/fandhe-backend/" へネストして置く。
# base_path="/fandhe-backend"（site/nav.toml）のため、ネストしないと全アセットが
# 404 になり無スタイルのショットが撮れてしまう（実装計画の明示的な罠。実際に
# 本チェックで一度踏み、p1〜p5 が全ページ同一ハッシュの 404 応答になっていた）。
rm -rf "$SERVE_LIGHT" "$SERVE_DARK" "$SERVE_NOJS"
mkdir -p "$SERVE_LIGHT$BASE_PATH" "$SERVE_DARK$BASE_PATH" "$SERVE_NOJS$BASE_PATH"
cp -r "$DIST/." "$SERVE_LIGHT$BASE_PATH"
cp -r "$DIST/." "$SERVE_DARK$BASE_PATH"
cp -r "$DIST/." "$SERVE_NOJS$BASE_PATH"

before_count="$(grep -rl '<html lang="ja">' "$SERVE_DARK$BASE_PATH" --include='*.html' | wc -l | tr -d ' ')"
find "$SERVE_DARK$BASE_PATH" -name '*.html' -print0 | xargs -0 sed -i \
  's/<html lang="ja">/<html lang="ja" data-theme="dark">/'
# --include='*.html' に限定する: assets/site.css は data-theme="dark" セレクタを
# 字面として含むため、全ファイル走査だと無関係な一致で件数がずれる
# （実際に本チェックで検出した不具合。site.css の CSS ルールは正当でバグではない）。
after_count="$(grep -rl '<html lang="ja" data-theme="dark">' "$SERVE_DARK$BASE_PATH" --include='*.html' | wc -l | tr -d ' ')"
if [ "$before_count" != "$after_count" ] || [ "$before_count" -eq 0 ]; then
  echo "error: dark variant substitution count mismatch (before=$before_count after=$after_count)" >&2
  exit 1
fi
echo "==> dark variant generated ($after_count page(s))"

# no-JS 相当: CSP script-src 'none' をレスポンスヘッダで配信することで
# --blink-settings=scriptEnabled=false（headless で無音失敗しうる）を使わず
# JS 未到達状態を再現する（実装計画 §ステップ2 N1/N2）。実配信は下記の
# NoJsHTTPRequestHandler が CSP ヘッダを付与する専用サーバで行う。

# ── ステップ 2: サーバ起動（TOCTOU 対策付きポート探索） ────────────────
# start_server_on_free_port: 候補ポートへ実サーバを起動し、ss の pid フィールドで
# 「自分が bind した」ことを確認する。並列イシュー実行中のポート衝突に効く
# （実装計画 §ステップ1）。
# NOTE: この関数は呼び出し側で `port="$(start_server_on_free_port ...)"` の
# ようにコマンド置換経由で呼ばれる。コマンド置換はサブシェルで実行されるため、
# 関数内で SERVER_PIDS（親シェルの配列）へ追記しても親シェルには反映されず、
# EXIT トラップの cleanup が起動したサーバを kill できない不具合があった
# （イシュー #399 PR #413 Bugbot 指摘 1）。PID は `.pid-$mode-$port` ファイルへ
# 書き出す形を維持し、SERVER_PIDS への追記は呼び出し側（コマンド置換の外）で
# 同ファイルを読み直して行う。
start_server_on_free_port() {
  local root="$1"
  local mode="$2" # "plain" or "nojs"
  local attempt=0
  local port
  local pid
  while [ "$attempt" -lt "$PORT_ATTEMPTS" ]; do
    port=$((20000 + RANDOM % 20000))
    (
      cd "$root"
      if [ "$mode" = "nojs" ]; then
        python3 "$REPO_ROOT/scripts/docs-site-visual-nojs-server.py" "$port" \
          >"$LOGS_DIR/server-$mode-$port.log" 2>&1 &
      else
        python3 -m http.server --bind 127.0.0.1 "$port" \
          >"$LOGS_DIR/server-$mode-$port.log" 2>&1 &
      fi
      echo $! >"$LOGS_DIR/.pid-$mode-$port"
    )
    sleep 0.3
    pid="$(cat "$LOGS_DIR/.pid-$mode-$port" 2>/dev/null || true)"
    if [ -n "$pid" ] && kill -0 "$pid" >/dev/null 2>&1; then
      if COLUMNS=1000 ss -ltnp 2>/dev/null | grep ":$port " | grep -q "pid=$pid,"; then
        echo "$port"
        return 0
      fi
    fi
    kill "$pid" >/dev/null 2>&1 || true
    attempt=$((attempt + 1))
    sleep 2
  done
  echo "error: could not bind a free port after $PORT_ATTEMPTS attempts" >&2
  return 1
}

# register_server_pid: start_server_on_free_port が成功後に書き出した
# `.pid-$mode-$port` ファイルを親シェル（コマンド置換の外）で読み直し、
# 親シェルの SERVER_PIDS へ追記する。EXIT トラップの cleanup が実際にこの
# PID を kill できるようにするための橋渡し。
register_server_pid() {
  local mode="$1" port="$2"
  local pid
  pid="$(cat "$LOGS_DIR/.pid-$mode-$port" 2>/dev/null || true)"
  if [ -n "$pid" ]; then
    SERVER_PIDS+=("$pid")
  fi
}

LIGHT_PORT="$(start_server_on_free_port "$SERVE_LIGHT" plain)"
register_server_pid plain "$LIGHT_PORT"
DARK_PORT="$(start_server_on_free_port "$SERVE_DARK" plain)"
register_server_pid plain "$DARK_PORT"
NOJS_PORT="$(start_server_on_free_port "$SERVE_NOJS" nojs)"
register_server_pid nojs "$NOJS_PORT"
echo "==> servers up: light=127.0.0.1:$LIGHT_PORT dark=127.0.0.1:$DARK_PORT nojs=127.0.0.1:$NOJS_PORT"

# ── ステップ 3: 撮影ヘルパー ──────────────────────────────────────────
TOTAL_SHOTS=0

shoot() {
  local name="$1" port="$2" path="$3" width="$4" height="$5"
  local url="http://127.0.0.1:$port$BASE_PATH$path"
  local file="$SHOTS_DIR/$name.png"
  TOTAL_SHOTS=$((TOTAL_SHOTS + 1))
  if [ "$TOTAL_SHOTS" -gt "$MAX_SHOTS" ]; then
    echo "error: shot budget exceeded ($MAX_SHOTS)" >&2
    exit 1
  fi
  "$CHROMIUM_BIN" --headless --disable-gpu --no-sandbox \
    --hide-scrollbars=false \
    --window-size="${width},${height}" \
    --screenshot="$file" \
    --virtual-time-budget=4000 \
    "$url" >"$LOGS_DIR/shot-$name.log" 2>&1 || true

  if [ ! -s "$file" ]; then
    echo "error: screenshot failed or empty: $name ($url)" >&2
    exit 1
  fi

  local bytes sha
  bytes="$(stat -c%s "$file")"
  sha="$(sha256sum "$file" | awk '{print $1}')"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "shots/$name.png" "$url" "$width" "$height" "$6" "$7" "$bytes" "$sha" >>"$MANIFEST"
}

# ── ステップ 4: 撮影マトリクス（実装計画 §ステップ2、容量バジェット 3.5MiB 順守のため
#    トリミング順位（実装計画 §ステップ4: (1) S2 → (2) P3/P5 の dark → (3) P1 の
#    1024 幅 dark）に従い一部の dark/375 変種を既定で間引く。受け入れ条件 1
#    （主要ページ × light/dark × 広幅/狭幅、各 4 枚以上）は P1 単独で
#    3 breakpoint × light/dark の 6 枚で literal に充足するため、他ページの
#    間引きで条件を割らない） ───────────────────────────────────────────
# P1: トップ（3 breakpoint × light/dark、フル）
shoot p1-top-1440-light "$LIGHT_PORT" "/" 1440 900 light js
shoot p1-top-1440-dark  "$DARK_PORT"  "/" 1440 900 dark  js
shoot p1-top-1024-light "$LIGHT_PORT" "/" 1024 768 light js
shoot p1-top-1024-dark  "$DARK_PORT"  "/" 1024 768 dark  js
shoot p1-top-375-light  "$LIGHT_PORT" "/" 375 812 light js
shoot p1-top-375-dark   "$DARK_PORT"  "/" 375 812 dark  js

# P2: ガイド索引（1440 は light のみ、375 は light/dark 両方）
shoot p2-guides-1440-light "$LIGHT_PORT" "/guides/" 1440 900 light js
shoot p2-guides-375-light  "$LIGHT_PORT" "/guides/" 375 812 light js
shoot p2-guides-375-dark   "$DARK_PORT"  "/guides/" 375 812 dark  js

# P3: ガイド本文（コードフェンス折り返し・横スクロール確認、tall window。
#     375 は light のみ、1440 は light/dark）
shoot p3-streaming-1440-light "$LIGHT_PORT" "/guides/streaming/" 1440 1150 light js
shoot p3-streaming-1440-dark  "$DARK_PORT"  "/guides/streaming/" 1440 1150 dark  js
shoot p3-streaming-375-light  "$LIGHT_PORT" "/guides/streaming/" 375 1150 light js

# P4: API 本文（表 + コード、tall window。375 は light のみ、1440 は light/dark）
shoot p4-http-api-1440-light "$LIGHT_PORT" "/api/http-api/" 1440 1150 light js
shoot p4-http-api-1440-dark  "$DARK_PORT"  "/api/http-api/" 1440 1150 dark  js
shoot p4-http-api-375-light  "$LIGHT_PORT" "/api/http-api/" 375 1150 light js

# P5: Examples 索引（1440 のみ、容量バジェット順守のためのトリム。dark・375 は
# P1/P2/P3/P4 で同一コンポーネント（ヘッダー・サイドバー・カード状レイアウト）を
# 既に light/dark 双方で検証済みのため、証跡の限界としてレポートに明記する）
shoot p5-examples-1440-light "$LIGHT_PORT" "/examples/" 1440 900 light js

# N1: no-JS トップ（CSP script-src 'none' 配信、light のみ）
shoot n1-nojs-top-1440 "$NOJS_PORT" "/" 1440 900 light nojs
shoot n1-nojs-top-375  "$NOJS_PORT" "/" 375 812 light nojs

# N2: no-JS API 本文（狭幅でのサイドバー到達性）
# N2 は狭幅でのサイドバー到達性（チェックボックスハック）の確認が主眼のため、
# 表・コードフェンスを含む本文全体を写す必要はなく短い高さで十分
# （容量バジェット順守のための追加トリム）。
shoot n2-nojs-http-api-375 "$NOJS_PORT" "/api/http-api/" 375 700 light nojs

echo "==> captured $TOTAL_SHOTS screenshot(s) (Tier 1)"

# ── S2（Tier 2、失敗許容・既定 off）: 検索結果ドロップダウン ────────────
# 使い捨ての配信コピーにのみクエリ注入ハーネスを挿入する（crates/docs-site の
# ソースは一切変更しない、実装計画 §5 A03）。固定リテラルクエリのみを扱う。
# 容量バジェット（3.5MiB）内に収めるため既定では実行しない
# （実装計画 §ステップ4 のトリミング順位 (1) が S2）。
# `DOCS_SITE_VISUAL_TIER2=1` を指定した場合のみ実行する。
if [ "${DOCS_SITE_VISUAL_TIER2:-0}" != "1" ]; then
  echo "==> S2 (search results, Tier 2) skipped by default (set DOCS_SITE_VISUAL_TIER2=1 to attempt)"
  echo "==> done: $TOTAL_SHOTS shot(s)"
  TOTAL_BYTES="$(find "$SHOTS_DIR" -name '*.png' -printf '%s\n' | awk '{s+=$1} END {print s+0}')"
  if [ "$TOTAL_BYTES" -gt "$MAX_TOTAL_BYTES" ]; then
    echo "error: total screenshot size $TOTAL_BYTES bytes exceeds budget $MAX_TOTAL_BYTES bytes" >&2
    exit 1
  fi
  echo "    output: $OUT_DIR"
  echo "    manifest: $MANIFEST"
  exit 0
fi

S2_ROOT="$OUT_DIR/serve-search"
rm -rf "$S2_ROOT"
mkdir -p "$S2_ROOT$BASE_PATH"
cp -r "$DIST/." "$S2_ROOT$BASE_PATH"
S2_HARNESS='<script>
// 固定リテラルクエリのみ。利用者入力・外部値を混ぜない。
window.addEventListener("load", function () {
  var i = document.querySelector(".docs-search-input");
  if (i) {
    i.value = "router";
    i.dispatchEvent(new Event("input"));
  }
});
</script>
</body>'
S2_OK=1
if [ -f "$S2_ROOT$BASE_PATH/index.html" ]; then
  if ! python3 - "$S2_ROOT$BASE_PATH/index.html" "$S2_HARNESS" <<'PYEOF'
import sys
path, harness = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as f:
    content = f.read()
if "</body>" not in content:
    sys.exit(1)
content = content.replace("</body>", harness, 1)
with open(path, "w", encoding="utf-8") as f:
    f.write(content)
PYEOF
  then
    S2_OK=0
  fi
else
  S2_OK=0
fi

# S2 の dark 用配信ツリー: SERVE_DARK 生成時と同じ
# `<html lang="ja">` → `<html lang="ja" data-theme="dark">` 置換を S2_ROOT の
# コピーに適用する。誤って light-only の S2_ROOT を dark 変種としても撮影して
# いた不具合（イシュー #399 PR #413 Bugbot 指摘 2）の修正。
S2_DARK_ROOT="$OUT_DIR/serve-search-dark"
if [ "$S2_OK" = "1" ]; then
  rm -rf "$S2_DARK_ROOT"
  cp -r "$S2_ROOT" "$S2_DARK_ROOT"
  s2_dark_before="$(grep -rl '<html lang="ja">' "$S2_DARK_ROOT$BASE_PATH" --include='*.html' | wc -l | tr -d ' ')"
  find "$S2_DARK_ROOT$BASE_PATH" -name '*.html' -print0 | xargs -0 sed -i \
    's/<html lang="ja">/<html lang="ja" data-theme="dark">/'
  s2_dark_after="$(grep -rl '<html lang="ja" data-theme="dark">' "$S2_DARK_ROOT$BASE_PATH" --include='*.html' | wc -l | tr -d ' ')"
  if [ "$s2_dark_before" != "$s2_dark_after" ] || [ "$s2_dark_before" -eq 0 ]; then
    echo "warning: S2 (search results, Tier 2) dark variant substitution count mismatch (before=$s2_dark_before after=$s2_dark_after) — skipped" >&2
    S2_OK=0
  fi
fi

if [ "$S2_OK" = "1" ]; then
  S2_PORT="$(start_server_on_free_port "$S2_ROOT" plain || echo "")"
  if [ -n "$S2_PORT" ]; then register_server_pid plain "$S2_PORT"; fi
  S2_DARK_PORT="$(start_server_on_free_port "$S2_DARK_ROOT" plain || echo "")"
  if [ -n "$S2_DARK_PORT" ]; then register_server_pid plain "$S2_DARK_PORT"; fi
  if [ -n "$S2_PORT" ] && [ -n "$S2_DARK_PORT" ]; then
    S2_URL_LIGHT="http://127.0.0.1:$S2_PORT$BASE_PATH/"
    S2_URL_DARK="http://127.0.0.1:$S2_DARK_PORT$BASE_PATH/"
    for variant in light dark; do
      file="$SHOTS_DIR/s2-search-results-$variant.png"
      if [ "$variant" = "dark" ]; then
        url="$S2_URL_DARK"
      else
        url="$S2_URL_LIGHT"
      fi
      "$CHROMIUM_BIN" --headless --disable-gpu --no-sandbox \
        --window-size=1440,900 \
        --screenshot="$file" \
        --virtual-time-budget=5000 \
        "$url" >"$LOGS_DIR/shot-s2-$variant.log" 2>&1 || true
      if [ -s "$file" ]; then
        bytes="$(stat -c%s "$file")"
        sha="$(sha256sum "$file" | awk '{print $1}')"
        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
          "shots/s2-search-results-$variant.png" "$url" 1440 900 "$variant" harness "$bytes" "$sha" >>"$MANIFEST"
        TOTAL_SHOTS=$((TOTAL_SHOTS + 1))
      else
        echo "warning: S2 (search results, Tier 2) capture failed for $variant — omitted from manifest (see docs/acceptance report 検証の限界)" >&2
        rm -f "$file"
      fi
    done
  else
    echo "warning: S2 (search results, Tier 2) server did not bind — skipped" >&2
  fi
else
  echo "warning: S2 (search results, Tier 2) harness injection failed — skipped" >&2
fi

# ── ステップ 5: バジェット検証 ───────────────────────────────────────
if [ "$TOTAL_SHOTS" -gt "$MAX_SHOTS" ]; then
  echo "error: total shot count $TOTAL_SHOTS exceeds budget $MAX_SHOTS" >&2
  exit 1
fi
TOTAL_BYTES="$(find "$SHOTS_DIR" -name '*.png' -printf '%s\n' | awk '{s+=$1} END {print s+0}')"
if [ "$TOTAL_BYTES" -gt "$MAX_TOTAL_BYTES" ]; then
  echo "error: total screenshot size $TOTAL_BYTES bytes exceeds budget $MAX_TOTAL_BYTES bytes" >&2
  exit 1
fi

echo "==> done: $TOTAL_SHOTS shot(s), $TOTAL_BYTES bytes total"
echo "    output: $OUT_DIR"
echo "    manifest: $MANIFEST"
