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
#
# `uptime` の末尾表記は Linux が「load average: 0.1, 0.2, 0.3」、macOS が
# 「load averages: 0.1 0.2 0.3」と異なる（複数形 + カンマなし）。旧実装は
# `-F'load average'` で分割した第 2 フィールドの先頭語を取るため、macOS では
# 「s:」という非数値が返り、`check_quiescence_once` がフェイルクローズで永遠に
# BUSY と判定して静穏確認が成立しなかった。区切りを「load averages?:」の正規
# 表現にして両表記から 1 分値のみを取り出す。
#
# `FANDHE_BACKEND_PROC_LOADAVG` は `/proc/loadavg` の参照先を差し替えるための
# テスト専用フック（既定は `/proc/loadavg` のままで本番挙動は変わらない）。
# 本番環境（Linux self-hosted runner）では常に `/proc/loadavg` が読めるため
# `uptime` 分岐へ実際には到達せず、`scripts/tests/run-nfr6-exclusive-tests.sh`
# は本フックで存在しないパスを指定して意図的に `uptime` 分岐を通し、上記の
# 表記差分解析ロジック本体を直接検証する（#274 レビュー指摘: 既存テストは
# 全ケースで `get_loadavg1` をモック関数に再定義しており、解析ロジック本体を
# 検証する回帰テストが存在しなかった）。
get_loadavg1() {
    local proc_loadavg="${FANDHE_BACKEND_PROC_LOADAVG:-/proc/loadavg}"
    if [ -r "${proc_loadavg}" ]; then
        cut -d' ' -f1 "${proc_loadavg}"
    else
        uptime | awk '{ sub(/.*load averages?: */, ""); sub(/,/, ""); print $1 }'
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

# 単発 FAIL を退行と誤認しないための限定再試行ラッパー（イシュー #285、
# `benches/bench-accept-exclusive.sh` の定期実行化で導入）。
#
# 背景: `benches/reports/task-1.6-1-performance.md` の申し送りどおり、初回計測が
# keep-alive 再接続ノイズ等で FAIL → 再実行 PASS と振れた実績がある。専有ロック
# （flock）・静穏確認だけでは「計測開始直後の一過性ノイズ」までは吸収できないため、
# 呼び出し元コマンドが終了コード 1（FAIL）を返した場合に限り、同一ロック保持中に
# 静穏確認をやり直してから、指定された残り再試行回数だけ再実行する。
#
# 終了コード 0（PASS）・2（BLOCKED）はノイズ起因ではなく確定的な結果として扱い、
# 再試行しない（0 を再試行すると偶然の 1 回 PASS を過大評価しうる。2 は計測環境
# 自体が壊れているため再試行しても無意味、フェイルクローズで即座に BLOCKED を返す）。
#
# 呼び出し対象コマンド側の契約（イシュー #479）: 終了コード 1（FAIL）は
# **非決定的な計測 FAIL 専用**とする。baseline バイナリ欠如・依存ツール欠如等の
# 決定論的な環境失敗を exit 1 で返すと、本関数がノイズと誤認して無意味な静穏待機
# （最大 `QUIESCE_WAIT_SECS`）を挟んだ再試行を行い、`bench-accept.sh` の
# 追記型レポート生成（`write_report_conclusion`）が複数回呼ばれて同一文言の
# 「## 結論」セクションが REPORT_MD へ重複追記される（#476 で実証）。呼び出し
# 対象コマンドは決定論的な環境失敗を `FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE`
# （2、BLOCKED）で返す契約とし、本関数はそれをそのまま再試行せず返す
# （#478 で baseline 欠如の exit コードを 1 → 2 へ統一し実害を解消、本イシューで
# 契約をここに明文化）。
#
# 引数:
#   $1          残り再試行回数（0 以上の整数。呼び出し元の `FAIL_RETRIES` をそのまま渡す。
#               `FAIL_RETRIES=N` を指定すると、FAIL が続く限り最大 N 回まで再試行する
#               ループとして扱う。PR #291 Bugbot 指摘対応: 旧実装は再試行ループを持たず
#               1 回目の再試行結果を無条件に最終結果としていたため、N=2 以上を指定しても
#               常に 1 回しか再試行されなかった）
#   $2 以降     実行するコマンドとその引数（例: bash bench-accept.sh ...）
# 標準エラー出力: 再試行の発生有無を人間可読ログとして出す（呼び出し元がログへ
# 転記しやすいよう終了コードとは独立に記録する）。
# 戻り値: 最終試行のコマンド終了コードをそのまま返す（PASS/FAIL の意味は呼び出し元
# スクリプトの doc comment から変えない）。ただし再試行前の静穏確認自体が
# `QUIESCE_WAIT_SECS` 待っても得られなかった場合は、直前の FAIL 結果をそのまま
# 返さず `FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE`（BLOCKED）を返す（PR #291 Bugbot
# 指摘対応: 初回の静穏待機失敗は `bench-accept-exclusive.sh` 側で BLOCKED として
# 扱われるのに対し、旧実装は再試行前の静穏待機失敗だけ FAIL のまま返しており、
# 「ホストが混雑しているだけ」のケースを性能退行として誤検知しうる非対称があった）。
#
# 呼び出し元: `benches/bench-accept-exclusive.sh`（`FAIL_RETRIES` 既定 0 で導入前と
# 同一挙動を維持し、週次 schedule ワークフロー（`.github/workflows/bench-schedule.yml`）
# からは `FAIL_RETRIES=1` を指定する）。
# セルフテスト: `scripts/tests/run-nfr6-exclusive-tests.sh` が `wait_for_quiescence` /
# `snapshot_environment` をモック化した上で本関数のみを検証する
# （既存の「副作用のある呼び出し元本体は対象にしない」方針を踏襲）。
nfr6_run_with_fail_retry() {
    local retries_left="$1"
    shift
    if ! [[ "${retries_left}" =~ ^[0-9]+$ ]]; then
        echo "エラー: 再試行回数は 0 以上の整数である必要があります（現在: ${retries_left}）" >&2
        return 1
    fi

    "$@"
    local status=$?

    # PASS（0）・BLOCKED（2）は再試行しない。FAIL（1）のみが再試行対象。
    # FAIL が続く限り、残り再試行回数が尽きるまでループする。
    #
    # BLOCKED（2）への到達を検知した直後にここで観測ログを出す想定だったが、
    # 初回呼び出し直後の 1 箇所だけに置くと「1 回目 FAIL → 2 回目以降の再試行で
    # BLOCKED へ遷移する」ケースでログが出ない（ループ内で status が更新されても
    # このチェックを再度通らないため）。BLOCKED は初回・再試行ループ内のどちらでも
    # 発生しうるため、判定はループの後（脱出直後）に一本化する（イシュー #479）。
    while [ "${status}" -eq 1 ] && [ "${retries_left}" -gt 0 ]; do
        echo "FAIL（終了コード 1）を検知。単発ノイズの可能性があるため、静穏確認をやり直して再試行します（残り再試行回数: ${retries_left} → $((retries_left - 1))）" >&2
        retries_left=$((retries_left - 1))

        if ! wait_for_quiescence; then
            echo "エラー: 再試行前の静穏確認が ${QUIESCE_WAIT_SECS}s 待っても得られませんでした。ホスト混雑等で計測不能と判断し BLOCKED として扱います（直前の FAIL 結果は採用しない）" >&2
            return "${FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE}"
        fi
        snapshot_environment retry >&2

        "$@"
        status=$?
    done

    if [ "${status}" -eq 1 ]; then
        echo "再試行をすべて使い切っても FAIL（終了コード 1）。退行として確定します" >&2
    fi

    # 上記の呼び出し対象コマンド側契約により、決定論的な環境失敗を意味する
    # BLOCKED（2）に到達した場合はここで観測ログを出す（週次 run のログから
    # 「契約どおり再試行がスキップされた／打ち切られた」ことを追跡可能にする。
    # 初回呼び出し直後・再試行ループ内のどちらで BLOCKED へ遷移しても本行を
    # 必ず通るため、両ケースを 1 箇所で観測できる。終了コード・戻り値契約自体は
    # 変えない、イシュー #479）。
    if [ "${status}" -eq "${FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE}" ]; then
        echo "BLOCKED（終了コード ${FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE}）のため再試行しません（決定論的失敗は再試行対象外）" >&2
    fi

    return "${status}"
}

export FANDHE_BACKEND_NFR6_LOCK LOAD1_MAX QUIESCE_WAIT_SECS QUIESCE_POLL_INTERVAL_SECS FANDHE_BACKEND_NFR6_BLOCKED_EXIT_CODE
