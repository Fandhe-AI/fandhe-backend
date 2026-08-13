#!/usr/bin/env bash
# 外部 CPU 占有率プローブ（イシュー #613）。
#
# このライブラリの役割:
#   `benches/bench-http.sh`（`CPU_PROBE=1` opt-in）・`benches/lib/interleave.sh`
#   （交互測定エンジン）が、各計測窓（oha 実行区間）の直前直後に `/proc/stat`
#   の総 jiffies とサーバ / oha それぞれの帰属 jiffies を採取し、窓内で計測対象
#   （サーバ・oha）以外が消費した CPU 比率（外部占有率）を算出するための関数群を
#   提供する。共有ホスト・共有テナンシー環境（ホステッド runner の noisy
#   neighbor を含む）で、他プロセスの負荷混入により計測窓が汚染されたことを
#   検知し、窓単位の再計測（有界）につなげる（背景・実証は
#   `benches/reports/issue593-p1-zero-copy-bench.md` 9 節・9.7 節、
#   `docs/design/bench-hosted-runner.md`）。
#
# 呼び出し元: `benches/bench-http.sh`・`benches/lib/interleave.sh` が
# `source "$(dirname "${BASH_SOURCE[0]}")/lib/cpu-probe.sh"` で読み込む。
# セルフテスト: `scripts/tests/run-bench-cpu-probe-tests.sh` が本ファイルのみを
# source し、`FANDHE_BACKEND_PROC_STAT` フィクスチャ・フェイクプロセスで
# 判定ロジックを検証する（`benches/lib/exclusive.sh` と同じ「副作用のある
# 呼び出し元本体は対象にしない」オフラインテスト方針を踏襲）。
#
# 単体では実行しない（関数定義のみ、副作用なし）。

# NOTE: `exclusive.sh` と同じ理由で `set -e` は付けない（source 専用ライブラリ、
# 呼び出し元の独自エラーハンドリングを壊さないため）。
set -uo pipefail

# 外部占有率のしきい値（%）。超過した窓を「汚染」とみなす。#616 較正ラン
# （固定 ref・同一コミット 797245a5 で mode=primary × 5・mode=pair × 2）では
# 汚染検知・再計測とも発火実績ゼロのため実測検証はできておらず、fail-closed
# 原則により #612 5.2 節の暫定値のまま維持（値の変更なし・確定扱いしない。
# 発火事例の蓄積または発火検証の実施をもって確定判断する。詳細は
# `benches/reports/issue616-hosted-runner-calibration.md` 11 節参照）。
EXT_CPU_MAX_PCT="${EXT_CPU_MAX_PCT:-5}"
# 汚染窓 1 個あたりの再計測回数上限（有界、無限リトライを防ぐフェイルクローズ設計）。
# `benches/reports/issue593-p1-zero-copy-bench.md` 9.7 節の実証値を既定とする。
WINDOW_REMEASURE_MAX="${WINDOW_REMEASURE_MAX:-2}"

# しきい値 env の数値検証（`benches/lib/common.sh` の `validate_numeric` と同型・
# 独立実装。exclusive.sh の `_nfr6_validate_numeric` と同じく、本ファイルは
# common.sh との同時 source を前提にしないため単体で完結させる）。
# 引数: $1 値 $2 変数名。不正なら非 0 終了で即座に落とす（invalid env でサイレントに
# 既定値へフォールバックしない）。
_cpu_probe_validate_numeric() {
    local value="$1" name="$2"
    if ! [[ "${value}" =~ ^[0-9]+([.][0-9]+)?$ ]]; then
        echo "エラー: ${name} は数値である必要があります（現在: ${value}）" >&2
        exit 1
    fi
}
_cpu_probe_validate_numeric "${EXT_CPU_MAX_PCT}" "EXT_CPU_MAX_PCT"
if ! [[ "${WINDOW_REMEASURE_MAX}" =~ ^[0-9]+$ ]]; then
    echo "エラー: WINDOW_REMEASURE_MAX は 0 以上の整数である必要があります（現在: ${WINDOW_REMEASURE_MAX}）" >&2
    exit 1
fi

# `/proc/stat` の参照先。テストからフィクスチャに差し替え可能にするためのフック
# （`FANDHE_BACKEND_PROC_LOADAVG` と同パターン、既定は本番挙動を変えない）。
: "${FANDHE_BACKEND_PROC_STAT:=/proc/stat}"

# `/proc/stat` 先頭行（集計 CPU 行）から総 jiffies と busy jiffies を読む。
#
# 標準出力: "<total> <busy>"（空白区切り、いずれも整数 jiffies）。
# busy = user+nice+system+irq+softirq+steal（`steal` を含める根拠は
# `docs/design/bench-hosted-runner.md` 7 節引き渡し: ホステッド VM 上では
# ハイパーバイザに奪われた CPU 時間 `steal` も「このプロセス以外が使った」
# 時間として外部占有率へ自然に算入すべきため）。total = busy + idle + iowait。
# guest/guest_nice は user に既に含まれるため加算しない（Linux カーネルの慣例）。
#
# 読み取り失敗（ファイル不在・フォーマット不正）時は空文字を返す（呼び出し元は
# 空を「計測不能 = 汚染扱い」としてフェイルクローズで処理する契約）。
probe_read_total_jiffies() {
    local line
    line="$(head -n 1 "${FANDHE_BACKEND_PROC_STAT}" 2>/dev/null || true)"
    # 先頭トークンが "cpu"（集計行）であることを確認する。
    case "${line}" in
        cpu\ *) ;;
        *)
            echo ""
            return 1
            ;;
    esac
    # NOTE: awk 変数名に `system` は使わない（awk 組み込み関数 `system()` と
    # 衝突し構文エラーになるため、`sys` に変更）。
    LC_NUMERIC=C awk '
        {
            user=$2; nice=$3; sys=$4; idle=$5; iowait=$6; irq=$7; softirq=$8; steal=$9
            for (i = 2; i <= 9; i++) { if ($i !~ /^[0-9]+$/) { exit 1 } }
            busy = user + nice + sys + irq + softirq + steal
            total = busy + idle + iowait
            printf "%d %d\n", total, busy
        }
    ' <<<"${line}" || { echo ""; return 1; }
}

# `/proc/<pid>/stat` から proc(5) の指定フィールド 2 個を読み取る内部ヘルパ。
#
# 引数: $1 PID $2 awk フィールド番号（1 起点、")" 以降を state=1 として数え直した
#       位置。utime=12, stime=13, cutime=14, cstime=15）$3 同上（2 個目）
# 標準出力: "<field_a> <field_b>"（空白区切り整数）、読み取り不能なら空文字。
#
# `/proc/<pid>/stat` の第 2 フィールド（comm）は `(name with spaces)` を含みうる
# ため、最後の ")" 以降を state 起点として awk で切り出す（空白分割の脆さを避ける、
# man proc(5) 推奨パターン）。`split(str, arr, " ")` は awk の特殊挙動（リテラル
# " " を FS に渡すと既定 FS と同じく先頭・連続空白を読み飛ばす）により、
# comm 除去後の文字列は f[1]=state（proc 第 3 フィールド）から詰めて格納される。
# 以降は `proc 元の番号 - 2` の位置に来る（例: utime は元 14 番目 → f[12]、
# cutime は元 16 番目 → f[14]）。実装時に `/proc/<bash-pid>/stat` を実際に読んで
# perl の既知 CPU 消費量と突き合わせ、この対応関係を実測検証済み。
_cpu_probe_read_pid_fields() {
    local pid="$1" field_a="$2" field_b="$3"
    local stat_path="/proc/${pid}/stat"
    if [ -n "${FANDHE_BACKEND_PROC_PID_STAT_DIR:-}" ]; then
        # テスト専用フック: フィクスチャディレクトリ配下の "<pid>" ファイルを参照する
        # （実 PID を持たない decoy stat ファイルでプロセス消滅ケースを再現するため）。
        stat_path="${FANDHE_BACKEND_PROC_PID_STAT_DIR}/${pid}"
    fi
    if [ ! -r "${stat_path}" ]; then
        echo ""
        return 1
    fi
    awk -v fa="${field_a}" -v fb="${field_b}" '
        {
            n = split($0, rest, ")")
            after = rest[n]
            for (i = n - 1; i >= 2; i--) { after = rest[i] ")" after }
            split(after, f, " ")
            a = f[fa]; b = f[fb]
            if (a !~ /^[0-9]+$/ || b !~ /^[0-9]+$/) { exit 1 }
            printf "%d %d\n", a, b
        }
    ' "${stat_path}" 2>/dev/null || { echo ""; return 1; }
}

# 指定 PID の累積 CPU jiffies（utime+stime、そのプロセス自身の消費）を読む。
#
# 引数: $1 PID
# 標準出力: jiffies 整数、読み取り不能（プロセス消滅・権限不足等）なら空文字。
# コマンドライン（argv）は読まない・記録しない（`list_busy_process_names` と同じ
# 情報漏えい対策方針、.claude/rules/security.md）。
probe_read_pid_jiffies() {
    local pid="$1"
    local pair
    pair="$(_cpu_probe_read_pid_fields "${pid}" 12 13)" || { echo ""; return 1; }
    if [ -z "${pair}" ]; then
        echo ""
        return 1
    fi
    LC_NUMERIC=C awk '{ printf "%d\n", $1 + $2 }' <<<"${pair}"
}

# 呼び出し元シェル自身（`$$`）の待機済み子プロセス累積 CPU jiffies
# （`/proc/self/stat` の cutime+cstime、proc(5)）を読む。
#
# oha は bench-http.sh / lib/interleave.sh から `json="$(oha ...)"`
# のようなコマンド置換で同期実行される（`for` ループ内で毎回 wait 完了する）ため、
# 呼び出し元シェルの cutime+cstime は「これまでに完了した子プロセス（oha を含む）が
# 消費した CPU の累積」を正確に反映する。bash 組み込み `times` は環境ロケール・
# バージョン依存の文字列フォーマットで扱いにくいため使わず、同じ情報を持つ
# `/proc/self/stat` を直接読む（`/proc/stat` と同一の jiffies 単位で比較でき、
# パースも一本化できる。実測検証: `benches/reports/` 参照なし、実装時に
# perl 300M ループを command substitution 経由で実行し cutime の増分が壁時計と
# 整合することを確認済み）。
#
# 前提: oha 呼び出しの直近の親（`bench-http.sh` 自身のシェル）から呼ぶこと。
# 別 PID 経由（サブシェル内呼び出し等）だと cutime/cstime の起点が変わるため、
# 呼び出し元は同一シェルプロセス内で before/after を一貫して呼ぶこと。
probe_read_self_children_jiffies() {
    local pid="$$"
    local pair
    pair="$(_cpu_probe_read_pid_fields "${pid}" 14 15)" || { echo ""; return 1; }
    if [ -z "${pair}" ]; then
        echo ""
        return 1
    fi
    LC_NUMERIC=C awk '{ printf "%d\n", $1 + $2 }' <<<"${pair}"
}

# 増分から外部占有率（%）を算出する純関数。
#
# 引数: $1 total_before $2 total_after $3 busy_before $4 busy_after
#       $5 attributed_before $6 attributed_after
#       （attributed = サーバ jiffies + oha 帰属 jiffies の合計。呼び出し元が
#       窓前後でそれぞれ合算して渡す）
# 標準出力: 外部占有率（%、小数）。分母（総増分）が 0 以下、または入力に
# 空文字・非数値が混入している場合は "nan" を返す（フェイルクローズ、
# 呼び出し元は "nan" を汚染扱いにする契約）。
#
# 算出式: 外部占有率 = (busy 増分 − 帰属増分) / 総増分 * 100
# （#611 実証プロトコルそのまま。帰属増分が busy 増分を超える場合は 0% に
# クランプする — 計測誤差で帰属側がわずかに busy を上回るケースを異常値
# として扱わないため）。
probe_external_share() {
    local total_before="$1" total_after="$2" busy_before="$3" busy_after="$4"
    local attributed_before="$5" attributed_after="$6"
    for v in "${total_before}" "${total_after}" "${busy_before}" "${busy_after}" "${attributed_before}" "${attributed_after}"; do
        if ! [[ "${v}" =~ ^[0-9]+$ ]]; then
            echo "nan"
            return 0
        fi
    done
    LC_NUMERIC=C awk -v tb="${total_before}" -v ta="${total_after}" \
        -v bb="${busy_before}" -v ba="${busy_after}" \
        -v ab="${attributed_before}" -v aa="${attributed_after}" '
        BEGIN {
            total_delta = ta - tb
            busy_delta = ba - bb
            attributed_delta = aa - ab
            if (total_delta <= 0) { print "nan"; exit }
            external_delta = busy_delta - attributed_delta
            if (external_delta < 0) { external_delta = 0 }
            printf "%.4f\n", (external_delta / total_delta) * 100
        }
    '
}

# 外部占有率がしきい値を超えているか（= 汚染窓か）を判定する。
#
# 引数: $1 外部占有率（%、`probe_external_share` の出力。"nan" 可）
#       $2 しきい値（%、省略時 `EXT_CPU_MAX_PCT`）
# 戻り値: 0 = 汚染（超過）, 1 = 非汚染
# "nan"（計測不能）はフェイルクローズで汚染扱いにする（PASS へ丸めない）。
probe_is_contaminated() {
    local share="$1"
    local threshold="${2:-${EXT_CPU_MAX_PCT}}"
    if [ "${share}" = "nan" ]; then
        return 0
    fi
    if ! [[ "${share}" =~ ^-?[0-9]+([.][0-9]+)?$ ]]; then
        return 0
    fi
    LC_NUMERIC=C awk -v s="${share}" -v t="${threshold}" 'BEGIN { exit !(s > t) }'
}

export EXT_CPU_MAX_PCT WINDOW_REMEASURE_MAX
