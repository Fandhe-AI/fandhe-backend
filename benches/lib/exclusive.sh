#!/usr/bin/env bash
# NFR-6（docs/spec/04-requirements.md）専有計測用の共通関数（TASK-15.4 相当 / #178）。
#
# このライブラリの役割:
#   `benches/{webrtc,graphql,hub}-nfr6-bench.sh` 単体は「同一マシン上の他負荷」を
#   考慮しない。並列 issue 実装ワークフロー下では host contention により
#   RPS 比が大きく振れ、REQ-5/REQ-8/REQ-9 の NFR-6 判定が確定できなかった
#   （`benches/reports/task-9.5-hub-wiring-performance.md` 診断:
#   同一バイナリ・同一マシンでも計測タイミングだけで RPS が約 4.4 倍変動）。
#   本ライブラリは (1) flock によるホストグローバル相互排他、(2) 静穏
#   （quiescence）確認、(3) 環境スナップショット出力、の 3 機能を提供し、
#   `benches/nfr6-exclusive.sh` から利用される「専有実行枠」の土台となる。
#
# ポート動的採番について（設計判断、docs/design/nfr6-exclusive-measurement.md 参照）:
#   計画時点ではポートの動的採番も検討したが、対象 example
#   （`crates/core/examples/{minimal,webrtc_nfr6,graphql_nfr6}.rs` ・
#   `crates/plugin-hub-wiring/examples/hub_link_only.rs`）はいずれも
#   `server.bind("127.0.0.1:<port>")` を Rust コード中にハードコードしており、
#   env 経由のポート上書きに対応していない。本イシューは `crates/**` を
#   変更しない方針のため、ポート動的採番は見送り、flock による直列化
#   （同時に 1 計測のみ）と静穏確認を専有性の担保手段とする
#   （見送りの理由・別イシュー化の要否は README・設計ドキュメントに記録）。
#
# 呼び出し元: `benches/nfr6-exclusive.sh` が
# `source "$(dirname "${BASH_SOURCE[0]}")/lib/exclusive.sh"` で読み込む。
# セルフテスト: `scripts/tests/run-nfr6-exclusive-tests.sh` が本ファイルのみを
# source し、`get_loadavg1` / `list_busy_process_names` を再定義してモック化した
# 上で判定ロジックを検証する（`benches/lib/common.sh` 系の既存オフラインテスト
# 方針・`scripts/accept/lib/nfr6-ratio.sh` と同じ「副作用のある呼び出し元本体は
# 対象にしない」方針を踏襲）。
#
# 単体では実行しない（関数定義のみ、副作用なし）。

# NOTE: `set -e` は付けない。本ファイルは source される前提であり、`-e` を
# 有効化すると呼び出し元スクリプトの独自エラーハンドリング（trap・戻り値判定）を
# 壊す可能性があるため（`benches/lib/common.sh` は独立実行される前提のスクリプトの
# 冒頭で `set -euo pipefail` するが、本ファイルは source 専用のライブラリとして
# 挙動を分ける）。
set -uo pipefail

# 専有ロックファイルの既定パス。env で上書き可能（並列 worktree でロック対象を
# 分離したい場合や、`/tmp` 以外の専有ディレクトリを使いたい場合に指定する）。
FANDHE_BACKEND_NFR6_LOCK="${FANDHE_BACKEND_NFR6_LOCK:-/tmp/fandhe-backend-nfr6-bench.lock}"
# 静穏とみなす 1 分 loadavg の上限（nproc に対する絶対値ではなく固定閾値。
# 既定 1.0 は「ほぼアイドル」を意味する保守的な値。env で上書き可能）。
LOAD1_MAX="${LOAD1_MAX:-1.0}"
# 静穏待機の上限秒数。超過時は BLOCKED（フェイルクローズ、PASS へ丸めない）。
QUIESCE_WAIT_SECS="${QUIESCE_WAIT_SECS:-1800}"
# 静穏ポーリング間隔秒数。
QUIESCE_POLL_INTERVAL_SECS="${QUIESCE_POLL_INTERVAL_SECS:-30}"

# `wait_for_quiescence` / `acquire_exclusive_lock` が静穏未達・ロック取得不能で
# 諦めた場合に呼び出し元が使うべき終了コード（BLOCKED、PASS/FAIL とは別区分）。
FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE=2

# 数値検証（`benches/lib/common.sh` の `validate_numeric` と同型・独立実装）。
# exclusive.sh は常に common.sh と同時に source される前提を置かないため、
# 依存を増やさず単体で完結させる。
_nfr6_validate_numeric() {
    local value="$1" name="$2"
    if ! [[ "${value}" =~ ^[0-9]+([.][0-9]+)?$ ]]; then
        echo "エラー: ${name} は数値である必要があります（現在: ${value}）" >&2
        return 1
    fi
    return 0
}

# 1 分 loadavg を取得する。`/proc/loadavg`（Linux）優先、フォールバックで `uptime`。
# テストから差し替え可能にするため独立関数に切り出す
# （セルフテストは本関数を再定義して固定値を返させ、静穏判定の境界値を検証する）。
get_loadavg1() {
    if [ -r /proc/loadavg ]; then
        cut -d' ' -f1 /proc/loadavg
    else
        uptime | awk -F'load average' '{ print $2 }' | tr -d ',' | awk '{ print $1 }'
    fi
}

# 自プロセス以外で稼働中の cargo/rustc/oha プロセス名を検出する（計測に影響しうる
# 他ジョブのビルド・負荷生成の検出）。
# 標準出力: 検出したプロセス名を改行区切りで返す（検出なしなら空）。
# コマンドライン引数は記録しない（.claude/rules/security.md「情報漏えい」対策。
# argv にトークン等が混入していてもここには一切現れない）。
# テストから差し替え可能にするため独立関数に切り出す。
list_busy_process_names() {
    local self_pid=$$
    local name pid
    for name in cargo rustc oha; do
        # pgrep -x: 完全一致のみ（部分一致による過検知を避ける）。
        # 見つからない場合 pgrep は非 0 を返すため || true で握りつぶす
        # （`set -e` 相当の途中終了を避ける。本関数は source 先で -e が
        # 有効化されていても安全に動くよう明示的に握りつぶす）。
        while read -r pid; do
            [ -z "${pid}" ] && continue
            [ "${pid}" = "${self_pid}" ] && continue
            echo "${name}"
        done < <(pgrep -x "${name}" 2>/dev/null || true)
    done
}

# 現時点の静穏可否を判定する。
# 標準出力: "QUIESCENT" | "BUSY"（人間可読な理由ログは呼び出し元が出す）
check_quiescence_once() {
    local load1
    load1="$(get_loadavg1)"
    if ! _nfr6_validate_numeric "${load1}" "loadavg"; then
        # loadavg が数値として取得できない = 前提が壊れているとみなし BUSY 扱い
        # （フェイルクローズ。QUIESCENT へ丸めない）。
        echo "BUSY"
        return 0
    fi
    if ! LC_NUMERIC=C awk -v v="${load1}" -v max="${LOAD1_MAX}" 'BEGIN { exit !(v <= max) }'; then
        echo "BUSY"
        return 0
    fi
    local busy
    busy="$(list_busy_process_names)"
    if [ -n "${busy}" ]; then
        echo "BUSY"
    else
        echo "QUIESCENT"
    fi
    return 0
}

# 静穏が得られるまで `QUIESCE_WAIT_SECS` を上限にポーリングする。
# 戻り値: 0 = 静穏取得済み、1 = BLOCKED（呼び出し元は
# `FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE` で終了し、試行記録を正直に残すこと。
# 待機を無限にしない・強制バイパスのフラグは設けない）。
#
# `QUIESCE_POLL_INTERVAL_SECS` が `0`（または不正な非数値）だと `sleep 0` を
# 繰り返して待機が進まず、ループが無限に BLOCKED を返せなくなる（有界待機の
# 契約に反する）。ポーリング間隔は 1 秒にクランプする。
#
# 有界性・待機時間の担保は経過時間（bash 組み込み変数 `SECONDS`、シェル起動
# からの経過秒数）そのもので判定する（PR #193 Bugbot 指摘の修正）。旧実装は
# ループ回数（`QUIESCE_WAIT_SECS` を実効間隔で割った上限回数）で終了条件を
# 判定しており、判定→sleep の順で「N 回目の判定失敗で BLOCKED」としていた
# ため、実際の壁時計待機時間が `QUIESCE_WAIT_SECS` より約 1 ポーリング間隔分
# 短くなり（`QUIESCE_POLL_INTERVAL_SECS` がそれ未満だと即座に BLOCKED になり
# 得た）、「秒単位で待つ」という契約に反していた。本実装は起点からの経過秒数
# が `QUIESCE_WAIT_SECS` に達するまでは BLOCKED と判定せず、残り時間を超えない
# 範囲でのみ sleep することで、有界性を保ったまま実際の待機時間を保証する。
wait_for_quiescence() {
    local effective_interval="${QUIESCE_POLL_INTERVAL_SECS}"
    if ! [[ "${effective_interval}" =~ ^[0-9]+$ ]] || [ "${effective_interval}" -lt 1 ]; then
        effective_interval=1
    fi
    local start_seconds="${SECONDS}"
    while true; do
        if [ "$(check_quiescence_once)" = "QUIESCENT" ]; then
            return 0
        fi
        local elapsed=$((SECONDS - start_seconds))
        if [ "${elapsed}" -ge "${QUIESCE_WAIT_SECS}" ]; then
            return 1
        fi
        local remaining=$((QUIESCE_WAIT_SECS - elapsed))
        local sleep_secs="${effective_interval}"
        [ "${sleep_secs}" -gt "${remaining}" ] && sleep_secs="${remaining}"
        sleep "${sleep_secs}"
    done
}

# `_NFR6_LOCK_FD` に紐づけたロックファイル記述子。`acquire_exclusive_lock` /
# `release_exclusive_lock` の対で使う（呼び出し元プロセス内でのみ有効）。
_NFR6_LOCK_FD=9

# ホストグローバルな専有ロックを取得する（並列 worktree の同時計測を防ぐ）。
# 引数: $1 ロックファイルパス（省略時 `FANDHE_BACKEND_NFR6_LOCK`）
# 戻り値: 0 = 取得成功、1 = 取得不能（symlink 拒否 or タイムアウト。BLOCKED 扱い）
#
# セキュリティ: ロックパスが symlink の場合は使用を拒否する（world-writable な
# `/tmp` 配下での symlink squat 対策、.claude/rules/security.md）。ロックファイルへ
# データを書き込むことはない（flock 待ちが発生するのみで情報漏えい・上書き破壊は
# 起きない設計）。
acquire_exclusive_lock() {
    local lockpath="${1:-${FANDHE_BACKEND_NFR6_LOCK}}"
    if [ -L "${lockpath}" ]; then
        echo "エラー: ロックパス ${lockpath} が symlink です。使用を拒否します" >&2
        return 1
    fi
    # shellcheck disable=SC2261 # eval によるファイルディスクリプタの動的オープンは
    # bash の制約上必要（変数化した FD 番号へ `exec N>path` するには eval が要る）。
    eval "exec ${_NFR6_LOCK_FD}>\"${lockpath}\"" 2>/dev/null || {
        echo "エラー: ロックファイル ${lockpath} を開けません" >&2
        return 1
    }
    # 再度 symlink 化されていないか（TOCTOU 対策の限定的な再検証）。
    if [ -L "${lockpath}" ]; then
        echo "エラー: ロックパス ${lockpath} が symlink に置き換えられました。使用を拒否します" >&2
        return 1
    fi
    if flock -n "${_NFR6_LOCK_FD}"; then
        return 0
    fi
    echo "他の計測プロセスがロックを保持中です（${lockpath}）。最大 ${QUIESCE_WAIT_SECS}s 待機します..." >&2
    if flock -w "${QUIESCE_WAIT_SECS}" "${_NFR6_LOCK_FD}"; then
        return 0
    fi
    echo "エラー: ロック取得タイムアウト（${QUIESCE_WAIT_SECS}s）。BLOCKED として扱います" >&2
    return 1
}

# `acquire_exclusive_lock` で取得したロックを解放する。
release_exclusive_lock() {
    # shellcheck disable=SC2261
    eval "exec ${_NFR6_LOCK_FD}>&-" 2>/dev/null || true
}

# 実行環境スナップショットを machine-readable な `key=value` 行で出力する
# （レポート追補への転記・再現性確保が目的）。
# 引数: $1 ラベル（例 "before" / "after"）
# 出力にプロセスのコマンドライン引数は含めない（`list_busy_process_names` と同じ
# 情報漏えい対策）。
snapshot_environment() {
    local label="${1:-snapshot}"
    local commit
    commit="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
    local nproc_value
    nproc_value="$(nproc 2>/dev/null || echo unknown)"
    local load1
    load1="$(get_loadavg1)"
    local busy
    busy="$(list_busy_process_names | tr '\n' ',' | sed 's/,$//')"
    printf 'snapshot_label=%s\n' "${label}"
    printf 'snapshot_time=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf 'snapshot_commit=%s\n' "${commit}"
    printf 'snapshot_nproc=%s\n' "${nproc_value}"
    printf 'snapshot_loadavg1=%s\n' "${load1}"
    printf 'snapshot_busy_processes=%s\n' "${busy:-none}"
}

export FANDHE_BACKEND_NFR6_LOCK LOAD1_MAX QUIESCE_WAIT_SECS QUIESCE_POLL_INTERVAL_SECS FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE
