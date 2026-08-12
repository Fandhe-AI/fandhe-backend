#!/usr/bin/env bash
# benches/*.sh 共通関数（TASK-1.2）。
#
# このライブラリの役割:
#   - サーバプロセスの起動・停止・trap による確実な回収
#   - 前提ツール（oha・jq）の存在検査（スクリプトが勝手にバイナリを取得しない）
#   - 複数回計測 (RUNS) の中央値算出（平均値ではなく中央値を採用する根拠は
#     benches/README.md を参照。PoC-2 で run 間に外れ値が発生した実例への是正）
#
# 呼び出し元: bench-http.sh / bench-rss.sh / bench-footprint.sh が
# `source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"` で読み込む。
# 単体では実行しない（関数定義のみで副作用を持たない）。

set -euo pipefail

# 計測パラメータ（env で上書き可能）。値はすべて後続コマンドでクォートして展開し、
# eval は使わない（コマンドインジェクション防止、.claude/rules/security.md）。
RUNS="${RUNS:-5}"
DURATION="${DURATION:-15s}"
CONNECTIONS="${CONNECTIONS:-128}"

# common.sh を呼ぶ各スクリプトから見て workspace ルート相対で解決できるよう、
# 呼び出し元スクリプトの1階層上（benches/ の親）を基準にする。
WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# 実効 target ディレクトリ（cargo build の実際の出力先）を導出する（イシュー #480）。
#
# 背景: 週次ベンチ（bench-schedule.yml）で `cargo build --release` 成功直後に
# `${WORKSPACE_ROOT}/target/release/axum-ref` が「見つからない」事象が発生した。
# 調査の結果、self-hosted runner フリートは同一 org（Fandhe-AI）の他リポジトリ
# （fandhe-frontend）の CI が明示的に前提としているとおり、ホスト共有の
# `CARGO_TARGET_DIR=/cargo-target` をジョブに注入する構成であることが濃厚（詳細・
# 一次証拠は benches/reports/issue480-target-dir-investigation.md）。この場合
# `cargo build` の実際の成果物は `${WORKSPACE_ROOT}/target` ではなく
# `${CARGO_TARGET_DIR}` 配下に生成され、`${WORKSPACE_ROOT}/target/release/axum-ref`
# 決め打ちのパスには最初から存在しない（「消失」ではなく「そもそも生成されていない」）。
#
# 導出順序（優先度順。副作用・コストの低い順に評価し、`set -euo pipefail` 下で
# source されるこのファイルが cargo/jq 不在時や manifest 未解決時に落ちないよう
# 各段を安全側にフォールバックする）:
#   1. `CARGO_TARGET_DIR` env が非空ならそれをそのまま使う（相対パス指定時は
#      WORKSPACE_ROOT 基準で絶対化する。cargo 自身の解釈と同じ）。観測された
#      失敗の直接原因はこの env 注入のため、追加プロセス起動なしで最短経路にする。
#   2. 上記が未設定なら `cargo metadata --no-deps` の `target_directory`
#      （`CARGO_TARGET_DIR` env・`.cargo/config.toml` の `build.target-dir` の
#      両方を正しく反映する cargo 自身の権威値）を使う。ただし `Cargo.lock` は
#      gitignore 対象で常に存在するとは限らず、`cargo metadata` はロックファイル
#      未解決時にネットワークアクセス・書き込みを伴いうるため、専有ロック保持中の
#      呼び出し元（bench-accept-exclusive.sh）で不用意に走らせないよう 1. で
#      先に短絡させる。失敗時（cargo/jq 不在・manifest 未解決等）は例外を
#      呼び出し元に伝播させず空文字にフォールバックする。
#   3. いずれも得られない場合は従来どおり `${WORKSPACE_ROOT}/target`。
#
# 注意: ここでは `CARGO_TARGET_DIR` そのものを export しない（cargo の実際の
# ビルド挙動を暗黙に変えないため）。各スクリプトのバイナリパス既定値の算出にのみ
# `BENCH_TARGET_DIR` を使う（read-only な参照値）。
_bench_derive_target_dir() {
    if [ -n "${CARGO_TARGET_DIR:-}" ]; then
        case "${CARGO_TARGET_DIR}" in
            /*) printf '%s\n' "${CARGO_TARGET_DIR}" ;;
            *) printf '%s\n' "${WORKSPACE_ROOT}/${CARGO_TARGET_DIR}" ;;
        esac
        return 0
    fi
    if command -v cargo >/dev/null 2>&1 && command -v jq >/dev/null 2>&1; then
        local dir
        dir="$(cargo metadata --format-version 1 --no-deps --manifest-path "${WORKSPACE_ROOT}/Cargo.toml" 2>/dev/null \
            | jq -r '.target_directory // empty' 2>/dev/null || true)"
        if [ -n "${dir}" ]; then
            printf '%s\n' "${dir}"
            return 0
        fi
    fi
    printf '%s\n' "${WORKSPACE_ROOT}/target"
}
BENCH_TARGET_DIR="${BENCH_TARGET_DIR:-$(_bench_derive_target_dir)}"

# 計測対象。既定は axum-ref だが、TASK-1.6 でフルスクラッチコアに差し替えて
# 同一スクリプトを再利用できるようにする。既定値は BENCH_TARGET_DIR（実効
# target ディレクトリ）からの絶対パスに解決し、カレントディレクトリにも
# 「ビルドが `${WORKSPACE_ROOT}/target` へ出力される」という前提にも依存しない
# （TARGET_BIN を環境変数で明示的に上書きした場合はその値をそのまま使う）。
TARGET_BIN="${TARGET_BIN:-${BENCH_TARGET_DIR}/release/axum-ref}"
TARGET_HOST="${TARGET_HOST:-127.0.0.1}"
TARGET_PORT="${TARGET_PORT:-3001}"
TARGET_URL="${TARGET_URL:-http://${TARGET_HOST}:${TARGET_PORT}}"

SERVER_PID=""

# 前提ツール（oha・jq・curl）の存在検査。
#
# 契約: 見つからなければ導入手順を案内して非 0 終了する。
# スクリプトが外部バイナリを自動ダウンロードすることはない（サプライチェーン考慮）。
# curl は wait_for_health が全スクリプト共通で使用するため、未導入時に原因不明の
# タイムアウトへ誤誘導しないようここで検査する。
check_dependencies() {
    local missing=0
    if ! command -v jq >/dev/null 2>&1; then
        echo "エラー: jq が見つかりません。導入してください（例: apt install jq）" >&2
        missing=1
    fi
    if ! command -v oha >/dev/null 2>&1; then
        echo "エラー: oha が見つかりません。導入してください（例: cargo install oha）" >&2
        missing=1
    fi
    if ! command -v curl >/dev/null 2>&1; then
        echo "エラー: curl が見つかりません。導入してください（例: apt install curl）" >&2
        missing=1
    fi
    if [ "${missing}" -ne 0 ]; then
        exit 1
    fi
}

# RUNS が最低 3 回であることを検証する（中央値評価の前提、README 参照）。
check_runs_minimum() {
    if [ "${RUNS}" -lt 3 ]; then
        echo "エラー: RUNS は最低 3 回必要です（現在: ${RUNS}）。中央値評価の前提を満たせません" >&2
        exit 1
    fi
}

# release バイナリを起動し、SERVER_PID にプロセス ID を記録する。
#
# 契約: 呼び出し元は `trap stop_server EXIT` を必ず設定すること。
# サーバは既定で 127.0.0.1 のみにバインドする（axum-ref 側の既定と同じ、外部公開しない）。
start_server() {
    if [ ! -x "${TARGET_BIN}" ]; then
        echo "エラー: ${TARGET_BIN} が見つかりません。先に 'cargo build --release' を実行してください" >&2
        exit 1
    fi
    # TARGET_URL は TARGET_HOST/TARGET_PORT と独立に上書きできるが、起動するサーバは
    # 常に TARGET_HOST:TARGET_PORT にバインドする。両者が食い違うと wait_for_health・
    # oha が実際に起動したプロセスと異なる宛先を叩き、原因不明のタイムアウトや
    # 誤ったサーバの計測につながるため、ここで整合性を検証する
    # （TARGET_URL を明示的に上書きしていない既定構成では常に一致する）。
    if [ "${TARGET_URL}" != "http://${TARGET_HOST}:${TARGET_PORT}" ]; then
        echo "エラー: TARGET_URL（${TARGET_URL}）が TARGET_HOST:TARGET_PORT（${TARGET_HOST}:${TARGET_PORT}）と一致しません。" >&2
        echo "        起動するサーバは TARGET_HOST:TARGET_PORT にバインドされるため、TARGET_URL は指定しないか同じ値に揃えてください" >&2
        exit 1
    fi
    BIND_ADDR="${TARGET_HOST}:${TARGET_PORT}" "${TARGET_BIN}" &
    SERVER_PID="$!"
}

# サーバプロセスを確実に停止する。trap から呼ばれる（プロセス残留防止）。
stop_server() {
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
}

# /health への応答が返るまでポーリングする（起動完了検知）。
#
# 引数: $1 最大待機ミリ秒数（既定 5000）。ポーリング間隔は 5ms。
# 標準出力: 起動完了までに要したミリ秒数（起動時間計測にそのまま使える）。
wait_for_health() {
    local timeout_ms="${1:-5000}"
    local elapsed_ms=0
    local interval_ms=5
    while [ "${elapsed_ms}" -lt "${timeout_ms}" ]; do
        if curl -s -o /dev/null -w '%{http_code}' "${TARGET_URL}/health" 2>/dev/null | grep -q '^200$'; then
            echo "${elapsed_ms}"
            return 0
        fi
        # LC_NUMERIC=C を明示: カンマを小数点区切りに使うロケール環境下では awk の出力が
        # "0,005" のような形式になり sleep への引数として不正になる（Bugbot 指摘）。
        # ロケールに関わらず "." 区切りを保証するため C ロケールを固定する。
        sleep "$(LC_NUMERIC=C awk "BEGIN { print ${interval_ms} / 1000 }")"
        elapsed_ms=$((elapsed_ms + interval_ms))
    done
    echo "エラー: ${TARGET_URL}/health が ${timeout_ms}ms 以内に応答しませんでした" >&2
    return 1
}

# 標準入力（改行区切りの数値列）の中央値を算出する。
#
# 偶数個の場合は中央 2 値の平均を採る。PoC-2 の外れ値事例（POST /echo の
# p99 が 3 回中 1 回だけ 13.5ms、他は 4.3ms・1.0ms）を踏まえ、平均値ではなく
# 中央値を標準の評価指標として採用する（README 参照）。
median() {
    # LC_NUMERIC=C を明示: 偶数個の場合の平均値算出でカンマ小数点ロケールの影響を受け、
    # 呼び出し元の数値比較・算術展開が壊れるのを防ぐ（wait_for_health と同根の Bugbot 指摘）。
    LC_NUMERIC=C sort -n | LC_NUMERIC=C awk '
        { values[NR] = $1 }
        END {
            if (NR == 0) { exit 1 }
            if (NR % 2 == 1) {
                printf "%s\n", values[(NR + 1) / 2]
            } else {
                printf "%s\n", (values[NR / 2] + values[NR / 2 + 1]) / 2
            }
        }
    '
}

# 改行区切りの数値列（bash 配列を `printf '%s\n' "${arr[@]}"` で渡す想定）を
# JSON 数値配列に変換する。RESULT_JSON 出力（機械可読）を組み立てる際の共通変換。
#
# 呼び出し元: bench-http.sh / bench-rss.sh / bench-footprint.sh。
# 空行は除外するため、末尾に改行が付いていても壊れない。
to_json_array() {
    jq -R -s 'split("\n") | map(select(length > 0) | tonumber)'
}

# `RESULT_JSON=<path>` が指定されている場合のみ、渡された JSON 文字列をそのまま
# ファイルへ書き出す（機械可読出力）。人間可読な既存 stdout 形式は変更しない
# —— bench-accept.sh（TASK-1.6-1）が比較・閾値判定のために stdout テキストを
# パースせずに済むよう、機械可読出力を RESULT_JSON 経由に分離する契約。
# 未指定時は no-op のため、既存呼び出し元（RESULT_JSON を渡さない従来利用）の
# 後方互換性を保つ。
write_result_json() {
    local json="$1"
    if [ -n "${RESULT_JSON:-}" ]; then
        printf '%s\n' "${json}" | jq '.' >"${RESULT_JSON}"
    fi
}

# 数値形式（整数または小数）であることを検証する。bench-accept.sh が env 経由で
# 受け取る閾値パラメータを awk の算術式・比較に渡す前段で使用し、想定外の文字列が
# シェル展開・awk プログラムに混入するのを防ぐ（インジェクション対策、
# .claude/rules/security.md）。
validate_numeric() {
    local value="$1"
    local name="$2"
    if ! [[ "${value}" =~ ^[0-9]+([.][0-9]+)?$ ]]; then
        echo "エラー: ${name} は数値である必要があります（現在: ${value}）" >&2
        exit 1
    fi
}

# 非負整数（小数不可）であることを検証する。`validate_numeric` は小数を許容する
# ため、bash の整数算術コンテキスト（`$((...))`・`-ge`/`-lt` 等）へそのまま渡す
# 値の検証には使えない（小数を渡すと `integer expression expected` で異常終了し、
# 呼び出し元が意図した BLOCKED/エラー終了コードの契約を通らずに落ちてしまう）。
# `SECTION_QUIESCE_WAIT_SECS`（`benches/lib/exclusive.sh` の `wait_for_quiescence`
# が `SECONDS`/`sleep` と共に整数算術で使用）等、整数専用パラメータの検証に使う。
validate_integer() {
    local value="$1"
    local name="$2"
    if ! [[ "${value}" =~ ^[0-9]+$ ]]; then
        echo "エラー: ${name} は非負整数である必要があります（現在: ${value}）" >&2
        exit 1
    fi
}

export RUNS DURATION CONNECTIONS TARGET_BIN TARGET_HOST TARGET_PORT TARGET_URL WORKSPACE_ROOT BENCH_TARGET_DIR
