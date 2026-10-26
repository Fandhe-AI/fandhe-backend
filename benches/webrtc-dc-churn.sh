#!/usr/bin/env bash
# イシュー #887: 上限超過で拒否されるデータチャネルの大量開閉時に、server プロセスの RSS が
# 有界に収まるかを長時間採取する（情報提供用。PASS/FAIL 判定は持たず CI にも組み込まない）。
#
# 構成: `crates/plugin-webrtc/examples/datachannel_churn.rs` の server 役を起動し、その RSS を
# `ps` で定期採取しながら client 役で「開く → 拒否される → 閉じる」を繰り返す。client 終了後
# （PC close 後）も採取を続け、増加が接続の寿命で区切られるかを見る。
#
# 使い方:
#   bash benches/webrtc-dc-churn.sh                  # 前景で実行（短時間の動作確認向け）
#   bash benches/webrtc-dc-churn.sh --detach         # 切り離して起動し OUT_DIR を表示
#   bash benches/webrtc-dc-churn.sh --wait DIR SEC   # DIR の完了を最長 SEC 秒待ち summary.txt を表示
#
# 環境変数（任意）:
#   OUT_DIR             出力先（未指定は mktemp -d）。rss.csv / client.csv / summary.txt / done
#   SAMPLE_INTERVAL_SEC RSS 採取間隔（既定 5）
#   IDLE_SEC            churn 前の待機時 RSS 採取秒数（既定 15）
#   MAX_CHANNELS        server の同時データチャネル数上限（既定 1）
#   DURATION_SEC / CONCURRENCY / LABEL_BYTES / REPORT_INTERVAL_SEC / CLOSE_PC_AT_END /
#   POST_CLOSE_IDLE_SEC は client 役へそのまま渡る（既定は example の doc 参照）
#
# 結果は benches/reports/issue887-webrtc-dc-churn.md に転記する。計測の一時出力は転記後に削除する。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SAMPLE_INTERVAL_SEC="${SAMPLE_INTERVAL_SEC:-5}"
IDLE_SEC="${IDLE_SEC:-15}"
MAX_CHANNELS="${MAX_CHANNELS:-1}"

# sleep を使わない有界待機（fifo の read -t）。
nap() {
  local f
  f="$(mktemp -u "${TMPDIR:-/tmp}/dcchurn-nap.XXXXXX")"
  mkfifo "$f"
  exec 9<>"$f"
  rm -f "$f"
  read -r -t "$1" -u 9 _ || true
  exec 9>&-
}

wait_mode() {
  local dir="$1" max="$2" waited=0
  while [ ! -f "$dir/done" ] && [ "$waited" -lt "$max" ]; do
    nap 5
    waited=$((waited + 5))
  done
  if [ -f "$dir/done" ]; then
    cat "$dir/summary.txt"
  else
    echo "実行中: $dir"
    tail -n 2 "$dir/rss.csv" 2>/dev/null || true
    tail -n 2 "$dir/client.csv" 2>/dev/null || true
  fi
}

summarize() {
  local dir="$1"
  awk -F, -v idle_end="$(cat "$dir/t_client_start")" -v close_at="$(cat "$dir/t_close")" '
    $1 < idle_end { n_idle++; idle += $2; next }
    $1 <= close_at {
      n++; t[n] = $1; r[n] = $2
    }
    $1 > close_at { m++; post[m] = $2 }
    END {
      if (n_idle) printf "idle_rss_kb(avg)=%d\n", idle / n_idle
      if (n) {
        printf "churn_start_rss_kb=%d\nchurn_end_rss_kb=%d\n", r[1], r[n]
        # 中央値
        for (i = 1; i <= n; i++) s[i] = r[i]
        for (i = 1; i <= n; i++) for (j = i + 1; j <= n; j++) if (s[j] < s[i]) { x = s[i]; s[i] = s[j]; s[j] = x }
        printf "churn_median_rss_kb=%d\n", s[int((n + 1) / 2)]
        # 後半半分の最小二乗傾き（KiB/分）
        from = int(n / 2) + 1; k = 0; sx = sy = sxx = sxy = 0
        for (i = from; i <= n; i++) { k++; x = (t[i] - t[from]) / 60; sx += x; sy += r[i]; sxx += x * x; sxy += x * r[i] }
        den = k * sxx - sx * sx
        if (k > 1 && den != 0) printf "second_half_slope_kb_per_min=%.1f\n", (k * sxy - sx * sy) / den
      }
      if (m) {
        mn = post[1]; last = post[m]
        for (i = 1; i <= m; i++) if (post[i] < mn) mn = post[i]
        printf "post_close_first_rss_kb=%d\npost_close_min_rss_kb=%d\npost_close_last_rss_kb=%d\n", post[1], mn, last
      }
    }' "$dir/rss.csv"
  grep -E '^[0-9]+,' "$dir/client.csv" | sed 's/^[0-9]*,//' | grep -E '^[0-9]+,[0-9]+,' | tail -n 1 |
    awk -F, '{ printf "cycles=%s\ncreate_errors=%s\ntimeouts=%s\nfirst_echo_ok(last)=%s\nstate(last)=%s\nclient_dc_stats(last)=%s\n", $2, $3, $4, $5, $6, $7 }'
}

run_mode() {
  local dir="${OUT_DIR:-$(mktemp -d)}"
  mkdir -p "$dir"
  echo "OUT_DIR=$dir"
  cd "$REPO_ROOT"
  cargo build --release -p fandhe-backend-plugin-webrtc --example datachannel_churn >"$dir/build.log" 2>&1
  local bin="$REPO_ROOT/target/release/examples/datachannel_churn"
  if [ -n "${CARGO_TARGET_DIR:-}" ]; then bin="$CARGO_TARGET_DIR/release/examples/datachannel_churn"; fi

  {
    echo "date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "uname=$(uname -srm)"
    echo "rustc=$(rustc --version)"
    echo "uptime=$(uptime)"
    echo "max_channels=$MAX_CHANNELS"
    echo "DURATION_SEC=${DURATION_SEC:-600} CONCURRENCY=${CONCURRENCY:-4} LABEL_BYTES=${LABEL_BYTES:-8}"
  } >"$dir/env.txt"

  local fifo="$dir/stdin.fifo"
  mkfifo "$fifo"
  FANDHE_DC_CHURN_MAX_CHANNELS="$MAX_CHANNELS" "$bin" server <"$fifo" >"$dir/server.out" 2>"$dir/server.err" &
  local server_wrapper=$!
  exec 5>"$fifo"
  local pid="" addr="" i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    if grep -q '^PID=' "$dir/server.out" 2>/dev/null; then break; fi
    nap 1
  done
  pid="$(sed -n 's/^PID=\([0-9]*\) ADDR=.*/\1/p' "$dir/server.out" | head -n 1)"
  addr="$(sed -n 's/^PID=[0-9]* ADDR=\(.*\)/\1/p' "$dir/server.out" | head -n 1)"
  [ -n "$pid" ] && [ -n "$addr" ] || { echo "server が起動しなかった" >&2; kill "$server_wrapper" 2>/dev/null || true; exit 1; }
  trap 'exec 5>&- 2>/dev/null || true; kill "$server_wrapper" 2>/dev/null || true' EXIT

  : >"$dir/rss.csv"
  (
    while kill -0 "$pid" 2>/dev/null && [ ! -f "$dir/stop_sampler" ]; do
      rss="$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ')"
      [ -n "$rss" ] && echo "$(date +%s),$rss" >>"$dir/rss.csv"
      nap "$SAMPLE_INTERVAL_SEC"
    done
  ) &
  local sampler=$!

  nap "$IDLE_SEC"
  date +%s >"$dir/t_client_start"
  : >"$dir/client.csv"
  # client の標準出力へ行ごとに epoch を付ける（PC_CLOSED の時刻を close 境界に使う）。
  "$bin" client "$addr" 2>"$dir/client.err" | while IFS= read -r line; do
    echo "$(date +%s),$line" >>"$dir/client.csv"
  done || true
  local t_close
  t_close="$(grep ',PC_CLOSED$' "$dir/client.csv" | head -n 1 | cut -d, -f1 || true)"
  [ -n "$t_close" ] || t_close="$(date +%s)"
  echo "$t_close" >"$dir/t_close"

  touch "$dir/stop_sampler"
  wait "$sampler" 2>/dev/null || true
  exec 5>&-
  wait "$server_wrapper" 2>/dev/null || true
  trap - EXIT

  { cat "$dir/env.txt"; echo "---"; summarize "$dir"; } >"$dir/summary.txt"
  touch "$dir/done"
  cat "$dir/summary.txt"
}

case "${1:-}" in
  --detach)
    dir="${OUT_DIR:-$(mktemp -d)}"
    mkdir -p "$dir"
    OUT_DIR="$dir" nohup bash "$0" >"$dir/run.log" 2>&1 </dev/null &
    echo "OUT_DIR=$dir"
    ;;
  --wait)
    wait_mode "${2:?usage: --wait DIR SEC}" "${3:-540}"
    ;;
  "")
    run_mode
    ;;
  *)
    echo "usage: $0 [--detach | --wait DIR SEC]" >&2
    exit 2
    ;;
esac
